use std::{fs, sync::Arc, time::Duration};

use base64::Engine as _;
use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceApplication};
use fixtrace_client::{AppClient, ClientError, InProcessClient};
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ArtifactReadRequest, ClientCapabilities,
    ConfigEntryUpdate, ConfigUpdateRequest, ConfigValue, ConnectionTestRequest, InitializeRequest,
    PROTOCOL_VERSION, PageRequest, SessionCreateRequest, SessionForkRequest, SessionIdRequest,
    SessionPageRequest, SessionSnapshotRequest, SubscribeRequest, TaskIdRequest, TaskInput,
    TaskStartRequest, TaskStatus, TrialRepeatRequest, TrialRunRequest,
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
                    value: ConfigValue::String("read_only".to_owned()),
                },
            ],
        }))
        .await
        .unwrap();
    assert!(matches!(
        config,
        AppResponsePayload::Config(ref config)
            if config.approval_policy == fixtrace_protocol::ApprovalPolicy::ReadOnly
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

#[cfg(unix)]
#[tokio::test]
async fn in_process_client_lists_and_reads_bounded_output_artifacts() {
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
                line: "yes x | head -c 70000".to_owned(),
            },
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(recorded) = recorded else {
        panic!("record task returned the wrong response");
    };
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
    assert_eq!(artifacts[0].size, 70_000);
    assert!(artifacts[0].name.ends_with("stdout.txt"));

    let chunk = client
        .request(AppRequest::ArtifactRead(ArtifactReadRequest {
            artifact_id: artifacts[0].id,
            offset: 1_024,
            limit: 4_096,
        }))
        .await
        .unwrap();
    let AppResponsePayload::Artifact(chunk) = chunk else {
        panic!("artifact/read returned the wrong response");
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk.bytes_base64)
        .unwrap();
    assert_eq!(bytes.len(), 4_096);
    assert!(
        bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == if index % 2 == 0 { b'x' } else { b'\n' })
    );
    assert_eq!(chunk.offset, 1_024);
    assert_eq!(chunk.next_offset, 5_120);
    assert!(!chunk.eof);
    assert_eq!(chunk.sha256, artifacts[0].sha256);
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
