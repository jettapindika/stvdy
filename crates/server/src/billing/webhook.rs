//! `POST /webhooks/qris` — verified, idempotent, amount-checked, one transaction.
//!
//! The route is public (no bearer token): the gateway has no session, and the
//! **signature** is the authorisation. That makes the order of operations the whole
//! security story, and it is fixed:
//!
//!  1. the IP allowlist, when `QRIS_WEBHOOK_IPS` is set;
//!  2. the signature, over the **raw bytes**;
//!  3. the parse;
//!  4. the idempotency insert;
//!  5. the guarded transition and the apply.
//!
//! A body that fails the signature is never parsed, and a request that fails any step
//! writes nothing. The handler extracts [`axum::body::Bytes`] rather than `Json<T>`
//! deliberately: a framework that deserialises and re-encodes produces different bytes
//! (key order, whitespace, number formatting) and the signature would not verify — a
//! false rejection of a real payment, which is the one failure this route must not have.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use stvdy_core::traits::payment::{PaymentError, WebhookEvent};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::apply::{self, Applied};

/// What the gateway is told.
///
/// `duplicate` is in the body, not only in the status: a gateway reading a 200 needs to
/// know whether its callback changed anything, and the field is the cheapest way to say
/// so without a second status code the gateway's retry policy would misread.
#[derive(Debug, Serialize)]
pub struct WebhookOutcome {
    pub duplicate: bool,
    pub status: String,
}

/// `POST /webhooks/qris`
pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<(StatusCode, axum::Json<WebhookOutcome>)> {
    let started = std::time::Instant::now();

    // The gateway, or the honest 502. Resolved before anything is verified, because a
    // deployment with no gateway cannot have sent this callback.
    let gateway = state.payment()?;

    // The IP allowlist, checked before the signature (A292). `ConnectInfo` is not used
    // here because the deployment terminates TLS at a proxy whose forwarded address we do
    // not trust; when `QRIS_WEBHOOK_IPS` is set, an operator has arranged for the peer
    // address to be meaningful. An empty list means "not configured", which is not the
    // same as "allow everything" — the signature is still required either way.
    if let Some(qris) = state.config.qris.as_ref() {
        if !qris.webhook_ips.is_empty() {
            let peer = peer_ip(&headers);
            if !peer.is_some_and(|ip| qris.webhook_ips.contains(&ip)) {
                tracing::warn!(
                    ?peer,
                    "a callback from outside QRIS_WEBHOOK_IPS was refused"
                );
                return Err(AppError::Unauthorized);
            }
        }
    }

    // Signature over the raw bytes, before any parse. Every failure is the same 401: a
    // response that says *which* check failed tells an attacker what to fix.
    let event = match gateway.parse_and_verify_webhook(&headers, &body) {
        Ok(event) => event,
        Err(PaymentError::Signature) | Err(PaymentError::Invalid(_)) => {
            tracing::warn!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                "a callback failed verification"
            );
            return Err(AppError::Unauthorized);
        }
        Err(PaymentError::UnknownOrder) => return Err(AppError::NotFound),
        Err(PaymentError::NotConfigured) => {
            // Our misconfiguration, not the caller's: 502 so the gateway retries rather
            // than treating a valid callback as forged.
            return Err(AppError::Provider(
                "the payment gateway is not configured".to_string(),
            ));
        }
        Err(other) => {
            tracing::error!(%other, "verifying a callback failed");
            return Err(AppError::Provider(other.to_string()));
        }
    };

    let outcome = apply_event(&state, gateway.name(), &event).await?;

    tracing::info!(
        order_id = %event.order_id,
        event_id = %event.event_id,
        duplicate = outcome.duplicate,
        status = %outcome.status,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "payment webhook handled"
    );

    Ok((StatusCode::OK, axum::Json(outcome)))
}

/// The transaction: audit, idempotency, resolve, apply.
///
/// Split from [`handle`] so a test can drive the same sequence without an HTTP request —
/// and, more importantly, so there is exactly one place the transaction boundary is
/// chosen. A second caller that opened its own transaction would be a second definition
/// of "the money path is atomic".
pub async fn apply_event(
    state: &AppState,
    provider: &str,
    event: &WebhookEvent,
) -> AppResult<WebhookOutcome> {
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning the webhook: {error}"))
        })?;

    // **Defence one.** `on conflict (provider, event_id) do nothing returning id`: no row
    // back means this exact event was already handled, so the answer is `duplicate` and
    // nothing else happens. The payload is stored so a human can read what the gateway
    // actually said (A294), and it is stored *before* the payment is resolved, because a
    // callback naming an order we do not have is evidence too.
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into webhook_events (provider, event_id, payload)
         values ($1, $2, $3)
         on conflict (provider, event_id) do nothing
         returning id",
    )
    .bind(provider)
    .bind(&event.event_id)
    .bind(&event.raw)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording the webhook event: {error}")))?;

    if inserted.is_none() {
        return Ok(WebhookOutcome {
            duplicate: true,
            status: "duplicate".to_string(),
        });
    }

    // The order id resolves to a payment scoped by `provider`, because `order_id` is only
    // unique within one gateway. An order we do not have is 404 and — note the order — the
    // `webhook_events` row is inserted first and the transaction then rolls back, so a
    // forged order id leaves **nothing** behind. That is deliberate: an unauthenticated
    // caller must not be able to write rows to this table.
    let Some(payment_id) = apply::payment_for_order(&mut tx, provider, &event.order_id).await?
    else {
        return Err(AppError::NotFound);
    };

    let applied = apply::apply_paid(&mut tx, state, payment_id, event).await?;

    sqlx::query("update webhook_events set payment_id = $2, processed_at = now() where id = $1")
        .bind(inserted)
        .bind(payment_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("closing the event: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing the webhook: {error}")))?;

    Ok(WebhookOutcome {
        duplicate: false,
        status: status_of(&applied).to_string(),
    })
}

/// The status the gateway is told about an applied payment.
fn status_of(applied: &Applied) -> &'static str {
    match applied {
        Applied::Activated { .. } => "activated",
        Applied::Extended { .. } => "extended",
        Applied::Credits { .. } => "credits",
        Applied::AlreadyPaid => "already_paid",
        Applied::Review { .. } => "review",
    }
}

/// The peer address, from the headers a reverse proxy sets.
///
/// `X-Forwarded-For`'s **last** entry, not the first: the first is what the client
/// claimed, and the last is what the nearest trusted hop observed. Neither is trustworthy
/// without a proxy that overwrites the header — which is why the allowlist is opt-in and
/// the signature is required regardless. This is a second lock on the door, not the lock.
fn peer_ip(headers: &HeaderMap) -> Option<std::net::IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next_back())
        .map(str::trim)
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
        })
}
