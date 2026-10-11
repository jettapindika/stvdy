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
        "select id, user_id, kind, file_key, url, title, deleted_at
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
        &kind,
        file_key.as_deref(),
        url.as_deref(),
        &title,
    )
    .await
    {
        Ok(chunk_count) => {
            set_status(&state.pool, source_id, "ready", None).await?;
            tracing::info!(%source_id, chunk_count, "source ingested");
            Ok(())
        }
        Err(failure) => {
            // The status is set here, not by the worker: the source must say what
            // went wrong even while the job is going to be retried, and the final
            // failure has to leave the same readable text.
            set_status(&state.pool, source_id, "failed", Some(&failure.message)).await?;
            Err(failure)
        }
    }
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

async fn ingest(
    state: &AppState,
    source_id: Uuid,
    user_id: Uuid,
    kind: &str,
    file_key: Option<&str>,
    url: Option<&str>,
    title: &str,
) -> Result<usize, Failure> {
    let pages = match kind {
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
