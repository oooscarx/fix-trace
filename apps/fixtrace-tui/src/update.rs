use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fixtrace_protocol::{ApprovalChoice, TaskInput};

use crate::{Effect, EffectResult, InspectorTab, Modal, Model, TuiEvent};

pub fn update(model: &mut Model, event: TuiEvent) -> Vec<Effect> {
    model.dirty = true;
    match event {
        TuiEvent::Terminal(event) => terminal(model, event),
        TuiEvent::Server(event) => {
            if event.session_id != model.selected_session_id
                || event.sequence <= model.last_sequence
            {
                Vec::new()
            } else {
                model.offline = false;
                model.apply_event(&event)
            }
        }
        TuiEvent::Tick | TuiEvent::Render => Vec::new(),
        TuiEvent::Resize(width, height) => {
            model.viewport = (width, height);
            Vec::new()
        }
        TuiEvent::EffectCompleted(result) => effect_completed(model, result),
        TuiEvent::FatalError(error) => {
            model.modal = Some(Modal::Error(error.clone()));
            model.status = error;
            Vec::new()
        }
    }
}

fn effect_completed(model: &mut Model, result: EffectResult) -> Vec<Effect> {
    match result {
        EffectResult::Sessions(mut sessions) => {
            sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
            model.sessions = sessions;
            if model.selected_session_id.is_none()
                && let Some(session) = model.sessions.first()
            {
                model.selected_session_id = Some(session.id);
            }
            model.status = format!("{} sessions", model.sessions.len());
            if model.session.is_none() {
                model
                    .selected_session_id
                    .map(Effect::OpenSession)
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        }
        EffectResult::Snapshot(snapshot) => {
            let snapshot = *snapshot;
            let session_id = snapshot.session.summary.id;
            let after_sequence = snapshot.through_sequence;
            model.selected_session_id = Some(session_id);
            model.last_sequence = after_sequence;
            model.session = Some(snapshot.session);
            model.modal = None;
            model.offline = false;
            model.follow_tail = true;
            model.status = "Session opened".to_owned();
            vec![Effect::Subscribe {
                session_id,
                after_sequence,
            }]
        }
        EffectResult::Task(task) => {
            if let Some(session) = &mut model.session {
                session.task = Some(task.clone());
            }
            model.status = format!("Task queued: {}", task.title);
            Vec::new()
        }
        EffectResult::Accepted(message) => {
            model.modal = None;
            model.status = message;
            Vec::new()
        }
        EffectResult::Exported(path) => {
            model.modal = None;
            model.status = format!("Exported to {}", path.display());
            Vec::new()
        }
        EffectResult::Error(error) => {
            model.status = error.message.clone();
            model.modal = Some(Modal::Error(error.message));
            Vec::new()
        }
        EffectResult::TransportError(error) => {
            model.offline = true;
            model.status = error.clone();
            model.modal = Some(Modal::Error(error));
            Vec::new()
        }
    }
}

fn terminal(model: &mut Model, event: Event) -> Vec<Effect> {
    match event {
        Event::Resize(width, height) => update(model, TuiEvent::Resize(width, height)),
        Event::Paste(text) => {
            model.composer.insert_str(text);
            Vec::new()
        }
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            key_event(model, key)
        }
        _ => Vec::new(),
    }
}

fn key_event(model: &mut Model, key: KeyEvent) -> Vec<Effect> {
    if let Some(modal) = model.modal.clone() {
        return modal_key(model, modal, key);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return ctrl_c(model);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('p') => model.modal = Some(Modal::CommandPalette),
            KeyCode::Char('b') => model.show_sidebar = !model.show_sidebar,
            KeyCode::Char('i') => model.show_inspector = !model.show_inspector,
            KeyCode::Char('o') => model.modal = Some(Modal::SessionPicker),
            KeyCode::Char('r') => model.inspector_tab = InspectorTab::Overview,
            KeyCode::Char('e') => return export_current(model),
            KeyCode::Char('j') => model.composer.insert_newline(),
            KeyCode::Up => history(model, true),
            KeyCode::Down => history(model, false),
            _ => {
                model.composer.input(key);
            }
        }
        return Vec::new();
    }
    match key.code {
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            model.composer.insert_newline();
            Vec::new()
        }
        KeyCode::Enter => submit(model),
        KeyCode::Tab => {
            model.inspector_tab = if key.modifiers.contains(KeyModifiers::SHIFT) {
                model.inspector_tab.previous()
            } else {
                model.inspector_tab.next()
            };
            Vec::new()
        }
        KeyCode::BackTab => {
            model.inspector_tab = model.inspector_tab.previous();
            Vec::new()
        }
        KeyCode::PageUp => {
            model.follow_tail = false;
            model.scroll = model.scroll.saturating_sub(10);
            Vec::new()
        }
        KeyCode::PageDown => {
            model.scroll = model.scroll.saturating_add(10);
            Vec::new()
        }
        KeyCode::Char('?') if model.composer_text().is_empty() => {
            model.modal = Some(Modal::Help);
            Vec::new()
        }
        KeyCode::Char('g') if model.composer_text().is_empty() => {
            model.follow_tail = false;
            model.scroll = 0;
            Vec::new()
        }
        KeyCode::Char('G') if model.composer_text().is_empty() => {
            model.follow_tail = true;
            Vec::new()
        }
        _ => {
            model.composer.input(key);
            Vec::new()
        }
    }
}

fn modal_key(model: &mut Model, modal: Modal, key: KeyEvent) -> Vec<Effect> {
    if key.code == KeyCode::Esc {
        model.modal = None;
        return Vec::new();
    }
    match modal {
        Modal::SessionPicker => match key.code {
            KeyCode::Up => select_session(model, true),
            KeyCode::Down => select_session(model, false),
            KeyCode::Enter => {
                model.modal = None;
                model
                    .selected_session_id
                    .map(Effect::OpenSession)
                    .into_iter()
                    .collect()
            }
            _ => Vec::new(),
        },
        Modal::Approval(approval_id) => {
            let choice = match key.code {
                KeyCode::Char('y') => Some(ApprovalChoice::ApproveOnce),
                KeyCode::Char('t') => Some(ApprovalChoice::ApproveForTask),
                KeyCode::Char('s') => Some(ApprovalChoice::ApproveEquivalentForSession),
                KeyCode::Char('n') => Some(ApprovalChoice::Deny),
                KeyCode::Char('c') => Some(ApprovalChoice::CancelTask),
                _ => None,
            };
            choice
                .map(|choice| Effect::RespondApproval {
                    approval_id,
                    choice,
                })
                .into_iter()
                .collect()
        }
        Modal::CommandPalette => match key.code {
            KeyCode::Enter => {
                model.modal = None;
                submit(model)
            }
            _ => {
                model.composer.input(key);
                Vec::new()
            }
        },
        Modal::Help | Modal::Settings | Modal::Error(_) | Modal::Export => {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char('q')) {
                model.modal = None;
            }
            Vec::new()
        }
    }
}

fn ctrl_c(model: &mut Model) -> Vec<Effect> {
    let now = Instant::now();
    if model
        .last_ctrl_c
        .is_some_and(|previous| now.duration_since(previous) <= Duration::from_millis(1500))
    {
        model.should_quit = true;
        return Vec::new();
    }
    model.last_ctrl_c = Some(now);
    model.status = "Press Ctrl+C again within 1.5s to exit".to_owned();
    model
        .active_task()
        .filter(|task| !task.status.is_terminal())
        .map(|task| Effect::CancelTask(task.id))
        .into_iter()
        .collect()
}

fn submit(model: &mut Model) -> Vec<Effect> {
    let text = model.take_composer();
    if text.is_empty() {
        return Vec::new();
    }
    model.input_history.push(text.clone());
    if text.starts_with('/') {
        slash_command(model, &text)
    } else if let Some(session_id) = model.selected_session_id {
        vec![Effect::SendMessage { session_id, text }]
    } else {
        model.status = "Open a session before sending a message".to_owned();
        model.modal = Some(Modal::SessionPicker);
        Vec::new()
    }
}

fn slash_command(model: &mut Model, input: &str) -> Vec<Effect> {
    let mut words = input.split_whitespace();
    let command = words.next().unwrap_or_default();
    match command {
        "/open" | "/resume" => {
            model.modal = Some(Modal::SessionPicker);
            Vec::new()
        }
        "/cancel" => model
            .active_task()
            .map(|task| Effect::CancelTask(task.id))
            .into_iter()
            .collect(),
        "/analyze" => start_current(model, TaskInput::AnalyzeMinimalTrace { no_llm: false }),
        "/diagnose" => model
            .selected_session_id
            .map(|session_id| Effect::SendMessage {
                session_id,
                text: words.collect::<Vec<_>>().join(" ").trim().to_owned(),
            })
            .into_iter()
            .collect(),
        "/record" => start_current(model, TaskInput::RecordTrace),
        "/verify" => start_current(model, TaskInput::VerifyBaseline),
        "/replay" => start_current(model, TaskInput::ReplayFullTrace),
        "/demo" => vec![Effect::StartTask {
            session_id: model.selected_session_id,
            input: TaskInput::Demo { no_llm: true },
        }],
        "/actions" => select_tab(model, InspectorTab::Actions),
        "/trials" => select_tab(model, InspectorTab::Trials),
        "/graph" => select_tab(model, InspectorTab::Graph),
        "/diff" => select_tab(model, InspectorTab::Diff),
        "/artifacts" => select_tab(model, InspectorTab::Artifacts),
        "/usage" | "/budget" => select_tab(model, InspectorTab::Usage),
        "/status" | "/report" => select_tab(model, InspectorTab::Overview),
        "/model" | "/effort" | "/permissions" | "/config" => {
            model.inspector_tab = InspectorTab::Settings;
            model.modal = Some(Modal::Settings);
            Vec::new()
        }
        "/export" => export_current(model),
        "/help" => {
            model.modal = Some(Modal::Help);
            Vec::new()
        }
        "/quit" => {
            model.should_quit = true;
            Vec::new()
        }
        "/new" | "/fork" | "/archive" | "/import" | "/theme" => {
            model.status = format!("{command} is available from the full command dialog");
            Vec::new()
        }
        _ => {
            model.status = format!("Unknown command: {command}");
            model.modal = Some(Modal::CommandPalette);
            model.replace_composer(input);
            Vec::new()
        }
    }
}

fn start_current(model: &mut Model, input: TaskInput) -> Vec<Effect> {
    model
        .selected_session_id
        .map(|session_id| Effect::StartTask {
            session_id: Some(session_id),
            input,
        })
        .into_iter()
        .collect()
}

fn select_tab(model: &mut Model, tab: InspectorTab) -> Vec<Effect> {
    model.inspector_tab = tab;
    model.show_inspector = true;
    Vec::new()
}

fn export_current(model: &mut Model) -> Vec<Effect> {
    model
        .selected_session_id
        .map(|session_id| Effect::ExportSession {
            session_id,
            output: PathBuf::from(format!("fixtrace-{session_id}.json")),
        })
        .into_iter()
        .collect()
}

fn history(model: &mut Model, older: bool) {
    if model.input_history.is_empty() {
        return;
    }
    let next = match (model.history_index, older) {
        (None, true) => model.input_history.len() - 1,
        (Some(index), true) => index.saturating_sub(1),
        (Some(index), false) => (index + 1).min(model.input_history.len() - 1),
        (None, false) => return,
    };
    model.history_index = Some(next);
    let text = model.input_history[next].clone();
    model.replace_composer(&text);
}

fn select_session(model: &mut Model, previous: bool) -> Vec<Effect> {
    if model.sessions.is_empty() {
        return Vec::new();
    }
    let current = model
        .selected_session_id
        .and_then(|id| model.sessions.iter().position(|session| session.id == id))
        .unwrap_or(0);
    let next = if previous {
        (current + model.sessions.len() - 1) % model.sessions.len()
    } else {
        (current + 1) % model.sessions.len()
    };
    model.selected_session_id = Some(model.sessions[next].id);
    Vec::new()
}
