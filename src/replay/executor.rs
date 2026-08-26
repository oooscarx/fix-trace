use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::{
        action::{Action, ActionKind, ActionResult, ArtifactRef, FileReplacement},
        snapshot::SnapshotManifest,
    },
    error::AppError,
    sandbox::local_copy::{safe_existing_path, safe_write_path},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessEvidence {
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub stdout_artifact: Option<ArtifactRef>,
    #[serde(default)]
    pub stderr_artifact: Option<ArtifactRef>,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl ProcessEvidence {
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled
    }
}

#[derive(Debug)]
pub struct ReplayState {
    root: PathBuf,
    cwd: PathBuf,
    environment: BTreeMap<String, Option<String>>,
    include_target: bool,
}

impl ReplayState {
    pub fn new(root: PathBuf, include_target: bool) -> Self {
        Self {
            root,
            cwd: PathBuf::new(),
            environment: BTreeMap::new(),
            include_target,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn environment(&self) -> &BTreeMap<String, Option<String>> {
        &self.environment
    }

    pub fn restore_context(
        root: PathBuf,
        include_target: bool,
        actions: &[Action],
    ) -> Result<Self, AppError> {
        let mut state = Self::new(root, include_target);
        let mut ordered: Vec<_> = actions.iter().collect();
        ordered.sort_by_key(|action| action.original_order);
        for action in ordered {
            match &action.kind {
                ActionKind::SetEnvironment { key, value } => {
                    validate_environment_key(key)?;
                    state.environment.insert(key.clone(), Some(value.clone()));
                }
                ActionKind::UnsetEnvironment { key } => {
                    validate_environment_key(key)?;
                    state.environment.insert(key.clone(), None);
                }
                ActionKind::ChangeDirectory { path } => {
                    let directory = safe_existing_path(&state.root, path)?;
                    if !directory.is_dir() {
                        return Err(AppError::UnsafePath {
                            path: path.clone(),
                            reason: "recorded working directory is not a directory".to_owned(),
                        });
                    }
                    state.cwd = path.clone();
                }
                ActionKind::ShellCommand { .. } | ActionKind::FilePatch { .. } => {}
            }
        }
        Ok(state)
    }
}

pub async fn replay_action(
    state: &mut ReplayState,
    action: &Action,
    command_timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<ActionResult, AppError> {
    if !action.replayable {
        return Err(AppError::NonReplayable {
            action_id: action.id,
            reason: "action was marked non-replayable when recorded".to_owned(),
        });
    }
    if action.cwd_before != state.cwd {
        return Err(AppError::WorkingDirectoryMismatch {
            action_id: action.id,
            expected: action.cwd_before.clone(),
            actual: state.cwd.clone(),
        });
    }
    if cancellation.is_cancelled() {
        return Ok(cancelled_action_result(
            SnapshotManifest::capture(&state.root, state.include_target)?.root_hash,
        ));
    }

    let before = SnapshotManifest::capture(&state.root, state.include_target)?;
    let started = Instant::now();
    let process = match &action.kind {
        ActionKind::ShellCommand { command } => {
            run_shell_command(
                &state.root,
                &state.cwd,
                &state.environment,
                command,
                command_timeout,
                cancellation,
            )
            .await?
        }
        ActionKind::FilePatch { files } => {
            for replacement in files {
                apply_file_replacement(&state.root, replacement)?;
            }
            successful_internal_action(started.elapsed())
        }
        ActionKind::SetEnvironment { key, value } => {
            validate_environment_key(key)?;
            state.environment.insert(key.clone(), Some(value.clone()));
            successful_internal_action(started.elapsed())
        }
        ActionKind::UnsetEnvironment { key } => {
            validate_environment_key(key)?;
            state.environment.insert(key.clone(), None);
            successful_internal_action(started.elapsed())
        }
        ActionKind::ChangeDirectory { path } => {
            let directory = safe_existing_path(&state.root, path)?;
            if !directory.is_dir() {
                return Err(AppError::UnsafePath {
                    path: path.clone(),
                    reason: "working directory target is not a directory".to_owned(),
                });
            }
            state.cwd = path.clone();
            successful_internal_action(started.elapsed())
        }
    };
    let after = SnapshotManifest::capture(&state.root, state.include_target)?;

    Ok(ActionResult {
        exit_code: process.exit_code,
        duration_ms: process.duration_ms,
        stdout: process.stdout,
        stderr: process.stderr,
        stdout_artifact: process.stdout_artifact,
        stderr_artifact: process.stderr_artifact,
        timed_out: process.timed_out,
        cancelled: process.cancelled,
        before_snapshot_hash: before.root_hash.clone(),
        after_snapshot_hash: after.root_hash.clone(),
        delta: before.diff(&after),
    })
}

pub async fn run_shell_command(
    root: &Path,
    cwd: &Path,
    environment: &BTreeMap<String, Option<String>>,
    command: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<ProcessEvidence, AppError> {
    if command.trim().is_empty() {
        return Err(AppError::Process("command cannot be empty".to_owned()));
    }
    let working_directory = safe_existing_path(root, cwd)?;
    let mut process = shell_command(command);
    process
        .current_dir(&working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in environment {
        match value {
            Some(value) => {
                process.env(key, value);
            }
            None => {
                process.env_remove(key);
            }
        }
    }

    let started = Instant::now();
    let mut child = process
        .spawn()
        .map_err(|error| AppError::Process(format!("could not spawn `{command}`: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Process("child stdout was not captured".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Process("child stderr was not captured".to_owned()))?;
    let stdout_task = tokio::spawn(read_stream(stdout));
    let stderr_task = tokio::spawn(read_stream(stderr));

    let mut timed_out = false;
    let mut cancelled = false;
    let status = tokio::select! {
        _ = cancellation.cancelled() => {
            cancelled = true;
            terminate_child(&mut child).await?
        }
        result = tokio::time::timeout(timeout, child.wait()) => {
            match result {
                Ok(status) => status.map_err(|error| AppError::Process(format!("could not wait for `{command}`: {error}")))?,
                Err(_) => {
                    timed_out = true;
                    terminate_child(&mut child).await?
                }
            }
        }
    };

    let stdout = join_output(stdout_task, "stdout").await?;
    let stderr = join_output(stderr_task, "stderr").await?;
    Ok(ProcessEvidence {
        exit_code: status.code(),
        duration_ms: elapsed_ms(started.elapsed()),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        stdout_artifact: None,
        stderr_artifact: None,
        timed_out,
        cancelled,
    })
}

fn apply_file_replacement(root: &Path, replacement: &FileReplacement) -> Result<(), AppError> {
    let target = safe_write_path(root, &replacement.path)?;
    match &replacement.content {
        Some(content) => {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    AppError::io("create patch parent directory", parent, error)
                })?;
            }
            fs::write(&target, content)
                .map_err(|error| AppError::io("write file replacement", &target, error))?;
            set_unix_mode(&target, replacement.unix_mode)?;
        }
        None => match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&target)
                .map_err(|error| AppError::io("delete replaced directory", &target, error))?,
            Ok(_) => fs::remove_file(&target)
                .map_err(|error| AppError::io("delete replaced file", &target, error))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::io(
                    "inspect file replacement target",
                    &target,
                    error,
                ));
            }
        },
    }
    Ok(())
}

fn validate_environment_key(key: &str) -> Result<(), AppError> {
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return Err(AppError::Process(format!(
            "invalid environment variable name `{key}`"
        )));
    }
    Ok(())
}

fn successful_internal_action(duration: Duration) -> ProcessEvidence {
    ProcessEvidence {
        exit_code: Some(0),
        duration_ms: elapsed_ms(duration),
        stdout: String::new(),
        stderr: String::new(),
        stdout_artifact: None,
        stderr_artifact: None,
        timed_out: false,
        cancelled: false,
    }
}

fn cancelled_action_result(snapshot_hash: String) -> ActionResult {
    ActionResult {
        exit_code: None,
        duration_ms: 0,
        stdout: String::new(),
        stderr: String::new(),
        stdout_artifact: None,
        stderr_artifact: None,
        timed_out: false,
        cancelled: true,
        before_snapshot_hash: snapshot_hash.clone(),
        after_snapshot_hash: snapshot_hash,
        delta: Default::default(),
    }
}

async fn terminate_child(
    child: &mut tokio::process::Child,
) -> Result<std::process::ExitStatus, AppError> {
    #[cfg(unix)]
    {
        use nix::{
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };

        if let Some(process_id) = child.id() {
            let raw_id = i32::try_from(process_id)
                .map_err(|_| AppError::Process("child process ID exceeds i32".to_owned()))?;
            let process_group = Pid::from_raw(raw_id);
            let _ignored = killpg(process_group, Signal::SIGTERM);
            if let Ok(status) = tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                return status.map_err(|error| {
                    AppError::Process(format!("could not reap terminated process group: {error}"))
                });
            }
            let _ignored = killpg(process_group, Signal::SIGKILL);
            return child.wait().await.map_err(|error| {
                AppError::Process(format!("could not reap killed process group: {error}"))
            });
        }
    }

    child.kill().await.map_err(|error| {
        AppError::Process(format!("could not terminate child process: {error}"))
    })?;
    child
        .wait()
        .await
        .map_err(|error| AppError::Process(format!("could not reap child process: {error}")))
}

async fn read_stream(mut stream: impl AsyncRead + Unpin) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

async fn join_output(
    task: tokio::task::JoinHandle<Result<Vec<u8>, std::io::Error>>,
    stream: &str,
) -> Result<Vec<u8>, AppError> {
    task.await
        .map_err(|error| AppError::Process(format!("{stream} reader task failed: {error}")))?
        .map_err(|error| AppError::Process(format!("could not read child {stream}: {error}")))
}

fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: Option<u32>) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;

    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| AppError::io("set replacement permissions", path, error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_unix_mode(_path: &Path, _mode: Option<u32>) -> Result<(), AppError> {
    Ok(())
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    use std::os::unix::process::CommandExt;

    let mut process = Command::new("/bin/sh");
    process.arg("-c").arg(command);
    process.as_std_mut().process_group(0);
    process
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("cmd.exe");
    process.arg("/C").arg(command);
    process
}

#[cfg(not(any(unix, windows)))]
fn shell_command(_command: &str) -> Command {
    Command::new("false")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use crate::domain::action::{Action, ActionKind};

    use super::{ReplayState, replay_action, run_shell_command};

    fn action(id: u64, kind: ActionKind) -> Action {
        Action {
            id,
            original_order: id,
            cwd_before: PathBuf::new(),
            kind,
            replayable: true,
            note: None,
            result: None,
        }
    }

    #[tokio::test]
    async fn environment_set_and_unset_are_replayed() {
        let temp = tempdir().expect("temporary directory should be created");
        fs::write(temp.path().join("tracked.txt"), "fixture").expect("fixture should exist");
        let cancellation = CancellationToken::new();
        let mut state = ReplayState::new(temp.path().to_path_buf(), false);

        replay_action(
            &mut state,
            &action(
                1,
                ActionKind::SetEnvironment {
                    key: "FIXTRACE_REPLAY_TEST".to_owned(),
                    value: "present".to_owned(),
                },
            ),
            Duration::from_secs(5),
            &cancellation,
        )
        .await
        .expect("set environment action should replay");
        let visible = run_shell_command(
            temp.path(),
            PathBuf::new().as_path(),
            state.environment(),
            "test \"$FIXTRACE_REPLAY_TEST\" = present",
            Duration::from_secs(5),
            &cancellation,
        )
        .await
        .expect("environment probe should run");
        assert!(visible.passed());

        replay_action(
            &mut state,
            &action(
                2,
                ActionKind::UnsetEnvironment {
                    key: "FIXTRACE_REPLAY_TEST".to_owned(),
                },
            ),
            Duration::from_secs(5),
            &cancellation,
        )
        .await
        .expect("unset environment action should replay");
        let absent = run_shell_command(
            temp.path(),
            PathBuf::new().as_path(),
            state.environment(),
            "test -z \"${FIXTRACE_REPLAY_TEST+x}\"",
            Duration::from_secs(5),
            &cancellation,
        )
        .await
        .expect("environment probe should run");
        assert!(absent.passed());
        assert_eq!(
            state.environment(),
            &BTreeMap::from([("FIXTRACE_REPLAY_TEST".to_owned(), None)])
        );
    }
}
