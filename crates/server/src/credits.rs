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

/// The balance, as the ledger sums it. There is deliberately no balance column
/// to keep consistent: `sum(delta)` cannot drift from the rows that produced it.
pub async fn balance(pool: &PgPool, user_id: Uuid) -> AppResult<i64> {
    let total: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the credit balance: {error}")))?;
    Ok(total)
}

pub async fn bucket_balance(pool: &PgPool, user_id: Uuid, bucket: &str) -> AppResult<i64> {
    let total: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = $2",
    )
    .bind(user_id)
    .bind(bucket)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a credit bucket: {error}")))?;
    Ok(total)
}

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

/// Spends `credits` for `action`, returning the remaining balance.
///
/// The advisory lock is taken **first**, before any read of the ledger: without
/// it two concurrent requests can both see a sufficient balance and both insert,
/// which is a double spend. It is transaction-scoped, so it is released at
/// commit or rollback with no cleanup path to get wrong.
pub async fn spend(pool: &PgPool, user_id: Uuid, action: &str, credits: i64) -> AppResult<i64> {
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
            message: format!("this action costs {credits} credits and you have {available}"),
            min_plan: None,
            limit: Some(credits),
            used: Some(available),
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

    sqlx::query("insert into usage_events (user_id, action, credits) values ($1, $2, $3)")
        .bind(user_id)
        .bind(action)
        .bind(i32::try_from(credits).unwrap_or(i32::MAX))
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("writing a usage event: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a spend: {error}")))?;

    Ok(available - credits)
}

/// Attaches the provider's cost, token counts and prompt version to the most
/// recent `usage_events` row for this action.
///
/// Written after the stream because the token counts only exist then, and the
/// row is created before it because the credit must be spent whether or not the
/// provider answers. `None` cost stays `None`: recording zero would understate
/// spend in the margin report (A41).
pub async fn record_provider_usage(
    pool: &PgPool,
    user_id: Uuid,
    action: &str,
    in_tokens: u32,
    out_tokens: u32,
    provider_cost_usd: Option<f64>,
    prompt_version: &str,
) -> AppResult<()> {
    sqlx::query(
        "update usage_events
            set tokens_in = $3, tokens_out = $4, provider_cost_usd = $5, prompt_version = $6
          where id = (
                select id from usage_events
                 where user_id = $1 and action = $2
                 order by created_at desc
                 limit 1)",
    )
    .bind(user_id)
    .bind(action)
    .bind(i32::try_from(in_tokens).unwrap_or(i32::MAX))
    .bind(i32::try_from(out_tokens).unwrap_or(i32::MAX))
    .bind(provider_cost_usd)
    .bind(prompt_version)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording provider usage: {error}")))?;
    Ok(())
}

/// How many chat messages the user has sent since their local midnight.
///
/// Counted from `usage_events` rather than a counter column: a counter can drift
/// from the events that produced it, and the drift is invisible until a user is
/// wrongly blocked. `since` is computed by the caller through `chrono-tz` (A6),
/// never from a fixed offset.
pub async fn count_since(
    pool: &PgPool,
    user_id: Uuid,
    action: &str,
    since: DateTime<Utc>,
) -> AppResult<i64> {
    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from usage_events
          where user_id = $1 and action = $2 and created_at >= $3",
    )
    .bind(user_id)
    .bind(action)
    .bind(since)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("counting usage: {error}")))?;
    Ok(count)
}

/// Enforces `chat_msgs_per_day` for the user's local day.
pub async fn enforce_daily_chat_cap(
    state: &AppState,
    user: &crate::auth::extract::AuthUser,
) -> AppResult<i64> {
    let plan = user.plan();
    let limit = state.config.plans.get(plan).chat_msgs_per_day;
    let since = crate::util::local_day_start(&user.timezone, state.clock.as_ref());

    let used = count_since(&state.pool, user.user_id, "chat_message", since).await?;

    crate::limits::enforce_count(
        &state.config.plans,
        plan,
        limit,
        used,
        "chat messages today",
    )?;
    Ok(used)
}

/// The cost of one chat message, from the config rather than a literal.
pub fn chat_message_cost(state: &AppState) -> i64 {
    state.config.plans.credits.chat_message
}

/// Exposed so a handler can report the balance it just left behind.
pub async fn remaining_after(pool: &PgPool, user_id: Uuid) -> AppResult<i64> {
    balance(pool, user_id).await
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
