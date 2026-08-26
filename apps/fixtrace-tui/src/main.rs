use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Parser;
use fixtrace::{
    application::{AppServiceOptions, FixTraceAppService},
    history::paths::StatePaths,
};
use fixtrace_client::{AppClient, InProcessClient, WebSocketClient};
use fixtrace_protocol::{ClientCapabilities, InitializeRequest, PROTOCOL_VERSION};
use fixtrace_server::WriterLock;
use fixtrace_tui::{ConnectionMode, Model};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(about = "FixTrace terminal UI", version)]
struct Args {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    session: Option<Uuid>,
    #[arg(long)]
    connect: Option<String>,
    #[arg(long)]
    token_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "fixtrace_tui=info".into()),
        )
        .init();
    let args = Args::parse();
    let cancellation = CancellationToken::new();
    let mut writer_lock = None;
    let (client, connection, info): (Arc<dyn AppClient>, ConnectionMode, _) =
        if let Some(url) = args.connect.clone() {
            let paths = StatePaths::discover(args.state_dir.clone())?;
            let token_file = args
                .token_file
                .unwrap_or_else(|| paths.database.with_file_name("app-server.token"));
            let token = read_token(&token_file)?;
            let client = Arc::new(WebSocketClient::new(&url, token)?);
            (
                client,
                ConnectionMode::WebSocket(url),
                WebSocketClient::client_info(),
            )
        } else {
            let paths = StatePaths::discover(args.state_dir.clone())?;
            writer_lock = Some(WriterLock::acquire(
                paths.database.with_file_name("app-server.writer.lock"),
            )?);
            let service = Arc::new(FixTraceAppService::start(
                AppServiceOptions {
                    state_dir: args.state_dir,
                    config_path: args.config,
                    initialize_event_store: true,
                },
                cancellation.clone(),
            )?);
            let client = Arc::new(InProcessClient::new(service));
            (
                client,
                ConnectionMode::InProcess,
                InProcessClient::client_info(),
            )
        };
    let _writer_lock = writer_lock;
    let initialized = client
        .initialize(InitializeRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            client: info,
            capabilities: ClientCapabilities {
                supports_streaming: true,
                supports_approvals: true,
                supports_diff: true,
                supports_graph: true,
                supports_artifacts: true,
            },
        })
        .await?;
    let result = fixtrace_tui::run(client, Model::new(initialized, connection), args.session).await;
    cancellation.cancel();
    result?;
    Ok(())
}

fn read_token(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(format!("token file {} must have mode 0600", path.display()).into());
        }
    }
    let token = fs::read_to_string(path)?.trim().to_owned();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("token file {} is malformed", path.display()).into());
    }
    Ok(token)
}
