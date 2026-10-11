//! `generate_cheatsheet`: the job shell around `cheatsheets::generate`.
//!
//! Owns the transaction boundary (the artifacts plus the status change), the
//! permanent-failure path (status, readable error, refund), and the give-up case: a
//! transient failure on the last allowed attempt is final, and the worker's `mark_failed` is
//! the only place that knows it, so the check is here.
//!
//! It is also where the pipeline's three artifacts are written, in the order that makes a
//! failure survivable:
//!
//! 1. the blocks (the document) and the HTML, in one transaction with `status = 'ready'`;
//! 2. then, and separately, the print job.
//!
//! A print that fails therefore leaves a `ready` cheatsheet with its blocks and its HTML
//! stored, which is the state `POST /cheatsheets/:id/re-render` exists to recover from. The
//! reverse order would make a browser problem destroy a paid-for generation.

use bytes::Bytes;
use uuid::Uuid;

use stvdy_core::render::blocks::parse_blocks;

use crate::cheatsheets::generate::{self, CheatsheetRow};
use crate::jobs::{self, ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let cheatsheet_id: Uuid = job
        .payload
        .get("cheatsheet_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid cheatsheet_id"))?;

    let cheatsheet = match load_cheatsheet(state, cheatsheet_id).await? {
        Some(cheatsheet) => cheatsheet,
        // Deleted between enqueue and claim: nothing to do, and not an error.
        None => return Ok(()),
    };

    // Idempotent: a retried job whose first attempt already stored the document must not
    // store a second one, and a cheatsheet the enqueue path already failed must not be
    // resurrected into `ready`. The status is the marker.
    if cheatsheet.status != "generating" {
        return Ok(());
    }

    match generate::generate(state, &cheatsheet).await {
        Ok(generation) => {
            store_document(state, &cheatsheet, generation).await?;
            // Enqueued after the document is committed, so a failure here leaves a `ready`
            // cheatsheet whose PDF is missing rather than a document nobody can print. A
            // failure to *enqueue* is logged rather than failing the job: the generation
            // succeeded, and the print can be re-requested from the route.
            if let Err(error) = jobs::enqueue(
                &state.pool,
                &state.jobs,
                "render_cheatsheet_pdf",
                serde_json::json!({ "cheatsheet_id": cheatsheet_id }),
                None,
            )
            .await
            {
                tracing::error!(%error, %cheatsheet_id, "could not enqueue the print job");
            }
            Ok(())
        }
        Err(failure) => {
            // A transient failure on the last attempt is final in every way that matters to
            // the user: no retry is coming, so the cheatsheet must stop saying `generating`
            // and the credits must come back.
            let final_attempt = !failure.retryable || job.attempts >= jobs::MAX_ATTEMPTS;
            if final_attempt {
                fail_and_refund(
                    state,
                    cheatsheet.id,
                    cheatsheet.user_id,
                    cheatsheet.credits_spent,
                    &failure.message,
                )
                .await;
            }
            Err(failure)
        }
    }
}

/// The cheatsheet a job runs for, joined to its workspace so the prompt can name it.
///
/// Public because the re-render route loads the same row: the print path and the re-render
/// path must read the document's identity the same way, or the two would build different
/// covers for one document.
pub async fn load_cheatsheet(
    state: &AppState,
    cheatsheet_id: Uuid,
) -> Result<Option<CheatsheetRow>, Failure> {
    let row = sqlx::query(
        "select c.id, c.user_id, c.title, c.scope, c.exam_date, c.depth, c.status,
                c.source_ids, c.credits_spent, c.usage_event_id, c.created_at,
                w.name as workspace_name
           from cheatsheets c
           join workspaces w on w.id = c.workspace_id
          where c.id = $1 and c.deleted_at is null",
    )
    .bind(cheatsheet_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the cheatsheet: {error}")))?;

    Ok(row.as_ref().map(generate::row_from))
}

/// Stores the blocks, the rendered HTML and the status change, in one transaction.
///
/// The two artifacts and the status are one fact — "this document exists and is readable" —
/// so a crash between them cannot leave a `ready` row whose blocks are missing. The objects
/// are written *before* the transaction opens, because object storage has no transaction to
/// join: an object written for a generation that then fails is an orphan, which is a cleanup
/// job's problem, whereas a row that points at an object that was never written is a 500 in
/// front of a student.
async fn store_document(
    state: &AppState,
    cheatsheet: &CheatsheetRow,
    generation: generate::Generation,
) -> Result<(), Failure> {
    let blocks_key = generate::object_key(cheatsheet.user_id, cheatsheet.id, "blocks", "json");

    let bytes = generation.document.to_bytes().map_err(|error| {
        Failure::permanent(format!(
            "the document artifact is not serialisable: {error}"
        ))
    })?;
    state
        .storage
        .put(&blocks_key, Bytes::from(bytes), "application/json")
        .await
        .map_err(|error| Failure::transient(format!("storing the blocks: {error}")))?;

    let (html_key, _rendered) =
        generate::render_and_store(state, cheatsheet, &generation.blocks, &generation.labels)
            .await?;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| Failure::transient(format!("beginning the store: {error}")))?;

    // The status flip is the claim: it is the only thing that decides whether this job
    // writes the document, so two claims for one cheatsheet cannot both write.
    //
    // Zeroing `credits_spent` here is what makes `credits_spent > 0` mean "charged and not
    // yet delivered" for the refund guard: without it a cheatsheet that was paid for and
    // delivered still carries its charge, and the loser of a claim race would satisfy the
    // guard on the winner's charge and refund a generation the user received.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update cheatsheets
            set status = 'ready', error = null, blocks_key = $2, html_key = $3,
                template_version = $4, credits_spent = 0, updated_at = now()
          where id = $1 and status = 'generating'
         returning id",
    )
    .bind(cheatsheet.id)
    .bind(&blocks_key)
    .bind(&html_key)
    .bind(generate::template_version())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| Failure::transient(format!("claiming the cheatsheet: {error}")))?;

    if claimed.is_none() {
        // Someone else already finished or failed this cheatsheet. Nothing to write, and
        // not an error: the work is done, just not by this attempt.
        return Ok(());
    }

    tx.commit()
        .await
        .map_err(|error| Failure::transient(format!("committing the cheatsheet: {error}")))?;

    if let Some(usage_event_id) = cheatsheet.usage_event_id {
        // Attached after the document exists, because the token counts and the cost only
        // exist once the provider has answered, and the row was created before that so the
        // credit is spent whether or not the model replies.
        if let Err(error) = crate::credits::record_usage(
            &state.pool,
            Some(usage_event_id),
            Some(generation.in_tokens),
            Some(generation.out_tokens),
            generation.provider_cost_usd,
            &generation.prompt_version,
        )
        .await
        {
            // A usage row that loses its cost is a reporting gap, not a user-visible
            // failure: the document is stored and paid for, so the job is not failed.
            tracing::error!(%error, %usage_event_id, "could not attach provider usage");
        }
    }

    Ok(())
}

/// Marks the cheatsheet failed and gives back exactly what was charged, once.
///
/// The guard is `credits_spent > 0`, consumed by the claim that zeroes it: a second call (a
/// retried job, the enqueue-failure path in the route, or the boot sweep) refunds nothing
/// because there is nothing left to refund. The cheatsheet is left terminal in every case,
/// for the same reason `generate_quiz::fail_and_refund` is: this is only ever called on the
/// final attempt, so a rollback would leave the row `generating` with no retry left to
/// resolve it.
pub async fn fail_and_refund(
    state: &AppState,
    cheatsheet_id: Uuid,
    user_id: Uuid,
    credits: i32,
    message: &str,
) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %cheatsheet_id, "could not begin the cheatsheet failure transaction");
            return;
        }
    };

    let claimed: Option<i32> = sqlx::query_scalar(
        "update cheatsheets set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where id = $1 and credits_spent > 0
         returning credits_spent",
    )
    .bind(cheatsheet_id)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if claimed.is_none() {
        // Either the cheatsheet was already failed, or it never had a charge. Make sure the
        // status is right either way, then stop.
        if let Err(error) = sqlx::query(
            "update cheatsheets set status = 'failed', error = $2, updated_at = now()
              where id = $1 and status = 'generating'",
        )
        .bind(cheatsheet_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %cheatsheet_id, "could not mark the cheatsheet failed");
        }
    }

    if claimed.is_some() {
        // The ledger row is issued inside the same transaction as the claim that zeroed
        // `credits_spent`, so a crash between the two cannot charge a user for a document
        // they never received with no retry left to fix it. The refund runs under a
        // savepoint because Postgres aborts a transaction on any failed statement.
        if let Err(error) = sqlx::query("savepoint refund").execute(&mut *tx).await {
            tracing::error!(%error, %cheatsheet_id, "could not open the refund savepoint");
            return;
        }

        if let Err(error) = crate::credits::refund_in(
            &mut tx,
            user_id,
            i64::from(credits),
            cheatsheet_id,
            "refund",
        )
        .await
        {
            // A refund that cannot be written is an infrastructure failure, and the job that
            // reached here has no retry left. The status change is committed with
            // `credits_spent` restored to the charged amount: the row is terminal, and the
            // boot sweep recovers the charge.
            tracing::error!(%error, %cheatsheet_id, "could not refund a failed generation; the charge stays pending for recovery");

            if let Err(rollback_error) = sqlx::query("rollback to savepoint refund")
                .execute(&mut *tx)
                .await
            {
                tracing::error!(%rollback_error, %cheatsheet_id, "could not roll back the failed refund insert");
                return;
            }
            if let Err(restore_error) =
                sqlx::query("update cheatsheets set credits_spent = $2 where id = $1")
                    .bind(cheatsheet_id)
                    .bind(credits)
                    .execute(&mut *tx)
                    .await
            {
                tracing::error!(%restore_error, %cheatsheet_id, "could not restore the pending charge");
                return;
            }
            if let Err(commit_error) = tx.commit().await {
                tracing::error!(%commit_error, %cheatsheet_id, "could not commit the cheatsheet failure");
                return;
            }
            return;
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %cheatsheet_id, "could not commit the cheatsheet failure");
        return;
    }

    if claimed.is_some() {
        tracing::warn!(%cheatsheet_id, credits, "generation failed permanently; credits refunded");
    }
}

/// Reads a stored `blocks_key` back into the document.
///
/// Shared by the print job and the re-render route so the two cannot disagree about what a
/// stored document is: one parses the bytes, and both render the result through the same
/// pure function.
pub async fn load_stored_blocks(
    state: &AppState,
    key: &str,
) -> Result<stvdy_core::render::blocks::Blocks, Failure> {
    let bytes = state
        .storage
        .get(key)
        .await
        .map_err(|error| Failure::transient(format!("reading the stored blocks: {error}")))?;

    let text = std::str::from_utf8(&bytes)
        .map_err(|error| Failure::permanent(format!("the stored blocks are not UTF-8: {error}")))?;

    parse_blocks(text)
        .map_err(|error| Failure::permanent(format!("the stored blocks no longer parse: {error}")))
}

/// The `blocks_key` of a cheatsheet, or a permanent failure naming the state.
pub async fn blocks_key_of(state: &AppState, cheatsheet_id: Uuid) -> Result<String, Failure> {
    let key: Option<String> = sqlx::query_scalar(
        "select blocks_key from cheatsheets where id = $1 and deleted_at is null",
    )
    .bind(cheatsheet_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the blocks key: {error}")))?
    .flatten();

    key.ok_or_else(|| {
        Failure::permanent("the cheatsheet has no stored blocks, so there is nothing to render")
    })
}
