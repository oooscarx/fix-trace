use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },

    #[error("failed to read configuration {path}: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("failed to parse configuration: {0}")]
    ParseConfig(#[from] toml::de::Error),

    #[error("failed to serialize configuration: {0}")]
    SerializeConfig(#[from] toml::ser::Error),

    #[error("failed to parse JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("failed to walk project tree: {0}")]
    WalkDirectory(#[from] walkdir::Error),

    #[error("unsafe project-relative path `{path}`: {reason}")]
    UnsafePath { path: PathBuf, reason: String },

    #[error("invalid project directory {path}: {reason}")]
    InvalidProject { path: PathBuf, reason: String },

    #[error("baseline hash mismatch: expected {expected}, found {actual}")]
    BaselineMismatch { expected: String, actual: String },

    #[error("action {action_id} is not replayable: {reason}")]
    NonReplayable { action_id: u64, reason: String },

    #[error("action {action_id} expected cwd `{expected}` but replay is in `{actual}`")]
    WorkingDirectoryMismatch {
        action_id: u64,
        expected: PathBuf,
        actual: PathBuf,
    },

    #[error("process execution failed: {0}")]
    Process(String),

    #[error("demo verification failed: {0}")]
    DemoVerification(String),

    #[error("the `{0}` command is planned but not implemented in this milestone")]
    NotImplemented(String),
}

impl AppError {
    pub fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}
