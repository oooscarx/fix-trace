use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fixtrace_protocol::{ApprovalChoice, ConfigValue, TaskInput};

use crate::{Effect, EffectResult, InspectorTab, Modal, Model, Theme, TuiEvent, commands};

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
                && let Some(session) = model
                    .sessions
                    .iter()
                    .find(|session| !session.archived)
                    .or_else(|| model.sessions.first())
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
        EffectResult::Session(summary) => {
            let session_id = summary.id;
            let archived = summary.archived;
            if let Some(existing) = model
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
            {
                *existing = summary;
            } else {
                model.sessions.push(summary);
            }
            model.modal = None;
            if archived && model.selected_session_id == Some(session_id) {
                model.selected_session_id = None;
                model.session = None;
                model.last_sequence = 0;
                model.status = "Session archived".to_owned();
                vec![Effect::LoadSessions]
            } else {
                model.selected_session_id = Some(session_id);
                model.status = "Session ready".to_owned();
                vec![Effect::LoadSessions, Effect::OpenSession(session_id)]
            }
        }
        EffectResult::Imported(session_id) => {
            model.modal = None;
            model.selected_session_id = Some(session_id);
            model.status = "Session imported".to_owned();
            vec![Effect::LoadSessions, Effect::OpenSession(session_id)]
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
        EffectResult::Config(config) => {
            model.initialized.config_summary = config;
            model.modal = None;
            model.inspector_tab = InspectorTab::Settings;
            model.show_inspector = true;
            model.status = "Configuration saved".to_owned();
            Vec::new()
        }
        EffectResult::Edited(text) => {
            model.replace_composer(&text);
            model.status = "Prompt returned from editor".to_owned();
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
            KeyCode::Char('p') => {
                if model.composer_text().is_empty() {
                    model.replace_composer("/");
                }
                model.modal = Some(Modal::CommandPalette);
            }
            KeyCode::Char('n') => {
                model.replace_composer("/new ");
                model.modal = Some(Modal::CommandPalette);
            }
            KeyCode::Char('b') => model.show_sidebar = !model.show_sidebar,
            KeyCode::Char('i') => model.show_inspector = !model.show_inspector,
            KeyCode::Char('o') => model.modal = Some(Modal::SessionPicker),
            KeyCode::Char('r') => model.inspector_tab = InspectorTab::Overview,
            KeyCode::Char('e') => return export_current(model),
            KeyCode::Char('j') => model.composer.insert_newline(),
            KeyCode::Char('x') => {
                return vec![Effect::EditPrompt(model.composer_text())];
            }
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
        KeyCode::Tab if model.reference_query().is_some() => {
            complete_reference(model);
            Vec::new()
        }
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
            if model.follow_tail {
                let total = model
                    .session
                    .as_ref()
                    .map_or(0, |session| session.timeline.len());
                let window = usize::from(model.viewport.1).saturating_mul(3).max(64);
                model.scroll = u16::try_from(total.saturating_sub(window)).unwrap_or(u16::MAX);
            }
            model.follow_tail = false;
            model.scroll = model.scroll.saturating_sub(10);
            Vec::new()
        }
        KeyCode::PageDown => {
            model.scroll = model.scroll.saturating_add(10);
            let total = model
                .session
                .as_ref()
                .map_or(0, |session| session.timeline.len());
            let window = usize::from(model.viewport.1).saturating_mul(3).max(64);
            if usize::from(model.scroll) >= total.saturating_sub(window) {
                model.follow_tail = true;
            }
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
        KeyCode::Char('x') if model.composer_text().is_empty() => {
            toggle_latest_expandable(model);
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
                .filter(|choice| {
                    model.session.as_ref().is_some_and(|session| {
                        session.approvals.iter().any(|approval| {
                            approval.request.id == approval_id
                                && approval.request.choices.contains(choice)
                        })
                    })
                })
                .map(|choice| Effect::RespondApproval {
                    approval_id,
                    choice,
                })
                .into_iter()
                .collect()
        }
        Modal::CommandPalette => match key.code {
            KeyCode::Enter => {
                complete_palette(model);
                model.modal = None;
                submit(model)
            }
            KeyCode::Tab => {
                complete_palette(model);
                Vec::new()
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
    } else if let Some(task) = model
        .active_task()
        .filter(|task| !task.status.is_terminal())
    {
        if task.supports_steer {
            vec![Effect::SteerTask {
                task_id: task.id,
                text,
            }]
        } else {
            command_error(
                model,
                "The active task does not accept steering; wait for it or use /cancel",
            )
        }
    } else if let Some(session_id) = model.selected_session_id {
        vec![Effect::SendMessage { session_id, text }]
    } else {
        model.status = "Open a session before sending a message".to_owned();
        model.modal = Some(Modal::SessionPicker);
        Vec::new()
    }
}

fn slash_command(model: &mut Model, input: &str) -> Vec<Effect> {
    let parsed = match shell_words::split(input) {
        Ok(parsed) => parsed,
        Err(error) => return command_error(model, format!("Invalid quoting: {error}")),
    };
    let command = parsed.first().map(String::as_str).unwrap_or_default();
    let arguments = &parsed[1..];
    match command {
        "/new" => create_session(model, arguments),
        "/open" | "/resume" => {
            model.modal = Some(Modal::SessionPicker);
            Vec::new()
        }
        "/fork" => current_session(model)
            .map(|session_id| Effect::ForkSession {
                session_id,
                title: non_empty(arguments.join(" ")),
            })
            .into_iter()
            .collect(),
        "/archive" => current_session(model)
            .map(Effect::ArchiveSession)
            .into_iter()
            .collect(),
        "/import" => arguments.first().map_or_else(
            || command_error(model, "Usage: /import <export.json>"),
            |path| vec![Effect::ImportSession(PathBuf::from(path))],
        ),
        "/cancel" => model
            .active_task()
            .map(|task| Effect::CancelTask(task.id))
            .into_iter()
            .collect(),
        "/analyze" => start_current(model, TaskInput::AnalyzeMinimalTrace { no_llm: false }),
        "/diagnose" => start_current(
            model,
            TaskInput::GenerateDiagnosis {
                prompt: non_empty(arguments.join(" ")),
            },
        ),
        "/record" => {
            if arguments.is_empty() {
                command_error(model, "Usage: /record <command> | /record :done")
            } else {
                start_current(
                    model,
                    TaskInput::RecordTrace {
                        line: arguments.join(" "),
                    },
                )
            }
        }
        "/verify" => start_current(model, TaskInput::VerifyBaseline),
        "/replay" => start_current(model, TaskInput::ReplayFullTrace),
        "/repeat" => repeat_trial(model, arguments),
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
        "/model" => update_setting_or_open(model, "model.model", arguments),
        "/effort" => update_setting_or_open(model, "model.reasoning_mode", arguments),
        "/permissions" => update_setting_or_open(model, "approval.policy", arguments),
        "/config" => update_config(model, arguments),
        "/export" => export_to(model, arguments),
        "/help" => {
            model.modal = Some(Modal::Help);
            Vec::new()
        }
        "/quit" => {
            model.should_quit = true;
            Vec::new()
        }
        "/theme" => set_theme(model, arguments),
        _ => {
            model.status = format!("Unknown command: {command}");
            model.modal = Some(Modal::CommandPalette);
            model.replace_composer(input);
            Vec::new()
        }
    }
}

fn create_session(model: &mut Model, arguments: &[String]) -> Vec<Effect> {
    let Some(project) = arguments.first().filter(|value| !value.starts_with("--")) else {
        return command_error(
            model,
            "Usage: /new <project> --oracle <command> [--title <title>]",
        );
    };
    let mut oracle = None;
    let mut title = None;
    let mut index = 1;
    while index < arguments.len() {
        let flag = &arguments[index];
        if flag != "--oracle" && flag != "--title" {
            return command_error(model, format!("Unknown /new option: {flag}"));
        }
        index += 1;
        let start = index;
        while index < arguments.len() && !arguments[index].starts_with("--") {
            index += 1;
        }
        let value = arguments[start..index].join(" ");
        if value.is_empty() {
            return command_error(model, format!("{flag} requires a value"));
        }
        if flag == "--oracle" {
            oracle = Some(value);
        } else {
            title = Some(value);
        }
    }
    let Some(oracle) = oracle else {
        return command_error(model, "/new requires --oracle <command>");
    };
    vec![Effect::CreateSession {
        project: PathBuf::from(project),
        oracle,
        title,
    }]
}

fn repeat_trial(model: &mut Model, arguments: &[String]) -> Vec<Effect> {
    let Some(raw_id) = arguments.first() else {
        return command_error(model, "Usage: /repeat <trial-id>");
    };
    let trial_id = match raw_id.parse() {
        Ok(trial_id) => trial_id,
        Err(error) => return command_error(model, format!("Invalid trial id: {error}")),
    };
    start_current(model, TaskInput::RepeatTrial { trial_id })
}

fn update_setting_or_open(model: &mut Model, key: &str, arguments: &[String]) -> Vec<Effect> {
    if arguments.is_empty() {
        model.inspector_tab = InspectorTab::Settings;
        model.modal = Some(Modal::Settings);
        return Vec::new();
    }
    vec![Effect::UpdateConfig {
        key: key.to_owned(),
        value: ConfigValue::String(arguments.join(" ")),
    }]
}

fn update_config(model: &mut Model, arguments: &[String]) -> Vec<Effect> {
    if arguments.is_empty() {
        model.inspector_tab = InspectorTab::Settings;
        model.modal = Some(Modal::Settings);
        return Vec::new();
    }
    if arguments.len() < 2 {
        return command_error(model, "Usage: /config <key> <value>");
    }
    let value = arguments[1..].join(" ");
    vec![Effect::UpdateConfig {
        key: arguments[0].clone(),
        value: config_value(&value),
    }]
}

fn config_value(value: &str) -> ConfigValue {
    match value {
        "true" => ConfigValue::Boolean(true),
        "false" => ConfigValue::Boolean(false),
        _ if value.parse::<i64>().is_ok() => {
            ConfigValue::Integer(value.parse().expect("integer was validated"))
        }
        _ if value.parse::<f64>().is_ok() => {
            ConfigValue::Float(value.parse().expect("float was validated"))
        }
        _ => ConfigValue::String(value.to_owned()),
    }
}

fn export_to(model: &mut Model, arguments: &[String]) -> Vec<Effect> {
    let Some(session_id) = current_session(model) else {
        return Vec::new();
    };
    let output = arguments.first().map_or_else(
        || PathBuf::from(format!("fixtrace-{session_id}.json")),
        PathBuf::from,
    );
    vec![Effect::ExportSession { session_id, output }]
}

fn current_session(model: &mut Model) -> Option<uuid::Uuid> {
    if model.selected_session_id.is_none() {
        model.status = "Open a session first".to_owned();
        model.modal = Some(Modal::SessionPicker);
    }
    model.selected_session_id
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn command_error(model: &mut Model, message: impl Into<String>) -> Vec<Effect> {
    let message = message.into();
    model.status = message.clone();
    model.modal = Some(Modal::Error(message));
    Vec::new()
}

fn set_theme(model: &mut Model, arguments: &[String]) -> Vec<Effect> {
    let theme = match arguments.first().map(String::as_str) {
        None => match model.theme {
            Theme::Color => Theme::HighContrast,
            Theme::HighContrast => Theme::Monochrome,
            Theme::Monochrome => Theme::Color,
        },
        Some("color" | "dark") => Theme::Color,
        Some("high-contrast" | "contrast") => Theme::HighContrast,
        Some("mono" | "monochrome") => Theme::Monochrome,
        Some(value) => {
            return command_error(
                model,
                format!("Unknown theme {value}; use color, high-contrast, or mono"),
            );
        }
    };
    model.set_theme(theme);
    model.status = format!("Theme: {}", theme.label());
    Vec::new()
}

fn complete_reference(model: &mut Model) {
    let Some(query) = model.reference_query().map(str::to_owned) else {
        return;
    };
    let Some((token, _)) = model.reference_suggestions(&query).into_iter().next() else {
        return;
    };
    let text = model.composer_text();
    let prefix_len = text.len().saturating_sub(query.len());
    model.replace_composer(&format!("{}{token} ", &text[..prefix_len]));
}

fn complete_palette(model: &mut Model) {
    let text = model.composer_text();
    let trimmed = text.trim();
    if trimmed.split_whitespace().count() > 1
        || commands::COMMANDS
            .iter()
            .any(|command| command.name == trimmed)
    {
        return;
    }
    let query = trimmed.trim_start_matches('/');
    if let Some(command) = commands::matching(query).first() {
        model.replace_composer(command.name);
    }
}

fn toggle_latest_expandable(model: &mut Model) {
    let Some(id) = model.session.as_ref().and_then(|session| {
        session.timeline.iter().rev().find_map(|item| match item {
            fixtrace_protocol::TimelineItem::ToolCall(item) => Some(item.header.id),
            _ => None,
        })
    }) else {
        return;
    };
    if !model.expanded_items.remove(&id) {
        model.expanded_items.insert(id);
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
