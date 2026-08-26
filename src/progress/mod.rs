pub mod renderer;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
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
    AgentMessageStarted {
        item_id: Uuid,
    },
    AgentTextDelta {
        item_id: Uuid,
        text_delta: String,
    },
    AgentMessageCompleted {
        item_id: Uuid,
        text: String,
    },
    ToolCallStarted {
        item_id: Uuid,
        tool_call_id: String,
        name: String,
        arguments_summary: String,
    },
    ToolCallCompleted {
        item_id: Uuid,
        tool_call_id: String,
        name: String,
        arguments_summary: String,
        result_summary: String,
    },
    UsageUpdated {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    },
    BudgetExceeded {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    },
    Cancelled,
    Finished,
}

#[derive(Clone)]
pub struct ProgressSender {
    sender: broadcast::Sender<ProgressEvent>,
    observer: Option<ProgressObserver>,
}

type ProgressObserver = Arc<dyn Fn(&ProgressEvent) + Send + Sync>;

impl ProgressSender {
    pub fn channel(capacity: usize) -> (Self, broadcast::Receiver<ProgressEvent>) {
        let (sender, receiver) = broadcast::channel(capacity);
        (
            Self {
                sender,
                observer: None,
            },
            receiver,
        )
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProgressEvent> {
        self.sender.subscribe()
    }

    pub fn with_observer(&self, observer: impl Fn(&ProgressEvent) + Send + Sync + 'static) -> Self {
        Self {
            sender: self.sender.clone(),
            observer: Some(Arc::new(observer)),
        }
    }

    pub fn emit(&self, event: ProgressEvent) {
        if let Some(observer) = &self.observer {
            observer(&event);
        }
        let _ignored = self.sender.send(event);
    }
}
