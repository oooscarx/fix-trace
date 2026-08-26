use std::sync::Arc;

use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceProtocolApplication};
use fixtrace_protocol::{
    AppRequest, ClientCapabilities, ClientFrame, ClientInfo, InitializeRequest, PROTOCOL_VERSION,
    ServerFrame,
};
use fixtrace_server::serve_websocket;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Error, Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
    },
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn websocket_requires_bearer_auth_and_serves_protocol_frames() {
    let temp = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::new();
    let application: Arc<dyn FixTraceProtocolApplication> = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(temp.path().to_path_buf()),
                config_path: None,
                initialize_event_store: true,
            },
            cancellation.clone(),
        )
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let token = "a".repeat(64);
    let server_cancellation = cancellation.clone();
    let server = tokio::spawn(serve_websocket(
        listener,
        application,
        token.clone(),
        server_cancellation,
    ));
    let url = format!("ws://{address}/");

    let unauthorized = connect_async(&url).await.unwrap_err();
    assert!(matches!(
        unauthorized,
        Error::Http(response) if response.status().as_u16() == 401
    ));

    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    let (mut socket, _) = connect_async(request).await.unwrap();
    let initialize = AppRequest::Initialize(InitializeRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        client: ClientInfo {
            name: "ws-test".to_owned(),
            title: "WebSocket Test".to_owned(),
            version: "1".to_owned(),
        },
        capabilities: ClientCapabilities {
            supports_streaming: true,
            supports_approvals: true,
            supports_diff: true,
            supports_graph: true,
            supports_artifacts: true,
        },
    });
    let frame = ClientFrame::Request(
        initialize
            .into_envelope(Uuid::new_v4(), Uuid::new_v4())
            .unwrap(),
    );
    socket
        .send(Message::Text(serde_json::to_string(&frame).unwrap().into()))
        .await
        .unwrap();
    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
        panic!("expected a text response")
    };
    let response: ServerFrame = serde_json::from_str(text.as_ref()).unwrap();
    assert!(matches!(
        response,
        ServerFrame::Response(response) if response.result.is_some() && response.error.is_none()
    ));

    socket.close(None).await.unwrap();
    cancellation.cancel();
    server.await.unwrap().unwrap();
}
