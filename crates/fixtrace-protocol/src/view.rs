use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{ApprovalView, PageInfo, TaskSummary, TimelineItem, UsageSummary};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatusView {
    Recording,
    ReadyForAnalysis,
    Analyzing,
    Analyzed,
    Cancelled,
    Invalid,
    Archived,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionSummary {
    pub id: Uuid,
    pub project_name: String,
    pub status: SessionStatusView,
    pub active_task_id: Option<Uuid>,
    pub parent_session_id: Option<Uuid>,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ActionView {
    pub id: u64,
    pub original_order: u64,
    pub kind: String,
    pub cwd: String,
    pub summary: String,
    pub replayable: bool,
    pub can_rerun: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TrialClassification {
    StablePass,
    StableFail,
    Flaky,
    Unresolved,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialAttemptView {
    pub index: u32,
    pub passed: Option<bool>,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialView {
    pub id: Uuid,
    pub action_ids: Vec<u64>,
    pub classification: TrialClassification,
    pub attempts: Vec<TrialAttemptView>,
    pub trial_summary: String,
    pub can_rerun: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClassificationView {
    Necessary,
    Removable,
    Uncertain,
    Untested,
    NonReplayable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct EvidenceView {
    pub claim: String,
    pub classification: EvidenceClassificationView,
    pub action_ids: Vec<u64>,
    pub trial_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DiagnosisView {
    pub statement: String,
    pub minimal_action_ids: Vec<u64>,
    pub evidence: Vec<EvidenceView>,
    pub limitations: Vec<String>,
    pub confidence: String,
    pub diagnosis_summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DependencyNodeView {
    pub action_id: u64,
    pub label: String,
    pub in_minimal_set: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DependencyEdgeView {
    pub from_action_id: u64,
    pub to_action_id: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DependencyGraphView {
    pub nodes: Vec<DependencyNodeView>,
    pub edges: Vec<DependencyEdgeView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DiffFileView {
    pub path: String,
    pub change_kind: String,
    pub unified_diff: Option<String>,
    pub artifact_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct DiffView {
    pub files: Vec<DiffFileView>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct SessionView {
    pub summary: SessionSummary,
    pub task: Option<TaskSummary>,
    pub timeline: Vec<TimelineItem>,
    pub actions: Vec<ActionView>,
    pub trials: Vec<TrialView>,
    pub diagnosis: Option<DiagnosisView>,
    pub usage: UsageSummary,
    pub approvals: Vec<ApprovalView>,
    pub dependency_graph: DependencyGraphView,
    pub diff: DiffView,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct SessionSnapshot {
    pub stream_id: Uuid,
    pub through_sequence: u64,
    pub session: SessionView,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionSummary>,
    pub page: PageInfo,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ActionListResponse {
    pub actions: Vec<ActionView>,
    pub page: PageInfo,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialListResponse {
    pub trials: Vec<TrialView>,
    pub page: PageInfo,
}
