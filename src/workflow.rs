use std::{fs, path::Path};

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    config::FixTraceConfig,
    domain::{
        session::{SessionRecord, SessionStatus},
        snapshot::SnapshotManifest,
        trial::TrialOutcome,
    },
    error::AppError,
    history::{database::HistoryDatabase, paths::StatePaths},
    minimize::engine::{MinimizationReport, minimize},
    progress::{ProgressEvent, ProgressSender},
    replay::{oracle::OracleSpec, runner::TrialRunner},
    sandbox::local_copy::{copy_project, protect_read_only},
};

pub async fn initialize_session(
    state: &StatePaths,
    database: &HistoryDatabase,
    config: &FixTraceConfig,
    project: &Path,
    oracle_command: String,
    cancellation: &CancellationToken,
    progress: ProgressSender,
) -> Result<SessionRecord, AppError> {
    let original_project = project
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize project", project, error))?;
    if !original_project.is_dir() {
        return Err(AppError::InvalidProject {
            path: original_project,
            reason: "project path is not a directory".to_owned(),
        });
    }
    let project_name = original_project
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned();
    let session_id = Uuid::new_v4();
    let session_root = state.session_root(session_id);
    let baseline_path = session_root.join("baseline");
    let worktree_path = session_root.join("worktree");
    fs::create_dir(&session_root)
        .map_err(|error| AppError::io("create session directory", &session_root, error))?;
    copy_project(
        &original_project,
        &baseline_path,
        config.replay.include_target,
    )?;
    progress.emit(ProgressEvent::BaselineCopied);
    copy_project(
        &original_project,
        &worktree_path,
        config.replay.include_target,
    )?;
    let baseline_manifest =
        SnapshotManifest::capture(&baseline_path, config.replay.include_target)?;
    let oracle = OracleSpec {
        command: oracle_command,
        timeout_ms: config.replay.oracle_timeout_secs.saturating_mul(1000),
    };
    let now = Utc::now();
    let mut session = SessionRecord {
        id: session_id,
        parent_session_id: None,
        archived: false,
        project_name,
        original_project,
        baseline_path: baseline_path.clone(),
        worktree_path,
        oracle: oracle.clone(),
        baseline_manifest,
        status: SessionStatus::Invalid,
        created_at: now,
        updated_at: now,
    };
    database.save_session(&session)?;
    progress.emit(ProgressEvent::SessionCreated { session_id });
    database.record_progress(
        Some(session_id),
        &ProgressEvent::SessionCreated { session_id },
    )?;

    let runner = TrialRunner::new(
        baseline_path.clone(),
        oracle,
        config.replay.repetitions,
        config.replay.include_target,
    )?
    .with_progress(progress.clone());
    let baseline_trial = runner.run(&[], cancellation).await?;
    database.save_trial(session_id, &baseline_trial)?;
    if baseline_trial.outcome != TrialOutcome::StableFail {
        session.updated_at = Utc::now();
        session.status = if baseline_trial.outcome == TrialOutcome::Cancelled {
            SessionStatus::Cancelled
        } else {
            SessionStatus::Invalid
        };
        database.save_session(&session)?;
        protect_read_only(&baseline_path)?;
        return Err(AppError::BaselineOracle(format!(
            "{:?} (trial {})",
            baseline_trial.outcome, baseline_trial.id
        )));
    }

    protect_read_only(&baseline_path)?;
    session.status = SessionStatus::Recording;
    session.updated_at = Utc::now();
    database.save_session(&session)?;
    Ok(session)
}

pub async fn analyze_session(
    database: &HistoryDatabase,
    config: &FixTraceConfig,
    session_id: Uuid,
    cancellation: &CancellationToken,
    progress: ProgressSender,
) -> Result<MinimizationReport, AppError> {
    let mut session = database.load_session(session_id)?;
    if !matches!(
        session.status,
        SessionStatus::ReadyForAnalysis | SessionStatus::Analyzed
    ) {
        return Err(AppError::Minimization(format!(
            "session {session_id} has status {:?}; finish it with `:done` first",
            session.status
        )));
    }
    let actions = database.load_actions(session_id)?;
    let runner = TrialRunner::with_manifest(
        session.baseline_path.clone(),
        session.baseline_manifest.clone(),
        session.oracle.clone(),
        config.replay.repetitions,
        config.replay.include_target,
    )?
    .with_progress(progress.clone());
    let report = minimize(&runner, &actions, cancellation).await?;
    for trial in &report.trials {
        database.save_trial(session_id, trial)?;
    }
    database.insert_json(
        "diagnoses",
        Some(session_id),
        &serde_json::to_value(&report)?,
    )?;
    session.status = SessionStatus::Analyzed;
    session.updated_at = Utc::now();
    database.save_session(&session)?;
    progress.emit(ProgressEvent::Finished);
    database.record_progress(Some(session_id), &ProgressEvent::Finished)?;
    Ok(report)
}

pub fn runner_for_session(
    session: &SessionRecord,
    config: &FixTraceConfig,
    progress: ProgressSender,
) -> Result<TrialRunner, AppError> {
    Ok(TrialRunner::with_manifest(
        session.baseline_path.clone(),
        session.baseline_manifest.clone(),
        session.oracle.clone(),
        config.replay.repetitions,
        config.replay.include_target,
    )?
    .with_progress(progress))
}
