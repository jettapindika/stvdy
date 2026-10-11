//! The first `LlmProvider`: an OpenAI-compatible `/chat/completions` client
//! (A1). It covers [OI], OpenRouter, Groq, DeepSeek and a local vLLM or Ollama,
//! so a provider is configuration rather than code.
//!
//! Streaming is parsed by hand rather than with an SDK because the SSE frames are
//! simple and an SDK would add a dependency whose version tracks the vendor
//! rather than the API. Two rules the parser must keep: a frame can be split
//! across reads, and `[DONE]` ends the stream even though it is not JSON.

use std::time::Duration;

use futures::StreamExt;
use stvdy_core::traits::llm::{
    ChatRequest, ChatResponse, ChatStream, LlmError, LlmProvider, Role, StreamEvent, ToolCall,
    Usage,
};
use tokio::sync::mpsc;

use crate::config::LlmConfig;

/// A connect timeout is worth setting: a provider that accepts a socket and never
/// answers would otherwise hold a chat request open until the client gives up,
/// having already spent the credit.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct OpenAiCompatLlm {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    /// The default model when a request does not name one.
    default_model: String,
}

impl OpenAiCompatLlm {
    pub fn new(cfg: &LlmConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| anyhow::anyhow!("building the LLM http client: {error}"))?;

        Ok(Self {
            client,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            api_key: cfg.api_key.clone(),
            default_model: cfg.model.clone(),
        })
    }

    fn body(&self, req: &ChatRequest, stream: bool) -> serde_json::Value {
        let messages: Vec<serde_json::Value> = req
            .messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    },
                    "content": m.content,
                })
            })
            .collect();

        let model = if req.model.is_empty() {
            self.default_model.clone()
        } else {
            req.model.clone()
        };

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": stream,
        });

        if req.json_mode {
            body["response_format"] = serde_json::json!({ "type": "json_object" });
        }
        if !req.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(
                req.tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.parameters,
                            }
                        })
                    })
                    .collect(),
            );
        }
        body
    }

    /// The usage block a non-streaming reply carries. Some providers put it under
    /// `usage`, some omit it; a missing one is `None` cost rather than zero,
    /// because zero would understate spend in the margin report (A41).
    fn usage_from(value: &serde_json::Value) -> Usage {
        let usage = value.get("usage");
        Usage {
            in_tokens: usage
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            out_tokens: usage
                .and_then(|u| u.get("completion_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            provider_cost_usd: usage
                .and_then(|u| u.get("cost"))
                .and_then(|v| v.as_f64())
                .or_else(|| {
                    // OpenRouter reports its own cost; a direct provider does not,
                    // so the server estimates from the price table instead
                    // (Phase 7's margin work).
                    value
                        .get("usage")
                        .and_then(|u| u.get("total_cost"))
                        .and_then(|v| v.as_f64())
                }),
        }
    }
}

fn parse_tool_calls(value: &serde_json::Value) -> Vec<ToolCall> {
    value
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let function = call.get("function")?;
                    Some(ToolCall {
                        id: call
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        name: function.get("name")?.as_str()?.to_string(),
                        // Arguments arrive as a JSON *string*, which is the part
                        // that trips an implementation that expects an object.
                        arguments: function
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .and_then(|raw| serde_json::from_str(raw).ok())
                            .unwrap_or(serde_json::Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatLlm {
    fn name(&self) -> &'static str {
        "openai_compat"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&self.body(&req, false))
            .send()
            .await
            .map_err(|error| LlmError::Provider(error.to_string()))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| LlmError::Provider(format!("decoding the reply: {error}")))?;

        if !status.is_success() {
            // The provider's message is the difference between a five-minute fix
            // and an afternoon, so it is surfaced rather than swallowed.
            let reason = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("no error detail");
            return Err(if status.is_client_error() {
                LlmError::Invalid(format!("{status}: {reason}"))
            } else {
                LlmError::Provider(format!("{status}: {reason}"))
            });
        }

        let choice = body
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| LlmError::Provider("the reply had no choices".to_string()))?;

        Ok(ChatResponse {
            text: choice
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            tool_calls: parse_tool_calls(choice.get("message").unwrap_or(&serde_json::Value::Null)),
            usage: Self::usage_from(&body),
        })
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&self.body(&req, true))
            .send()
            .await
            .map_err(|error| LlmError::Provider(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(if status.is_client_error() {
                LlmError::Invalid(format!("{status}: {body}"))
            } else {
                LlmError::Provider(format!("{status}: {body}"))
            });
        }

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut sent_done = false;

            while let Some(next) = stream.next().await {
                let bytes = match next {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let _ = tx
                            .send(StreamEvent::Token(format!(
                                "\n\n[stream interrupted: {error}]"
                            )))
                            .await;
                        break;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // A frame ends at a newline; anything after the last newline is a
                // partial frame and must wait for the next read.
                while let Some(newline) = buffer.find('\n') {
                    let line = buffer[..newline].trim_end_matches('\r').to_string();
                    buffer.drain(..=newline);
                    let Some(payload) = line.strip_prefix("data:") else {
                        // `event:`, `id:`, `:` comments and blank separators.
                        continue;
                    };
                    let payload = payload.trim();
                    if payload == "[DONE]" {
                        sent_done = true;
                        break;
                    }
                    if payload.is_empty() {
                        continue;
                    }

                    let Ok(frame) = serde_json::from_str::<serde_json::Value>(payload) else {
                        // A malformed frame is skipped, not fatal: the rest of the
                        // answer is still worth delivering.
                        tracing::warn!("skipping a malformed stream frame");
                        continue;
                    };

                    if let Some(usage) = frame.get("usage").filter(|u| !u.is_null()) {
                        let _ = tx
                            .send(StreamEvent::Usage(Usage {
                                in_tokens: usage
                                    .get("prompt_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32,
                                out_tokens: usage
                                    .get("completion_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                                provider_cost_usd: usage
                                    .get("cost")
                                    .and_then(|v| v.as_f64())
                                    .or_else(|| usage.get("total_cost").and_then(|v| v.as_f64())),
                            }))
                            .await;
                    }

                    let Some(choice) = frame.get("choices").and_then(|c| c.get(0)) else {
                        continue;
                    };
                    let delta = choice.get("delta").unwrap_or(choice);

                    if let Some(text) = delta.get("content").and_then(|v| v.as_str())
                        && !text.is_empty()
                        && tx.send(StreamEvent::Token(text.to_string())).await.is_err()
                    {
                        // The client went away; stop reading the provider rather
                        // than keep paying for tokens nobody will see.
                        return;
                    }

                    for call in parse_tool_calls(delta) {
                        if tx.send(StreamEvent::ToolCall(call)).await.is_err() {
                            return;
                        }
                    }
                }

                if sent_done {
                    break;
                }
            }

            let _ = tx.send(StreamEvent::Done).await;
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LlmConfig {
        LlmConfig {
            base_url: "http://localhost:9/v1".to_string(),
            api_key: "test".to_string(),
            model: "test-model".to_string(),
        }
    }

    #[test]
    fn the_body_maps_roles_and_omits_empty_optional_fields() {
        let llm = OpenAiCompatLlm::new(&cfg()).expect("client");
        let req = ChatRequest {
            model: String::new(),
            messages: vec![
                stvdy_core::traits::llm::ChatMsg {
                    role: Role::System,
                    content: "be brief".to_string(),
                },
                stvdy_core::traits::llm::ChatMsg {
                    role: Role::User,
                    content: "hi".to_string(),
                },
            ],
            tools: Vec::new(),
            temperature: 0.2,
            max_tokens: 512,
            json_mode: false,
        };

        let body = llm.body(&req, true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        // An empty request model falls back to the configured default rather
        // than sending an empty string the provider would reject.
        assert_eq!(body["model"], "test-model");
        assert!(body.get("response_format").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn tool_arguments_are_parsed_from_their_json_string() {
        let value = serde_json::json!({
            "tool_calls": [{
                "id": "call_1",
                "function": { "name": "create_task", "arguments": "{\"title\":\"read ch 4\"}" }
            }]
        });
        let calls = parse_tool_calls(&value);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_task");
        assert_eq!(calls[0].arguments["title"], "read ch 4");
    }

    #[test]
    fn a_missing_usage_is_unknown_cost_not_zero() {
        let usage = OpenAiCompatLlm::usage_from(&serde_json::json!({ "choices": [] }));
        assert_eq!(usage.in_tokens, 0);
        assert_eq!(
            usage.provider_cost_usd, None,
            "zero would understate spend in the margin report"
        );
    }

    #[test]
    fn a_reported_cost_is_carried_through() {
        let usage = OpenAiCompatLlm::usage_from(&serde_json::json!({
            "usage": { "prompt_tokens": 1200, "completion_tokens": 300, "cost": 0.00119 }
        }));
        assert_eq!(usage.in_tokens, 1200);
        assert_eq!(usage.out_tokens, 300);
        assert_eq!(usage.provider_cost_usd, Some(0.00119));
    }
}
