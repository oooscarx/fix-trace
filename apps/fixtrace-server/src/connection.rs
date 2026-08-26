use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use fixtrace::application::FixTraceProtocolApplication;
use fixtrace_protocol::{
    AppErrorView, AppEvent, AppRequest, AppResponsePayload, ClientFrame, EVENT_SCHEMA_VERSION,
    ErrorCode, EventEnvelope, EventGap, RequestEnvelope, ResponseEnvelope, ServerFrame,
    SubscribeRequest,
};
use uuid::Uuid;

pub struct ConnectionState {
    application: Arc<dyn FixTraceProtocolApplication>,
    client_id: Option<Uuid>,
    subscriptions: HashMap<Uuid, SubscriptionState>,
}

pub struct ConnectionReply {
    pub frame: ServerFrame,
    pub action: ConnectionAction,
}

#[derive(Clone, Debug)]
pub enum ConnectionAction {
    None,
    Close,
    Subscribe {
        subscription_id: Uuid,
        request: SubscribeRequest,
    },
    Unsubscribe {
        subscription_id: Uuid,
    },
}

struct SubscriptionState {
    session_id: Uuid,
    last_sequence: u64,
    gapped: bool,
}

impl ConnectionState {
    pub fn new(application: Arc<dyn FixTraceProtocolApplication>) -> Self {
        Self {
            application,
            client_id: None,
            subscriptions: HashMap::new(),
        }
    }

    pub async fn handle_text(&mut self, text: &str) -> ConnectionReply {
        let frame = match serde_json::from_str::<ClientFrame>(text) {
            Ok(frame) => frame,
            Err(error) => {
                return error_reply(
                    Uuid::nil(),
                    AppErrorView::new(
                        ErrorCode::InvalidRequest,
                        format!("invalid JSON frame: {error}"),
                    ),
                );
            }
        };
        let ClientFrame::Request(request) = frame;
        self.handle_request(request).await
    }

    pub async fn handle_request(&mut self, request: RequestEnvelope) -> ConnectionReply {
        let typed = match request.decode() {
            Ok(request) => request,
            Err(error) => return error_reply(request.id, error),
        };
        match typed {
            AppRequest::Initialize(initialize) => {
                if self.client_id.is_some() {
                    return error_reply(
                        request.id,
                        AppErrorView::new(
                            ErrorCode::AlreadyInitialized,
                            "connection has already initialized",
                        ),
                    );
                }
                match self.application.initialize_protocol(initialize).await {
                    Ok(response) => {
                        self.client_id = Some(response.client_id);
                        success_reply(request.id, &AppResponsePayload::Initialized(response))
                    }
                    Err(error) => {
                        let should_close = error.code == ErrorCode::IncompatibleProtocol;
                        let mut reply = error_reply(request.id, error);
                        if should_close {
                            reply.action = ConnectionAction::Close;
                        }
                        reply
                    }
                }
            }
            request_typed => {
                let Some(client_id) = self.client_id else {
                    return error_reply(
                        request.id,
                        AppErrorView::new(
                            ErrorCode::NotInitialized,
                            "initialize must be the first request on a connection",
                        ),
                    );
                };
                let subscribe = match &request_typed {
                    AppRequest::EventSubscribe(request) => Some(request.clone()),
                    _ => None,
                };
                let unsubscribe = match &request_typed {
                    AppRequest::EventUnsubscribe(request) => Some(request.subscription_id),
                    _ => None,
                };
                match self
                    .application
                    .execute_protocol(client_id, request.operation_id, request_typed)
                    .await
                {
                    Ok(response) => {
                        let action = if let (
                            Some(subscribe),
                            AppResponsePayload::Subscription(subscription),
                        ) = (&subscribe, &response)
                        {
                            ConnectionAction::Subscribe {
                                subscription_id: subscription.subscription_id,
                                request: subscribe.clone(),
                            }
                        } else if let Some(subscription_id) = unsubscribe {
                            ConnectionAction::Unsubscribe { subscription_id }
                        } else {
                            ConnectionAction::None
                        };
                        let mut reply = success_reply(request.id, &response);
                        reply.action = action;
                        reply
                    }
                    Err(error) => error_reply(request.id, error),
                }
            }
        }
    }

    pub fn apply_action(
        &mut self,
        action: ConnectionAction,
    ) -> Result<Vec<ServerFrame>, AppErrorView> {
        match action {
            ConnectionAction::None | ConnectionAction::Close => Ok(Vec::new()),
            ConnectionAction::Unsubscribe { subscription_id } => {
                self.subscriptions.remove(&subscription_id);
                Ok(Vec::new())
            }
            ConnectionAction::Subscribe {
                subscription_id,
                request,
            } => {
                self.subscriptions
                    .retain(|_, subscription| subscription.session_id != request.session_id);
                let after = request.after_sequence.unwrap_or(0);
                let (frames, last_sequence, gapped) = self.catch_up(request.session_id, after)?;
                self.subscriptions.insert(
                    subscription_id,
                    SubscriptionState {
                        session_id: request.session_id,
                        last_sequence,
                        gapped,
                    },
                );
                Ok(frames)
            }
        }
    }

    pub fn on_live_event(
        &mut self,
        event: EventEnvelope,
    ) -> Result<Vec<ServerFrame>, AppErrorView> {
        let Some(session_id) = event.session_id else {
            return Ok(Vec::new());
        };
        let Some((subscription_id, last_sequence, gapped)) = self
            .subscriptions
            .iter()
            .find(|(_, subscription)| subscription.session_id == session_id)
            .map(|(id, subscription)| (*id, subscription.last_sequence, subscription.gapped))
        else {
            return Ok(Vec::new());
        };
        if gapped || event.sequence <= last_sequence {
            return Ok(Vec::new());
        }
        if event.sequence == last_sequence.saturating_add(1) {
            if let Some(subscription) = self.subscriptions.get_mut(&subscription_id) {
                subscription.last_sequence = event.sequence;
            }
            return Ok(vec![ServerFrame::Event(Box::new(event))]);
        }
        let (frames, last_sequence, gapped) = self.catch_up(session_id, last_sequence)?;
        if let Some(subscription) = self.subscriptions.get_mut(&subscription_id) {
            subscription.last_sequence = last_sequence;
            subscription.gapped = gapped;
        }
        Ok(frames)
    }

    pub fn recover_lagged(&mut self) -> Result<Vec<ServerFrame>, AppErrorView> {
        let subscriptions: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, subscription)| !subscription.gapped)
            .map(|(id, subscription)| (*id, subscription.session_id, subscription.last_sequence))
            .collect();
        let mut frames = Vec::new();
        for (subscription_id, session_id, after) in subscriptions {
            let (mut recovered, last_sequence, gapped) = self.catch_up(session_id, after)?;
            frames.append(&mut recovered);
            if let Some(subscription) = self.subscriptions.get_mut(&subscription_id) {
                subscription.last_sequence = last_sequence;
                subscription.gapped = gapped;
            }
        }
        Ok(frames)
    }

    fn catch_up(
        &self,
        session_id: Uuid,
        after_sequence: u64,
    ) -> Result<(Vec<ServerFrame>, u64, bool), AppErrorView> {
        let first =
            self.application
                .catch_up_protocol_events(Some(session_id), after_sequence, 10_000)?;
        let target = first.high_watermark;
        let mut batch = first;
        let mut frames = Vec::new();
        let mut last = after_sequence;
        loop {
            if let Some(gap) = batch.gap {
                frames.push(gap_frame(session_id, gap.clone()));
                return Ok((frames, gap.high_watermark, true));
            }
            if batch.events.is_empty() {
                break;
            }
            for event in batch.events {
                last = event.sequence;
                frames.push(ServerFrame::Event(Box::new(event)));
            }
            if last >= target {
                break;
            }
            batch = self
                .application
                .catch_up_protocol_events(Some(session_id), last, 10_000)?;
        }
        Ok((frames, last, false))
    }
}

fn success_reply<T: serde::Serialize>(id: Uuid, value: &T) -> ConnectionReply {
    let response = ResponseEnvelope::success(id, value).unwrap_or_else(|_| {
        ResponseEnvelope::error(
            id,
            AppErrorView::new(ErrorCode::Internal, "failed to serialize response"),
        )
    });
    ConnectionReply {
        frame: ServerFrame::Response(response),
        action: ConnectionAction::None,
    }
}

fn error_reply(id: Uuid, error: AppErrorView) -> ConnectionReply {
    ConnectionReply {
        frame: ServerFrame::Response(ResponseEnvelope::error(id, error)),
        action: ConnectionAction::None,
    }
}

fn gap_frame(session_id: Uuid, gap: EventGap) -> ServerFrame {
    ServerFrame::Event(Box::new(EventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        stream_id: gap.stream_id,
        sequence: gap.expected_sequence,
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        session_id: Some(session_id),
        task_id: None,
        payload: AppEvent::EventGap(gap),
    }))
}
