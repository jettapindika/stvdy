//! `ingest_source`: bytes to chunks with embeddings.
//!
//! The pipeline is the spec's (docs/SPEC.md § Ingestion): extract text with page
//! numbers, chunk sentence-safe, embed in batches, skip re-embedding a chunk
//! whose `(embedder, content_hash)` already exists, and leave a readable error
//! on the source when anything fails. The last part matters as much as the happy
//! path: the client shows `sources.error` verbatim, so "PDF ingestion is not
//! configured on this server" is the difference between a bug report and a
//! mystery.
//!
//! Failures are classified, not just reported. A missing pdfium library or an
//! unparseable document fails identically on every attempt, so it is permanent
//! and the job ends immediately; a storage or provider error is transient and is
//! retried with backoff. Getting this wrong in either direction is visible: too
//! eager and the source sits `processing` for half a minute, too timid and a
//! recovered network never gets a second chance.

use sqlx::Row;
use stvdy_core::entitlements::Feature;
use stvdy_core::traits::search::{FetchError, PageFetcher};
use uuid::Uuid;

use crate::jobs::{ClaimedJob, Failure};
use crate::sources::parse;
use crate::state::AppState;

/// How many chunks go through the embedder in one call. Batched because the
/// model's cost is dominated by per-call overhead, not by the batch size.
const EMBED_BATCH: usize = 32;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let source_id: Uuid = job
        .payload
        .get("source_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid source_id"))?;

    // The source may have been deleted between enqueue and claim.
    let row = sqlx::query(
        "select id, user_id, kind, file_key, url, title, mime, deleted_at
           from sources where id = $1",
    )
    .bind(source_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the source: {error}")))?;

    let Some(row) = row else {
        return Ok(());
    };
    if row
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")
        .is_some()
    {
        return Ok(());
    }

    let user_id: Uuid = row.get("user_id");
    let kind: String = row.get("kind");
    let file_key: Option<String> = row.get("file_key");
    let url: Option<String> = row.get("url");
    let title: String = row.get("title");
    let mime: String = row.get("mime");

    // The user may have downgraded between the request and the claim: the upload
    // checked `UploadSource` against the plan that existed then, and a source the
    // new plan does not allow must not be embedded on the old plan's authority.
    // Permanent, because no retry changes a plan.
    if let Err(error) =
        crate::entitlements::require_for(state, user_id, Feature::IngestSource).await
    {
        let message = error.to_string();
        tracing::info!(%source_id, %user_id, code = error.code(), "ingest refused after a plan change");
        set_status(&state.pool, source_id, "failed", Some(&message)).await?;
        return Err(Failure::permanent(message));
    }

    set_status(&state.pool, source_id, "processing", None).await?;

    match ingest(
        state,
        source_id,
        user_id,
        IngestBody {
            kind: &kind,
            file_key: file_key.as_deref(),
            url: url.as_deref(),
            title: &title,
            mime: &mime,
        },
    )
    .await
    {
        Ok(chunk_count) => {
            // A syllabus adds one step after the chunks exist (A250): the extraction. It
            // rides this job rather than a second kind because the roadmap's § 8 job list is
            // fixed and has no syllabus kind, and it runs here rather than in the route
            // because a whole document is not a route-sized call — the contrast is
            // `create_outline`, which is one short call over selected chunks.
            if kind == SYLLABUS_KIND
                && let Err(failure) = extract_syllabus(state, source_id, user_id).await
            {
                // The draft says what went wrong even while the job is going to be retried,
                // for the same reason the source does: the client is polling it. A failure
                // with no retry left also gives back the eight credits the upload charged —
                // the same two-arm refund the quiz path has (A253).
                if final_attempt(&failure, job.attempts) {
                    fail_draft_and_refund(state, source_id, &failure.message).await;
                } else {
                    mark_draft_failed(&state.pool, source_id, &failure.message).await;
                }
                set_status(&state.pool, source_id, "failed", Some(&failure.message)).await?;
                return Err(failure);
            }

            set_status(&state.pool, source_id, "ready", None).await?;
            tracing::info!(%source_id, chunk_count, "source ingested");
            Ok(())
        }
        Err(failure) => {
            // The status is set here, not by the worker: the source must say what
            // went wrong even while the job is going to be retried, and the final
            // failure has to leave the same readable text.
            set_status(&state.pool, source_id, "failed", Some(&failure.message)).await?;
            // A parse failure that will never succeed is the same charge-with-nothing-
            // delivered the extraction's own failure is, so the refund arm is here too.
            if final_attempt(&failure, job.attempts) {
                fail_draft_and_refund(state, source_id, &failure.message).await;
            } else {
                mark_draft_failed(&state.pool, source_id, &failure.message).await;
            }
            Err(failure)
        }
    }
}

/// Whether this failure is the last one the job gets.
///
/// A transient failure on the last allowed attempt is final in every way that matters to the
/// student: no retry is coming, so the charge must come back. The same predicate the quiz
/// handler uses, kept here as one function because it is asked twice.
fn final_attempt(failure: &Failure, attempts: i32) -> bool {
    !failure.retryable || attempts >= crate::jobs::MAX_ATTEMPTS
}

/// The source kind that carries an extraction step (A250).
pub const SYLLABUS_KIND: &str = "syllabus";

/// The prompt version of the extraction, for `syllabus_drafts.prompt_version`.
const EXTRACTION_MODULE: stvdy_core::prompts::ModuleId = stvdy_core::prompts::ModuleId::SyllabusV1;

/// The model temperature for an extraction: low, because a timetable is a reading of a
/// document rather than a piece of writing.
const EXTRACTION_TEMPERATURE: f32 = 0.1;

/// Generous, because a whole syllabus's meetings, deadlines and exams have to fit in one
/// object. The contract's own bounds (`MAX_MEETINGS` and friends) are what keep the reply
/// finite; this is what keeps the *call* finite.
const EXTRACTION_MAX_TOKENS: u32 = 4000;

/// What the model is told when its first reply was not usable.
const EXTRACTION_CORRECTION: &str = "Your previous reply was not a valid JSON object matching the schema. Reply with only \
     the JSON object, with no prose before or after it and no markdown fences.";

/// The extraction step, for `kind = 'syllabus'` only (A250).
///
/// It rides `ingest_source` because the roadmap's job-kind list is fixed and has no syllabus
/// kind, and it runs here rather than in the route because a whole document is not a
/// route-sized call.
///
/// **Idempotent**: the draft is keyed uniquely on `source_id` (migration `0031`), so a retry
/// `update`s the same row rather than appending a second draft. A reply that does not parse
/// is transient — one retry with a corrective message is worth making — while a missing
/// provider is permanent, because no retry invents one.
///
/// The chunks are read back from the database rather than threaded through from [`ingest`]:
/// the extraction is a separate step with its own failure modes, and passing the text along
/// would make the two inseparable — a syllabus's extraction would then only be testable
/// through the whole parse-and-embed pipeline.
async fn extract_syllabus(state: &AppState, source_id: Uuid, user_id: Uuid) -> Result<(), Failure> {
    let Some(provider) = state.llm.clone() else {
        return Err(Failure::permanent(
            "no model provider is configured on this server (LLM_BASE_URL is unset)",
        ));
    };

    let chunks = crate::chunks::select_for_generation(&state.pool, &[source_id], user_id)
        .await
        .map_err(|error| Failure::transient(format!("loading the syllabus's chunks: {error}")))?;
    if chunks.is_empty() {
        return Err(Failure::permanent(
            "the syllabus has no indexed text to read",
        ));
    }

    let row = sqlx::query("select email, timezone, plan_id from users where id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| Failure::transient(format!("loading the syllabus's owner: {error}")))?
        .ok_or_else(|| Failure::permanent("the syllabus's owner no longer exists"))?;

    let base: stvdy_core::prompts::ChatVars = crate::chat::prompt::base_vars(
        state,
        row.get("email"),
        row.get("timezone"),
        stvdy_core::config::PlanId::parse(row.get("plan_id")),
        false,
        false,
        // A syllabus extraction is always drawn from the student's own document, so the
        // grounding toggle is `true`; it is never read, because task modules compose on v1's
        // trimmed base, which has no `{{sources_only}}`.
        true,
        // A syllabus belongs to a workspace, but the extraction prompt never names one: the
        // document is the context, and the base's `<user_context_data>` block is filled by
        // the module's own values.
        "not set",
    );

    // `semester_context` is deliberately absent: the module declares it optional and its
    // default sentence is the honest value until the upload carries one (A254).
    let composed = stvdy_core::prompts::compose_task(
        EXTRACTION_MODULE,
        &base,
        &std::collections::BTreeMap::new(),
        &chunks,
    )
    .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    let request = stvdy_core::traits::llm::ChatRequest {
        model: String::new(),
        messages: vec![stvdy_core::traits::llm::ChatMsg {
            role: stvdy_core::traits::llm::Role::System,
            content: composed.text.clone(),
        }],
        tools: Vec::new(),
        temperature: EXTRACTION_TEMPERATURE,
        max_tokens: EXTRACTION_MAX_TOKENS,
        json_mode: true,
    };

    let first = provider
        .complete(request.clone())
        .await
        .map_err(|error| Failure::transient(format!("calling the model: {error}")))?;
    let mut usage = first.usage.clone();

    let (file, reply) = match crate::syllabus::parse_extraction(&first.text) {
        Ok(file) => (file, first),
        Err(error) if stvdy_core::generation::is_retryable(&error) => {
            let mut retry = request;
            retry.messages.push(stvdy_core::traits::llm::ChatMsg {
                role: stvdy_core::traits::llm::Role::Assistant,
                content: first.text.clone(),
            });
            retry.messages.push(stvdy_core::traits::llm::ChatMsg {
                role: stvdy_core::traits::llm::Role::User,
                content: EXTRACTION_CORRECTION.to_string(),
            });
            let second = provider
                .complete(retry)
                .await
                .map_err(|error| Failure::transient(format!("retrying the model call: {error}")))?;
            usage.in_tokens += second.usage.in_tokens;
            usage.out_tokens += second.usage.out_tokens;
            usage.provider_cost_usd =
                match (usage.provider_cost_usd, second.usage.provider_cost_usd) {
                    (Some(first), Some(second)) => Some(first + second),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                };
            let file = crate::syllabus::parse_extraction(&second.text).map_err(|error| {
                // The model was handed its own reply and told what was wrong with it, and it
                // failed the same way: a third attempt would be the same call with the same
                // result, so this is permanent. The retry loop would otherwise leave the
                // student's draft `extracting` for the length of the backoff for nothing.
                Failure::permanent(format!(
                    "the model did not return a usable extraction after a retry: {error}"
                ))
            })?;
            (file, second)
        }
        Err(error) => {
            // A parse failure that is not worth retrying (`Empty`, or a `Json`/`Schema` the
            // corrective turn cannot fix) fails the extraction outright. The one retry above
            // is the whole of the retry policy for a reply.
            return Err(Failure::permanent(format!(
                "the model did not return a usable extraction: {error}"
            )));
        }
    };

    let extraction = serde_json::to_value(&file).map_err(|error| {
        Failure::permanent(format!("the extraction is not serialisable: {error}"))
    })?;

    // The `update` is what makes a retry overwrite rather than append, and the unique index
    // on `source_id` is what makes the two statements one row either way.
    sqlx::query(
        "insert into syllabus_drafts
            (user_id, workspace_id, source_id, status, extraction, prompt_version, error)
         select $1, s.workspace_id, s.id, 'ready', $2, $3, null
           from sources s where s.id = $4
         on conflict (source_id) do update
            set status = 'ready', extraction = excluded.extraction,
                prompt_version = excluded.prompt_version, error = null, updated_at = now()",
    )
    .bind(user_id)
    .bind(&extraction)
    .bind(EXTRACTION_MODULE.version())
    .bind(source_id)
    .execute(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("storing the extraction: {error}")))?;

    // The provider's tokens and cost are attached to the import's own `usage_events` row.
    // The id is found rather than carried: the upload's transaction committed before this
    // job was claimed, and a `None` here is a reporting gap rather than a failed import.
    attach_extraction_usage(state, user_id, &usage, EXTRACTION_MODULE.version()).await;

    tracing::info!(
        %source_id,
        meetings = file.meetings.len(),
        deadlines = file.deadlines.len(),
        exams = file.exams.len(),
        reply_chars = reply.text.len(),
        "syllabus extracted"
    );
    Ok(())
}

/// Attaches the extraction's provider usage to the import's `usage_events` row.
///
/// The spend is the upload's and committed minutes at most before this job is claimed, so a
/// short lookback finds it — the same fallback `flashcards::generate::find_usage_event_id`
/// uses for a row that does not carry the id. A miss is logged and the extraction still
/// succeeds: the rows the student paid for are stored, and an accounting gap must not turn
/// into a failed import.
async fn attach_extraction_usage(
    state: &AppState,
    user_id: Uuid,
    usage: &stvdy_core::traits::llm::Usage,
    prompt_version: &str,
) {
    let since = state.clock.now() - chrono::Duration::minutes(5);
    let found = crate::credits::latest_usage_event(
        &state.pool,
        user_id,
        crate::syllabus::SYLLABUS_ACTION,
        since,
    )
    .await;

    let usage_event_id = match found {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "could not find the syllabus import's usage event");
            return;
        }
    };

    if let Err(error) = crate::credits::record_usage(
        &state.pool,
        usage_event_id,
        Some(usage.in_tokens),
        Some(usage.out_tokens),
        usage.provider_cost_usd,
        prompt_version,
    )
    .await
    {
        tracing::error!(%error, "could not attach the extraction's provider usage");
    }
}

/// Marks a source's draft `failed`, when it has one.
///
/// Best-effort by design: a source that is not a syllabus has no draft, and a failure to
/// record the reason must not replace the reason the job is already failing with.
async fn mark_draft_failed(pool: &sqlx::PgPool, source_id: Uuid, error: &str) {
    if let Err(record_error) = sqlx::query(
        "update syllabus_drafts
            set status = 'failed', error = $2, updated_at = now()
          where source_id = $1 and status <> 'confirmed'",
    )
    .bind(source_id)
    .bind(error)
    .execute(pool)
    .await
    {
        tracing::error!(%source_id, %record_error, "could not record the draft's failure");
    }
}

/// Marks the draft failed **and gives back exactly what the upload charged**, once.
///
/// The same shape `generate_quiz::fail_and_refund` uses, for the same reason: the upload
/// charged eight credits before the model was ever called, so a failure with no retry left is
/// a charge with nothing delivered. The guard is `credits_spent > 0`, consumed by the claim
/// that zeroes it — a second call (a retried job or the boot sweep) refunds nothing because
/// there is nothing left to refund.
///
/// The draft is left terminal in every case. When the refund insert cannot be written, an
/// infrastructure failure rather than a business one, the charge is kept (`credits_spent > 0`,
/// "charged and not delivered, refund pending") and the status change still commits, because
/// this is only ever called on the final attempt: a rollback would leave the draft
/// `extracting` with no retry left to resolve it.
/// `credits::refund_pending(RefundTarget::SyllabusDraft)` recovers the charge at the next
/// boot.
async fn fail_draft_and_refund(state: &AppState, source_id: Uuid, message: &str) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %source_id, "could not begin the draft failure transaction");
            return;
        }
    };

    // The charge is read **before** the claim zeroes it: `returning credits_spent` on an
    // update yields the new value, which is 0, and a refund of 0 is no refund at all. The
    // row is locked so two passes cannot both read the same charge.
    let pending: Option<(Uuid, i32)> = sqlx::query_as(
        "select user_id, credits_spent from syllabus_drafts
          where source_id = $1 and credits_spent > 0 and status <> 'confirmed'
            for update skip locked",
    )
    .bind(source_id)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    let Some((user_id, credits)) = pending else {
        // Either the draft was already failed, it never had a charge, or it has been
        // confirmed — a confirmed draft's rows are the student's and must not be touched.
        // Make sure the status is right, then stop.
        if let Err(error) = sqlx::query(
            "update syllabus_drafts
                set status = 'failed', error = $2, updated_at = now()
              where source_id = $1 and status not in ('failed', 'confirmed')",
        )
        .bind(source_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %source_id, "could not mark the draft failed");
        }
        if let Err(error) = tx.commit().await {
            tracing::error!(%error, %source_id, "could not commit the draft failure");
        }
        return;
    };

    if let Err(error) = sqlx::query(
        "update syllabus_drafts
            set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where source_id = $1",
    )
    .bind(source_id)
    .bind(message)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(%error, %source_id, "could not mark the draft failed");
        return;
    }

    // The ledger row is issued inside the same transaction as the claim that zeroed
    // `credits_spent`, so a crash between the two cannot charge a student for an extraction
    // they never received with no retry left to fix it. The refund runs under a savepoint
    // because Postgres aborts a transaction on any failed statement.
    if let Err(error) = sqlx::query("savepoint refund").execute(&mut *tx).await {
        tracing::error!(%error, %source_id, "could not open the refund savepoint");
        return;
    }

    if let Err(error) =
        crate::credits::refund_in(&mut tx, user_id, i64::from(credits), source_id, "refund").await
    {
        tracing::error!(%error, %source_id, "could not refund a failed extraction; the charge stays pending for recovery");

        if let Err(rollback_error) = sqlx::query("rollback to savepoint refund")
            .execute(&mut *tx)
            .await
        {
            tracing::error!(%rollback_error, %source_id, "could not roll back the failed refund insert");
            return;
        }
        if let Err(restore_error) =
            sqlx::query("update syllabus_drafts set credits_spent = $2 where source_id = $1")
                .bind(source_id)
                .bind(credits)
                .execute(&mut *tx)
                .await
        {
            tracing::error!(%restore_error, %source_id, "could not restore the pending charge");
            return;
        }
        if let Err(commit_error) = tx.commit().await {
            tracing::error!(%commit_error, %source_id, "could not commit the draft failure");
        }
        return;
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %source_id, "could not commit the draft failure");
        return;
    }

    tracing::warn!(%source_id, credits, "extraction failed permanently; credits refunded");
}

async fn set_status(
    pool: &sqlx::PgPool,
    source_id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<(), Failure> {
    sqlx::query("update sources set status = $2, error = $3 where id = $1")
        .bind(source_id)
        .bind(status)
        .bind(error)
        .execute(pool)
        .await
        .map_err(|e| Failure::transient(format!("setting the source status: {e}")))?;
    Ok(())
}

/// The source's body, as `ingest` reads it: which shape the parse takes (`kind`), the
/// stored object's key and/or the fetched URL, and the title and mime the parse uses.
///
/// Grouped because the five are the claimed row's body columns and travel together from
/// the one call site; `kind` and `mime` are the pair that decides a parse, and `file_key`
/// and `url` are the pair that says where the bytes are.
struct IngestBody<'a> {
    kind: &'a str,
    file_key: Option<&'a str>,
    url: Option<&'a str>,
    title: &'a str,
    mime: &'a str,
}

async fn ingest(
    state: &AppState,
    source_id: Uuid,
    user_id: Uuid,
    body: IngestBody<'_>,
) -> Result<usize, Failure> {
    let IngestBody {
        kind,
        file_key,
        url,
        title,
        mime,
    } = body;
    let pages = match kind {
        // A syllabus is stored as either a PDF or pasted text — the upload decides which and
        // records it in `sources.mime` — so its parse is the one it was uploaded as. The
        // `kind` alone cannot say which: `syllabus` is one kind with two shapes, exactly as
        // `text` and `web` share a body shape here.
        "syllabus" => {
            let key =
                file_key.ok_or_else(|| Failure::permanent("a syllabus source has no body"))?;
            let bytes =
                state.storage.get(key).await.map_err(|error| {
                    Failure::transient(format!("reading the syllabus: {error}"))
                })?;
            if mime == "application/pdf" {
                let pdfium = crate::integrations::documents::handle().ok_or_else(|| {
                    Failure::permanent(
                        "PDF ingestion is not configured on this server (PDFIUM_LIB_PATH is unset)",
                    )
                })?;
                parse::pdf_pages(&pdfium, &bytes, title).map_err(Failure::permanent)?
            } else {
                let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
                    Failure::permanent("the stored syllabus text is not valid UTF-8")
                })?;
                vec![(None, text)]
            }
        }
        // Pasted text is stored as an object, so both kinds read from storage
        // and there is one path to test. A `web` source is the same shape: the
        // route fetched the page and stored what it read, so the job's job is
        // still just extraction.
        "text" | "web" => {
            let key = file_key.ok_or_else(|| Failure::permanent("a text source has no body"))?;
            let bytes =
                state.storage.get(key).await.map_err(|error| {
                    Failure::transient(format!("reading the source text: {error}"))
                })?;
            let text = String::from_utf8(bytes.to_vec())
                .map_err(|_| Failure::permanent("the stored text is not valid UTF-8"))?;
            vec![(None, text)]
        }
        "pdf" => {
            let key = file_key.ok_or_else(|| Failure::permanent("a pdf source has no file"))?;
            let bytes =
                state.storage.get(key).await.map_err(|error| {
                    Failure::transient(format!("reading the source file: {error}"))
                })?;
            // Extraction failures are permanent by construction: a missing
            // library, a corrupt document and an encrypted PDF all fail the same
            // way on every attempt.
            let pdfium = crate::integrations::documents::handle().ok_or_else(|| {
                Failure::permanent(
                    "PDF ingestion is not configured on this server (PDFIUM_LIB_PATH is unset)",
                )
            })?;
            parse::pdf_pages(&pdfium, &bytes, title).map_err(Failure::permanent)?
        }
        "docx" | "pptx" => {
            let key =
                file_key.ok_or_else(|| Failure::permanent("an office document has no file"))?;
            let bytes =
                state.storage.get(key).await.map_err(|error| {
                    Failure::transient(format!("reading the source file: {error}"))
                })?;

            let pages = if kind == "docx" {
                parse::docx_runs(&bytes)
            } else {
                parse::pptx_slides(&bytes)
            }
            .map_err(Failure::permanent)?;

            // The PDF rendition for the uniform viewer, and it is best-effort by
            // policy (A158): a deployment without LibreOffice still ingests the
            // document, because its text comes from the OOXML itself. What it does
            // not do is stay quiet about it — a source with no preview and no
            // explanation looks like a scanned document.
            match state.config.soffice_bin.as_deref() {
                Some(soffice) => match parse::preview_pdf(soffice, kind, &bytes) {
                    Ok(pdf) => {
                        let preview_key = preview_key(user_id, source_id);
                        match state
                            .storage
                            .put(&preview_key, bytes::Bytes::from(pdf), "application/pdf")
                            .await
                        {
                            Ok(()) => {
                                sqlx::query("update sources set preview_key = $2 where id = $1")
                                    .bind(source_id)
                                    .bind(&preview_key)
                                    .execute(&state.pool)
                                    .await
                                    .map_err(|error| {
                                        Failure::transient(format!(
                                            "recording the preview key: {error}"
                                        ))
                                    })?;
                            }
                            Err(error) => {
                                tracing::warn!(%source_id, %error, "storing the preview failed; the source still ingests");
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            %source_id,
                            %kind,
                            %error,
                            "rendering the PDF preview failed; the source still ingests without one"
                        );
                    }
                },
                None => {
                    tracing::info!(
                        %source_id,
                        %kind,
                        "SOFFICE_BIN is unset; this source ingests without a PDF preview"
                    );
                }
            }

            pages
        }
        // A URL source stores no bytes at all, so the fetch happens here rather
        // than at upload: the request does not block on someone else's server, and
        // a refusal lands on `sources.error`, which the client already shows.
        // **Through the chain**, never a bare `reqwest` call: the chain is what
        // carries the SSRF guard (A18/A34).
        "url" => {
            let url = url.ok_or_else(|| Failure::permanent("a url source has no url"))?;
            let page = state
                .fetcher
                .fetch(url)
                .await
                .map_err(|error| match error {
                    // A refusal or an invalid URL fails identically on every attempt, so
                    // it is permanent; only a provider failure is worth a retry.
                    FetchError::Refused(reason) | FetchError::Invalid(reason) => {
                        Failure::permanent(reason)
                    }
                    FetchError::Provider(reason) => {
                        Failure::transient(format!("fetching the page: {reason}"))
                    }
                })?;
            vec![(None, page.text)]
        }
        other => {
            return Err(Failure::permanent(format!(
                "unsupported source kind {other}"
            )));
        }
    };

    let total_chars: usize = pages.iter().map(|(_, text)| text.len()).sum();
    if pages.iter().all(|(_, text)| text.trim().is_empty()) {
        return Err(Failure::permanent(
            "no extractable text: the document may be a scan",
        ));
    }
    tracing::info!(%source_id, pages = pages.len(), total_chars, "text extracted");

    // Chunk against the model's own tokenizer, not a character estimate: the
    // 480/512 pair in A2 is the *model's* budget, and a heuristic that
    // undercounts lets a chunk exceed `max_length`, at which point fastembed
    // truncates it silently and the tail of the chunk becomes unretrievable.
    // Indonesian is where that bites, because its words are longer.
    let cfg = stvdy_core::chunk::ChunkConfig::default();
    let count = |text: &str| state.embedder.token_count(text);

    let mut pending: Vec<(Option<i32>, i32, String)> = Vec::new();
    for (page, text) in &pages {
        for (idx, chunk) in stvdy_core::chunk::chunk(text, &cfg, &count)
            .into_iter()
            .enumerate()
        {
            pending.push((*page, idx as i32, chunk));
        }
    }

    // Replace any previous chunks: a retry must not double the corpus.
    sqlx::query("delete from chunks where source_id = $1")
        .bind(source_id)
        .execute(&state.pool)
        .await
        .map_err(|e| Failure::transient(format!("clearing old chunks: {e}")))?;

    let model_id = state.embedder.model_id().to_string();
    let mut inserted = 0usize;

    for batch in pending.chunks(EMBED_BATCH) {
        let mut to_embed: Vec<String> = Vec::with_capacity(batch.len());
        let mut reused: Vec<Option<Vec<f32>>> = Vec::with_capacity(batch.len());

        for (_, _, content) in batch {
            let hash = content_hash(content);
            // Dedupe is per user: the index on (embedder, content_hash) is
            // deliberately non-unique (migration 0007), so one account's vectors
            // are never reused for another's copy of the same textbook.
            let existing: Option<Vec<f32>> = sqlx::query_scalar(
                "select c.embedding::float4[]
                   from chunks c
                   join sources s on s.id = c.source_id
                  where s.user_id = $1 and c.embedder = $2 and c.content_hash = $3
                  limit 1",
            )
            .bind(user_id)
            .bind(&model_id)
            .bind(&hash)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| Failure::transient(format!("looking up a cached embedding: {e}")))?;

            match existing {
                Some(vector) => reused.push(Some(vector)),
                None => {
                    reused.push(None);
                    to_embed.push(content.clone());
                }
            }
        }

        let mut fresh = if to_embed.is_empty() {
            Vec::new()
        } else {
            state
                .embedder
                .embed(&to_embed)
                .await
                .map_err(|error| Failure::transient(format!("embedding failed: {error}")))?
        };
        let mut fresh = fresh.drain(..);

        for ((page, idx, content), cached) in batch.iter().zip(reused) {
            let vector = match cached {
                Some(vector) => vector,
                None => fresh.next().ok_or_else(|| {
                    Failure::permanent("embedder returned fewer vectors than requested")
                })?,
            };
            let hash = content_hash(content);

            sqlx::query(
                "insert into chunks (source_id, page, idx, content, content_hash, embedder, embedding)
                 values ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(source_id)
            .bind(page)
            .bind(idx)
            .bind(content)
            .bind(&hash)
            .bind(&model_id)
            .bind(pgvector::Vector::from(vector))
            .execute(&state.pool)
            .await
            .map_err(|e| Failure::transient(format!("inserting a chunk: {e}")))?;
            inserted += 1;
        }
    }

    if inserted == 0 {
        return Err(Failure::permanent("chunking produced no chunks"));
    }
    Ok(inserted)
}

/// The dedupe key: SHA-256 of the exact chunk text, which is what the index on
/// `(embedder, content_hash)` compares.
fn content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// The object key of a source's PDF rendition.
///
/// Prefixed by user for the same reason `sources::routes::object_key` is: a key can
/// never be guessed into another account's prefix. `preview.pdf` rather than a
/// second id, because there is exactly one rendition per source and a deterministic
/// key means a retry overwrites instead of accumulating.
fn preview_key(user_id: Uuid, source_id: Uuid) -> String {
    format!("users/{user_id}/sources/{source_id}.preview.pdf")
}
