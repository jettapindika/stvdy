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
    /// The `usage_events.action` this cap counts.
    pub action: &'static str,
    pub limit: stvdy_core::config::Limit,
    /// Start of the user's local day, from `util::local_day_start`.
    pub since: DateTime<Utc>,
    /// The label the error names, e.g. "chat messages today".
    pub label: &'static str,
    /// Count only rows the user actually paid for.
    ///
    /// The web-search cap needs this: a cache hit costs no credit and must not
    /// consume the daily allowance either, or re-asking a question would burn the
    /// allowance that A40 introduced the cache to protect.
    pub uncached_only: bool,
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
    daily_caps: [Option<DailyCap>; 2],
) -> AppResult<Spent> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a spend: {error}")))?;
    let spent = spend_in(&mut tx, state, user_id, plan, action, credits, daily_caps).await?;
    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a spend: {error}")))?;
    Ok(spent)
}

/// [`spend`] against an existing connection, so a caller that must make the charge
/// atomic with a check the charge depends on can.
///
/// `create_deck` is that caller. The monthly card limit is counted from the cards
/// already written **plus** the cards in-flight decks have requested, and that count
/// only means anything if the charge and the deck row that reserves the cards commit
/// together: in two transactions a second request reads a count the first has not
/// committed yet, which is how a monthly cap is exceeded by asking twice.
///
/// The advisory lock is still taken here, on the caller's connection. It is
/// transaction-scoped and re-entrant, so a caller that already holds it (as
/// `create_deck` does, before it counts) is not blocked by its own lock.
pub async fn spend_in(
    conn: &mut sqlx::PgConnection,
    state: &AppState,
    user_id: Uuid,
    plan: stvdy_core::config::PlanId,
    action: &str,
    credits: i64,
    daily_caps: [Option<DailyCap>; 2],
) -> AppResult<Spent> {
    if credits < 0 {
        return Err(AppError::Internal(anyhow::anyhow!(
            "a spend must not be negative (action {action}, credits {credits})"
        )));
    }

    // `pg_advisory_xact_lock` takes a bigint; `hashtext` gives a stable int4 from
    // the user id, which is the same key the sync triggers use for their own
    // ordering lock, so the two cannot interleave in a surprising order.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user_id.to_string())
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the spend lock: {error}")))?;

    for cap in daily_caps.iter().flatten() {
        let used: i64 = sqlx::query_scalar(
            "select count(*)::bigint from usage_events
              where user_id = $1 and action = $2 and created_at >= $3
                and (not $4 or cache_hit = false)",
        )
        .bind(user_id)
        .bind(cap.action)
        .bind(cap.since)
        .bind(cap.uncached_only)
        .fetch_one(&mut *conn)
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
        .fetch_one(&mut *conn)
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
        .fetch_one(&mut *conn)
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
        .execute(&mut *conn)
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
    .fetch_one(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing a usage event: {error}")))?;

    Ok(Spent {
        usage_event_id,
        remaining: available - credits,
    })
}

/// The `usage_events` row a spend just created, for a caller that has to look it up
/// later (a job, which has no access to the route's `Spent`).
///
/// `(user_id, action, created_at)` is unique in practice: `spend` writes exactly one
/// row per action per transaction, and the `created_at` is the spend's. The caller
/// that already has a `usage_event_id` passes it instead.
pub async fn latest_usage_event(
    pool: &PgPool,
    user_id: Uuid,
    action: &str,
    since: DateTime<Utc>,
) -> AppResult<Option<Uuid>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "select id from usage_events
          where user_id = $1 and action = $2 and created_at >= $3
          order by created_at desc
          limit 1",
    )
    .bind(user_id)
    .bind(action)
    .bind(since)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("looking up a usage event: {error}")))?;
    Ok(id)
}

/// Attaches token counts, the provider's cost and the prompt version to a usage row.
///
/// `usage_event_id: Option<Uuid>` because the generation job has no id: the row was
/// written by `spend`, and the job finds it with `latest_usage_event`. A `None` id is
/// a no-op rather than an error: a usage row that cannot be found is a reporting gap,
/// not a reason to fail a job whose cards are already stored and paid for.
///
/// `prompt_version` is mandatory: every model call must be attributable to a prompt
/// version, or a quality regression cannot be traced (the roadmap's rule).
///
/// `in_tokens`/`out_tokens` are `Option<u32>`: a failed generation records what the
/// provider reported, because the call happened and was billed. `Some(0)` is a real
/// zero from a provider that reported nothing; `None` means the value is unknown,
/// which is a different thing and must not be collapsed into a zero.
pub async fn record_usage(
    pool: &PgPool,
    usage_event_id: Option<Uuid>,
    in_tokens: Option<u32>,
    out_tokens: Option<u32>,
    provider_cost_usd: Option<f64>,
    prompt_version: &str,
) -> AppResult<()> {
    let Some(usage_event_id) = usage_event_id else {
        return Ok(());
    };

    sqlx::query(
        "update usage_events
            set tokens_in = $2, tokens_out = $3, provider_cost_usd = $4, prompt_version = $5
          where id = $1",
    )
    .bind(usage_event_id)
    .bind(in_tokens.map(|n| i32::try_from(n).unwrap_or(i32::MAX)))
    .bind(out_tokens.map(|n| i32::try_from(n).unwrap_or(i32::MAX)))
    .bind(provider_cost_usd)
    .bind(prompt_version)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording provider usage: {error}")))?;
    Ok(())
}

/// Reverses a spend that could not be delivered.
///
/// A generation that fails for good has already charged the user and bought
/// nothing. The row is a positive `credit_ledger` entry with the documented
/// `refund` reason and `ref_id` naming what failed, so the ledger explains itself
/// to the admin margin report instead of showing an unexplained balance change.
/// `monthly` is the bucket because that is the one `spend` consumes first.
pub async fn refund(
    pool: &PgPool,
    user_id: Uuid,
    credits: i64,
    ref_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("refunding credits: {error}")))?;
    refund_in(&mut conn, user_id, credits, ref_id, reason).await
}

/// [`refund`] against an existing connection, so a caller that must issue the
/// compensating ledger row and the state change it compensates in one
/// transaction can.
///
/// `fail_and_refund` is that caller: the claim that zeroes `decks.credits_spent`
/// and the refund row must commit together, or a failure between them charges a
/// user for a deck they never received with nothing left to retry.
pub async fn refund_in(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    credits: i64,
    ref_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    if credits <= 0 {
        return Ok(());
    }
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason, ref_id)
         values ($1, $2, 'monthly', $3, $4)",
    )
    .bind(user_id)
    .bind(credits)
    .bind(reason)
    .bind(ref_id)
    .execute(conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("refunding credits: {error}")))?;
    Ok(())
}

/// How many pending refunds one boot sweep will attempt.
///
/// Bounded so a pathological backlog cannot hold startup open indefinitely: the
/// next boot (or a later pass) picks up the rest, and the sweep is idempotent.
const REFUND_SWEEP_LIMIT: i64 = 1000;

/// A table that carries `credits_spent` and a terminal `failed` status, and can
/// therefore be left with a charge that is "pending a refund".
///
/// An enum rather than a table name from the caller: the sweep interpolates the name
/// into its SQL, and a value chosen from a closed set of compile-time constants cannot
/// carry anything a user supplied. `decks` and `quizzes` share the whole recovery
/// sequence (claim, refund, zero, commit), and the sequence is subtle enough that a
/// second copy of it would be a second place to get it wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundTarget {
    Deck,
    Quiz,
    Report,
    Cheatsheet,
}

impl RefundTarget {
    /// What the row is called in a log line, for whoever reads the log.
    fn label(self) -> &'static str {
        match self {
            RefundTarget::Deck => "deck",
            RefundTarget::Quiz => "quiz",
            RefundTarget::Report => "report",
            RefundTarget::Cheatsheet => "cheatsheet",
        }
    }

    /// The statements this target needs, as literals rather than as `format!`ed
    /// strings: `sqlx` refuses a dynamically built query precisely because a table name
    /// is the kind of value that should never be interpolated, and writing one arm per
    /// target is what keeps that guarantee. The logic around them is shared; only the
    /// three statements are not.
    fn list_failed_sql(self) -> &'static str {
        match self {
            RefundTarget::Deck => {
                "select id from decks
                  where status = 'failed' and credits_spent > 0
                  order by updated_at
                  limit $1"
            }
            RefundTarget::Quiz => {
                "select id from quizzes
                  where status = 'failed' and credits_spent > 0
                  order by updated_at
                  limit $1"
            }
            RefundTarget::Report => {
                "select id from reports
                  where status = 'failed' and credits_spent > 0
                  order by updated_at
                  limit $1"
            }
            RefundTarget::Cheatsheet => {
                "select id from cheatsheets
                  where status = 'failed' and credits_spent > 0
                  order by updated_at
                  limit $1"
            }
        }
    }

    fn claim_pending_sql(self) -> &'static str {
        match self {
            RefundTarget::Deck => {
                "select user_id, credits_spent
                   from decks
                  where id = $1 and status = 'failed' and credits_spent > 0
                    for update skip locked"
            }
            RefundTarget::Quiz => {
                "select user_id, credits_spent
                   from quizzes
                  where id = $1 and status = 'failed' and credits_spent > 0
                    for update skip locked"
            }
            RefundTarget::Report => {
                "select user_id, credits_spent
                   from reports
                  where id = $1 and status = 'failed' and credits_spent > 0
                    for update skip locked"
            }
            RefundTarget::Cheatsheet => {
                "select user_id, credits_spent
                   from cheatsheets
                  where id = $1 and status = 'failed' and credits_spent > 0
                    for update skip locked"
            }
        }
    }

    fn zero_charge_sql(self) -> &'static str {
        match self {
            RefundTarget::Deck => {
                "update decks set credits_spent = 0, updated_at = now() where id = $1"
            }
            RefundTarget::Quiz => {
                "update quizzes set credits_spent = 0, updated_at = now() where id = $1"
            }
            RefundTarget::Report => {
                "update reports set credits_spent = 0, updated_at = now() where id = $1"
            }
            RefundTarget::Cheatsheet => {
                "update cheatsheets set credits_spent = 0, updated_at = now() where id = $1"
            }
        }
    }
}

/// Refunds every row whose charge is still pending, once.
///
/// The failure path in `jobs::handlers::generate::fail_and_refund` (and its quiz
/// counterpart) commits the row as `failed` and leaves `credits_spent` in place when the
/// ledger insert cannot be written, because the job that reached that point has no retry
/// left. This is what makes that state recoverable instead of permanent: it runs at boot,
/// before the worker, and is idempotent because the same statement zeroes the charge it
/// refunds.
///
/// Returns how many charges it recovered.
pub async fn refund_pending(pool: &PgPool, target: RefundTarget) -> AppResult<u64> {
    let ids: Vec<Uuid> = sqlx::query_scalar(target.list_failed_sql())
        .bind(REFUND_SWEEP_LIMIT)
        .fetch_all(pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!(
                "listing {}s awaiting a refund: {error}",
                target.label()
            ))
        })?;

    let mut recovered = 0u64;
    for id in ids {
        match refund_one_pending(pool, target, id).await {
            Ok(true) => recovered += 1,
            Ok(false) => {}
            // One row's failure must not abort the sweep: the rest are still
            // recoverable, and the next boot retries this one.
            Err(error) => {
                tracing::error!(%error, %id, target = target.label(), "could not recover a pending refund");
            }
        }
    }
    Ok(recovered)
}

/// Refunds one row's pending charge, if it still has one.
///
/// `false` when there was nothing left to do: another instance claimed the row first
/// (`for update skip locked`), or an earlier pass already zeroed the charge. The row lock
/// and the `credits_spent > 0` predicate together make the refund once-only.
async fn refund_one_pending(pool: &PgPool, target: RefundTarget, id: Uuid) -> AppResult<bool> {
    let mut tx = pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("starting a refund transaction: {error}"))
    })?;

    let pending: Option<(Uuid, i32)> = sqlx::query_as(target.claim_pending_sql())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("claiming a pending refund: {error}"))
        })?;

    let Some((user_id, credits)) = pending else {
        return Ok(false);
    };

    refund_in(&mut tx, user_id, i64::from(credits), id, "refund").await?;

    // Zeroing the charge in the same transaction as the ledger row is what makes a
    // second pass a no-op: the guard and the refund move together.
    sqlx::query(target.zero_charge_sql())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("zeroing a recovered charge: {error}"))
        })?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a recovered refund: {error}"))
    })?;
    Ok(true)
}

/// The daily chat cap for a user, from their plan and their local day.
///
/// Counted from `usage_events` rather than a counter column: a counter can drift
/// from the events that produced it, and the drift is invisible until a user is
/// wrongly blocked. The day boundary goes through `chrono-tz` (A6), never a fixed
/// offset.
pub fn daily_chat_cap(state: &AppState, user: &crate::auth::extract::AuthUser) -> DailyCap {
    let plan = user.effective_plan();
    DailyCap {
        action: "chat_message",
        limit: state.config.plans.get(plan).chat_msgs_per_day,
        since: crate::util::local_day_start(&user.timezone, state.clock.as_ref()),
        label: "chat messages today",
        uncached_only: false,
    }
}

/// The daily web-search cap: `web_searches_per_day`, 3 on free and 30 on pro.
///
/// Counts **uncached searches only**. A cache hit costs no credit (A40) and must
/// not consume the allowance either: a repeat of a question already answered is
/// exactly the case the cache exists for, and charging the allowance for it would
/// make the free tier's three searches per day unusable for a student who revises.
pub fn daily_search_cap(state: &AppState, user: &crate::auth::extract::AuthUser) -> DailyCap {
    let plan = user.effective_plan();
    DailyCap {
        action: "web_search",
        limit: stvdy_core::config::Limit::Max(state.config.plans.get(plan).web_searches_per_day),
        since: crate::util::local_day_start(&user.timezone, state.clock.as_ref()),
        label: "web searches today",
        uncached_only: true,
    }
}

/// The cost of one web search, from the config rather than a literal.
///
/// Charged once per search that actually called a provider. A cache hit is free
/// (A40): re-billing a repeated query would burn the user's daily allowance on a
/// question already answered.
pub fn search_cost(state: &AppState) -> i64 {
    state.config.plans.credits.web_search
}

/// Records a search that ran, whether or not it cost anything.
///
/// A cache hit still writes a row with `cache_hit = true`, because the row is what
/// makes the cache visible in the margin report: without it a cache hit and a
/// never-run search look identical. `search_tier` and `unresponsive_engines` are
/// the A3 health signal, so silent degradation of the free path shows up in the
/// report rather than being discovered by a user.
pub async fn record_search(
    pool: &PgPool,
    user_id: Uuid,
    credits: i64,
    answered_by: Option<&str>,
    unresponsive: &[String],
    cache_hit: bool,
) -> AppResult<()> {
    sqlx::query(
        "insert into usage_events
           (user_id, action, credits, search_tier, unresponsive_engines, cache_hit)
         values ($1, 'web_search', $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(i32::try_from(credits).unwrap_or(i32::MAX))
    .bind(answered_by)
    .bind(unresponsive)
    .bind(cache_hit)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a search: {error}")))?;
    Ok(())
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
