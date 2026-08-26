use std::{fs, sync::Arc, time::Duration};

use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceProtocolApplication};
use fixtrace_client::{AppClient, WebSocketClient};
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ClientCapabilities, InitializeRequest,
    PROTOCOL_VERSION, SessionCreateRequest, SubscribeRequest, TaskInput, TaskStartRequest,
};
use fixtrace_server::serve_websocket;
use tokio::{net::TcpListener, time::timeout};
use tokio_util::sync::CancellationToken;

fn initialize_request(name: &str) -> InitializeRequest {
    InitializeRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        client: fixtrace_protocol::ClientInfo {
            name: name.to_owned(),
            title: name.to_owned(),
            version: "1".to_owned(),
        },
        capabilities: ClientCapabilities {
            supports_streaming: true,
            supports_approvals: true,
            supports_diff: true,
            supports_graph: true,
            supports_artifacts: true,
        },
    }
}

#[tokio::test]
async fn two_websocket_clients_observe_the_same_task_event() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("fixture.txt"), "broken").unwrap();
    let cancellation = CancellationToken::new();
    let application: Arc<dyn FixTraceProtocolApplication> = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(state_dir),
                config_path: None,
                initialize_event_store: true,
            },
            cancellation.clone(),
        )
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let token = "c".repeat(64);
    let server = tokio::spawn(serve_websocket(
        listener,
        application,
        token.clone(),
        cancellation.clone(),
    ));
    let first = WebSocketClient::new(format!("ws://{address}/"), &token).unwrap();
    let second = WebSocketClient::new(format!("ws://{address}/"), &token).unwrap();
    first.initialize(initialize_request("first")).await.unwrap();
    second
        .initialize(initialize_request("second"))
        .await
        .unwrap();

    let created = first
        .request(AppRequest::SessionCreate(SessionCreateRequest {
            project,
            oracle: "false".to_owned(),
            title: None,
        }))
        .await
        .unwrap();
    let AppResponsePayload::Session(session) = created else {
        panic!("session/create returned an unexpected response")
    };
    let subscribe = SubscribeRequest {
        session_id: session.id,
        after_sequence: Some(1),
    };
    let mut first_events = first.subscribe(subscribe.clone()).await.unwrap();
    let mut second_events = second.subscribe(subscribe).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    first
        .request(AppRequest::TaskStart(TaskStartRequest {
            session_id: Some(session.id),
            input: TaskInput::ExportSession {
                output: temp.path().join("session.json"),
            },
        }))
        .await
        .unwrap();
    let first_event = timeout(Duration::from_secs(5), first_events.recv())
        .await
        .unwrap()
        .unwrap();
    let second_event = timeout(Duration::from_secs(5), second_events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_event.event_id, second_event.event_id);
    assert_eq!(first_event.sequence, second_event.sequence);
    assert!(matches!(first_event.payload, AppEvent::TaskStarted(_)));
    assert!(matches!(second_event.payload, AppEvent::TaskStarted(_)));

    drop(first_events);
    drop(second_events);
    cancellation.cancel();
    server.await.unwrap().unwrap();
}
