mod cli;
mod config;
mod demo;
mod domain;
mod error;
mod minimize;
mod replay;
mod sandbox;

use clap::Parser;
use cli::{Cli, Command, ConfigCommand};
use config::FixTraceConfig;
use demo::run_demo;
use error::AppError;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Config {
            command: ConfigCommand::Show,
        } => {
            let config = match cli.config {
                Some(path) => FixTraceConfig::load(&path)?,
                None => FixTraceConfig::default(),
            };
            print!("{}", config.to_toml()?);
            Ok(())
        }
        Command::Demo { no_llm } => {
            let cancellation = CancellationToken::new();
            install_ctrl_c_handler(cancellation.clone());
            run_demo(no_llm, cancellation).await
        }
        command => Err(AppError::NotImplemented(command.label().to_owned())),
    }
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
