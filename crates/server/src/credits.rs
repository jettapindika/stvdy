//! Credits: the Phase 2 spend, exactly as roadmap § 3 pins it.
//!
//! One transaction, `pg_advisory_xact_lock(hashtext(user_id::text))`, read the
//! bucket sums, reject if the total is short, then insert a negative
//! `credit_ledger` row **and** a `usage_events` row. Monthly bucket first, then
//! purchased (A9), because purchased credits never expire and spending them
//! first would waste the monthly grant.
//!
//! This is the **Phase 2 shim**: chat is the first route that spends, and the SSE
//! contract carries a `usage` event, so the spend has to exist now. Phase 7 adds
//! the plan matrix, the counters, the downgrade path and `check(&UsageSnapshot,
//! Feature)` on top; when it does, this module keeps the ledger write and hands
//! the *decision* to the engine. Nothing here hardcodes a limit: the cost comes
//! from `config/plans.toml`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Which bucket a spend came from, in the order they are consumed.
const BUCKETS: [&str; 2] = ["monthly", "purchased"];

/// Grants the plan's monthly allowance, once, at the start of a period.
///
/// Called at signup because that is when the first period starts. Without it a
/// fresh account has a zero balance and cannot send a single message, which is
/// what the chat suite caught: every chat test was a 402.
///
/// Phase 7 adds the recurring `monthly_grant` job on top for renewals; the
/// signup grant is not that job's work, because a job that runs at period start
/// cannot help the account whose period starts at signup.
pub async fn grant_monthly(pool: &PgPool, user_id: Uuid, credits: i64) -> AppResult<()> {
    if credits <= 0 {
        // A plan with no monthly allowance grants nothing rather than writing a
        // zero row that would confuse a later sum.
        return Ok(());
    }

    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason)
         values ($1, $2, 'monthly', 'monthly_grant')",
    )
    .bind(user_id)
    .bind(credits)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("granting monthly credits: {error}")))?;

    tracing::info!(%user_id, credits, "monthly credit grant");
    Ok(())
}

/// The plan's monthly allowance, from the config rather than a literal.
pub fn monthly_allowance(state: &AppState, plan: stvdy_core::config::PlanId) -> i64 {
    state.config.plans.get(plan).credits_per_month
}

/// The plan whose monthly allowance is larger than `plan`'s, for an upgrade hint.
///
/// `None` on the top plan: there is no higher tier, so suggesting one would be a
/// dead end. This mirrors `limits::upgrade_from`, which does the same for the
/// countable limits.
fn plan_for_more_credits(plan: stvdy_core::config::PlanId) -> Option<&'static str> {
    match plan {
        stvdy_core::config::PlanId::Free => Some("pro"),
        stvdy_core::config::PlanId::Pro => Some("power"),
        stvdy_core::config::PlanId::Power => None,
    }
}

/// A per-local-day cap on how many times `action` may be spent.
///
/// Passed into [`spend`] rather than checked before it, because a check and a
/// spend in separate statements can interleave: two concurrent requests both see
/// 19 of 20 used and both proceed. Under the spend's advisory lock the count is
/// exact.
#[derive(Debug, Clone, Copy)]
pub struct DailyCap {
    pub limit: stvdy_core::config::Limit,
    /// Start of the user's local day, from `util::local_day_start`.
    pub since: DateTime<Utc>,
    /// The label the error names, e.g. "chat messages today".
    pub label: &'static str,
}

/// What a successful spend leaves behind.
#[derive(Debug, Clone, Copy)]
pub struct Spent {
    /// The `usage_events` row this spend created.
    ///
    /// Returned so the provider's cost and token counts can be attached to *this*
    /// request. Looking up "the newest row for this user and action" instead
    /// attributes the cost to whichever request happened to finish last, which
    /// with two chats in flight is the wrong one.
    pub usage_event_id: Uuid,
    /// The balance after the spend.
    pub remaining: i64,
}

/// Spends `credits` for `action`, returning the usage row and the new balance.
///
/// The advisory lock is taken **first**, before any read of the ledger: without
/// it two concurrent requests can both see a sufficient balance and both insert,
/// which is a double spend. It is transaction-scoped, so it is released at
/// commit or rollback with no cleanup path to get wrong. The daily cap is checked
/// inside the same lock, for the same reason.
pub async fn spend(
    state: &AppState,
    user_id: Uuid,
    plan: stvdy_core::config::PlanId,
    action: &str,
    credits: i64,
    daily_cap: Option<DailyCap>,
) -> AppResult<Spent> {
    let pool = &state.pool;
    if credits < 0 {
        return Err(AppError::Internal(anyhow::anyhow!(
            "a spend must not be negative (action {action}, credits {credits})"
        )));
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a spend: {error}")))?;

    // `pg_advisory_xact_lock` takes a bigint; `hashtext` gives a stable int4 from
    // the user id, which is the same key the sync triggers use for their own
    // ordering lock, so the two cannot interleave in a surprising order.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the spend lock: {error}")))?;

    if let Some(cap) = daily_cap {
        let used: i64 = sqlx::query_scalar(
            "select count(*)::bigint from usage_events
              where user_id = $1 and action = $2 and created_at >= $3",
        )
        .bind(user_id)
        .bind(action)
        .bind(cap.since)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("counting usage: {error}")))?;

        crate::limits::enforce_count(&state.config.plans, plan, cap.limit, used, cap.label)?;
    }

    let mut available = 0i64;
    for bucket in BUCKETS {
        let sum: i64 = sqlx::query_scalar(
            "select coalesce(sum(delta), 0)::bigint from credit_ledger
              where user_id = $1 and bucket = $2",
        )
        .bind(user_id)
        .bind(bucket)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a bucket: {error}")))?;
        available += sum;
    }

    if available < credits {
        return Err(AppError::Entitlement {
            code: "insufficient_credits",
            // `limit` and `used` mean the plan-matrix sense ("the free plan allows
            // 2 workspaces and you have 2"), so they stay empty here: putting this
            // action's cost and the balance in them would make a client render
            // "the limit is 1, you have 59", which is the opposite of the truth.
            // The numbers are in the message instead.
            //
            // `min_plan` is the plan that would raise the allowance, which is what
            // a spent-out user needs to be told. It is `None` on the top plan
            // because there is nothing left to sell.
            message: format!("this action costs {credits} credits and you have {available}"),
            min_plan: plan_for_more_credits(plan),
            limit: None,
            used: None,
        });
    }

    // Monthly first, then purchased. A spend that fits in one bucket writes one
    // row; one that straddles two writes two, so the ledger always names which
    // bucket paid.
    let mut remaining_to_charge = credits;
    for bucket in BUCKETS {
        if remaining_to_charge == 0 {
            break;
        }
        let bucket_total: i64 = sqlx::query_scalar(
            "select coalesce(sum(delta), 0)::bigint from credit_ledger
              where user_id = $1 and bucket = $2",
        )
        .bind(user_id)
        .bind(bucket)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a bucket: {error}")))?;

        let take = remaining_to_charge.min(bucket_total.max(0));
        if take <= 0 {
            continue;
        }

        sqlx::query(
            "insert into credit_ledger (user_id, delta, bucket, reason)
             values ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(-take)
        .bind(bucket)
        .bind(format!("usage:{action}"))
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the ledger: {error}")))?;

        remaining_to_charge -= take;
    }

    let usage_event_id: Uuid = sqlx::query_scalar(
        "insert into usage_events (user_id, action, credits) values ($1, $2, $3)
      returning id",
    )
    .bind(user_id)
    .bind(action)
    .bind(i32::try_from(credits).unwrap_or(i32::MAX))
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing a usage event: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a spend: {error}")))?;

    Ok(Spent {
        usage_event_id,
        remaining: available - credits,
    })
}

/// Attaches the provider's cost, token counts and prompt version to the usage row
/// recent `usage_events` row for this action.
///
/// Written after the stream because the token counts only exist then, and the
/// row is created before it because the credit must be spent whether or not the
/// provider answers. `None` cost stays `None`: recording zero would understate
/// spend in the margin report (A41).
pub async fn record_provider_usage(
    pool: &PgPool,
    usage_event_id: Uuid,
    in_tokens: u32,
    out_tokens: u32,
    provider_cost_usd: Option<f64>,
    prompt_version: &str,
) -> AppResult<()> {
    sqlx::query(
        "update usage_events
            set tokens_in = $2, tokens_out = $3, provider_cost_usd = $4, prompt_version = $5
          where id = $1",
    )
    .bind(usage_event_id)
    .bind(i32::try_from(in_tokens).unwrap_or(i32::MAX))
    .bind(i32::try_from(out_tokens).unwrap_or(i32::MAX))
    .bind(provider_cost_usd)
    .bind(prompt_version)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording provider usage: {error}")))?;
    Ok(())
}

/// The daily chat cap for a user, from their plan and their local day.
///
/// Counted from `usage_events` rather than a counter column: a counter can drift
/// from the events that produced it, and the drift is invisible until a user is
/// wrongly blocked. The day boundary goes through `chrono-tz` (A6), never a fixed
/// offset.
pub fn daily_chat_cap(state: &AppState, user: &crate::auth::extract::AuthUser) -> DailyCap {
    let plan = user.plan();
    DailyCap {
        limit: state.config.plans.get(plan).chat_msgs_per_day,
        since: crate::util::local_day_start(&user.timezone, state.clock.as_ref()),
        label: "chat messages today",
    }
}

/// The cost of one chat message, from the config rather than a literal.
pub fn chat_message_cost(state: &AppState) -> i64 {
    state.config.plans.credits.chat_message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bucket_order_spends_the_monthly_grant_first() {
        // The order is the contract (A9): purchased credits never expire, so
        // spending them first would waste the monthly grant that does.
        assert_eq!(BUCKETS, ["monthly", "purchased"]);
    }
}
