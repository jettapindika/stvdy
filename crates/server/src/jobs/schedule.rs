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

/// How often the payment passes are enqueued (A301).
///
/// Five minutes is the resolution of a lost webhook: the checkout screen polls the server,
/// the server polls the gateway, and a student who paid should see their plan inside a
/// coffee break rather than at the next quarter hour. It is deliberately its own ticker
/// rather than the boundary pass's 900 seconds — a per-account boundary job is a monthly
/// event and does not want to run three times as often, and a payment is a minute-scale
/// event that must not wait fifteen.
pub const PAYMENT_TICK: Duration = Duration::from_secs(300);

/// How often the reminder pass runs (A287).
///
/// Five minutes is the resolution of a reminder, and the number is deliberately the same as
/// [`PAYMENT_TICK`]: both are "a student-facing event on a minute scale", and two different
/// five-minute constants would be two things to keep in step for no gain. It is *not*
/// [`SCHEDULE_INTERVAL`]: a monthly boundary job does not want to run three times as often.
pub const REMINDER_TICK: Duration = Duration::from_secs(300);

/// Accounts one reminder pass enqueues for. Bounded, and idempotent inside its window, so a
/// second pass covers the rest rather than one pass reading every account.
const REMINDER_BATCH: i64 = 500;

/// Spawns the scheduler. The `JoinHandle` is returned so `main` can keep it alive and a
/// test can abort it.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(
            interval_secs = SCHEDULE_INTERVAL.as_secs(),
            payment_secs = PAYMENT_TICK.as_secs(),
            reminder_secs = REMINDER_TICK.as_secs(),
            "job scheduler started"
        );
        let mut boundary = tokio::time::interval(SCHEDULE_INTERVAL);
        let mut payments = tokio::time::interval(PAYMENT_TICK);
        let mut reminders = tokio::time::interval(REMINDER_TICK);
        loop {
            // Every tick completes immediately on the first pass, which is deliberate: a
            // process that starts mid-month still schedules the accounts whose boundary it
            // missed, and one that starts mid-outage starts asking the gateway at once.
            tokio::select! {
                _ = boundary.tick() => match pass(&state).await {
                    Ok(0) => {}
                    Ok(scheduled) => tracing::info!(scheduled, "scheduled period jobs"),
                    // A pass that fails is retried by the next tick; nothing is lost, because
                    // the pass is idempotent and reads the same accounts again.
                    Err(error) => tracing::warn!(%error, "the scheduler pass failed"),
                },
                _ = payments.tick() => match payment_pass(&state).await {
                    Ok(enqueued) => tracing::debug!(enqueued, "enqueued the payment passes"),
                    Err(error) => tracing::warn!(%error, "enqueuing the payment passes failed"),
                },
                _ = reminders.tick() => match reminder_pass(&state).await {
                    Ok(0) => {}
                    Ok(enqueued) => tracing::debug!(enqueued, "scheduled reminder derivations"),
                    Err(error) => tracing::warn!(%error, "the reminder pass failed"),
                },
            }
        }
    })
}

/// One reminder pass: enqueue a `send_reminder` for each account that does not have one
/// waiting in this window (A287).
///
/// The key is `"send_reminder:<user_id>:<utc window>"`, which is the whole of the idempotence
/// — a scheduler restart, a second instance, a double tick and an overlapping pass all produce
/// the same key inside one window, so the unique index on `jobs.dedupe_key` turns every
/// duplicate into a no-op. The window is `floor(now / REMINDER_TICK)` and is stated as a
/// **watermark in UTC seconds** rather than a local-time fact, deliberately: local-date
/// arithmetic lives in `crates/core/src/reminders.rs` and nowhere else, so nothing here has to
/// know a user's zone to decide whether it already ran.
///
/// The `not exists` guard is the other half, exactly as [`pass`] and [`calendar_pass`] do it:
/// it keeps a pass from *examining* an account that already has a derivation queued, so the
/// batch covers the rest of the accounts rather than re-enqueuing the same ones.
///
/// Public because it is the unit the tests drive: waiting five minutes for a tick would be a
/// timing test.
pub async fn reminder_pass(state: &AppState) -> AppResult<u64> {
    let window = reminder_window(state);
    let accounts: Vec<Uuid> = sqlx::query_scalar(
        "select u.id
           from users u
          where u.deleted_at is null
            and not exists (
                  select 1 from jobs j
                   where j.status in ('queued', 'running')
                     and j.dedupe_key = 'send_reminder:' || u.id::text || ':' || $1::text)
          order by u.id
          limit $2",
    )
    .bind(window)
    .bind(REMINDER_BATCH)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing accounts to remind: {error}")))?;

    let mut enqueued = 0u64;
    for user_id in accounts {
        jobs::enqueue(
            &state.pool,
            &state.jobs,
            "send_reminder",
            serde_json::json!({ "user_id": user_id }),
            Some(&format!("send_reminder:{user_id}:{window}")),
        )
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing send_reminder: {error}")))?;
        enqueued += 1;
    }
    if enqueued > 0 {
        tracing::info!(enqueued, "scheduled reminder derivations");
    }
    Ok(enqueued)
}

/// `floor(now / REMINDER_TICK)`, the reminder pass's dedupe window.
///
/// A UTC second count rather than a local date, and the file's header says why: the pass is a
/// producer of rows and its idempotence is a watermark, while every local-time decision belongs
/// to the pure engine. A windowed key also means a pass that runs a second after the last one
/// is a no-op, which a per-date key would not give.
pub fn reminder_window(state: &AppState) -> i64 {
    state.clock.now().timestamp() / REMINDER_TICK.as_secs().max(1) as i64
}

/// One payment pass: enqueue the poller and the sweep, idempotent by their windows.
///
/// Public because it is the unit the tests drive — waiting five minutes for a tick would
/// be a timing test. Both kinds are **singletons**: `dedupe_key = "poll_payment"` and
/// `"reconcile_payments"` with no window, because there is one of each per deployment and
/// a second enqueue must be a no-op rather than a second scan of the same table. The
/// cadence comes from this ticker, not from the key.
pub async fn payment_pass(state: &AppState) -> AppResult<u64> {
    // Nothing to poll when no gateway is configured: a deployment with no gateway has no
    // payments to reconcile, and enqueuing a job that would answer `provider_error` every
    // five minutes is noise in the failure log rather than a signal.
    if state.config.qris.is_none() {
        return Ok(0);
    }

    jobs::enqueue(
        &state.pool,
        &state.jobs,
        "poll_payment",
        serde_json::json!({}),
        Some("poll_payment"),
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing poll_payment: {error}")))?;

    jobs::enqueue(
        &state.pool,
        &state.jobs,
        "reconcile_payments",
        serde_json::json!({}),
        Some("reconcile_payments"),
    )
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("enqueuing reconcile_payments: {error}"))
    })?;

    Ok(2)
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

    // And the calendar poll, which is per *connection* rather than per account (A238).
    calendar_pass(state).await?;

    // And the renewal reminders, which are per *subscription* rather than per account
    // (A300): four boundaries at 7/3/1/0 days before the period end, each keyed by the
    // period it belongs to. Deliberately here rather than in the payment pass — a renewal
    // is a monthly event, and the account it belongs to may have no gateway traffic at all.
    crate::jobs::handlers::renewal::renewal_pass(state).await?;

    Ok(scheduled)
}

/// One pass over the live calendar connections whose watermark is older than the interval.
///
/// The same shape as [`pass`]: a producer of rows, bounded, idempotent. Two bounds and two
/// independent idempotences, and they do different jobs:
///
///  * the `not exists` guard plus the windowed `dedupe_key` mean a restart, a second instance
///    or a double tick enqueues **one** job rather than two;
///  * `last_synced_at < now() - INTERVAL` (both sides read from `state.clock`) means a
///    connection synced inside the window is not even examined, so a pass costs one query
///    rather than one row per connection.
///
/// A connection in `status = 'revoked'` is skipped: its grant is gone, and a pass would only
/// mark it revoked again. `error` is deliberately **not** skipped — a faculty calendar that
/// 500s for an hour must be picked back up on its own, and the whole point of A247's "the
/// error is cleared by the next clean pass" is that the pass keeps happening.
pub async fn calendar_pass(state: &AppState) -> AppResult<u64> {
    let interval_secs = state.config.calendar_poll_interval.as_secs().max(1) as i64;
    let window = calendar_window(state);
    // The cutoff is the **injected** clock's instant, not the database's `now()`. The dedupe
    // key below is built from the same clock, so a pass that compared the database's clock
    // against a key built from the injected one had two sources of truth for "what time is
    // it": a test that moved the clock could make the key fresh while the selection still
    // measured the wall clock, and no time-dependent assertion about this pass could be
    // written. One instant, read once.
    let cutoff = state.clock.now() - chrono::Duration::seconds(interval_secs);
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "select c.id, c.user_id
           from calendar_connections c
          where c.status <> 'revoked'
            and (c.last_synced_at is null
                 or c.last_synced_at < $1)
            and not exists (
                  select 1 from jobs j
                   where j.status in ('queued', 'running')
                     and j.dedupe_key = 'sync_calendar:' || c.id::text || ':' || $2::text)
          order by c.last_synced_at asc nulls first, c.id
          limit $3",
    )
    .bind(cutoff)
    .bind(window.to_string())
    .bind(SCHEDULE_BATCH)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing connections to poll: {error}")))?;

    let mut scheduled = 0u64;
    for (connection_id, user_id) in rows {
        enqueue_connection_sync(state, user_id, connection_id).await?;
        scheduled += 1;
    }
    if scheduled > 0 {
        tracing::info!(scheduled, "scheduled calendar syncs");
    }
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

/// Enqueues one connection's `sync_calendar`, idempotent inside the poll window (A238).
///
/// The key is `"sync_calendar:<connection_id>:<floor(now/INTERVAL)>"`, which is the whole of
/// the idempotence: a scheduler restart, a second instance, a double tick and the *first*
/// import enqueued by `POST /calendar/ics` all produce the same key inside one window, so the
/// unique index on `jobs.dedupe_key` turns every duplicate into a no-op. The watermark column
/// (`last_synced_at`) is the other half — it keeps a pass from *examining* a connection that
/// was synced inside the interval, while the key keeps a second pass from *enqueuing* one.
///
/// `POST /calendar/sync` uses its own per-minute key rather than this one: a manual sync must
/// work when the poll window has already been used, and the two keys are deliberately
/// different so a user pressing the button does not silently do nothing for fourteen minutes.
pub async fn enqueue_connection_sync(
    state: &AppState,
    user_id: Uuid,
    connection_id: Uuid,
) -> AppResult<()> {
    let window = calendar_window(state);
    let dedupe_key = format!("sync_calendar:{connection_id}:{window}");

    jobs::enqueue(
        &state.pool,
        &state.jobs,
        "sync_calendar",
        serde_json::json!({ "user_id": user_id, "connection_id": connection_id }),
        Some(&dedupe_key),
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing a calendar sync: {error}")))?;

    tracing::debug!(%connection_id, %dedupe_key, "calendar sync scheduled");
    Ok(())
}

/// The manual sync's key window: one per minute per connection (A238).
///
/// A per-minute key bounds a user's own retry loop without a second rate limiter, and it is
/// deliberately not the poll's window: a student who has just added a class and wants it now
/// must not be told "wait fifteen minutes" because the poll already ran.
pub fn manual_window(state: &AppState) -> i64 {
    state.clock.now().timestamp() / 60
}

/// `floor(now / CALENDAR_POLL_INTERVAL)`, the poll's dedupe window.
fn calendar_window(state: &AppState) -> i64 {
    let interval = state.config.calendar_poll_interval.as_secs().max(1) as i64;
    state.clock.now().timestamp() / interval
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
