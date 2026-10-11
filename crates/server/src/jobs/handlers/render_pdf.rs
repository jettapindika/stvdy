//! `render_cheatsheet_pdf`: the print step, as its own job.
//!
//! Two jobs rather than one because the roadmap's job table has both, and because the
//! separation is what makes a browser problem survivable: the blocks are the document, the
//! HTML is a pure function of them, and the print is the only step that needs Chromium. A
//! print that fails leaves a `ready` cheatsheet with its document stored, which
//! `POST /cheatsheets/:id/re-render` recovers from — the opposite arrangement would let a
//! failed browser launch destroy a generation the student paid for.
//!
//! # What it never touches
//!
//! `status` and `credits_spent`. The generation succeeded and was delivered; this job's
//! failure is recorded in `error` and nothing else, because a cheatsheet that is `ready` with
//! a readable error is the truth: the document exists and its print does not.

use sqlx::Row;
use uuid::Uuid;

use crate::cheatsheets::generate::object_key;
use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let cheatsheet_id: Uuid = job
        .payload
        .get("cheatsheet_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid cheatsheet_id"))?;

    let row = sqlx::query(
        "select id, user_id, title, html_key, status from cheatsheets
          where id = $1 and deleted_at is null",
    )
    .bind(cheatsheet_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the cheatsheet: {error}")))?;

    let Some(row) = row else {
        // Deleted between enqueue and claim: nothing to do, and not an error.
        return Ok(());
    };

    let user_id: Uuid = row.get("user_id");
    let title: String = row.get("title");
    let status: String = row.get("status");
    let html_key: Option<String> = row.get("html_key");

    // The document must exist to be printed. A cheatsheet that is not `ready` has no HTML,
    // and this job was enqueued after the generation committed, so a missing key is a
    // permanent inconsistency rather than a race.
    let Some(html_key) = html_key else {
        return Err(Failure::permanent(
            "the cheatsheet has no rendered HTML to print",
        ));
    };

    // A `failed` generation must not be printed: its blocks were never stored, so whatever
    // HTML exists belongs to an earlier document. This is the one guard that keeps a retried
    // job from resurrecting a failed row's artifacts.
    if status == "failed" {
        return Ok(());
    }

    let Some(renderer) = state.renderer.clone() else {
        // No browser on this deployment. Permanent: no retry will install one, and five
        // attempts with backoff would burn the queue to be told the same thing. The message
        // is what the student sees, so it names the remedy rather than the cause.
        return Err(Failure::permanent(
            "no Chromium renderer is configured on this server, so the PDF cannot be printed; \
             the cheatsheet itself is ready and can be re-rendered once one is available",
        ));
    };

    let html = state
        .storage
        .get(&html_key)
        .await
        .map_err(|error| Failure::transient(format!("reading the rendered HTML: {error}")))?;
    let html = String::from_utf8(html.to_vec())
        .map_err(|error| Failure::permanent(format!("the rendered HTML is not UTF-8: {error}")))?;

    // `Renderer::render` is blocking: it drives a browser over CDP, and running it on an
    // async worker thread would stall every other job that thread is holding.
    let running_header = title.clone();
    let pdf = tokio::task::spawn_blocking(move || renderer.render(&html, &running_header))
        .await
        .map_err(|error| Failure::transient(format!("the render task failed: {error}")))?;

    let pdf = match pdf {
        Ok(pdf) => pdf,
        Err(error) => {
            // The failure is recorded where the student can read it, and the status is left
            // alone: the document exists, only its print does not.
            record_error(state, cheatsheet_id, &error.to_string()).await;
            return Err(Failure::transient(format!(
                "printing the cheatsheet: {error}"
            )));
        }
    };

    let pdf_key = object_key(user_id, cheatsheet_id, "document", "pdf");
    state
        .storage
        .put(
            &pdf_key,
            bytes::Bytes::from(pdf.bytes.clone()),
            "application/pdf",
        )
        .await
        .map_err(|error| Failure::transient(format!("storing the PDF: {error}")))?;

    // `page_count` is the renderer's own count, never a number this pipeline guessed: the
    // footer's `Halaman N dari M` is printed from it, so the stored value and the printed one
    // must be the same value.
    sqlx::query(
        "update cheatsheets
            set pdf_key = $2, page_count = $3, error = null, updated_at = now()
          where id = $1",
    )
    .bind(cheatsheet_id)
    .bind(&pdf_key)
    .bind(pdf.page_count as i32)
    .execute(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("recording the PDF: {error}")))?;

    tracing::info!(
        %cheatsheet_id,
        pages = pdf.page_count,
        "printed a cheatsheet"
    );

    Ok(())
}

/// Records a readable failure on the row, without touching its status or its charge.
///
/// Best-effort by design: this runs on the path where something is already wrong, and a
/// failure to record it must not replace a readable error with an opaque one.
async fn record_error(state: &AppState, cheatsheet_id: Uuid, message: &str) {
    if let Err(error) =
        sqlx::query("update cheatsheets set error = $2, updated_at = now() where id = $1")
            .bind(cheatsheet_id)
            .bind(message)
            .execute(&state.pool)
            .await
    {
        tracing::error!(%error, %cheatsheet_id, "could not record the print failure");
    }
}

/// The print job's kind, named once so the enqueue sites and the dispatcher cannot disagree.
pub const KIND: &str = "render_cheatsheet_pdf";

/// True when a print can be attempted at all.
///
/// Used by the enqueue sites so a deployment without a browser does not queue a job that is
/// guaranteed to fail: the row keeps its readable error and the re-render route remains the
/// way to retry once a renderer exists.
pub fn is_available(state: &AppState) -> bool {
    state.renderer.is_some()
}
