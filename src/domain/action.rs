use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::snapshot::SnapshotDelta;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Action {
    pub id: u64,
    pub original_order: u64,
    #[serde(default)]
    pub cwd_before: PathBuf,
    pub kind: ActionKind,
    #[serde(default = "default_replayable")]
    pub replayable: bool,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub result: Option<ActionResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionKind {
    ShellCommand { command: String },
    FilePatch { files: Vec<FileReplacement> },
    SetEnvironment { key: String, value: String },
    UnsetEnvironment { key: String },
    ChangeDirectory { path: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileReplacement {
    pub path: PathBuf,
    /// UTF-8 MVP payload. `None` records deletion.
    pub content: Option<String>,
    #[serde(default)]
    pub unix_mode: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActionResult {
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub stdout_artifact: Option<ArtifactRef>,
    #[serde(default)]
    pub stderr_artifact: Option<ArtifactRef>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub before_snapshot_hash: String,
    pub after_snapshot_hash: String,
    pub delta: SnapshotDelta,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

const fn default_replayable() -> bool {
    true
}
