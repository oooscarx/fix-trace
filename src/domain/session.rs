use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::snapshot::SnapshotManifest;
use crate::replay::oracle::OracleSpec;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Recording,
    ReadyForAnalysis,
    Analyzed,
    Cancelled,
    Invalid,
}

impl SessionStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::ReadyForAnalysis => "ready_for_analysis",
            Self::Analyzed => "analyzed",
            Self::Cancelled => "cancelled",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionRecord {
    pub id: Uuid,
    #[serde(default)]
    pub parent_session_id: Option<Uuid>,
    #[serde(default)]
    pub archived: bool,
    pub project_name: String,
    pub original_project: PathBuf,
    pub baseline_path: PathBuf,
    pub worktree_path: PathBuf,
    pub oracle: OracleSpec,
    pub baseline_manifest: SnapshotManifest,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
