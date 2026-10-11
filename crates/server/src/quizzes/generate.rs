//! `generate_quiz`: the prompt, the one retry, and the questions to store.
//!
//! Mirrors `flashcards::generate` deliberately. The retry is here rather than in the
//! provider because it is a *contract* decision: a reply that is not JSON is worth one
//! corrective attempt, and a reply that is valid JSON with no questions is not.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::config::PlanId;
use stvdy_core::generation::is_retryable;
use stvdy_core::generation::quiz::{parse_questions, resolve_questions};
use stvdy_core::prompts::{ChatVars, ModuleId, PromptChunk, compose_task};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role};

use crate::credits;
use crate::jobs::Failure;
use crate::quizzes::routes::GENERATE_ACTION;
use crate::state::AppState;

/// Low, because a question must be faithful to the material; not zero, because a fully
/// deterministic model produces near-duplicate questions.
const TEMPERATURE: f32 = 0.3;
const MAX_TOKENS: u32 = 3000;

/// How far back the fallback usage lookup reaches. See the deck generator: the spend
/// commits immediately before the quiz row, so this only absorbs that gap.
const USAGE_LOOKBACK: chrono::Duration = chrono::Duration::minutes(5);

/// What the second attempt says when the reply was not JSON at all.
///
/// Unlike the deck's constant, the quiz's correction is **appended to**, not fixed: quiz
/// validation is far richer than a card's (the kind, exactly four options, an answer
/// that is one of them, key points on a short answer), and every one of those messages
/// names the question's index. A constant correction would leave that index in the logs
/// and tell the model nothing about what to fix, so the single retry would not converge.
/// The divergence from `flashcards::generate::CORRECTION` is deliberate.
const CORRECTION: &str = "Your previous reply was not a valid JSON object matching the schema. Reply with only the \
     JSON object, with no prose before or after it and no markdown fences.";

/// The quiz a generation runs for, loaded by the job.
pub struct QuizRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub requested_count: i32,
    pub difficulty: String,
    pub question_types: String,
    pub status: String,
    pub credits_spent: i32,
    pub usage_event_id: Option<Uuid>,
    pub source_ids: Vec<Uuid>,
    pub workspace_name: String,
    pub created_at: DateTime<Utc>,
}

pub struct Generation {
    pub questions: Vec<ResolvedQuestionInsert>,
    pub dropped_labels: Vec<u32>,
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
    pub prompt_version: String,
}

pub struct ResolvedQuestionInsert {
    pub kind: String,
    pub prompt: String,
    pub options: Option<Vec<String>>,
    pub answer: String,
    pub explanation: String,
    pub key_points: Option<Vec<String>>,
    pub chunk_id: Option<Uuid>,
}

/// Runs one generation for one quiz. Returns the questions to insert.
pub async fn generate(state: &AppState, quiz: &QuizRow) -> Result<Generation, Failure> {
    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let chunks = load_chunks(state, quiz).await?;
    if chunks.is_empty() {
        return Err(Failure::permanent(
            "the selected sources have no indexed text to generate from",
        ));
    }

    let user = load_user(state, quiz.user_id).await?;
    let base: ChatVars = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        PlanId::parse(&user.plan_id),
        false,
        false,
        &quiz.workspace_name,
    );

    // Every setting the module declares as required is supplied from the stored row,
    // not re-derived from the request: the job runs after the request returned, and a
    // prompt rebuilt from anything else would be a different prompt than the one the
    // student's credit bought.
    let mut module_vars: BTreeMap<&'static str, String> = BTreeMap::new();
    module_vars.insert("count", quiz.requested_count.to_string());
    module_vars.insert("difficulty", quiz.difficulty.clone());
    module_vars.insert("types", quiz.question_types.clone());
    // `output_language` is deliberately absent: the module declares it optional and
    // defaults to the material's language, which is the right behaviour until there is
    // a user language setting (Phase 9).
    let composed = compose_task(ModuleId::QuizV1, &base, &module_vars, &chunks)
        .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    let usage_row = find_usage_event_id(state, quiz).await;
    attach_prompt_version(state, usage_row, &composed.prompt_version).await;

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

    // The first call is billed whether or not its reply is usable.
    let (first_in, first_out, first_cost) = (
        first.usage.in_tokens,
        first.usage.out_tokens,
        first.usage.provider_cost_usd,
    );

    let (questions, in_tokens, out_tokens, provider_cost_usd) = match parse_questions(&first.text) {
        Ok(questions) => (questions, first_in, first_out, first_cost),
        Err(error) if is_retryable(&error) => {
            let mut retry = request.clone();
            retry.messages.push(ChatMsg {
                role: Role::Assistant,
                content: first.text.clone(),
            });
            // The parse error is appended so the model is told which question was
            // wrong and why. `is_retryable` is only true for a `Json` or `Schema`
            // error, and both carry their own description.
            retry.messages.push(ChatMsg {
                role: Role::User,
                content: format!("{CORRECTION}\n\nThe problem was: {error}"),
            });
            let second = match provider.complete(retry).await {
                Ok(second) => second,
                Err(error) => {
                    record_failed_usage(
                        state,
                        usage_row,
                        first_in,
                        first_out,
                        first_cost,
                        &composed.prompt_version,
                    )
                    .await;
                    return Err(Failure::transient(format!(
                        "retrying the model call: {error}"
                    )));
                }
            };
            let both_in = first_in + second.usage.in_tokens;
            let both_out = first_out + second.usage.out_tokens;
            let both_cost = sum_costs(first_cost, second.usage.provider_cost_usd);
            match parse_questions(&second.text) {
                Ok(questions) => (questions, both_in, both_out, both_cost),
                Err(error) => {
                    record_failed_usage(
                        state,
                        usage_row,
                        both_in,
                        both_out,
                        both_cost,
                        &composed.prompt_version,
                    )
                    .await;
                    return Err(Failure::permanent(format!(
                        "the model did not return usable JSON after a retry: {error}"
                    )));
                }
            }
        }
        Err(error) => {
            record_failed_usage(
                state,
                usage_row,
                first_in,
                first_out,
                first_cost,
                &composed.prompt_version,
            )
            .await;
            return Err(Failure::permanent(format!(
                "the model's reply was unusable: {error}"
            )));
        }
    };

    let outcome = resolve_questions(questions, &composed.labels);

    // `resolve_questions` is infallible by design: it drops a question whose trimmed
    // prompt or answer is empty and records nothing. So a reply of whitespace-only
    // questions parses cleanly and resolves to zero, and without this check the job
    // would mark the quiz `ready` with no questions and keep the charge. Permanent
    // rather than transient: the prompt retry that matters already happened above.
    if outcome.questions.is_empty() {
        record_failed_usage(
            state,
            usage_row,
            in_tokens,
            out_tokens,
            provider_cost_usd,
            &composed.prompt_version,
        )
        .await;
        return Err(Failure::permanent(
            "the model returned no usable questions: every question had an empty prompt or answer",
        ));
    }

    Ok(Generation {
        questions: outcome
            .questions
            .into_iter()
            .map(|question| ResolvedQuestionInsert {
                kind: question.kind,
                prompt: question.prompt,
                options: question.options,
                answer: question.answer,
                explanation: question.explanation,
                key_points: question.key_points,
                chunk_id: question.chunk_id,
            })
            .collect(),
        dropped_labels: outcome.dropped_labels,
        in_tokens,
        out_tokens,
        provider_cost_usd,
        prompt_version: composed.prompt_version,
    })
}

/// Sums two optional costs, keeping `None` for "unknown" rather than treating it as
/// zero: recording zero understates spend in the margin report (A41).
fn sum_costs(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

/// Records what a failed generation already spent, before it gives up.
async fn record_failed_usage(
    state: &AppState,
    usage_event_id: Option<Uuid>,
    in_tokens: u32,
    out_tokens: u32,
    provider_cost_usd: Option<f64>,
    prompt_version: &str,
) {
    if let Err(error) = credits::record_usage(
        &state.pool,
        usage_event_id,
        Some(in_tokens),
        Some(out_tokens),
        provider_cost_usd,
        prompt_version,
    )
    .await
    {
        tracing::error!(%error, "could not record a failed generation's usage");
    }
}

/// Names the prompt version on the quiz's usage row. Best-effort: a reporting gap must
/// not change the job's outcome.
async fn attach_prompt_version(
    state: &AppState,
    usage_event_id: Option<Uuid>,
    prompt_version: &str,
) {
    if let Err(error) = credits::record_usage(
        &state.pool,
        usage_event_id,
        None,
        None,
        None,
        prompt_version,
    )
    .await
    {
        tracing::error!(%error, "could not record the generation's prompt version");
    }
}

/// The usage row this quiz's spend created.
pub(crate) async fn find_usage_event_id(state: &AppState, quiz: &QuizRow) -> Option<Uuid> {
    if let Some(id) = quiz.usage_event_id {
        return Some(id);
    }

    let since = quiz.created_at - USAGE_LOOKBACK;
    match credits::latest_usage_event(&state.pool, quiz.user_id, GENERATE_ACTION, since).await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, quiz_id = %quiz.id, "could not find the quiz's usage event");
            None
        }
    }
}

/// The chunks the prompt is built from, in a deterministic order.
async fn load_chunks(state: &AppState, quiz: &QuizRow) -> Result<Vec<PromptChunk>, Failure> {
    crate::chunks::select_for_generation(&state.pool, &quiz.source_ids, quiz.user_id)
        .await
        .map_err(|error| Failure::transient(format!("loading the quiz's chunks: {error}")))
}

/// The values `base_vars` needs, read from the row rather than from an `AuthUser` the
/// job does not have.
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
        .map_err(|error| Failure::transient(format!("loading the quiz's owner: {error}")))?
        .ok_or_else(|| Failure::permanent("the quiz's owner no longer exists"))?;

    Ok(UserRow {
        email: row.get("email"),
        timezone: row.get("timezone"),
        plan_id: row.get("plan_id"),
    })
}
