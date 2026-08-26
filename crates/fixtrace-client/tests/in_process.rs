use std::{fs, sync::Arc, time::Duration};

use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceApplication};
use fixtrace_client::{AppClient, ClientError, InProcessClient};
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ClientCapabilities, ConfigEntryUpdate,
    ConfigUpdateRequest, ConfigValue, InitializeRequest, PROTOCOL_VERSION, PageRequest,
    SessionCreateRequest, SessionSnapshotRequest, SubscribeRequest, TaskIdRequest, TaskInput,
    TaskStartRequest, TaskStatus,
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
