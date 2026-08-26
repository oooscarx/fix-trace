use std::fs;

use fixtrace::{
    application::{
        AppCommand, AppResponse, AppServiceOptions, FixTraceAppService, FixTraceApplication,
        FixTraceProtocolApplication,
    },
    progress::ProgressEvent,
};
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ApprovalChoice, ApprovalKind, ApprovalRequest,
    ApprovalScope, ApprovalStatus, ItemStatus, Notice, NoticeItem, NoticeLevel, PageRequest,
    RiskLevel, SessionSnapshotRequest, TaskKind, TaskStatus, TaskSummary, TimelineItem,
    TimelineItemHeader,
};
use fixtrace_store::EventStore;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn app_service_is_the_stateful_entry_point_used_without_cli_types() {
    let temp = tempdir().expect("temporary directory should be created");
    let state_dir = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).expect("project fixture should be created");
    fs::write(project.join("fixture.txt"), "broken").expect("project fixture should be written");

    let service = FixTraceAppService::start(
        AppServiceOptions {
            state_dir: Some(state_dir),
            config_path: None,
            initialize_event_store: true,
        },
        CancellationToken::new(),
    )
    .expect("App Service should start");
    let mut progress = service.subscribe_progress();

    let saved = service
        .execute(AppCommand::SetConfig {
            key: "replay.repetitions".to_owned(),
            value: "1".to_owned(),
        })
        .await
        .expect("configuration should update through App Service");
    assert!(matches!(saved, AppResponse::ConfigSaved { .. }));

    let config = service
        .execute(AppCommand::GetConfig)
        .await
        .expect("configuration should load through App Service");
    let AppResponse::Config { toml } = config else {
        panic!("GetConfig returned the wrong response");
    };
    assert!(toml.contains("repetitions = 1"));

    let initialized = service
        .execute(AppCommand::InitializeSession {
            project,
            oracle: "false".to_owned(),
            title: None,
        })
        .await
        .expect("session should initialize through App Service");
    let AppResponse::SessionInitialized { session } = initialized else {
        panic!("InitializeSession returned the wrong response");
    };
    assert!(session.baseline_path.is_dir());
    assert!(session.worktree_path.is_dir());

    let sessions = service
        .execute(AppCommand::ListSessions)
        .await
        .expect("history should load through App Service");
    let AppResponse::Sessions { sessions } = sessions else {
        panic!("ListSessions returned the wrong response");
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session.id);

    let mut saw_created = false;
    while let Ok(event) = progress.try_recv() {
        if event
            == (ProgressEvent::SessionCreated {
                session_id: session.id,
            })
        {
            saw_created = true;
        }
    }
    assert!(saw_created, "App Service should publish workflow progress");
}

#[cfg(unix)]
#[tokio::test]
async fn app_service_runs_independent_sessions_concurrently() {
    let temp = tempdir().unwrap();
    let service = FixTraceAppService::start(
        AppServiceOptions {
            state_dir: Some(temp.path().join("state")),
            config_path: None,
            initialize_event_store: true,
        },
        CancellationToken::new(),
    )
    .unwrap();
    service
        .execute(AppCommand::SetConfig {
            key: "replay.repetitions".to_owned(),
            value: "1".to_owned(),
        })
        .await
        .unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    fs::write(first.join("fixture.txt"), "first").unwrap();
    fs::write(second.join("fixture.txt"), "second").unwrap();

    let started = std::time::Instant::now();
    let (first_result, second_result) = tokio::join!(
        service.execute(AppCommand::InitializeSession {
            project: first,
            oracle: "sleep 1; false".to_owned(),
            title: None,
        }),
        service.execute(AppCommand::InitializeSession {
            project: second,
            oracle: "sleep 1; false".to_owned(),
            title: None,
        }),
    );
    first_result.expect("first session should initialize");
    second_result.expect("second session should initialize");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1_800),
        "independent sessions were serialized instead of running concurrently"
    );
}

#[tokio::test]
async fn app_service_restart_marks_orphaned_tasks_interrupted() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let database_path = state_dir.join("history.sqlite3");
    let store = EventStore::open(&database_path).unwrap();
    let session_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let task = TaskSummary {
        id: uuid::Uuid::new_v4(),
        session_id: Some(session_id),
        operation_id: uuid::Uuid::new_v4(),
        kind: TaskKind::AgentTurn,
        status: TaskStatus::Running,
        title: "Interrupted fixture".to_owned(),
        created_at: now,
        started_at: Some(now),
        finished_at: None,
        progress_ratio: Some(0.5),
        is_cancellable: true,
        supports_steer: true,
    };
    store.save_task(&task).unwrap();
    let approval = ApprovalRequest {
        id: uuid::Uuid::new_v4(),
        session_id,
        task_id: task.id,
        kind: ApprovalKind::ReplayCommand,
        title: "Dead task approval".to_owned(),
        reason: "Must expire during recovery".to_owned(),
        risk: RiskLevel::Low,
        command_preview: Some("cargo test".to_owned()),
        cwd: None,
        affected_paths: Vec::new(),
        action_ids: Vec::new(),
        accesses_network: false,
        sandbox_path: None,
        requested_scope: ApprovalScope::Once,
        choices: vec![ApprovalChoice::ApproveOnce, ApprovalChoice::Deny],
        created_at: now,
    };
    store.save_approval(&approval).unwrap();

    let _service = FixTraceAppService::start(
        AppServiceOptions {
            state_dir: Some(state_dir),
            config_path: None,
            initialize_event_store: true,
        },
        CancellationToken::new(),
    )
    .unwrap();

    let recovered = store.load_task(task.id).unwrap();
    assert_eq!(recovered.status, TaskStatus::Interrupted);
    assert!(recovered.finished_at.is_some());
    assert!(!recovered.is_cancellable);
    let expired = store.load_approval(approval.id).unwrap();
    assert_eq!(expired.status, ApprovalStatus::Expired);
    assert!(!expired.can_approve);
    let events = store.load_after(Some(session_id), 0, 100).unwrap();
    assert!(events.events.iter().any(|event| {
        matches!(
            &event.payload,
            AppEvent::TaskFailed(failure)
                if failure.task.id == task.id
                    && failure.task.status == TaskStatus::Interrupted
                    && failure.error.retryable
        )
    }));
}

#[tokio::test]
async fn session_snapshot_recovers_past_a_single_ten_thousand_event_batch() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("fixture.txt"), "broken\n").unwrap();
    let service = FixTraceAppService::start(
        AppServiceOptions {
            state_dir: Some(state_dir.clone()),
            config_path: None,
            initialize_event_store: true,
        },
        CancellationToken::new(),
    )
    .unwrap();
    let created = service
        .execute(AppCommand::InitializeSession {
            project,
            oracle: "false".to_owned(),
            title: Some("large timeline".to_owned()),
        })
        .await
        .unwrap();
    let AppResponse::SessionInitialized { session } = created else {
        panic!("InitializeSession returned the wrong response");
    };
    let store = EventStore::open(state_dir.join("history.sqlite3")).unwrap();
    let now = chrono::Utc::now();
    for index in 0..10_005_u64 {
        store
            .append(
                Some(session.id),
                None,
                AppEvent::ItemCompleted(TimelineItem::Notice(NoticeItem {
                    header: TimelineItemHeader {
                        id: uuid::Uuid::new_v4(),
                        status: ItemStatus::Completed,
                        started_at: now,
                        completed_at: Some(now),
                        parent_id: None,
                        artifacts: Vec::new(),
                        entities: Vec::new(),
                    },
                    notice: Notice {
                        code: format!("fixture_{index}"),
                        level: NoticeLevel::Info,
                        title: "Fixture".to_owned(),
                        message: format!("timeline item {index}"),
                    },
                })),
            )
            .unwrap();
    }

    let response = service
        .execute_protocol(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
                session_id: session.id,
                timeline_page: PageRequest {
                    cursor: None,
                    limit: Some(10_000),
                },
            }),
        )
        .await
        .unwrap();
    let AppResponsePayload::SessionSnapshot(snapshot) = response else {
        panic!("session/get_snapshot returned the wrong response");
    };
    assert_eq!(snapshot.session.timeline.len(), 10_005);
    assert_eq!(snapshot.through_sequence, 10_006);
}
