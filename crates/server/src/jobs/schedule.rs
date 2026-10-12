//! The scheduler: the server's only time-based producer of jobs.
//!
//! A **producer of rows**, not a second worker: it enqueues the per-account
//! period-boundary jobs and the existing worker runs them. Nothing here does work
//! itself, so a slow rollover cannot hold this task open and a scheduler restart
//! cannot lose a job that is already a row.
//!
//! `run_at` is the account's **next local month start**, computed in Rust through
//! `chrono-tz` because Postgres cannot do IANA arithmetic on a text timezone (A6), and
//! `dedupe_key` is `"<kind>:<user_id>:<local YYYY-MM>"`. There is no watermark column:
//! the key *is* the watermark, and it is the same month string the ledger's `period_key`
//! uses, so the job table and the ledger cannot disagree about which month is done.
//! A restart, a second instance or an overlapping pass is therefore a no-op rather than
//! a second grant.
//!
//! The pass is bounded twice over: [`SCHEDULE_BATCH`] accounts per pass, and the
//! `not exists` guard means a second pass covers the *rest* of the accounts rather than
//! re-examining the ones that already have a job waiting.

use std::time::Duration;

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::jobs;
use crate::state::AppState;

/// How often the scheduler looks for accounts whose local boundary is near.
pub const SCHEDULE_INTERVAL: Duration = Duration::from_secs(900);

/// Accounts examined per pass. Bounded, and idempotent, so a second pass covers the rest
/// rather than one pass holding a transaction over every account.
pub const SCHEDULE_BATCH: i64 = 500;

/// The two kinds a local boundary produces, in the order they matter.
///
/// `reset_counters` runs first in the worker's own ordering only because it is enqueued
/// first; it expires a lapsed grace window and recomputes the read-only flags, and the
/// grant that follows is the new period's allowance.
const BOUNDARY_KINDS: [&str; 2] = ["reset_counters", "monthly_grant"];

/// Spawns the scheduler. The `JoinHandle` is returned so `main` can keep it alive and a
/// test can abort it.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(
            interval_secs = SCHEDULE_INTERVAL.as_secs(),
            "job scheduler started"
        );
        let mut ticker = tokio::time::interval(SCHEDULE_INTERVAL);
        loop {
            // The first tick completes immediately, which is deliberate: a process that
            // starts mid-month still schedules the accounts whose boundary it missed.
            ticker.tick().await;
            match pass(&state).await {
                Ok(0) => {}
                Ok(scheduled) => tracing::info!(scheduled, "scheduled period jobs"),
                // A pass that fails is retried by the next tick; nothing is lost, because
                // the pass is idempotent and reads the same accounts again.
                Err(error) => tracing::warn!(%error, "the scheduler pass failed"),
            }
        }
    })
}

/// One pass: enqueue the next boundary jobs for the accounts that do not have them.
///
/// Returns how many accounts it enqueued for. Public because it is the unit the tests
/// drive: waiting 900 seconds for a tick would be a timing test.
pub async fn pass(state: &AppState) -> AppResult<u64> {
    let accounts: Vec<(Uuid, String)> = sqlx::query_as(
        "select u.id, u.timezone
           from users u
          where u.deleted_at is null
            and not exists (
                  select 1 from jobs j
                   where j.status in ('queued', 'running')
                     and (j.dedupe_key like 'monthly_grant:' || u.id::text || ':%'
                       or j.dedupe_key like 'reset_counters:' || u.id::text || ':%'))
          order by u.id
          limit $1",
    )
    .bind(SCHEDULE_BATCH)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("listing accounts to schedule: {error}"))
    })?;

    let mut scheduled = 0u64;
    for (user_id, timezone) in accounts {
        enqueue_boundary_jobs(state, user_id, &timezone).await?;
        scheduled += 1;
    }

    // The daily change-log compaction is a *global* singleton rather than a per-account
    // boundary job, so it is enqueued once per pass and deduped by the UTC date (A184).
    enqueue_compaction(state).await?;

    Ok(scheduled)
}

/// Enqueues the daily change-log compaction.
///
/// Idempotent by the UTC date: the unique index on `jobs.dedupe_key` means a second
/// enqueue in one day is a no-op, which is what makes it safe to call from every scheduler
/// pass, on every instance, across restarts. A `pass` that also enqueued the per-account
/// boundaries runs the same way whether or not the compaction is already waiting.
pub async fn enqueue_compaction(state: &AppState) -> AppResult<()> {
    let key = format!("compact_change_log:{}", state.clock.now().date_naive());
    jobs::enqueue(
        &state.pool,
        &state.jobs,
        "compact_change_log",
        serde_json::json!({}),
        Some(&key),
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing the compaction: {error}")))?;
    Ok(())
}

/// Enqueues every boundary kind for one account.
pub async fn enqueue_boundary_jobs(
    state: &AppState,
    user_id: Uuid,
    timezone: &str,
) -> AppResult<()> {
    for kind in BOUNDARY_KINDS {
        enqueue_kind(state, kind, user_id, timezone).await?;
    }
    Ok(())
}

/// Enqueues just the next period's grant, for a producer that is not the scheduler.
///
/// The rollover (`handlers::counters`) calls this for an account whose grace window just
/// lapsed, and it is the same key and the same run time the scheduler would produce — one
/// convention, so the two producers cannot disagree about which month is granted.
pub async fn enqueue_grant(state: &AppState, user_id: Uuid, timezone: &str) -> AppResult<()> {
    enqueue_kind(state, "monthly_grant", user_id, timezone).await
}

/// Inserts one boundary job, idempotent by its `dedupe_key`.
async fn enqueue_kind(
    state: &AppState,
    kind: &str,
    user_id: Uuid,
    timezone: &str,
) -> AppResult<()> {
    let run_at = crate::util::next_local_month_start(timezone, state.clock.as_ref());
    let month = crate::util::local_month_key(timezone, run_at);
    let dedupe_key = format!("{kind}:{user_id}:{month}");

    jobs::enqueue_at(
        &state.pool,
        &state.jobs,
        kind,
        serde_json::json!({ "user_id": user_id }),
        Some(&dedupe_key),
        run_at,
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing {kind}: {error}")))?;

    tracing::debug!(%user_id, kind, %run_at, %dedupe_key, "period job scheduled");
    Ok(())
}
