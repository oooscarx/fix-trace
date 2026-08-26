use std::{
    io::{Read, Write},
    process::Command,
    sync::Arc,
};

use crossterm::event::EventStream;
use fixtrace_client::{AppClient, ClientError};
use fixtrace_protocol::{
    AppRequest, AppResponsePayload, ApprovalRespondRequest, ConfigEntryUpdate, ConfigUpdateRequest,
    MessageSendRequest, PageRequest, SessionCreateRequest, SessionExportRequest,
    SessionForkRequest, SessionIdRequest, SessionImportRequest, SessionListRequest,
    SessionSnapshotRequest, SubscribeRequest, TaskIdRequest, TaskStartRequest, TaskSteerRequest,
};
use futures_util::StreamExt;
use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, interval},
};
use uuid::Uuid;

use crate::{Effect, EffectResult, Model, TuiEvent, render, terminal::TerminalGuard, update};

pub async fn run(
    client: Arc<dyn AppClient>,
    mut model: Model,
    initial_session: Option<Uuid>,
) -> Result<(), std::io::Error> {
    let mut terminal = TerminalGuard::enter()?;
    let size = terminal.terminal().size()?;
    model.viewport = (size.width, size.height);
    let (sender, mut receiver) = mpsc::channel::<TuiEvent>(2_048);
    let mut terminal_events = spawn_terminal_events(sender.clone());
    let mut subscription: Option<JoinHandle<()>> = None;
    let mut ticker = interval(Duration::from_millis(33));
    let mut effects = vec![Effect::LoadSessions];
    if let Some(session_id) = initial_session {
        effects.push(Effect::OpenSession(session_id));
    }
    schedule_effects(&client, &sender, &mut subscription, effects);

    while !model.should_quit {
        tokio::select! {
            _ = ticker.tick() => {
                if model.dirty {
                    terminal.terminal().draw(|frame| render(frame, &model))?;
                    model.dirty = false;
                }
            }
            event = receiver.recv() => {
                let Some(event) = event else { break };
                let effects = update(&mut model, event);
                let mut scheduled = Vec::new();
                for effect in effects {
                    if let Effect::EditPrompt(text) = effect {
                        terminal_events.abort();
                        let _ = (&mut terminal_events).await;
                        terminal.suspend()?;
                        let result = edit_prompt(text).await;
                        terminal.resume()?;
                        terminal_events = spawn_terminal_events(sender.clone());
                        scheduled.extend(update(&mut model, TuiEvent::EffectCompleted(result)));
                    } else {
                        scheduled.push(effect);
                    }
                }
                schedule_effects(&client, &sender, &mut subscription, scheduled);
            }
        }
    }
    if let Some(subscription) = subscription {
        subscription.abort();
    }
    Ok(())
}

fn spawn_terminal_events(sender: mpsc::Sender<TuiEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = EventStream::new();
        while let Some(event) = events.next().await {
            match event {
                Ok(event) => {
                    if sender.send(TuiEvent::Terminal(event)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(TuiEvent::FatalError(error.to_string())).await;
                    return;
                }
            }
        }
    })
}

fn schedule_effects(
    client: &Arc<dyn AppClient>,
    sender: &mpsc::Sender<TuiEvent>,
    subscription: &mut Option<JoinHandle<()>>,
    effects: Vec<Effect>,
) {
    for effect in effects {
        if let Effect::Subscribe {
            session_id,
            after_sequence,
        } = effect
        {
            if let Some(previous) = subscription.take() {
                previous.abort();
            }
            *subscription = Some(spawn_subscription(
                client.clone(),
                sender.clone(),
                session_id,
                after_sequence,
            ));
            continue;
        }
        let client = client.clone();
        let sender = sender.clone();
        tokio::spawn(async move {
            let result = execute_effect(client, effect).await;
            let _ = sender.send(TuiEvent::EffectCompleted(result)).await;
        });
    }
}

fn spawn_subscription(
    client: Arc<dyn AppClient>,
    sender: mpsc::Sender<TuiEvent>,
    session_id: Uuid,
    after_sequence: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut subscription = match client
            .subscribe(SubscribeRequest {
                session_id,
                after_sequence: Some(after_sequence),
            })
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                let _ = sender.send(TuiEvent::FatalError(error.to_string())).await;
                return;
            }
        };
        loop {
            match subscription.recv().await {
                Ok(event) => {
                    if sender
                        .send(TuiEvent::Server(Box::new(event)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(TuiEvent::FatalError(error.to_string())).await;
                    return;
                }
            }
        }
    })
}

async fn execute_effect(client: Arc<dyn AppClient>, effect: Effect) -> EffectResult {
    let response = match effect {
        Effect::EditPrompt(_) => unreachable!("editor effects are handled by the TUI runtime"),
        Effect::LoadSessions => {
            client
                .request(AppRequest::SessionList(SessionListRequest {
                    page: PageRequest {
                        cursor: None,
                        limit: Some(500),
                    },
                    include_archived: true,
                }))
                .await
        }
        Effect::CreateSession {
            project,
            oracle,
            title,
        } => {
            client
                .request(AppRequest::SessionCreate(SessionCreateRequest {
                    project,
                    oracle,
                    title,
                }))
                .await
        }
        Effect::ForkSession { session_id, title } => {
            client
                .request(AppRequest::SessionFork(SessionForkRequest {
                    session_id,
                    title,
                }))
                .await
        }
        Effect::ArchiveSession(session_id) => {
            client
                .request(AppRequest::SessionArchive(SessionIdRequest { session_id }))
                .await
        }
        Effect::ImportSession(input) => {
            client
                .request(AppRequest::SessionImport(SessionImportRequest { input }))
                .await
        }
        Effect::OpenSession(session_id) => {
            client
                .request(AppRequest::SessionGetSnapshot(SessionSnapshotRequest {
                    session_id,
                    timeline_page: PageRequest {
                        cursor: None,
                        limit: Some(500),
                    },
                }))
                .await
        }
        Effect::SendMessage { session_id, text } => {
            client
                .request(AppRequest::MessageSend(MessageSendRequest {
                    session_id,
                    text,
                }))
                .await
        }
        Effect::SteerTask { task_id, text } => {
            client
                .request(AppRequest::TaskSteer(TaskSteerRequest {
                    task_id,
                    message: text,
                }))
                .await
        }
        Effect::StartTask { session_id, input } => {
            client
                .request(AppRequest::TaskStart(TaskStartRequest {
                    session_id,
                    input,
                }))
                .await
        }
        Effect::CancelTask(task_id) => {
            client
                .request(AppRequest::TaskCancel(TaskIdRequest { task_id }))
                .await
        }
        Effect::RespondApproval {
            approval_id,
            choice,
        } => {
            client
                .request(AppRequest::ApprovalRespond(ApprovalRespondRequest {
                    approval_id,
                    choice,
                }))
                .await
        }
        Effect::ExportSession { session_id, output } => {
            client
                .request(AppRequest::SessionExport(SessionExportRequest {
                    session_id,
                    output,
                }))
                .await
        }
        Effect::UpdateConfig { key, value } => {
            client
                .request(AppRequest::ConfigUpdate(ConfigUpdateRequest {
                    updates: vec![ConfigEntryUpdate { key, value }],
                }))
                .await
        }
        Effect::Subscribe { .. } => unreachable!("subscription effects are scheduled separately"),
    };
    map_response(response)
}

async fn edit_prompt(text: String) -> EffectResult {
    match tokio::task::spawn_blocking(move || edit_prompt_blocking(&text)).await {
        Ok(Ok(text)) => EffectResult::Edited(text),
        Ok(Err(error)) => EffectResult::TransportError(error),
        Err(error) => EffectResult::TransportError(format!("editor task failed: {error}")),
    }
}

fn edit_prompt_blocking(text: &str) -> Result<String, String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_owned());
    let words = shell_words::split(&editor).map_err(|error| format!("invalid editor: {error}"))?;
    let (program, arguments) = words
        .split_first()
        .ok_or_else(|| "VISUAL/EDITOR is empty".to_owned())?;
    let mut file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    let status = Command::new(program)
        .args(arguments)
        .arg(file.path())
        .status()
        .map_err(|error| format!("failed to launch {program}: {error}"))?;
    if !status.success() {
        return Err(format!("editor exited with {status}"));
    }
    let mut edited = String::new();
    file.reopen()
        .map_err(|error| error.to_string())?
        .read_to_string(&mut edited)
        .map_err(|error| error.to_string())?;
    Ok(edited)
}

fn map_response(response: Result<AppResponsePayload, ClientError>) -> EffectResult {
    match response {
        Ok(AppResponsePayload::SessionList(response)) => EffectResult::Sessions(response.sessions),
        Ok(AppResponsePayload::Session(session)) => EffectResult::Session(session),
        Ok(AppResponsePayload::Imported { session_id }) => EffectResult::Imported(session_id),
        Ok(AppResponsePayload::SessionSnapshot(snapshot)) => EffectResult::Snapshot(snapshot),
        Ok(AppResponsePayload::Config(config)) => EffectResult::Config(config),
        Ok(AppResponsePayload::Task(task)) => EffectResult::Task(task),
        Ok(AppResponsePayload::Accepted { message }) => EffectResult::Accepted(message),
        Ok(AppResponsePayload::Exported { output, .. }) => EffectResult::Exported(output),
        Ok(other) => {
            EffectResult::TransportError(format!("unexpected App Server response: {other:?}"))
        }
        Err(ClientError::Protocol(error)) => EffectResult::Error(error),
        Err(error) => EffectResult::TransportError(error.to_string()),
    }
}
