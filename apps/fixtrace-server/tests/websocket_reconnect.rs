use std::{net::SocketAddr, sync::Arc, time::Duration};

use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceProtocolApplication};
use fixtrace_client::{AppClient, WebSocketClient};
use fixtrace_protocol::{
    AppEvent, ClientCapabilities, InitializeRequest, Notice, NoticeLevel, PROTOCOL_VERSION,
    SubscribeRequest,
};
use fixtrace_server::serve_websocket;
use fixtrace_store::EventStore;
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn application(
    state_dir: &std::path::Path,
    cancellation: CancellationToken,
) -> Arc<dyn FixTraceProtocolApplication> {
    Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(state_dir.to_path_buf()),
                config_path: None,
                initialize_event_store: true,
            },
            cancellation,
        )
        .unwrap(),
    )
}

async fn start_server(
    state_dir: &std::path::Path,
    address: SocketAddr,
    token: &str,
) -> (
    CancellationToken,
    JoinHandle<Result<(), fixtrace_server::WebSocketServerError>>,
) {
    let cancellation = CancellationToken::new();
    let listener = TcpListener::bind(address).await.unwrap();
    let server = tokio::spawn(serve_websocket(
        listener,
        application(state_dir, cancellation.clone()),
        token.to_owned(),
        cancellation.clone(),
    ));
    (cancellation, server)
}

#[tokio::test]
async fn websocket_subscription_reconnects_and_catches_up_from_persistent_events() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let token = "b".repeat(64);
    let (first_cancellation, first_server) = start_server(&state_dir, address, &token).await;

    let client = WebSocketClient::new(format!("ws://{address}/"), &token).unwrap();
    client
        .initialize(InitializeRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            client: WebSocketClient::client_info(),
            capabilities: ClientCapabilities {
                supports_streaming: true,
                supports_approvals: true,
                supports_diff: true,
                supports_graph: true,
                supports_artifacts: true,
            },
        })
        .await
        .unwrap();
    let session_id = Uuid::new_v4();
    let mut subscription = client
        .subscribe(SubscribeRequest {
            session_id,
            after_sequence: Some(0),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    first_cancellation.cancel();
    first_server.await.unwrap().unwrap();
    let store = EventStore::open(state_dir.join("history.sqlite3")).unwrap();
    store
        .append(
            Some(session_id),
            None,
            AppEvent::Notice(Notice {
                code: "offline".to_owned(),
                level: NoticeLevel::Info,
                title: "Offline event".to_owned(),
                message: "persisted while the server was down".to_owned(),
            }),
        )
        .unwrap();

    let (second_cancellation, second_server) = start_server(&state_dir, address, &token).await;
    let event = timeout(Duration::from_secs(5), subscription.recv())
        .await
        .expect("client should reconnect")
        .expect("catch-up should stay contiguous");
    assert_eq!(event.session_id, Some(session_id));
    assert_eq!(event.sequence, 1);
    assert!(matches!(event.payload, AppEvent::Notice(_)));

    drop(subscription);
    second_cancellation.cancel();
    second_server.await.unwrap().unwrap();
}
