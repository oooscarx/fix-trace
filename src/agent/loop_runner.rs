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
    let mut messages = initial_messages();
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
        let response = match provider
            .complete(
                LlmRequest {
                    messages: messages.clone(),
                    tools: tools.definitions(),
                },
                cancellation.clone(),
            )
            .await
        {
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
                history.record("tool_calls", &serde_json::to_value(&call)?)?;
                let tool_result = tools
                    .execute(&call.name, call.arguments.clone(), &usage)
                    .await;
                let (value, failed) = match tool_result {
                    Ok(value) => (value, false),
                    Err(error) => (json!({"error": error.to_string()}), true),
                };
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
        let assistant = ChatMessage::text(MessageRole::Assistant, content.clone());
        history.record("messages", &serde_json::to_value(&assistant)?)?;
        let mut diagnosis: Diagnosis = serde_json::from_str(&content).map_err(|error| {
            AppError::Agent(format!(
                "final diagnosis is not valid Diagnosis JSON: {error}"
            ))
        })?;
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

fn initial_messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage::text(
            MessageRole::System,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/fixtrace_agent.md"
            )),
        ),
        ChatMessage::text(
            MessageRole::User,
            "Analyze the verified repair trace. Use tools before concluding. Return only Diagnosis JSON with fields: statement, minimal_action_ids, evidence[{claim,classification,action_ids,trial_ids}], limitations, usage. Valid classifications: necessary, removable, uncertain, untested, non_replayable.",
        ),
    ]
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
        replay::oracle::OracleSpec,
    };

    use super::{AgentHistory, AgentStopReason, run_agent};

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

        let result = run_agent(
            &provider,
            &mut tools,
            &FixTraceConfig::default(),
            CancellationToken::new(),
            None,
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
