use std::{collections::VecDeque, sync::Arc};

use async_trait::async_trait;
use fixtrace::application::FixTraceProtocolApplication;
use fixtrace_protocol::{
    AppErrorView, AppRequest, AppResponsePayload, ClientInfo, EventEnvelope, EventGap,
    InitializeRequest, InitializeResponse, SubscribeRequest,
};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast, mpsc};
use uuid::Uuid;

mod websocket;

pub use websocket::WebSocketClient;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("FixTrace client must initialize before sending requests")]
    NotInitialized,
    #[error("FixTrace protocol error: {0:?}")]
    Protocol(AppErrorView),
    #[error("FixTrace event stream has a gap: {0:?}")]
    EventGap(EventGap),
    #[error("FixTrace event stream closed")]
    EventStreamClosed,
    #[error("FixTrace transport error: {0}")]
    Transport(String),
}

#[async_trait]
pub trait AppClient: Send + Sync {
    async fn initialize(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, ClientError>;

    async fn request(&self, request: AppRequest) -> Result<AppResponsePayload, ClientError>;

    async fn request_with_operation(
        &self,
        operation_id: Uuid,
        request: AppRequest,
    ) -> Result<AppResponsePayload, ClientError>;

    async fn subscribe(&self, request: SubscribeRequest) -> Result<EventSubscription, ClientError>;
}

pub struct InProcessClient {
    application: Arc<dyn FixTraceProtocolApplication>,
    initialized: Mutex<Option<InitializeResponse>>,
}

impl InProcessClient {
    pub fn new(application: Arc<dyn FixTraceProtocolApplication>) -> Self {
        Self {
            application,
            initialized: Mutex::new(None),
        }
    }

    pub fn client_info() -> ClientInfo {
        ClientInfo {
            name: "fixtrace-in-process".to_owned(),
            title: "FixTrace InProcess Client".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    async fn initialized(&self) -> Result<InitializeResponse, ClientError> {
        self.initialized
            .lock()
            .await
            .clone()
            .ok_or(ClientError::NotInitialized)
    }
}

#[async_trait]
impl AppClient for InProcessClient {
    async fn initialize(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, ClientError> {
        let response = self
            .application
            .initialize_protocol(request)
            .await
            .map_err(ClientError::Protocol)?;
        *self.initialized.lock().await = Some(response.clone());
        Ok(response)
    }

    async fn request(&self, request: AppRequest) -> Result<AppResponsePayload, ClientError> {
        self.request_with_operation(Uuid::new_v4(), request).await
    }

    async fn request_with_operation(
        &self,
        operation_id: Uuid,
        request: AppRequest,
    ) -> Result<AppResponsePayload, ClientError> {
        let initialized = self.initialized().await?;
        self.application
            .execute_protocol(initialized.client_id, operation_id, request)
            .await
            .map_err(ClientError::Protocol)
    }

    async fn subscribe(&self, request: SubscribeRequest) -> Result<EventSubscription, ClientError> {
        self.initialized().await?;
        let live = self.application.subscribe_protocol_events();
        let mut after = request.after_sequence.unwrap_or(0);
        let mut pending = VecDeque::new();
        let first = self
            .application
            .catch_up_protocol_events(Some(request.session_id), after, 10_000)
            .map_err(ClientError::Protocol)?;
        let target = first.high_watermark;
        let mut gap = first.gap;
        for event in first.events {
            after = after.max(event.sequence);
            pending.push_back(event);
        }
        while gap.is_none() && after < target {
            let batch = self
                .application
                .catch_up_protocol_events(Some(request.session_id), after, 10_000)
                .map_err(ClientError::Protocol)?;
            if batch.events.is_empty() {
                break;
            }
            gap = batch.gap;
            for event in batch.events {
                after = after.max(event.sequence);
                pending.push_back(event);
            }
        }
        Ok(EventSubscription {
            session_id: Some(request.session_id),
            pending,
            live: EventSource::InProcess(live),
            last_sequence: request.after_sequence.unwrap_or(0),
            initial_gap: gap,
        })
    }
}

pub struct EventSubscription {
    session_id: Option<Uuid>,
    pending: VecDeque<EventEnvelope>,
    live: EventSource,
    last_sequence: u64,
    initial_gap: Option<EventGap>,
}

enum EventSource {
    InProcess(broadcast::Receiver<EventEnvelope>),
    External(mpsc::Receiver<Result<EventEnvelope, ClientError>>),
}

impl EventSubscription {
    pub(crate) fn external(
        session_id: Uuid,
        after_sequence: u64,
        receiver: mpsc::Receiver<Result<EventEnvelope, ClientError>>,
    ) -> Self {
        Self {
            session_id: Some(session_id),
            pending: VecDeque::new(),
            live: EventSource::External(receiver),
            last_sequence: after_sequence,
            initial_gap: None,
        }
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub async fn recv(&mut self) -> Result<EventEnvelope, ClientError> {
        if let Some(gap) = self.initial_gap.take() {
            return Err(ClientError::EventGap(gap));
        }
        if let Some(event) = self.pending.pop_front() {
            return self.accept(event);
        }
        loop {
            let event = match &mut self.live {
                EventSource::InProcess(live) => match live.recv().await {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(ClientError::EventStreamClosed);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        return Err(ClientError::EventGap(EventGap {
                            stream_id: Uuid::nil(),
                            expected_sequence: self.last_sequence.saturating_add(1),
                            available_from_sequence: self.last_sequence.saturating_add(1),
                            high_watermark: self.last_sequence,
                            reason: "InProcess receiver lagged behind the bounded live buffer"
                                .to_owned(),
                        }));
                    }
                },
                EventSource::External(live) => match live.recv().await {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => return Err(error),
                    None => return Err(ClientError::EventStreamClosed),
                },
            };
            if event.session_id != self.session_id || event.sequence <= self.last_sequence {
                continue;
            }
            return self.accept(event);
        }
    }

    fn accept(&mut self, event: EventEnvelope) -> Result<EventEnvelope, ClientError> {
        let expected = self.last_sequence.saturating_add(1);
        if event.sequence != expected {
            return Err(ClientError::EventGap(EventGap {
                stream_id: event.stream_id,
                expected_sequence: expected,
                available_from_sequence: event.sequence,
                high_watermark: event.sequence,
                reason: "event sequence is not contiguous".to_owned(),
            }));
        }
        self.last_sequence = event.sequence;
        Ok(event)
    }
}
