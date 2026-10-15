//! Language-model capability.
//!
//! The chat route, the JSON task modules and (later) the agent all reach a
//! model through this one trait, so a provider swap or a test double is a
//! constructor change rather than a rewrite. `stream` is on the trait from the
//! start because the SSE contract needs token-by-token output, and bolting it
//! on later would fork the trait in two.

use tokio::sync::mpsc;

/// Who authored a chat turn.
///
/// Serialised lowercase so a provider payload reads `"role": "user"`, which is
/// what an OpenAI-compatible endpoint expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One turn handed to the model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMsg {
    pub role: Role,
    pub content: String,
}

/// A tool the model may call. The schema is `serde_json::Value` because the
/// shape belongs to the provider, not to the domain; core never inspects it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Everything a provider needs for one completion.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMsg>,
    pub tools: Vec<ToolSpec>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub json_mode: bool,
}

/// Token counts and, when the provider reports it, the money spent.
///
/// `provider_cost_usd` is optional because a local model has no price and a
/// missing price must not be recorded as a zero cost: `None` means unknown,
/// `Some(0.0)` means free.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
}

/// A finished completion.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

/// A tool call the model asked for. `arguments` stays untyped for the same
/// reason `ToolSpec::parameters` does: validating it is the caller's job.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// One step of a streamed completion.
///
/// `Usage` carries the cost so the SSE `usage` event does not have to be
/// reconstructed from token counts, which loses any provider-reported price.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Token(String),
    /// A fragment of the model's chain-of-thought reasoning, when the provider
    /// exposes one. Not every model sends it; the client shows it only when at
    /// least one fragment arrives.
    Reasoning(String),
    ToolCall(ToolCall),
    Usage(Usage),
    Done,
}

/// Failure modes for a model call.
///
/// `Provider` is transport or upstream and is retryable; `Invalid` is a bad
/// request or configuration and a retry cannot help; `NotConfigured` is a
/// deployment with no model wired in, which must surface as a clear message
/// rather than a panic at first use.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("{0}")]
    Provider(String),
    #[error("{0}")]
    Invalid(String),
    #[error("no provider is configured")]
    NotConfigured,
}

/// The receiving half of a streamed completion.
pub type ChatStream = mpsc::Receiver<StreamEvent>;

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn supports_tools(&self) -> bool;

    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, LlmError>;

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError>;
}
