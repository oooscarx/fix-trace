use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::diagnosis::Diagnosis,
    domain::{action::Action, trial::Trial},
    error::AppError,
    history::database::HistoryDatabase,
    llm::{provider::ToolDefinition, usage::UsageSummary},
    minimize::engine::MinimizationReport,
    replay::runner::TrialRunner,
};

#[async_trait]
pub trait AgentToolExecutor: Send {
    fn definitions(&self) -> Vec<ToolDefinition>;

    fn validate_diagnosis(&self, _diagnosis: &Diagnosis) -> Result<(), AppError> {
        Ok(())
    }

    async fn execute(
        &mut self,
        name: &str,
        arguments: Value,
        usage: &UsageSummary,
    ) -> Result<Value, AppError>;
}

pub struct AnalysisTools<'a> {
    runner: &'a TrialRunner,
    actions: BTreeMap<u64, Action>,
    report: &'a MinimizationReport,
    trials: BTreeMap<Uuid, Trial>,
    database: Option<&'a HistoryDatabase>,
    session_id: Option<Uuid>,
    cancellation: CancellationToken,
}

impl<'a> AnalysisTools<'a> {
    pub fn new(
        runner: &'a TrialRunner,
        actions: &'a [Action],
        report: &'a MinimizationReport,
        database: Option<&'a HistoryDatabase>,
        session_id: Option<Uuid>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            runner,
            actions: actions
                .iter()
                .cloned()
                .map(|action| (action.id, action))
                .collect(),
            report,
            trials: report
                .trials
                .iter()
                .cloned()
                .map(|trial| (trial.id, trial))
                .collect(),
            database,
            session_id,
            cancellation,
        }
    }

    async fn run_ids(&mut self, ids: Vec<u64>) -> Result<Trial, AppError> {
        let mut actions = Vec::new();
        for id in ids {
            let action = self.actions.get(&id).cloned().ok_or_else(|| {
                AppError::Agent(format!("run_candidate references unknown action {id}"))
            })?;
            actions.push(action);
        }
        let trial = self.runner.run(&actions, &self.cancellation).await?;
        if let (Some(database), Some(session_id)) = (self.database, self.session_id) {
            database.save_trial(session_id, &trial)?;
        }
        self.trials.insert(trial.id, trial.clone());
        Ok(trial)
    }
}

#[async_trait]
impl AgentToolExecutor for AnalysisTools<'_> {
    fn definitions(&self) -> Vec<ToolDefinition> {
        tool_definitions()
    }

    fn validate_diagnosis(&self, diagnosis: &Diagnosis) -> Result<(), AppError> {
        for action_id in diagnosis.minimal_action_ids.iter().chain(
            diagnosis
                .evidence
                .iter()
                .flat_map(|claim| &claim.action_ids),
        ) {
            if !self.actions.contains_key(action_id) {
                return Err(AppError::Agent(format!(
                    "Diagnosis cites unknown action {action_id}"
                )));
            }
        }
        for trial_id in diagnosis.evidence.iter().flat_map(|claim| &claim.trial_ids) {
            if !self.trials.contains_key(trial_id) {
                return Err(AppError::Agent(format!(
                    "Diagnosis cites unknown trial {trial_id}"
                )));
            }
        }
        Ok(())
    }

    async fn execute(
        &mut self,
        name: &str,
        arguments: Value,
        usage: &UsageSummary,
    ) -> Result<Value, AppError> {
        match name {
            "get_session_summary" => Ok(json!({
                "baseline_hash": self.report.baseline_hash,
                "action_count": self.actions.len(),
                "trial_count": self.trials.len(),
                "minimal_action_ids": self.report.minimal_action_ids,
                "statement": self.report.statement,
            })),
            "list_actions" => Ok(Value::Array(
                self.actions
                    .values()
                    .map(|action| {
                        json!({
                            "id": action.id,
                            "original_order": action.original_order,
                            "kind": action.kind,
                            "replayable": action.replayable,
                            "note": action.note,
                        })
                    })
                    .collect(),
            )),
            "inspect_action" => {
                let arguments: ActionIdArgs = serde_json::from_value(arguments)?;
                self.actions
                    .get(&arguments.action_id)
                    .map(serde_json::to_value)
                    .transpose()?
                    .ok_or_else(|| {
                        AppError::Agent(format!("unknown action {}", arguments.action_id))
                    })
            }
            "inspect_state_delta" => {
                let arguments: ActionIdArgs = serde_json::from_value(arguments)?;
                let action = self.actions.get(&arguments.action_id).ok_or_else(|| {
                    AppError::Agent(format!("unknown action {}", arguments.action_id))
                })?;
                Ok(json!({
                    "action_id": action.id,
                    "delta": action.result.as_ref().map(|result| &result.delta),
                }))
            }
            "get_dependency_graph" => Ok(serde_json::to_value(&self.report.dependency_graph)?),
            "run_candidate" => {
                let arguments: CandidateArgs = serde_json::from_value(arguments)?;
                Ok(serde_json::to_value(
                    self.run_ids(arguments.action_ids).await?,
                )?)
            }
            "repeat_trial" => {
                let arguments: TrialIdArgs = serde_json::from_value(arguments)?;
                let action_ids = self
                    .trials
                    .get(&arguments.trial_id)
                    .map(|trial| trial.action_ids.clone())
                    .ok_or_else(|| {
                        AppError::Agent(format!("unknown trial {}", arguments.trial_id))
                    })?;
                Ok(serde_json::to_value(self.run_ids(action_ids).await?)?)
            }
            "compare_trials" => {
                let arguments: CompareArgs = serde_json::from_value(arguments)?;
                let left = self.trials.get(&arguments.left_trial_id).ok_or_else(|| {
                    AppError::Agent(format!("unknown trial {}", arguments.left_trial_id))
                })?;
                let right = self.trials.get(&arguments.right_trial_id).ok_or_else(|| {
                    AppError::Agent(format!("unknown trial {}", arguments.right_trial_id))
                })?;
                Ok(json!({
                    "left": {"id": left.id, "action_ids": left.action_ids, "outcome": left.outcome},
                    "right": {"id": right.id, "action_ids": right.action_ids, "outcome": right.outcome},
                    "same_outcome": left.outcome == right.outcome,
                }))
            }
            "run_minimizer" => Ok(json!({
                "minimal_action_ids": self.report.minimal_action_ids,
                "final_trial_id": self.report.final_trial.id,
                "final_outcome": self.report.final_trial.outcome,
                "ablations": self.report.ablations,
                "statement": self.report.statement,
            })),
            "get_usage_summary" => Ok(serde_json::to_value(usage)?),
            _ => Err(AppError::Agent(format!("unknown registered tool `{name}`"))),
        }
    }
}

#[derive(Deserialize)]
struct ActionIdArgs {
    action_id: u64,
}

#[derive(Deserialize)]
struct CandidateArgs {
    action_ids: Vec<u64>,
}

#[derive(Deserialize)]
struct TrialIdArgs {
    trial_id: Uuid,
}

#[derive(Deserialize)]
struct CompareArgs {
    left_trial_id: Uuid,
    right_trial_id: Uuid,
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        definition(
            "get_session_summary",
            "Summarize the current recorded session and verified minimization.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        definition(
            "list_actions",
            "List recorded actions in original order.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        definition(
            "inspect_action",
            "Inspect one recorded action and its evidence.",
            id_schema("action_id"),
        ),
        definition(
            "inspect_state_delta",
            "Inspect the file state delta for one action.",
            id_schema("action_id"),
        ),
        definition(
            "get_dependency_graph",
            "Return inferred resources and hard dependencies.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        definition(
            "run_candidate",
            "Replay only existing action IDs from a fresh baseline.",
            json!({"type":"object","properties":{"action_ids":{"type":"array","items":{"type":"integer","minimum":1}}},"required":["action_ids"],"additionalProperties":false}),
        ),
        definition(
            "repeat_trial",
            "Repeat a previous trial candidate from a fresh baseline.",
            id_schema("trial_id"),
        ),
        definition(
            "compare_trials",
            "Compare outcomes and action sets for two trials.",
            json!({"type":"object","properties":{"left_trial_id":{"type":"string","format":"uuid"},"right_trial_id":{"type":"string","format":"uuid"}},"required":["left_trial_id","right_trial_id"],"additionalProperties":false}),
        ),
        definition(
            "run_minimizer",
            "Return the verified dependency-aware minimizer result.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        definition(
            "get_usage_summary",
            "Return current model token and cost usage.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
    ]
}

fn definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
    }
}

fn id_schema(name: &str) -> Value {
    if name == "trial_id" {
        json!({
            "type": "object",
            "properties": {"trial_id": {"type":"string","format":"uuid"}},
            "required": ["trial_id"],
            "additionalProperties": false
        })
    } else {
        json!({
            "type": "object",
            "properties": {"action_id": {"type":"integer","minimum":1}},
            "required": ["action_id"],
            "additionalProperties": false
        })
    }
}
