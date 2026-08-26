use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "fixtrace",
    version,
    about = "Replay-verified minimal repair traces for Rust/Cargo projects"
)]
pub struct Cli {
    /// Load configuration from this TOML file.
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Store sessions and history under this directory.
    #[arg(long, global = true, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,

    /// Enable verbose diagnostic logs.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a debugging session and immutable baseline.
    Init {
        project: PathBuf,
        #[arg(long)]
        oracle: String,
    },
    /// Enter the controlled session REPL.
    Shell { session_id: String },
    /// Replay and minimize a completed session.
    Analyze { session_id: String },
    /// Show a live or completed session.
    Show { session_id: String },
    /// List or inspect persisted sessions.
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    /// Export a complete redacted session as JSON.
    Export {
        session_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Import a previously exported session.
    Import { input: PathBuf },
    /// Inspect or update configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run the deterministic bundled demonstration.
    Demo {
        /// Disable model calls and emit a deterministic evidence report.
        #[arg(long)]
        no_llm: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum HistoryCommand {
    List,
    Show { session_id: String },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Show,
    Set { key: String, value: String },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn cli_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }
}
