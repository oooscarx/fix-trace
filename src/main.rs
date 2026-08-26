mod cli;
mod config;
mod error;

use clap::Parser;
use cli::{Cli, Command, ConfigCommand};
use config::FixTraceConfig;
use error::AppError;
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
        command => Err(AppError::NotImplemented(command.label().to_owned())),
    }
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
