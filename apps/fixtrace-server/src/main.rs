use std::{path::PathBuf, sync::Arc};

use clap::Parser;
use fixtrace::{
    application::{AppServiceOptions, FixTraceAppService, FixTraceProtocolApplication},
    history::paths::StatePaths,
};
use fixtrace_server::{
    WriterLock, load_or_create_token, parse_ws_bind, serve_stdio, serve_websocket,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "FixTrace local App Server", version)]
struct Args {
    /// Directory containing history.sqlite3, config.toml, and App Server state.
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Override the FixTrace configuration file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// `stdio` or a WebSocket URL such as `ws://127.0.0.1:4765`.
    #[arg(long, default_value = "stdio")]
    listen: String,

    /// File containing the WebSocket bearer token (created with mode 0600).
    #[arg(long)]
    token_file: Option<PathBuf>,

    /// Permit a WebSocket bind to a non-loopback address.
    #[arg(long)]
    allow_remote: bool,

    /// Increase stderr logging detail.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let level = match args.verbose {
        0 => "fixtrace_server=info",
        1 => "fixtrace_server=debug",
        _ => "fixtrace_server=trace",
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| level.into()))
        .init();

    let paths = StatePaths::discover(args.state_dir.clone())?;
    let lock_path = paths.database.with_file_name("app-server.writer.lock");
    let _writer_lock = WriterLock::acquire(lock_path)?;
    let cancellation = CancellationToken::new();
    let signal_cancellation = cancellation.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    let application: Arc<dyn FixTraceProtocolApplication> = Arc::new(FixTraceAppService::start(
        AppServiceOptions {
            state_dir: args.state_dir,
            config_path: args.config,
            initialize_event_store: true,
        },
        cancellation.clone(),
    )?);

    let result = if args.listen == "stdio" {
        serve_stdio(application, cancellation.clone())
            .await
            .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })
    } else {
        let address = parse_ws_bind(&args.listen, args.allow_remote)?;
        let token_path = args
            .token_file
            .unwrap_or_else(|| paths.database.with_file_name("app-server.token"));
        let token = load_or_create_token(&token_path)?;
        let listener = TcpListener::bind(address).await?;
        tracing::info!(address = %listener.local_addr()?, token_file = %token_path.display(), "App Server listening");
        serve_websocket(listener, application, token, cancellation.clone())
            .await
            .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })
    };
    cancellation.cancel();
    result
}
