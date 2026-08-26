use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Instant,
};

use chrono::Utc;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    config::FixTraceConfig,
    domain::{
        action::{Action, ActionKind, ActionResult},
        session::SessionStatus,
        snapshot::SnapshotManifest,
        trial::TrialOutcome,
    },
    error::AppError,
    history::database::HistoryDatabase,
    progress::{ProgressEvent, ProgressSender},
    replay::{
        executor::{ReplayState, replay_action},
        oracle::run_oracle,
    },
    sandbox::local_copy::{normalize_project_path, safe_write_path},
    workflow::runner_for_session,
};

use super::patch::replacements_between;

pub async fn run(
    database: &HistoryDatabase,
    config: &FixTraceConfig,
    session_id: Uuid,
    cancellation: &CancellationToken,
    progress: ProgressSender,
) -> Result<(), AppError> {
    let mut session = database.load_session(session_id)?;
    if session.status != SessionStatus::Recording {
        return Err(AppError::Process(format!(
            "session {session_id} is {:?}, not recording",
            session.status
        )));
    }
    let mut actions = database.load_actions(session_id)?;
    let mut replay_state = ReplayState::restore_context(
        session.worktree_path.clone(),
        config.replay.include_target,
        &actions,
    )?;
    let mut last_snapshot =
        SnapshotManifest::capture(&session.worktree_path, config.replay.include_target)?;
    println!("FixTrace controlled shell. Type :help for commands.");

    loop {
        if cancellation.is_cancelled() {
            session.status = SessionStatus::Cancelled;
            session.updated_at = Utc::now();
            database.save_session(&session)?;
            progress.emit(ProgressEvent::Cancelled);
            return Ok(());
        }
        print!("fixtrace:{}> ", display_cwd(replay_state.cwd()));
        io::stdout()
            .flush()
            .map_err(|error| AppError::io("flush REPL prompt", ".", error))?;
        let mut line = String::new();
        let bytes = io::stdin()
            .read_line(&mut line)
            .map_err(|error| AppError::io("read REPL input", ".", error))?;
        if bytes == 0 {
            println!();
            return Ok(());
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match line {
            ":help" => print_help(),
            ":quit" => return Ok(()),
            ":status" => {
                println!(
                    "session={session_id} status={:?} actions={} cwd={}",
                    session.status,
                    actions.len(),
                    display_cwd(replay_state.cwd())
                );
            }
            ":verify" => {
                let evidence = run_oracle(&session.oracle, &replay_state, cancellation).await?;
                print_process_output(&evidence.stdout, &evidence.stderr);
                println!(
                    "Oracle: {} (exit={:?}, {} ms)",
                    if evidence.passed() { "Pass" } else { "Fail" },
                    evidence.exit_code,
                    evidence.duration_ms
                );
            }
            ":done" => {
                checkpoint_if_changed(
                    database,
                    &mut session,
                    &mut actions,
                    &mut last_snapshot,
                    replay_state.cwd(),
                    "automatic checkpoint before :done",
                    config.replay.include_target,
                )?;
                let runner = runner_for_session(&session, config, progress.clone())?;
                let trial = runner.run(&actions, cancellation).await?;
                database.save_trial(session_id, &trial)?;
                println!("full replay trial {}: {:?}", trial.id, trial.outcome);
                match trial.outcome {
                    TrialOutcome::StablePass => {
                        session.status = SessionStatus::ReadyForAnalysis;
                        session.updated_at = Utc::now();
                        database.save_session(&session)?;
                        println!("session is ready: fixtrace analyze {session_id}");
                        return Ok(());
                    }
                    TrialOutcome::Cancelled => {
                        session.status = SessionStatus::Cancelled;
                        session.updated_at = Utc::now();
                        database.save_session(&session)?;
                        return Ok(());
                    }
                    outcome => println!(
                        "full trace is not stably sufficient ({outcome:?}); continue recording"
                    ),
                }
            }
            _ if line.starts_with(":checkpoint") => {
                let note = line
                    .strip_prefix(":checkpoint")
                    .map(str::trim)
                    .filter(|note| !note.is_empty())
                    .unwrap_or("manual checkpoint");
                checkpoint_if_changed(
                    database,
                    &mut session,
                    &mut actions,
                    &mut last_snapshot,
                    replay_state.cwd(),
                    note,
                    config.replay.include_target,
                )?;
            }
            _ if line.starts_with(":edit ") => {
                let requested = Path::new(line.trim_start_matches(":edit ").trim());
                let relative = normalize_project_path(replay_state.cwd(), requested)?;
                let target = safe_write_path(&session.worktree_path, &relative)?;
                let before = SnapshotManifest::capture(
                    &session.worktree_path,
                    config.replay.include_target,
                )?;
                let started = Instant::now();
                let status = run_editor(&target).await?;
                let after = SnapshotManifest::capture(
                    &session.worktree_path,
                    config.replay.include_target,
                )?;
                let replacements = replacements_between(&session.worktree_path, &before, &after)?;
                if replacements.is_empty() {
                    println!("editor made no tracked changes");
                } else {
                    let action = external_patch_action(
                        next_action_id(&actions),
                        replay_state.cwd().to_path_buf(),
                        replacements,
                        Some(format!("edited {}", relative.display())),
                        PatchExecution {
                            before,
                            after: after.clone(),
                            exit_code: status.code(),
                            duration_ms: elapsed_ms(started),
                        },
                    );
                    database.save_action(session_id, &action)?;
                    actions.push(action);
                    touch_session(database, &mut session)?;
                }
                last_snapshot = after;
            }
            _ => {
                let kind = parse_action(line, replay_state.cwd())?;
                let mut action = Action {
                    id: next_action_id(&actions),
                    original_order: next_action_id(&actions),
                    cwd_before: replay_state.cwd().to_path_buf(),
                    kind,
                    replayable: true,
                    note: None,
                    result: None,
                };
                let result = replay_action(
                    &mut replay_state,
                    &action,
                    session.oracle.timeout(),
                    cancellation,
                )
                .await?;
                print_process_output(&result.stdout, &result.stderr);
                println!("exit={:?}, {} ms", result.exit_code, result.duration_ms);
                action.result = Some(result);
                database.save_action(session_id, &action)?;
                actions.push(action);
                touch_session(database, &mut session)?;
                last_snapshot = SnapshotManifest::capture(
                    &session.worktree_path,
                    config.replay.include_target,
                )?;
            }
        }
    }
}

fn checkpoint_if_changed(
    database: &HistoryDatabase,
    session: &mut crate::domain::session::SessionRecord,
    actions: &mut Vec<Action>,
    last_snapshot: &mut SnapshotManifest,
    cwd_before: &Path,
    note: &str,
    include_target: bool,
) -> Result<(), AppError> {
    let after = SnapshotManifest::capture(&session.worktree_path, include_target)?;
    let replacements = replacements_between(&session.worktree_path, last_snapshot, &after)?;
    if replacements.is_empty() {
        println!("no tracked file changes since the previous snapshot");
        *last_snapshot = after;
        return Ok(());
    }
    let action = external_patch_action(
        next_action_id(actions),
        cwd_before.to_path_buf(),
        replacements,
        Some(note.to_owned()),
        PatchExecution {
            before: last_snapshot.clone(),
            after: after.clone(),
            exit_code: Some(0),
            duration_ms: 0,
        },
    );
    database.save_action(session.id, &action)?;
    actions.push(action);
    touch_session(database, session)?;
    *last_snapshot = after;
    println!("checkpoint recorded as action {}", actions.len());
    Ok(())
}

fn external_patch_action(
    id: u64,
    cwd_before: PathBuf,
    files: Vec<crate::domain::action::FileReplacement>,
    note: Option<String>,
    execution: PatchExecution,
) -> Action {
    Action {
        id,
        original_order: id,
        cwd_before,
        kind: ActionKind::FilePatch { files },
        replayable: true,
        note,
        result: Some(ActionResult {
            exit_code: execution.exit_code,
            duration_ms: execution.duration_ms,
            stdout: String::new(),
            stderr: String::new(),
            stdout_artifact: None,
            stderr_artifact: None,
            timed_out: false,
            cancelled: false,
            before_snapshot_hash: execution.before.root_hash.clone(),
            after_snapshot_hash: execution.after.root_hash.clone(),
            delta: execution.before.diff(&execution.after),
        }),
    }
}

struct PatchExecution {
    before: SnapshotManifest,
    after: SnapshotManifest,
    exit_code: Option<i32>,
    duration_ms: u64,
}

fn parse_action(line: &str, current_cwd: &Path) -> Result<ActionKind, AppError> {
    if line == "cd" || line.starts_with("cd ") {
        let words = shell_words::split(line)
            .map_err(|error| AppError::Process(format!("invalid cd syntax: {error}")))?;
        if words.len() != 2 {
            return Err(AppError::Process(
                "usage: cd <project-relative-path>".to_owned(),
            ));
        }
        return Ok(ActionKind::ChangeDirectory {
            path: normalize_project_path(current_cwd, Path::new(&words[1]))?,
        });
    }
    if let Some(assignment) = line.strip_prefix("export ") {
        let (key, value) = assignment.split_once('=').ok_or_else(|| {
            AppError::Process("usage: export KEY=value (without shell expansion)".to_owned())
        })?;
        return Ok(ActionKind::SetEnvironment {
            key: key.trim().to_owned(),
            value: value.to_owned(),
        });
    }
    if let Some(key) = line.strip_prefix("unset ") {
        return Ok(ActionKind::UnsetEnvironment {
            key: key.trim().to_owned(),
        });
    }
    Ok(ActionKind::ShellCommand {
        command: line.to_owned(),
    })
}

async fn run_editor(path: &Path) -> Result<std::process::ExitStatus, AppError> {
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_owned());
    let words = shell_words::split(&editor)
        .map_err(|error| AppError::Process(format!("invalid EDITOR setting: {error}")))?;
    let (program, arguments) = words
        .split_first()
        .ok_or_else(|| AppError::Process("EDITOR cannot be empty".to_owned()))?;
    Command::new(program)
        .args(arguments)
        .arg(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|error| AppError::Process(format!("editor failed to start: {error}")))
}

fn touch_session(
    database: &HistoryDatabase,
    session: &mut crate::domain::session::SessionRecord,
) -> Result<(), AppError> {
    session.updated_at = Utc::now();
    database.save_session(session)
}

fn next_action_id(actions: &[Action]) -> u64 {
    actions.iter().map(|action| action.id).max().unwrap_or(0) + 1
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn display_cwd(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", path.display())
    }
}

fn print_process_output(stdout: &str, stderr: &str) {
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
}

fn print_help() {
    println!(
        "ordinary shell command | cd <path> | export KEY=value | unset KEY\n\
         :edit <path> | :checkpoint <note> | :verify | :status | :done | :quit"
    );
}
