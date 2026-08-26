use std::{fs, path::PathBuf};

use chrono::{DateTime, Utc};
use fixtrace_protocol::{
    AppEvent, ApprovalRequest, ApprovalResolution, ApprovalStatus, ApprovalView, EventBatch,
    EventEnvelope, EventGap, TaskStatus, TaskSummary,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

use crate::migrations;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("event store SQLite error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("event store JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("event store {operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("event store migration failed: {0}")]
    Migration(String),
    #[error("event stream invariant failed: {0}")]
    Invariant(String),
    #[error("task {0} was not found")]
    TaskNotFound(Uuid),
    #[error("invalid task transition from {from:?} to {to:?}")]
    InvalidTaskTransition { from: TaskStatus, to: TaskStatus },
    #[error("approval {0} was not found")]
    ApprovalNotFound(Uuid),
    #[error("approval {0} has already been resolved")]
    ApprovalAlreadyResolved(Uuid),
}

#[derive(Clone, Debug)]
pub struct EventStore {
    path: PathBuf,
    migration_backup: Option<PathBuf>,
}

impl EventStore {
    pub fn deferred(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            migration_backup: None,
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let existed_before_open =
            path.exists() && fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                operation: "create event store directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut connection = Connection::open(&path)?;
        let migration_backup = migrations::migrate(&mut connection, &path, existed_before_open)?;
        Ok(Self {
            path,
            migration_backup,
        })
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        migrations::schema_version(&self.connection()?)
    }

    pub fn migration_backup(&self) -> Option<&std::path::Path> {
        self.migration_backup.as_deref()
    }

    pub fn append(
        &self,
        session_id: Option<Uuid>,
        task_id: Option<Uuid>,
        payload: AppEvent,
    ) -> Result<EventEnvelope, StoreError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope_key = stream_scope(session_id);
        let stream = transaction
            .query_row(
                "SELECT id, next_sequence FROM app_event_streams WHERE scope_key=?1",
                [&scope_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let (stream_id, sequence) = match stream {
            Some((stream_id, sequence)) => (parse_uuid(&stream_id)?, sequence),
            None => {
                let stream_id = Uuid::new_v4();
                transaction.execute(
                    "INSERT INTO app_event_streams
                     (id, scope_key, session_id, next_sequence, created_at)
                     VALUES(?1, ?2, ?3, 1, ?4)",
                    params![
                        stream_id.to_string(),
                        scope_key,
                        session_id.map(|id| id.to_string()),
                        Utc::now().to_rfc3339(),
                    ],
                )?;
                (stream_id, 1)
            }
        };
        let sequence = u64::try_from(sequence)
            .map_err(|_| StoreError::Invariant("negative event sequence".to_owned()))?;
        if sequence == 0 || sequence > fixtrace_protocol::MAX_SAFE_WIRE_INTEGER {
            return Err(StoreError::Invariant(
                "event sequence exceeds the wire-safe range".to_owned(),
            ));
        }
        let timestamp = Utc::now();
        let event_id = Uuid::new_v4();
        transaction.execute(
            "INSERT INTO app_events
             (event_id, stream_id, sequence, session_id, task_id, timestamp,
              event_type, payload_json, schema_version)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event_id.to_string(),
                stream_id.to_string(),
                i64::try_from(sequence).expect("wire-safe sequence fits SQLite integer"),
                session_id.map(|id| id.to_string()),
                task_id.map(|id| id.to_string()),
                timestamp.to_rfc3339(),
                payload.event_type(),
                serde_json::to_string(&payload)?,
                fixtrace_protocol::EVENT_SCHEMA_VERSION,
            ],
        )?;
        transaction.execute(
            "UPDATE app_event_streams SET next_sequence=?1 WHERE id=?2",
            params![
                i64::try_from(sequence + 1).expect("wire-safe sequence fits SQLite integer"),
                stream_id.to_string()
            ],
        )?;
        transaction.commit()?;
        Ok(EventEnvelope {
            schema_version: fixtrace_protocol::EVENT_SCHEMA_VERSION,
            stream_id,
            sequence,
            event_id,
            timestamp,
            session_id,
            task_id,
            payload,
        })
    }

    pub fn load_after(
        &self,
        session_id: Option<Uuid>,
        after_sequence: u64,
        limit: u32,
    ) -> Result<EventBatch, StoreError> {
        let connection = self.connection()?;
        let scope_key = stream_scope(session_id);
        let stream = connection
            .query_row(
                "SELECT id, next_sequence FROM app_event_streams WHERE scope_key=?1",
                [&scope_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((stream_id, next_sequence)) = stream else {
            return Ok(EventBatch {
                events: Vec::new(),
                high_watermark: 0,
                gap: None,
            });
        };
        let stream_id = parse_uuid(&stream_id)?;
        let high_watermark = u64::try_from(next_sequence.saturating_sub(1))
            .map_err(|_| StoreError::Invariant("negative stream high watermark".to_owned()))?;
        let safe_after =
            i64::try_from(after_sequence.min(fixtrace_protocol::MAX_SAFE_WIRE_INTEGER))
                .expect("wire-safe sequence fits SQLite integer");
        let limit = i64::from(limit.clamp(1, 10_000));
        let mut statement = connection.prepare(
            "SELECT event_id, sequence, session_id, task_id, timestamp,
                    payload_json, schema_version
             FROM app_events
             WHERE stream_id=?1 AND sequence>?2
             ORDER BY sequence
             LIMIT ?3",
        )?;
        let rows =
            statement.query_map(params![stream_id.to_string(), safe_after, limit], |row| {
                Ok(EventRow {
                    event_id: row.get(0)?,
                    sequence: row.get(1)?,
                    session_id: row.get(2)?,
                    task_id: row.get(3)?,
                    timestamp: row.get(4)?,
                    payload_json: row.get(5)?,
                    schema_version: row.get(6)?,
                })
            })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?.into_envelope(stream_id)?);
        }
        let expected = after_sequence.saturating_add(1);
        let gap = events.first().and_then(|event| {
            (event.sequence != expected).then(|| EventGap {
                stream_id,
                expected_sequence: expected,
                available_from_sequence: event.sequence,
                high_watermark,
                reason: "persisted event sequence is not contiguous".to_owned(),
            })
        });
        Ok(EventBatch {
            events,
            high_watermark,
            gap,
        })
    }

    pub fn save_task(&self, task: &TaskSummary) -> Result<(), StoreError> {
        let connection = self.connection()?;
        let status = wire_string(&task.status)?;
        let kind = wire_string(&task.kind)?;
        connection.execute(
            "INSERT INTO tasks
             (id, session_id, operation_id, kind, status, task_json, created_at,
              started_at, finished_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               status=excluded.status, task_json=excluded.task_json,
               started_at=excluded.started_at, finished_at=excluded.finished_at,
               updated_at=excluded.updated_at",
            params![
                task.id.to_string(),
                task.session_id.map(|id| id.to_string()),
                task.operation_id.to_string(),
                kind,
                status,
                serde_json::to_string(task)?,
                task.created_at.to_rfc3339(),
                task.started_at.map(|date| date.to_rfc3339()),
                task.finished_at.map(|date| date.to_rfc3339()),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn load_task(&self, task_id: Uuid) -> Result<TaskSummary, StoreError> {
        let connection = self.connection()?;
        let json = connection
            .query_row(
                "SELECT task_json FROM tasks WHERE id=?1",
                [task_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(StoreError::TaskNotFound(task_id))?;
        Ok(serde_json::from_str(&json)?)
    }

    pub fn load_task_by_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<TaskSummary>, StoreError> {
        let connection = self.connection()?;
        let json = connection
            .query_row(
                "SELECT task_json FROM tasks WHERE operation_id=?1",
                [operation_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(StoreError::from)
    }

    pub fn active_task_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<TaskSummary>, StoreError> {
        let connection = self.connection()?;
        let json = connection
            .query_row(
                "SELECT task_json FROM tasks
                 WHERE session_id=?1
                   AND status IN ('queued', 'running', 'waiting_for_approval', 'cancelling')
                 ORDER BY created_at DESC LIMIT 1",
                [session_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(StoreError::from)
    }

    pub fn transition_task(
        &self,
        task_id: Uuid,
        next: TaskStatus,
    ) -> Result<TaskSummary, StoreError> {
        let mut task = self.load_task(task_id)?;
        if !task.status.can_transition_to(next) {
            return Err(StoreError::InvalidTaskTransition {
                from: task.status,
                to: next,
            });
        }
        task.status = next;
        let now = Utc::now();
        if next == TaskStatus::Running && task.started_at.is_none() {
            task.started_at = Some(now);
        }
        if next.is_terminal() {
            task.finished_at = Some(now);
            task.is_cancellable = false;
        }
        self.save_task(&task)?;
        Ok(task)
    }

    pub fn save_approval(&self, request: &ApprovalRequest) -> Result<ApprovalView, StoreError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO approvals
             (id, session_id, task_id, status, request_json, created_at)
             VALUES(?1, ?2, ?3, 'pending', ?4, ?5)",
            params![
                request.id.to_string(),
                request.session_id.to_string(),
                request.task_id.to_string(),
                serde_json::to_string(request)?,
                request.created_at.to_rfc3339(),
            ],
        )?;
        Ok(ApprovalView {
            request: request.clone(),
            status: ApprovalStatus::Pending,
            resolution: None,
            can_approve: true,
        })
    }

    pub fn load_approval(&self, approval_id: Uuid) -> Result<ApprovalView, StoreError> {
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT request_json, status, resolution_json
                 FROM approvals WHERE id=?1",
                [approval_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::ApprovalNotFound(approval_id))?;
        let status: ApprovalStatus = serde_json::from_value(serde_json::Value::String(row.1))?;
        Ok(ApprovalView {
            request: serde_json::from_str(&row.0)?,
            status: status.clone(),
            resolution: row
                .2
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            can_approve: status == ApprovalStatus::Pending,
        })
    }

    pub fn resolve_approval(
        &self,
        resolution: &ApprovalResolution,
    ) -> Result<ApprovalView, StoreError> {
        if resolution.status == ApprovalStatus::Pending {
            return Err(StoreError::Invariant(
                "approval resolution cannot remain pending".to_owned(),
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE approvals
             SET status=?1, resolution_json=?2, resolved_by_client_id=?3, resolved_at=?4
             WHERE id=?5 AND status='pending'",
            params![
                wire_string(&resolution.status)?,
                serde_json::to_string(resolution)?,
                resolution.resolved_by_client_id.to_string(),
                resolution.resolved_at.to_rfc3339(),
                resolution.approval_id.to_string(),
            ],
        )?;
        if changed == 0 {
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM approvals WHERE id=?1)",
                [resolution.approval_id.to_string()],
                |row| row.get(0),
            )?;
            return if exists {
                Err(StoreError::ApprovalAlreadyResolved(resolution.approval_id))
            } else {
                Err(StoreError::ApprovalNotFound(resolution.approval_id))
            };
        }
        transaction.commit()?;
        self.load_approval(resolution.approval_id)
    }

    fn connection(&self) -> Result<Connection, StoreError> {
        let connection = Connection::open(&self.path)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        Ok(connection)
    }
}

struct EventRow {
    event_id: String,
    sequence: i64,
    session_id: Option<String>,
    task_id: Option<String>,
    timestamp: String,
    payload_json: String,
    schema_version: i64,
}

impl EventRow {
    fn into_envelope(self, stream_id: Uuid) -> Result<EventEnvelope, StoreError> {
        let timestamp = DateTime::parse_from_rfc3339(&self.timestamp)
            .map_err(|error| StoreError::Invariant(format!("invalid event timestamp: {error}")))?
            .with_timezone(&Utc);
        Ok(EventEnvelope {
            schema_version: u16::try_from(self.schema_version)
                .map_err(|_| StoreError::Invariant("invalid event schema version".to_owned()))?,
            stream_id,
            sequence: u64::try_from(self.sequence)
                .map_err(|_| StoreError::Invariant("negative event sequence".to_owned()))?,
            event_id: parse_uuid(&self.event_id)?,
            timestamp,
            session_id: self.session_id.as_deref().map(parse_uuid).transpose()?,
            task_id: self.task_id.as_deref().map(parse_uuid).transpose()?,
            payload: serde_json::from_str(&self.payload_json)?,
        })
    }
}

fn stream_scope(session_id: Option<Uuid>) -> String {
    session_id.map_or_else(|| "global".to_owned(), |id| format!("session:{id}"))
}

fn parse_uuid(value: &str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value)
        .map_err(|error| StoreError::Invariant(format!("invalid stored UUID `{value}`: {error}")))
}

fn wire_string<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    let value = serde_json::to_value(value)?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::Invariant("wire enum did not serialize as a string".to_owned()))
}
