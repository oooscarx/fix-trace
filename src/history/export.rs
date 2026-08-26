use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    domain::{
        action::{Action, ActionKind},
        session::SessionRecord,
        trial::Trial,
    },
    error::AppError,
};

use super::database::HistoryDatabase;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionExport {
    pub format_version: u32,
    pub session: SessionRecord,
    pub actions: Vec<Action>,
    pub trials: Vec<Trial>,
    pub messages: Vec<Value>,
    pub tool_calls: Vec<Value>,
    pub api_usage: Vec<Value>,
    pub progress_events: Vec<Value>,
    pub diagnoses: Vec<Value>,
}

pub fn export_session(
    database: &HistoryDatabase,
    session_id: Uuid,
    output: &Path,
) -> Result<SessionExport, AppError> {
    let mut export = SessionExport {
        format_version: 1,
        session: database.load_session(session_id)?,
        actions: database.load_actions(session_id)?,
        trials: database.load_trials(session_id)?,
        messages: database.load_json("messages", session_id)?,
        tool_calls: database.load_json("tool_calls", session_id)?,
        api_usage: database.load_json("api_usage", session_id)?,
        progress_events: database.load_json("progress_events", session_id)?,
        diagnoses: database.load_json("diagnoses", session_id)?,
    };
    redact_export(&mut export);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("create export directory", parent, error))?;
    }
    let encoded = serde_json::to_vec_pretty(&export)?;
    fs::write(output, encoded)
        .map_err(|error| AppError::io("write session export", output, error))?;
    Ok(export)
}

pub fn import_session(database: &HistoryDatabase, input: &Path) -> Result<SessionExport, AppError> {
    let bytes =
        fs::read(input).map_err(|error| AppError::io("read session import", input, error))?;
    let mut export: SessionExport = serde_json::from_slice(&bytes)?;
    if export.format_version != 1 {
        return Err(AppError::Import(format!(
            "unsupported format version {}",
            export.format_version
        )));
    }
    if database.session_exists(export.session.id)? {
        return Err(AppError::Import(format!(
            "session {} already exists",
            export.session.id
        )));
    }
    redact_export(&mut export);
    database.save_session(&export.session)?;
    for action in &export.actions {
        database.save_action(export.session.id, action)?;
    }
    for trial in &export.trials {
        database.save_trial(export.session.id, trial)?;
    }
    for (table, values) in [
        ("messages", &export.messages),
        ("tool_calls", &export.tool_calls),
        ("api_usage", &export.api_usage),
        ("progress_events", &export.progress_events),
        ("diagnoses", &export.diagnoses),
    ] {
        for value in values {
            database.insert_json(table, Some(export.session.id), value)?;
        }
    }
    Ok(export)
}

fn redact_export(export: &mut SessionExport) {
    let mut secrets = Vec::new();
    for action in &mut export.actions {
        if let ActionKind::SetEnvironment { key, value } = &mut action.kind
            && is_sensitive_key(key)
        {
            if !value.is_empty() && value != "<redacted>" {
                secrets.push(value.clone());
            }
            *value = "<redacted>".to_owned();
        }
    }
    if secrets.is_empty() {
        return;
    }
    for action in &mut export.actions {
        let mut value = serde_json::to_value(&*action).unwrap_or(Value::Null);
        redact_value(&mut value, &secrets);
        if let Ok(redacted) = serde_json::from_value(value) {
            *action = redacted;
        }
    }
    for trial in &mut export.trials {
        let mut value = serde_json::to_value(&*trial).unwrap_or(Value::Null);
        redact_value(&mut value, &secrets);
        if let Ok(redacted) = serde_json::from_value(value) {
            *trial = redacted;
        }
    }
    for value in export
        .messages
        .iter_mut()
        .chain(&mut export.tool_calls)
        .chain(&mut export.api_usage)
        .chain(&mut export.progress_events)
        .chain(&mut export.diagnoses)
    {
        redact_value(value, &secrets);
    }
}

fn redact_value(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => {
            for secret in secrets {
                if !secret.is_empty() {
                    *text = text.replace(secret, "<redacted>");
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_value(value, secrets);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                if is_sensitive_key(key) {
                    *value = Value::String("<redacted>".to_owned());
                } else {
                    redact_value(value, secrets);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let uppercase = key.to_ascii_uppercase();
    ["API_KEY", "TOKEN", "SECRET", "PASSWORD", "AUTHORIZATION"]
        .iter()
        .any(|marker| uppercase.contains(marker))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::{
        domain::{
            action::{Action, ActionKind},
            session::{SessionRecord, SessionStatus},
            snapshot::SnapshotManifest,
        },
        replay::oracle::OracleSpec,
    };

    use super::{export_session, import_session};
    use crate::history::database::HistoryDatabase;

    #[test]
    fn json_export_import_round_trip_preserves_session_and_redacts_secret() {
        let temp = tempdir().expect("temporary directory should be created");
        let source = HistoryDatabase::open(temp.path().join("source.sqlite3"))
            .expect("source database should open");
        let id = Uuid::new_v4();
        let now = Utc::now();
        let session = SessionRecord {
            id,
            project_name: "fixture".to_owned(),
            original_project: PathBuf::from("/tmp/fixture"),
            baseline_path: PathBuf::from("/tmp/baseline"),
            worktree_path: PathBuf::from("/tmp/worktree"),
            oracle: OracleSpec {
                command: "false".to_owned(),
                timeout_ms: 1000,
            },
            baseline_manifest: SnapshotManifest {
                root_hash: "hash".to_owned(),
                files: Default::default(),
            },
            status: SessionStatus::Recording,
            created_at: now,
            updated_at: now,
        };
        let action = Action {
            id: 1,
            original_order: 1,
            cwd_before: PathBuf::new(),
            kind: ActionKind::SetEnvironment {
                key: "SERVICE_TOKEN".to_owned(),
                value: "top-secret".to_owned(),
            },
            replayable: true,
            note: None,
            result: None,
        };
        source.save_session(&session).expect("session should save");
        source.save_action(id, &action).expect("action should save");
        let file = temp.path().join("session.json");
        let exported = export_session(&source, id, &file).expect("export should succeed");

        let destination = HistoryDatabase::open(temp.path().join("destination.sqlite3"))
            .expect("destination database should open");
        let imported = import_session(&destination, &file).expect("import should succeed");

        assert_eq!(imported.session, exported.session);
        assert_eq!(
            destination.load_session(id).expect("session should load"),
            session
        );
        let actions = destination.load_actions(id).expect("actions should load");
        assert!(matches!(
            &actions[0].kind,
            ActionKind::SetEnvironment { value, .. } if value == "<redacted>"
        ));
    }
}
