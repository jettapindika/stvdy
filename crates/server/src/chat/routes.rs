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
use stvdy_core::entitlements::Feature;
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role, StreamEvent};

use crate::auth::extract::VerifiedUser;
use crate::chat::{agent, prompt, retrieve, sse, web};
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

/// How many provider calls one turn may make when tool results keep coming back.
///
/// One is the turn itself; the rest are the rounds a tool result causes. Four is enough for
/// "list my tasks, then tell me what is due, then summarise it" — the deepest chain a single
/// question plausibly needs — and it is a *bound* rather than a policy, because the failure it
/// prevents is unbounded: a model that calls a read tool in every round would spend the
/// student's window, and their `agent_turn` charge is per turn rather than per call (A282), so
/// the cost of a loop is paid in latency and context rather than in credits.
///
/// `MAX_PROPOSALS_PER_TURN` caps the proposals a turn may make (A278); this caps the rounds,
/// because a read loop produces no proposals at all and would otherwise never stop.
const MAX_TOOL_ROUNDS: usize = 4;

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
    /// The per-chat "Answer only from my sources" toggle. When true the prompt
    /// forbids an ungrounded general-knowledge answer; web results, when the user
    /// turned web search on, are still allowed and citable.
    ///
    /// Defaults to `true` when the field is omitted because that is the UI
    /// default: a client that predates the toggle, or a request that drops it,
    /// must behave as grounded ON rather than silently answering from general
    /// knowledge.
    #[serde(default = "sources_only_default")]
    pub sources_only: bool,
}

/// The serde default for [`ChatBody::sources_only`]: grounded ON, the UI default.
fn sources_only_default() -> bool {
    true
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

    // The plan decision, before any read that can fail: a refused turn must not retrieve,
    // must not search and must not reach a model. The daily cap and the balance are checked
    // again inside the spend below, under the advisory lock, because a check here and a
    // spend there can interleave; this call is what makes the refusal cheap and what the
    // route sweep drives.
    //
    // `WebSearch` is deliberately **not** checked here, even when `use_web` is set: whether
    // this turn costs a search is not known until the search has run, because a cache hit
    // costs nothing and must not consume `web_searches_per_day` either (A40). Checking it up
    // front would refuse a repeat of a question the cache can answer, which is exactly the
    // case the cache exists for. Its gate is below, where `search_cost` is known.
    crate::entitlements::require(&state, &user, Feature::ChatMessage).await?;

    let workspace_name = crate::workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    // Every check that can fail runs before the spend and before anything is
    // written, so a refused request leaves no credit spent and no message behind.
    // The provider check is here for the same reason: a server with no
    // `LLM_BASE_URL` can never succeed on retry, so charging for it would take a
    // credit for a request that could not have worked.
    //
    // **BYOK is resolved once, here** (A302). When the account has its own key and its plan
    // grants BYOK, that provider is used and the spend below is skipped: the student paid
    // their own provider, so charging them an allowance for it would be charging twice. The
    // handle is passed down rather than re-resolved, because a request half on one provider
    // and half on another is a bug that shows up as a strange bill.
    let byok = crate::billing::byok::provider_for(&state, &user).await?;
    let using_own_key = byok.is_some();

    let provider = match byok {
        Some(provider) => provider,
        None => state.llm.clone().ok_or_else(|| {
            AppError::Provider(
                "no model provider is configured on this server (LLM_BASE_URL is unset)"
                    .to_string(),
            )
        })?,
    };

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
        body.sources_only,
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

    // The `WebSearch` gate, now that it is known whether this turn ran a chargeable search.
    // A cache hit and an empty result both leave `search_cost` at zero and neither consumes
    // `web_searches_per_day` (A40), so neither may be refused by this check. The cap is also
    // enforced inside the spend below, under the advisory lock, which is what makes it hold
    // under concurrency; this call is what refuses before the provider is reached.
    if search_cost > 0 {
        crate::entitlements::require(&state, &user, Feature::WebSearch).await?;
    }

    // The spend comes after every read that can fail, and immediately before the
    // provider is called: retrieval and composition are a database read and a
    // pure function, so doing them first costs nothing and shrinks the window in
    // which a credit can be spent on a request that never reaches a model.
    //
    // The daily cap is enforced inside this call, under the same lock, because a
    // separate check and spend can interleave and let two requests through on the
    // last slot.
    //
    // **A BYOK turn does not pay for the model call** (A302): the student paid their own
    // provider. It does still pay for a **web search**, because that is our provider and our
    // cost — skipping it would let a BYOK account hammer the search path for free and would
    // leave `web_searches_per_day` uncounted for exactly the accounts with the largest
    // allowance. So the amount is the search's, and the row is marked `byok` so the model's
    // cost stays out of our margin (A156).
    let charged = if using_own_key {
        search_cost
    } else {
        total_cost
    };

    // The daily cap is enforced inside this call, under the same lock, because a
    // separate check and spend can interleave and let two requests through on the
    // last slot.
    //
    // A turn with nothing to charge — a BYOK turn with no search, or a free turn with a
    // cached search — opens its usage row instead. Both branches answer the same `Spent`, so
    // the rest of this request, including the `usage` event the client renders, is one code
    // path.
    // A BYOK turn with nothing to charge — no search — opens its usage row directly. Every
    // other case goes through `spend`, which writes the usage row and, when `charged` is
    // zero, writes no ledger row at all: the balance check passes and the bucket loop has
    // nothing to take. So there is one writer for a charged turn and one for a BYOK one, and
    // no path where a row is labelled by what the *caller* happened to be doing.
    let spent = if using_own_key && charged == 0 {
        credits::begin_byok_usage(&state.pool, user.user_id, user.effective_plan(), ACTION).await?
    } else {
        credits::spend(
            &state,
            user.user_id,
            user.effective_plan(),
            ACTION,
            charged,
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
            using_own_key,
        )
        .await?
    };
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
    //
    // The tool list is the plan's (A272): four READ tools always, the six WRITE ones only
    // with `Feature::AgentAction`. Sending no tools at all — which is what this route did
    // before this phase — is not a safer default: it means a paid account's model cannot
    // read the student's own tasks either.
    let tools = agent::tool_specs(&state, &user).await;
    let mut request = ChatRequest {
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
        tools,
        temperature: TEMPERATURE,
        max_tokens: MAX_TOKENS,
        json_mode: false,
    };

    // The first provider call happens **before** the response body starts, so a provider that
    // is down is an HTTP error rather than an `error` event inside a 200 stream. Later rounds
    // (the ones a tool result causes) happen inside the stream, where the only way to report a
    // failure is an `error` event.
    let stream = provider
        .stream(request.clone())
        .await
        .map_err(|error| AppError::Provider(error.to_string()))?;

    let state_for_stream = state.clone();
    let user_id = user.user_id;
    let prompt_version_for_stream = prompt_version.clone();

    // The agent's per-turn state, moved into the stream. `gate_context` carries the two
    // answers `gate` cannot compute itself (A271): whether the provider can call tools at all,
    // and whether the plan grants the six WRITE tools (A272).
    let user_for_stream = user.clone();
    let provider_for_stream = provider.clone();
    let mut gate_context = stvdy_core::agent::gate::GateContext {
        supports_tools: provider.supports_tools(),
        agent_actions: agent::agent_actions_granted(&state, &user).await,
        proposals_so_far: 0,
    };
    let mut turn = agent::Turn::new(using_own_key);
    let mut tool_messages: Vec<ChatMsg> = Vec::new();

    let events = async_stream::stream! {
        let mut answer = String::new();
        let mut usage = None;
        // How much of `answer` has already been shown. Only complete markers are
        // emitted, so a fabricated one is dropped before the user sees it rather
        // than disappearing when the persisted copy replaces the live turn.
        let mut emitted = 0usize;
        // Which provider call this is. A tool result needs a second call so the model can
        // read it: a READ tool that ran and whose answer never reached the model is a tool
        // the student paid for and did not get.
        let mut round = 0usize;
        // The receiver for the round being read. `select!` pins each future it polls on its own
        // stack, so a plain `&mut Receiver` is enough and no `tokio::pin!` is needed — which
        // matters here because the binding is *reassigned* between rounds.
        let mut stream = stream;

        loop {
            // Whether anything in *this* round needs the model to see it. A proposal does not
            // (the card is the next step, not the model); a read result and a refusal do.
            let mut needs_model = false;
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
                    Some(StreamEvent::Reasoning(text)) => {
                        // Reasoning fragments are forwarded to the client as-is.
                        // They are not appended to `answer` (they are not the
                        // answer) and not held back for marker safety.
                        yield Ok(sse::reasoning(&text));
                    }
                    Some(StreamEvent::Usage(reported)) => {
                        usage = Some(reported);
                    }
                    Some(StreamEvent::ToolCall(call)) => {
                        // The one place a tool call becomes an effect (A271). The route holds
                        // no tool logic: it hands the call to the gate and then to
                        // `agent::handle`, which runs a read inline, turns a write into a
                        // proposal plus a card, or answers a refusal.
                        //
                        // A failure here is **not** fatal to the turn: the answer is already
                        // streaming, and aborting it would replace a partial answer with an
                        // error event for a tool the student never asked about. It is logged
                        // and answered to the model as a result-shaped message, so the model
                        // can say something true rather than continuing as if the tool had
                        // succeeded.
                        match agent::handle(
                            &state_for_stream,
                            &user_for_stream,
                            workspace_id,
                            user_message_id,
                            &call,
                            &mut gate_context,
                            &mut turn,
                        )
                        .await
                        {
                            Ok(agent::AgentOutcome::Answered(result)) => {
                                // A read ran, so the model has something to read back.
                                needs_model = true;
                                tool_messages.push(result);
                            }
                            Ok(agent::AgentOutcome::Proposed { pending_action_id, result }) => {
                                yield Ok(sse::action(pending_action_id, &call.name, &call.arguments));
                                // A proposal needs no follow-up round: the card is already on
                                // the wire and the student, not the model, is what happens
                                // next. Asking the model again would spend a second call to
                                // hear it repeat itself.
                                tool_messages.push(result);
                            }
                            Ok(agent::AgentOutcome::Refused(result)) => {
                                // A refusal does: the model is the only thing that can tell the
                                // student why in their own language, which is the whole point
                                // of answering a refusal instead of dropping the call (A271).
                                needs_model = true;
                                tool_messages.push(result);
                            }
                            Err(error) => {
                                tracing::error!(%error, tool = %call.name, "handling a tool call failed");
                                tool_messages.push(ChatMsg {
                                    role: Role::User,
                                    content: format!(
                                        "The call to `{}` ({}) failed and produced no result.",
                                        call.name, call.id
                                    ),
                                });
                            }
                        }
                    }
                    Some(StreamEvent::Done) | None => break,
                }
            }

            // No call to answer, or the bound is reached: the turn is over. The bound is
            // what stops a model that calls a tool in every round from spending the student's
            // window on one question — `MAX_PROPOSALS_PER_TURN` caps the proposals, and this
            // caps the *rounds*, because a read loop produces no proposals at all.
            if !needs_model || round + 1 >= MAX_TOOL_ROUNDS {
                if needs_model {
                    tracing::warn!(
                        rounds = round + 1,
                        "the tool-round bound was reached with results still pending"
                    );
                }
                break;
            }

            // The next round: the model's own text, then what each tool answered. The role is
            // `Assistant` for the text and `User` for the results, which is the only shape
            // `ChatMsg` carries — a `tool` role would be a change to a trait the roadmap fixes
            // (`gate::tool_result` says the same thing from the other side).
            round += 1;
            request.messages.push(ChatMsg {
                role: Role::Assistant,
                content: answer.clone(),
            });
            request.messages.append(&mut tool_messages);

            match provider_for_stream.stream(request.clone()).await {
                Ok(next) => stream = next,
                Err(error) => {
                    tracing::error!(%error, "the provider call after a tool result failed");
                    yield Ok(sse::error(&AppError::Provider(error.to_string())));
                    break;
                }
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
            && let Err(error) = credits::record_usage(
                &state_for_stream.pool,
                Some(usage_event_id),
                Some(reported.in_tokens),
                Some(reported.out_tokens),
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
        // leaves a user message and no half-answer.
        //
        // The rule is narrowed rather than dropped (A281): a turn that handled at least one
        // tool call persists its message **even when the text is empty**, because such a turn
        // is not empty — it produced a confirmation card, and the card needs a message to
        // hang on. `content = ''` is what the client renders the card beside. A turn with
        // neither text nor a handled call is still not persisted: an empty assistant turn is
        // worse than none.
        //
        // Every exit path must emit exactly one terminator. Returning here without
        // one would leave a client waiting for a `done` that never arrives, since
        // the contract makes `done` the end of the turn.
        if cleaned.trim().is_empty() && !turn.handled {
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
                    // The turn's own price, which is `chat_message` plus `agent_turn` when a
                    // tool call was handled (A282). The message spend above already reported
                    // its own `remaining`; the agent's charge happened later, inside the
                    // stream, so its balance is the honest one to report.
                    yield Ok(sse::usage(
                        total_cost + turn.extra_cost,
                        turn.remaining.unwrap_or(remaining),
                    ));
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
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    // The workspace check is separate from the message query so that a workspace
    // id from another account is a 404 rather than an empty list, which would
    // leak that the id exists but has no messages.
    crate::workspaces::owned_name(&state, user.user_id, workspace_id).await?;

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
