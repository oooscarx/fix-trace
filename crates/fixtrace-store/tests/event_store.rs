use std::{sync::Arc, thread};

use chrono::Utc;
use fixtrace_protocol::{
    AppEvent, ApprovalChoice, ApprovalKind, ApprovalRequest, ApprovalResolution, ApprovalScope,
    ApprovalStatus, Notice, NoticeLevel, RiskLevel, TaskKind, TaskStatus, TaskSummary,
};
use fixtrace_store::{CURRENT_SCHEMA_VERSION, EventStore, StoreError};
use rusqlite::{Connection, params};
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn existing_v1_database_is_backed_up_and_migrated_without_data_loss() {
    let temp = tempdir().expect("temporary directory should be created");
    let path = temp.path().join("history.sqlite3");
    let connection = Connection::open(&path).expect("v1 fixture should open");
    connection
        .execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                session_json TEXT NOT NULL
             );",
        )
        .expect("v1 schema should be created");
    connection
        .execute(
            "INSERT INTO sessions VALUES(?1, 'recording', ?2, ?2, '{}')",
            params![Uuid::new_v4().to_string(), Utc::now().to_rfc3339()],
        )
        .expect("v1 row should be inserted");
    drop(connection);

    let store = EventStore::open(&path).expect("v1 database should migrate");
    assert_eq!(store.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    let backup = store
        .migration_backup()
        .expect("an existing v1 database should have a backup");
    assert!(backup.is_file());

    let migrated = Connection::open(&path).unwrap();
    let session_count: i64 = migrated
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(session_count, 1);
    let event_table_count: i64 = migrated
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name='app_events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_table_count, 1);

    let backup_connection = Connection::open(backup).unwrap();
    let backup_session_count: i64 = backup_connection
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(backup_session_count, 1);
}

#[test]
fn concurrent_appends_allocate_one_contiguous_session_sequence() {
    let temp = tempdir().unwrap();
    let store = Arc::new(EventStore::open(temp.path().join("history.sqlite3")).unwrap());
    let session_id = Uuid::new_v4();
    let mut workers = Vec::new();
    for index in 0..16 {
        let store = store.clone();
        workers.push(thread::spawn(move || {
            store
                .append(
                    Some(session_id),
                    None,
                    AppEvent::Notice(Notice {
                        code: format!("worker_{index}"),
                        level: NoticeLevel::Info,
                        title: "Concurrent append".to_owned(),
                        message: index.to_string(),
                    }),
                )
                .expect("concurrent event should append")
        }));
    }
    let mut events: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker should not panic"))
        .collect();
    events.sort_by_key(|event| event.sequence);
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1..=16).collect::<Vec<_>>()
    );
    assert!(
        events
            .iter()
            .all(|event| event.stream_id == events[0].stream_id)
    );

    let catch_up = store.load_after(Some(session_id), 4, 100).unwrap();
    assert_eq!(catch_up.high_watermark, 16);
    assert_eq!(catch_up.events.first().unwrap().sequence, 5);
    assert_eq!(catch_up.events.last().unwrap().sequence, 16);
    assert!(catch_up.gap.is_none());
}

#[test]
fn missing_persisted_sequence_is_reported_as_a_gap() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("history.sqlite3");
    let store = EventStore::open(&path).unwrap();
    let session_id = Uuid::new_v4();
    for index in 0..3 {
        store
            .append(
                Some(session_id),
                None,
                AppEvent::Notice(Notice {
                    code: "gap_fixture".to_owned(),
                    level: NoticeLevel::Info,
                    title: "Gap fixture".to_owned(),
                    message: index.to_string(),
                }),
            )
            .unwrap();
    }
    Connection::open(&path)
        .unwrap()
        .execute(
            "DELETE FROM app_events WHERE session_id=?1 AND sequence=2",
            [session_id.to_string()],
        )
        .unwrap();

    let catch_up = store.load_after(Some(session_id), 1, 100).unwrap();
    let gap = catch_up.gap.expect("deleted sequence should produce a gap");
    assert_eq!(gap.expected_sequence, 2);
    assert_eq!(gap.available_from_sequence, 3);
    assert_eq!(gap.high_watermark, 3);
}

#[test]
fn task_transitions_are_validated_by_the_store() {
    let temp = tempdir().unwrap();
    let store = EventStore::open(temp.path().join("history.sqlite3")).unwrap();
    let now = Utc::now();
    let task = TaskSummary {
        id: Uuid::new_v4(),
        session_id: Some(Uuid::new_v4()),
        operation_id: Uuid::new_v4(),
        kind: TaskKind::AnalyzeMinimalTrace,
        status: TaskStatus::Queued,
        title: "Analyze".to_owned(),
        created_at: now,
        started_at: None,
        finished_at: None,
        progress_ratio: None,
        is_cancellable: true,
        supports_steer: false,
    };
    store.save_task(&task).unwrap();
    let running = store.transition_task(task.id, TaskStatus::Running).unwrap();
    assert!(running.started_at.is_some());
    let completed = store
        .transition_task(task.id, TaskStatus::Completed)
        .unwrap();
    assert!(completed.finished_at.is_some());
    assert!(!completed.is_cancellable);

    assert!(matches!(
        store.transition_task(task.id, TaskStatus::Running),
        Err(StoreError::InvalidTaskTransition { .. })
    ));
}

#[test]
fn approval_resolution_is_compare_and_set() {
    let temp = tempdir().unwrap();
    let store = EventStore::open(temp.path().join("history.sqlite3")).unwrap();
    let request = ApprovalRequest {
        id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        task_id: Uuid::new_v4(),
        kind: ApprovalKind::ReplayCommand,
        title: "Replay cargo test".to_owned(),
        reason: "Recorded command".to_owned(),
        risk: RiskLevel::Low,
        command_preview: Some("cargo test".to_owned()),
        cwd: Some(std::path::PathBuf::new()),
        affected_paths: Vec::new(),
        action_ids: vec![1],
        accesses_network: false,
        sandbox_path: Some(std::path::PathBuf::from("trial")),
        requested_scope: ApprovalScope::Once,
        choices: vec![ApprovalChoice::ApproveOnce, ApprovalChoice::Deny],
        created_at: Utc::now(),
    };
    store.save_approval(&request).unwrap();
    let resolution = ApprovalResolution {
        approval_id: request.id,
        choice: ApprovalChoice::ApproveOnce,
        status: ApprovalStatus::Approved,
        resolved_by_client_id: Uuid::new_v4(),
        resolved_at: Utc::now(),
        equivalent_rule_id: None,
    };
    let resolved = store.resolve_approval(&resolution).unwrap();
    assert_eq!(resolved.status, ApprovalStatus::Approved);
    assert!(!resolved.can_approve);
    assert!(matches!(
        store.resolve_approval(&resolution),
        Err(StoreError::ApprovalAlreadyResolved(id)) if id == request.id
    ));
}

#[test]
fn migration_checksum_mismatch_stops_before_ui_tables_are_created() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("history.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                checksum TEXT NOT NULL
             );
             INSERT INTO schema_migrations VALUES(1, 'fixture', 'tampered');",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        EventStore::open(&path),
        Err(StoreError::Migration(message)) if message.contains("checksum mismatch")
    ));
    let connection = Connection::open(&path).unwrap();
    let event_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='app_events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_tables, 0);
}
