use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::Arc,
};

use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::get,
};
use fixtrace::application::FixTraceProtocolApplication;
use fixtrace_protocol::{AppErrorView, ErrorCode, ResponseEnvelope, ServerFrame};
use futures_util::StreamExt;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{net::TcpListener, sync::broadcast};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};
use uuid::Uuid;

use crate::{ConnectionAction, ConnectionState, MAX_FRAME_BYTES};

#[derive(Debug, Error)]
pub enum WebSocketServerError {
    #[error("invalid WebSocket listen URL: {0}")]
    InvalidListenUrl(String),
    #[error("refusing non-loopback bind {0}; pass --allow-remote explicitly")]
    RemoteBindDenied(SocketAddr),
    #[error("WebSocket server failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
struct WebSocketState {
    application: Arc<dyn FixTraceProtocolApplication>,
    token: Arc<str>,
    cancellation: CancellationToken,
}

pub fn parse_ws_bind(listen: &str, allow_remote: bool) -> Result<SocketAddr, WebSocketServerError> {
    let url = Url::parse(listen)
        .map_err(|error| WebSocketServerError::InvalidListenUrl(error.to_string()))?;
    if url.scheme() != "ws"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(WebSocketServerError::InvalidListenUrl(
            "expected ws://HOST:PORT with no credentials, path, query, or fragment".to_owned(),
        ));
    }
    let port = url.port_or_known_default().ok_or_else(|| {
        WebSocketServerError::InvalidListenUrl("listen URL is missing a port".to_owned())
    })?;
    let address = match url.host().ok_or_else(|| {
        WebSocketServerError::InvalidListenUrl("listen URL is missing a host".to_owned())
    })? {
        Host::Ipv4(ip) => SocketAddr::new(IpAddr::V4(ip), port),
        Host::Ipv6(ip) => SocketAddr::new(IpAddr::V6(ip), port),
        Host::Domain("localhost") => SocketAddr::from(([127, 0, 0, 1], port)),
        Host::Domain(host) => (host, port)
            .to_socket_addrs()
            .map_err(|error| WebSocketServerError::InvalidListenUrl(error.to_string()))?
            .next()
            .ok_or_else(|| {
                WebSocketServerError::InvalidListenUrl("host did not resolve".to_owned())
            })?,
    };
    if !address.ip().is_loopback() && !allow_remote {
        return Err(WebSocketServerError::RemoteBindDenied(address));
    }
    Ok(address)
}

pub async fn serve_websocket(
    listener: TcpListener,
    application: Arc<dyn FixTraceProtocolApplication>,
    token: String,
    cancellation: CancellationToken,
) -> Result<(), WebSocketServerError> {
    let state = WebSocketState {
        application,
        token: Arc::from(token),
        cancellation: cancellation.clone(),
    };
    let router = Router::new().route("/", get(upgrade)).with_state(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await?;
    Ok(())
}

async fn upgrade(
    State(state): State<WebSocketState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    websocket
        .read_buffer_size(256 * 1024)
        .write_buffer_size(256 * 1024)
        .max_write_buffer_size(16 * 1024 * 1024)
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| serve_connection(socket, state.application, state.cancellation))
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    bool::from(value.as_bytes().ct_eq(expected.as_bytes()))
}

async fn serve_connection(
    mut socket: WebSocket,
    application: Arc<dyn FixTraceProtocolApplication>,
    cancellation: CancellationToken,
) {
    let mut events = application.subscribe_protocol_events();
    let mut connection = ConnectionState::new(application);
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let reply = connection.handle_text(text.as_str()).await;
                    if send_frame(&mut socket, reply.frame).await.is_err() {
                        break;
                    }
                    if matches!(reply.action, ConnectionAction::Close) {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    let frames = connection.apply_action(reply.action).unwrap_or_else(|error| {
                        vec![error_frame(error)]
                    });
                    if send_frames(&mut socket, frames).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Binary(_))) => {
                    if send_frame(
                        &mut socket,
                        error_frame(AppErrorView::new(
                            ErrorCode::InvalidRequest,
                            "binary WebSocket frames are not supported",
                        )),
                    ).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Err(error)) => {
                    tracing::debug!(%error, "WebSocket client disconnected");
                    break;
                }
            },
            event = events.recv() => match event {
                Ok(event) => {
                    let frames = connection.on_live_event(event).unwrap_or_else(|error| {
                        vec![error_frame(error)]
                    });
                    if send_frames(&mut socket, frames).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let frames = connection.recover_lagged().unwrap_or_else(|error| {
                        vec![error_frame(error)]
                    });
                    if send_frames(&mut socket, frames).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

async fn send_frames(socket: &mut WebSocket, frames: Vec<ServerFrame>) -> Result<(), axum::Error> {
    for frame in frames {
        send_frame(socket, frame).await?;
    }
    Ok(())
}

async fn send_frame(socket: &mut WebSocket, frame: ServerFrame) -> Result<(), axum::Error> {
    let text = serde_json::to_string(&frame).unwrap_or_else(|_| {
        serde_json::to_string(&error_frame(AppErrorView::new(
            ErrorCode::Internal,
            "failed to serialize server frame",
        )))
        .expect("the static fallback frame is serializable")
    });
    socket.send(Message::Text(text.into())).await
}

fn error_frame(error: AppErrorView) -> ServerFrame {
    ServerFrame::Response(ResponseEnvelope::error(Uuid::nil(), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_parser_defaults_to_loopback_only() {
        assert_eq!(
            parse_ws_bind("ws://127.0.0.1:4765", false).unwrap(),
            SocketAddr::from(([127, 0, 0, 1], 4765))
        );
        assert!(matches!(
            parse_ws_bind("ws://0.0.0.0:4765", false),
            Err(WebSocketServerError::RemoteBindDenied(_))
        ));
        assert!(parse_ws_bind("ws://0.0.0.0:4765", true).is_ok());
        assert!(parse_ws_bind("ws://127.0.0.1:4765?token=secret", false).is_err());
    }
}
