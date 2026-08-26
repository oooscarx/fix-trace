mod agent;
mod app;
mod cli;
mod config;
mod demo;
mod domain;
mod error;
mod history;
mod llm;
mod minimize;
mod progress;
mod replay;
mod sandbox;
mod workflow;

use clap::Parser;
use cli::Cli;
use error::AppError;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let cancellation = CancellationToken::new();
    install_ctrl_c_handler(cancellation.clone());
    app::run(cli, cancellation).await
}

fn install_ctrl_c_handler(cancellation: CancellationToken) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancellation.cancel();
        }
    });
}

fn init_tracing(verbose: bool) {
    let fallback = if verbose {
        "fixtrace=debug"
    } else {
        "fixtrace=info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(fallback));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}
mod recorder;
