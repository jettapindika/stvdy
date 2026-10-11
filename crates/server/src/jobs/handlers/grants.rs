//! `monthly_grant`: the period's allowance, granted once.
//!
//! The amount is the **effective** plan's `credits_per_month` read at run time, so a
//! job enqueued while an account was on pro grants what the account is on when it runs —
//! an expiry that lands in between does not hand out a period of pro credits.
//!
//! The period is the key: `"sub:<subscription_id>:<period start>"` for a paid account, so
//! a period *extension* grants once per period rather than once per calendar month, and
//! `"monthly:<local YYYY-MM>"` for a free one. Both are written with `insert … on conflict
//! do nothing` against the partial unique index (A152), which is what makes the signup
//! grant and this job unable to double-grant in one month.

use uuid::Uuid;

use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

/// The account's plan and its live subscription, as the grant needs them.
///
/// A `left join lateral` rather than four scalar subselects: the plan, the timezone and the
/// subscription are one fact about one account, and reading them in one statement is what
/// keeps them from describing two different moments.
#[derive(sqlx::FromRow)]
struct GrantAccount {
    plan_id: String,
    timezone: String,
    status: Option<String>,
    subscription_id: Option<String>,
    /// `current_period_start` as a date: the period key names the *day* the period began,
    /// so an extension that keeps the same start (a renewal recorded in place) grants once.
    period_start: Option<String>,
}

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let user_id = job.user_id()?;

    let account: Option<GrantAccount> = sqlx::query_as(
        "select u.plan_id, u.timezone, sub.status,
                sub.id::text as subscription_id,
                sub.current_period_start::date::text as period_start
           from users u
           left join lateral (
                select s.* from subscriptions s
                 where s.user_id = u.id
                 order by (s.status in ('active', 'grace')) desc,
                          s.current_period_end desc
                 limit 1) sub on true
          where u.id = $1 and u.deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the account: {error}")))?;

    // The account may have been deleted between enqueue and claim; a grant for it is
    // nothing to do, and retrying would never change that.
    let Some(account) = account else {
        return Ok(());
    };

    let status = crate::entitlements::subscription_status(account.status.as_deref(), user_id);
    let plan = crate::entitlements::effective_plan(&account.plan_id, status);
    // Through `credits::monthly_allowance`, which is the module allowed to read the cost
    // table: the sweep test fails a plan limit read anywhere else, and a second reader would
    // be a second answer to "how much does this plan grant".
    let credits = crate::credits::monthly_allowance(state, plan);

    let period_key = match (&account.subscription_id, &account.period_start) {
        (Some(id), Some(start)) if status.grants_paid_limits() => format!("sub:{id}:{start}"),
        _ => format!(
            "monthly:{}",
            crate::util::local_month_key(&account.timezone, state.clock.now())
        ),
    };

    grant_period(state, user_id, plan, &period_key, credits).await
}

/// Expires the previous period's unspent monthly balance and grants the new one, once.
///
/// Shared with the rollover, which enqueues this job rather than granting itself: one
/// sequence, so a grant made from two producers cannot take two different shapes.
///
/// The whole sequence is one transaction under the per-user advisory lock — the same lock
/// the credit spend takes. The lock is what makes the expiry safe: a grant that landed
/// between the "already granted?" check and the expiry would be wiped by the expiry and
/// then skipped as a duplicate, losing the account a whole period's allowance.
pub(crate) async fn grant_period(
    state: &AppState,
    user_id: Uuid,
    plan: stvdy_core::config::PlanId,
    period_key: &str,
    credits: i64,
) -> Result<(), Failure> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| Failure::transient(format!("beginning the grant: {error}")))?;

    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("taking the grant lock: {error}")))?;

    let already: Option<Uuid> =
        sqlx::query_scalar("select id from credit_ledger where user_id = $1 and period_key = $2")
            .bind(user_id)
            .bind(period_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| Failure::transient(format!("checking the period: {error}")))?;

    if already.is_some() {
        // This period is already granted — the signup grant and this job share the key
        // (A152) — so there is nothing to expire and nothing to grant. Expiring anyway
        // would wipe the allowance that was just given.
        tracing::debug!(%user_id, period_key, "period already granted");
        return Ok(());
    }

    if !state.config.plans.credits.monthly_rollover {
        // No rollover by default (A153): an unspent monthly balance expires at the period
        // boundary instead of accumulating. Purchased credits are never touched, and the
        // `expire:<period>` key makes the expiry once-only for the same reason the grant
        // is.
        sqlx::query(
            "insert into credit_ledger (user_id, delta, bucket, reason, period_key)
             select $1, -t.total, 'monthly', 'expire', $2
               from (select coalesce(sum(delta), 0)::int as total from credit_ledger
                      where user_id = $1 and bucket = 'monthly') t
              where t.total <> 0
             on conflict (user_id, period_key) where period_key is not null do nothing",
        )
        .bind(user_id)
        .bind(format!("expire:{period_key}"))
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("expiring the last period: {error}")))?;
    }

    crate::credits::grant_in(
        &mut tx,
        user_id,
        plan,
        crate::credits::Grant::monthly(period_key, credits),
    )
    .await
    .map_err(|error| Failure::transient(format!("granting the period: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| Failure::transient(format!("committing the grant: {error}")))?;
    Ok(())
}
