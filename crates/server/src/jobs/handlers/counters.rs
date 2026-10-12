//! `reset_counters`: the per-account local period rollover.
//!
//! Not a zeroing job. Counters are derived from `usage_events` at read time through
//! `chrono-tz` (A147), so there is nothing to zero: what a boundary actually changes is
//! the subscription's *status* (a subscription past its paid window lapses into `grace`,
//! and a lapsed grace window becomes `expired`), a recorded downgrade that is now due,
//! the read-only flags that follow from the effective plan, and the fact that the next
//! period needs a grant. A counters table would be a second source of truth for "how many
//! chat messages today", and it would drift from the events that produced it.
//!
//! It also prunes `signup_attempts`, which is the only table in this phase that grows
//! without a plan.

use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

/// How long a signup attempt is kept.
///
/// A43's windows are 24 hours, so 48 hours is twice the window it guards: a prune that ran
/// during a limiter's read could otherwise shorten the window it is supposed to enforce.
pub const SIGNUP_ATTEMPT_RETENTION_HOURS: i32 = 48;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let user_id = job.user_id()?;

    // 1. A subscription whose paid window has ended moves to `grace`. This step is why
    //    grace exists at all: without it an `active` row stayed `active` forever,
    //    `effective_plan` kept granting paid limits past the period the student paid for,
    //    and A296's "otherwise" branch — a fresh period at `paid_at` — was unreachable.
    //    Grace, not expiry, because the window is the student's: a payment that arrives a
    //    day late must not cost them the plan.
    let lapsed = crate::billing::periods::lapse_into_grace(&state.pool, user_id)
        .await
        .map_err(|error| Failure::transient(format!("lapsing into grace: {error}")))?;

    // 2. A grace window past `current_period_end + GRACE_PERIOD_DAYS` becomes expired, which
    //    is what drops the account to the free limits (A146) without anyone editing
    //    `users.plan_id`. The grace period is the spec's "3-day grace period
    //    (configurable)", read from the config rather than written down.
    let expired = sqlx::query(
        "update subscriptions set status = 'expired'
          where user_id = $1 and status = 'grace'
            and current_period_end + make_interval(days => $2) <= now()",
    )
    .bind(user_id)
    .bind(state.config.grace_period_days as i32)
    .execute(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("expiring the grace window: {error}")))?;

    // 3. A recorded downgrade takes effect at the period end (A296). The rollover is the
    //    one place it is applied, so a downgrade never shortens paid time: the window is
    //    the student's until it is spent. This calls `recompute_read_only` itself, because
    //    a downgrade and a plan switch are the same event from the data's point of view.
    let pending_plan = crate::billing::periods::apply_pending_plan(state, user_id)
        .await
        .map_err(|error| Failure::transient(format!("applying a pending plan: {error}")))?;

    // 4. The read-only flags follow from the plan that is now effective. Idempotent, and
    //    the same call the admin plan switch makes, so a downgrade and a plan switch cannot
    //    leave different flags behind.
    let report =
        crate::entitlements::recompute_read_only(&state.pool, &state.config.plans, user_id)
            .await
            .map_err(|error| Failure::transient(format!("recomputing read-only flags: {error}")))?;

    // 5. The account needs its next period's allowance. The same enqueue the scheduler
    //    makes, so the two producers cannot disagree about the key or the run time.
    let timezone: Option<String> =
        sqlx::query_scalar("select timezone from users where id = $1 and deleted_at is null")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| Failure::transient(format!("loading the timezone: {error}")))?;

    let Some(timezone) = timezone else {
        // The account is gone between enqueue and claim. Nothing left to roll over.
        return Ok(());
    };
    crate::jobs::schedule::enqueue_grant(state, user_id, &timezone)
        .await
        .map_err(|error| Failure::transient(format!("enqueuing the next grant: {error}")))?;

    // 6. Prune the signup attempts this phase's A43 limiter writes. Bounded and old enough
    //    that it can never shorten a window the limiter is reading.
    let pruned = sqlx::query(
        "delete from signup_attempts where created_at < now() - make_interval(hours => $1)",
    )
    .bind(SIGNUP_ATTEMPT_RETENTION_HOURS)
    .execute(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("pruning signup attempts: {error}")))?;

    tracing::info!(
        %user_id,
        lapsed_into_grace = lapsed,
        expired = expired.rows_affected(),
        pending_plan = pending_plan.map(|plan| plan.as_str()),
        read_only_workspaces = report.workspaces,
        read_only_sources = report.sources,
        read_only_devices = report.devices,
        signup_attempts_pruned = pruned.rows_affected(),
        "period rollover"
    );
    Ok(())
}
