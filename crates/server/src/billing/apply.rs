//! **The one function that activates a paid payment** (A301).
//!
//! The webhook calls it; `poll_payment` calls it; a future admin "mark paid" calls it.
//! That is the whole design: a payment recovered by polling and one recovered by a
//! webhook cannot differ, because they are the same code. If a second activation path
//! ever appears, this module's reason for existing is gone.
//!
//! It takes a **connection** rather than a pool, and that is what makes the money path
//! one transaction: the payment's status, the subscription, the credit grant, the fee
//! and the audit row either all commit or none of them do. A partially applied payment is
//! a student who paid and has no plan, or has a plan and no credits.
//!
//! Two idempotency defences, because they defend different replays (A293):
//!
//!  * the caller inserts into `webhook_events` first, and `on conflict (provider,
//!    event_id) do nothing returning id` means *no row* is "this exact event was already
//!    handled";
//!  * the payment's own transition is `update payments set status = 'paid' … where id =
//!    $1 and status = 'pending'`, so a gateway that re-reports the same payment under a
//!    **new** event id updates zero rows and lands on [`Applied::AlreadyPaid`].
//!
//! Only the first is what a unique key can do, and only the second is what happens when a
//! real gateway retries with a fresh id. A defence against one does nothing against the
//! other.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use stvdy_core::billing::{BillingPeriod, apply_payment};
use stvdy_core::config::PlanId;
use stvdy_core::traits::mailer::Email;
use stvdy_core::traits::payment::{WebhookEvent, WebhookKind};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// What applying a paid payment did.
///
/// Every variant is a fact a caller logs or a test asserts, and `Review` carries the
/// reason because it is what an operator reads in the queue (A294).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// A new subscription period started.
    Activated {
        plan: PlanId,
        period_end: DateTime<Utc>,
    },
    /// An existing period was extended (A296) — paying early does not lose days.
    Extended {
        plan: PlanId,
        period_end: DateTime<Utc>,
    },
    /// A credit pack landed in the `purchased` bucket (A297).
    Credits { granted: i32 },
    /// The payment was already paid. The second report of one payment is a no-op.
    AlreadyPaid,
    /// Refused and recorded for a human: an amount that does not match, a currency that
    /// is not IDR, or a payment made after its invoice expired (A294/A295).
    Review { reason: String },
}

/// The `payments` row as the money path reads it, in one statement.
///
/// One read rather than four: the amount, the status and the subscription are one fact
/// about one payment, and reading them separately would let the function act on a
/// mixture of two moments.
#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: Uuid,
    user_id: Uuid,
    kind: String,
    plan_id: Option<String>,
    billing_period: Option<String>,
    credits: Option<i32>,
    amount_idr: i64,
    expires_at: DateTime<Utc>,
}

/// The live subscription a payment extends, if any.
#[derive(sqlx::FromRow)]
struct SubscriptionRow {
    id: Uuid,
    plan_id: String,
    current_period_end: DateTime<Utc>,
}

/// Applies a **verified, paid** event to one payment, inside the caller's transaction.
///
/// The caller has already checked the signature and the amount's currency is in the
/// event; what this function decides is whether the money may be applied at all. It
/// returns `Ok(Applied::Review { .. })` rather than an error for a payment that must not
/// be activated, because a mismatch is a *recorded outcome*, not a failed request: the
/// gateway should get a 200 and stop retrying.
pub async fn apply_paid(
    conn: &mut PgConnection,
    state: &AppState,
    payment_id: Uuid,
    event: &WebhookEvent,
) -> AppResult<Applied> {
    let payment: Option<PaymentRow> = sqlx::query_as(
        // `amount_idr::bigint`: the column is INT4 and the field is `i64`, and sqlx decodes
        // by SQL type — without the cast every read of this row fails at runtime.
        "select id, user_id, kind, plan_id, billing_period, credits,
                amount_idr::bigint as amount_idr, expires_at
           from payments
          where id = $1
          for update",
    )
    .bind(payment_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the payment: {error}")))?;

    let Some(payment) = payment else {
        // The caller resolved this id from the order id, so a miss here is a
        // programming error rather than a bad request.
        return Err(AppError::Internal(anyhow::anyhow!(
            "payment {payment_id} disappeared between the lookup and the apply"
        )));
    };

    // A non-paid callback is a recorded fact, not an activation. `expired` on an order we
    // still have pending is the gateway telling us the code died, which is worth writing
    // down; the other kinds are noted and otherwise left alone.
    if event.kind != WebhookKind::Paid {
        let status = match event.kind {
            WebhookKind::Expired => "expired",
            WebhookKind::Failed => "failed",
            WebhookKind::Refunded => "refunded",
            WebhookKind::Paid => unreachable!("handled above"),
        };
        // Only a `pending` row moves: a `paid` payment must not be un-paid by a late
        // `expired` callback, which is exactly the out-of-order delivery a real gateway
        // produces.
        let updated = sqlx::query(
            "update payments set status = $2, reconciled_at = now(), provider_status = $3
              where id = $1 and status = 'pending'",
        )
        .bind(payment.id)
        .bind(status)
        .bind(kind_str(event.kind))
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a callback: {error}")))?;

        if updated.rows_affected() == 0 {
            return Ok(Applied::AlreadyPaid);
        }
        return Ok(Applied::Review {
            reason: format!(
                "the gateway reported this order as {}",
                kind_str(event.kind)
            ),
        });
    }

    // **Defence two, the guarded transition.** A gateway that re-reports one payment
    // under a new event id updates zero rows here, and that is the whole point: a unique
    // key on the event id cannot see it.
    let updated = sqlx::query(
        "update payments
            set status = 'paid', paid_at = $2, provider_payment_id = coalesce($3, provider_payment_id),
                reconciled_at = now(), provider_status = 'paid'
          where id = $1 and status = 'pending'",
    )
    .bind(payment.id)
    .bind(event.paid_at)
    .bind(&event.provider_payment_id)
    .execute(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("marking the payment paid: {error}")))?;

    if updated.rows_affected() == 0 {
        tracing::info!(
            payment_id = %payment.id,
            order_id = %event.order_id,
            event_id = %event.event_id,
            "a second report of an already-paid payment: nothing to apply"
        );
        return Ok(Applied::AlreadyPaid);
    }

    // The amount check comes **after** the transition, so a mismatched payment is marked
    // paid-but-under-review rather than left `pending` for a retry loop. Both directions
    // are the same outcome: over- and under-payment both need a human (A294).
    if event.currency != "IDR" {
        return review(
            conn,
            payment.id,
            format!(
                "the gateway reported {} {} but this invoice is in IDR",
                event.amount_idr, event.currency
            ),
        )
        .await;
    }
    if event.amount_idr != payment.amount_idr {
        return review(
            conn,
            payment.id,
            format!(
                "the gateway reported {} IDR but the invoice is for {} IDR",
                event.amount_idr, payment.amount_idr
            ),
        )
        .await;
    }

    // A295, and the two cases are **different facts**:
    //
    //  * the student paid *after* the code expired — a gateway-side race a human should
    //    look at, and `review` unconditionally. This is not what the env switch is for: a
    //    switch that could activate a payment made after the deadline would be a switch
    //    that sells expired invoices.
    //  * the student paid *before* the expiry and our confirmation arrived after it — the
    //    lateness is ours, and it must not cost the student their plan. This is the case
    //    `QRIS_HONOR_LATE_PAYMENT` (default true) decides.
    let paid_at = event.paid_at.unwrap_or_else(|| state.clock.now());
    if paid_at > payment.expires_at {
        return review(
            conn,
            payment.id,
            format!(
                "paid at {paid_at} but the invoice expired at {}",
                payment.expires_at
            ),
        )
        .await;
    }

    let confirmed_at = state.clock.now();
    if confirmed_at > payment.expires_at
        && !state
            .config
            .qris
            .as_ref()
            .is_some_and(|qris| qris.honor_late_payment)
    {
        return review(
            conn,
            payment.id,
            format!(
                "confirmed at {confirmed_at}, after the invoice expired at {}, with                  QRIS_HONOR_LATE_PAYMENT off",
                payment.expires_at
            ),
        )
        .await;
    }

    // The fee: the gateway's own number where it reports one, otherwise the configured
    // rate, otherwise nothing at all. Never a guess (A304).
    let fee_idr = event.fee_idr.or_else(|| {
        state
            .config
            .qris
            .as_ref()
            .and_then(|qris| qris.fee_idr_for(payment.amount_idr))
    });
    sqlx::query("update payments set gateway_fee_idr = $2 where id = $1")
        .bind(payment.id)
        .bind(fee_idr)
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("recording the fee: {error}")))?;

    // A297: the credit step branches on `kind`, and this is the only place either branch
    // runs.
    let applied = match payment.kind.as_str() {
        "credit_pack" => {
            let credits = payment.credits.unwrap_or(0);
            crate::credits::grant_purchased(conn, payment.user_id, i64::from(credits), payment.id)
                .await?;
            Applied::Credits { granted: credits }
        }
        _ => activate_subscription(conn, state, &payment, paid_at).await?,
    };

    notify_paid(state, &payment, &applied).await;

    tracing::info!(
        payment_id = %payment.id,
        user_id = %payment.user_id,
        order_id = %event.order_id,
        event_id = %event.event_id,
        ?applied,
        "payment applied"
    );

    Ok(applied)
}

/// Activates or extends the subscription a paid subscription payment bought.
///
/// The upgrade rule (`payments.plan_id <> subscriptions.plan_id` **with live paid time**)
/// is a switch that leaves the period dates alone; everything else is A296's extension or
/// fresh period. The live-time clause is load-bearing: without it, an expired subscription
/// whose `plan_id` differs would "upgrade" instead of starting a fresh period, and the
/// student would receive no days at all.
async fn activate_subscription(
    conn: &mut PgConnection,
    state: &AppState,
    payment: &PaymentRow,
    paid_at: DateTime<Utc>,
) -> AppResult<Applied> {
    let Some(plan_id) = payment.plan_id.as_deref() else {
        return review(
            conn,
            payment.id,
            "a subscription payment with no plan to activate".to_string(),
        )
        .await;
    };
    let plan = PlanId::parse(plan_id);
    // The provider recorded on the subscription is the gateway that took the money, from the
    // same `name()` the webhook resolves orders with — never a literal. One string for "who
    // was paid" is what keeps `payments.provider` and `subscriptions.provider` comparable.
    let provider = state.payment()?.name();

    let Some(period) = payment
        .billing_period
        .as_deref()
        .and_then(BillingPeriod::parse)
    else {
        return review(
            conn,
            payment.id,
            "a subscription payment with no billing period".to_string(),
        )
        .await;
    };

    // **The plan the student can feel is `users.plan_id`**, and this is the one write that
    // makes it live. `entitlements::effective_plan` reads `users.plan_id` gated by the
    // subscription's status (A146), so writing only `subscriptions.plan_id` below would
    // leave a student who just paid for Power on Pro limits for the whole period they paid
    // for — the exact failure this phase exists to prevent. `admin::set_plan` and
    // `periods::apply_pending_plan` both move it for the same reason; this was the only
    // writer of `subscriptions.plan_id` that did not.
    //
    // Written here, before the branch, so all three outcomes — upgrade, extension and fresh
    // period — set it exactly once and none of them can forget.
    sqlx::query("update users set plan_id = $2 where id = $1 and deleted_at is null")
        .bind(payment.user_id)
        .bind(plan.as_str())
        .execute(&mut *conn)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!(
                "activating the plan on the account: {error}"
            ))
        })?;

    // The live subscription, taken under the same transaction as the update below.
    let live: Option<SubscriptionRow> = sqlx::query_as(
        "select id, plan_id, current_period_end
           from subscriptions
          where user_id = $1 and status in ('active', 'grace')
          order by current_period_end desc
          limit 1
          for update",
    )
    .bind(payment.user_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the subscription: {error}")))?;

    // An upgrade: the plan changes and the dates do not, because the student already paid
    // for the time being switched.
    if let Some(live) = &live {
        if live.current_period_end > paid_at && live.plan_id != plan.as_str() {
            sqlx::query("update subscriptions set plan_id = $2, provider = $3 where id = $1")
                .bind(live.id)
                .bind(plan.as_str())
                .bind(provider)
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("switching the subscription plan: {error}"))
                })?;

            sqlx::query("update payments set subscription_id = $2 where id = $1")
                .bind(payment.id)
                .bind(live.id)
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("linking the payment: {error}"))
                })?;

            return Ok(Applied::Activated {
                plan,
                period_end: live.current_period_end,
            });
        }
    }

    // A296: extend from a live period end, or start a fresh period at `paid_at`.
    let active_end = live.as_ref().map(|row| row.current_period_end);
    let change = apply_payment(active_end, paid_at, period);

    let subscription_id = match &live {
        Some(live) => {
            sqlx::query(
                "update subscriptions
                    set plan_id = $2, status = 'active', billing_period = $3,
                        current_period_start = $4, current_period_end = $5,
                        provider = $6
                  where id = $1",
            )
            .bind(live.id)
            .bind(plan.as_str())
            .bind(period.as_str())
            .bind(change.start)
            .bind(change.end)
            .bind(provider)
            .execute(&mut *conn)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("extending the subscription: {error}"))
            })?;
            live.id
        }
        None => {
            // The partial unique index on `(user_id) where status in ('active','grace')`
            // is what makes "at most one live subscription" a data invariant; a second
            // live row is impossible, so there is no upsert here to get wrong.
            let id: Uuid = sqlx::query_scalar(
                "insert into subscriptions
                     (user_id, plan_id, status, billing_period,
                      current_period_start, current_period_end, provider)
                 values ($1, $2, 'active', $3, $4, $5, $6)
                 returning id",
            )
            .bind(payment.user_id)
            .bind(plan.as_str())
            .bind(period.as_str())
            .bind(change.start)
            .bind(change.end)
            .bind(provider)
            .fetch_one(&mut *conn)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("creating the subscription: {error}"))
            })?;
            id
        }
    };

    sqlx::query("update payments set subscription_id = $2 where id = $1")
        .bind(payment.id)
        .bind(subscription_id)
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("linking the payment: {error}")))?;

    // The period's allowance, through the **existing** grant with the period key Phase 7
    // established, so a renewal grants once per period (A297) and a second payment of the
    // same period cannot double-grant.
    let credits = crate::credits::monthly_allowance(state, plan);
    let period_key = format!("sub:{subscription_id}:{}", change.start.date_naive());
    crate::credits::grant_in(
        conn,
        payment.user_id,
        plan,
        crate::credits::Grant::monthly(&period_key, credits),
    )
    .await?;

    Ok(if change.extends {
        Applied::Extended {
            plan,
            period_end: change.end,
        }
    } else {
        Applied::Activated {
            plan,
            period_end: change.end,
        }
    })
}

/// Marks the payment `review` with the reason, and returns the outcome.
///
/// **The one place a payment becomes `review`**, so every refusal — a mismatched amount, a
/// non-IDR currency, a late payment, a row with nothing to activate — leaves the same kind
/// of trace: a status a human filters on, and a sentence naming the two numbers that did
/// not agree (A294).
async fn review(conn: &mut PgConnection, payment_id: Uuid, reason: String) -> AppResult<Applied> {
    sqlx::query("update payments set status = 'review', review_reason = $2 where id = $1")
        .bind(payment_id)
        .bind(&reason)
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a review: {error}")))?;

    // One error line with the id, because this is the row an operator has to find.
    tracing::error!(payment_id = %payment_id, reason = %reason, "payment needs review");

    Ok(Applied::Review { reason })
}

/// Tells the student their plan is live.
///
/// Transactional mail through the existing `Mailer`, not a `send_push` job: a payment
/// receipt is not a reminder, and Phase 12's `send_push` loads a `reminders` row whose
/// `kind` has no variant for a payment — a payload that handler cannot load would be a
/// silent permanent failure, which is worse than no notification.
///
/// **Deliberately not gated on the reminder-channel matrix.** `reminder_channels` decides
/// which channels may carry a *reminder*, and a free account has no `email` there; a
/// receipt is transactional, and the accounts most likely to need one are exactly the
/// ones a gate would silence. The only precondition is that the account has an address.
///
/// A mail failure is logged and **never** fails the payment: the plan is already live, and
/// turning a mail hiccup into a rollback would un-sell a paid period.
async fn notify_paid(state: &AppState, payment: &PaymentRow, applied: &Applied) {
    let Ok(Some(email)) = sqlx::query_scalar::<_, Option<String>>(
        "select email from users where id = $1 and deleted_at is null",
    )
    .bind(payment.user_id)
    .fetch_optional(&state.pool)
    .await
    else {
        return;
    };
    let Some(email) = email else {
        return;
    };

    let (subject, body) = match applied {
        Applied::Activated { plan, period_end } | Applied::Extended { plan, period_end } => (
            format!("Your Stvdy {} plan is active", plan.as_str()),
            format!(
                "Your payment of {} IDR went through.\n\n\
                 Your {} plan is active until {}.\n\n\
                 Every device on your account has it already — there is nothing to \
                 install and no code to enter.",
                payment.amount_idr,
                plan.as_str(),
                period_end.to_rfc3339()
            ),
        ),
        Applied::Credits { granted } => (
            "Your Stvdy credits are ready".to_string(),
            format!(
                "Your payment of {} IDR went through and {granted} credits were added to \
                 your account.\n\nPurchased credits do not expire.",
                payment.amount_idr
            ),
        ),
        // A review or a no-op is not something to mail a student about; an operator reads
        // the queue and the log line.
        Applied::Review { .. } | Applied::AlreadyPaid => return,
    };

    let message = Email {
        to: email,
        subject,
        text: body,
        html: None,
    };
    if let Err(error) = state.mailer.send(message).await {
        tracing::error!(
            %error,
            payment_id = %payment.id,
            user_id = %payment.user_id,
            "sending the payment receipt failed"
        );
    }
}

/// The stored spelling of an event kind (`payments.provider_status`).
///
/// A free function rather than a method on [`WebhookEvent`]: the type lives in
/// `crates/core`, and an inherent `impl` for a foreign type is not something a server
/// crate can write. The vocabulary is the same one `mapping` reads, so the wire spelling
/// and the stored spelling cannot drift.
pub fn kind_str(kind: WebhookKind) -> &'static str {
    match kind {
        WebhookKind::Paid => "paid",
        WebhookKind::Expired => "expired",
        WebhookKind::Failed => "failed",
        WebhookKind::Refunded => "refunded",
    }
}

/// The `payments` row a callback's order id resolves to, scoped to its owner.
///
/// `provider` is part of the lookup as well as the order id, because `order_id` is only
/// unique *within* a provider (`unique (provider, order_id)`) — a second gateway with the
/// same order numbering must not resolve to the first gateway's payment.
pub async fn payment_for_order(
    conn: &mut PgConnection,
    provider: &str,
    order_id: &str,
) -> AppResult<Option<Uuid>> {
    let row: Option<Uuid> =
        sqlx::query_scalar("select id from payments where provider = $1 and order_id = $2")
            .bind(provider)
            .bind(order_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("resolving the order: {error}")))?;
    Ok(row)
}

/// The amount and status a caller needs to decide whether to bother calling [`apply_paid`].
pub async fn payment_summary(
    conn: &mut PgConnection,
    payment_id: Uuid,
) -> AppResult<Option<(i64, String)>> {
    let row =
        sqlx::query("select amount_idr::bigint as amount_idr, status from payments where id = $1")
            .bind(payment_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the payment: {error}")))?;
    Ok(row.map(|row| (row.get("amount_idr"), row.get("status"))))
}
