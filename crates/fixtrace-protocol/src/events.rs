use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    AppErrorView, ApprovalRequest, ApprovalResolution, ArtifactSummary, BudgetWarning,
    DiagnosisView, ItemDelta, Notice, SessionSummary, TaskFailure, TaskProgress, TaskResult,
    TaskSummary, TimelineItem, UsageSummary,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SubscribeRequest {
    pub session_id: Uuid,
    pub after_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct UnsubscribeRequest {
    pub subscription_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SubscriptionStarted {
    pub subscription_id: Uuid,
    pub stream_id: Uuid,
    pub after_sequence: u64,
    pub high_watermark: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct EventGap {
    pub stream_id: Uuid,
    pub expected_sequence: u64,
    pub available_from_sequence: u64,
    pub high_watermark: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AppEvent {
    SessionCreated(SessionSummary),
    SessionUpdated(SessionSummary),
    TaskStarted(TaskSummary),
    TaskProgress(TaskProgress),
    TaskCompleted(TaskResult),
    TaskFailed(TaskFailure),
    TaskCancelled(TaskSummary),
    ItemStarted(TimelineItem),
    ItemDelta(ItemDelta),
    ItemCompleted(TimelineItem),
    ApprovalRequested(ApprovalRequest),
    ApprovalResolved(ApprovalResolution),
    UsageUpdated(UsageSummary),
    BudgetWarning(BudgetWarning),
    DiagnosisUpdated(DiagnosisView),
    ArtifactCreated(ArtifactSummary),
    Notice(Notice),
    Error(AppErrorView),
    EventGap(EventGap),
}

impl AppEvent {
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::SessionCreated(_) => "session_created",
            Self::SessionUpdated(_) => "session_updated",
            Self::TaskStarted(_) => "task_started",
            Self::TaskProgress(_) => "task_progress",
            Self::TaskCompleted(_) => "task_completed",
            Self::TaskFailed(_) => "task_failed",
            Self::TaskCancelled(_) => "task_cancelled",
            Self::ItemStarted(_) => "item_started",
            Self::ItemDelta(_) => "item_delta",
            Self::ItemCompleted(_) => "item_completed",
            Self::ApprovalRequested(_) => "approval_requested",
            Self::ApprovalResolved(_) => "approval_resolved",
            Self::UsageUpdated(_) => "usage_updated",
            Self::BudgetWarning(_) => "budget_warning",
            Self::DiagnosisUpdated(_) => "diagnosis_updated",
            Self::ArtifactCreated(_) => "artifact_created",
            Self::Notice(_) => "notice",
            Self::Error(_) => "error",
            Self::EventGap(_) => "event_gap",
        }
    }

    pub const fn should_persist_immediately(&self) -> bool {
        !matches!(self, Self::ItemDelta(_) | Self::TaskProgress(_))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct EventEnvelope {
    pub schema_version: u16,
    pub stream_id: Uuid,
    pub sequence: u64,
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub payload: AppEvent,
}

impl EventEnvelope {
    pub fn validate(&self) -> Result<(), AppErrorView> {
        if self.schema_version != crate::EVENT_SCHEMA_VERSION {
            return Err(AppErrorView::new(
                crate::ErrorCode::InvalidRequest,
                format!("unsupported event schema version {}", self.schema_version),
            ));
        }
        if self.sequence == 0 || self.sequence > crate::MAX_SAFE_WIRE_INTEGER {
            return Err(AppErrorView::new(
                crate::ErrorCode::InvalidRequest,
                "event sequence is outside the wire-safe range",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct EventBatch {
    pub events: Vec<EventEnvelope>,
    pub high_watermark: u64,
    pub gap: Option<EventGap>,
}
