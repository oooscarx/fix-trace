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
    sandbox::local_copy::copy_project,
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
        })
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
            copy_project(&self.baseline, &trial_root, self.include_target)?;
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
        Ok(Trial {
            id: Uuid::new_v4(),
            action_ids,
            repetitions: attempts,
            outcome,
            baseline_hash: self.baseline_hash.clone(),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
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
}
