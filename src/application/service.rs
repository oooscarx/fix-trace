use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use fixtrace_protocol::{AppEvent, EventBatch, EventEnvelope};
use fixtrace_store::EventStore;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::{
        diagnosis::Diagnosis,
        loop_runner::{AgentHistory, run_agent},
        tools::AnalysisTools,
    },
    config::FixTraceConfig,
    demo::run_demo,
    error::AppError,
    history::{
        database::HistoryDatabase,
        export::{export_session, import_session},
        paths::StatePaths,
    },
    llm::openai_compatible::OpenAiCompatibleProvider,
    progress::{ProgressEvent, ProgressSender},
    recorder::repl,
    workflow::{analyze_session, initialize_session, runner_for_session},
};

use super::types::{AnalysisResult, AppCommand, AppResponse, AppServiceOptions, SessionDetail};
use super::{presentation, protocol::progress_event_payload};

#[async_trait]
pub trait FixTraceApplication: Send + Sync {
    async fn execute(&self, command: AppCommand) -> Result<AppResponse, AppError>;

    fn subscribe_progress(&self) -> broadcast::Receiver<ProgressEvent>;

    fn subscribe_events(&self) -> broadcast::Receiver<EventEnvelope>;

    fn catch_up_events(
        &self,
        session_id: Option<Uuid>,
        after_sequence: u64,
        limit: u32,
    ) -> Result<EventBatch, AppError>;
}

#[derive(Clone)]
pub struct FixTraceAppService {
    pub(super) commands: mpsc::Sender<CommandEnvelope>,
    pub(super) progress: ProgressSender,
    pub(super) event_store: EventStore,
    pub(super) events: broadcast::Sender<EventEnvelope>,
    pub(super) cancellation: CancellationToken,
    pub(super) task_cancellations: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
    pub(super) session_tasks: Arc<Mutex<HashMap<Uuid, Uuid>>>,
}

impl FixTraceAppService {
    pub fn start(
        options: AppServiceOptions,
        cancellation: CancellationToken,
    ) -> Result<Self, AppError> {
        let state_paths = StatePaths::discover(options.state_dir)?;
        let config_path = options
            .config_path
            .unwrap_or_else(|| state_paths.config.clone());
        let config = FixTraceConfig::load_or_default(&config_path)?;
        let event_store = if options.initialize_event_store {
            EventStore::open(&state_paths.database)?
        } else {
            EventStore::deferred(&state_paths.database)
        };
        let (progress, initial_receiver) = ProgressSender::channel(1_024);
        drop(initial_receiver);
        let (events, initial_events) = broadcast::channel(4_096);
        drop(initial_events);
        let (commands, receiver) = mpsc::channel(32);
        let actor = AppServiceActor {
            state_paths,
            config_path,
            config,
            progress: progress.clone(),
            event_store: event_store.clone(),
            events: events.clone(),
            receiver,
        };
        tokio::spawn(actor.run());
        Ok(Self {
            commands,
            progress,
            event_store,
            events,
            cancellation,
            task_cancellations: Arc::new(Mutex::new(HashMap::new())),
            session_tasks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub(super) async fn execute_with_cancellation(
        &self,
        command: AppCommand,
        cancellation: CancellationToken,
    ) -> Result<AppResponse, AppError> {
        self.execute_with_context(command, cancellation, None).await
    }

    pub(super) async fn execute_with_context(
        &self,
        command: AppCommand,
        cancellation: CancellationToken,
        task_id: Option<Uuid>,
    ) -> Result<AppResponse, AppError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(CommandEnvelope {
                command,
                cancellation,
                task_id,
                response: response_tx,
            })
            .await
            .map_err(|_| AppError::ServiceUnavailable)?;
        response_rx
            .await
            .map_err(|_| AppError::ServiceUnavailable)?
    }

    pub(super) fn publish(
        &self,
        session_id: Option<Uuid>,
        task_id: Option<Uuid>,
        payload: AppEvent,
    ) -> Result<EventEnvelope, AppError> {
        publish_event(
            &self.event_store,
            &self.events,
            session_id,
            task_id,
            payload,
        )
    }
}

#[async_trait]
impl FixTraceApplication for FixTraceAppService {
    async fn execute(&self, command: AppCommand) -> Result<AppResponse, AppError> {
        self.execute_with_cancellation(command, self.cancellation.child_token())
            .await
    }

    fn subscribe_progress(&self) -> broadcast::Receiver<ProgressEvent> {
        self.progress.subscribe()
    }

    fn subscribe_events(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    fn catch_up_events(
        &self,
        session_id: Option<Uuid>,
        after_sequence: u64,
        limit: u32,
    ) -> Result<EventBatch, AppError> {
        Ok(self
            .event_store
            .load_after(session_id, after_sequence, limit)?)
    }
}

pub(super) struct CommandEnvelope {
    command: AppCommand,
    cancellation: CancellationToken,
    task_id: Option<Uuid>,
    response: oneshot::Sender<Result<AppResponse, AppError>>,
}

struct AppServiceActor {
    state_paths: StatePaths,
    config_path: std::path::PathBuf,
    config: FixTraceConfig,
    progress: ProgressSender,
    event_store: EventStore,
    events: broadcast::Sender<EventEnvelope>,
    receiver: mpsc::Receiver<CommandEnvelope>,
}

impl AppServiceActor {
    async fn run(mut self) {
        while let Some(envelope) = self.receiver.recv().await {
            match envelope.command {
                AppCommand::GetConfig => {
                    let result = self
                        .config
                        .to_toml()
                        .map(|toml| AppResponse::Config { toml });
                    let _ignored = envelope.response.send(result);
                }
                AppCommand::SetConfig { key, value } => {
                    let mut config = self.config.clone();
                    let result = config.set(&key, &value).and_then(|()| {
                        config.save(&self.config_path)?;
                        Ok(AppResponse::ConfigSaved {
                            key,
                            path: self.config_path.clone(),
                        })
                    });
                    if result.is_ok() {
                        self.config = config;
                    }
                    let _ignored = envelope.response.send(result);
                }
                command => {
                    let worker = AppServiceWorker {
                        state_paths: self.state_paths.clone(),
                        config: self.config.clone(),
                        progress: self.progress.clone(),
                        event_store: self.event_store.clone(),
                        events: self.events.clone(),
                    };
                    tokio::spawn(async move {
                        let result = worker
                            .execute(command, envelope.cancellation, envelope.task_id)
                            .await;
                        let _ignored = envelope.response.send(result);
                    });
                }
            }
        }
    }
}

#[derive(Clone)]
struct AppServiceWorker {
    state_paths: StatePaths,
    config: FixTraceConfig,
    progress: ProgressSender,
    event_store: EventStore,
    events: broadcast::Sender<EventEnvelope>,
}

impl AppServiceWorker {
    async fn execute(
        &self,
        command: AppCommand,
        cancellation: CancellationToken,
        task_id: Option<Uuid>,
    ) -> Result<AppResponse, AppError> {
        match command {
            AppCommand::RunDemo { no_llm } => Ok(AppResponse::Demo {
                report: Box::new(run_demo(no_llm, cancellation).await?),
            }),
            AppCommand::GetConfig | AppCommand::SetConfig { .. } => {
                Err(AppError::ServiceInvariant(
                    "configuration command reached an App Service worker".to_owned(),
                ))
            }
            command => {
                let database = HistoryDatabase::open(&self.state_paths.database)?;
                self.execute_with_database(command, &database, cancellation, task_id)
                    .await
            }
        }
    }

    async fn execute_with_database(
        &self,
        command: AppCommand,
        database: &HistoryDatabase,
        cancellation: CancellationToken,
        task_id: Option<Uuid>,
    ) -> Result<AppResponse, AppError> {
        match command {
            AppCommand::InitializeSession { project, oracle } => {
                let session = initialize_session(
                    &self.state_paths,
                    database,
                    &self.config,
                    &project,
                    oracle,
                    &cancellation,
                    self.progress.clone(),
                )
                .await?;
                self.publish(
                    Some(session.id),
                    None,
                    AppEvent::SessionCreated(presentation::session_summary(&session)),
                )?;
                Ok(AppResponse::SessionInitialized { session })
            }
            AppCommand::RunControlledShell { session_id } => {
                let progress = self.progress_for(Some(session_id), task_id);
                repl::run(database, &self.config, session_id, &cancellation, progress).await?;
                Ok(AppResponse::ControlledShellCompleted { session_id })
            }
            AppCommand::AnalyzeSession { session_id, no_llm } => {
                let progress = self.progress_for(Some(session_id), task_id);
                let report = analyze_session(
                    database,
                    &self.config,
                    session_id,
                    &cancellation,
                    progress.clone(),
                )
                .await?;
                let mut diagnosis = Diagnosis::offline(&report);
                let mut agent = None;
                if !no_llm && std::env::var_os(&self.config.model.api_key_env).is_some() {
                    let session = database.load_session(session_id)?;
                    let actions = database.load_actions(session_id)?;
                    let runner = runner_for_session(&session, &self.config, progress.clone())?;
                    let provider = OpenAiCompatibleProvider::from_config(&self.config.model)?;
                    let mut tools = AnalysisTools::new(
                        &runner,
                        &actions,
                        &report,
                        Some(database),
                        Some(session_id),
                        cancellation.child_token(),
                    );
                    let result = run_agent(
                        &provider,
                        &mut tools,
                        &self.config,
                        cancellation.child_token(),
                        Some(&progress),
                        AgentHistory {
                            database: Some(database),
                            session_id: Some(session_id),
                        },
                    )
                    .await?;
                    if let Some(model_diagnosis) = &result.diagnosis {
                        diagnosis = model_diagnosis.clone();
                    }
                    agent = Some(result);
                } else {
                    database.insert_json(
                        "diagnoses",
                        Some(session_id),
                        &serde_json::to_value(&diagnosis)?,
                    )?;
                }
                let llm_mode = if agent.is_some() {
                    "configured-provider"
                } else {
                    "offline-no-llm"
                };
                self.publish(
                    Some(session_id),
                    None,
                    AppEvent::DiagnosisUpdated(presentation::diagnosis_view(&diagnosis)),
                )?;
                let session = database.load_session(session_id)?;
                self.publish(
                    Some(session_id),
                    None,
                    AppEvent::SessionUpdated(presentation::session_summary(&session)),
                )?;
                Ok(AppResponse::SessionAnalyzed {
                    result: Box::new(AnalysisResult {
                        session_id,
                        report,
                        diagnosis,
                        agent,
                        llm_mode,
                    }),
                })
            }
            AppCommand::GetSession { session_id } => Ok(AppResponse::Session {
                detail: Box::new(SessionDetail {
                    session: database.load_session(session_id)?,
                    actions: database.load_actions(session_id)?,
                    trials: database.load_trials(session_id)?,
                    messages: database.load_json("messages", session_id)?,
                    tool_calls: database.load_json("tool_calls", session_id)?,
                    api_usage: database.load_json("api_usage", session_id)?,
                    progress_events: database.load_json("progress_events", session_id)?,
                    diagnoses: database.load_json("diagnoses", session_id)?,
                }),
            }),
            AppCommand::ListSessions => Ok(AppResponse::Sessions {
                sessions: database.list_sessions()?,
            }),
            AppCommand::ExportSession { session_id, output } => {
                export_session(database, session_id, &output)?;
                Ok(AppResponse::SessionExported { session_id, output })
            }
            AppCommand::ImportSession { input } => {
                let imported = import_session(database, &input)?;
                self.publish(
                    Some(imported.session.id),
                    None,
                    AppEvent::SessionCreated(presentation::session_summary(&imported.session)),
                )?;
                Ok(AppResponse::SessionImported {
                    session_id: imported.session.id,
                })
            }
            AppCommand::GetConfig | AppCommand::SetConfig { .. } | AppCommand::RunDemo { .. } => {
                Err(AppError::ServiceInvariant(
                    "command reached the wrong App Service dispatcher".to_owned(),
                ))
            }
        }
    }

    fn progress_for(&self, session_id: Option<Uuid>, task_id: Option<Uuid>) -> ProgressSender {
        let store = self.event_store.clone();
        let events = self.events.clone();
        let config = self.config.clone();
        self.progress.with_observer(move |progress| {
            if let Some(payload) = progress_event_payload(progress, &config)
                && let Err(error) = publish_event(&store, &events, session_id, task_id, payload)
            {
                tracing::error!("failed to persist progress event: {error}");
            }
        })
    }

    fn publish(
        &self,
        session_id: Option<Uuid>,
        task_id: Option<Uuid>,
        payload: AppEvent,
    ) -> Result<EventEnvelope, AppError> {
        publish_event(
            &self.event_store,
            &self.events,
            session_id,
            task_id,
            payload,
        )
    }
}

fn publish_event(
    store: &EventStore,
    events: &broadcast::Sender<EventEnvelope>,
    session_id: Option<Uuid>,
    task_id: Option<Uuid>,
    payload: AppEvent,
) -> Result<EventEnvelope, AppError> {
    let envelope = store.append(session_id, task_id, payload)?;
    let _ignored = events.send(envelope.clone());
    Ok(envelope)
}
