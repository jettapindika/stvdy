//! The report prompts: the outline (one call, in the route) and one section (a job).
//!
//! Mirrors `quizzes::generate` deliberately, including the one retry: a reply that is not
//! the documented shape is worth one corrective attempt, and a reply that is empty is not.
//!
//! The two calls differ in one way that decides where each lives. The outline is a single
//! short completion whose result the student edits, so it is made **in the route** and its
//! failure is the request's failure. A section is per-index, retryable, and long enough to
//! outlive a request, so it is made by a job and its failure is the job's — which is why the
//! section path returns `Failure` (retryable or permanent) and the outline path does not.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::config::PlanId;
use stvdy_core::generation::is_retryable;
use stvdy_core::generation::report::{ReportOutline, parse_outline, parse_section};
use stvdy_core::prompts::{ChatVars, ModuleId, PromptChunk, compose_task};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role};

use crate::auth::extract::AuthUser;
use crate::jobs::Failure;
use crate::quizzes::generate::{attach_prompt_version, load_user, record_failed_usage, sum_costs};
use crate::state::AppState;

/// A report's prose must be faithful to the sources; the small amount of variation above
/// the quiz's 0.3 is there because a section is paragraphs rather than short answers, and
/// a fully deterministic model writes the same opening sentence for every section.
const TEMPERATURE: f32 = 0.4;
const MAX_TOKENS: u32 = 3000;

/// What the second attempt says when the reply was not the documented shape.
///
/// Appended to rather than fixed, exactly as the quiz's is: `parse_section`'s schema
/// failure names what was wrong (the reply was JSON, not Markdown), and a constant
/// correction would throw that away and leave the model to guess.
const CORRECTION: &str = "Your previous reply did not match the contract. Reply with only the section's \
     Markdown, with its [n] labels copied from the sources, and no JSON, no prose about the \
     reply and no markdown fences.";

/// The report a section generation runs for, loaded by the job.
pub struct ReportRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    /// The outline, already parsed: the route validated it before it was stored, and a row
    /// whose outline cannot be read is a permanent failure rather than a retry.
    pub outline: ReportOutline,
    pub source_ids: Vec<Uuid>,
    pub style: String,
    pub length: String,
    pub status: String,
    /// The section index the route put in flight. The job's payload must name the same one.
    pub generating_index: Option<i32>,
    /// Index-aligned with `outline.sections`: the text each section has, or `None`.
    pub sections_md: Vec<Option<String>>,
    pub credits_spent: i32,
    pub usage_event_id: Option<Uuid>,
    pub workspace_name: String,
    pub created_at: DateTime<Utc>,
}

impl ReportRow {
    /// The text of the section the job is writing, or `None` when the row has no such
    /// index (an outline edited shorter while the job was queued).
    pub fn target(&self) -> Option<&stvdy_core::generation::report::OutlineSection> {
        let index = usize::try_from(self.generating_index?).ok()?;
        self.outline.sections.get(index)
    }

    /// The headings of the other sections that already have text, for
    /// `{{other_section_headings}}`.
    ///
    /// Only the written ones: the variable exists so the model does not repeat what the
    /// report already says, and a heading with no text under it has nothing to repeat. The
    /// order is the document's, so the prompt is a function of the row.
    pub fn other_headings(&self) -> Vec<String> {
        let mut headings = Vec::new();
        for (index, section) in self.outline.sections.iter().enumerate() {
            if Some(index as i32) == self.generating_index {
                continue;
            }
            if self.sections_md.get(index).is_some_and(Option::is_some) {
                headings.push(section.heading.clone());
            }
        }
        headings
    }
}

/// One section's reply and what the call cost.
pub struct SectionGeneration {
    pub body_md: String,
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
    pub prompt_version: String,
}

/// One outline and what the call cost.
pub struct OutlineGeneration {
    pub outline: ReportOutline,
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
    pub prompt_version: String,
}

/// What the outline route asks the model.
pub struct OutlineRequest<'a> {
    pub topic: &'a str,
    pub style: &'a str,
    pub length: &'a str,
    pub source_ids: &'a [Uuid],
    pub workspace_name: &'a str,
    /// The chunks the prompt is built from, already loaded and checked non-empty by the
    /// route. Passed in rather than re-selected here so the validation the route did before
    /// charging is the same selection the prompt uses.
    pub chunks: &'a [PromptChunk],
}

/// Proposes an outline. One call, no retry: this is the step whose result the student
/// reads and edits, so a reply that does not parse is worth telling them about rather than
/// silently spending a second call on.
pub async fn propose_outline(
    state: &AppState,
    user: &AuthUser,
    usage_event_id: Uuid,
    request: &OutlineRequest<'_>,
) -> Result<OutlineGeneration, Failure> {
    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let base: ChatVars = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        PlanId::parse(&user.plan_id),
        false,
        false,
        // Task prompts compose on v1's trimmed base, which has no `{{sources_only}}`; an
        // outline is always drawn from the user's sources, so the value is `true` and is
        // never read.
        true,
        request.workspace_name,
    );

    let mut module_vars: BTreeMap<&'static str, String> = BTreeMap::new();
    module_vars.insert("topic", request.topic.to_string());
    module_vars.insert("style", request.style.to_string());
    module_vars.insert("length", request.length.to_string());
    // `output_language` is deliberately absent: the module declares it optional and defaults
    // to the material's language, which is what makes an Indonesian source produce an
    // Indonesian report without a user language setting (Phase 9).
    let composed = compose_task(
        ModuleId::ReportOutlineV1,
        &base,
        &module_vars,
        request.chunks,
    )
    .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    attach_prompt_version(state, Some(usage_event_id), &composed.prompt_version).await;

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

    let reply = provider
        .complete(request)
        .await
        .map_err(|error| Failure::transient(format!("calling the model: {error}")))?;

    match parse_outline(&reply.text) {
        Ok(outline) => Ok(OutlineGeneration {
            outline,
            in_tokens: reply.usage.in_tokens,
            out_tokens: reply.usage.out_tokens,
            provider_cost_usd: reply.usage.provider_cost_usd,
            prompt_version: composed.prompt_version,
        }),
        Err(error) => {
            record_failed_usage(
                state,
                Some(usage_event_id),
                reply.usage.in_tokens,
                reply.usage.out_tokens,
                reply.usage.provider_cost_usd,
                &composed.prompt_version,
            )
            .await;
            Err(Failure::permanent(format!(
                "the model did not return a usable outline: {error}"
            )))
        }
    }
}

/// Writes one section. The prompt is rebuilt from the stored row, not from the request
/// that asked for it: the job runs after that request returned, and a prompt rebuilt from
/// anything else would be a different prompt than the one the student's credit bought.
pub async fn generate_section(
    state: &AppState,
    report: &ReportRow,
) -> Result<SectionGeneration, Failure> {
    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let section = report.target().ok_or_else(|| {
        Failure::permanent("the report's in-flight section is outside its outline")
    })?;

    let chunks =
        crate::chunks::select_for_generation(&state.pool, &report.source_ids, report.user_id)
            .await
            .map_err(|error| Failure::transient(format!("loading the report's chunks: {error}")))?;
    if chunks.is_empty() {
        return Err(Failure::permanent(
            "the selected sources have no indexed text to write the section from",
        ));
    }

    let user = load_user(state, report.user_id).await?;
    let base: ChatVars = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        PlanId::parse(&user.plan_id),
        false,
        false,
        true,
        &report.workspace_name,
    );

    let mut module_vars: BTreeMap<&'static str, String> = BTreeMap::new();
    module_vars.insert("heading", section.heading.clone());
    module_vars.insert("title", report.title.clone());
    module_vars.insert("purpose", section.purpose.clone());
    module_vars.insert("style", report.style.clone());
    module_vars.insert("length", report.length.clone());
    let others = report.other_headings();
    if !others.is_empty() {
        module_vars.insert("other_section_headings", others.join("; "));
    }
    // `other_section_headings` is absent when no other section has been written yet, and
    // the module declares it optional for exactly that reason: the first section of a
    // report has nothing to avoid repeating, and the prompt says so rather than receiving
    // an empty list.
    let composed = compose_task(ModuleId::ReportSectionV1, &base, &module_vars, &chunks)
        .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    let usage_row = find_usage_event_id(state, report).await;
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
        json_mode: false,
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

    let (body_md, in_tokens, out_tokens, provider_cost_usd) = match parse_section(&first.text) {
        Ok(section) => (section.body_md, first_in, first_out, first_cost),
        Err(error) if is_retryable(&error) => {
            let mut retry = request.clone();
            retry.messages.push(ChatMsg {
                role: Role::Assistant,
                content: first.text.clone(),
            });
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
            match parse_section(&second.text) {
                Ok(section) => (section.body_md, both_in, both_out, both_cost),
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
                        "the model did not return a usable section after a retry: {error}"
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

    Ok(SectionGeneration {
        body_md,
        in_tokens,
        out_tokens,
        provider_cost_usd,
        prompt_version: composed.prompt_version,
    })
}

/// The usage row this report's spend created.
///
/// The route always stores the id on the row, so the lookback only covers a row written
/// before this phase added the column.
pub(crate) async fn find_usage_event_id(state: &AppState, report: &ReportRow) -> Option<Uuid> {
    if let Some(id) = report.usage_event_id {
        return Some(id);
    }

    let since = report.created_at - crate::quizzes::generate::USAGE_LOOKBACK;
    match crate::credits::latest_usage_event(
        &state.pool,
        report.user_id,
        crate::reports::routes::SECTION_ACTION,
        since,
    )
    .await
    {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, report_id = %report.id, "could not find the report's usage event");
            None
        }
    }
}

/// Reads a report row into [`ReportRow`], parsing its stored outline.
///
/// A row whose `outline` cannot be read is a **permanent** failure: the route validated the
/// outline before storing it, so this is a corrupt row rather than a model reply worth
/// retrying, and retrying would spend four more attempts to read the same bytes.
pub fn row_from(row: &sqlx::postgres::PgRow) -> Result<ReportRow, Failure> {
    let outline_json: serde_json::Value = row.get("outline");
    let outline: ReportOutline = serde_json::from_value(outline_json).map_err(|error| {
        Failure::permanent(format!("the report's outline is unreadable: {error}"))
    })?;

    let sections: Vec<Option<String>> = row
        .get::<Option<serde_json::Value>, _>("sections_md")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    Ok(ReportRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        title: row.get("title"),
        outline,
        source_ids: row.get("source_ids"),
        style: row.get("style"),
        length: row.get("length"),
        status: row.get("status"),
        generating_index: row.get("generating_index"),
        sections_md: sections,
        credits_spent: row.get("credits_spent"),
        usage_event_id: row.get("usage_event_id"),
        workspace_name: row.get("workspace_name"),
        created_at: row.get("created_at"),
    })
}
