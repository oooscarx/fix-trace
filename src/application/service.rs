use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

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

#[async_trait]
pub trait FixTraceApplication: Send + Sync {
    async fn execute(&self, command: AppCommand) -> Result<AppResponse, AppError>;

    fn subscribe_progress(&self) -> broadcast::Receiver<ProgressEvent>;
}

#[derive(Clone)]
pub struct FixTraceAppService {
    commands: mpsc::Sender<CommandEnvelope>,
    progress: ProgressSender,
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
        let (progress, initial_receiver) = ProgressSender::channel(1_024);
        drop(initial_receiver);
        let (commands, receiver) = mpsc::channel(32);
        let actor = AppServiceActor {
            state_paths,
            config_path,
            config,
            cancellation,
            progress: progress.clone(),
            receiver,
        };
        tokio::spawn(actor.run());
        Ok(Self { commands, progress })
    }
}

#[async_trait]
impl FixTraceApplication for FixTraceAppService {
    async fn execute(&self, command: AppCommand) -> Result<AppResponse, AppError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(CommandEnvelope {
                command,
                response: response_tx,
            })
            .await
            .map_err(|_| AppError::ServiceUnavailable)?;
        response_rx
            .await
            .map_err(|_| AppError::ServiceUnavailable)?
    }

    fn subscribe_progress(&self) -> broadcast::Receiver<ProgressEvent> {
        self.progress.subscribe()
    }
}

struct CommandEnvelope {
    command: AppCommand,
    response: oneshot::Sender<Result<AppResponse, AppError>>,
}

struct AppServiceActor {
    state_paths: StatePaths,
    config_path: std::path::PathBuf,
    config: FixTraceConfig,
    cancellation: CancellationToken,
    progress: ProgressSender,
    receiver: mpsc::Receiver<CommandEnvelope>,
}

impl AppServiceActor {
    async fn run(mut self) {
        while let Some(envelope) = self.receiver.recv().await {
            let result = self.execute(envelope.command).await;
            let _ignored = envelope.response.send(result);
        }
    }

    async fn execute(&mut self, command: AppCommand) -> Result<AppResponse, AppError> {
        match command {
            AppCommand::GetConfig => Ok(AppResponse::Config {
                toml: self.config.to_toml()?,
            }),
            AppCommand::SetConfig { key, value } => {
                self.config.set(&key, &value)?;
                self.config.save(&self.config_path)?;
                Ok(AppResponse::ConfigSaved {
                    key,
                    path: self.config_path.clone(),
                })
            }
            AppCommand::RunDemo { no_llm } => Ok(AppResponse::Demo {
                report: Box::new(run_demo(no_llm, self.cancellation.child_token()).await?),
            }),
            command => {
                let database = HistoryDatabase::open(&self.state_paths.database)?;
                self.execute_with_database(command, &database).await
            }
        }
    }

    async fn execute_with_database(
        &self,
        command: AppCommand,
        database: &HistoryDatabase,
    ) -> Result<AppResponse, AppError> {
        match command {
            AppCommand::InitializeSession { project, oracle } => {
                let session = initialize_session(
                    &self.state_paths,
                    database,
                    &self.config,
                    &project,
                    oracle,
                    &self.cancellation,
                    self.progress.clone(),
                )
                .await?;
                Ok(AppResponse::SessionInitialized { session })
            }
            AppCommand::RunControlledShell { session_id } => {
                repl::run(
                    database,
                    &self.config,
                    session_id,
                    &self.cancellation,
                    self.progress.clone(),
                )
                .await?;
                Ok(AppResponse::ControlledShellCompleted { session_id })
            }
            AppCommand::AnalyzeSession { session_id, no_llm } => {
                let report = analyze_session(
                    database,
                    &self.config,
                    session_id,
                    &self.cancellation,
                    self.progress.clone(),
                )
                .await?;
                let mut diagnosis = Diagnosis::offline(&report);
                let mut agent = None;
                if !no_llm && std::env::var_os(&self.config.model.api_key_env).is_some() {
                    let session = database.load_session(session_id)?;
                    let actions = database.load_actions(session_id)?;
                    let runner = runner_for_session(&session, &self.config, self.progress.clone())?;
                    let provider = OpenAiCompatibleProvider::from_config(&self.config.model)?;
                    let mut tools = AnalysisTools::new(
                        &runner,
                        &actions,
                        &report,
                        Some(database),
                        Some(session_id),
                        self.cancellation.child_token(),
                    );
                    let result = run_agent(
                        &provider,
                        &mut tools,
                        &self.config,
                        self.cancellation.child_token(),
                        Some(&self.progress),
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
}
