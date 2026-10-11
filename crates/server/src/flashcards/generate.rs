//! `generate_flashcards`: the prompt, the one retry, the insert, and the refund.
//!
//! The retry is here rather than in the provider because it is a *contract*
//! decision: a reply that is not JSON is worth one corrective attempt, and a reply
//! that is valid JSON with no cards is not.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::config::PlanId;
use stvdy_core::generation::flashcards::{parse_cards, resolve_cards};
use stvdy_core::generation::is_retryable;
use stvdy_core::prompts::{ChatVars, ModuleId, PromptChunk, compose_task};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role};

use crate::credits;
use crate::flashcards::routes::GENERATE_ACTION;
use crate::jobs::Failure;
use crate::state::AppState;

/// How many chunks of the selected sources the prompt carries.
///
/// Deterministic (`order by source_id, idx`) and bounded, because the prompt has a
/// token budget and the same deck must produce the same prompt twice.
pub const GENERATION_CHUNK_LIMIT: i64 = 40;

/// Low, because a card must be faithful to the material; not zero, because a fully
/// deterministic model produces near-duplicate cards.
const TEMPERATURE: f32 = 0.2;
const MAX_TOKENS: u32 = 2000;

/// How far back the fallback usage lookup reaches.
///
/// The spend that creates the row commits immediately before the deck row is
/// inserted, so the event's `created_at` is a few milliseconds earlier than the
/// deck's. This window only has to absorb that gap; it is not a product limit.
const USAGE_LOOKBACK: chrono::Duration = chrono::Duration::minutes(5);

/// What the second attempt says. Short and specific: the model has the bad reply
/// in its own context, so repeating the whole instruction is what makes a model
/// answer the same way again.
const CORRECTION: &str = "Your previous reply was not a valid JSON object matching the schema. Reply with only the \
     JSON object, with no prose before or after it and no markdown fences.";

/// The deck a generation runs for, loaded by the job.
pub struct DeckRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub requested_count: i32,
    pub status: String,
    pub credits_spent: i32,
    pub usage_event_id: Option<Uuid>,
    pub source_ids: Vec<Uuid>,
    pub workspace_name: String,
    pub created_at: DateTime<Utc>,
}

pub struct Generation {
    pub cards: Vec<ResolvedCardInsert>,
    pub dropped_labels: Vec<u32>,
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
    pub prompt_version: String,
}

pub struct ResolvedCardInsert {
    pub front: String,
    pub back: String,
    pub chunk_id: Option<Uuid>,
}

/// Runs one generation for one deck. Returns the cards to insert.
pub async fn generate(state: &AppState, deck: &DeckRow) -> Result<Generation, Failure> {
    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let chunks = load_chunks(state, deck).await?;
    if chunks.is_empty() {
        return Err(Failure::permanent(
            "the selected sources have no indexed text to generate from",
        ));
    }

    let user = load_user(state, deck.user_id).await?;
    let base: ChatVars = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        PlanId::parse(&user.plan_id),
        false,
        false,
        &deck.workspace_name,
    );
    let mut module_vars: BTreeMap<&'static str, String> = BTreeMap::new();
    module_vars.insert("count", deck.requested_count.to_string());
    // `output_language` is deliberately absent: the module declares it optional and
    // defaults to the material's language, which is the right behaviour until there
    // is a user language setting (Phase 9).
    let composed = compose_task(ModuleId::FlashcardsV1, &base, &module_vars, &chunks)
        .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    // The prompt version is recorded before the model is called, so a generation
    // that fails afterwards still leaves a usage row attributable to a prompt
    // version (the roadmap's rule: every model call must be attributable). The
    // token counts and the provider's cost are attached on success, when they
    // exist; recording them as zero here would read as "the model returned
    // nothing" in the margin report (A41).
    attach_prompt_version(state, deck, &composed.prompt_version).await;

    let request = ChatRequest {
        model: String::new(),
        messages: vec![ChatMsg {
            role: Role::System,
            content: composed.text.clone(),
        }],
        tools: Vec::new(),
        temperature: TEMPERATURE,
        max_tokens: MAX_TOKENS,
        json_mode: true,
    };

    let first = provider
        .complete(request.clone())
        .await
        .map_err(|error| Failure::transient(format!("calling the model: {error}")))?;

    let (cards, in_tokens, out_tokens, provider_cost_usd) = match parse_cards(&first.text) {
        Ok(cards) => (
            cards,
            first.usage.in_tokens,
            first.usage.out_tokens,
            first.usage.provider_cost_usd,
        ),
        Err(error) if is_retryable(&error) => {
            let mut retry = request.clone();
            retry.messages.push(ChatMsg {
                role: Role::Assistant,
                content: first.text.clone(),
            });
            retry.messages.push(ChatMsg {
                role: Role::User,
                content: CORRECTION.to_string(),
            });
            let second = provider
                .complete(retry)
                .await
                .map_err(|error| Failure::transient(format!("retrying the model call: {error}")))?;
            match parse_cards(&second.text) {
                Ok(cards) => (
                    cards,
                    first.usage.in_tokens + second.usage.in_tokens,
                    first.usage.out_tokens + second.usage.out_tokens,
                    sum_costs(
                        first.usage.provider_cost_usd,
                        second.usage.provider_cost_usd,
                    ),
                ),
                Err(error) => {
                    return Err(Failure::permanent(format!(
                        "the model did not return usable JSON after a retry: {error}"
                    )));
                }
            }
        }
        Err(error) => {
            return Err(Failure::permanent(format!(
                "the model's reply was unusable: {error}"
            )));
        }
    };

    let outcome = resolve_cards(cards, &composed.labels);

    // `resolve_cards` is infallible by design: it drops a card whose trimmed front
    // or back is empty and records nothing. So a reply of whitespace-only cards
    // parses cleanly and resolves to zero, and without this check the job would
    // mark the deck `ready` with no cards and keep the charge. Permanent rather
    // than transient: the prompt retry that matters already happened above, and a
    // second job attempt would call the model again and buy a second chance at the
    // same empty answer.
    if outcome.cards.is_empty() {
        return Err(Failure::permanent(
            "the model returned no usable cards: every card had an empty front or back",
        ));
    }

    Ok(Generation {
        cards: outcome
            .cards
            .into_iter()
            .map(|card| ResolvedCardInsert {
                front: card.front,
                back: card.back,
                chunk_id: card.chunk_id,
            })
            .collect(),
        dropped_labels: outcome.dropped_labels,
        in_tokens,
        out_tokens,
        provider_cost_usd,
        prompt_version: composed.prompt_version,
    })
}

/// Sums two optional costs, keeping `None` for "unknown" rather than treating it
/// as zero: recording zero understates spend in the margin report (A41).
fn sum_costs(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

/// Names the prompt version on the deck's usage row.
///
/// Best-effort: the cards and the credit are unaffected by this write, so a
/// failure here is a reporting gap and is logged rather than propagated.
async fn attach_prompt_version(state: &AppState, deck: &DeckRow, prompt_version: &str) {
    let Some(usage_event_id) = usage_event_id(state, deck).await else {
        return;
    };
    if let Err(error) = credits::record_usage(
        &state.pool,
        Some(usage_event_id),
        None,
        None,
        None,
        prompt_version,
    )
    .await
    {
        tracing::error!(%error, %usage_event_id, "could not record the generation's prompt version");
    }
}

/// The usage row this deck's spend created.
///
/// The deck normally carries the id. The lookup is the fallback the nullable column
/// exists for: a row written without one still has to be attributable, and the
/// spend commits immediately before the deck row, so a short lookback finds it.
pub(crate) async fn usage_event_id(state: &AppState, deck: &DeckRow) -> Option<Uuid> {
    if let Some(id) = deck.usage_event_id {
        return Some(id);
    }

    let since = deck.created_at - USAGE_LOOKBACK;
    match credits::latest_usage_event(&state.pool, deck.user_id, GENERATE_ACTION, since).await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, deck_id = %deck.id, "could not find the deck's usage event");
            None
        }
    }
}

/// The chunks the prompt is built from, in a deterministic order.
async fn load_chunks(state: &AppState, deck: &DeckRow) -> Result<Vec<PromptChunk>, Failure> {
    let rows = sqlx::query(
        "select c.id, c.source_id, c.page, c.content, s.title
           from chunks c
           join sources s on s.id = c.source_id
          where c.source_id = any($1)
          order by c.source_id, c.idx
          limit $2",
    )
    .bind(&deck.source_ids)
    .bind(GENERATION_CHUNK_LIMIT)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the deck's chunks: {error}")))?;

    // The label is the index plus one, which is what the composer writes into the
    // prompt and what the label resolver maps back.
    Ok(rows
        .iter()
        .enumerate()
        .map(|(index, row)| PromptChunk {
            label: index as u32 + 1,
            chunk_id: row.get("id"),
            source_id: row.get("source_id"),
            source_title: row.get("title"),
            page: row.get("page"),
            content: row.get("content"),
        })
        .collect())
}

/// The values `base_vars` needs, read from the row rather than from an `AuthUser`
/// the job does not have.
struct UserRow {
    email: String,
    timezone: String,
    plan_id: String,
}

async fn load_user(state: &AppState, user_id: Uuid) -> Result<UserRow, Failure> {
    let row = sqlx::query("select email, timezone, plan_id from users where id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| Failure::transient(format!("loading the deck's owner: {error}")))?
        .ok_or_else(|| Failure::permanent("the deck's owner no longer exists"))?;

    Ok(UserRow {
        email: row.get("email"),
        timezone: row.get("timezone"),
        plan_id: row.get("plan_id"),
    })
}
