use std::time::Duration;

use async_trait::async_trait;
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ClientFrame, ClientInfo, EventEnvelope, EventGap,
    InitializeRequest, InitializeResponse, ServerFrame, SubscribeRequest,
};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::TcpStream,
    sync::{Mutex, mpsc},
    time::sleep,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
        protocol::WebSocketConfig,
    },
};
use url::Url;
use uuid::Uuid;

use crate::{AppClient, ClientError, EventSubscription};

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const EVENT_QUEUE_CAPACITY: usize = 256;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(100);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(5);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct WebSocketClient {
    url: String,
    token: String,
    initialized: Mutex<Option<(InitializeRequest, InitializeResponse)>>,
}

impl WebSocketClient {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Result<Self, ClientError> {
        let url = url.into();
        let parsed = Url::parse(&url)
            .map_err(|error| ClientError::Transport(format!("invalid WebSocket URL: {error}")))?;
        if !matches!(parsed.scheme(), "ws" | "wss")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ClientError::Transport(
                "WebSocket URL must not contain credentials, query, or fragment".to_owned(),
            ));
        }
        Ok(Self {
            url,
            token: token.into(),
            initialized: Mutex::new(None),
        })
    }

    pub fn client_info() -> ClientInfo {
        ClientInfo {
            name: "fixtrace-websocket".to_owned(),
            title: "FixTrace WebSocket Client".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    async fn initialization(&self) -> Result<InitializeRequest, ClientError> {
        self.initialized
            .lock()
            .await
            .as_ref()
            .map(|(request, _)| request.clone())
            .ok_or(ClientError::NotInitialized)
    }

    async fn connected_and_initialized(
        &self,
        initialize: &InitializeRequest,
    ) -> Result<Socket, ClientError> {
        connected_and_initialized(&self.url, &self.token, initialize).await
    }
}

#[async_trait]
impl AppClient for WebSocketClient {
    async fn initialize(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, ClientError> {
        let mut socket = connect(&self.url, &self.token).await?;
        let response = send_request(
            &mut socket,
            Uuid::new_v4(),
            AppRequest::Initialize(request.clone()),
        )
        .await?;
        let AppResponsePayload::Initialized(initialized) = response else {
            return Err(ClientError::Transport(
                "initialize returned an unexpected response".to_owned(),
            ));
        };
        *self.initialized.lock().await = Some((request, initialized.clone()));
        Ok(initialized)
    }

    async fn request(&self, request: AppRequest) -> Result<AppResponsePayload, ClientError> {
        self.request_with_operation(Uuid::new_v4(), request).await
    }

    async fn request_with_operation(
        &self,
        operation_id: Uuid,
        request: AppRequest,
    ) -> Result<AppResponsePayload, ClientError> {
        let initialize = self.initialization().await?;
        let mut socket = self.connected_and_initialized(&initialize).await?;
        send_request(&mut socket, operation_id, request).await
    }

    async fn subscribe(&self, request: SubscribeRequest) -> Result<EventSubscription, ClientError> {
        let initialize = self.initialization().await?;
        let url = self.url.clone();
        let token = self.token.clone();
        let session_id = request.session_id;
        let after_sequence = request.after_sequence.unwrap_or(0);
        let (sender, receiver) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        tokio::spawn(async move {
            reconnecting_subscription(
                &url,
                &token,
                &initialize,
                session_id,
                after_sequence,
                sender,
            )
            .await;
        });
        Ok(EventSubscription::external(
            session_id,
            after_sequence,
            receiver,
        ))
    }
}

async fn connect(url: &str, token: &str) -> Result<Socket, ClientError> {
    let mut request = url
        .into_client_request()
        .map_err(|error| ClientError::Transport(error.to_string()))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| ClientError::Transport("invalid bearer token".to_owned()))?;
    request.headers_mut().insert(AUTHORIZATION, authorization);
    let config = WebSocketConfig::default()
        .read_buffer_size(256 * 1024)
        .write_buffer_size(256 * 1024)
        .max_write_buffer_size(16 * 1024 * 1024)
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    connect_async_with_config(request, Some(config), false)
        .await
        .map(|(socket, _)| socket)
        .map_err(|error| ClientError::Transport(error.to_string()))
}

async fn connected_and_initialized(
    url: &str,
    token: &str,
    initialize: &InitializeRequest,
) -> Result<Socket, ClientError> {
    let mut socket = connect(url, token).await?;
    let response = send_request(
        &mut socket,
        Uuid::new_v4(),
        AppRequest::Initialize(initialize.clone()),
    )
    .await?;
    if !matches!(response, AppResponsePayload::Initialized(_)) {
        return Err(ClientError::Transport(
            "initialize returned an unexpected response".to_owned(),
        ));
    }
    Ok(socket)
}

async fn send_request(
    socket: &mut Socket,
    operation_id: Uuid,
    request: AppRequest,
) -> Result<AppResponsePayload, ClientError> {
    let request_id = Uuid::new_v4();
    let frame = ClientFrame::Request(
        request
            .into_envelope(request_id, operation_id)
            .map_err(|error| ClientError::Transport(error.to_string()))?,
    );
    let text =
        serde_json::to_string(&frame).map_err(|error| ClientError::Transport(error.to_string()))?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))?;
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let frame: ServerFrame = serde_json::from_str(text.as_ref())
                    .map_err(|error| ClientError::Transport(error.to_string()))?;
                let ServerFrame::Response(response) = frame else {
                    continue;
                };
                if response.id != request_id {
                    continue;
                }
                if let Some(error) = response.error {
                    return Err(ClientError::Protocol(error));
                }
                let result = response.result.ok_or_else(|| {
                    ClientError::Transport("response has neither result nor error".to_owned())
                })?;
                return serde_json::from_value(result)
                    .map_err(|error| ClientError::Transport(error.to_string()));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                return Err(ClientError::Transport(
                    "server sent a non-text protocol frame".to_owned(),
                ));
            }
            Some(Ok(Message::Close(_))) | None => return Err(ClientError::EventStreamClosed),
            Some(Err(error)) => return Err(ClientError::Transport(error.to_string())),
        }
    }
}

async fn reconnecting_subscription(
    url: &str,
    token: &str,
    initialize: &InitializeRequest,
    session_id: Uuid,
    initial_after: u64,
    sender: mpsc::Sender<Result<EventEnvelope, ClientError>>,
) {
    let mut after = initial_after;
    let mut delay = INITIAL_RECONNECT_DELAY;
    loop {
        if sender.is_closed() {
            return;
        }
        match subscription_connection(url, token, initialize, session_id, &mut after, &sender).await
        {
            SubscriptionExit::ReceiverClosed | SubscriptionExit::Terminal => return,
            SubscriptionExit::Disconnected => {
                sleep(delay).await;
                delay = (delay * 2).min(MAX_RECONNECT_DELAY);
            }
        }
    }
}

enum SubscriptionExit {
    Disconnected,
    ReceiverClosed,
    Terminal,
}

async fn subscription_connection(
    url: &str,
    token: &str,
    initialize: &InitializeRequest,
    session_id: Uuid,
    after: &mut u64,
    sender: &mpsc::Sender<Result<EventEnvelope, ClientError>>,
) -> SubscriptionExit {
    let mut socket = match connected_and_initialized(url, token, initialize).await {
        Ok(socket) => socket,
        Err(_) => return SubscriptionExit::Disconnected,
    };
    if send_request(
        &mut socket,
        Uuid::new_v4(),
        AppRequest::EventSubscribe(SubscribeRequest {
            session_id,
            after_sequence: Some(*after),
        }),
    )
    .await
    .is_err()
    {
        return SubscriptionExit::Disconnected;
    }
    while let Some(message) = socket.next().await {
        let Ok(Message::Text(text)) = message else {
            return SubscriptionExit::Disconnected;
        };
        let Ok(ServerFrame::Event(event)) = serde_json::from_str(text.as_ref()) else {
            continue;
        };
        if event.session_id != Some(session_id) || event.sequence <= *after {
            continue;
        }
        if let AppEvent::EventGap(gap) = &event.payload {
            return if sender
                .send(Err(ClientError::EventGap(gap.clone())))
                .await
                .is_ok()
            {
                SubscriptionExit::Terminal
            } else {
                SubscriptionExit::ReceiverClosed
            };
        }
        let expected = after.saturating_add(1);
        if event.sequence != expected {
            let gap = EventGap {
                stream_id: event.stream_id,
                expected_sequence: expected,
                available_from_sequence: event.sequence,
                high_watermark: event.sequence,
                reason: "WebSocket event sequence is not contiguous".to_owned(),
            };
            return if sender.send(Err(ClientError::EventGap(gap))).await.is_ok() {
                SubscriptionExit::Terminal
            } else {
                SubscriptionExit::ReceiverClosed
            };
        }
        *after = event.sequence;
        if sender.send(Ok(*event)).await.is_err() {
            return SubscriptionExit::ReceiverClosed;
        }
    }
    SubscriptionExit::Disconnected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_credentials_in_websocket_urls() {
        assert!(WebSocketClient::new("ws://127.0.0.1:4765", "token").is_ok());
        assert!(WebSocketClient::new("ws://user@127.0.0.1:4765", "token").is_err());
        assert!(WebSocketClient::new("ws://127.0.0.1:4765?api_key=x", "token").is_err());
    }
}
