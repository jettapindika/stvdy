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
        // Permanent: no retry will invent a handler for a kind this binary does
        // not know, which is what an old row from a newer version looks like.
        other => Err(jobs::Failure::permanent(format!(
            "unknown job kind {other}"
        ))),
    }
}
