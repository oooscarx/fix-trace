pub mod renderer;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::domain::trial::TrialOutcome;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    SessionCreated {
        session_id: Uuid,
    },
    BaselineCopied,
    OracleAttemptStarted {
        current: u32,
        total: u32,
    },
    ActionReplayStarted {
        action_id: u64,
    },
    TrialStarted {
        trial_id: Uuid,
        current: usize,
        total: usize,
    },
    TrialCompleted {
        trial_id: Uuid,
        outcome: TrialOutcome,
    },
    CandidateReduced {
        before: usize,
        after: usize,
    },
    AgentStepStarted {
        step: usize,
    },
    UsageUpdated {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    },
    Cancelled,
    Finished,
}

#[derive(Clone)]
pub struct ProgressSender {
    sender: mpsc::Sender<ProgressEvent>,
}

impl ProgressSender {
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<ProgressEvent>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }

    pub fn emit(&self, event: ProgressEvent) {
        let _ignored = self.sender.try_send(event);
    }
}
