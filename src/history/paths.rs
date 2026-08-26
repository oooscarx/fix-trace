use std::{env, fs, path::PathBuf};

use uuid::Uuid;

use crate::error::AppError;

#[derive(Clone, Debug)]
pub struct StatePaths {
    pub database: PathBuf,
    pub config: PathBuf,
    pub sessions: PathBuf,
}

impl StatePaths {
    pub fn discover(override_root: Option<PathBuf>) -> Result<Self, AppError> {
        let root = match override_root {
            Some(path) => path,
            None => env::var_os("FIXTRACE_HOME")
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|home| home.join(".fixtrace")))
                .ok_or_else(|| {
                    AppError::InvalidConfig(
                        "cannot determine state directory; set FIXTRACE_HOME".to_owned(),
                    )
                })?,
        };
        let paths = Self {
            database: root.join("history.sqlite3"),
            config: root.join("config.toml"),
            sessions: root.join("sessions"),
        };
        paths.ensure()?;
        Ok(paths)
    }

    pub fn ensure(&self) -> Result<(), AppError> {
        fs::create_dir_all(&self.sessions)
            .map_err(|error| AppError::io("create FixTrace state directory", &self.sessions, error))
    }

    pub fn session_root(&self, session_id: Uuid) -> PathBuf {
        self.sessions.join(session_id.to_string())
    }
}
