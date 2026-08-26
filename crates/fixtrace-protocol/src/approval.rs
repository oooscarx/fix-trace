use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    ReadOnly,
    AskAlways,
    #[default]
    AskForOpaque,
    AutoRecordedSafe,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    ReplayCommand,
    FileMutation,
    NetworkAccess,
    ExternalPath,
    OpaqueAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    Once,
    Task,
    EquivalentForSession,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChoice {
    ApproveOnce,
    ApproveForTask,
    ApproveEquivalentForSession,
    Deny,
    CancelTask,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ApprovalRequest {
    pub id: Uuid,
    pub session_id: Uuid,
    pub task_id: Uuid,
    pub kind: ApprovalKind,
    pub title: String,
    pub reason: String,
    pub risk: RiskLevel,
    pub command_preview: Option<String>,
    pub cwd: Option<PathBuf>,
    pub affected_paths: Vec<PathBuf>,
    pub action_ids: Vec<u64>,
    pub accesses_network: bool,
    pub sandbox_path: Option<PathBuf>,
    pub requested_scope: ApprovalScope,
    pub choices: Vec<ApprovalChoice>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ApprovalResolution {
    pub approval_id: Uuid,
    pub choice: ApprovalChoice,
    pub status: ApprovalStatus,
    pub resolved_by_client_id: Uuid,
    pub resolved_at: DateTime<Utc>,
    pub equivalent_rule_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ApprovalView {
    pub request: ApprovalRequest,
    pub status: ApprovalStatus,
    pub resolution: Option<ApprovalResolution>,
    pub can_approve: bool,
}
