use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Instant,
};

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    domain::{
        action::Action,
        snapshot::SnapshotManifest,
        trial::{Trial, TrialAttempt, TrialOutcome},
    },
    error::AppError,
    progress::{ProgressEvent, ProgressSender},
    sandbox::local_copy::{copy_project, restore_manifest_permissions},
};

use super::{
    executor::{ReplayState, replay_action},
    oracle::{OracleSpec, run_oracle},
};

pub struct TrialRunner {
    baseline: PathBuf,
    baseline_hash: String,
    oracle: OracleSpec,
    repetitions: u32,
    include_target: bool,
    expected_manifest: Option<SnapshotManifest>,
    progress: Option<ProgressSender>,
}

impl TrialRunner {
    pub fn new(
        baseline: PathBuf,
        oracle: OracleSpec,
        repetitions: u32,
        include_target: bool,
    ) -> Result<Self, AppError> {
        if repetitions == 0 {
            return Err(AppError::InvalidConfig(
                "trial repetitions must be positive".to_owned(),
            ));
        }
        let baseline_hash = SnapshotManifest::capture(&baseline, include_target)?.root_hash;
        Ok(Self {
            baseline,
            baseline_hash,
            oracle,
            repetitions,
            include_target,
            expected_manifest: None,
            progress: None,
        })
    }

    pub fn with_manifest(
        baseline: PathBuf,
        manifest: SnapshotManifest,
        oracle: OracleSpec,
        repetitions: u32,
        include_target: bool,
    ) -> Result<Self, AppError> {
        if repetitions == 0 {
            return Err(AppError::InvalidConfig(
                "trial repetitions must be positive".to_owned(),
            ));
        }
        let physical = SnapshotManifest::capture(&baseline, include_target)?;
        if !physical.has_same_content(&manifest) {
            return Err(AppError::BaselineMismatch {
                expected: manifest.root_hash,
                actual: physical.root_hash,
            });
        }
        let baseline_hash = manifest.root_hash.clone();
        Ok(Self {
            baseline,
            baseline_hash,
            oracle,
            repetitions,
            include_target,
            expected_manifest: Some(manifest),
            progress: None,
        })
    }

    pub fn with_progress(mut self, progress: ProgressSender) -> Self {
        self.progress = Some(progress);
        self
    }

    pub fn emit_progress(&self, event: ProgressEvent) {
        if let Some(progress) = &self.progress {
            progress.emit(event);
        }
    }

    pub fn baseline_hash(&self) -> &str {
        &self.baseline_hash
    }

    pub fn baseline(&self) -> &Path {
        &self.baseline
    }

    pub fn include_target(&self) -> bool {
        self.include_target
    }

    pub fn oracle(&self) -> &OracleSpec {
        &self.oracle
    }

    pub fn repetitions(&self) -> u32 {
        self.repetitions
    }

    pub async fn run(
        &self,
        actions: &[Action],
        cancellation: &CancellationToken,
    ) -> Result<Trial, AppError> {
        let started = Instant::now();
        let trial_id = Uuid::new_v4();
        let mut ordered: Vec<_> = actions.iter().collect();
        ordered.sort_by_key(|action| action.original_order);
        ensure_unique_actions(&ordered)?;
        let action_ids = ordered.iter().map(|action| action.id).collect();
        let mut attempts = Vec::new();

        for index in 1..=self.repetitions {
            if cancellation.is_cancelled() {
                break;
            }
            let temporary = tempdir()
                .map_err(|error| AppError::io("create trial temporary directory", ".", error))?;
            let trial_root = temporary.path().join("project");
            self.emit_progress(ProgressEvent::TrialStarted {
                trial_id,
                current: index as usize,
                total: self.repetitions as usize,
            });
            copy_project(&self.baseline, &trial_root, self.include_target)?;
            self.emit_progress(ProgressEvent::BaselineCopied);
            if let Some(manifest) = &self.expected_manifest {
                restore_manifest_permissions(&trial_root, manifest)?;
            }
            let actual_hash =
                SnapshotManifest::capture(&trial_root, self.include_target)?.root_hash;
            if actual_hash != self.baseline_hash {
                return Err(AppError::BaselineMismatch {
                    expected: self.baseline_hash.clone(),
                    actual: actual_hash,
                });
            }
            let mut state = ReplayState::new(trial_root, self.include_target);
            let mut action_results = Vec::new();
            let mut error = None;

            for action in &ordered {
                self.emit_progress(ProgressEvent::ActionReplayStarted {
                    action_id: action.id,
                });
                match replay_action(&mut state, action, self.oracle.timeout(), cancellation).await {
                    Ok(result) if result.cancelled => {
                        action_results.push(result);
                        break;
                    }
                    Ok(result) => action_results.push(result),
                    Err(problem) => {
                        error = Some(problem.to_string());
                        break;
                    }
                }
            }

            let oracle = if error.is_none() && !cancellation.is_cancelled() {
                self.emit_progress(ProgressEvent::OracleAttemptStarted {
                    current: index,
                    total: self.repetitions,
                });
                match run_oracle(&self.oracle, &state, cancellation).await {
                    Ok(evidence) => Some(evidence),
                    Err(problem) => {
                        error = Some(problem.to_string());
                        None
                    }
                }
            } else {
                None
            };
            attempts.push(TrialAttempt {
                index,
                actions: action_results,
                oracle,
                error,
            });

            if cancellation.is_cancelled()
                || attempts
                    .last()
                    .is_some_and(|attempt| attempt.error.is_some())
            {
                break;
            }
        }

        let outcome = classify_outcome(&attempts, self.repetitions, cancellation.is_cancelled());
        let trial = Trial {
            id: trial_id,
            action_ids,
            repetitions: attempts,
            outcome,
            baseline_hash: self.baseline_hash.clone(),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        self.emit_progress(ProgressEvent::TrialCompleted {
            trial_id,
            outcome: trial.outcome.clone(),
        });
        Ok(trial)
    }
}

fn ensure_unique_actions(actions: &[&Action]) -> Result<(), AppError> {
    let mut ids = BTreeSet::new();
    let mut orders = BTreeSet::new();
    for action in actions {
        if !ids.insert(action.id) || !orders.insert(action.original_order) {
            return Err(AppError::Process(format!(
                "candidate contains duplicate action id/order at action {}",
                action.id
            )));
        }
    }
    Ok(())
}

fn classify_outcome(
    attempts: &[TrialAttempt],
    expected_repetitions: u32,
    cancelled: bool,
) -> TrialOutcome {
    if cancelled {
        return TrialOutcome::Cancelled;
    }
    if attempts.len() != expected_repetitions as usize
        || attempts.iter().any(|attempt| attempt.error.is_some())
        || attempts.iter().any(|attempt| {
            attempt
                .oracle
                .as_ref()
                .is_none_or(|oracle| oracle.timed_out || oracle.cancelled)
        })
    {
        return TrialOutcome::Unresolved;
    }
    let passes = attempts
        .iter()
        .filter(|attempt| {
            attempt
                .oracle
                .as_ref()
                .is_some_and(|oracle| oracle.passed())
        })
        .count();
    if passes == attempts.len() {
        TrialOutcome::StablePass
    } else if passes == 0 {
        TrialOutcome::StableFail
    } else {
        TrialOutcome::Flaky
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{fs, time::Duration};

    #[cfg(unix)]
    use tempfile::tempdir;
    #[cfg(unix)]
    use tokio_util::sync::CancellationToken;

    use crate::replay::executor::ProcessEvidence;

    use super::{TrialAttempt, TrialOutcome, classify_outcome};

    fn attempt(index: u32, passed: bool) -> TrialAttempt {
        TrialAttempt {
            index,
            actions: Vec::new(),
            oracle: Some(ProcessEvidence {
                exit_code: Some(if passed { 0 } else { 1 }),
                duration_ms: 1,
                stdout: String::new(),
                stderr: String::new(),
                stdout_artifact: None,
                stderr_artifact: None,
                timed_out: false,
                cancelled: false,
            }),
            error: None,
        }
    }

    #[test]
    fn mixed_attempts_are_flaky_not_pass() {
        let attempts = [attempt(1, true), attempt(2, false), attempt(3, true)];

        assert_eq!(classify_outcome(&attempts, 3, false), TrialOutcome::Flaky);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_stops_a_long_running_trial() {
        use crate::replay::{oracle::OracleSpec, runner::TrialRunner};

        let baseline = tempdir().expect("temporary baseline should be created");
        fs::write(baseline.path().join("fixture.txt"), "fixture")
            .expect("fixture should be written");
        let runner = TrialRunner::new(
            baseline.path().to_path_buf(),
            OracleSpec {
                command: "sleep 30".to_owned(),
                timeout_ms: 60_000,
            },
            1,
            false,
        )
        .expect("runner should be created");
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            trigger.cancel();
        });

        let trial = runner
            .run(&[], &cancellation)
            .await
            .expect("cancelled trial should still be recorded");

        assert_eq!(trial.outcome, TrialOutcome::Cancelled);
        assert!(trial.duration_ms < 5_000);
    }
}
