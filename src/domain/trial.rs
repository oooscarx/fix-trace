use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::action::ActionResult;
use crate::replay::executor::ProcessEvidence;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialOutcome {
    StablePass,
    StableFail,
    Flaky,
    Unresolved,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Trial {
    pub id: Uuid,
    pub action_ids: Vec<u64>,
    pub repetitions: Vec<TrialAttempt>,
    pub outcome: TrialOutcome,
    pub baseline_hash: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrialAttempt {
    pub index: u32,
    pub actions: Vec<ActionResult>,
    pub oracle: Option<ProcessEvidence>,
    pub error: Option<String>,
}
