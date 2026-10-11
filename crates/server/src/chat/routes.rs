//! `POST /workspaces/:id/chat` and `GET /workspaces/:id/messages`.
//!
//! The order of operations in `chat` is the contract, not an implementation
//! detail: verified email, owned workspace, daily cap, credit spend, persist the
//! user message, retrieve, compose, stream. Every check that can fail therefore
//! fails *before* a credit is spent and before anything is written, so a rejected
//! request leaves no trace and a client can retry it safely.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use futures::stream::Stream;
use serde::Deserialize;
use uuid::Uuid;

use stvdy_core::citations::{Citation, ContextLabels, strip_unknown};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role, StreamEvent};

use crate::auth::extract::VerifiedUser;
use crate::chat::{prompt, retrieve, sse};
use crate::credits;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Low, because a study answer should be grounded rather than inventive. Not
/// zero: a fully deterministic model re-reads the same source identically and
/// produces stilted prose.
const TEMPERATURE: f32 = 0.3;
const MAX_TOKENS: u32 = 1500;

/// The action name used in `usage_events` and the credit ledger, so a cost
/// report can separate chat from generation.
const ACTION: &str = "chat_message";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{id}/chat", post(chat))
        .route("/workspaces/{id}/messages", get(list_messages))
}

#[derive(Debug, Deserialize)]
pub struct ChatBody {
    pub message: String,
    /// Narrows retrieval to the user's selected sources. Absent means all of them.
    #[serde(default)]
    pub source_ids: Vec<Uuid>,
    /// Recorded per message. The search arm itself is Phase 3, so today this
    /// changes the prompt's instruction and is stored on the message rather than
    /// pretending to have searched.
    #[serde(default)]
    pub use_web: bool,
    /// Study mode makes the prompt guide with hints before the answer.
    #[serde(default)]
    pub study_mode: bool,
}

/// The whole reply, for a non-streaming caller and for the tests' convenience.
#[derive(Debug, serde::Serialize)]
pub struct ChatOutcome {
    pub message_id: Uuid,
    pub content: String,
    pub citations: Vec<Citation>,
    pub credits_remaining: i64,
}

/// `POST /workspaces/:id/chat`, answering `text/event-stream`.
pub async fn chat(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<ChatBody>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>> {
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return Err(AppError::Validation("the message is empty".to_string()));
    }
    if message.chars().count() > 8_000 {
        return Err(AppError::Validation(
            "the message is too long (8000 characters)".to_string(),
        ));
    }

    let workspace_name = owned_workspace_name(&state, user.user_id, workspace_id).await?;

    // Cap, then spend, then write: a request that fails any check must leave no
    // credit spent and no message behind.
    credits::enforce_daily_chat_cap(&state, &user).await?;
    let cost = credits::chat_message_cost(&state);
    let remaining = credits::spend(&state.pool, user.user_id, ACTION, cost).await?;

    let user_message_id = insert_message(
        &state,
        user.user_id,
        workspace_id,
        "user",
        &message,
        &[],
        body.use_web,
    )
    .await?;

    let chunks = retrieve::retrieve(
        &state,
        user.user_id,
        workspace_id,
        &message,
        &body.source_ids,
    )
    .await?;

    let composed = prompt::compose(
        &state,
        &user,
        &workspace_name,
        &chunks,
        &[],
        body.use_web,
        body.study_mode,
    )?;

    let labels = composed.labels.clone();
    let prompt_version = composed.prompt_version.clone();
    let provider = state.llm.clone().ok_or_else(|| {
        AppError::Provider(
            "no model provider is configured on this server (LLM_BASE_URL is unset)".to_string(),
        )
    })?;

    // History is deliberately just the prompt plus the user's turn: the prompt
    // carries the sources, and replaying prior turns would send a second copy of
    // the whole context on every request.
    let request = ChatRequest {
        model: String::new(),
        messages: vec![
            ChatMsg {
                role: Role::System,
                content: composed.text,
            },
            ChatMsg {
                role: Role::User,
                content: message,
            },
        ],
        tools: Vec::new(),
        temperature: TEMPERATURE,
        max_tokens: MAX_TOKENS,
        json_mode: false,
    };

    let stream = provider
        .stream(request)
        .await
        .map_err(|error| AppError::Provider(error.to_string()))?;

    let state_for_stream = state.clone();
    let user_id = user.user_id;
    let prompt_version_for_stream = prompt_version.clone();

    let events = async_stream::stream! {
        let mut answer = String::new();
        let mut usage = None;

        tokio::pin!(stream);
        loop {
            let next = tokio::select! {
                next = stream.recv() => next,
                () = tokio::time::sleep(sse::HEARTBEAT) => {
                    yield Ok(sse::heartbeat());
                    continue;
                }
            };

            match next {
                Some(StreamEvent::Token(text)) => {
                    answer.push_str(&text);
                    yield Ok(sse::token(&text));
                }
                Some(StreamEvent::Usage(reported)) => {
                    usage = Some(reported);
                }
                Some(StreamEvent::ToolCall(_)) => {
                    // Tools are Phase 12. A call now would be a model doing
                    // something the server cannot yet honour, so it is dropped
                    // rather than streamed to a client that cannot act on it.
                    tracing::warn!("ignoring a tool call: tools arrive in Phase 12");
                }
                Some(StreamEvent::Done) | None => break,
            }
        }

        // The citations are resolved from the labels the server actually sent, so
        // a fabricated one cannot survive. The text is stripped in the same pass,
        // so the user never sees a citation they cannot open.
        let resolved = stvdy_core::citations::resolve(&answer, &labels);
        let cleaned = strip_unknown(&answer, &labels);
        if cleaned != answer {
            tracing::info!(
                dropped = resolved.len(),
                "the model cited a label that was not in the context"
            );
        }

        if let Some(reported) = &usage
            && let Err(error) = credits::record_provider_usage(
                &state_for_stream.pool,
                user_id,
                ACTION,
                reported.in_tokens,
                reported.out_tokens,
                reported.provider_cost_usd,
                &prompt_version_for_stream,
            )
            .await
        {
            // Not fatal: the answer is already produced and the credit is already
            // spent. Losing the cost detail affects the margin report, not the user.
            tracing::error!(%error, "recording provider usage failed");
        }

        // Persisted after the stream, so a client that disconnects mid-answer
        // leaves a user message and no half-answer. A blank answer is not
        // persisted either: an empty assistant turn is worse than none.
        if cleaned.trim().is_empty() {
            tracing::warn!(%user_message_id, "the model returned an empty answer");
        } else {
            match insert_message(
                &state_for_stream,
                user_id,
                workspace_id,
                "assistant",
                &cleaned,
                &resolved,
                body.use_web,
            )
            .await
            {
                Ok(message_id) => {
                    yield Ok(sse::citations(&resolved));
                    yield Ok(sse::usage(cost, remaining));
                    yield Ok(sse::done(message_id, body.use_web));
                }
                Err(error) => {
                    tracing::error!(%error, "persisting the assistant message failed");
                    yield Ok(sse::error(&error));
                }
            }
        }
    };

    Ok(Sse::new(events))
}

/// Confirms the workspace belongs to the caller and returns its name.
async fn owned_workspace_name(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
) -> AppResult<String> {
    let name: Option<String> = sqlx::query_scalar(
        "select name from workspaces
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the workspace: {error}")))?;

    name.ok_or(AppError::NotFound)
}

pub(crate) async fn insert_message(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
    role: &str,
    content: &str,
    citations: &[Citation],
    used_web: bool,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into messages
           (id, user_id, workspace_id, role, content, citations, used_web)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(user_id)
    .bind(workspace_id)
    .bind(role)
    .bind(content)
    .bind(
        serde_json::to_value(citations).map_err(|error| {
            AppError::Internal(anyhow::anyhow!("serializing citations: {error}"))
        })?,
    )
    .bind(used_web)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing a message: {error}")))?;
    Ok(id)
}

#[derive(Debug, serde::Serialize)]
pub struct MessageView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub role: String,
    pub content: String,
    pub citations: serde_json::Value,
    pub used_web: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Uuid,
    workspace_id: Uuid,
    role: String,
    content: String,
    citations: serde_json::Value,
    used_web: bool,
    created_at: Option<DateTime<Utc>>,
}

/// `GET /workspaces/:id/messages`: the persisted thread, oldest first.
///
/// This is the second-device half of the phase gate: the same account on another
/// client reads what the first one wrote.
pub async fn list_messages(
    State(state): State<AppState>,
    user: crate::auth::extract::AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    // The workspace check is separate from the message query so that a workspace
    // id from another account is a 404 rather than an empty list, which would
    // leak that the id exists but has no messages.
    owned_workspace_name(&state, user.user_id, workspace_id).await?;

    let rows = sqlx::query_as::<_, MessageRow>(
        "select id, workspace_id, role, content, citations, used_web, created_at
           from messages
          where workspace_id = $1 and user_id = $2 and deleted_at is null
          order by created_at asc, id asc",
    )
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing messages: {error}")))?;

    let messages: Vec<MessageView> = rows
        .into_iter()
        .map(|row| MessageView {
            id: row.id,
            workspace_id: row.workspace_id,
            role: row.role,
            content: row.content,
            citations: row.citations,
            used_web: row.used_web,
            created_at: row.created_at,
        })
        .collect();

    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// Exposed for the tests: the label map a request would build, without the
/// streaming half.
pub fn labels_for(chunks: &[retrieve::RetrievedChunk]) -> ContextLabels {
    ContextLabels::new(
        chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| stvdy_core::citations::LabeledChunk {
                label: index as u32 + 1,
                chunk_id: chunk.chunk_id,
                source_id: chunk.source_id,
                page: chunk.page,
            })
            .collect(),
        Vec::new(),
    )
}
