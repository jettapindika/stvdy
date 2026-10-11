//! `generate_cheatsheet`: the prompt, the one retry, and the blocks to store.
//!
//! Mirrors `quizzes::generate` deliberately. The contract is not a second definition of the
//! document shape: the reply is parsed by `stvdy_core::render::blocks::parse_blocks`, which
//! is the very type the pure renderer takes as input, so the parser that accepts a reply and
//! the renderer that draws it cannot disagree about one key.
//!
//! What is stored is the **fence-stripped reply text**, not a re-serialisation of the parsed
//! blocks. The core block model is `Deserialize`-only on purpose (the renderer never needs
//! to write one), and re-encoding it here would mean a `Serialize` impl that is a second
//! statement of the schema — the exact drift the shared type exists to prevent. The reply
//! text is the canonical form, `parse_blocks` is deterministic, and a re-render therefore
//! parses the same bytes into the same document for free.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::config::PlanId;
use stvdy_core::generation::is_retryable;
use stvdy_core::prompts::{ChatVars, ModuleId, PromptChunk, compose_task};
use stvdy_core::render::blocks::{Blocks, parse_blocks};
use stvdy_core::render::{Cover, cheatsheet_v1, render_html};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role};

use crate::jobs::Failure;
use crate::quizzes::generate::{attach_prompt_version, load_user, record_failed_usage, sum_costs};
use crate::state::AppState;

/// Content that must be faithful to the material, with the small amount of variation a
/// cheatsheet's tables and examples need to not read as one repeated sentence.
const TEMPERATURE: f32 = 0.3;

/// The blocks of a document are far more output than a quiz's questions, and the module asks
/// for up to six chapters of up to eight blocks each.
const MAX_TOKENS: u32 = 8000;

/// What the second attempt says when the reply did not parse.
///
/// Appended to rather than fixed, like the quiz's: `parse_blocks`'s messages name the
/// chapter, the section and the block index of the offending block, and a constant
/// correction would throw that away and leave the model to guess which of forty blocks was
/// wrong.
const CORRECTION: &str = "Your previous reply did not match the schema. Reply with only the JSON object, \
     with no prose before or after it and no markdown fences. Every field the schema names is \
     required, and no field it does not name may appear.";

/// The cheatsheet a generation runs for, loaded by the job.
pub struct CheatsheetRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub scope: String,
    pub exam_date: Option<chrono::NaiveDate>,
    pub depth: String,
    pub status: String,
    pub source_ids: Vec<Uuid>,
    pub credits_spent: i32,
    pub usage_event_id: Option<Uuid>,
    pub workspace_name: String,
    pub created_at: DateTime<Utc>,
}

/// One generation's reply and what the call cost.
pub struct Generation {
    /// What `blocks_key` will hold: the reply text and the label map it was written against.
    pub document: StoredDocument,
    /// The parsed document: the renderer's input, and the proof the reply is renderable
    /// before anything is stored.
    pub blocks: Blocks,
    /// The label map for this render, built from the same chunks the prompt carried.
    pub labels: ContextLabels,
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
    pub prompt_version: String,
}

/// What `blocks_key` holds: the reply text, plus the label map it was generated against.
///
/// The reply text is stored **verbatim** (fence-stripped), because it is the canonical form
/// the renderer parses and re-encoding the parsed blocks would need a second statement of
/// the schema in the server. The labels are stored beside it because they cannot be
/// recovered from the blocks: they are request-scoped, and a re-render that re-derived them
/// would silently drop every citation marker whenever a source had been deleted or
/// re-indexed — a different document from the one the student paid for, with nothing on the
/// page to say so.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StoredDocument {
    pub blocks: String,
    pub labels: Vec<StoredLabel>,
}

/// One label as it was sent to the model.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StoredLabel {
    pub label: u32,
    pub chunk_id: Uuid,
    pub source_id: Uuid,
    pub page: Option<i32>,
}

impl StoredDocument {
    /// The artifact for a generation.
    ///
    /// Built from the chunks the prompt was composed with, not from the `ContextLabels`
    /// the composer returned: the map is request-scoped and offers no way to enumerate
    /// itself, and the chunks are the thing it was built from in the first place.
    pub fn of(raw_blocks: &str, chunks: &[PromptChunk]) -> Self {
        Self {
            blocks: raw_blocks.to_string(),
            labels: chunks
                .iter()
                .map(|chunk| StoredLabel {
                    label: chunk.label,
                    chunk_id: chunk.chunk_id,
                    source_id: chunk.source_id,
                    page: chunk.page,
                })
                .collect(),
        }
    }

    /// The bytes to write to object storage.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Parses a stored artifact back into the renderer's two inputs.
pub fn from_stored(
    raw: &str,
) -> Result<(Blocks, ContextLabels), stvdy_core::generation::GenerationError> {
    let stored: StoredDocument = serde_json::from_str(raw).map_err(|error| {
        stvdy_core::generation::GenerationError::Schema(format!(
            "the stored document is not readable: {error}"
        ))
    })?;

    let blocks = parse_blocks(&stored.blocks)?;
    let labels = ContextLabels::new(
        stored
            .labels
            .into_iter()
            .map(|label| LabeledChunk {
                label: label.label,
                chunk_id: label.chunk_id,
                source_id: label.source_id,
                page: label.page,
            })
            .collect(),
        Vec::new(),
    );

    Ok((blocks, labels))
}

/// Runs one generation for one cheatsheet.
pub async fn generate(state: &AppState, cheatsheet: &CheatsheetRow) -> Result<Generation, Failure> {
    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let chunks = crate::chunks::select_for_generation(
        &state.pool,
        &cheatsheet.source_ids,
        cheatsheet.user_id,
    )
    .await
    .map_err(|error| Failure::transient(format!("loading the cheatsheet's chunks: {error}")))?;
    if chunks.is_empty() {
        return Err(Failure::permanent(
            "the selected sources have no indexed text to build a cheatsheet from",
        ));
    }

    let user = load_user(state, cheatsheet.user_id).await?;
    let base: ChatVars = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        PlanId::parse(&user.plan_id),
        false,
        false,
        // Task prompts compose on v1's trimmed base, which has no `{{sources_only}}`; a
        // cheatsheet is always built from the user's sources, so the value is `true` and is
        // never read.
        true,
        &cheatsheet.workspace_name,
    );

    // Every setting the module declares as required is supplied from the stored row, not
    // re-derived from the request: the job runs after the request returned, and a prompt
    // rebuilt from anything else would be a different prompt than the one the student's
    // credit bought. `exam_date` is left to the module's own default when the student has no
    // date, which is the state the schema models rather than an omission.
    let mut module_vars: BTreeMap<&'static str, String> = BTreeMap::new();
    module_vars.insert("scope", cheatsheet.scope.clone());
    module_vars.insert("depth", cheatsheet.depth.clone());
    if let Some(exam_date) = cheatsheet.exam_date {
        module_vars.insert("exam_date", exam_date.to_string());
    }
    // `output_language` is deliberately absent: the module declares it optional and defaults
    // to the material's language, which is what makes an Indonesian source produce an
    // Indonesian cheatsheet without a user language setting (Phase 9).
    let composed = compose_task(ModuleId::CheatsheetV1, &base, &module_vars, &chunks)
        .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    let usage_row = cheatsheet.usage_event_id;
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

    let (blocks, raw_blocks, in_tokens, out_tokens, provider_cost_usd) =
        match parse_blocks(&first.text) {
            Ok(blocks) => (blocks, first.text, first_in, first_out, first_cost),
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
                match parse_blocks(&second.text) {
                    Ok(blocks) => (blocks, second.text, both_in, both_out, both_cost),
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

    // The fence is stripped before it is stored, so the artifact holds exactly what the
    // parser reads: `parse_blocks` strips one too, and storing the fenced text would mean
    // the stored form and the parsed form differ by a wrapper.
    let document = StoredDocument::of(
        stvdy_core::generation::strip_code_fence(&raw_blocks),
        &chunks,
    );

    Ok(Generation {
        document,
        blocks,
        labels: composed.labels,
        in_tokens,
        out_tokens,
        provider_cost_usd,
        prompt_version: composed.prompt_version,
    })
}

/// The object key for one cheatsheet artifact.
///
/// Scoped by user first, like a source's key, so a key cannot be guessed from another
/// account's ids and so a bucket listing groups by owner.
pub fn object_key(user_id: Uuid, cheatsheet_id: Uuid, name: &str, extension: &str) -> String {
    format!("users/{user_id}/cheatsheets/{cheatsheet_id}/{name}.{extension}")
}

/// The cover the pure renderer fills from the blocks and the row.
///
/// Every field is either from the document or from the row, and none of them is a clock
/// reading inside the renderer — which is what keeps `render_html` pure. `generated_at` is
/// the row's own `created_at` rather than "now", and that is not a shortcut: a re-render
/// must produce byte-identical HTML, and a clock reading in the cover would change the
/// document on every re-render while the content stayed the same. `created_at` is when the
/// student asked for the document, which is what the cover's "Dibuat" means anyway.
pub fn cover_for(
    cheatsheet: &CheatsheetRow,
    blocks: &Blocks,
    timezone: &str,
    source_count: usize,
) -> Cover {
    let zone: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::UTC);

    Cover {
        course: blocks
            .course
            .clone()
            .unwrap_or_else(|| cheatsheet.scope.clone()),
        exam: blocks
            .exam
            .clone()
            .unwrap_or_else(|| cheatsheet.scope.clone()),
        exam_date: cheatsheet
            .exam_date
            .map(|date| date.to_string())
            .unwrap_or_else(|| stvdy_core::prompts::registry::NO_EXAM_DATE.to_string()),
        source_count,
        generated_at: cheatsheet
            .created_at
            .with_timezone(&zone)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        lang: DEFAULT_LANG.to_string(),
    }
}

/// The document's language. Indonesian is the product's second language and the one roadmap
/// gate 6 exercises; the module writes the content in the material's language, and this is
/// what tells the browser and a screen reader how to read it back.
pub const DEFAULT_LANG: &str = "id";

/// The labels the renderer resolves a block's citations against.
///
/// The same map the prompt was composed with, so a label the model cited resolves to the
/// chunk this request sent — and a label it invented resolves to nothing, which the renderer
/// drops rather than rendering as a dangling reference (A17).
pub fn labels_of(chunks: &[PromptChunk]) -> ContextLabels {
    ContextLabels::new(
        chunks
            .iter()
            .map(|chunk| LabeledChunk {
                label: chunk.label,
                chunk_id: chunk.chunk_id,
                source_id: chunk.source_id,
                page: chunk.page,
            })
            .collect(),
        Vec::new(),
    )
}

/// Renders a document's HTML from its blocks.
///
/// The template is `stvdy_core::render::cheatsheet_v1()`, compiled into the binary, and the
/// call is pure: no clock, no network, no randomness. That is the property the re-render
/// endpoint exists for, and the reason this function takes the cover rather than building it.
pub fn render_document(
    blocks: &Blocks,
    labels: &ContextLabels,
    cover: &Cover,
) -> Result<stvdy_core::render::Rendered, stvdy_core::render::RenderError> {
    render_html(blocks, &cheatsheet_v1(), labels, cover)
}

/// The template version stored on every artifact this binary produces.
///
/// Read from the template itself rather than written down a second time: the value in
/// `cheatsheets.template_version` is what lets an admin list documents rendered by an older
/// template, and a literal here would keep saying "v1" after the template's own version
/// moved.
pub fn template_version() -> &'static str {
    cheatsheet_v1().version
}

/// Renders the document and stores the HTML, returning the key it was written to.
///
/// Shared by the generation job and `POST /cheatsheets/:id/re-render`, which is the whole
/// point: re-render must produce **byte-identical** HTML from the same blocks, and two
/// call sites that each built their own cover would drift the first time one of them
/// changed a field. The renderer is pure, so this is a function of the blocks, the labels,
/// the row and the template — nothing else.
pub async fn render_and_store(
    state: &AppState,
    cheatsheet: &CheatsheetRow,
    blocks: &Blocks,
    labels: &ContextLabels,
) -> Result<(String, stvdy_core::render::Rendered), Failure> {
    let timezone = load_timezone(state, cheatsheet.user_id).await?;
    let cover = cover_for(cheatsheet, blocks, &timezone, cheatsheet.source_ids.len());

    let rendered = render_document(blocks, labels, &cover).map_err(|error| {
        // The blocks parsed but did not render, which is a contract failure rather than a
        // transient one: retrying would ask the model the same question and get a document
        // with the same defect.
        Failure::permanent(format!("rendering the cheatsheet: {error}"))
    })?;

    if !rendered.dropped.is_empty() {
        // Recorded rather than silent: a climbing rate means the prompt or the model changed,
        // and the student is owed the knowledge that their document is missing a block.
        tracing::warn!(
            cheatsheet_id = %cheatsheet.id,
            dropped = ?rendered.dropped,
            "the renderer refused blocks the model returned"
        );
    }

    let html_key = object_key(cheatsheet.user_id, cheatsheet.id, "document", "html");
    state
        .storage
        .put(
            &html_key,
            bytes::Bytes::from(rendered.html.clone().into_bytes()),
            "text/html",
        )
        .await
        .map_err(|error| Failure::transient(format!("storing the rendered HTML: {error}")))?;

    Ok((html_key, rendered))
}

/// The user's IANA zone, for the cover's `generated_at`.
///
/// A missing user is impossible here (the row references one), and an unparseable zone falls
/// back to UTC rather than failing a generation that is otherwise complete.
pub async fn load_timezone(state: &AppState, user_id: Uuid) -> Result<String, Failure> {
    let zone: Option<String> = sqlx::query_scalar("select timezone from users where id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| Failure::transient(format!("loading the owner's timezone: {error}")))?;

    Ok(zone.unwrap_or_else(|| "UTC".to_string()))
}

/// Reads a cheatsheet row into [`CheatsheetRow`].
pub fn row_from(row: &sqlx::postgres::PgRow) -> CheatsheetRow {
    CheatsheetRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        title: row.get("title"),
        scope: row.get("scope"),
        exam_date: row.get("exam_date"),
        depth: row.get("depth"),
        status: row.get("status"),
        source_ids: row.get("source_ids"),
        credits_spent: row.get("credits_spent"),
        usage_event_id: row.get("usage_event_id"),
        workspace_name: row.get("workspace_name"),
        created_at: row.get("created_at"),
    }
}
