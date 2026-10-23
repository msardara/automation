use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use librqbit::{AddTorrent, AddTorrentOptions, Session};
use tracing::info;

const URL_TEMPLATE: &str = "https://www.opfitalia.net/tor/{episode}fhd.torrent";

/// Fetch the .torrent file. Returns None when the episode is not published yet.
pub async fn fetch(http: &reqwest::Client, episode: u32) -> Result<Option<Vec<u8>>> {
    let url = URL_TEMPLATE.replace("{episode}", &episode.to_string());
    let resp = http.get(&url).send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::FORBIDDEN {
        return Ok(None);
    }
    let data = resp.error_for_status()?.bytes().await?;
    // a missing file may be served as an HTML page with status 200
    let is_torrent = data.starts_with(b"d") && data.windows(6).any(|w| w == b"4:info");
    Ok(is_torrent.then(|| data.to_vec()))
}

/// Download the torrent and return the absolute paths of its files.
pub async fn download(session: &Arc<Session>, torrent: Vec<u8>) -> Result<Vec<PathBuf>> {
    let response = session
        .add_torrent(
            AddTorrent::from_bytes(torrent),
            Some(AddTorrentOptions {
                // lets an interrupted download resume on top of existing files
                overwrite: true,
                ..Default::default()
            }),
        )
        .await
        .context("adding torrent")?;
    let handle = response.into_handle().context("torrent was not started")?;
    let id = handle.id();

    info!("downloading {}", handle.name().unwrap_or_default());
    handle.wait_until_completed().await?;

    let base = handle.output_folder().to_path_buf();
    let files = handle.with_metadata(|m| {
        m.file_infos
            .iter()
            .map(|f| base.join(&f.relative_filename))
            .collect::<Vec<_>>()
    })?;

    // stop seeding, the files stay on disk
    session.delete(id.into(), false).await?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hits the real site, run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn detects_published_and_missing_torrents() {
        let http = reqwest::Client::new();
        assert!(fetch(&http, 1180).await.unwrap().is_some());
        assert!(fetch(&http, 9999).await.unwrap().is_none());
    }
}
