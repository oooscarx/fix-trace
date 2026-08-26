use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::AppError, llm::usage::UsageSummary, minimize::engine::MinimizationReport};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClassification {
    Necessary,
    Removable,
    Uncertain,
    Untested,
    NonReplayable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceClaim {
    pub claim: String,
    pub classification: EvidenceClassification,
    #[serde(default)]
    pub action_ids: Vec<u64>,
    #[serde(default)]
    pub trial_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Diagnosis {
    pub statement: String,
    pub minimal_action_ids: Vec<u64>,
    pub evidence: Vec<EvidenceClaim>,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub usage: UsageSummary,
}

impl Diagnosis {
    pub fn offline(report: &MinimizationReport) -> Self {
        let evidence = report
            .ablations
            .iter()
            .map(|ablation| EvidenceClaim {
                claim: format!(
                    "Removing action {} produced {:?}",
                    ablation.removed_action_id, ablation.outcome
                ),
                classification: EvidenceClassification::Necessary,
                action_ids: vec![ablation.removed_action_id],
                trial_ids: vec![ablation.trial_id],
            })
            .collect();
        Self {
            statement: report.statement.clone(),
            minimal_action_ids: report.minimal_action_ids.clone(),
            evidence,
            limitations: vec![
                "The result is scoped to the recorded baseline, Oracle, environment, and repetition count."
                    .to_owned(),
                "The result is not a claim of a unique global minimum or philosophical root cause."
                    .to_owned(),
            ],
            usage: UsageSummary::default(),
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self.statement.trim().is_empty() {
            return Err(AppError::Agent(
                "Diagnosis.statement cannot be empty".to_owned(),
            ));
        }
        if self.minimal_action_ids.is_empty() {
            return Err(AppError::Agent(
                "Diagnosis.minimal_action_ids cannot be empty".to_owned(),
            ));
        }
        for evidence in &self.evidence {
            if evidence.claim.trim().is_empty()
                || (evidence.action_ids.is_empty() && evidence.trial_ids.is_empty())
            {
                return Err(AppError::Agent(
                    "every evidence claim must contain text and cite an action or trial".to_owned(),
                ));
            }
        }
        Ok(())
    }
}
