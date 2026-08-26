use std::{fs, path::PathBuf};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    domain::{
        action::{Action, ActionResult, ArtifactRef},
        session::SessionRecord,
        trial::Trial,
    },
    error::AppError,
    progress::ProgressEvent,
};

#[derive(Clone, Debug)]
pub struct HistoryDatabase {
    path: PathBuf,
}

impl HistoryDatabase {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, AppError> {
        let database = Self { path: path.into() };
        database.initialize()?;
        Ok(database)
    }

    pub fn save_session(&self, session: &SessionRecord) -> Result<(), AppError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO sessions (id, status, created_at, updated_at, session_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                status=excluded.status,
                updated_at=excluded.updated_at,
                session_json=excluded.session_json",
            params![
                session.id.to_string(),
                session.status.as_str(),
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                serde_json::to_string(session)?,
            ],
        )?;
        Ok(())
    }

    pub fn session_exists(&self, session_id: Uuid) -> Result<bool, AppError> {
        let connection = self.connection()?;
        let exists = connection
            .query_row(
                "SELECT 1 FROM sessions WHERE id=?1",
                [session_id.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        Ok(exists)
    }

    pub fn load_session(&self, session_id: Uuid) -> Result<SessionRecord, AppError> {
        let connection = self.connection()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT session_json FROM sessions WHERE id=?1",
                [session_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value))
            .transpose()?
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT session_json FROM sessions ORDER BY created_at DESC, id DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(serde_json::from_str(&row?)?);
        }
        Ok(sessions)
    }

    pub fn save_action(&self, session_id: Uuid, action: &Action) -> Result<(), AppError> {
        let mut action = action.clone();
        let artifact_dir = self.artifact_dir(session_id)?;
        if let Some(result) = &mut action.result {
            externalize_action_result(result, &artifact_dir, &format!("action-{}", action.id))?;
        }
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO actions (session_id, action_id, original_order, action_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, action_id) DO UPDATE SET
                original_order=excluded.original_order,
                action_json=excluded.action_json",
            params![
                session_id.to_string(),
                action.id,
                action.original_order,
                serde_json::to_string(&action)?,
            ],
        )?;
        Ok(())
    }

    pub fn load_actions(&self, session_id: Uuid) -> Result<Vec<Action>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT action_json FROM actions WHERE session_id=?1 ORDER BY original_order",
        )?;
        let rows = statement.query_map([session_id.to_string()], |row| row.get::<_, String>(0))?;
        let mut actions = Vec::new();
        for row in rows {
            actions.push(serde_json::from_str(&row?)?);
        }
        Ok(actions)
    }

    pub fn save_trial(&self, session_id: Uuid, trial: &Trial) -> Result<(), AppError> {
        let mut trial = trial.clone();
        let artifact_dir = self.artifact_dir(session_id)?;
        for attempt in &mut trial.repetitions {
            for (index, result) in attempt.actions.iter_mut().enumerate() {
                externalize_action_result(
                    result,
                    &artifact_dir,
                    &format!(
                        "trial-{}-attempt-{}-action-{index}",
                        trial.id, attempt.index
                    ),
                )?;
            }
            if let Some(oracle) = &mut attempt.oracle {
                externalize_text(
                    &mut oracle.stdout,
                    &mut oracle.stdout_artifact,
                    &artifact_dir,
                    &format!(
                        "trial-{}-attempt-{}-oracle-stdout.txt",
                        trial.id, attempt.index
                    ),
                )?;
                externalize_text(
                    &mut oracle.stderr,
                    &mut oracle.stderr_artifact,
                    &artifact_dir,
                    &format!(
                        "trial-{}-attempt-{}-oracle-stderr.txt",
                        trial.id, attempt.index
                    ),
                )?;
            }
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO trials (id, session_id, outcome, created_at, trial_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET trial_json=excluded.trial_json, outcome=excluded.outcome",
            params![
                trial.id.to_string(),
                session_id.to_string(),
                format!("{:?}", trial.outcome),
                Utc::now().to_rfc3339(),
                serde_json::to_string(&trial)?,
            ],
        )?;
        transaction.execute(
            "DELETE FROM trial_attempts WHERE trial_id=?1",
            [trial.id.to_string()],
        )?;
        for attempt in &trial.repetitions {
            transaction.execute(
                "INSERT INTO trial_attempts (trial_id, attempt_index, attempt_json)
                 VALUES (?1, ?2, ?3)",
                params![
                    trial.id.to_string(),
                    attempt.index,
                    serde_json::to_string(attempt)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_trials(&self, session_id: Uuid) -> Result<Vec<Trial>, AppError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT trial_json FROM trials WHERE session_id=?1 ORDER BY created_at, id")?;
        let rows = statement.query_map([session_id.to_string()], |row| row.get::<_, String>(0))?;
        let mut trials = Vec::new();
        for row in rows {
            trials.push(serde_json::from_str(&row?)?);
        }
        Ok(trials)
    }

    pub fn record_progress(
        &self,
        session_id: Option<Uuid>,
        event: &ProgressEvent,
    ) -> Result<(), AppError> {
        self.insert_json("progress_events", session_id, &serde_json::to_value(event)?)
    }

    pub fn insert_json(
        &self,
        table: &'static str,
        session_id: Option<Uuid>,
        value: &Value,
    ) -> Result<(), AppError> {
        if !matches!(
            table,
            "messages" | "tool_calls" | "api_usage" | "progress_events" | "diagnoses"
        ) {
            return Err(AppError::Database(rusqlite::Error::InvalidQuery));
        }
        let connection = self.connection()?;
        let sql = format!(
            "INSERT INTO {table} (id, session_id, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)"
        );
        connection.execute(
            &sql,
            params![
                Uuid::new_v4().to_string(),
                session_id.map(|id| id.to_string()),
                Utc::now().to_rfc3339(),
                serde_json::to_string(value)?,
            ],
        )?;
        Ok(())
    }

    pub fn load_json(&self, table: &'static str, session_id: Uuid) -> Result<Vec<Value>, AppError> {
        if !matches!(
            table,
            "messages" | "tool_calls" | "api_usage" | "progress_events" | "diagnoses"
        ) {
            return Err(AppError::Database(rusqlite::Error::InvalidQuery));
        }
        let connection = self.connection()?;
        let sql =
            format!("SELECT payload_json FROM {table} WHERE session_id=?1 ORDER BY created_at, id");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map([session_id.to_string()], |row| row.get::<_, String>(0))?;
        let mut values = Vec::new();
        for row in rows {
            values.push(serde_json::from_str(&row?)?);
        }
        Ok(values)
    }

    fn initialize(&self) -> Result<(), AppError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::io("create history directory", parent, error))?;
        }
        let connection = self.connection()?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                session_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS actions (
                session_id TEXT NOT NULL,
                action_id INTEGER NOT NULL,
                original_order INTEGER NOT NULL,
                action_json TEXT NOT NULL,
                PRIMARY KEY(session_id, action_id),
                FOREIGN KEY(session_id) REFERENCES sessions(id)
             );
             CREATE TABLE IF NOT EXISTS trials (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                outcome TEXT NOT NULL,
                created_at TEXT NOT NULL,
                trial_json TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
             );
             CREATE TABLE IF NOT EXISTS trial_attempts (
                trial_id TEXT NOT NULL,
                attempt_index INTEGER NOT NULL,
                attempt_json TEXT NOT NULL,
                PRIMARY KEY(trial_id, attempt_index),
                FOREIGN KEY(trial_id) REFERENCES trials(id)
             );
             CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY, session_id TEXT, created_at TEXT NOT NULL, payload_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tool_calls (
                id TEXT PRIMARY KEY, session_id TEXT, created_at TEXT NOT NULL, payload_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS api_usage (
                id TEXT PRIMARY KEY, session_id TEXT, created_at TEXT NOT NULL, payload_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS progress_events (
                id TEXT PRIMARY KEY, session_id TEXT, created_at TEXT NOT NULL, payload_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS diagnoses (
                id TEXT PRIMARY KEY, session_id TEXT, created_at TEXT NOT NULL, payload_json TEXT NOT NULL
             );",
        )?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection, AppError> {
        let connection = Connection::open(&self.path)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        Ok(connection)
    }

    fn artifact_dir(&self, session_id: Uuid) -> Result<PathBuf, AppError> {
        let session = self.load_session(session_id)?;
        let session_root =
            session
                .baseline_path
                .parent()
                .ok_or_else(|| AppError::InvalidProject {
                    path: session.baseline_path.clone(),
                    reason: "baseline has no session parent directory".to_owned(),
                })?;
        Ok(session_root.join("artifacts"))
    }
}

const INLINE_OUTPUT_LIMIT: usize = 64 * 1024;

fn externalize_action_result(
    result: &mut ActionResult,
    artifact_dir: &std::path::Path,
    prefix: &str,
) -> Result<(), AppError> {
    externalize_text(
        &mut result.stdout,
        &mut result.stdout_artifact,
        artifact_dir,
        &format!("{prefix}-stdout.txt"),
    )?;
    externalize_text(
        &mut result.stderr,
        &mut result.stderr_artifact,
        artifact_dir,
        &format!("{prefix}-stderr.txt"),
    )
}

fn externalize_text(
    text: &mut String,
    artifact: &mut Option<ArtifactRef>,
    artifact_dir: &std::path::Path,
    file_name: &str,
) -> Result<(), AppError> {
    if text.len() <= INLINE_OUTPUT_LIMIT {
        return Ok(());
    }
    let bytes = text.as_bytes();
    fs::create_dir_all(artifact_dir)
        .map_err(|error| AppError::io("create artifact directory", artifact_dir, error))?;
    let path = artifact_dir.join(file_name);
    fs::write(&path, bytes).map_err(|error| AppError::io("write output artifact", &path, error))?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let sha256 = hex::encode(Sha256::digest(bytes));
    let mut boundary = INLINE_OUTPUT_LIMIT;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    text.push_str("\n[output truncated; complete content stored as an artifact]\n");
    *artifact = Some(ArtifactRef {
        path: PathBuf::from("artifacts").join(file_name),
        size,
        sha256,
    });
    Ok(())
}
