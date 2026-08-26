use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    domain::{
        action::Action,
        snapshot::SnapshotManifest,
        trial::{Trial, TrialOutcome},
    },
    error::AppError,
    progress::ProgressEvent,
    replay::runner::TrialRunner,
};

use super::{
    cache::{TrialCache, TrialCacheKey},
    ddmin::{CandidateTester, ddmin},
    dependency::DependencyGraph,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AblationEvidence {
    pub removed_action_id: u64,
    pub candidate_action_ids: Vec<u64>,
    pub trial_id: Uuid,
    pub outcome: TrialOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MinimizationReport {
    pub baseline_hash: String,
    pub empty_trial: Trial,
    pub full_trial: Trial,
    pub minimal_action_ids: Vec<u64>,
    pub ablations: Vec<AblationEvidence>,
    pub final_trial: Trial,
    pub trials: Vec<Trial>,
    pub dependency_graph: DependencyGraph,
    pub statement: String,
}

pub async fn minimize(
    runner: &TrialRunner,
    actions: &[Action],
    cancellation: &CancellationToken,
) -> Result<MinimizationReport, AppError> {
    let baseline = SnapshotManifest::capture(runner.baseline(), runner.include_target())?;
    let dependency_graph = DependencyGraph::infer(actions, &baseline);
    minimize_with_graph(runner, actions, dependency_graph, cancellation).await
}

pub async fn minimize_with_graph(
    runner: &TrialRunner,
    actions: &[Action],
    dependency_graph: DependencyGraph,
    cancellation: &CancellationToken,
) -> Result<MinimizationReport, AppError> {
    let mut context = EvaluationContext::new(runner, actions, dependency_graph, cancellation)?;
    let empty_ids = BTreeSet::new();
    let empty_trial = context.evaluate(&empty_ids).await?;
    require_outcome("empty action set", &empty_trial, TrialOutcome::StableFail)?;

    let full_ids: BTreeSet<_> = actions.iter().map(|action| action.id).collect();
    let full_trial = context.evaluate(&full_ids).await?;
    require_outcome("full action set", &full_trial, TrialOutcome::StablePass)?;

    let mut minimal = ddmin(&mut context, full_ids).await?;
    loop {
        let mut reduced = false;
        for action_id in context.ordered_ids(&minimal) {
            let candidate = context
                .dependency_graph
                .remove_with_dependents(&minimal, action_id);
            if candidate.len() >= minimal.len() {
                continue;
            }
            if context.evaluate(&candidate).await?.outcome == TrialOutcome::StablePass {
                context
                    .runner
                    .emit_progress(ProgressEvent::CandidateReduced {
                        before: minimal.len(),
                        after: candidate.len(),
                    });
                minimal = candidate;
                reduced = true;
                break;
            }
        }
        if !reduced {
            break;
        }
    }

    let mut ablations = Vec::new();
    for action_id in context.ordered_ids(&minimal) {
        let candidate = context
            .dependency_graph
            .remove_with_dependents(&minimal, action_id);
        let trial = context.evaluate(&candidate).await?;
        if trial.outcome == TrialOutcome::StablePass {
            return Err(AppError::Minimization(format!(
                "action {action_id} remained removable after the ablation pass"
            )));
        }
        ablations.push(AblationEvidence {
            removed_action_id: action_id,
            candidate_action_ids: trial.action_ids.clone(),
            trial_id: trial.id,
            outcome: trial.outcome,
        });
    }

    let final_trial = context.evaluate_uncached(&minimal).await?;
    require_outcome(
        "final minimized action set",
        &final_trial,
        TrialOutcome::StablePass,
    )?;
    let minimal_action_ids = context.ordered_ids(&minimal);
    Ok(MinimizationReport {
        baseline_hash: runner.baseline_hash().to_owned(),
        empty_trial,
        full_trial,
        minimal_action_ids,
        ablations,
        final_trial,
        trials: context.trials,
        dependency_graph: context.dependency_graph,
        statement: "dependency-constrained 1-minimal sufficient repair trace".to_owned(),
    })
}

struct EvaluationContext<'a> {
    runner: &'a TrialRunner,
    actions: BTreeMap<u64, &'a Action>,
    dependency_graph: DependencyGraph,
    cache: TrialCache,
    trials: Vec<Trial>,
    cancellation: &'a CancellationToken,
}

impl<'a> EvaluationContext<'a> {
    fn new(
        runner: &'a TrialRunner,
        actions: &'a [Action],
        dependency_graph: DependencyGraph,
        cancellation: &'a CancellationToken,
    ) -> Result<Self, AppError> {
        let action_map: BTreeMap<_, _> = actions.iter().map(|action| (action.id, action)).collect();
        if action_map.len() != actions.len() {
            return Err(AppError::Minimization(
                "action IDs must be unique before minimization".to_owned(),
            ));
        }
        Ok(Self {
            runner,
            actions: action_map,
            dependency_graph,
            cache: TrialCache::default(),
            trials: Vec::new(),
            cancellation,
        })
    }

    async fn evaluate(&mut self, candidate: &BTreeSet<u64>) -> Result<Trial, AppError> {
        let candidate = self.normalize(candidate);
        let action_ids = self.ordered_ids(&candidate);
        let key = TrialCacheKey::new(
            self.runner.baseline_hash(),
            self.runner.oracle(),
            action_ids,
            self.runner.repetitions(),
        );
        if let Some(trial) = self.cache.get(&key) {
            return Ok(trial.clone());
        }
        let trial = self.run_candidate(&candidate).await?;
        self.cache.insert(key, trial.clone());
        self.trials.push(trial.clone());
        Ok(trial)
    }

    async fn evaluate_uncached(&mut self, candidate: &BTreeSet<u64>) -> Result<Trial, AppError> {
        let trial = self.run_candidate(candidate).await?;
        self.trials.push(trial.clone());
        Ok(trial)
    }

    async fn run_candidate(&self, candidate: &BTreeSet<u64>) -> Result<Trial, AppError> {
        let actions: Result<Vec<_>, _> = self
            .ordered_ids(candidate)
            .into_iter()
            .map(|id| {
                self.actions
                    .get(&id)
                    .map(|action| (*action).clone())
                    .ok_or_else(|| {
                        AppError::Minimization(format!("candidate references unknown action {id}"))
                    })
            })
            .collect();
        self.runner.run(&actions?, self.cancellation).await
    }
}

#[async_trait]
impl CandidateTester for EvaluationContext<'_> {
    fn normalize(&self, candidate: &BTreeSet<u64>) -> BTreeSet<u64> {
        self.dependency_graph
            .closure(candidate)
            .into_iter()
            .filter(|id| self.actions.contains_key(id))
            .collect()
    }

    fn ordered_ids(&self, candidate: &BTreeSet<u64>) -> Vec<u64> {
        let mut actions: Vec<_> = candidate
            .iter()
            .filter_map(|id| self.actions.get(id))
            .collect();
        actions.sort_by_key(|action| action.original_order);
        actions.into_iter().map(|action| action.id).collect()
    }

    async fn outcome(&mut self, candidate: &BTreeSet<u64>) -> Result<TrialOutcome, AppError> {
        Ok(self.evaluate(candidate).await?.outcome)
    }
}

fn require_outcome(label: &str, trial: &Trial, expected: TrialOutcome) -> Result<(), AppError> {
    if trial.outcome != expected {
        return Err(AppError::Minimization(format!(
            "{label} must be {expected:?}, got {:?} in trial {}",
            trial.outcome, trial.id
        )));
    }
    Ok(())
}
