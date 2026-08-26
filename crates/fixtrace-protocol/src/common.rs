use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

impl PageRequest {
    pub fn validated_limit(&self) -> u32 {
        self.limit
            .unwrap_or(crate::DEFAULT_PAGE_LIMIT)
            .clamp(1, crate::MAX_PAGE_LIMIT)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct PageInfo {
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Session,
    Task,
    Action,
    Trial,
    ToolCall,
    Approval,
    Artifact,
    Diagnosis,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct EntityRef {
    pub kind: EntityKind,
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ArtifactSummary {
    pub id: Uuid,
    pub session_id: Uuid,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ArtifactChunk {
    pub artifact_id: Uuid,
    pub offset: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub bytes_base64: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub token_limit: u64,
    pub cost_limit_usd: f64,
    pub budget_ratio: f64,
    pub exact: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct BudgetWarning {
    pub usage: UsageSummary,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
    Success,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct Notice {
    pub code: String,
    pub level: NoticeLevel,
    pub title: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct PublicConfigSummary {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub api_style: String,
    pub context_length: u64,
    pub reasoning_mode: String,
    pub replay_repetitions: u32,
    pub oracle_timeout_secs: u64,
    pub has_api_key: bool,
    pub approval_policy: crate::ApprovalPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct EmptyRequest {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionIdRequest {
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TaskIdRequest {
    pub task_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionPageRequest {
    pub session_id: Uuid,
    pub page: PageRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct LocalPath {
    pub path: PathBuf,
}
