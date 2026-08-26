use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;
use uuid::Uuid;

use crate::AppErrorView;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    AgentTurn,
    RecordTrace,
    VerifyBaseline,
    ReplayFullTrace,
    AnalyzeMinimalTrace,
    RepeatTrial,
    GenerateDiagnosis,
    ExportSession,
    Demo,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    WaitingForApproval,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    Interrupted,
}

impl TaskStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Completed | Self::Failed | Self::Interrupted
        )
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Queued,
                Self::Running | Self::Cancelled | Self::Failed | Self::Interrupted,
            ) | (
                Self::Running,
                Self::WaitingForApproval
                    | Self::Cancelling
                    | Self::Completed
                    | Self::Failed
                    | Self::Interrupted
            ) | (
                Self::WaitingForApproval,
                Self::Running | Self::Cancelling | Self::Failed | Self::Interrupted
            ) | (
                Self::Cancelling,
                Self::Cancelled | Self::Failed | Self::Interrupted
            )
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct TaskSummary {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub operation_id: Uuid,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub progress_ratio: Option<f64>,
    pub is_cancellable: bool,
    pub supports_steer: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct TaskProgress {
    pub task: TaskSummary,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct TaskResult {
    pub task: TaskSummary,
    pub output: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct TaskFailure {
    pub task: TaskSummary,
    pub error: AppErrorView,
}

#[cfg(test)]
mod tests {
    use super::TaskStatus;

    #[test]
    fn task_state_machine_accepts_only_documented_transitions() {
        assert!(TaskStatus::Queued.can_transition_to(TaskStatus::Running));
        assert!(TaskStatus::Running.can_transition_to(TaskStatus::WaitingForApproval));
        assert!(TaskStatus::WaitingForApproval.can_transition_to(TaskStatus::Running));
        assert!(TaskStatus::Running.can_transition_to(TaskStatus::Cancelling));
        assert!(TaskStatus::Cancelling.can_transition_to(TaskStatus::Cancelled));
        assert!(TaskStatus::Running.can_transition_to(TaskStatus::Interrupted));
        assert!(TaskStatus::Queued.can_transition_to(TaskStatus::Interrupted));

        assert!(!TaskStatus::Queued.can_transition_to(TaskStatus::Completed));
        assert!(!TaskStatus::Completed.can_transition_to(TaskStatus::Running));
        assert!(!TaskStatus::Cancelled.can_transition_to(TaskStatus::Failed));
    }
}
