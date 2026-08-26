use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    time::Instant,
};

use async_trait::async_trait;
use base64::Engine;
use chrono::Utc;
use fixtrace_presenter::{SessionViewInput, present_session};
use fixtrace_protocol::{
    ActionListResponse, AgentMessageDelta, AgentMessageItem, AppErrorView, AppEvent, AppRequest,
    AppResponsePayload, ArtifactSummary, ConfigValue, ConnectionTestRequest,
    ConnectionTestResponse, DependencyGraphView, DiffFileView, DiffView, EmptyRequest, EntityKind,
    EntityRef, ErrorCode, EventBatch, EventEnvelope, InitializeRequest, InitializeResponse,
    ItemDelta, ItemStatus, Notice, NoticeLevel, PROTOCOL_VERSION, PageInfo, PublicConfigSummary,
    ServerCapabilities, SessionListResponse, SessionSnapshot, SubscriptionStarted, TaskFailure,
    TaskInput, TaskProgress, TaskResult, TaskStatus, TaskSummary, TimelineItem, TimelineItemHeader,
    ToolCallItem, TrialItem, TrialListResponse, UserMessageItem,
};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::diagnosis::Diagnosis,
    application::{AppCommand, AppResponse, FixTraceAppService, FixTraceApplication},
    config::FixTraceConfig,
    domain::action::ArtifactRef,
    domain::snapshot::SnapshotManifest,
    error::AppError,
    llm::usage::UsageSummary as CoreUsageSummary,
    minimize::engine::MinimizationReport,
    progress::ProgressEvent,
};

use super::presentation;

#[async_trait]
pub trait FixTraceProtocolApplication: Send + Sync {
    async fn initialize_protocol(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, AppErrorView>;

    async fn execute_protocol(
        &self,
        client_id: Uuid,
        operation_id: Uuid,
        request: AppRequest,
    ) -> Result<AppResponsePayload, AppErrorView>;

    fn subscribe_protocol_events(&self) -> broadcast::Receiver<EventEnvelope>;

    fn catch_up_protocol_events(
        &self,
        session_id: Option<Uuid>,
        after_sequence: u64,
        limit: u32,
    ) -> Result<EventBatch, AppErrorView>;
}

#[async_trait]
impl FixTraceProtocolApplication for FixTraceAppService {
    async fn initialize_protocol(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, AppErrorView> {
        if !fixtrace_protocol::is_compatible_protocol_version(&request.protocol_version) {
            return Err(AppErrorView::new(
                ErrorCode::IncompatibleProtocol,
                format!(
                    "client requested `{}`; server supports `{PROTOCOL_VERSION}`",
                    request.protocol_version
                ),
            ));
        }
        let config = self.current_config().await?;
        Ok(InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: server_capabilities(),
            config_summary: public_config(&config),
            client_id: Uuid::new_v4(),
        })
    }

    async fn execute_protocol(
        &self,
        client_id: Uuid,
        operation_id: Uuid,
        request: AppRequest,
    ) -> Result<AppResponsePayload, AppErrorView> {
        match request {
            AppRequest::Initialize(request) => self
                .initialize_protocol(request)
                .await
                .map(AppResponsePayload::Initialized),
            AppRequest::SessionList(request) => {
                let AppResponse::Sessions { sessions } = self
                    .execute(AppCommand::ListSessions)
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                let mut sessions: Vec<_> = sessions
                    .iter()
                    .map(|session| self.protocol_session_summary(session))
                    .collect::<Result<_, _>>()?;
                if !request.include_archived {
                    sessions.retain(|session| !session.archived);
                }
                let (sessions, page) = paginate(sessions, &request.page)?;
                Ok(AppResponsePayload::SessionList(SessionListResponse {
                    sessions,
                    page,
                }))
            }
            AppRequest::SessionCreate(request) => {
                let AppResponse::SessionInitialized { session } = self
                    .execute(AppCommand::InitializeSession {
                        project: request.project,
                        oracle: request.oracle,
                        title: request.title,
                    })
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                Ok(AppResponsePayload::Session(
                    self.protocol_session_summary(&session)?,
                ))
            }
            AppRequest::SessionOpen(request) => {
                let detail = self.session_detail(request.session_id).await?;
                Ok(AppResponsePayload::Session(
                    self.protocol_session_summary(&detail.session)?,
                ))
            }
            AppRequest::SessionFork(request) => {
                let AppResponse::SessionChanged { session } = self
                    .execute(AppCommand::ForkSession {
                        session_id: request.session_id,
                        title: request.title,
                    })
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                Ok(AppResponsePayload::Session(
                    self.protocol_session_summary(&session)?,
                ))
            }
            AppRequest::SessionArchive(request) => {
                let AppResponse::SessionChanged { session } = self
                    .execute(AppCommand::ArchiveSession {
                        session_id: request.session_id,
                    })
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                Ok(AppResponsePayload::Session(
                    self.protocol_session_summary(&session)?,
                ))
            }
            AppRequest::SessionGetSnapshot(request) => self
                .session_snapshot(request.session_id)
                .await
                .map(|snapshot| AppResponsePayload::SessionSnapshot(Box::new(snapshot))),
            AppRequest::TaskStart(request) => self
                .start_task(operation_id, request.session_id, request.input)
                .await
                .map(AppResponsePayload::Task),
            AppRequest::TaskGet(request) => self
                .event_store
                .load_task(request.task_id)
                .map(AppResponsePayload::Task)
                .map_err(store_error_view),
            AppRequest::TaskCancel(request) => self
                .cancel_task(request.task_id)
                .map(AppResponsePayload::Task),
            AppRequest::TaskSteer(request) => {
                if request.message.trim().is_empty() {
                    return Err(AppErrorView::new(
                        ErrorCode::InvalidRequest,
                        "steering message cannot be empty",
                    ));
                }
                let task = self
                    .event_store
                    .load_task(request.task_id)
                    .map_err(store_error_view)?;
                if !task.supports_steer || task.status.is_terminal() {
                    return Err(AppErrorView::new(
                        ErrorCode::InvalidTransition,
                        "the current task does not accept steering",
                    ));
                }
                self.task_steers
                    .lock()
                    .map_err(|_| invariant_error())?
                    .get(&task.id)
                    .ok_or_else(|| {
                        AppErrorView::new(
                            ErrorCode::NotFound,
                            "task steering channel was not found",
                        )
                    })?
                    .send(request.message.clone())
                    .map_err(|_| {
                        AppErrorView::new(
                            ErrorCode::InvalidTransition,
                            "task stopped before steering was delivered",
                        )
                    })?;
                if let Some(session_id) = task.session_id {
                    self.publish(
                        Some(session_id),
                        Some(task.id),
                        AppEvent::ItemCompleted(TimelineItem::UserMessage(UserMessageItem {
                            header: timeline_header(
                                Uuid::new_v4(),
                                ItemStatus::Completed,
                                Some(EntityRef {
                                    kind: EntityKind::Task,
                                    id: task.id.to_string(),
                                }),
                            ),
                            text: request.message,
                        })),
                    )
                    .map_err(app_error_view)?;
                }
                Ok(AppResponsePayload::Accepted {
                    message: "Steering queued for the active task".to_owned(),
                })
            }
            AppRequest::ActionList(request) => {
                let detail = self.session_detail(request.session_id).await?;
                let actions: Vec<_> = detail
                    .actions
                    .iter()
                    .map(presentation::action_view)
                    .collect();
                let (actions, page) = paginate(actions, &request.page)?;
                Ok(AppResponsePayload::ActionList(ActionListResponse {
                    actions,
                    page,
                }))
            }
            AppRequest::ActionGet(request) => {
                let detail = self.session_detail(request.session_id).await?;
                detail
                    .actions
                    .iter()
                    .find(|action| action.id == request.action_id)
                    .map(presentation::action_view)
                    .map(AppResponsePayload::Action)
                    .ok_or_else(|| AppErrorView::new(ErrorCode::NotFound, "action was not found"))
            }
            AppRequest::TrialList(request) => {
                let detail = self.session_detail(request.session_id).await?;
                let trials: Vec<_> = detail.trials.iter().map(presentation::trial_view).collect();
                let (trials, page) = paginate(trials, &request.page)?;
                Ok(AppResponsePayload::TrialList(TrialListResponse {
                    trials,
                    page,
                }))
            }
            AppRequest::TrialGet(request) => {
                let detail = self.session_detail(request.session_id).await?;
                detail
                    .trials
                    .iter()
                    .find(|trial| trial.id == request.trial_id)
                    .map(presentation::trial_view)
                    .map(AppResponsePayload::Trial)
                    .ok_or_else(|| AppErrorView::new(ErrorCode::NotFound, "trial was not found"))
            }
            AppRequest::TrialRun(request) => {
                let AppResponse::TrialCompleted { trial, .. } = self
                    .execute(AppCommand::RunCandidate {
                        session_id: request.session_id,
                        action_ids: Some(request.action_ids),
                        repetitions: None,
                    })
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                Ok(AppResponsePayload::Trial(presentation::trial_view(&trial)))
            }
            AppRequest::TrialRepeat(request) => {
                let AppResponse::TrialCompleted { trial, .. } = self
                    .execute(AppCommand::RepeatTrial {
                        session_id: request.session_id,
                        trial_id: request.trial_id,
                        repetitions: request.repetitions,
                    })
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                Ok(AppResponsePayload::Trial(presentation::trial_view(&trial)))
            }
            AppRequest::DependencyGetGraph(request) => {
                let detail = self.session_detail(request.session_id).await?;
                Ok(AppResponsePayload::DependencyGraph(
                    report_from_history(&detail.diagnoses).map_or_else(
                        || DependencyGraphView {
                            nodes: detail
                                .actions
                                .iter()
                                .map(|action| fixtrace_protocol::DependencyNodeView {
                                    action_id: action.id,
                                    label: presentation::action_view(action).summary,
                                    in_minimal_set: false,
                                })
                                .collect(),
                            edges: Vec::new(),
                        },
                        |report| {
                            presentation::dependency_graph_view(
                                &report.dependency_graph,
                                &detail.actions,
                                &report.minimal_action_ids,
                            )
                        },
                    ),
                ))
            }
            AppRequest::DiagnosisGet(request) => {
                let detail = self.session_detail(request.session_id).await?;
                Ok(AppResponsePayload::Diagnosis(
                    diagnosis_from_history(&detail.diagnoses)
                        .as_ref()
                        .map(presentation::diagnosis_view),
                ))
            }
            AppRequest::ConfigGet(EmptyRequest {}) => {
                let config = self.current_config().await?;
                Ok(AppResponsePayload::Config(public_config(&config)))
            }
            AppRequest::ConfigUpdate(request) => {
                for update in request.updates {
                    self.execute(AppCommand::SetConfig {
                        key: update.key,
                        value: config_value_text(update.value),
                    })
                    .await
                    .map_err(app_error_view)?;
                }
                let config = self.current_config().await?;
                Ok(AppResponsePayload::Config(public_config(&config)))
            }
            AppRequest::UsageGet(request) => {
                let config = self.current_config().await?;
                let usage = if let Some(session_id) = request.session_id {
                    let detail = self.session_detail(session_id).await?;
                    usage_from_history(&detail.api_usage)
                } else {
                    CoreUsageSummary::default()
                };
                Ok(AppResponsePayload::Usage(presentation::usage_view(
                    &usage, &config,
                )))
            }
            AppRequest::SessionExport(request) => {
                self.execute(AppCommand::ExportSession {
                    session_id: request.session_id,
                    output: request.output.clone(),
                })
                .await
                .map_err(app_error_view)?;
                Ok(AppResponsePayload::Exported {
                    session_id: request.session_id,
                    output: request.output,
                })
            }
            AppRequest::SessionImport(request) => {
                let AppResponse::SessionImported { session_id } = self
                    .execute(AppCommand::ImportSession {
                        input: request.input,
                    })
                    .await
                    .map_err(app_error_view)?
                else {
                    return Err(invariant_error());
                };
                Ok(AppResponsePayload::Imported { session_id })
            }
            AppRequest::EventSubscribe(request) => {
                let batch = self.catch_up_protocol_events(
                    Some(request.session_id),
                    request.after_sequence.unwrap_or(0),
                    10_000,
                )?;
                let stream_id = batch
                    .events
                    .first()
                    .map(|event| event.stream_id)
                    .unwrap_or_else(Uuid::nil);
                Ok(AppResponsePayload::Subscription(SubscriptionStarted {
                    subscription_id: Uuid::new_v4(),
                    stream_id,
                    after_sequence: request.after_sequence.unwrap_or(0),
                    high_watermark: batch.high_watermark,
                }))
            }
            AppRequest::EventUnsubscribe(_) => Ok(AppResponsePayload::Accepted {
                message: "subscription closed".to_owned(),
            }),
            AppRequest::ApprovalRespond(request) => {
                let pending = self
                    .event_store
                    .load_approval(request.approval_id)
                    .map_err(store_error_view)?;
                let status = match request.choice {
                    fixtrace_protocol::ApprovalChoice::ApproveOnce
                    | fixtrace_protocol::ApprovalChoice::ApproveForTask
                    | fixtrace_protocol::ApprovalChoice::ApproveEquivalentForSession => {
                        fixtrace_protocol::ApprovalStatus::Approved
                    }
                    fixtrace_protocol::ApprovalChoice::Deny => {
                        fixtrace_protocol::ApprovalStatus::Denied
                    }
                    fixtrace_protocol::ApprovalChoice::CancelTask => {
                        fixtrace_protocol::ApprovalStatus::Cancelled
                    }
                };
                let resolution = fixtrace_protocol::ApprovalResolution {
                    approval_id: request.approval_id,
                    choice: request.choice,
                    status,
                    resolved_by_client_id: client_id,
                    resolved_at: Utc::now(),
                    equivalent_rule_id: None,
                };
                self.event_store
                    .resolve_approval(&resolution)
                    .map_err(store_error_view)?;
                self.publish(
                    Some(pending.request.session_id),
                    Some(pending.request.task_id),
                    AppEvent::ApprovalResolved(resolution),
                )
                .map_err(app_error_view)?;
                Ok(AppResponsePayload::Accepted {
                    message: "approval resolved".to_owned(),
                })
            }
            AppRequest::ConfigTestConnection(request) => Ok(AppResponsePayload::ConnectionTest(
                self.test_connection(request).await,
            )),
            AppRequest::ArtifactList(request) => {
                let detail = self.session_detail(request.session_id).await?;
                let artifacts = self.artifacts_for_session(&detail)?;
                let (artifacts, page) = paginate(
                    artifacts
                        .into_iter()
                        .map(|artifact| artifact.summary)
                        .collect(),
                    &request.page,
                )?;
                Ok(AppResponsePayload::ArtifactList { artifacts, page })
            }
            AppRequest::ArtifactRead(request) => {
                let artifact = self.find_artifact(request.artifact_id).await?;
                let mut file = File::open(&artifact.path).map_err(|error| {
                    AppErrorView::new(
                        ErrorCode::Internal,
                        format!("cannot open artifact: {error}"),
                    )
                })?;
                let file_size = file
                    .metadata()
                    .map_err(|error| {
                        AppErrorView::new(
                            ErrorCode::Internal,
                            format!("cannot inspect artifact: {error}"),
                        )
                    })?
                    .len();
                let offset = request.offset.min(file_size);
                file.seek(SeekFrom::Start(offset)).map_err(|error| {
                    AppErrorView::new(
                        ErrorCode::Internal,
                        format!("cannot seek artifact: {error}"),
                    )
                })?;
                let limit = usize::try_from(request.validated_limit())
                    .expect("artifact read limit fits usize");
                let mut bytes = vec![0_u8; limit];
                let read = file.read(&mut bytes).map_err(|error| {
                    AppErrorView::new(
                        ErrorCode::Internal,
                        format!("cannot read artifact: {error}"),
                    )
                })?;
                bytes.truncate(read);
                let next_offset = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                Ok(AppResponsePayload::Artifact(
                    fixtrace_protocol::ArtifactChunk {
                        artifact_id: artifact.summary.id,
                        offset,
                        next_offset,
                        eof: next_offset >= file_size,
                        bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                        sha256: artifact.summary.sha256,
                    },
                ))
            }
            AppRequest::MessageSend(request) => self
                .start_task(
                    operation_id,
                    Some(request.session_id),
                    TaskInput::AgentTurn {
                        prompt: request.text,
                    },
                )
                .await
                .map(AppResponsePayload::Task),
            AppRequest::SessionDelete(_) => Err(AppErrorView::new(
                ErrorCode::InvalidRequest,
                "request is defined by the protocol but is not enabled in this milestone",
            )),
        }
    }

    fn subscribe_protocol_events(&self) -> broadcast::Receiver<EventEnvelope> {
        self.subscribe_events()
    }

    fn catch_up_protocol_events(
        &self,
        session_id: Option<Uuid>,
        after_sequence: u64,
        limit: u32,
    ) -> Result<EventBatch, AppErrorView> {
        self.catch_up_events(session_id, after_sequence, limit)
            .map_err(app_error_view)
    }
}

impl FixTraceAppService {
    async fn current_config(&self) -> Result<FixTraceConfig, AppErrorView> {
        let AppResponse::Config { toml } = self
            .execute(AppCommand::GetConfig)
            .await
            .map_err(app_error_view)?
        else {
            return Err(invariant_error());
        };
        toml::from_str(&toml).map_err(|_| invariant_error())
    }

    async fn test_connection(&self, request: ConnectionTestRequest) -> ConnectionTestResponse {
        let started = Instant::now();
        let config = match self.current_config().await {
            Ok(config) => config,
            Err(error) => {
                return connection_test_failure(started, error.message);
            }
        };
        if request.provider != "openai-compatible" {
            return connection_test_failure(
                started,
                format!("unsupported provider `{}`", request.provider),
            );
        }
        let credential_env = request
            .credential_id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(config.model.api_key_env);
        if !valid_environment_name(&credential_env) {
            return connection_test_failure(
                started,
                "credential reference must be an environment variable name",
            );
        }
        let Ok(key) = std::env::var(&credential_env) else {
            return connection_test_failure(
                started,
                format!("environment variable `{credential_env}` is not configured"),
            );
        };
        let key = key.trim();
        if key.is_empty() {
            return connection_test_failure(
                started,
                format!("environment variable `{credential_env}` is empty"),
            );
        }
        let endpoint = request.endpoint.trim().trim_end_matches('/');
        if endpoint.is_empty() {
            return connection_test_failure(started, "endpoint cannot be empty");
        }
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
        {
            Ok(client) => client,
            Err(error) => return connection_test_failure(started, error.to_string()),
        };
        let response = match client
            .get(format!("{endpoint}/models"))
            .bearer_auth(key)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return connection_test_failure(started, error.to_string()),
        };
        if !response.status().is_success() {
            return connection_test_failure(
                started,
                format!("model endpoint returned HTTP {}", response.status()),
            );
        }
        let value: serde_json::Value = match response.json().await {
            Ok(value) => value,
            Err(error) => {
                return connection_test_failure(
                    started,
                    format!("model endpoint returned invalid JSON: {error}"),
                );
            }
        };
        let models = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        let requested_model = request.model.trim();
        let selected_model = models
            .iter()
            .copied()
            .find(|model| *model == requested_model)
            .or_else(|| models.first().copied());
        let exact_model_available = requested_model.is_empty() || models.contains(&requested_model);
        ConnectionTestResponse {
            ok: exact_model_available && selected_model.is_some(),
            model: selected_model.map(str::to_owned),
            latency_ms: elapsed_millis(started),
            message: if selected_model.is_none() {
                "connected, but the endpoint returned no models".to_owned()
            } else if exact_model_available {
                format!(
                    "connected; model `{}` is available",
                    selected_model.unwrap_or_default()
                )
            } else {
                format!("connected, but model `{requested_model}` is not listed")
            },
        }
    }

    fn artifacts_for_session(
        &self,
        detail: &super::SessionDetail,
    ) -> Result<Vec<ResolvedArtifact>, AppErrorView> {
        let mut references = BTreeMap::<PathBuf, ArtifactRef>::new();
        for action in &detail.actions {
            if let Some(result) = &action.result {
                insert_artifact_ref(&mut references, result.stdout_artifact.as_ref());
                insert_artifact_ref(&mut references, result.stderr_artifact.as_ref());
            }
        }
        for trial in &detail.trials {
            for attempt in &trial.repetitions {
                for result in &attempt.actions {
                    insert_artifact_ref(&mut references, result.stdout_artifact.as_ref());
                    insert_artifact_ref(&mut references, result.stderr_artifact.as_ref());
                }
                if let Some(oracle) = &attempt.oracle {
                    insert_artifact_ref(&mut references, oracle.stdout_artifact.as_ref());
                    insert_artifact_ref(&mut references, oracle.stderr_artifact.as_ref());
                }
            }
        }
        let Some(session_root) = detail.session.baseline_path.parent() else {
            return Err(invariant_error());
        };
        let canonical_root = fs::canonicalize(session_root).map_err(|error| {
            AppErrorView::new(
                ErrorCode::Internal,
                format!("cannot inspect session artifact root: {error}"),
            )
        })?;
        let mut artifacts = Vec::new();
        for reference in references.into_values() {
            if !safe_artifact_relative_path(&reference.path) {
                continue;
            }
            let path = session_root.join(&reference.path);
            let Ok(canonical_path) = fs::canonicalize(&path) else {
                continue;
            };
            if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
                continue;
            }
            let metadata = fs::metadata(&canonical_path).map_err(|error| {
                AppErrorView::new(
                    ErrorCode::Internal,
                    format!("cannot inspect artifact metadata: {error}"),
                )
            })?;
            let created_at = metadata
                .modified()
                .map(chrono::DateTime::<Utc>::from)
                .unwrap_or(detail.session.updated_at);
            let name = reference
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("artifact")
                .to_owned();
            artifacts.push(ResolvedArtifact {
                summary: ArtifactSummary {
                    id: artifact_id(detail.session.id, &reference.path),
                    session_id: detail.session.id,
                    name,
                    media_type: "text/plain; charset=utf-8".to_owned(),
                    size: metadata.len(),
                    sha256: reference.sha256,
                    created_at,
                },
                path: canonical_path,
            });
        }
        artifacts.sort_by(|left, right| left.summary.name.cmp(&right.summary.name));
        Ok(artifacts)
    }

    async fn find_artifact(&self, artifact_id: Uuid) -> Result<ResolvedArtifact, AppErrorView> {
        let AppResponse::Sessions { sessions } = self
            .execute(AppCommand::ListSessions)
            .await
            .map_err(app_error_view)?
        else {
            return Err(invariant_error());
        };
        for session in sessions {
            let detail = self.session_detail(session.id).await?;
            if let Some(artifact) = self
                .artifacts_for_session(&detail)?
                .into_iter()
                .find(|artifact| artifact.summary.id == artifact_id)
            {
                return Ok(artifact);
            }
        }
        Err(AppErrorView::new(ErrorCode::NotFound, "artifact not found"))
    }

    fn protocol_session_summary(
        &self,
        session: &crate::domain::session::SessionRecord,
    ) -> Result<fixtrace_protocol::SessionSummary, AppErrorView> {
        let mut summary = presentation::session_summary(session);
        summary.active_task_id = self
            .event_store
            .active_task_for_session(session.id)
            .map_err(store_error_view)?
            .map(|task| task.id);
        Ok(summary)
    }

    async fn session_detail(&self, session_id: Uuid) -> Result<super::SessionDetail, AppErrorView> {
        let AppResponse::Session { detail } = self
            .execute(AppCommand::GetSession { session_id })
            .await
            .map_err(app_error_view)?
        else {
            return Err(invariant_error());
        };
        Ok(*detail)
    }

    async fn session_snapshot(&self, session_id: Uuid) -> Result<SessionSnapshot, AppErrorView> {
        let detail = self.session_detail(session_id).await?;
        let mut batch = self.catch_up_protocol_events(Some(session_id), 0, 10_000)?;
        if batch.events.is_empty() {
            self.publish(
                Some(session_id),
                None,
                AppEvent::SessionUpdated(presentation::session_summary(&detail.session)),
            )
            .map_err(app_error_view)?;
            batch = self.catch_up_protocol_events(Some(session_id), 0, 10_000)?;
        }
        let mut items = BTreeMap::new();
        for event in &batch.events {
            match &event.payload {
                AppEvent::ItemStarted(item) | AppEvent::ItemCompleted(item) => {
                    if let Some(id) = item.id() {
                        items.insert(id, item.clone());
                    }
                }
                _ => {}
            }
        }
        let report = report_from_history(&detail.diagnoses);
        let diagnosis = diagnosis_from_history(&detail.diagnoses);
        let config = self.current_config().await?;
        let usage = usage_from_history(&detail.api_usage);
        let dependency_graph = report.as_ref().map_or_else(
            || DependencyGraphView {
                nodes: detail
                    .actions
                    .iter()
                    .map(|action| fixtrace_protocol::DependencyNodeView {
                        action_id: action.id,
                        label: presentation::action_view(action).summary,
                        in_minimal_set: false,
                    })
                    .collect(),
                edges: Vec::new(),
            },
            |report| {
                presentation::dependency_graph_view(
                    &report.dependency_graph,
                    &detail.actions,
                    &report.minimal_action_ids,
                )
            },
        );
        let stream_id = batch
            .events
            .first()
            .map(|event| event.stream_id)
            .ok_or_else(invariant_error)?;
        let active_task = self
            .event_store
            .active_task_for_session(session_id)
            .map_err(store_error_view)?;
        let diff = if detail.session.worktree_path.is_dir() {
            snapshot_diff_view(
                &detail.session.baseline_manifest,
                &SnapshotManifest::capture(
                    &detail.session.worktree_path,
                    config.replay.include_target,
                )
                .map_err(app_error_view)?,
                &detail.session.baseline_path,
                &detail.session.worktree_path,
            )
        } else {
            DiffView {
                files: Vec::new(),
                truncated: false,
            }
        };
        Ok(SessionSnapshot {
            stream_id,
            through_sequence: batch.high_watermark,
            session: present_session(SessionViewInput {
                summary: self.protocol_session_summary(&detail.session)?,
                task: active_task,
                timeline: items.into_values().collect(),
                actions: detail
                    .actions
                    .iter()
                    .map(presentation::action_view)
                    .collect(),
                trials: detail.trials.iter().map(presentation::trial_view).collect(),
                diagnosis: diagnosis.as_ref().map(presentation::diagnosis_view),
                usage: presentation::usage_view(&usage, &config),
                approvals: Vec::new(),
                dependency_graph,
                diff,
            }),
        })
    }

    async fn start_task(
        &self,
        operation_id: Uuid,
        session_id: Option<Uuid>,
        input: TaskInput,
    ) -> Result<TaskSummary, AppErrorView> {
        if let Some(task) = self
            .event_store
            .load_task_by_operation(operation_id)
            .map_err(store_error_view)?
        {
            return Ok(task);
        }
        if let Some(session_id) = session_id {
            self.session_detail(session_id).await?;
        }
        let command = task_command(session_id, input.clone())?;
        let task_id = Uuid::new_v4();
        if let Some(session_id) = session_id {
            let mut sessions = self.session_tasks.lock().map_err(|_| invariant_error())?;
            if sessions.contains_key(&session_id) {
                return Err(AppErrorView::new(
                    ErrorCode::OperationInProgress,
                    "this session already has a mutating task",
                ));
            }
            sessions.insert(session_id, task_id);
        }
        let now = Utc::now();
        let task = TaskSummary {
            id: task_id,
            session_id,
            operation_id,
            kind: input.kind(),
            status: TaskStatus::Queued,
            title: task_title(&input).to_owned(),
            created_at: now,
            started_at: None,
            finished_at: None,
            progress_ratio: None,
            is_cancellable: true,
            supports_steer: task_supports_steer(&input),
        };
        if let Err(error) = self.event_store.save_task(&task) {
            if let Some(session_id) = session_id
                && let Ok(mut sessions) = self.session_tasks.lock()
            {
                sessions.remove(&session_id);
            }
            return Err(store_error_view(error));
        }
        let (steer_sender, steer_receiver) = tokio::sync::mpsc::unbounded_channel();
        self.task_steers
            .lock()
            .map_err(|_| invariant_error())?
            .insert(task_id, steer_sender);
        self.task_steer_receivers
            .lock()
            .map_err(|_| invariant_error())?
            .insert(task_id, steer_receiver);
        if let (Some(session_id), TaskInput::AgentTurn { prompt }) = (session_id, &input) {
            self.publish(
                Some(session_id),
                Some(task_id),
                AppEvent::ItemCompleted(TimelineItem::UserMessage(UserMessageItem {
                    header: timeline_header(
                        Uuid::new_v4(),
                        ItemStatus::Completed,
                        Some(EntityRef {
                            kind: EntityKind::Session,
                            id: session_id.to_string(),
                        }),
                    ),
                    text: prompt.clone(),
                })),
            )
            .map_err(app_error_view)?;
        }
        let cancellation = self.cancellation.child_token();
        self.task_cancellations
            .lock()
            .map_err(|_| invariant_error())?
            .insert(task_id, cancellation.clone());
        let service = self.clone();
        tokio::spawn(async move {
            service.run_task(task, command, cancellation).await;
        });
        self.event_store
            .load_task(task_id)
            .map_err(store_error_view)
    }

    async fn run_task(
        &self,
        queued: TaskSummary,
        command: AppCommand,
        cancellation: CancellationToken,
    ) {
        let result = async {
            let current = self.event_store.load_task(queued.id)?;
            if current.status.is_terminal() {
                return Ok::<(), AppError>(());
            }
            let running = self
                .event_store
                .transition_task(queued.id, TaskStatus::Running)?;
            self.publish(
                running.session_id,
                Some(running.id),
                AppEvent::TaskStarted(running.clone()),
            )?;
            let response = self
                .execute_with_context(command, cancellation.clone(), Some(queued.id))
                .await;
            if cancellation.is_cancelled() {
                let current = self.event_store.load_task(queued.id)?;
                let cancelled = if current.status == TaskStatus::Cancelling {
                    self.event_store
                        .transition_task(queued.id, TaskStatus::Cancelled)?
                } else if current.status == TaskStatus::Running {
                    let cancelling = self
                        .event_store
                        .transition_task(queued.id, TaskStatus::Cancelling)?;
                    self.publish(
                        cancelling.session_id,
                        Some(cancelling.id),
                        AppEvent::TaskProgress(TaskProgress {
                            task: cancelling.clone(),
                            current: None,
                            total: None,
                            unit: None,
                            message: "Cancelling".to_owned(),
                        }),
                    )?;
                    self.event_store
                        .transition_task(queued.id, TaskStatus::Cancelled)?
                } else {
                    current
                };
                self.publish(
                    cancelled.session_id,
                    Some(cancelled.id),
                    AppEvent::TaskCancelled(cancelled),
                )?;
            } else {
                match response {
                    Ok(response) => {
                        let completed = self
                            .event_store
                            .transition_task(queued.id, TaskStatus::Completed)?;
                        self.publish(
                            completed.session_id,
                            Some(completed.id),
                            AppEvent::TaskCompleted(TaskResult {
                                task: completed,
                                output: Some(serde_json::to_value(response)?),
                            }),
                        )?;
                    }
                    Err(error) => {
                        let failed = self
                            .event_store
                            .transition_task(queued.id, TaskStatus::Failed)?;
                        self.publish(
                            failed.session_id,
                            Some(failed.id),
                            AppEvent::TaskFailed(TaskFailure {
                                task: failed,
                                error: app_error_view(error),
                            }),
                        )?;
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            tracing::error!(task_id = %queued.id, "task supervisor failed: {error}");
        }
        if let Ok(mut tasks) = self.task_cancellations.lock() {
            tasks.remove(&queued.id);
        }
        if let Ok(mut steers) = self.task_steers.lock() {
            steers.remove(&queued.id);
        }
        if let Ok(mut receivers) = self.task_steer_receivers.lock() {
            receivers.remove(&queued.id);
        }
        if let Some(session_id) = queued.session_id
            && let Ok(mut sessions) = self.session_tasks.lock()
        {
            sessions.remove(&session_id);
        }
    }

    fn cancel_task(&self, task_id: Uuid) -> Result<TaskSummary, AppErrorView> {
        let task = self
            .event_store
            .load_task(task_id)
            .map_err(store_error_view)?;
        if task.status.is_terminal() {
            return Ok(task);
        }
        let next = if task.status == TaskStatus::Queued {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Cancelling
        };
        let updated = self
            .event_store
            .transition_task(task_id, next)
            .map_err(store_error_view)?;
        if let Some(cancellation) = self
            .task_cancellations
            .lock()
            .map_err(|_| invariant_error())?
            .get(&task_id)
        {
            cancellation.cancel();
        }
        if next == TaskStatus::Cancelled {
            self.publish(
                updated.session_id,
                Some(task_id),
                AppEvent::TaskCancelled(updated.clone()),
            )
            .map_err(app_error_view)?;
        }
        Ok(updated)
    }
}

pub(super) fn progress_event_payload(
    progress: &ProgressEvent,
    config: &FixTraceConfig,
) -> Option<AppEvent> {
    let notice = |code: &str, message: String| {
        AppEvent::Notice(Notice {
            code: code.to_owned(),
            level: NoticeLevel::Info,
            title: "FixTrace progress".to_owned(),
            message,
        })
    };
    match progress {
        ProgressEvent::SessionCreated { .. } => None,
        ProgressEvent::BaselineCopied => {
            Some(notice("baseline_copied", "Baseline copied".to_owned()))
        }
        ProgressEvent::OracleAttemptStarted { current, total } => Some(notice(
            "oracle_attempt",
            format!("Oracle attempt {current}/{total}"),
        )),
        ProgressEvent::ActionReplayStarted { action_id } => Some(notice(
            "action_replay_started",
            format!("Replaying action {action_id}"),
        )),
        ProgressEvent::TrialStarted {
            trial_id,
            current,
            total,
        } => Some(AppEvent::ItemStarted(TimelineItem::Trial(TrialItem {
            header: timeline_header(
                *trial_id,
                ItemStatus::Running,
                Some(EntityRef {
                    kind: EntityKind::Trial,
                    id: trial_id.to_string(),
                }),
            ),
            trial_id: *trial_id,
            action_ids: Vec::new(),
            classification: fixtrace_protocol::TrialClassification::Unresolved,
            repetition_current: u32::try_from(*current).ok(),
            repetition_total: u32::try_from(*total).unwrap_or(u32::MAX),
            summary: format!("Trial repetition {current}/{total}"),
        }))),
        ProgressEvent::TrialCompleted { trial_id, outcome } => {
            Some(AppEvent::ItemCompleted(TimelineItem::Trial(TrialItem {
                header: timeline_header(
                    *trial_id,
                    ItemStatus::Completed,
                    Some(EntityRef {
                        kind: EntityKind::Trial,
                        id: trial_id.to_string(),
                    }),
                ),
                trial_id: *trial_id,
                action_ids: Vec::new(),
                classification: presentation::trial_classification(outcome),
                repetition_current: None,
                repetition_total: 0,
                summary: format!("Trial completed: {outcome:?}"),
            })))
        }
        ProgressEvent::CandidateReduced { before, after } => Some(notice(
            "candidate_reduced",
            format!("Candidate reduced from {before} to {after} actions"),
        )),
        ProgressEvent::AgentStepStarted { step } => Some(notice(
            "agent_step_started",
            format!("Agent step {step} started"),
        )),
        ProgressEvent::AgentMessageStarted { item_id } => Some(AppEvent::ItemStarted(
            TimelineItem::AgentMessage(AgentMessageItem {
                header: timeline_header(*item_id, ItemStatus::Running, None),
                text: String::new(),
                public_reasoning_summary: None,
            }),
        )),
        ProgressEvent::AgentTextDelta {
            item_id,
            text_delta,
        } => Some(AppEvent::ItemDelta(ItemDelta::AgentMessage(
            AgentMessageDelta {
                item_id: *item_id,
                text_delta: text_delta.clone(),
            },
        ))),
        ProgressEvent::AgentMessageCompleted { item_id, text } => Some(AppEvent::ItemCompleted(
            TimelineItem::AgentMessage(AgentMessageItem {
                header: timeline_header(*item_id, ItemStatus::Completed, None),
                text: text.clone(),
                public_reasoning_summary: None,
            }),
        )),
        ProgressEvent::ToolCallStarted {
            item_id,
            tool_call_id,
            name,
            arguments_summary,
        } => Some(AppEvent::ItemStarted(TimelineItem::ToolCall(
            ToolCallItem {
                header: timeline_header(*item_id, ItemStatus::Running, None),
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                arguments_summary: arguments_summary.clone(),
                result_summary: None,
                selection_reason: None,
            },
        ))),
        ProgressEvent::ToolCallCompleted {
            item_id,
            tool_call_id,
            name,
            arguments_summary,
            result_summary,
        } => Some(AppEvent::ItemCompleted(TimelineItem::ToolCall(
            ToolCallItem {
                header: timeline_header(*item_id, ItemStatus::Completed, None),
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                arguments_summary: arguments_summary.clone(),
                result_summary: Some(result_summary.clone()),
                selection_reason: None,
            },
        ))),
        ProgressEvent::UsageUpdated {
            input_tokens,
            output_tokens,
            cost_usd,
        } => Some(AppEvent::UsageUpdated(fixtrace_presenter::present_usage(
            fixtrace_presenter::UsagePresentationInput {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                total_cost_usd: *cost_usd,
                token_limit: config.budget.max_total_tokens,
                cost_limit_usd: config.budget.max_cost_usd,
                exact: true,
            },
        ))),
        ProgressEvent::Cancelled => Some(notice("cancelled", "Operation cancelled".to_owned())),
        ProgressEvent::Finished => Some(notice("finished", "Operation finished".to_owned())),
    }
}

fn timeline_header(id: Uuid, status: ItemStatus, entity: Option<EntityRef>) -> TimelineItemHeader {
    TimelineItemHeader {
        id,
        status,
        started_at: Utc::now(),
        completed_at: (status == ItemStatus::Completed).then(Utc::now),
        parent_id: None,
        artifacts: Vec::new(),
        entities: entity.into_iter().collect(),
    }
}

fn task_command(session_id: Option<Uuid>, input: TaskInput) -> Result<AppCommand, AppErrorView> {
    match input {
        TaskInput::AnalyzeMinimalTrace { no_llm } => Ok(AppCommand::AnalyzeSession {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "analysis requires a session")
            })?,
            no_llm,
            prompt: None,
        }),
        TaskInput::AgentTurn { prompt } => Ok(AppCommand::AnalyzeSession {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "agent turn requires a session")
            })?,
            no_llm: false,
            prompt: Some(prompt),
        }),
        TaskInput::VerifyBaseline => Ok(AppCommand::RunCandidate {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "verification requires a session")
            })?,
            action_ids: Some(Vec::new()),
            repetitions: None,
        }),
        TaskInput::ReplayFullTrace => Ok(AppCommand::RunCandidate {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "replay requires a session")
            })?,
            action_ids: None,
            repetitions: None,
        }),
        TaskInput::RepeatTrial { trial_id } => Ok(AppCommand::RepeatTrial {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "trial repeat requires a session")
            })?,
            trial_id,
            repetitions: None,
        }),
        TaskInput::GenerateDiagnosis { prompt } => Ok(AppCommand::AnalyzeSession {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "diagnosis requires a session")
            })?,
            no_llm: false,
            prompt,
        }),
        TaskInput::RecordTrace { line } => Ok(AppCommand::RecordLine {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "recording requires a session")
            })?,
            line,
        }),
        TaskInput::ExportSession { output } => Ok(AppCommand::ExportSession {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "export requires a session")
            })?,
            output,
        }),
        TaskInput::Demo { no_llm } => Ok(AppCommand::RunDemo { no_llm }),
    }
}

fn task_title(input: &TaskInput) -> &'static str {
    match input {
        TaskInput::AgentTurn { .. } => "Agent turn",
        TaskInput::RecordTrace { .. } => "Record repair action",
        TaskInput::VerifyBaseline => "Verify baseline",
        TaskInput::ReplayFullTrace => "Replay full trace",
        TaskInput::AnalyzeMinimalTrace { .. } => "Analyze minimal trace",
        TaskInput::RepeatTrial { .. } => "Repeat trial",
        TaskInput::GenerateDiagnosis { .. } => "Generate diagnosis",
        TaskInput::ExportSession { .. } => "Export session",
        TaskInput::Demo { .. } => "Run demo",
    }
}

fn task_supports_steer(input: &TaskInput) -> bool {
    matches!(
        input,
        TaskInput::AgentTurn { .. }
            | TaskInput::AnalyzeMinimalTrace { no_llm: false }
            | TaskInput::GenerateDiagnosis { .. }
    )
}

fn paginate<T>(
    items: Vec<T>,
    page: &fixtrace_protocol::PageRequest,
) -> Result<(Vec<T>, PageInfo), AppErrorView> {
    let start = page
        .cursor
        .as_deref()
        .unwrap_or("0")
        .parse::<usize>()
        .map_err(|_| AppErrorView::new(ErrorCode::InvalidRequest, "invalid page cursor"))?;
    let limit = usize::try_from(page.validated_limit()).expect("page limit fits usize");
    if start > items.len() {
        return Err(AppErrorView::new(
            ErrorCode::InvalidRequest,
            "page cursor is beyond the result set",
        ));
    }
    let end = start.saturating_add(limit).min(items.len());
    let has_more = end < items.len();
    let mut tail = items
        .into_iter()
        .skip(start)
        .take(end - start)
        .collect::<Vec<_>>();
    let result = std::mem::take(&mut tail);
    Ok((
        result,
        PageInfo {
            next_cursor: has_more.then(|| end.to_string()),
            has_more,
        },
    ))
}

#[derive(Debug)]
struct ResolvedArtifact {
    summary: ArtifactSummary,
    path: PathBuf,
}

fn insert_artifact_ref(
    artifacts: &mut BTreeMap<PathBuf, ArtifactRef>,
    reference: Option<&ArtifactRef>,
) {
    if let Some(reference) = reference {
        artifacts
            .entry(reference.path.clone())
            .or_insert_with(|| reference.clone());
    }
}

fn safe_artifact_relative_path(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(first)) if first == "artifacts")
        && components.all(|component| matches!(component, Component::Normal(_)))
}

fn artifact_id(session_id: Uuid, path: &Path) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(session_id.as_bytes());
    digest.update([0]);
    digest.update(path.as_os_str().as_encoded_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn valid_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn connection_test_failure(started: Instant, message: impl Into<String>) -> ConnectionTestResponse {
    ConnectionTestResponse {
        ok: false,
        model: None,
        latency_ms: elapsed_millis(started),
        message: message.into(),
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn public_config(config: &FixTraceConfig) -> PublicConfigSummary {
    PublicConfigSummary {
        provider: config.model.provider.clone(),
        endpoint: config.model.endpoint.clone(),
        api_key_env: config.model.api_key_env.clone(),
        model: config.model.model.clone(),
        api_style: config.model.api_style.clone(),
        context_length: config.model.context_length,
        reasoning_mode: config.model.reasoning_mode.clone(),
        max_agent_steps: u64::try_from(config.model.max_agent_steps).unwrap_or(u64::MAX),
        input_per_million_usd: config.pricing.input_per_million_usd,
        output_per_million_usd: config.pricing.output_per_million_usd,
        max_total_tokens: config.budget.max_total_tokens,
        max_cost_usd: config.budget.max_cost_usd,
        replay_repetitions: config.replay.repetitions,
        oracle_timeout_secs: config.replay.oracle_timeout_secs,
        include_target: config.replay.include_target,
        has_api_key: std::env::var(&config.model.api_key_env)
            .is_ok_and(|value| !value.trim().is_empty()),
        approval_policy: config.approval.policy.clone(),
    }
}

fn snapshot_diff_view(
    before: &SnapshotManifest,
    after: &SnapshotManifest,
    baseline_root: &Path,
    worktree_root: &Path,
) -> DiffView {
    const MAX_DIFF_FILES: usize = 500;
    let delta = before.diff(after);
    let mut files = BTreeMap::<std::path::PathBuf, Vec<&'static str>>::new();
    for path in delta.created {
        files.entry(path).or_default().push("created");
    }
    for path in delta.deleted {
        files.entry(path).or_default().push("deleted");
    }
    for path in delta.content_modified {
        files.entry(path).or_default().push("content_modified");
    }
    for path in delta.permission_modified {
        files.entry(path).or_default().push("permission_modified");
    }
    let truncated = files.len() > MAX_DIFF_FILES;
    DiffView {
        files: files
            .into_iter()
            .take(MAX_DIFF_FILES)
            .map(|(path, kinds)| DiffFileView {
                path: path.to_string_lossy().into_owned(),
                change_kind: kinds.join("+"),
                unified_diff: unified_text_diff(baseline_root, worktree_root, &path),
                artifact_id: None,
            })
            .collect(),
        truncated,
    }
}

fn unified_text_diff(
    baseline_root: &Path,
    worktree_root: &Path,
    relative: &Path,
) -> Option<String> {
    const MAX_INPUT_BYTES: usize = 256 * 1_024;
    const MAX_OUTPUT_BYTES: usize = 512 * 1_024;
    let read_text = |root: &Path| match fs::read(root.join(relative)) {
        Ok(bytes) if bytes.len() <= MAX_INPUT_BYTES => String::from_utf8(bytes).ok(),
        Ok(_) | Err(_) => None,
    };
    let before = read_text(baseline_root).unwrap_or_default();
    let after = read_text(worktree_root).unwrap_or_default();
    if before.is_empty() && after.is_empty() {
        return None;
    }
    let label = relative.to_string_lossy();
    let rendered = similar::TextDiff::from_lines(&before, &after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{label}"), &format!("b/{label}"))
        .to_string();
    if rendered.len() <= MAX_OUTPUT_BYTES {
        Some(rendered)
    } else {
        let boundary = rendered
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= MAX_OUTPUT_BYTES)
            .last()
            .unwrap_or(0);
        Some(format!("{}\n… diff truncated …", &rendered[..boundary]))
    }
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        supports_streaming: true,
        supports_approvals: true,
        supports_diff: true,
        supports_graph: true,
        supports_artifacts: true,
        supports_event_catch_up: true,
        supports_multiple_clients: true,
        max_page_limit: fixtrace_protocol::MAX_PAGE_LIMIT,
        max_artifact_read_bytes: fixtrace_protocol::MAX_ARTIFACT_READ_BYTES,
    }
}

fn config_value_text(value: ConfigValue) -> String {
    match value {
        ConfigValue::String(value) => value,
        ConfigValue::Integer(value) => value.to_string(),
        ConfigValue::Float(value) => value.to_string(),
        ConfigValue::Boolean(value) => value.to_string(),
    }
}

fn report_from_history(values: &[serde_json::Value]) -> Option<MinimizationReport> {
    values
        .iter()
        .rev()
        .find_map(|value| serde_json::from_value(value.clone()).ok())
}

fn diagnosis_from_history(values: &[serde_json::Value]) -> Option<Diagnosis> {
    values
        .iter()
        .rev()
        .find_map(|value| serde_json::from_value(value.clone()).ok())
}

fn usage_from_history(values: &[serde_json::Value]) -> CoreUsageSummary {
    values
        .iter()
        .rev()
        .filter_map(|value| value.get("summary"))
        .find_map(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn app_error_view(error: AppError) -> AppErrorView {
    match error {
        AppError::SessionNotFound(_) => {
            AppErrorView::new(ErrorCode::NotFound, "session was not found")
        }
        AppError::UnsafePath { .. }
        | AppError::BaselineMismatch { .. }
        | AppError::WorkingDirectoryMismatch { .. }
        | AppError::NonReplayable { .. } => AppErrorView::new(
            ErrorCode::SandboxDenied,
            "sandbox policy denied the operation",
        ),
        AppError::InvalidConfig(message)
        | AppError::Process(message)
        | AppError::Minimization(message)
        | AppError::Agent(message)
        | AppError::Llm(message) => AppErrorView::new(ErrorCode::InvalidRequest, message),
        AppError::ServiceUnavailable => {
            let mut error = AppErrorView::new(ErrorCode::Internal, "App Service is unavailable");
            error.retryable = true;
            error
        }
        other => {
            tracing::error!("protocol operation failed: {other}");
            AppErrorView::new(
                ErrorCode::Internal,
                "FixTrace operation failed; consult the server log",
            )
        }
    }
}

fn store_error_view(error: fixtrace_store::StoreError) -> AppErrorView {
    match error {
        fixtrace_store::StoreError::TaskNotFound(_) => {
            AppErrorView::new(ErrorCode::NotFound, "task was not found")
        }
        fixtrace_store::StoreError::InvalidTaskTransition { .. } => {
            AppErrorView::new(ErrorCode::InvalidTransition, error.to_string())
        }
        fixtrace_store::StoreError::ApprovalNotFound(_) => {
            AppErrorView::new(ErrorCode::NotFound, "approval was not found")
        }
        fixtrace_store::StoreError::ApprovalAlreadyResolved(_) => AppErrorView::new(
            ErrorCode::ApprovalResolved,
            "approval has already been resolved",
        ),
        other => {
            tracing::error!("protocol store operation failed: {other}");
            AppErrorView::new(ErrorCode::Internal, "event store operation failed")
        }
    }
}

fn invariant_error() -> AppErrorView {
    AppErrorView::new(ErrorCode::Internal, "App Service response invariant failed")
}
