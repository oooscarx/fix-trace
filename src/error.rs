use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("failed to read configuration {path}: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("failed to parse configuration: {0}")]
    ParseConfig(#[from] toml::de::Error),

    #[error("failed to serialize configuration: {0}")]
    SerializeConfig(#[from] toml::ser::Error),

    #[error("the `{0}` command is planned but not implemented in this milestone")]
    NotImplemented(String),
}
