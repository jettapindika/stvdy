//! The polling fallback and the reconciliation sweep (A301).
//!
//! **The gateway never pushes, so the server asks — and it asks from two places.** The
//! bridge reports an outcome only through its own status route, which makes two passes
//! necessary and gives each a different question:
//!
//!  * [`refresh_pending`] is the **fast** ingress: the checkout screen already polls
//!    `GET /billing/payments/:id` every couple of seconds (A306), so that request asks
//!    the gateway once while the row is still `pending`. Without it a student who has
//!    paid waits for the next sweep, which is up to five minutes of "waiting for
//!    confirmation" for money that already moved.
//!  * [`poll_pending`] is the **backstop**: every five minutes, over pending payments
//!    inside their expiry window (the partial index `payments (expires_at) where status =
//!    'pending'` exists for exactly this), for a client that closed the tab.
//!
//! Both call [`super::apply::apply_paid`] — the same function the admin "mark paid" tool
//! calls. That is not a convenience: a payment recovered by the client's poll and one
//! recovered by the sweep must not be able to differ, and the only way to guarantee that
//! is one code path.
//!
//!  * [`reconcile`] runs hourly and compares the gateway's settled transactions against
//!    our rows, flagging both mismatch kinds: money that arrived with no payment row, and
//!    one of ours the gateway has no transaction for. Both are `review` rows with a
//!    `review_reason`, never silent edits.
//!
//! Neither retries a permanent failure into a loop: an unknown payment at the gateway is
//! `review`, not a retry.

use chrono::Duration;
use stvdy_core::traits::payment::{GatewayEvent, GatewayEventKind, GatewayStatus, PaymentError};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::integrations::payment::GatewayTransaction;
use crate::state::AppState;

/// One pending payment, as the poll needs it.
///
/// `provider_payment_id` is the gateway's own id, and it is what every status question is
/// asked with. It is nullable because a row exists from the moment checkout opens the
/// invoice and the gateway's id arrives with its answer, so a row with none is one whose
/// invoice was never minted.
#[derive(sqlx::FromRow)]
struct PendingPayment {
    id: Uuid,
    provider_payment_id: Option<String>,
}

/// How many payments one pass examines.
///
/// Bounded, like every other pass in this tree: a pass that examined an unbounded set
/// would hold a connection per payment for as long as the gateway is slow, and the next
/// pass would queue behind it. The window below is what keeps this from being a backlog —
/// a payment that falls out of it is `expired`, not forgotten.
const POLL_BATCH: i64 = 100;

/// Asks the gateway about **one** payment, for a caller that is already looking at it.
///
/// The checkout screen's poll is the only caller, and the cost is one gateway round trip
/// per poll while the invoice is live. Everything that is not a live pending payment
/// returns `Ok(false)` without a call: a terminal row is finished, and a row with no
/// gateway id has no invoice to ask about.
///
/// A gateway failure is **swallowed into `Ok(false)`**, and deliberately: this runs
/// inside a user-facing request, and a bridge that is down must not turn the checkout
/// screen's poll into an error page. The row stays `pending`, the sweep asks again, and
/// the failure is logged here.
pub async fn refresh_pending(state: &AppState, user_id: Uuid, payment_id: Uuid) -> AppResult<bool> {
    let Ok(gateway) = state.payment() else {
        // No gateway is configured, so there is nothing to ask and nothing to report.
        return Ok(false);
    };

    // Scoped by `user_id`, like every other query a client can reach: this runs inside a
    // request whose payment id came from the URL, and "the caller may not ask about another
    // account's payment" is a rule about the query rather than about the id's entropy.
    let row: Option<(Option<String>, String)> = sqlx::query_as(
        "select provider_payment_id, status from payments where id = $1 and user_id = $2",
    )
    .bind(payment_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the payment: {error}")))?;

    let Some((Some(gateway_id), status)) = row else {
        return Ok(false);
    };
    if status != "pending" {
        return Ok(false);
    }

    match gateway.get_status(&gateway_id).await {
        Ok(status) => apply_status(state, payment_id, &gateway_id, &status).await,
        Err(error) => {
            tracing::debug!(
                %payment_id,
                %gateway_id,
                %error,
                "the gateway could not answer a checkout poll; the row stays pending"
            );
            Ok(false)
        }
    }
}

/// The sweep: pending payments inside their expiry window, asked one at a time.
///
/// Returns how many payments the gateway reported paid.
pub async fn poll_pending(state: &AppState) -> AppResult<u64> {
    let gateway = state.payment()?;

    let pending: Vec<PendingPayment> = sqlx::query_as(
        "select id, provider_payment_id
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
        // A pending row with no gateway id is one whose invoice was never minted, which
        // the checkout's rollback is supposed to make impossible. There is nothing to
        // ask, so it is skipped rather than sent to the gateway as a request for a
        // payment named "null".
        let Some(gateway_id) = payment.provider_payment_id.as_deref() else {
            tracing::warn!(
                payment_id = %payment.id,
                "a pending payment has no gateway id; skipping it"
            );
            continue;
        };

        match gateway.get_status(gateway_id).await {
            Ok(status) => {
                if apply_status(state, payment.id, gateway_id, &status).await? {
                    applied += 1;
                }
            }
            // The gateway does not know a payment we think is pending. That is a fact a
            // human needs — it means either we recorded an invoice the gateway never
            // minted or the gateway lost it (a restart with a fresh state file does
            // exactly that) — and retrying would ask the same question forever.
            Err(PaymentError::UnknownOrder) => {
                flag_review(
                    state,
                    payment.id,
                    "the gateway does not know this payment".to_string(),
                )
                .await?;
            }
            // A transport or provider failure is transient: leave the row pending and let
            // the next pass ask again. Writing `review` here would flood the queue during
            // an outage.
            Err(error) => {
                tracing::warn!(
                    %gateway_id,
                    %error,
                    "polling a payment failed; it stays pending"
                );
            }
        }
    }

    Ok(applied)
}

/// Applies one gateway status to one payment, through the same `apply_paid` the checkout
/// poll uses. Returns whether the payment was newly activated.
async fn apply_status(
    state: &AppState,
    payment_id: Uuid,
    gateway_id: &str,
    status: &GatewayStatus,
) -> AppResult<bool> {
    match status {
        GatewayStatus::Paid {
            paid_at,
            amount_idr,
            provider_payment_id,
            fee_idr,
        } => {
            // The event, and its id is **stable**: `gateway:<id>:<paid_at>` is the same
            // string on every read of the same payment, so two reads collide on
            // `gateway_events`' unique key exactly as two callbacks would. A random id
            // here would make the poller's own retries look like distinct events — which
            // is the race the guarded transition catches, but only after doing the work
            // twice.
            let event = GatewayEvent {
                event_id: format!("gateway:{gateway_id}:{}", paid_at.to_rfc3339()),
                kind: GatewayEventKind::Paid,
                order_id: gateway_id.to_string(),
                amount_idr: *amount_idr,
                currency: "IDR".to_string(),
                paid_at: Some(*paid_at),
                provider_payment_id: provider_payment_id.clone(),
                fee_idr: *fee_idr,
                raw: serde_json::json!({
                    "source": "poll",
                    "provider_payment_id": gateway_id,
                    "status": "paid",
                    "amount_idr": amount_idr,
                }),
            };

            let mut tx = state.pool.begin().await.map_err(|error| {
                AppError::Internal(anyhow::anyhow!("beginning a poll apply: {error}"))
            })?;
            let inserted: Option<Uuid> = sqlx::query_scalar(
                "insert into gateway_events (provider, event_id, payload)
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
                AppError::Internal(anyhow::anyhow!("recording the gateway event: {error}"))
            })?;

            if inserted.is_none() {
                // This exact answer was already applied. The row is not touched again.
                return Ok(false);
            }

            let outcome = super::apply::apply_paid(&mut tx, state, payment_id, &event).await?;

            sqlx::query(
                "update gateway_events set payment_id = $2, processed_at = now() where id = $1",
            )
            .bind(inserted)
            .bind(payment_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("linking the gateway event: {error}"))
            })?;

            tx.commit().await.map_err(|error| {
                AppError::Internal(anyhow::anyhow!("committing a poll apply: {error}"))
            })?;

            tracing::info!(
                payment_id = %payment_id,
                %gateway_id,
                ?outcome,
                "the gateway reported a payment paid"
            );
            Ok(matches!(
                outcome,
                super::apply::Applied::Activated { .. } | super::apply::Applied::Extended { .. }
            ))
        }
        // The gateway says the invoice is dead. The row follows, so a pending payment is
        // never left pending forever — which is what the checkout screen would otherwise
        // poll on until the student gave up.
        //
        // `cancelled` and `expired` stay distinct in the row: one is a person or the buyer
        // stopping the invoice, the other is the clock, and a support question about which
        // it was is answered from `payments.status` rather than by asking the bridge what
        // it said last week.
        GatewayStatus::Expired | GatewayStatus::Cancelled | GatewayStatus::Failed => {
            let status = match status {
                GatewayStatus::Expired => "expired",
                GatewayStatus::Cancelled => "cancelled",
                _ => "failed",
            };
            let updated = sqlx::query(
                "update payments
                    set status = $2, provider_status = $3, reconciled_at = now()
                  where id = $1 and status = 'pending'",
            )
            .bind(payment_id)
            .bind(status)
            .bind(status)
            .execute(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("expiring a payment: {error}")))?;

            if updated.rows_affected() > 0 {
                tracing::info!(
                    payment_id = %payment_id,
                    %gateway_id,
                    status,
                    "the gateway says this invoice is dead"
                );
            }
            Ok(false)
        }
        GatewayStatus::Pending => Ok(false),
        // A refund reported by the status query is not an activation and not an expiry:
        // it is a money movement a human records (A308), so it is flagged rather than
        // applied.
        GatewayStatus::Refunded => {
            flag_review(
                state,
                payment_id,
                "the gateway reports this payment as refunded".to_string(),
            )
            .await?;
            Ok(false)
        }
    }
}

/// The reconciliation pass (A301).
///
/// Two mismatches, both `review` rows: money the gateway took that we have no payment
/// for, and one of our payments the gateway has no transaction for. Returns how many rows
/// it flagged.
///
/// It never edits a payment's status. A sweep that could change a status would be a second
/// activation path with a schedule, which is exactly what this module exists to prevent.
///
/// **The join is on the amount, not on an order id.** The bridge's feed carries the
/// *provider's* order id, which is not ours — our `order_id` travels to the bridge as a
/// `reference` and is never sent back in the feed — so the only field that identifies one
/// of our payments is the amount that landed. That is the same fact the bridge itself
/// matches its settlements by, and it is why `payments.payable_amount_idr` exists.
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

    // Direction one: the gateway took money we never recorded.
    for transaction in &transactions {
        let known: Option<Uuid> = sqlx::query_scalar(
            "select id from payments
              where provider = $1 and coalesce(payable_amount_idr, amount_idr) = $2",
        )
        .bind(gateway.name())
        .bind(transaction.amount_idr)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("checking a transaction: {error}")))?;

        if known.is_none() {
            tracing::error!(
                transaction_id = %transaction.transaction_id,
                amount_idr = transaction.amount_idr,
                paid_at = %transaction.paid_at,
                "the gateway reports a transaction with no payment row"
            );
            // There is no row to flag, so the trace is the log line and the count. The
            // `admin` payment queue is fed by rows; this case is the one that needs an
            // operator to look at the gateway's dashboard, which is what the error line is
            // for.
            flagged += 1;
        }
    }

    // Direction two: one of ours the gateway has no transaction for. Only payments inside
    // the window, and only ones the gateway should have settled by now — a pending invoice
    // created seconds ago is not a mismatch.
    let ours: Vec<(Uuid, Option<String>, i64)> = sqlx::query_as(
        "select id, provider_payment_id, coalesce(payable_amount_idr, amount_idr)::bigint as payable
           from payments
          where status in ('pending', 'paid')
            and created_at >= now() - make_interval(hours => $1::int)
            and created_at < now() - interval '10 minutes'",
    )
    .bind(hours)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing our payments: {error}")))?;

    let settled: std::collections::HashSet<i64> = transactions
        .iter()
        .map(|transaction| transaction.amount_idr)
        .collect();

    for (payment_id, gateway_id, payable) in ours {
        if settled.contains(&payable) {
            continue;
        }
        // A pending row whose invoice was never minted is not something the gateway can be
        // asked about, so it is not a mismatch either — it is a row the checkout's rollback
        // failed to remove, and `refresh_pending` skips it for the same reason.
        if gateway_id.is_none() {
            continue;
        }
        flag_review(
            state,
            payment_id,
            format!("the gateway has no transaction for {payable} IDR"),
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

/// The settled amounts a sweep compares against, for a caller that already has them (a
/// test, or an admin view). Kept here rather than in the job handler so the comparison
/// rule has one home.
pub fn settled_amounts(transactions: &[GatewayTransaction]) -> Vec<i64> {
    transactions
        .iter()
        .map(|transaction| transaction.amount_idr)
        .collect()
}

/// The window a sweep looks back over, as a duration — exposed so a test can seed a row
/// inside it without re-deriving the same arithmetic.
pub fn reconcile_window(hours: i64) -> Duration {
    Duration::hours(hours)
}
