use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::Utc;
use fixtrace_presenter::{SessionViewInput, present_session};
use fixtrace_protocol::{
    ActionListResponse, AgentMessageDelta, AgentMessageItem, AppErrorView, AppEvent, AppRequest,
    AppResponsePayload, ArtifactSummary, ConfigValue, ConnectionTestResponse, DependencyGraphView,
    DiffView, EmptyRequest, EntityKind, EntityRef, ErrorCode, EventBatch, EventEnvelope,
    InitializeRequest, InitializeResponse, ItemDelta, ItemStatus, Notice, NoticeLevel,
    PROTOCOL_VERSION, PageInfo, PublicConfigSummary, ServerCapabilities, SessionListResponse,
    SessionSnapshot, SubscriptionStarted, TaskFailure, TaskInput, TaskProgress, TaskResult,
    TaskStatus, TaskSummary, TimelineItem, TimelineItemHeader, ToolCallItem, TrialItem,
    TrialListResponse, UserMessageItem,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::diagnosis::Diagnosis,
    application::{AppCommand, AppResponse, FixTraceAppService, FixTraceApplication},
    config::FixTraceConfig,
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
            AppRequest::TaskSteer(_) => Err(AppErrorView::new(
                ErrorCode::InvalidTransition,
                "the current task does not accept steering",
            )),
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
            AppRequest::ConfigTestConnection(_) => {
                Ok(AppResponsePayload::ConnectionTest(ConnectionTestResponse {
                    ok: false,
                    model: None,
                    latency_ms: 0,
                    message: "connection testing is enabled in the App Server milestone".to_owned(),
                }))
            }
            AppRequest::ArtifactList(request) => {
                self.session_detail(request.session_id).await?;
                Ok(AppResponsePayload::ArtifactList {
                    artifacts: Vec::<ArtifactSummary>::new(),
                    page: PageInfo {
                        next_cursor: None,
                        has_more: false,
                    },
                })
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
            AppRequest::SessionFork(_)
            | AppRequest::SessionArchive(_)
            | AppRequest::SessionDelete(_)
            | AppRequest::TrialRun(_)
            | AppRequest::TrialRepeat(_)
            | AppRequest::ArtifactRead(_) => Err(AppErrorView::new(
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
                diff: DiffView {
                    files: Vec::new(),
                    truncated: false,
                },
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
            supports_steer: false,
        };
        if let Err(error) = self.event_store.save_task(&task) {
            if let Some(session_id) = session_id
                && let Ok(mut sessions) = self.session_tasks.lock()
            {
                sessions.remove(&session_id);
            }
            return Err(store_error_view(error));
        }
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
        TaskInput::ExportSession { output } => Ok(AppCommand::ExportSession {
            session_id: session_id.ok_or_else(|| {
                AppErrorView::new(ErrorCode::InvalidRequest, "export requires a session")
            })?,
            output,
        }),
        TaskInput::Demo { no_llm } => Ok(AppCommand::RunDemo { no_llm }),
        _ => Err(AppErrorView::new(
            ErrorCode::InvalidRequest,
            "task kind is defined but is not enabled in this milestone",
        )),
    }
}

fn task_title(input: &TaskInput) -> &'static str {
    match input {
        TaskInput::AgentTurn { .. } => "Agent turn",
        TaskInput::RecordTrace => "Record repair trace",
        TaskInput::VerifyBaseline => "Verify baseline",
        TaskInput::ReplayFullTrace => "Replay full trace",
        TaskInput::AnalyzeMinimalTrace { .. } => "Analyze minimal trace",
        TaskInput::RepeatTrial { .. } => "Repeat trial",
        TaskInput::GenerateDiagnosis { .. } => "Generate diagnosis",
        TaskInput::ExportSession { .. } => "Export session",
        TaskInput::Demo { .. } => "Run demo",
    }
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

fn public_config(config: &FixTraceConfig) -> PublicConfigSummary {
    PublicConfigSummary {
        provider: config.model.provider.clone(),
        endpoint: config.model.endpoint.clone(),
        model: config.model.model.clone(),
        api_style: config.model.api_style.clone(),
        context_length: config.model.context_length,
        reasoning_mode: config.model.reasoning_mode.clone(),
        replay_repetitions: config.replay.repetitions,
        oracle_timeout_secs: config.replay.oracle_timeout_secs,
        has_api_key: std::env::var(&config.model.api_key_env)
            .is_ok_and(|value| !value.trim().is_empty()),
        approval_policy: fixtrace_protocol::ApprovalPolicy::AskForOpaque,
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
