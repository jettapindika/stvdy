//! The polling worker.
//!
//! One task per process. It claims a job, runs its handler, and records the
//! outcome. A handler that panics is caught so the job row is retried rather
//! than left `running`: the worker is the only thing that can un-park work.

use std::panic::AssertUnwindSafe;

use futures::FutureExt;

use crate::jobs::{self, ClaimedJob};
use crate::state::AppState;

/// Spawns the worker. The `JoinHandle` is returned so `main` can keep it alive
/// and tests can abort it.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    let notifier = state.jobs.notifier();
    tokio::spawn(async move {
        tracing::info!("job worker started");
        loop {
            match jobs::claim(&state.pool).await {
                Ok(Some(job)) => run_one(&state, job).await,
                Ok(None) => {
                    // Nothing due: wait for a wake-up or the poll interval,
                    // whichever comes first. This is what lets a `run_at` in the
                    // future fire without a busy loop.
                    tokio::select! {
                        _ = notifier.notified() => {}
                        _ = tokio::time::sleep(jobs::POLL_INTERVAL) => {}
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "claiming a job failed");
                    tokio::time::sleep(jobs::POLL_INTERVAL).await;
                }
            }
        }
    })
}

async fn run_one(state: &AppState, job: ClaimedJob) {
    let started = std::time::Instant::now();
    let outcome = AssertUnwindSafe(dispatch(state, &job))
        .catch_unwind()
        .await
        .map_err(|_| jobs::Failure::transient("handler panicked"))
        .and_then(|result| result);

    match outcome {
        Ok(()) => {
            if let Err(error) = jobs::mark_done(&state.pool, job.id).await {
                tracing::error!(%error, job_id = %job.id, "marking a job done failed");
            }
            tracing::info!(
                job_id = %job.id,
                kind = %job.kind,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "job finished"
            );
        }
        Err(failure) => {
            tracing::warn!(
                job_id = %job.id,
                kind = %job.kind,
                retryable = failure.retryable,
                error = %failure,
                "job failed"
            );
            match jobs::mark_failed(&state.pool, job.id, job.attempts, &failure).await {
                // A retry was scheduled with a delay, so nudge the loop rather
                // than waiting for the poll interval to notice.
                Ok(true) => state.jobs.wake(),
                Ok(false) => {}
                Err(mark_error) => {
                    tracing::error!(%mark_error, job_id = %job.id, "recording a job failure failed")
                }
            }
        }
    }
}

/// An unknown `kind` is a failed job with a readable error, never a panic: the
/// column is free text and a stale row must not take the worker down.
async fn dispatch(state: &AppState, job: &ClaimedJob) -> Result<(), jobs::Failure> {
    match job.kind.as_str() {
        "ingest_source" => crate::jobs::handlers::ingest::run(state, job).await,
        "generate_flashcards" => crate::jobs::handlers::generate::run(state, job).await,
        "generate_quiz" => crate::jobs::handlers::generate_quiz::run(state, job).await,
        // The report's two steps are asymmetric on purpose: the outline is generated in its
        // route (the roadmap's job table has no outline kind), so only the section is here.
        "generate_report_section" => crate::jobs::handlers::report::run(state, job).await,
        "generate_cheatsheet" => crate::jobs::handlers::cheatsheet::run(state, job).await,
        // The period's allowance, and the rollover that precedes it. Separate kinds
        // because they fail differently: a grant that cannot be written is retryable,
        // while the rollover is idempotent and cheap to repeat.
        "monthly_grant" => crate::jobs::handlers::grants::run(state, job).await,
        "reset_counters" => crate::jobs::handlers::counters::run(state, job).await,
        // The daily change-log compaction: a global singleton, unlike the per-account
        // boundary jobs above (A184).
        "compact_change_log" => crate::jobs::handlers::compact::run(state, job).await,
        // Separate from `generate_cheatsheet` so a failed print cannot discard a
        // successfully generated document: the blocks are the document, and a print is
        // retryable and free.
        "render_cheatsheet_pdf" => crate::jobs::handlers::render_pdf::run(state, job).await,
        // One connection, one pass (A238). The payload names the connection rather than the
        // account, because two Google accounts for one student are two rows.
        "sync_calendar" => crate::jobs::handlers::calendar::run(state, job).await,
        // The planner's one model call (A253). Enqueued by `POST /goals/:id/plan` and
        // `POST /plan/replan`, never scheduled: a plan is something the student asks for.
        "plan_goal" => crate::jobs::handlers::plan::run(state, job).await,
        // The two payment passes (A301). Both are singletons with a `dedupe_key`, so a
        // restart or a second instance cannot double-run them, and both call the *same*
        // `billing::apply::apply_paid` the webhook does — one code path, so a payment
        // recovered by polling and one recovered by a webhook cannot differ.
        "poll_payment" => crate::jobs::handlers::poll_payment::run(state, job).await,
        "reconcile_payments" => crate::jobs::handlers::reconcile_payments::run(state, job).await,
        // The hard delete behind `DELETE /account` (A355). Scheduled by the deletion request at
        // `deleted_at + ACCOUNT_DELETION_GRACE_DAYS`, with the scheduler's pass as the backstop
        // for a marked account whose job was lost — the window is the `run_at`, so the handler
        // purges whatever it is handed.
        "purge_deleted_accounts" => crate::jobs::handlers::purge_accounts::run(state, job).await,
        // One boundary of a renewal's 7/3/1/0-day notice (A300). Not a singleton: there are
        // four boundaries per period per account, and the `dedupe_key` — which names the
        // account, the period end and the boundary — is what makes each of them once-only.
        "renewal_reminder" => crate::jobs::handlers::renewal::run(state, job).await,
        // The reminder pair (A287). `send_reminder` is the producer and is per account: it
        // derives the account's due reminders through the pure engine, registers them
        // (`on conflict (dedupe_key) do nothing`) and claims the ones that are due, then
        // enqueues one `send_push` per claimed reminder. `send_push` is the delivery and is
        // per reminder, so a delivery that fails is retried for *that* reminder rather than
        // for every reminder the account has.
        "send_reminder" => crate::jobs::handlers::reminders::run(state, job).await,
        "send_push" => crate::jobs::handlers::reminders::deliver(state, job).await,
        // Permanent: no retry will invent a handler for a kind this binary does
        // not know, which is what an old row from a newer version looks like.
        other => Err(jobs::Failure::permanent(format!(
            "unknown job kind {other}"
        ))),
    }
}
