use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::{diagnosis::Diagnosis, tools::AgentToolExecutor},
    config::FixTraceConfig,
    error::AppError,
    history::database::HistoryDatabase,
    llm::{
        provider::{ChatMessage, LlmProvider, LlmRequest, MessageRole},
        usage::UsageSummary,
    },
    progress::{ProgressEvent, ProgressSender},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStopReason {
    Completed,
    MaxSteps,
    Cancelled,
    Budget,
    ContextLimit,
    ToolFailureLimit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentRunResult {
    pub diagnosis: Option<Diagnosis>,
    pub usage: UsageSummary,
    pub steps: usize,
    pub stop_reason: AgentStopReason,
}

#[derive(Clone, Copy)]
pub struct AgentHistory<'a> {
    pub database: Option<&'a HistoryDatabase>,
    pub session_id: Option<Uuid>,
}

impl AgentHistory<'_> {
    pub const fn none() -> Self {
        Self {
            database: None,
            session_id: None,
        }
    }

    fn record(&self, table: &'static str, value: &Value) -> Result<(), AppError> {
        if let Some(database) = self.database {
            database.insert_json(table, self.session_id, value)?;
        }
        Ok(())
    }
}

pub async fn run_agent<P, T>(
    provider: &P,
    tools: &mut T,
    config: &FixTraceConfig,
    cancellation: CancellationToken,
    progress: Option<&ProgressSender>,
    history: AgentHistory<'_>,
) -> Result<AgentRunResult, AppError>
where
    P: LlmProvider,
    T: AgentToolExecutor,
{
    run_agent_with_prompt(
        provider,
        tools,
        config,
        cancellation,
        progress,
        history,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_agent_with_prompt<P, T>(
    provider: &P,
    tools: &mut T,
    config: &FixTraceConfig,
    cancellation: CancellationToken,
    progress: Option<&ProgressSender>,
    history: AgentHistory<'_>,
    user_prompt: Option<&str>,
) -> Result<AgentRunResult, AppError>
where
    P: LlmProvider,
    T: AgentToolExecutor,
{
    run_agent_with_prompt_and_steering(
        provider,
        tools,
        config,
        cancellation,
        progress,
        history,
        user_prompt,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_agent_with_prompt_and_steering<P, T>(
    provider: &P,
    tools: &mut T,
    config: &FixTraceConfig,
    cancellation: CancellationToken,
    progress: Option<&ProgressSender>,
    history: AgentHistory<'_>,
    user_prompt: Option<&str>,
    mut steering: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
) -> Result<AgentRunResult, AppError>
where
    P: LlmProvider,
    T: AgentToolExecutor,
{
    let mut messages = initial_messages(user_prompt);
    for message in &messages {
        history.record("messages", &serde_json::to_value(message)?)?;
    }
    let mut usage = UsageSummary::default();
    let mut consecutive_tool_failures = 0_usize;

    for step in 1..=config.model.max_agent_steps {
        if cancellation.is_cancelled() {
            return Ok(stopped(
                usage,
                step.saturating_sub(1),
                AgentStopReason::Cancelled,
            ));
        }
        if approximate_tokens(&messages) > config.model.context_length {
            return Ok(stopped(
                usage,
                step.saturating_sub(1),
                AgentStopReason::ContextLimit,
            ));
        }
        if let Some(progress) = progress {
            progress.emit(ProgressEvent::AgentStepStarted { step });
        }
        let response_result = loop {
            let completion = provider.complete(
                LlmRequest {
                    messages: messages.clone(),
                    tools: tools.definitions(),
                },
                cancellation.clone(),
            );
            tokio::pin!(completion);
            if let Some(receiver) = steering.as_mut() {
                tokio::select! {
                    biased;
                    message = receiver.recv() => {
                        if let Some(message) = message {
                            let message = ChatMessage::text(
                                MessageRole::User,
                                format!("User steering for the active analysis:\n{message}"),
                            );
                            history.record("messages", &serde_json::to_value(&message)?)?;
                            messages.push(message);
                            continue;
                        }
                        steering = None;
                        break completion.await;
                    }
                    result = &mut completion => break result,
                }
            } else {
                break completion.await;
            }
        };
        let response = match response_result {
            Err(_) if cancellation.is_cancelled() => {
                return Ok(stopped(usage, step, AgentStopReason::Cancelled));
            }
            result => result?,
        };
        usage.record(response.usage, &config.pricing);
        history.record(
            "api_usage",
            &json!({
                "request_id": response.request_id,
                "model": response.model,
                "observation": response.usage,
                "summary": usage,
            }),
        )?;
        if let Some(progress) = progress {
            progress.emit(ProgressEvent::UsageUpdated {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cost_usd: usage.total_cost_usd,
            });
        }
        if usage.exceeds(&config.budget) {
            if let Some(progress) = progress {
                progress.emit(ProgressEvent::BudgetExceeded {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cost_usd: usage.total_cost_usd,
                });
            }
            return Ok(stopped(usage, step, AgentStopReason::Budget));
        }

        if !response.tool_calls.is_empty() {
            let assistant = ChatMessage {
                role: MessageRole::Assistant,
                content: response.content,
                tool_calls: response.tool_calls,
                tool_call_id: None,
            };
            history.record("messages", &serde_json::to_value(&assistant)?)?;
            let calls = assistant.tool_calls.clone();
            messages.push(assistant);
            for call in calls {
                let timeline_id = Uuid::new_v4();
                if let Some(progress) = progress {
                    progress.emit(ProgressEvent::ToolCallStarted {
                        item_id: timeline_id,
                        tool_call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments_summary: compact_json(&call.arguments),
                    });
                }
                history.record("tool_calls", &serde_json::to_value(&call)?)?;
                let tool_result = tools
                    .execute(&call.name, call.arguments.clone(), &usage)
                    .await;
                let (value, failed) = match tool_result {
                    Ok(value) => (value, false),
                    Err(error) => (json!({"error": error.to_string()}), true),
                };
                if let Some(progress) = progress {
                    progress.emit(ProgressEvent::ToolCallCompleted {
                        item_id: timeline_id,
                        tool_call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments_summary: compact_json(&call.arguments),
                        result_summary: compact_json(&value),
                    });
                }
                consecutive_tool_failures = if failed {
                    consecutive_tool_failures.saturating_add(1)
                } else {
                    0
                };
                let tool_message = ChatMessage {
                    role: MessageRole::Tool,
                    content: Some(serde_json::to_string(&value)?),
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call.id),
                };
                history.record("messages", &serde_json::to_value(&tool_message)?)?;
                messages.push(tool_message);
                if consecutive_tool_failures >= 3 {
                    return Ok(stopped(usage, step, AgentStopReason::ToolFailureLimit));
                }
            }
            continue;
        }

        let content = response.content.ok_or_else(|| {
            AppError::Agent("model returned neither tool calls nor final content".to_owned())
        })?;
        if let Some(progress) = progress {
            let item_id = Uuid::new_v4();
            progress.emit(ProgressEvent::AgentMessageStarted { item_id });
            for chunk in text_chunks(&content, 128) {
                progress.emit(ProgressEvent::AgentTextDelta {
                    item_id,
                    text_delta: chunk,
                });
            }
            progress.emit(ProgressEvent::AgentMessageCompleted {
                item_id,
                text: content.clone(),
            });
        }
        let assistant = ChatMessage::text(MessageRole::Assistant, content.clone());
        history.record("messages", &serde_json::to_value(&assistant)?)?;
        let mut diagnosis = parse_diagnosis(&content)?;
        diagnosis.validate()?;
        tools.validate_diagnosis(&diagnosis)?;
        diagnosis.usage = usage.clone();
        history.record("diagnoses", &serde_json::to_value(&diagnosis)?)?;
        return Ok(AgentRunResult {
            diagnosis: Some(diagnosis),
            usage,
            steps: step,
            stop_reason: AgentStopReason::Completed,
        });
    }

    Ok(stopped(
        usage,
        config.model.max_agent_steps,
        AgentStopReason::MaxSteps,
    ))
}

fn initial_messages(user_prompt: Option<&str>) -> Vec<ChatMessage> {
    let mut messages = vec![
        ChatMessage::text(
            MessageRole::System,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/fixtrace_agent.md"
            )),
        ),
        ChatMessage::text(
            MessageRole::User,
            "Analyze the verified repair trace. Use tools before concluding. Return only one bare Diagnosis JSON object (no Markdown fence or prose) with fields: statement (string), minimal_action_ids (integer array), evidence (array of {claim, classification, action_ids, trial_ids}), limitations (string array), and usage (object; FixTrace replaces it with measured API usage). Valid classifications: necessary, removable, uncertain, untested, non_replayable.",
        ),
    ];
    if let Some(prompt) = user_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        messages.push(ChatMessage::text(
            MessageRole::User,
            format!(
                "User focus for this analysis:\n{prompt}\n\nStill return only the required evidence-bound Diagnosis JSON."
            ),
        ));
    }
    messages
}

fn compact_json(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_owned());
    let mut chars = text.chars();
    let preview: String = chars.by_ref().take(512).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn text_chunks(text: &str, size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for character in text.chars() {
        current.push(character);
        count += 1;
        if count >= size {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn parse_diagnosis(content: &str) -> Result<Diagnosis, AppError> {
    let source = strip_json_fence(content).unwrap_or_else(|| content.trim());
    let mut value: Value = serde_json::from_str(source).map_err(|error| {
        AppError::Agent(format!(
            "final diagnosis is not valid Diagnosis JSON: {error}"
        ))
    })?;

    if let Some(object) = value.as_object_mut() {
        // Usage is security- and budget-sensitive accounting. Never trust the
        // model's self-report; run_agent replaces it with provider observations.
        object.remove("usage");
        if let Some(limitations) = object.get_mut("limitations")
            && limitations.is_string()
        {
            let limitation = limitations.take();
            *limitations = Value::Array(vec![limitation]);
        }
    }

    serde_json::from_value(value).map_err(|error| {
        AppError::Agent(format!(
            "final diagnosis is not valid Diagnosis JSON: {error}"
        ))
    })
}

fn strip_json_fence(content: &str) -> Option<&str> {
    let fenced = content.trim().strip_prefix("```")?;
    let fenced = fenced
        .strip_prefix("json")
        .or_else(|| fenced.strip_prefix("JSON"))
        .unwrap_or(fenced);
    let fenced = fenced
        .strip_prefix("\r\n")
        .or_else(|| fenced.strip_prefix('\n'))?;
    fenced.strip_suffix("```").map(str::trim)
}

fn stopped(usage: UsageSummary, steps: usize, stop_reason: AgentStopReason) -> AgentRunResult {
    AgentRunResult {
        diagnosis: None,
        usage,
        steps,
        stop_reason,
    }
}

fn approximate_tokens(messages: &[ChatMessage]) -> u64 {
    let bytes = serde_json::to_vec(messages).map_or(0, |value| value.len());
    u64::try_from(bytes.div_ceil(4)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use crate::{
        agent::{
            diagnosis::{Diagnosis, EvidenceClaim, EvidenceClassification},
            tools::AgentToolExecutor,
        },
        config::FixTraceConfig,
        domain::{
            session::{SessionRecord, SessionStatus},
            snapshot::SnapshotManifest,
        },
        error::AppError,
        history::database::HistoryDatabase,
        llm::{
            mock::MockProvider,
            provider::{LlmResponse, ToolCall, ToolDefinition},
            usage::{Usage, UsageObservation, UsageSummary},
        },
        progress::{ProgressEvent, ProgressSender},
        replay::oracle::OracleSpec,
    };

    use super::{
        AgentHistory, AgentStopReason, parse_diagnosis, run_agent,
        run_agent_with_prompt_and_steering,
    };

    struct FakeTools;

    #[async_trait]
    impl AgentToolExecutor for FakeTools {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "get_session_summary".to_owned(),
                description: "test summary".to_owned(),
                parameters: json!({"type":"object"}),
            }]
        }

        async fn execute(
            &mut self,
            name: &str,
            _arguments: Value,
            _usage: &UsageSummary,
        ) -> Result<Value, AppError> {
            if name == "get_session_summary" {
                Ok(json!({"minimal_action_ids":[1]}))
            } else {
                Err(AppError::Agent("unexpected test tool".to_owned()))
            }
        }
    }

    #[tokio::test]
    async fn mock_tool_call_completes_agent_loop_and_persists_history() {
        let trial_id = Uuid::new_v4();
        let diagnosis = Diagnosis {
            statement: "dependency-constrained 1-minimal sufficient repair trace".to_owned(),
            minimal_action_ids: vec![1],
            evidence: vec![EvidenceClaim {
                claim: "Action 1 is retained by verified ablation".to_owned(),
                classification: EvidenceClassification::Necessary,
                action_ids: vec![1],
                trial_ids: vec![trial_id],
            }],
            limitations: vec!["test fixture".to_owned()],
            usage: UsageSummary::default(),
        };
        let provider = MockProvider::new([
            LlmResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".to_owned(),
                    name: "get_session_summary".to_owned(),
                    arguments: json!({}),
                }],
                usage: known_usage(),
                request_id: Some("request-1".to_owned()),
                model: Some("mock".to_owned()),
            },
            LlmResponse {
                content: Some(serde_json::to_string(&diagnosis).expect("diagnosis should encode")),
                tool_calls: Vec::new(),
                usage: known_usage(),
                request_id: Some("request-2".to_owned()),
                model: Some("mock".to_owned()),
            },
        ]);
        let temp = tempdir().expect("temporary directory should be created");
        let database = HistoryDatabase::open(temp.path().join("history.sqlite3"))
            .expect("history database should open");
        let session = test_session(temp.path().to_path_buf());
        database
            .save_session(&session)
            .expect("session should save");
        let mut tools = FakeTools;
        let (progress, mut progress_events) = ProgressSender::channel(64);

        let result = run_agent(
            &provider,
            &mut tools,
            &FixTraceConfig::default(),
            CancellationToken::new(),
            Some(&progress),
            AgentHistory {
                database: Some(&database),
                session_id: Some(session.id),
            },
        )
        .await
        .expect("agent loop should finish");

        assert_eq!(result.stop_reason, AgentStopReason::Completed);
        assert_eq!(
            result
                .diagnosis
                .expect("diagnosis should exist")
                .minimal_action_ids,
            [1]
        );
        assert_eq!(
            database
                .load_json("api_usage", session.id)
                .expect("usage should load")
                .len(),
            2
        );
        assert_eq!(
            database
                .load_json("tool_calls", session.id)
                .expect("tools should load")
                .len(),
            1
        );
        assert!(
            database
                .load_json("messages", session.id)
                .expect("messages should load")
                .len()
                >= 5
        );
        let mut observed = Vec::new();
        while let Ok(event) = progress_events.try_recv() {
            observed.push(event);
        }
        assert!(
            observed
                .iter()
                .any(|event| matches!(event, ProgressEvent::ToolCallStarted { name, .. } if name == "get_session_summary"))
        );
        assert!(
            observed
                .iter()
                .any(|event| matches!(event, ProgressEvent::ToolCallCompleted { name, .. } if name == "get_session_summary"))
        );
        assert!(
            observed
                .iter()
                .any(|event| matches!(event, ProgressEvent::AgentTextDelta { text_delta, .. } if !text_delta.is_empty()))
        );
        assert!(
            observed
                .iter()
                .any(|event| matches!(event, ProgressEvent::AgentMessageCompleted { .. }))
        );
    }

    #[tokio::test]
    async fn queued_steering_is_added_without_consuming_an_agent_step() {
        let trial_id = Uuid::new_v4();
        let diagnosis = Diagnosis {
            statement: "steered verified diagnosis".to_owned(),
            minimal_action_ids: vec![1],
            evidence: vec![EvidenceClaim {
                claim: "Action 1 is necessary".to_owned(),
                classification: EvidenceClassification::Necessary,
                action_ids: vec![1],
                trial_ids: vec![trial_id],
            }],
            limitations: vec!["test".to_owned()],
            usage: UsageSummary::default(),
        };
        let provider = MockProvider::new([LlmResponse {
            content: Some(serde_json::to_string(&diagnosis).unwrap()),
            tool_calls: Vec::new(),
            usage: known_usage(),
            request_id: Some("steered-request".to_owned()),
            model: Some("mock".to_owned()),
        }]);
        let temp = tempdir().unwrap();
        let database = HistoryDatabase::open(temp.path().join("history.sqlite3")).unwrap();
        let session = test_session(temp.path().to_path_buf());
        database.save_session(&session).unwrap();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send("focus on filesystem evidence".to_owned())
            .unwrap();
        drop(sender);

        let result = run_agent_with_prompt_and_steering(
            &provider,
            &mut FakeTools,
            &FixTraceConfig::default(),
            CancellationToken::new(),
            None,
            AgentHistory {
                database: Some(&database),
                session_id: Some(session.id),
            },
            Some("initial focus"),
            Some(receiver),
        )
        .await
        .unwrap();

        assert_eq!(result.stop_reason, AgentStopReason::Completed);
        assert_eq!(result.steps, 1);
        let messages = database.load_json("messages", session.id).unwrap();
        assert!(messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.contains("focus on filesystem evidence"))
        }));
    }

    #[tokio::test]
    async fn budget_exhaustion_stops_before_tools_or_another_model_call() {
        let provider = MockProvider::new([LlmResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: "must-not-run".to_owned(),
                name: "get_session_summary".to_owned(),
                arguments: json!({}),
            }],
            usage: known_usage(),
            request_id: Some("budget-request".to_owned()),
            model: Some("mock".to_owned()),
        }]);
        let mut config = FixTraceConfig::default();
        config.budget.max_total_tokens = 1;
        let (progress, mut progress_events) = ProgressSender::channel(16);

        let result = run_agent(
            &provider,
            &mut FakeTools,
            &config,
            CancellationToken::new(),
            Some(&progress),
            AgentHistory::none(),
        )
        .await
        .unwrap();

        assert_eq!(result.stop_reason, AgentStopReason::Budget);
        assert_eq!(result.steps, 1);
        assert_eq!(result.usage.total_tokens, 15);
        let mut saw_budget = false;
        while let Ok(event) = progress_events.try_recv() {
            saw_budget |= matches!(event, ProgressEvent::BudgetExceeded { .. });
            assert!(!matches!(event, ProgressEvent::ToolCallStarted { .. }));
        }
        assert!(saw_budget);
    }

    #[test]
    fn parses_fenced_diagnosis_and_uses_runtime_usage_instead_of_model_claims() {
        let trial_id = Uuid::new_v4();
        let content = format!(
            r#"```json
{{
  "statement": "dependency-constrained 1-minimal sufficient repair trace",
  "minimal_action_ids": [1],
  "evidence": [{{
    "claim": "Action 1 is necessary",
    "classification": "necessary",
    "action_ids": [1],
    "trial_ids": ["{trial_id}"]
  }}],
  "limitations": "Scoped to the recorded baseline",
  "usage": {{"calls": 999999}}
}}
```"#
        );

        let diagnosis = parse_diagnosis(&content).expect("fenced diagnosis should parse");

        assert_eq!(diagnosis.limitations, ["Scoped to the recorded baseline"]);
        assert_eq!(diagnosis.usage, UsageSummary::default());
    }

    fn known_usage() -> UsageObservation {
        UsageObservation::Known {
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
            },
        }
    }

    fn test_session(root: PathBuf) -> SessionRecord {
        let now = Utc::now();
        SessionRecord {
            id: Uuid::new_v4(),
            parent_session_id: None,
            archived: false,
            project_name: "agent-test".to_owned(),
            original_project: root.join("project"),
            baseline_path: root.join("baseline"),
            worktree_path: root.join("worktree"),
            oracle: OracleSpec {
                command: "false".to_owned(),
                timeout_ms: 1000,
            },
            baseline_manifest: SnapshotManifest {
                root_hash: "baseline".to_owned(),
                files: Default::default(),
            },
            status: SessionStatus::Analyzed,
            created_at: now,
            updated_at: now,
        }
    }
}
