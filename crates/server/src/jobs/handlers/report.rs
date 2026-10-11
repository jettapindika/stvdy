//! `generate_report_section`: the job shell around `reports::generate`.
//!
//! Owns the transaction boundary (the section's text plus the report's status), the
//! permanent-failure path (status, readable error, refund), and the give-up case: a transient
//! failure on the last allowed attempt is final, and the worker's `mark_failed` is the only
//! place that knows it, so the check is here.
//!
//! The one thing this shell does that the quiz's does not is **replace a section**: the
//! report's `sections_md` is index-aligned with its outline, and regenerating section 3
//! rewrites index 3 and nothing else. `body_md` is then re-derived from the whole array, so
//! the document and the per-section view cannot disagree and a reorder cannot leave the two
//! in different orders.

use uuid::Uuid;

use crate::jobs::{self, ClaimedJob, Failure};
use crate::reports::generate::{self, ReportRow};
use crate::reports::routes::compose_body_md;
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let report_id: Uuid = job
        .payload
        .get("report_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid report_id"))?;

    let index = job
        .payload
        .get("index")
        .and_then(serde_json::Value::as_u64)
        .and_then(|raw| usize::try_from(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid section index"))?;

    let report = match load_report(state, report_id).await? {
        Some(report) => report,
        // Deleted between enqueue and claim: nothing to do, and not an error.
        None => return Ok(()),
    };

    // Idempotent, and specific to *this* section: a retried job whose first attempt already
    // wrote the section must not write it twice, and a report whose in-flight index has moved
    // on — a second device regenerating another section — must not be overwritten by this
    // attempt. Both facts are on the row.
    if report.status != "generating" || report.generating_index != Some(index as i32) {
        return Ok(());
    }

    match generate::generate_section(state, &report).await {
        Ok(generation) => {
            store_section(state, &report, index, generation).await?;
            Ok(())
        }
        Err(failure) => {
            // A transient failure on the last attempt is final in every way that matters to
            // the user: no retry is coming, so the report must stop saying `generating` and
            // the credits must come back.
            let final_attempt = !failure.retryable || job.attempts >= jobs::MAX_ATTEMPTS;
            if final_attempt {
                fail_and_refund(
                    state,
                    report.id,
                    report.user_id,
                    report.credits_spent,
                    &failure.message,
                )
                .await;
            }
            Err(failure)
        }
    }
}

/// The report a job runs for, joined to its workspace so the prompt can name it.
async fn load_report(state: &AppState, report_id: Uuid) -> Result<Option<ReportRow>, Failure> {
    let row = sqlx::query(
        "select r.id, r.user_id, r.title, r.status, r.error, r.outline, r.body_md,
                r.sections_md, r.generating_index, r.source_ids, r.style, r.length,
                r.credits_spent, r.usage_event_id, r.created_at,
                w.name as workspace_name
           from reports r
           join workspaces w on w.id = r.workspace_id
          where r.id = $1 and r.deleted_at is null",
    )
    .bind(report_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the report: {error}")))?;

    row.as_ref().map(generate::row_from).transpose()
}

/// Writes the section's text, re-derives the document, and flips the status, in one
/// transaction.
async fn store_section(
    state: &AppState,
    report: &ReportRow,
    index: usize,
    generation: generate::SectionGeneration,
) -> Result<(), Failure> {
    let mut texts = report.sections_md.clone();
    if texts.len() < report.outline.sections.len() {
        texts.resize(report.outline.sections.len(), None);
    }
    if let Some(slot) = texts.get_mut(index) {
        *slot = Some(generation.body_md.clone());
    } else {
        // The outline was shortened while this job was in flight, so there is no section to
        // write. Permanent: the index cannot come back, and retrying would write nothing
        // four more times.
        return Err(Failure::permanent(
            "the report's outline no longer has the section this job was queued for",
        ));
    }

    let headings: Vec<String> = report
        .outline
        .sections
        .iter()
        .map(|section| section.heading.clone())
        .collect();
    let body_md = compose_body_md(&headings, &texts);

    // Every section written means the report is `ready`; anything else is `draft`, because
    // the client polls while a report is `generating` and a status left at `generating` would
    // poll forever.
    let complete = texts.iter().all(Option::is_some);
    let status = if complete { "ready" } else { "draft" };

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| Failure::transient(format!("beginning the section write: {error}")))?;

    // The status flip is the claim: it is the only thing that decides whether this job writes
    // the section, so two claims for one report cannot both write.
    //
    // Zeroing `credits_spent` here is what makes `credits_spent > 0` mean "charged and not
    // yet delivered" for the refund guard: without it a report that was paid for and
    // delivered still carries its charge, and the loser of a claim race would satisfy the
    // guard on the winner's charge and refund a generation the user received.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update reports
            set status = $3, error = null, sections_md = $4, body_md = $5,
                generating_index = null, credits_spent = 0, updated_at = now()
          where id = $1 and status = 'generating' and generating_index = $2
         returning id",
    )
    .bind(report.id)
    .bind(index as i32)
    .bind(status)
    .bind(serde_json::Value::from(texts))
    .bind(&body_md)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| Failure::transient(format!("claiming the report: {error}")))?;

    if claimed.is_none() {
        // Someone else already finished or failed this section. Nothing to write, and not an
        // error: the work is done, just not by this attempt.
        return Ok(());
    }

    tx.commit()
        .await
        .map_err(|error| Failure::transient(format!("committing the section: {error}")))?;

    if let Some(usage_event_id) = generate::find_usage_event_id(state, report).await {
        // Attached after the section exists, because the token counts and the cost only exist
        // once the provider has answered, and the row was created before that so the credit
        // is spent whether or not the model replies.
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
            // A usage row that loses its cost is a reporting gap, not a user-visible failure:
            // the section is stored and paid for, so the job is not failed.
            tracing::error!(%error, %usage_event_id, "could not attach provider usage");
        }
    }

    Ok(())
}

/// Marks the report failed and gives back exactly what was charged, once.
///
/// The guard is `credits_spent > 0`, consumed by the claim that zeroes it: a second call (a
/// retried job, the enqueue-failure path in the route, or the boot sweep) refunds nothing
/// because there is nothing left to refund.
///
/// `generating_index` is **kept**: it names the section that failed, which is what lets the
/// screen point at that row instead of saying "the report failed" about a document whose
/// other sections are perfectly good. The status is what stops the client polling, and it is
/// terminal, so nothing shows a spinner for work that will never arrive. The report is left
/// terminal in every case, for the same reason `generate_quiz::fail_and_refund` is.
pub async fn fail_and_refund(
    state: &AppState,
    report_id: Uuid,
    user_id: Uuid,
    credits: i32,
    message: &str,
) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %report_id, "could not begin the report failure transaction");
            return;
        }
    };

    let claimed: Option<i32> = sqlx::query_scalar(
        "update reports
            set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where id = $1 and credits_spent > 0
         returning credits_spent",
    )
    .bind(report_id)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if claimed.is_none() {
        // Either the report was already failed, or it never had a charge. Make sure the
        // status is right either way, then stop.
        if let Err(error) = sqlx::query(
            "update reports
                set status = 'failed', error = $2, updated_at = now()
              where id = $1 and status = 'generating'",
        )
        .bind(report_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %report_id, "could not mark the report failed");
        }
    }

    if claimed.is_some() {
        // The ledger row is issued inside the same transaction as the claim that zeroed
        // `credits_spent`, so a crash between the two cannot charge a user for a section they
        // never received with no retry left to fix it. The refund runs under a savepoint
        // because Postgres aborts a transaction on any failed statement.
        if let Err(error) = sqlx::query("savepoint refund").execute(&mut *tx).await {
            tracing::error!(%error, %report_id, "could not open the refund savepoint");
            return;
        }

        if let Err(error) =
            crate::credits::refund_in(&mut tx, user_id, i64::from(credits), report_id, "refund")
                .await
        {
            // A refund that cannot be written is an infrastructure failure, and the job that
            // reached here has no retry left. The status change is committed with
            // `credits_spent` restored to the charged amount: the row is terminal, and the
            // boot sweep recovers the charge.
            tracing::error!(%error, %report_id, "could not refund a failed generation; the charge stays pending for recovery");

            if let Err(rollback_error) = sqlx::query("rollback to savepoint refund")
                .execute(&mut *tx)
                .await
            {
                tracing::error!(%rollback_error, %report_id, "could not roll back the failed refund insert");
                return;
            }
            if let Err(restore_error) =
                sqlx::query("update reports set credits_spent = $2 where id = $1")
                    .bind(report_id)
                    .bind(credits)
                    .execute(&mut *tx)
                    .await
            {
                tracing::error!(%restore_error, %report_id, "could not restore the pending charge");
                return;
            }
            if let Err(commit_error) = tx.commit().await {
                tracing::error!(%commit_error, %report_id, "could not commit the report failure");
                return;
            }
            return;
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %report_id, "could not commit the report failure");
        return;
    }

    if claimed.is_some() {
        tracing::warn!(%report_id, credits, "generation failed permanently; credits refunded");
    }
}
