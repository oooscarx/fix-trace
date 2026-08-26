use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    application::{
        AppCommand, AppResponse, AppServiceOptions, FixTraceAppService, FixTraceApplication,
    },
    cli::{Cli, Command, ConfigCommand, HistoryCommand},
    error::AppError,
    progress::renderer,
};

/// Compatibility adapter for the original script-oriented CLI.
///
/// All stateful operations are submitted through `FixTraceApplication`; this
/// module only maps clap types, renders progress, and preserves legacy stdout.
pub async fn run(cli: Cli, cancellation: CancellationToken) -> Result<(), AppError> {
    let options = AppServiceOptions {
        state_dir: cli.state_dir,
        config_path: cli.config,
    };
    let command = command_from_cli(cli.command)?;
    let service = FixTraceAppService::start(options, cancellation)?;
    let _renderer = renderer::spawn(service.subscribe_progress());
    let response = service.execute(command).await?;
    render_response(response)
}

fn command_from_cli(command: Command) -> Result<AppCommand, AppError> {
    Ok(match command {
        Command::Init { project, oracle } => AppCommand::InitializeSession { project, oracle },
        Command::Shell { session_id } => AppCommand::RunControlledShell {
            session_id: parse_session_id(&session_id)?,
        },
        Command::Analyze { session_id, no_llm } => AppCommand::AnalyzeSession {
            session_id: parse_session_id(&session_id)?,
            no_llm,
        },
        Command::Show { session_id } => AppCommand::GetSession {
            session_id: parse_session_id(&session_id)?,
        },
        Command::History {
            command: HistoryCommand::List,
        } => AppCommand::ListSessions,
        Command::History {
            command: HistoryCommand::Show { session_id },
        } => AppCommand::GetSession {
            session_id: parse_session_id(&session_id)?,
        },
        Command::Export { session_id, output } => AppCommand::ExportSession {
            session_id: parse_session_id(&session_id)?,
            output,
        },
        Command::Import { input } => AppCommand::ImportSession { input },
        Command::Config {
            command: ConfigCommand::Show,
        } => AppCommand::GetConfig,
        Command::Config {
            command: ConfigCommand::Set { key, value },
        } => AppCommand::SetConfig { key, value },
        Command::Demo { no_llm } => AppCommand::RunDemo { no_llm },
    })
}

fn render_response(response: AppResponse) -> Result<(), AppError> {
    match response {
        AppResponse::SessionInitialized { session } => {
            println!("session_id={}", session.id);
            println!("worktree={}", session.worktree_path.display());
            println!("next: fixtrace shell {}", session.id);
        }
        AppResponse::ControlledShellCompleted { .. } => {}
        AppResponse::SessionAnalyzed { result } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "session_id": result.session_id,
                    "baseline_hash": result.report.baseline_hash,
                    "minimal_action_ids": result.report.minimal_action_ids,
                    "final_trial_id": result.report.final_trial.id,
                    "final_outcome": result.report.final_trial.outcome,
                    "ablations": result.report.ablations,
                    "statement": result.report.statement,
                    "diagnosis": result.diagnosis,
                    "agent": result.agent,
                    "llm_mode": result.llm_mode,
                }))?
            );
        }
        AppResponse::Session { detail } => {
            println!("{}", serde_json::to_string_pretty(&detail)?);
        }
        AppResponse::Sessions { sessions } => {
            if sessions.is_empty() {
                println!("no sessions");
            } else {
                for session in sessions {
                    println!(
                        "{}\t{}\t{}\t{}",
                        session.id,
                        session.status.as_str(),
                        session.updated_at.to_rfc3339(),
                        session.project_name
                    );
                }
            }
        }
        AppResponse::SessionExported { session_id, output } => {
            println!("exported {session_id} to {}", output.display());
        }
        AppResponse::SessionImported { session_id } => {
            println!("imported session {session_id}");
        }
        AppResponse::Config { toml } => print!("{toml}"),
        AppResponse::ConfigSaved { key, path } => {
            println!("saved {key} in {}", path.display());
        }
        AppResponse::Demo { report } => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn parse_session_id(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value)
        .map_err(|error| AppError::Process(format!("invalid session ID `{value}`: {error}")))
}
