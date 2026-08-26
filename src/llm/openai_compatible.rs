use std::{env, time::Duration};

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    config::ModelConfig,
    error::AppError,
    llm::{
        provider::{ChatMessage, LlmProvider, LlmRequest, LlmResponse, MessageRole, ToolCall},
        usage::{Usage, UsageObservation},
    },
};

pub struct OpenAiCompatibleProvider {
    client: Client,
    endpoint: String,
    api_key: String,
    model: String,
    reasoning_mode: String,
}

impl OpenAiCompatibleProvider {
    pub fn from_config(config: &ModelConfig) -> Result<Self, AppError> {
        if config.api_style != "chat-completions" {
            return Err(AppError::Llm(format!(
                "unsupported API style `{}`; expected chat-completions",
                config.api_style
            )));
        }
        let api_key = env::var(&config.api_key_env).map_err(|_| {
            AppError::Llm(format!(
                "API key environment variable `{}` is not set",
                config.api_key_env
            ))
        })?;
        let api_key = api_key.trim().to_owned();
        if api_key.is_empty() {
            return Err(AppError::Llm(format!(
                "API key environment variable `{}` is empty",
                config.api_key_env
            )));
        }
        let endpoint = chat_completions_endpoint(&config.endpoint);
        let client = Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|error| AppError::Llm(format!("could not create HTTP client: {error}")))?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            model: config.model.clone(),
            reasoning_mode: config.reasoning_mode.clone(),
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(
        &self,
        request: LlmRequest,
        cancellation: CancellationToken,
    ) -> Result<LlmResponse, AppError> {
        let body = json!({
            "model": self.model,
            "messages": request.messages.iter().map(message_json).collect::<Vec<_>>(),
            "tools": request.tools.iter().map(|tool| json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })).collect::<Vec<_>>(),
            "tool_choice": "auto",
            "reasoning_effort": self.reasoning_mode,
        });
        let pending = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send();
        let response = tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(AppError::Agent("cancelled before the model response".to_owned()));
            }
            response = pending => response.map_err(|error| AppError::Llm(format!("request failed: {error}")))?,
        };
        let header_request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable error body>".to_owned());
            return Err(AppError::Llm(format!(
                "endpoint returned {status}: {}",
                truncate(&body, 2048)
            )));
        }
        let response: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|error| AppError::Llm(format!("invalid response JSON: {error}")))?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Llm("response contained no choices".to_owned()))?;
        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|call| {
                let arguments =
                    serde_json::from_str(&call.function.arguments).map_err(|error| {
                        AppError::Llm(format!(
                            "tool `{}` returned invalid argument JSON: {error}",
                            call.function.name
                        ))
                    })?;
                Ok(ToolCall {
                    id: call.id,
                    name: call.function.name,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let usage =
            response
                .usage
                .map_or(UsageObservation::Unknown, |usage| UsageObservation::Known {
                    usage: Usage {
                        input_tokens: usage.prompt_tokens,
                        output_tokens: usage.completion_tokens,
                    },
                });
        Ok(LlmResponse {
            content: choice.message.content,
            tool_calls,
            usage,
            request_id: header_request_id.or(response.id),
            model: response.model,
        })
    }
}

fn message_json(message: &ChatMessage) -> Value {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    let mut value = json!({
        "role": role,
        "content": message.content,
    });
    if !message.tool_calls.is_empty() {
        value["tool_calls"] = Value::Array(
            message
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_owned()),
                        }
                    })
                })
                .collect(),
        );
    }
    if let Some(tool_call_id) = &message.tool_call_id {
        value["tool_call_id"] = Value::String(tool_call_id.clone());
    }
    value
}

fn chat_completions_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.ends_with("/chat/completions") {
        endpoint.to_owned()
    } else {
        format!("{endpoint}/chat/completions")
    }
}

#[cfg(test)]
mod tests {
    use super::chat_completions_endpoint;

    #[test]
    fn endpoint_whitespace_and_trailing_slashes_are_normalized() {
        assert_eq!(
            chat_completions_endpoint(" https://example.test/v1/ "),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("https://example.test/v1/chat/completions"),
            "https://example.test/v1/chat/completions"
        );
    }
}

fn truncate(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Deserialize)]
struct ApiToolCall {
    id: String,
    function: ApiFunctionCall,
}

#[derive(Deserialize)]
struct ApiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct ApiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}
