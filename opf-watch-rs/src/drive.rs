use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use google_drive3::api::File;
use google_drive3::common::{Delegate, Retry};
use google_drive3::hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use google_drive3::hyper_util::client::legacy::connect::HttpConnector;
use google_drive3::hyper_util::client::legacy::Client;
use google_drive3::hyper_util::rt::TokioExecutor;
use google_drive3::yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use google_drive3::yup_oauth2::{
    self, InstalledFlowAuthenticator, InstalledFlowReturnMethod, client::CustomHyperClientBuilder,
};
use google_drive3::DriveHub;
use md5::{Digest, Md5};
use tracing::{info, warn};

pub type Hub = DriveHub<HttpsConnector<HttpConnector>>;

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const SCOPE: &str = "https://www.googleapis.com/auth/drive";
const CHUNK_SIZE: u64 = 32 * 1024 * 1024;
const MAX_RETRIES: u32 = 8;

fn https_connector() -> HttpsConnector<HttpConnector> {
    HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("loading native root certificates")
        .https_only()
        .enable_http2()
        .build()
}

/// Opens the consent page in the default browser, and prints the URL in case that fails.
struct BrowserDelegate;

impl InstalledFlowDelegate for BrowserDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            println!("Opening the browser to authorize Google Drive access.");
            println!("If it does not open, visit:\n\n{url}\n");
            if let Err(e) = open::that_detached(url) {
                warn!("could not open the browser: {e}");
            }
            // the redirect to the local port delivers the code, nothing to type
            Ok(String::new())
        })
    }
}

pub async fn connect(
    secret_path: &Path,
    token_path: &Path,
    redirect_port: u16,
    open_browser: bool,
) -> Result<Hub> {
    let secret = yup_oauth2::read_application_secret(secret_path)
        .await
        .with_context(|| format!("reading OAuth client file {}", secret_path.display()))?;

    let auth_client = Client::builder(TokioExecutor::new()).build(https_connector());
    let builder = InstalledFlowAuthenticator::with_client(
        secret,
        InstalledFlowReturnMethod::HTTPPortRedirect(redirect_port),
        CustomHyperClientBuilder::from(auth_client),
    )
    .persist_tokens_to_disk(token_path);
    let auth = if open_browser {
        builder.flow_delegate(Box::new(BrowserDelegate)).build().await?
    } else {
        builder.build().await?
    };

    let client = Client::builder(TokioExecutor::new()).build(https_connector());
    Ok(DriveHub::new(client, auth))
}

/// Run the consent flow once and store the token. Prints a URL to open in a browser.
pub async fn authorize(hub: &Hub) -> Result<()> {
    hub.files()
        .list()
        .page_size(1)
        .add_scope(SCOPE)
        .doit()
        .await
        .context("listing files to trigger authorization")?;
    Ok(())
}

fn escape(name: &str) -> String {
    name.replace('\\', "\\\\").replace('\'', "\\'")
}

async fn find(hub: &Hub, parent: &str, name: &str, folder: bool) -> Result<Option<File>> {
    let mut query = format!("name = '{}' and '{}' in parents and trashed = false", escape(name), parent);
    let op = if folder { "=" } else { "!=" };
    query.push_str(&format!(" and mimeType {op} '{FOLDER_MIME}'"));
    let (_, list) = hub
        .files()
        .list()
        .q(&query)
        .spaces("drive")
        .supports_all_drives(true)
        .include_items_from_all_drives(true)
        .param("fields", "files(id,name,size,md5Checksum)")
        .add_scope(SCOPE)
        .doit()
        .await?;
    Ok(list.files.and_then(|f| f.into_iter().next()))
}

/// Resolve a folder path like "One Piece/16 - Wano" under My Drive, creating missing folders.
pub async fn ensure_folder(hub: &Hub, path: &str) -> Result<String> {
    let mut parent = "root".to_string();
    for name in path.split('/').filter(|p| !p.is_empty()) {
        parent = match find(hub, &parent, name, true).await? {
            Some(folder) => folder.id.context("folder without id")?,
            None => {
                info!("creating Drive folder {name}");
                let req = File {
                    name: Some(name.to_string()),
                    mime_type: Some(FOLDER_MIME.to_string()),
                    parents: Some(vec![parent]),
                    ..Default::default()
                };
                let (_, created) = hub
                    .files()
                    .create(req)
                    .supports_all_drives(true)
                    .param("fields", "id")
                    .add_scope(SCOPE)
                    .upload(std::io::Cursor::new(Vec::new()), mime_for("").parse()?)
                    .await?;
                created.id.context("created folder without id")?
            }
        };
    }
    Ok(parent)
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "srt" => "application/x-subrip",
        _ => "application/octet-stream",
    }
}

async fn md5_of(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Md5::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })
    .await?
}

/// Retries transient failures with exponential backoff and keeps the resumable session URL.
#[derive(Default)]
struct RetryDelegate {
    attempts: u32,
    upload_url: Option<String>,
}

impl RetryDelegate {
    fn backoff(&mut self) -> Retry {
        self.attempts += 1;
        if self.attempts > MAX_RETRIES {
            return Retry::Abort;
        }
        let secs = (1u64 << self.attempts).min(60);
        warn!("transient Drive error, retry {}/{MAX_RETRIES} in {secs}s", self.attempts);
        Retry::After(Duration::from_secs(secs))
    }
}

impl Delegate for RetryDelegate {
    fn http_error(&mut self, _err: &google_drive3::hyper_util::client::legacy::Error) -> Retry {
        self.backoff()
    }

    fn http_failure(
        &mut self,
        res: &google_drive3::common::Response,
        _err: Option<&serde_json::Value>,
    ) -> Retry {
        let status = res.status();
        if status.is_server_error() || status.as_u16() == 429 {
            self.backoff()
        } else {
            Retry::Abort
        }
    }

    fn upload_url(&mut self) -> Option<String> {
        self.upload_url.clone()
    }

    fn store_upload_url(&mut self, url: Option<&str>) {
        self.upload_url = url.map(str::to_string);
    }

    fn chunk_size(&mut self) -> u64 {
        CHUNK_SIZE
    }
}

/// Upload a file into the given folder, skipping it when an identical copy is already there.
pub async fn upload(hub: &Hub, path: &Path, folder_id: &str) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("file name is not valid UTF-8")?
        .to_string();
    let local_md5 = md5_of(path.to_path_buf()).await?;
    let existing = find(hub, folder_id, &name, false).await?;

    if let Some(file) = &existing
        && file.md5_checksum.as_deref() == Some(local_md5.as_str())
    {
        info!("{name} already on Drive, skipping");
        return Ok(());
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mime = mime_for(ext).parse()?;
    let content = std::fs::File::open(path)?;
    let mut delegate = RetryDelegate::default();

    info!("uploading {name}");
    let (_, uploaded) = match existing.and_then(|f| f.id) {
        Some(id) => {
            hub.files()
                .update(File::default(), &id)
                .delegate(&mut delegate)
                .supports_all_drives(true)
                .param("fields", "id,size,md5Checksum")
                .add_scope(SCOPE)
                .upload_resumable(content, mime)
                .await?
        }
        None => {
            let req = File {
                name: Some(name.clone()),
                parents: Some(vec![folder_id.to_string()]),
                ..Default::default()
            };
            hub.files()
                .create(req)
                .delegate(&mut delegate)
                .supports_all_drives(true)
                .param("fields", "id,size,md5Checksum")
                .add_scope(SCOPE)
                .upload_resumable(content, mime)
                .await?
        }
    };

    if uploaded.md5_checksum.as_deref() != Some(local_md5.as_str()) {
        bail!("checksum mismatch after uploading {name}");
    }
    info!("{name} uploaded and verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_quotes_and_backslashes() {
        assert_eq!(escape(r"it's a\b"), r"it\'s a\\b");
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime_for("mp4"), "video/mp4");
        assert_eq!(mime_for("xyz"), "application/octet-stream");
    }

    #[tokio::test]
    async fn md5_matches_known_digest() {
        let path = std::env::temp_dir().join("opf-watch-md5-test");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(md5_of(path).await.unwrap(), "5d41402abc4b2a76b9719d911017c592");
    }
}
