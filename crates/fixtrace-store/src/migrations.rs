use std::{fs, path::Path};

use chrono::Utc;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::{CURRENT_SCHEMA_VERSION, StoreError};

const V1_CHECKSUM: &str = "legacy-core-schema-v1";
const V2_CHECKSUM: &str = "ui-v2-events-tasks-approvals-v1";

pub(crate) fn migrate(
    connection: &mut Connection,
    database_path: &Path,
    existed_before_open: bool,
) -> Result<Option<std::path::PathBuf>, StoreError> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL,
            checksum TEXT NOT NULL
         );",
    )?;
    let version = schema_version(connection)?;
    verify_known_checksums(connection)?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::Migration(format!(
            "database schema v{version} is newer than supported v{CURRENT_SCHEMA_VERSION}"
        )));
    }

    let backup = if existed_before_open && version < CURRENT_SCHEMA_VERSION {
        Some(create_backup(connection, database_path)?)
    } else {
        None
    };

    if version < 1 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at, checksum)
             VALUES(1, ?1, ?2)",
            params![Utc::now().to_rfc3339(), V1_CHECKSUM],
        )?;
        transaction.commit()?;
    }

    if schema_version(connection)? < 2 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE app_event_streams (
                id TEXT PRIMARY KEY,
                scope_key TEXT NOT NULL UNIQUE,
                session_id TEXT,
                next_sequence INTEGER NOT NULL CHECK(next_sequence >= 1),
                created_at TEXT NOT NULL
             );
             CREATE TABLE app_events (
                event_id TEXT PRIMARY KEY,
                stream_id TEXT NOT NULL,
                sequence INTEGER NOT NULL CHECK(sequence >= 1),
                session_id TEXT,
                task_id TEXT,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                UNIQUE(stream_id, sequence),
                FOREIGN KEY(stream_id) REFERENCES app_event_streams(id)
             );
             CREATE INDEX app_events_session_sequence
                ON app_events(session_id, sequence);
             CREATE INDEX app_events_task
                ON app_events(task_id, sequence);

             CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                operation_id TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                task_json TEXT NOT NULL,
                request_json TEXT,
                result_json TEXT,
                error_json TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                updated_at TEXT NOT NULL
             );
             CREATE INDEX tasks_session_status ON tasks(session_id, status);
             CREATE UNIQUE INDEX tasks_one_active_per_session
                ON tasks(session_id)
                WHERE session_id IS NOT NULL
                  AND status IN ('queued', 'running', 'waiting_for_approval', 'cancelling');

             CREATE TABLE approvals (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                task_id TEXT NOT NULL,
                status TEXT NOT NULL,
                request_json TEXT NOT NULL,
                resolution_json TEXT,
                resolved_by_client_id TEXT,
                created_at TEXT NOT NULL,
                resolved_at TEXT
             );
             CREATE INDEX approvals_session_status ON approvals(session_id, status);

             CREATE TABLE client_sessions (
                id TEXT PRIMARY KEY,
                client_name TEXT NOT NULL,
                client_version TEXT NOT NULL,
                capabilities_json TEXT NOT NULL,
                connected_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
             );

             CREATE TABLE ui_preferences (
                client_scope TEXT NOT NULL,
                preference_key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(client_scope, preference_key)
             );

             CREATE TABLE operations (
                client_id TEXT NOT NULL,
                operation_id TEXT NOT NULL,
                request_hash TEXT NOT NULL,
                response_json TEXT,
                task_id TEXT,
                created_at TEXT NOT NULL,
                PRIMARY KEY(client_id, operation_id)
             );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at, checksum)
             VALUES(2, ?1, ?2)",
            params![Utc::now().to_rfc3339(), V2_CHECKSUM],
        )?;
        transaction.commit()?;
    }

    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StoreError::Migration(format!(
            "SQLite integrity check failed after migration: {integrity}"
        )));
    }
    Ok(backup)
}

pub(crate) fn schema_version(connection: &Connection) -> Result<i64, StoreError> {
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

fn verify_known_checksums(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("SELECT version, checksum FROM schema_migrations WHERE version IN (1, 2)")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (version, actual) = row?;
        let expected = match version {
            1 => V1_CHECKSUM,
            2 => V2_CHECKSUM,
            _ => continue,
        };
        if actual != expected {
            return Err(StoreError::Migration(format!(
                "schema migration v{version} checksum mismatch"
            )));
        }
    }
    Ok(())
}

fn create_backup(
    connection: &Connection,
    database_path: &Path,
) -> Result<std::path::PathBuf, StoreError> {
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let file_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("history.sqlite3");
    let backup_path = database_path.with_file_name(format!("{file_name}.pre-ui-v2-v1.bak"));
    if !backup_path.exists() {
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                operation: "create migration backup directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        connection.backup(rusqlite::MAIN_DB, &backup_path, None)?;
    }
    Ok(backup_path)
}
