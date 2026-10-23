mod drive;
mod state;
mod torrent;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use librqbit::Session;
use tracing::{error, info};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
    #[command(flatten)]
    common: Common,
}

#[derive(Subcommand)]
enum Command {
    /// Authorize access to Google Drive and store the token
    Auth,
}

#[derive(Args, Clone)]
struct Common {
    /// OAuth client file downloaded from Google Cloud Console (Desktop app)
    #[arg(long, global = true, default_value_os_t = config_dir().join("client_secret.json"))]
    client_secret: PathBuf,
    /// Where the Drive token is stored
    #[arg(long, global = true, default_value_os_t = config_dir().join("token.json"))]
    token_file: PathBuf,
    /// Local port for the OAuth redirect during `auth`
    #[arg(long, global = true, default_value_t = 8085)]
    auth_port: u16,
}

#[derive(Args, Clone)]
struct RunArgs {
    /// First episode to look for (ignored once the state file exists)
    #[arg(long, default_value_t = 1181)]
    start: u32,
    #[arg(long, default_value = "/mnt/storagebox/onepiece")]
    download_dir: PathBuf,
    /// Destination folder under My Drive
    #[arg(long, default_value = "One Piece/16 - Wano (891-)")]
    drive_folder: String,
    #[arg(long, default_value_os_t = config_dir().join("state.json"))]
    state_file: PathBuf,
    /// Seconds between checks when the next torrent is not out
    #[arg(long, default_value_t = 600)]
    interval: u64,
    /// Remove local files once uploaded
    #[arg(long)]
    delete_after_upload: bool,
    /// Check once and exit if nothing is available
    #[arg(long)]
    once: bool,
}

fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".opf-watch")
}

#[tokio::main]
async fn main() -> Result<()> {
    // both aws-lc-rs and ring are compiled in, so pick the provider explicitly
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,librqbit=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Auth) => auth(&cli.common).await,
        None => run(&cli.common, cli.run).await,
    }
}

async fn auth(common: &Common) -> Result<()> {
    let hub = drive::connect(&common.client_secret, &common.token_file, common.auth_port).await?;
    drive::authorize(&hub).await?;
    info!("authorized, token stored in {}", common.token_file.display());
    Ok(())
}

async fn run(common: &Common, args: RunArgs) -> Result<()> {
    if !common.token_file.exists() {
        bail!(
            "no Drive token at {}, run `opf-watch-rs auth` first",
            common.token_file.display()
        );
    }
    let hub = drive::connect(&common.client_secret, &common.token_file, common.auth_port).await?;
    let folder_id = drive::ensure_folder(&hub, &args.drive_folder).await?;

    std::fs::create_dir_all(&args.download_dir)?;
    let session = Session::new(args.download_dir.clone())
        .await
        .context("creating torrent session")?;
    let http = reqwest::Client::builder()
        .user_agent("opf-watch-rs")
        .timeout(Duration::from_secs(30))
        .build()?;
    let interval = Duration::from_secs(args.interval);
    let mut episode = state::load_next_episode(&args.state_file).unwrap_or(args.start);

    loop {
        match process(&http, &session, &hub, &folder_id, episode, &args).await {
            Ok(true) => {
                episode += 1;
                state::save_next_episode(&args.state_file, episode)?;
                info!("episode {} done", episode - 1);
            }
            Ok(false) => {
                info!("episode {episode} not available yet");
                if args.once {
                    return Ok(());
                }
                tokio::time::sleep(interval).await;
            }
            Err(e) => {
                // state is not advanced, so the same episode is retried
                error!("episode {episode}: {e:#}, retrying in {}s", args.interval);
                tokio::time::sleep(interval).await;
            }
        }
    }
}

/// Returns false when the torrent is not published yet.
async fn process(
    http: &reqwest::Client,
    session: &Arc<Session>,
    hub: &drive::Hub,
    folder_id: &str,
    episode: u32,
    args: &RunArgs,
) -> Result<bool> {
    let Some(torrent) = torrent::fetch(http, episode).await? else {
        return Ok(false);
    };
    info!("episode {episode} available, downloading");
    let files = torrent::download(session, torrent).await?;
    for file in files {
        drive::upload(hub, &file, folder_id).await?;
        if args.delete_after_upload {
            std::fs::remove_file(&file)?;
        }
    }
    Ok(true)
}
