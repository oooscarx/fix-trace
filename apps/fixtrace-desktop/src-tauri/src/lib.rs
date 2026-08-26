use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use fixtrace::{
    application::{AppServiceOptions, FixTraceAppService},
    history::paths::StatePaths,
};
use fixtrace_client::{AppClient, InProcessClient};
use fixtrace_protocol::{
    AppRequest, AppResponsePayload, ClientCapabilities, EventEnvelope, InitializeRequest,
    InitializeResponse, PROTOCOL_VERSION, SubscribeRequest,
};
use fixtrace_server::WriterLock;
use serde::Serialize;
use tauri::{Manager, State, ipc::Channel};
use tokio_util::sync::CancellationToken;

struct DesktopState {
    client: Arc<InProcessClient>,
    initialized: Mutex<Option<InitializeResponse>>,
    cancellation: CancellationToken,
    subscriptions: Mutex<HashMap<u64, CancellationToken>>,
    next_subscription: AtomicU64,
    _writer_lock: WriterLock,
}

impl Drop for DesktopState {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Debug, Serialize)]
struct CommandError {
    message: String,
}

impl From<fixtrace_client::ClientError> for CommandError {
    fn from(error: fixtrace_client::ClientError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

#[tauri::command]
async fn initialize_client(
    state: State<'_, DesktopState>,
) -> Result<InitializeResponse, CommandError> {
    if let Some(initialized) = state
        .initialized
        .lock()
        .map_err(|_| CommandError {
            message: "desktop initialization lock is poisoned".to_owned(),
        })?
        .clone()
    {
        return Ok(initialized);
    }
    let initialized = state
        .client
        .initialize(InitializeRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            client: InProcessClient::client_info(),
            capabilities: ClientCapabilities {
                supports_streaming: true,
                supports_approvals: true,
                supports_diff: true,
                supports_graph: true,
                supports_artifacts: true,
            },
        })
        .await?;
    *state.initialized.lock().map_err(|_| CommandError {
        message: "desktop initialization lock is poisoned".to_owned(),
    })? = Some(initialized.clone());
    Ok(initialized)
}

#[tauri::command]
async fn execute_request(
    state: State<'_, DesktopState>,
    request: AppRequest,
) -> Result<AppResponsePayload, CommandError> {
    Ok(state.client.request(request).await?)
}

#[tauri::command]
async fn subscribe_events(
    state: State<'_, DesktopState>,
    request: SubscribeRequest,
    channel: Channel<EventEnvelope>,
) -> Result<u64, CommandError> {
    let mut subscription = state.client.subscribe(request).await?;
    let subscription_id = state.next_subscription.fetch_add(1, Ordering::Relaxed);
    let cancellation = state.cancellation.child_token();
    state
        .subscriptions
        .lock()
        .map_err(|_| CommandError {
            message: "desktop subscription lock is poisoned".to_owned(),
        })?
        .insert(subscription_id, cancellation.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                () = cancellation.cancelled() => break,
                event = subscription.recv() => match event {
                    Ok(event) => {
                        if channel.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });
    Ok(subscription_id)
}

#[tauri::command]
fn unsubscribe_events(
    state: State<'_, DesktopState>,
    subscription_id: u64,
) -> Result<(), CommandError> {
    if let Some(cancellation) = state
        .subscriptions
        .lock()
        .map_err(|_| CommandError {
            message: "desktop subscription lock is poisoned".to_owned(),
        })?
        .remove(&subscription_id)
    {
        cancellation.cancel();
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let state = tauri::async_runtime::block_on(async {
                let paths = StatePaths::discover(None)?;
                let writer_lock =
                    WriterLock::acquire(paths.database.with_file_name("app-server.writer.lock"))?;
                let cancellation = CancellationToken::new();
                let service = Arc::new(FixTraceAppService::start(
                    AppServiceOptions::default(),
                    cancellation.clone(),
                )?);
                Ok::<_, Box<dyn std::error::Error>>(DesktopState {
                    client: Arc::new(InProcessClient::new(service)),
                    initialized: Mutex::new(None),
                    cancellation,
                    subscriptions: Mutex::new(HashMap::new()),
                    next_subscription: AtomicU64::new(1),
                    _writer_lock: writer_lock,
                })
            })?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            initialize_client,
            execute_request,
            subscribe_events,
            unsubscribe_events
        ])
        .run(tauri::generate_context!())
        .expect("failed to run FixTrace desktop");
}
