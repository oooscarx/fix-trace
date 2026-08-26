use std::{collections::HashSet, path::PathBuf, time::Instant};

use crossterm::event::Event;
use fixtrace_protocol::{
    AppErrorView, AppEvent, ApprovalChoice, EventEnvelope, InitializeResponse, SessionSnapshot,
    SessionSummary, SessionView, TaskInput, TaskSummary, TimelineItem,
};
use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders},
};
use ratatui_textarea::TextArea;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionMode {
    InProcess,
    WebSocket(String),
}

impl ConnectionMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::InProcess => "in-process",
            Self::WebSocket(_) => "websocket",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectorTab {
    Overview,
    Actions,
    Trials,
    Graph,
    Diff,
    Artifacts,
    Usage,
    Settings,
}

impl InspectorTab {
    pub const ALL: [Self; 8] = [
        Self::Overview,
        Self::Actions,
        Self::Trials,
        Self::Graph,
        Self::Diff,
        Self::Artifacts,
        Self::Usage,
        Self::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Actions => "Actions",
            Self::Trials => "Trials",
            Self::Graph => "Graph",
            Self::Diff => "Diff",
            Self::Artifacts => "Artifacts",
            Self::Usage => "Usage",
            Self::Settings => "Settings",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Modal {
    CommandPalette,
    SessionPicker,
    Help,
    Approval(Uuid),
    Settings,
    Error(String),
    Export,
}

#[derive(Clone, Debug)]
pub enum Effect {
    LoadSessions,
    OpenSession(Uuid),
    Subscribe {
        session_id: Uuid,
        after_sequence: u64,
    },
    SendMessage {
        session_id: Uuid,
        text: String,
    },
    StartTask {
        session_id: Option<Uuid>,
        input: TaskInput,
    },
    CancelTask(Uuid),
    RespondApproval {
        approval_id: Uuid,
        choice: ApprovalChoice,
    },
    ExportSession {
        session_id: Uuid,
        output: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub enum EffectResult {
    Sessions(Vec<SessionSummary>),
    Snapshot(Box<SessionSnapshot>),
    Task(TaskSummary),
    Accepted(String),
    Exported(PathBuf),
    Error(AppErrorView),
    TransportError(String),
}

#[derive(Clone, Debug)]
pub enum TuiEvent {
    Terminal(Event),
    Server(Box<EventEnvelope>),
    Tick,
    Render,
    Resize(u16, u16),
    EffectCompleted(EffectResult),
    FatalError(String),
}

pub struct Model {
    pub initialized: InitializeResponse,
    pub connection: ConnectionMode,
    pub sessions: Vec<SessionSummary>,
    pub selected_session_id: Option<Uuid>,
    pub session: Option<SessionView>,
    pub last_sequence: u64,
    pub composer: TextArea<'static>,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub inspector_tab: InspectorTab,
    pub modal: Option<Modal>,
    pub show_sidebar: bool,
    pub show_inspector: bool,
    pub expanded_items: HashSet<Uuid>,
    pub status: String,
    pub offline: bool,
    pub dirty: bool,
    pub should_quit: bool,
    pub follow_tail: bool,
    pub scroll: u16,
    pub last_ctrl_c: Option<Instant>,
    pub viewport: (u16, u16),
}

impl Model {
    pub fn new(initialized: InitializeResponse, connection: ConnectionMode) -> Self {
        Self {
            initialized,
            connection,
            sessions: Vec::new(),
            selected_session_id: None,
            session: None,
            last_sequence: 0,
            composer: configured_composer(),
            input_history: Vec::new(),
            history_index: None,
            inspector_tab: InspectorTab::Overview,
            modal: None,
            show_sidebar: true,
            show_inspector: true,
            expanded_items: HashSet::new(),
            status: "Ready".to_owned(),
            offline: false,
            dirty: true,
            should_quit: false,
            follow_tail: true,
            scroll: 0,
            last_ctrl_c: None,
            viewport: (120, 36),
        }
    }

    pub fn active_task(&self) -> Option<&TaskSummary> {
        self.session
            .as_ref()
            .and_then(|session| session.task.as_ref())
    }

    pub fn composer_text(&self) -> String {
        self.composer.lines().join("\n")
    }

    pub fn take_composer(&mut self) -> String {
        let text = self.composer_text().trim().to_owned();
        self.composer = configured_composer();
        self.history_index = None;
        text
    }

    pub fn replace_composer(&mut self, text: &str) {
        self.composer = configured_composer();
        self.composer.insert_str(text);
    }

    pub fn upsert_timeline(&mut self, item: TimelineItem) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(id) = item.id() else {
            return;
        };
        if let Some(existing) = session
            .timeline
            .iter_mut()
            .find(|existing| existing.id() == Some(id))
        {
            *existing = item;
        } else {
            session.timeline.push(item);
        }
    }

    pub fn apply_event(&mut self, event: &EventEnvelope) -> Vec<Effect> {
        self.last_sequence = self.last_sequence.max(event.sequence);
        self.dirty = true;
        match &event.payload {
            AppEvent::SessionCreated(summary) | AppEvent::SessionUpdated(summary) => {
                if let Some(existing) = self.sessions.iter_mut().find(|item| item.id == summary.id)
                {
                    *existing = summary.clone();
                } else {
                    self.sessions.push(summary.clone());
                }
            }
            AppEvent::TaskStarted(task) | AppEvent::TaskCancelled(task) => {
                if let Some(session) = &mut self.session {
                    session.task = Some(task.clone());
                }
                self.status = format!("Task: {:?}", task.status);
            }
            AppEvent::TaskProgress(progress) => {
                if let Some(session) = &mut self.session {
                    session.task = Some(progress.task.clone());
                }
                self.status = progress.message.clone();
            }
            AppEvent::TaskCompleted(result) => {
                if let Some(session) = &mut self.session {
                    session.task = Some(result.task.clone());
                }
                self.status = "Task completed".to_owned();
                return self.refresh_effect();
            }
            AppEvent::TaskFailed(failure) => {
                if let Some(session) = &mut self.session {
                    session.task = Some(failure.task.clone());
                }
                self.status = format!("Task failed: {}", failure.error.message);
                self.modal = Some(Modal::Error(failure.error.message.clone()));
            }
            AppEvent::ItemStarted(item) | AppEvent::ItemCompleted(item) => {
                self.upsert_timeline(item.clone());
                self.follow_tail = true;
            }
            AppEvent::ItemDelta(fixtrace_protocol::ItemDelta::AgentMessage(delta)) => {
                if let Some(session) = &mut self.session
                    && let Some(TimelineItem::AgentMessage(item)) = session
                        .timeline
                        .iter_mut()
                        .find(|item| item.id() == Some(delta.item_id))
                {
                    item.text.push_str(&delta.text_delta);
                }
                self.follow_tail = true;
            }
            AppEvent::UsageUpdated(usage) => {
                if let Some(session) = &mut self.session {
                    session.usage = usage.clone();
                }
            }
            AppEvent::DiagnosisUpdated(diagnosis) => {
                if let Some(session) = &mut self.session {
                    session.diagnosis = Some(diagnosis.clone());
                }
            }
            AppEvent::ApprovalRequested(request) => {
                self.modal = Some(Modal::Approval(request.id));
                self.status = "Approval required".to_owned();
                return self.refresh_effect();
            }
            AppEvent::ApprovalResolved(_) | AppEvent::ArtifactCreated(_) => {
                return self.refresh_effect();
            }
            AppEvent::BudgetWarning(warning) => {
                self.status = warning.message.clone();
            }
            AppEvent::Notice(notice) => self.status = notice.message.clone(),
            AppEvent::Error(error) => self.modal = Some(Modal::Error(error.message.clone())),
            AppEvent::EventGap(_) => {
                self.status = "Event gap detected; rebuilding snapshot".to_owned();
                return self.refresh_effect();
            }
            AppEvent::ItemDelta(_) => {}
        }
        Vec::new()
    }

    fn refresh_effect(&self) -> Vec<Effect> {
        self.selected_session_id
            .map(Effect::OpenSession)
            .into_iter()
            .collect()
    }
}

fn configured_composer() -> TextArea<'static> {
    let mut composer = TextArea::default();
    composer.set_block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Message · Enter send · Alt+Enter newline "),
    );
    composer.set_placeholder_text("Ask FixTrace about the verified repair trace, or type /");
    composer.set_placeholder_style(Style::default().fg(Color::DarkGray));
    composer.set_cursor_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    composer.set_cursor_line_style(Style::default());
    composer
}
