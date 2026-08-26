use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    cli::{Cli, Command, ConfigCommand, HistoryCommand},
    config::FixTraceConfig,
    demo::run_demo,
    error::AppError,
    history::{
        database::HistoryDatabase,
        export::{export_session, import_session},
        paths::StatePaths,
    },
    progress::{ProgressSender, renderer},
    recorder::repl,
    workflow::{analyze_session, initialize_session},
};

pub async fn run(cli: Cli, cancellation: CancellationToken) -> Result<(), AppError> {
    let state = StatePaths::discover(cli.state_dir)?;
    let config_path = cli.config.unwrap_or_else(|| state.config.clone());
    let mut config = FixTraceConfig::load_or_default(&config_path)?;

    match cli.command {
        Command::Config {
            command: ConfigCommand::Show,
        } => {
            print!("{}", config.to_toml()?);
            Ok(())
        }
        Command::Config {
            command: ConfigCommand::Set { key, value },
        } => {
            config.set(&key, &value)?;
            config.save(&config_path)?;
            println!("saved {key} in {}", config_path.display());
            Ok(())
        }
        Command::Demo { no_llm } => run_demo(no_llm, cancellation).await,
        command => {
            let database = HistoryDatabase::open(&state.database)?;
            dispatch(command, &state, &database, &config, cancellation).await
        }
    }
}

async fn dispatch(
    command: Command,
    state: &StatePaths,
    database: &HistoryDatabase,
    config: &FixTraceConfig,
    cancellation: CancellationToken,
) -> Result<(), AppError> {
    match command {
        Command::Init { project, oracle } => {
            let (progress, receiver) = ProgressSender::channel(256);
            let _renderer = renderer::spawn(receiver);
            let session = initialize_session(
                state,
                database,
                config,
                &project,
                oracle,
                &cancellation,
                progress,
            )
            .await?;
            println!("session_id={}", session.id);
            println!("worktree={}", session.worktree_path.display());
            println!("next: fixtrace shell {}", session.id);
            Ok(())
        }
        Command::Shell { session_id } => {
            let session_id = parse_session_id(&session_id)?;
            let (progress, receiver) = ProgressSender::channel(256);
            let _renderer = renderer::spawn(receiver);
            repl::run(database, config, session_id, &cancellation, progress).await
        }
        Command::Analyze { session_id } => {
            let session_id = parse_session_id(&session_id)?;
            let (progress, receiver) = ProgressSender::channel(1024);
            let _renderer = renderer::spawn(receiver);
            let report =
                analyze_session(database, config, session_id, &cancellation, progress).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "session_id": session_id,
                    "baseline_hash": report.baseline_hash,
                    "minimal_action_ids": report.minimal_action_ids,
                    "final_trial_id": report.final_trial.id,
                    "final_outcome": report.final_trial.outcome,
                    "ablations": report.ablations,
                    "statement": report.statement,
                }))?
            );
            Ok(())
        }
        Command::Show { session_id } => show_session(database, parse_session_id(&session_id)?),
        Command::History {
            command: HistoryCommand::List,
        } => list_history(database),
        Command::History {
            command: HistoryCommand::Show { session_id },
        } => show_session(database, parse_session_id(&session_id)?),
        Command::Export { session_id, output } => {
            let session_id = parse_session_id(&session_id)?;
            export_session(database, session_id, &output)?;
            println!("exported {session_id} to {}", output.display());
            Ok(())
        }
        Command::Import { input } => {
            let imported = import_session(database, &input)?;
            println!("imported session {}", imported.session.id);
            Ok(())
        }
        Command::Config { .. } | Command::Demo { .. } => Err(AppError::Process(
            "internal command dispatch error".to_owned(),
        )),
    }
}

fn list_history(database: &HistoryDatabase) -> Result<(), AppError> {
    let sessions = database.list_sessions()?;
    if sessions.is_empty() {
        println!("no sessions");
        return Ok(());
    }
    for session in sessions {
        println!(
            "{}\t{}\t{}\t{}",
            session.id,
            session.status.as_str(),
            session.updated_at.to_rfc3339(),
            session.project_name
        );
    }
    Ok(())
}

fn show_session(database: &HistoryDatabase, session_id: Uuid) -> Result<(), AppError> {
    let session = database.load_session(session_id)?;
    let actions = database.load_actions(session_id)?;
    let trials = database.load_trials(session_id)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "session": session,
            "actions": actions,
            "trials": trials,
            "messages": database.load_json("messages", session_id)?,
            "tool_calls": database.load_json("tool_calls", session_id)?,
            "api_usage": database.load_json("api_usage", session_id)?,
            "progress_events": database.load_json("progress_events", session_id)?,
            "diagnoses": database.load_json("diagnoses", session_id)?,
        }))?
    );
    Ok(())
}

fn parse_session_id(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value)
        .map_err(|error| AppError::Process(format!("invalid session ID `{value}`: {error}")))
}
