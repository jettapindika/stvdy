//! The period rules after a payment: an upgrade, a downgrade, and the rollover that
//! applies a downgrade when its period ends.
//!
//! The arithmetic lives in `stvdy_core::billing` and is pure — `apply_payment` decides
//! the dates a payment buys and `upgrade_delta` the prorated difference. This module is
//! the shell over it: it reads the live subscription, writes the rows, and calls the one
//! invoice seam (`checkout::open_invoice`) so an upgrade cannot mint a QR by a second
//! route. **A second copy of "what does this cost" or "how long does this last" is the
//! defect this split exists to prevent.**
//!
//! Three rules, each of which has a way of going wrong that a reader would not see:
//!
//!  * **An upgrade switches immediately and never moves the period end** (A296). The
//!    student has already paid for the days that remain, so the switch takes those days
//!    at the higher plan and charges only the difference. The distinguishing fact is on
//!    the row — `payments.plan_id` differs from the subscription's while paid time is
//!    live — which is what `apply::apply_paid` reads. **This module therefore must not
//!    move `subscriptions.plan_id` at checkout time**: doing so would make the payment
//!    row look like a renewal and hand the student a free period.
//!  * **A downgrade is recorded, not applied** (A296). It writes the pending columns and
//!    nothing else, so a downgrade never shortens time already paid for and never deletes
//!    a row. The rollover applies it when the period ends, and `recompute_read_only`
//!    then flags what the lower plan puts over its limit — Phase 7's path, unchanged.
//!  * **A subscription that has outlived its paid window moves to `grace`, not to
//!    `expired`** (A146). Without that step nothing in the tree ever wrote the status:
//!    `counters` only expired a row that was *already* in grace, so an `active` row whose
//!    period had ended stayed `active` forever, kept granting paid limits, and left
//!    A296's "otherwise" branch — a fresh period at `paid_at` — unreachable.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use stvdy_core::billing::{BillingPeriod, PeriodChange, apply_payment, upgrade_delta};
use stvdy_core::config::{PlanConfig, PlanId, PlansFile};
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::billing::checkout::{NewPayment, PaymentView, open_invoice};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The live subscription's identity and window, as the period rules need them.
///
/// `order by (status in ('active','grace')) desc, current_period_end desc limit 1` is the
/// same read `entitlements` and `grants` make, deliberately: "the account's live
/// subscription" is one fact, and three different queries answering it would be three
/// chances to disagree about which row it is.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiveSubscription {
    pub id: Uuid,
    pub plan_id: String,
    pub billing_period: String,
    pub status: String,
    pub current_period_start: DateTime<Utc>,
    pub current_period_end: DateTime<Utc>,
    pub pending_plan_id: Option<String>,
    pub pending_billing_period: Option<String>,
}

/// The account's live subscription, if it has one.
pub async fn live_subscription(
    pool: &PgPool,
    user_id: Uuid,
) -> AppResult<Option<LiveSubscription>> {
    sqlx::query_as::<_, LiveSubscription>(
        "select s.id, s.plan_id, s.billing_period, s.status,
                s.current_period_start, s.current_period_end,
                s.pending_plan_id, s.pending_billing_period
           from subscriptions s
          where s.user_id = $1
          order by (s.status in ('active', 'grace')) desc, s.current_period_end desc
          limit 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the subscription: {error}")))
}

/// What a scheduled plan change does, and when.
///
/// A downgrade has no payment, so it has no `PaymentView`: the answer is the plan it will
/// move to and the instant it does. An upgrade answers with the invoice instead, because
/// there is money to collect before anything changes.
#[derive(Debug, Serialize)]
pub struct PlanChangeView {
    pub plan_id: String,
    pub billing_period: String,
    /// The current period's end: the instant the change takes effect. Printed rather than
    /// implied, because a "downgrade" that silently meant "now" would shorten paid time.
    pub effective_at: DateTime<Utc>,
    /// Always `true`: a downgrade is a recorded intent (A296). The field exists so a
    /// client does not have to infer it from the route it called.
    pub pending: bool,
}

/// Whether `target` is strictly more expensive than `current` for `period`.
///
/// Read from the config, never derived: the yearly price is the config's number and not
/// twelve monthlies, so a "cheaper" comparison that multiplied would be wrong for any plan
/// whose yearly price is not 12× its monthly one.
fn is_more_expensive(
    plans: &PlansFile,
    current: PlanId,
    target: PlanId,
    period: BillingPeriod,
) -> bool {
    list_price(plans.get(target), period) > list_price(plans.get(current), period)
}

/// The list price for a period, read from the plan and never derived from the other
/// period — the same rule `stvdy_core::billing` states for the price it quotes.
fn list_price(plan: &PlanConfig, period: BillingPeriod) -> i64 {
    match period {
        BillingPeriod::Monthly => plan.price_idr_monthly,
        BillingPeriod::Yearly => plan.price_idr_yearly,
    }
}

/// Starts an upgrade: prices the prorated difference and mints the invoice for it.
///
/// The subscription is **not touched here**. The switch happens when the payment is
/// confirmed (`apply::apply_paid`), because until the money arrives the student is still
/// on the plan they paid for — and because moving `subscriptions.plan_id` now would make
/// the payment row look like a renewal, so confirming it would extend the period instead
/// of switching the plan.
///
/// The platform refusal is the first statement, exactly as it is in `checkout::create`
/// (A151): an upgrade is a purchase, and a purchase is refused on a phone by the same rule
/// and with the same code.
pub async fn upgrade(
    state: &AppState,
    user: &AuthUser,
    target: PlanId,
    period: BillingPeriod,
) -> AppResult<PaymentView> {
    crate::billing::checkout_allowed(&state.config.billing_policy, &user.platform)?;

    let current = user.effective_plan();
    if !is_more_expensive(&state.config.plans, current, target, period) {
        // A "upgrade" to the plan the student is already on, or to a cheaper one, is a
        // renewal or a downgrade — never this route. Refused rather than silently treated
        // as one of them, because the amount would then be the prorated *difference*
        // where the student expected a list price.
        return Err(AppError::Validation(format!(
            "{} is not an upgrade from {} for a {period} period",
            target.as_str(),
            current.as_str(),
            period = period.as_str()
        )));
    }

    let subscription = live_subscription(&state.pool, user.user_id)
        .await?
        .filter(|row| row.status == "active" || row.status == "grace")
        .ok_or_else(|| {
            // No live subscription means there is no period to prorate against. The
            // student wants `POST /billing/checkout`, which starts a fresh period — the
            // same refusal a second invoice would otherwise produce by charging a delta
            // for days nobody paid for.
            AppError::Conflict(
                "there is no live subscription to upgrade; buy a period instead".to_string(),
            )
        })?;

    let now = state.clock.now();
    let remaining_days = (subscription.current_period_end - now).num_days();
    let amount_idr = upgrade_delta(
        state.config.plans.get(current),
        state.config.plans.get(target),
        period,
        remaining_days,
    );
    if amount_idr <= 0 {
        // `upgrade_delta` is zero when the target is not more expensive (which the check
        // above already refused) and when no paid time remains. An invoice for nothing is
        // a QR a student could scan to activate a plan for free.
        return Err(AppError::Conflict(
            "this subscription has no paid time left to upgrade within; buy a period \
             instead"
                .to_string(),
        ));
    }

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning an upgrade: {error}"))
        })?;

    let ttl_minutes = state.qris()?.invoice_ttl_minutes;

    let view = open_invoice(
        &mut tx,
        state,
        user,
        NewPayment {
            kind: "subscription",
            plan_id: Some(target.as_str()),
            billing_period: Some(period.as_str()),
            credits: None,
            amount_idr,
            subscription_id: Some(subscription.id),
            discount_code: None,
            discount_percent: 0,
            student_discount: false,
            // There is no list price to discount from: the amount *is* the prorated
            // difference, so a percentage off it would be a second, unconfigured discount
            // on top of the proration.
            amount_before_discount_idr: amount_idr,
            discount: None,
        },
        ttl_minutes,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing an upgrade: {error}")))?;

    tracing::info!(
        user_id = %user.user_id,
        from = current.as_str(),
        to = target.as_str(),
        period = period.as_str(),
        remaining_days,
        amount_idr,
        payment_id = %view.id,
        "upgrade invoice opened"
    );

    Ok(view)
}

/// Records a downgrade to take effect when the current period ends (A296).
///
/// Nothing about the subscription's window changes, and no row is deleted: the paid time
/// is the student's until it is spent. What changes is `users.plan_id` at the rollover,
/// after which the account is on the lower plan and `recompute_read_only` flags whatever
/// that plan puts over its limit.
pub async fn downgrade(
    state: &AppState,
    user: &AuthUser,
    target: PlanId,
    period: BillingPeriod,
) -> AppResult<PlanChangeView> {
    let current = user.effective_plan();
    if current == target {
        return Err(AppError::Validation(format!(
            "this account is already on {}",
            target.as_str()
        )));
    }
    if is_more_expensive(&state.config.plans, current, target, period) {
        return Err(AppError::Validation(format!(
            "{} is an upgrade from {}; upgrades are paid for, not scheduled",
            target.as_str(),
            current.as_str()
        )));
    }

    let affected = sqlx::query(
        "update subscriptions
            set pending_plan_id = $2, pending_billing_period = $3
          where user_id = $1 and status in ('active', 'grace')",
    )
    .bind(user.user_id)
    .bind(target.as_str())
    .bind(period.as_str())
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a downgrade: {error}")))?;

    if affected.rows_affected() == 0 {
        // A downgrade with no live subscription has no period end to wait for, and writing
        // `users.plan_id` instead would drop the account immediately — the one thing A296
        // forbids.
        return Err(AppError::Conflict(
            "there is no live subscription to downgrade".to_string(),
        ));
    }

    let subscription = live_subscription(&state.pool, user.user_id)
        .await?
        .ok_or(AppError::NotFound)?;

    tracing::info!(
        user_id = %user.user_id,
        from = current.as_str(),
        to = target.as_str(),
        effective_at = %subscription.current_period_end,
        "downgrade recorded"
    );

    Ok(PlanChangeView {
        plan_id: target.as_str().to_string(),
        billing_period: period.as_str().to_string(),
        effective_at: subscription.current_period_end,
        pending: true,
    })
}

/// Applies a recorded downgrade whose period has ended, and returns the new plan.
///
/// `None` when there is nothing to do, which is the common case: most accounts have no
/// pending plan, and this runs on every rollover.
///
/// The window check is `current_period_end <= now()`, so a downgrade applied early — by a
/// clock change, or by a rollover that ran before the boundary — cannot shorten paid time.
/// The subscription's own `plan_id` moves with `users.plan_id`, so the two agree after the
/// switch; leaving the subscription behind would make the next grant read the old plan.
pub async fn apply_pending_plan(state: &AppState, user_id: Uuid) -> AppResult<Option<PlanId>> {
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a rollover: {error}"))
        })?;

    let pending: Option<(Uuid, String)> = sqlx::query_as(
        "select id, pending_plan_id
           from subscriptions
          where user_id = $1
            and pending_plan_id is not null
            and current_period_end <= now()
          order by current_period_end desc
          limit 1",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a pending plan: {error}")))?;

    let Some((subscription_id, plan_id)) = pending else {
        return Ok(None);
    };

    sqlx::query(
        "update subscriptions
            set plan_id = pending_plan_id,
                billing_period = coalesce(pending_billing_period, billing_period),
                pending_plan_id = null,
                pending_billing_period = null
          where id = $1",
    )
    .bind(subscription_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("applying a pending plan: {error}")))?;

    sqlx::query("update users set plan_id = $2 where id = $1")
        .bind(user_id)
        .bind(&plan_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("moving the stored plan: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a rollover: {error}")))?;

    // The read-only flags follow the plan that is now effective, exactly as they do after
    // an admin switch (Phase 7's `162f728`): a downgrade and a switch are the same event
    // from the data's point of view, and either can leave rows over the new plan's limits.
    crate::entitlements::recompute_read_only(&state.pool, &state.config.plans, user_id).await?;

    let plan = PlanId::parse(&plan_id);
    tracing::info!(
        %user_id,
        plan = plan.as_str(),
        %subscription_id,
        "pending plan applied at the period end"
    );
    Ok(Some(plan))
}

/// Moves a subscription that has outlived its paid window into `grace`, and returns
/// whether it moved.
///
/// **Nothing in this tree wrote `grace` before this function.** `counters` only expired a
/// row that was *already* in grace, so an `active` subscription whose `current_period_end`
/// had passed stayed `active` — `effective_plan` kept granting paid limits, the account was
/// never dropped to free, and A296's "otherwise" branch (a fresh period at `paid_at`) could
/// not be reached, because the live window was in the past while the status still said the
/// plan was good.
///
/// Grace, not expiry, is the first step because the window is the student's: they paid
/// through `current_period_end`, and the spec's "3-day grace period (configurable)" is what
/// keeps a payment that arrives a day late from costing them the plan. `counters` expires
/// the row once the window has passed.
pub async fn lapse_into_grace(pool: &PgPool, user_id: Uuid) -> AppResult<bool> {
    let affected = sqlx::query(
        "update subscriptions set status = 'grace'
          where user_id = $1 and status = 'active' and current_period_end <= now()",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("lapsing into grace: {error}")))?;

    Ok(affected.rows_affected() > 0)
}

/// The dates a confirmed payment buys, from the subscription's live end (A296).
///
/// A thin re-export of the core function with the row's field already unwrapped, so the
/// caller in `apply` does not have to decide what "live" means: `apply_payment` is the one
/// place that rule lives, and it reads `active_end` as "the end, if it is still in the
/// future".
pub fn period_for_payment(
    subscription: Option<&LiveSubscription>,
    paid_at: DateTime<Utc>,
    period: BillingPeriod,
) -> PeriodChange {
    apply_payment(
        subscription.map(|row| row.current_period_end),
        paid_at,
        period,
    )
}
