use std::{fs, sync::Arc, time::Duration};

use base64::Engine as _;
use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceApplication};
use fixtrace::{
    domain::{
        action::{Action, ActionKind, ActionResult, ArtifactRef},
        snapshot::SnapshotDelta,
    },
    history::database::HistoryDatabase,
};
use fixtrace_client::{AppClient, ClientError, InProcessClient};
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ApprovalChoice, ApprovalKind, ApprovalRespondRequest,
    ArtifactReadRequest, ClientCapabilities, ConfigEntryUpdate, ConfigUpdateRequest, ConfigValue,
    ConnectionTestRequest, InitializeRequest, PROTOCOL_VERSION, PageRequest, SessionCreateRequest,
    SessionForkRequest, SessionIdRequest, SessionPageRequest, SessionSnapshotRequest,
    SubscribeRequest, TaskIdRequest, TaskInput, TaskStartRequest, TaskStatus, TrialRepeatRequest,
    TrialRunRequest,
};
use tempfile::tempdir;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn in_process_client_catches_up_then_receives_live_events_in_order() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("fixture.txt"), "broken").unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(temp.path().join("state")),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let client = InProcessClient::new(service);

    assert!(matches!(
        client
            .request(AppRequest::ConfigGet(fixtrace_protocol::EmptyRequest {}))
            .await,
        Err(ClientError::NotInitialized)
    ));
    let mut incompatible = initialize_request();
    incompatible.protocol_version = "fixtrace/999".to_owned();
    assert!(matches!(
        client.initialize(incompatible).await,
        Err(ClientError::Protocol(error))
            if error.code == fixtrace_protocol::ErrorCode::IncompatibleProtocol
    ));
    client.initialize(initialize_request()).await.unwrap();
    client
        .request(AppRequest::ConfigUpdate(ConfigUpdateRequest {
            updates: vec![ConfigEntryUpdate {
                key: "replay.repetitions".to_owned(),
                value: ConfigValue::Integer(1),
            }],
        }))
        .await
        .unwrap();

    let created = client
        .request(AppRequest::SessionCreate(SessionCreateRequest {
            project,
            oracle: "false".to_owned(),
            title: None,
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(session) = created else {
        panic!("session/create returned the wrong response");
    };

    let mut subscription = client
        .subscribe(SubscribeRequest {
            session_id: session.id,
            after_sequence: Some(0),
        })
        .await
        .unwrap();
    let created_event = subscription.recv().await.unwrap();
    assert_eq!(created_event.sequence, 1);
    assert!(matches!(created_event.payload, AppEvent::SessionCreated(_)));

    let export_path = temp.path().join("export.json");
    let operation_id = Uuid::new_v4();
    let task_request = AppRequest::TaskStart(TaskStartRequest {
        session_id: Some(session.id),
        input: TaskInput::ExportSession {
            output: export_path.clone(),
        },
    });
    let started = client
        .request_with_operation(operation_id, task_request.clone())
        .await
        .unwrap();
    let AppResponsePayload::Task(first_task) = started else {
        panic!("task/start returned the wrong response");
    };
    let retried = client
        .request_with_operation(operation_id, task_request)
        .await
        .unwrap();
    let AppResponsePayload::Task(retried_task) = retried else {
        panic!("idempotent task/start returned the wrong response");
    };
    assert_eq!(retried_task.id, first_task.id);

    let mut sequences = vec![created_event.sequence];
    loop {
        let event = timeout(Duration::from_secs(5), subscription.recv())
            .await
            .expect("task event should arrive")
            .expect("task event stream should stay contiguous");
        sequences.push(event.sequence);
        if let AppEvent::ApprovalRequested(request) = &event.payload {
            client
                .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
                    approval_id: request.id,
                    choice: ApprovalChoice::ApproveOnce,
                }))
                .await
                .unwrap();
        }
        if matches!(event.payload, AppEvent::TaskCompleted(_)) {
            break;
        }
    }
    assert_eq!(
        sequences,
        (1..=u64::try_from(sequences.len()).unwrap()).collect::<Vec<_>>()
    );
    assert!(export_path.is_file());

    let snapshot = client
        .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
            session_id: session.id,
            timeline_page: PageRequest {
                cursor: None,
                limit: Some(100),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::SessionSnapshot(snapshot) = snapshot else {
        panic!("snapshot returned the wrong response");
    };
    assert_eq!(snapshot.session.summary.id, session.id);
    assert!(snapshot.through_sequence >= 3);
}

#[tokio::test]
async fn in_process_client_supports_complete_session_and_trial_workflow() {
    let temp = tempdir().unwrap();
    let state = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("fixture.txt"), "broken\n").unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(state.clone()),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let client = InProcessClient::new(service);
    client.initialize(initialize_request()).await.unwrap();
    let config = client
        .request(AppRequest::ConfigUpdate(ConfigUpdateRequest {
            updates: vec![
                ConfigEntryUpdate {
                    key: "replay.repetitions".to_owned(),
                    value: ConfigValue::Integer(1),
                },
                ConfigEntryUpdate {
                    key: "approval.policy".to_owned(),
                    value: ConfigValue::String("ask_for_opaque".to_owned()),
                },
            ],
        }))
        .await
        .unwrap();
    assert!(matches!(
        config,
        AppResponsePayload::Config(ref config)
            if config.approval_policy == fixtrace_protocol::ApprovalPolicy::AskForOpaque
    ));

    let created = client
        .request(AppRequest::SessionCreate(SessionCreateRequest {
            project,
            oracle: "test \"$(cat fixture.txt)\" = fixed".to_owned(),
            title: Some("named session".to_owned()),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(created) = created else {
        panic!("session/create returned the wrong response");
    };
    assert_eq!(created.project_name, "named session");

    let record = client
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(created.id),
            input: TaskInput::RecordTrace {
                line: "printf fixed > fixture.txt".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(record) = record else {
        panic!("record task returned the wrong response");
    };
    approve_pending_task(&client, created.id, record.id).await;
    wait_for_task(&client, record.id, TaskStatus::Completed).await;
    let snapshot = client
        .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
            session_id: created.id,
            timeline_page: PageRequest {
                cursor: None,
                limit: Some(100),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::SessionSnapshot(snapshot) = snapshot else {
        panic!("session/get_snapshot returned the wrong response");
    };
    assert!(snapshot.session.diff.files.iter().any(|file| {
        file.path == "fixture.txt"
            && file.change_kind.contains("content_modified")
            && file
                .unified_diff
                .as_deref()
                .is_some_and(|diff| diff.contains("-broken") && diff.contains("+fixed"))
    }));
    assert_eq!(snapshot.session.actions.len(), 1);

    let finish = client
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(created.id),
            input: TaskInput::RecordTrace {
                line: ":done".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(finish) = finish else {
        panic!("finish recording task returned the wrong response");
    };
    approve_pending_task(&client, created.id, finish.id).await;
    wait_for_task(&client, finish.id, TaskStatus::Completed).await;

    let trial = client
        .request(AppRequest::TrialRun(TrialRunRequest {
            session_id: created.id,
            action_ids: Vec::new(),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Trial(trial) = trial else {
        panic!("trial/run returned the wrong response");
    };
    let repeated = client
        .request(AppRequest::TrialRepeat(TrialRepeatRequest {
            session_id: created.id,
            trial_id: trial.id,
            repetitions: Some(1),
        }))
        .await
        .unwrap();
    assert!(matches!(repeated, AppResponsePayload::Trial(_)));

    let forked = client
        .request(AppRequest::SessionFork(SessionForkRequest {
            session_id: created.id,
            title: Some("forked session".to_owned()),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(forked) = forked else {
        panic!("session/fork returned the wrong response");
    };
    assert_eq!(forked.parent_session_id, Some(created.id));
    assert_eq!(forked.project_name, "forked session");
    let archived = client
        .request(AppRequest::SessionArchive(SessionIdRequest {
            session_id: forked.id,
        }))
        .await
        .unwrap();
    assert!(matches!(
        archived,
        AppResponsePayload::Session(ref session) if session.archived
    ));
}

#[tokio::test]
async fn independent_clients_project_the_same_authoritative_session_view() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("fixture.txt"), "broken\n").unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(temp.path().join("state")),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let first = InProcessClient::new(service.clone());
    let second = InProcessClient::new(service);
    first.initialize(initialize_request()).await.unwrap();
    second.initialize(initialize_request()).await.unwrap();

    let created = first
        .request(AppRequest::SessionCreate(SessionCreateRequest {
            project,
            oracle: "false".to_owned(),
            title: Some("shared projection".to_owned()),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(session) = created else {
        panic!("session/create returned the wrong response");
    };
    let page = PageRequest {
        cursor: None,
        limit: Some(500),
    };
    let first_snapshot = first
        .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
            session_id: session.id,
            timeline_page: page.clone(),
        }))
        .await
        .unwrap();
    let second_snapshot = second
        .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
            session_id: session.id,
            timeline_page: page,
        }))
        .await
        .unwrap();
    let (
        AppResponsePayload::SessionSnapshot(first_snapshot),
        AppResponsePayload::SessionSnapshot(second_snapshot),
    ) = (first_snapshot, second_snapshot)
    else {
        panic!("session/get_snapshot returned the wrong response");
    };
    assert_eq!(first_snapshot.stream_id, second_snapshot.stream_id);
    assert_eq!(
        first_snapshot.through_sequence,
        second_snapshot.through_sequence
    );
    assert_eq!(first_snapshot.session, second_snapshot.session);
}

#[cfg(unix)]
#[tokio::test]
async fn in_process_client_lists_and_reads_bounded_output_artifacts() {
    let temp = tempdir().unwrap();
    let state = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("fixture.txt"), "broken\n").unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(state.clone()),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let client = InProcessClient::new(service);
    client.initialize(initialize_request()).await.unwrap();

    let created = client
        .request(AppRequest::SessionCreate(SessionCreateRequest {
            project,
            oracle: "false".to_owned(),
            title: Some("artifact session".to_owned()),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(session) = created else {
        panic!("session/create returned the wrong response");
    };
    let recorded = client
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::RecordTrace {
                line: "yes x | head -c 2097152".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(recorded) = recorded else {
        panic!("record task returned the wrong response");
    };
    approve_pending_task(&client, session.id, recorded.id).await;
    wait_for_task(&client, recorded.id, TaskStatus::Completed).await;

    let listed = client
        .request(AppRequest::ArtifactList(SessionPageRequest {
            session_id: session.id,
            page: PageRequest {
                cursor: None,
                limit: Some(10),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::ArtifactList { artifacts, page } = listed else {
        panic!("artifact/list returned the wrong response");
    };
    assert_eq!(artifacts.len(), 1);
    assert!(!page.has_more);
    assert_eq!(artifacts[0].size, 2_097_152);
    assert!(artifacts[0].name.ends_with("stdout.txt"));

    let chunk = client
        .request(AppRequest::ArtifactRead(ArtifactReadRequest {
            artifact_id: artifacts[0].id,
            offset: 0,
            limit: u32::MAX,
        }))
        .await
        .unwrap();
    let AppResponsePayload::Artifact(chunk) = chunk else {
        panic!("artifact/read returned the wrong response");
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk.bytes_base64)
        .unwrap();
    assert_eq!(bytes.len(), 1_048_576);
    assert!(
        bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == if index % 2 == 0 { b'x' } else { b'\n' })
    );
    assert_eq!(chunk.offset, 0);
    assert_eq!(chunk.next_offset, 1_048_576);
    assert!(!chunk.eof);
    assert_eq!(chunk.sha256, artifacts[0].sha256);

    let second = client
        .request(AppRequest::ArtifactRead(ArtifactReadRequest {
            artifact_id: artifacts[0].id,
            offset: chunk.next_offset,
            limit: u32::MAX,
        }))
        .await
        .unwrap();
    let AppResponsePayload::Artifact(second) = second else {
        panic!("second artifact/read returned the wrong response");
    };
    let second_bytes = base64::engine::general_purpose::STANDARD
        .decode(second.bytes_base64)
        .unwrap();
    assert_eq!(second_bytes.len(), 1_048_576);
    assert_eq!(second.next_offset, 2_097_152);
    assert!(second.eof);
    assert_eq!(second.sha256, artifacts[0].sha256);

    let artifact_dir = state
        .join("sessions")
        .join(session.id.to_string())
        .join("artifacts");
    fs::create_dir_all(&artifact_dir).unwrap();
    let sparse_path = artifact_dir.join("sparse-256m.log");
    let sparse_size = 256_u64 * 1_024 * 1_024;
    fs::File::create(&sparse_path)
        .unwrap()
        .set_len(sparse_size)
        .unwrap();
    HistoryDatabase::open(state.join("history.sqlite3"))
        .unwrap()
        .save_action(
            session.id,
            &Action {
                id: 2,
                original_order: 2,
                cwd_before: Default::default(),
                kind: ActionKind::FilePatch { files: Vec::new() },
                replayable: true,
                note: Some("sparse artifact range fixture".to_owned()),
                result: Some(ActionResult {
                    exit_code: Some(0),
                    duration_ms: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    stdout_artifact: Some(ArtifactRef {
                        path: "artifacts/sparse-256m.log".into(),
                        size: sparse_size,
                        sha256: "sparse-zero-fixture".to_owned(),
                    }),
                    stderr_artifact: None,
                    timed_out: false,
                    cancelled: false,
                    before_snapshot_hash: "fixture".to_owned(),
                    after_snapshot_hash: "fixture".to_owned(),
                    delta: SnapshotDelta::default(),
                }),
            },
        )
        .unwrap();
    let listed = client
        .request(AppRequest::ArtifactList(SessionPageRequest {
            session_id: session.id,
            page: PageRequest {
                cursor: None,
                limit: Some(10),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::ArtifactList { artifacts, .. } = listed else {
        panic!("artifact/list returned the wrong response");
    };
    let sparse = artifacts
        .iter()
        .find(|artifact| artifact.name == "sparse-256m.log")
        .unwrap();
    assert_eq!(sparse.size, sparse_size);
    let tail = client
        .request(AppRequest::ArtifactRead(ArtifactReadRequest {
            artifact_id: sparse.id,
            offset: sparse_size - 128,
            limit: 1_024,
        }))
        .await
        .unwrap();
    let AppResponsePayload::Artifact(tail) = tail else {
        panic!("artifact/read returned the wrong response");
    };
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(tail.bytes_base64)
            .unwrap()
            .len(),
        128
    );
    assert_eq!(tail.next_offset, sparse_size);
    assert!(tail.eof);
}

#[tokio::test]
async fn connection_test_requires_an_environment_credential_reference() {
    let temp = tempdir().unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(temp.path().join("state")),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let client = InProcessClient::new(service);
    client.initialize(initialize_request()).await.unwrap();

    let response = client
        .request(AppRequest::ConfigTestConnection(ConnectionTestRequest {
            provider: "openai-compatible".to_owned(),
            endpoint: "https://example.invalid/v1".to_owned(),
            model: "test-model".to_owned(),
            credential_id: Some("FIXTRACE_TEST_CREDENTIAL_MUST_NOT_EXIST_7E3C".to_owned()),
        }))
        .await
        .unwrap();
    assert!(matches!(
        response,
        AppResponsePayload::ConnectionTest(ref result)
            if !result.ok && result.message.contains("is not configured")
    ));
}

#[tokio::test]
async fn approval_is_single_use_and_deny_prevents_command_execution() {
    let temp = tempdir().unwrap();
    let state = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("marker.txt"), "not executed\n").unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(state.clone()),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let first = InProcessClient::new(service.clone());
    let second = InProcessClient::new(service);
    first.initialize(initialize_request()).await.unwrap();
    second.initialize(initialize_request()).await.unwrap();
    first
        .request(AppRequest::ConfigUpdate(ConfigUpdateRequest {
            updates: vec![ConfigEntryUpdate {
                key: "approval.policy".to_owned(),
                value: ConfigValue::String("ask_always".to_owned()),
            }],
        }))
        .await
        .unwrap();
    let created = first
        .request(AppRequest::SessionCreate(SessionCreateRequest {
            project,
            oracle: "false".to_owned(),
            title: Some("approval safety".to_owned()),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(session) = created else {
        panic!("session/create returned the wrong response");
    };
    let worktree_marker = state
        .join("sessions")
        .join(session.id.to_string())
        .join("worktree")
        .join("marker.txt");

    let denied = first
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::RecordTrace {
                line: "printf executed > marker.txt".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(denied) = denied else {
        panic!("task/start returned the wrong response");
    };
    let approval_id = pending_approval(&first, session.id, denied.id).await;
    assert_eq!(
        fs::read_to_string(&worktree_marker).unwrap(),
        "not executed\n"
    );
    first
        .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
            approval_id,
            choice: ApprovalChoice::Deny,
        }))
        .await
        .unwrap();
    assert!(matches!(
        second
            .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
                approval_id,
                choice: ApprovalChoice::ApproveOnce,
            }))
            .await,
        Err(ClientError::Protocol(ref error))
            if error.code == fixtrace_protocol::ErrorCode::ApprovalResolved
    ));
    wait_for_task(&first, denied.id, TaskStatus::Cancelled).await;
    assert_eq!(
        fs::read_to_string(&worktree_marker).unwrap(),
        "not executed\n"
    );

    let approved = first
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::RecordTrace {
                line: "printf executed > marker.txt".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(approved) = approved else {
        panic!("task/start returned the wrong response");
    };
    approve_pending_task(&first, session.id, approved.id).await;
    wait_for_task(&first, approved.id, TaskStatus::Completed).await;
    assert_eq!(fs::read_to_string(&worktree_marker).unwrap(), "executed");

    let cancelled = first
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::RecordTrace {
                line: "printf cancelled > marker.txt".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(cancelled) = cancelled else {
        panic!("task/start returned the wrong response");
    };
    let cancelled_approval = pending_approval(&first, session.id, cancelled.id).await;
    second
        .request(AppRequest::TaskCancel(TaskIdRequest {
            task_id: cancelled.id,
        }))
        .await
        .unwrap();
    wait_for_task(&first, cancelled.id, TaskStatus::Cancelled).await;
    assert_eq!(fs::read_to_string(&worktree_marker).unwrap(), "executed");
    assert!(matches!(
        first
            .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
                approval_id: cancelled_approval,
                choice: ApprovalChoice::ApproveOnce,
            }))
            .await,
        Err(ClientError::Protocol(ref error))
            if error.code == fixtrace_protocol::ErrorCode::ApprovalResolved
    ));

    first
        .request(AppRequest::ConfigUpdate(ConfigUpdateRequest {
            updates: vec![ConfigEntryUpdate {
                key: "approval.policy".to_owned(),
                value: ConfigValue::String("read_only".to_owned()),
            }],
        }))
        .await
        .unwrap();
    let blocked = first
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::RecordTrace {
                line: "printf bypassed > marker.txt".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(blocked) = blocked else {
        panic!("task/start returned the wrong response");
    };
    wait_for_task(&first, blocked.id, TaskStatus::Failed).await;
    assert_eq!(fs::read_to_string(&worktree_marker).unwrap(), "executed");

    first
        .request(AppRequest::ConfigUpdate(ConfigUpdateRequest {
            updates: vec![ConfigEntryUpdate {
                key: "approval.policy".to_owned(),
                value: ConfigValue::String("ask_for_opaque".to_owned()),
            }],
        }))
        .await
        .unwrap();
    let network = first
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::RecordTrace {
                line: "curl https://example.invalid".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(network) = network else {
        panic!("task/start returned the wrong response");
    };
    let network_approval = pending_approval(&first, session.id, network.id).await;
    let snapshot = first
        .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
            session_id: session.id,
            timeline_page: PageRequest {
                cursor: None,
                limit: Some(100),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::SessionSnapshot(snapshot) = snapshot else {
        panic!("session/get_snapshot returned the wrong response");
    };
    let approval = snapshot
        .session
        .approvals
        .iter()
        .find(|approval| approval.request.id == network_approval)
        .unwrap();
    assert_eq!(approval.request.kind, ApprovalKind::NetworkAccess);
    assert!(approval.request.accesses_network);
    first
        .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
            approval_id: network_approval,
            choice: ApprovalChoice::Deny,
        }))
        .await
        .unwrap();
    wait_for_task(&first, network.id, TaskStatus::Cancelled).await;
}

#[tokio::test]
async fn protocol_task_cancel_reaches_a_running_operation() {
    let temp = tempdir().unwrap();
    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(temp.path().join("state")),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let mut events = service.subscribe_events();
    let client = InProcessClient::new(service);
    client.initialize(initialize_request()).await.unwrap();
    let started = client
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: None,
            input: TaskInput::Demo { no_llm: true },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(task) = started else {
        panic!("demo task did not start");
    };
    loop {
        let event = timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("demo task should start")
            .expect("event stream should remain open");
        if matches!(
            event.payload,
            AppEvent::TaskStarted(ref started) if started.id == task.id
        ) {
            break;
        }
    }
    client
        .request(AppRequest::TaskCancel(TaskIdRequest { task_id: task.id }))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = client
            .request(AppRequest::TaskGet(TaskIdRequest { task_id: task.id }))
            .await
            .unwrap();
        let AppResponsePayload::Task(current) = response else {
            panic!("task/get returned the wrong response");
        };
        if current.status.is_terminal() {
            assert_eq!(current.status, TaskStatus::Cancelled);
            break;
        }
        assert!(Instant::now() < deadline, "cancelled task did not stop");
        tokio::task::yield_now().await;
    }
}

fn initialize_request() -> InitializeRequest {
    InitializeRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        client: InProcessClient::client_info(),
        capabilities: ClientCapabilities {
            supports_streaming: true,
            supports_approvals: true,
            supports_diff: true,
            supports_graph: true,
            supports_artifacts: true,
        },
    }
}

async fn wait_for_task(client: &InProcessClient, task_id: Uuid, expected: TaskStatus) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = client
            .request(AppRequest::TaskGet(TaskIdRequest { task_id }))
            .await
            .unwrap();
        let AppResponsePayload::Task(task) = response else {
            panic!("task/get returned the wrong response");
        };
        if task.status.is_terminal() {
            assert_eq!(task.status, expected);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "task did not finish before deadline"
        );
        tokio::task::yield_now().await;
    }
}

async fn approve_pending_task(client: &InProcessClient, session_id: Uuid, task_id: Uuid) {
    let approval_id = pending_approval(client, session_id, task_id).await;
    client
        .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
            approval_id,
            choice: ApprovalChoice::ApproveOnce,
        }))
        .await
        .unwrap();
}

async fn pending_approval(client: &InProcessClient, session_id: Uuid, task_id: Uuid) -> Uuid {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = client
            .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
                session_id,
                timeline_page: PageRequest {
                    cursor: None,
                    limit: Some(100),
                },
            }))
            .await
            .unwrap();
        let AppResponsePayload::SessionSnapshot(snapshot) = snapshot else {
            panic!("session/get_snapshot returned the wrong response");
        };
        if let Some(approval) = snapshot
            .session
            .approvals
            .iter()
            .find(|approval| approval.request.task_id == task_id && approval.can_approve)
        {
            return approval.request.id;
        }
        assert!(
            Instant::now() < deadline,
            "task did not request approval before deadline"
        );
        tokio::task::yield_now().await;
    }
}
