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

use stvdy_core::citations::{Citation, strip_unknown};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role, StreamEvent};

use crate::auth::extract::VerifiedUser;
use crate::chat::{prompt, retrieve, sse, web};
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

    // Every check that can fail runs before the spend and before anything is
    // written, so a refused request leaves no credit spent and no message behind.
    // The provider check is here for the same reason: a server with no
    // `LLM_BASE_URL` can never succeed on retry, so charging for it would take a
    // credit for a request that could not have worked.
    let provider = state.llm.clone().ok_or_else(|| {
        AppError::Provider(
            "no model provider is configured on this server (LLM_BASE_URL is unset)".to_string(),
        )
    })?;

    let chat_cap = credits::daily_chat_cap(&state, &user);
    let search_cap = credits::daily_search_cap(&state, &user);
    let cost = credits::chat_message_cost(&state);

    let chunks = retrieve::retrieve(
        &state,
        user.user_id,
        workspace_id,
        &message,
        &body.source_ids,
    )
    .await?;

    // The web arm runs before the spend, because its result decides whether a
    // search is charged: a cache hit costs nothing (A40), and a query every tier
    // failed is not an error, so the turn proceeds without web context rather than
    // failing. Either way the user's own sources are already in hand.
    // The composite does not return a `Result`, and that is deliberate: a tier
    // failing is recorded in `unresponsive` rather than propagated, because a
    // query every tier failed is not an error. The turn proceeds without web
    // context and answers from the user's own sources, which is strictly better
    // than an error for the user.
    let web = match (&state.search, body.use_web) {
        (Some(search), true) => web::build(
            search
                .search(&message, state.config.search.max_results)
                .await,
        ),
        (None, true) => {
            tracing::warn!("use_web was requested but no search provider is configured");
            web::WebContext::empty()
        }
        (_, false) => web::WebContext::empty(),
    };

    let composed = prompt::compose(
        &state,
        &user,
        &workspace_name,
        &chunks,
        &web.prompt_hits,
        body.use_web,
        body.study_mode,
    )?;

    let labels = composed.labels.clone();
    let prompt_version = composed.prompt_version.clone();

    // A search is charged only when it actually called a provider. A cache hit
    // writes its usage row below without a ledger entry (A40), because re-billing
    // a repeated query would burn the user's daily allowance on a question already
    // answered.
    let search_cost = if body.use_web && !web.cache_hit && !web.is_empty() {
        credits::search_cost(&state)
    } else {
        0
    };
    let total_cost = cost + search_cost;

    // The spend comes after every read that can fail, and immediately before the
    // provider is called: retrieval and composition are a database read and a
    // pure function, so doing them first costs nothing and shrinks the window in
    // which a credit can be spent on a request that never reaches a model.
    //
    // The daily cap is enforced inside this call, under the same lock, because a
    // separate check and spend can interleave and let two requests through on the
    // last slot.
    let spent = credits::spend(
        &state,
        user.user_id,
        user.plan(),
        ACTION,
        total_cost,
        // Both caps, in this one transaction. The search cap is only armed when a
        // search actually ran and was not a cache hit, because that is the case it
        // governs: an uncached search consumes `web_searches_per_day`, and a repeat
        // of a cached question does not (A40).
        [
            Some(chat_cap),
            if search_cost > 0 {
                Some(search_cap)
            } else {
                None
            },
        ],
    )
    .await?;
    let remaining = spent.remaining;
    let usage_event_id = spent.usage_event_id;

    // Two related-but-distinct facts, deliberately stored separately:
    //
    //  * the *user's* row records what the turn asked for, because that is what the
    //    user's message is;
    //  * the *assistant's* row and the `done` event record what the answer actually
    //    used, because a toggle-on turn where every tier returned nothing used no
    //    web context and claiming otherwise would misattribute the answer.
    //
    // They differ in exactly one case, which is why it is worth being explicit
    // rather than sharing one value: a search that returned nothing.
    let answered_with_web = !web.is_empty();

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

    // The search's own row, written whether or not it cost anything: `cache_hit`
    // is what makes the cache visible in the margin report, and `search_tier` plus
    // `unresponsive_engines` is the A3 health signal that turns silent degradation
    // of the free path into something an operator can see.
    if body.use_web
        && let Err(error) = credits::record_search(
            &state.pool,
            user.user_id,
            search_cost,
            web.answered_by.as_deref(),
            &web.unresponsive,
            web.cache_hit,
        )
        .await
    {
        // Not fatal: the answer is already being produced and the credit is
        // already spent. Losing the telemetry affects the report, not the user.
        tracing::error!(%error, "recording the search usage failed");
    }

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
        // How much of `answer` has already been shown. Only complete markers are
        // emitted, so a fabricated one is dropped before the user sees it rather
        // than disappearing when the persisted copy replaces the live turn.
        let mut emitted = 0usize;

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

                    // Emit only up to the last complete marker. The hold-back is
                    // at most a marker's length, so this does not delay ordinary
                    // prose.
                    let safe = stvdy_core::citations::streamable_end(&answer);
                    if safe > emitted {
                        let slice = &answer[emitted..safe];
                        let visible = stvdy_core::citations::strip_unknown(slice, &labels);
                        if !visible.is_empty() {
                            yield Ok(sse::token(&visible));
                        }
                        emitted = safe;
                    }
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

        // Flush whatever the hold-back was keeping, so the last marker of an
        // answer still reaches the client.
        if emitted < answer.len() {
            let visible = stvdy_core::citations::strip_unknown(&answer[emitted..], &labels);
            if !visible.is_empty() {
                yield Ok(sse::token(&visible));
            }
        }

        // The citations are resolved from the labels the server actually sent, so
        // a fabricated one cannot survive. The text is stripped in the same pass,
        // so the user never sees a citation they cannot open.
        let resolved = stvdy_core::citations::resolve(&answer, &labels);
        let cleaned = strip_unknown(&answer, &labels);
        // Counted, not inferred from `resolved.len()`: that is the number of
        // citations that survived, so logging it as "dropped" would report the
        // inverse of the citation-hallucination rate.
        let unknown = stvdy_core::citations::unknown_count(&answer, &labels);
        if unknown > 0 {
            tracing::info!(
                dropped = unknown,
                kept = resolved.len(),
                "the model cited labels that were not in the context"
            );
        }

        if let Some(reported) = &usage
            && let Err(error) = credits::record_provider_usage(
                &state_for_stream.pool,
                usage_event_id,
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
        //
        // Every exit path must emit exactly one terminator. Returning here without
        // one would leave a client waiting for a `done` that never arrives, since
        // the contract makes `done` the end of the turn.
        if cleaned.trim().is_empty() {
            tracing::warn!(%user_message_id, "the model returned an empty answer");
            yield Ok(sse::error(&AppError::Provider(
                "the model returned an empty answer".to_string(),
            )));
        } else {
            match insert_message(
                &state_for_stream,
                user_id,
                workspace_id,
                "assistant",
                &cleaned,
                &resolved,
                answered_with_web,
            )
            .await
            {
                Ok(message_id) => {
                    yield Ok(sse::citations(&resolved));
                    yield Ok(sse::usage(total_cost, remaining));
                    yield Ok(sse::done(message_id, answered_with_web));
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
