use std::sync::Arc;

use fixtrace::application::{AppServiceOptions, FixTraceAppService, FixTraceProtocolApplication};
use fixtrace_protocol::{
    AppEvent, AppRequest, ClientCapabilities, ClientInfo, ErrorCode, InitializeRequest, Notice,
    NoticeLevel, PROTOCOL_VERSION, ServerFrame, SubscribeRequest,
};
use fixtrace_server::{ConnectionAction, ConnectionState};
use fixtrace_store::EventStore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn initialized_request() -> AppRequest {
    AppRequest::Initialize(InitializeRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        client: ClientInfo {
            name: "server-test".to_owned(),
            title: "Server Test".to_owned(),
            version: "1".to_owned(),
        },
        capabilities: ClientCapabilities {
            supports_streaming: true,
            supports_approvals: true,
            supports_diff: true,
            supports_graph: true,
            supports_artifacts: true,
        },
    })
}

fn envelope(request: AppRequest) -> fixtrace_protocol::RequestEnvelope {
    request
        .into_envelope(Uuid::new_v4(), Uuid::new_v4())
        .unwrap()
}

fn application(path: &std::path::Path) -> Arc<dyn FixTraceProtocolApplication> {
    Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(path.to_path_buf()),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    )
}

fn error_code(reply: fixtrace_server::ConnectionReply) -> ErrorCode {
    let ServerFrame::Response(response) = reply.frame else {
        panic!("expected response frame")
    };
    response.error.expect("expected error response").code
}

#[tokio::test]
async fn connection_requires_exactly_one_initialize_before_other_requests() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = ConnectionState::new(application(temp.path()));
    let before_initialize = AppRequest::ConfigGet(fixtrace_protocol::EmptyRequest {});
    assert_eq!(
        error_code(connection.handle_request(envelope(before_initialize)).await),
        ErrorCode::NotInitialized
    );

    let initialized = connection
        .handle_request(envelope(initialized_request()))
        .await;
    let ServerFrame::Response(response) = initialized.frame else {
        panic!("expected response frame")
    };
    assert!(response.error.is_none());
    assert!(response.result.is_some());

    assert_eq!(
        error_code(
            connection
                .handle_request(envelope(initialized_request()))
                .await
        ),
        ErrorCode::AlreadyInitialized
    );
    assert_eq!(
        error_code(connection.handle_text("{not-json").await),
        ErrorCode::InvalidRequest
    );
}

#[tokio::test]
async fn duplicate_request_ids_are_rejected_without_reexecuting() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = ConnectionState::new(application(temp.path()));
    connection
        .handle_request(envelope(initialized_request()))
        .await;
    let request = envelope(AppRequest::ConfigGet(fixtrace_protocol::EmptyRequest {}));
    let first = connection.handle_request(request.clone()).await;
    let ServerFrame::Response(first) = first.frame else {
        panic!("expected response frame")
    };
    assert!(first.error.is_none());

    assert_eq!(
        error_code(connection.handle_request(request).await),
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn incompatible_protocol_returns_an_error_and_closes_the_connection() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = ConnectionState::new(application(temp.path()));
    let mut request = initialized_request();
    let AppRequest::Initialize(initialize) = &mut request else {
        unreachable!()
    };
    initialize.protocol_version = "fixtrace/999".to_owned();
    let reply = connection.handle_request(envelope(request)).await;
    assert!(matches!(reply.action, ConnectionAction::Close));
    let ServerFrame::Response(response) = reply.frame else {
        panic!("expected response frame")
    };
    assert_eq!(
        response.error.unwrap().code,
        ErrorCode::IncompatibleProtocol
    );
}

#[tokio::test]
async fn subscription_catches_up_then_accepts_the_next_live_event_once() {
    let temp = tempfile::tempdir().unwrap();
    let app = application(temp.path());
    let store = EventStore::open(temp.path().join("history.sqlite3")).unwrap();
    let session_id = Uuid::new_v4();
    store
        .append(
            Some(session_id),
            None,
            AppEvent::Notice(Notice {
                code: "persisted".to_owned(),
                level: NoticeLevel::Info,
                title: "Persisted".to_owned(),
                message: "catch up".to_owned(),
            }),
        )
        .unwrap();

    let mut connection = ConnectionState::new(app);
    connection
        .handle_request(envelope(initialized_request()))
        .await;
    let reply = connection
        .handle_request(envelope(AppRequest::EventSubscribe(SubscribeRequest {
            session_id,
            after_sequence: Some(0),
        })))
        .await;
    let caught_up = connection.apply_action(reply.action).unwrap();
    assert_eq!(caught_up.len(), 1);
    assert!(matches!(&caught_up[0], ServerFrame::Event(event) if event.sequence == 1));

    let live = store
        .append(
            Some(session_id),
            None,
            AppEvent::Notice(Notice {
                code: "live".to_owned(),
                level: NoticeLevel::Info,
                title: "Live".to_owned(),
                message: "next".to_owned(),
            }),
        )
        .unwrap();
    let delivered = connection.on_live_event(live.clone()).unwrap();
    assert_eq!(delivered.len(), 1);
    assert!(matches!(&delivered[0], ServerFrame::Event(event) if event.sequence == 2));
    assert!(connection.on_live_event(live).unwrap().is_empty());
}
