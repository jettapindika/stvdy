//! The polling fallback and the reconciliation sweep (A301).
//!
//! A webhook can be lost — our 200, or the gateway's retry, can vanish — so the server
//! asks. Two passes, and the difference between them is the question each asks:
//!
//!  * [`poll_pending`] runs every five minutes over payments still `pending` inside their
//!    expiry window (the partial index `payments (expires_at) where status = 'pending'`
//!    exists for exactly this), asks `get_status`, and applies a paid answer through
//!    [`super::apply::apply_paid`] — **the same function the webhook calls**. That is not
//!    a convenience: a payment recovered by polling and one recovered by a webhook must
//!    not be able to differ, and the only way to guarantee that is one code path.
//!  * [`reconcile`] runs hourly and compares the gateway's recent list against our rows,
//!    flagging both mismatch kinds: a gateway transaction we have no row for, and one of
//!    ours the gateway does not know. Both are `review` rows with a `review_reason`,
//!    never silent edits.
//!
//! Neither retries a permanent failure into a loop: an unknown order at the gateway is
//! `review`, not a retry.

use chrono::Duration;
use stvdy_core::traits::payment::{GatewayStatus, PaymentError, WebhookEvent, WebhookKind};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::integrations::payment::GatewayTransaction;
use crate::state::AppState;

/// One pending payment, as the poll needs it.
#[derive(sqlx::FromRow)]
struct PendingPayment {
    id: Uuid,
    order_id: String,
}

/// How many payments one pass examines.
///
/// Bounded, like every other pass in this tree: a pass that examined an unbounded set
/// would hold a connection per payment for as long as the gateway is slow, and the next
/// pass would queue behind it. The window below is what keeps this from being a backlog —
/// a payment that falls out of it is `expired`, not forgotten.
const POLL_BATCH: i64 = 100;

/// The poll pass: pending payments inside their expiry window, asked one at a time.
///
/// Returns how many payments the gateway reported paid.
pub async fn poll_pending(state: &AppState) -> AppResult<u64> {
    let gateway = state.payment()?;

    let pending: Vec<PendingPayment> = sqlx::query_as(
        "select id, order_id
           from payments
          where status = 'pending' and expires_at > now()
          order by expires_at
          limit $1",
    )
    .bind(POLL_BATCH)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing pending payments: {error}")))?;

    let mut applied = 0u64;
    for payment in pending {
        match gateway.get_status(&payment.order_id).await {
            Ok(status) => {
                if apply_status(state, &payment, &status).await? {
                    applied += 1;
                }
            }
            // The gateway does not know an order we think is pending. That is a fact a
            // human needs — it means either we recorded an invoice the gateway never
            // minted or the gateway lost it — and retrying would ask the same question
            // forever.
            Err(PaymentError::UnknownOrder) => {
                flag_review(
                    state,
                    payment.id,
                    "the gateway does not know this order".to_string(),
                )
                .await?;
            }
            // A transport or provider failure is transient: leave the row pending and let
            // the next pass ask again. Writing `review` here would flood the queue during
            // an outage.
            Err(error) => {
                tracing::warn!(
                    order_id = %payment.order_id,
                    %error,
                    "polling a payment failed; it stays pending"
                );
            }
        }
    }

    Ok(applied)
}

/// Applies one gateway status to one payment, through the same `apply_paid` the webhook
/// uses. Returns whether the payment was newly activated.
async fn apply_status(
    state: &AppState,
    payment: &PendingPayment,
    status: &GatewayStatus,
) -> AppResult<bool> {
    match status {
        GatewayStatus::Paid {
            paid_at,
            amount_idr,
            provider_payment_id,
            fee_idr,
        } => {
            // A synthetic event, and the id is **stable**: `poll:<order_id>:<paid_at>` is
            // the same string on every poll of the same payment, so two polls collide on
            // `webhook_events`' unique key exactly as two webhooks would. A random id here
            // would make the poller's own retries look like distinct events — which is the
            // race the guarded transition catches, but only after doing the work twice.
            let event = WebhookEvent {
                event_id: format!("poll:{}:{}", payment.order_id, paid_at.to_rfc3339()),
                kind: WebhookKind::Paid,
                order_id: payment.order_id.clone(),
                amount_idr: *amount_idr,
                currency: "IDR".to_string(),
                paid_at: Some(*paid_at),
                provider_payment_id: provider_payment_id.clone(),
                fee_idr: *fee_idr,
                raw: serde_json::json!({
                    "source": "poll",
                    "order_id": payment.order_id,
                    "status": "paid",
                }),
            };

            let mut tx = state.pool.begin().await.map_err(|error| {
                AppError::Internal(anyhow::anyhow!("beginning a poll apply: {error}"))
            })?;
            let inserted: Option<Uuid> = sqlx::query_scalar(
                "insert into webhook_events (provider, event_id, payload)
                 values ($1, $2, $3)
                 on conflict (provider, event_id) do nothing
                 returning id",
            )
            .bind("qris_gateway")
            .bind(&event.event_id)
            .bind(&event.raw)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("recording the polled event: {error}"))
            })?;

            if inserted.is_none() {
                return Ok(false);
            }

            let outcome = super::apply::apply_paid(&mut tx, state, payment.id, &event).await?;
            sqlx::query(
                "update webhook_events set payment_id = $2, processed_at = now() where id = $1",
            )
            .bind(inserted)
            .bind(payment.id)
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("closing the event: {error}")))?;

            tx.commit().await.map_err(|error| {
                AppError::Internal(anyhow::anyhow!("committing a poll apply: {error}"))
            })?;

            tracing::info!(
                order_id = %payment.order_id,
                payment_id = %payment.id,
                ?outcome,
                "a lost webhook was recovered by polling"
            );
            Ok(matches!(
                outcome,
                super::apply::Applied::Activated { .. } | super::apply::Applied::Extended { .. }
            ))
        }
        // The gateway says the invoice is dead. The row follows, so a pending payment is
        // never left pending forever — which is what the checkout screen would otherwise
        // poll on until the student gave up.
        GatewayStatus::Expired | GatewayStatus::Failed => {
            let status = match status {
                GatewayStatus::Expired => "expired",
                _ => "failed",
            };
            let updated = sqlx::query(
                "update payments
                    set status = $2, provider_status = $3, reconciled_at = now()
                  where id = $1 and status = 'pending'",
            )
            .bind(payment.id)
            .bind(status)
            .bind(status)
            .execute(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("expiring a payment: {error}")))?;

            if updated.rows_affected() > 0 {
                tracing::info!(
                    order_id = %payment.order_id,
                    payment_id = %payment.id,
                    status,
                    "the gateway says this invoice is dead"
                );
            }
            Ok(false)
        }
        GatewayStatus::Pending => Ok(false),
        // A refund reported by the status query is not an activation and not an expiry:
        // it is a money movement a human records (A308), so it is flagged rather than
        // applied. The webhook's `refunded` kind lands in the same place.
        GatewayStatus::Refunded => {
            flag_review(
                state,
                payment.id,
                "the gateway reports this payment as refunded".to_string(),
            )
            .await?;
            Ok(false)
        }
    }
}

/// The reconciliation pass (A301).
///
/// Two mismatches, both `review` rows: a gateway transaction we have no payment for, and
/// one of our payments the gateway does not know. Returns how many rows it flagged.
///
/// It never edits a payment's status. A sweep that could change a status would be a second
/// activation path with a schedule, which is exactly what this module exists to prevent.
pub async fn reconcile(state: &AppState) -> AppResult<u64> {
    let gateway = state.payment()?;
    let hours = state.qris()?.reconcile_hours;

    let transactions = match gateway.recent_transactions(hours).await {
        Ok(list) => list,
        // A sweep that cannot reach the gateway has learned nothing; the next hour asks
        // again. Flagging every payment as unknown-to-the-gateway during an outage would
        // bury the real mismatches in noise.
        Err(error) => {
            tracing::warn!(%error, "the reconciliation sweep could not list transactions");
            return Ok(0);
        }
    };

    let mut flagged = 0u64;

    // Direction one: the gateway has a transaction we never recorded.
    for transaction in &transactions {
        let known: Option<Uuid> =
            sqlx::query_scalar("select id from payments where provider = $1 and order_id = $2")
                .bind(gateway.name())
                .bind(&transaction.order_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("checking a transaction: {error}"))
                })?;

        if known.is_none() {
            tracing::error!(
                order_id = %transaction.order_id,
                amount_idr = transaction.amount_idr,
                "the gateway reports a transaction with no payment row"
            );
            // There is no row to flag, so the trace is the log line and the count. The
            // `admin` payment queue is fed by rows; this case is the one that needs an
            // operator to look at the gateway's dashboard, which is what the error line is
            // for.
            flagged += 1;
        }
    }

    // Direction two: one of ours the gateway does not know. Only payments inside the
    // window, and only ones the gateway should know about by now — a pending invoice
    // created seconds ago is not a mismatch.
    let ours: Vec<(Uuid, String)> = sqlx::query_as(
        "select id, order_id
           from payments
          where status in ('pending', 'paid')
            and created_at >= now() - make_interval(hours => $1::int)
            and created_at < now() - interval '10 minutes'",
    )
    .bind(hours)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing our payments: {error}")))?;

    let listed: std::collections::HashSet<&str> = transactions
        .iter()
        .map(|transaction| transaction.order_id.as_str())
        .collect();

    for (payment_id, order_id) in ours {
        if listed.contains(order_id.as_str()) {
            continue;
        }
        flag_review(
            state,
            payment_id,
            "the gateway does not know this order".to_string(),
        )
        .await?;
        flagged += 1;
    }

    if flagged > 0 {
        tracing::warn!(flagged, "the reconciliation sweep flagged mismatches");
    }
    Ok(flagged)
}

/// Marks a payment `review` with a reason, unless it is already terminal.
///
/// `status in ('pending', 'paid')` is the guard: a `refunded` or `review` row must not be
/// re-flagged into a different reason by a later sweep, because the first reason is what a
/// human is already acting on.
async fn flag_review(state: &AppState, payment_id: Uuid, reason: String) -> AppResult<()> {
    let updated = sqlx::query(
        "update payments set status = 'review', review_reason = $2, reconciled_at = now()
          where id = $1 and status in ('pending', 'paid')",
    )
    .bind(payment_id)
    .bind(&reason)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("flagging a payment: {error}")))?;

    if updated.rows_affected() > 0 {
        tracing::error!(%payment_id, %reason, "payment flagged for review");
    }
    Ok(())
}

/// The transactions a sweep compares against, for a caller that already has them (a test,
/// or an admin view). Kept here rather than in the job handler so the comparison rule has
/// one home.
pub fn order_ids(transactions: &[GatewayTransaction]) -> Vec<&str> {
    transactions
        .iter()
        .map(|transaction| transaction.order_id.as_str())
        .collect()
}

/// The window a sweep looks back over, as a duration — exposed so a test can seed a row
/// inside it without re-deriving the same arithmetic.
pub fn reconcile_window(hours: i64) -> Duration {
    Duration::hours(hours)
}
