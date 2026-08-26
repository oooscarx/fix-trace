use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    AppErrorView, ApprovalView, ArtifactSummary, EntityRef, Notice, TrialClassification,
    UsageSummary,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Queued,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TimelineItemHeader {
    pub id: Uuid,
    pub status: ItemStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub artifacts: Vec<ArtifactSummary>,
    pub entities: Vec<EntityRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct UserMessageItem {
    pub header: TimelineItemHeader,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct AgentMessageItem {
    pub header: TimelineItemHeader,
    pub text: String,
    pub public_reasoning_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct PlanStepView {
    pub text: String,
    pub status: ItemStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct PlanSummaryItem {
    pub header: TimelineItemHeader,
    pub steps: Vec<PlanStepView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ToolCallItem {
    pub header: TimelineItemHeader,
    pub tool_call_id: String,
    pub name: String,
    pub arguments_summary: String,
    pub result_summary: Option<String>,
    pub selection_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct CommandExecutionItem {
    pub header: TimelineItemHeader,
    pub command: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub stdout_preview: String,
    pub stderr_preview: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct FilePatchEntryView {
    pub path: String,
    pub change_kind: String,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct FilePatchItem {
    pub header: TimelineItemHeader,
    pub files: Vec<FilePatchEntryView>,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct RecordedActionItem {
    pub header: TimelineItemHeader,
    pub action_id: u64,
    pub kind: String,
    pub summary: String,
    pub replayable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialItem {
    pub header: TimelineItemHeader,
    pub trial_id: Uuid,
    pub action_ids: Vec<u64>,
    pub classification: TrialClassification,
    pub repetition_current: Option<u32>,
    pub repetition_total: u32,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct MinimizationItem {
    pub header: TimelineItemHeader,
    pub before_action_ids: Vec<u64>,
    pub candidate_action_ids: Vec<u64>,
    pub removed_action_ids: Vec<u64>,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DiagnosisItem {
    pub header: TimelineItemHeader,
    pub statement: String,
    pub minimal_action_ids: Vec<u64>,
    pub confidence: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct UsageItem {
    pub header: TimelineItemHeader,
    pub usage: UsageSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ApprovalItem {
    pub header: TimelineItemHeader,
    pub approval: ApprovalView,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct NoticeItem {
    pub header: TimelineItemHeader,
    pub notice: Notice,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct ErrorItem {
    pub header: TimelineItemHeader,
    pub error: AppErrorView,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "item", rename_all = "snake_case")]
pub enum TimelineItem {
    UserMessage(UserMessageItem),
    AgentMessage(AgentMessageItem),
    PlanSummary(PlanSummaryItem),
    ToolCall(ToolCallItem),
    CommandExecution(CommandExecutionItem),
    FilePatch(FilePatchItem),
    RecordedAction(RecordedActionItem),
    Trial(TrialItem),
    Minimization(MinimizationItem),
    Diagnosis(DiagnosisItem),
    Approval(Box<ApprovalItem>),
    Usage(UsageItem),
    Notice(NoticeItem),
    Error(ErrorItem),
}

impl TimelineItem {
    pub fn id(&self) -> Option<Uuid> {
        match self {
            Self::UserMessage(item) => Some(item.header.id),
            Self::AgentMessage(item) => Some(item.header.id),
            Self::PlanSummary(item) => Some(item.header.id),
            Self::ToolCall(item) => Some(item.header.id),
            Self::CommandExecution(item) => Some(item.header.id),
            Self::FilePatch(item) => Some(item.header.id),
            Self::RecordedAction(item) => Some(item.header.id),
            Self::Trial(item) => Some(item.header.id),
            Self::Minimization(item) => Some(item.header.id),
            Self::Diagnosis(item) => Some(item.header.id),
            Self::Approval(item) => Some(item.header.id),
            Self::Usage(item) => Some(item.header.id),
            Self::Notice(item) => Some(item.header.id),
            Self::Error(item) => Some(item.header.id),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct AgentMessageDelta {
    pub item_id: Uuid,
    pub text_delta: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct CommandOutputDelta {
    pub item_id: Uuid,
    pub stream: OutputStream,
    pub text_delta: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "delta", rename_all = "snake_case")]
pub enum ItemDelta {
    AgentMessage(AgentMessageDelta),
    CommandOutput(CommandOutputDelta),
    Progress {
        item_id: Uuid,
        progress_ratio: Option<f64>,
        message: String,
    },
}
