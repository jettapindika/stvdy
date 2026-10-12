//! Payment gateway capability.
//!
//! One trait, and the whole phase is built on it: B1 (the gateway's identity and
//! docs) is unresolved, so the only honest design is one a documented field mapping
//! can be dropped into later. Every behavioural test in this phase runs against the
//! trait — `MockPayment` in [`testing`](super::testing) — and never against the
//! vendor, so the suite that proves idempotency, amount-checking and the period
//! rules keeps passing on the day the docs arrive.
//!
//! **`parse_and_verify_webhook` is synchronous and takes raw bytes**, and that is a
//! correctness constraint rather than a style choice. A signature is computed over
//! the bytes the gateway sent; a framework that deserialises the JSON and re-encodes
//! it before verification has destroyed the evidence it is checking. So the caller
//! hands over the body it received — axum's `Bytes` extractor, never `Json<T>` — and
//! the trait keeps that requirement visible in its signature.

use chrono::{DateTime, Utc};
use http::HeaderMap;

/// What the server asks the gateway to charge.
///
/// `amount_idr` is the server's own number, from
/// [`billing::price`](crate::billing::price) — never the client's, and never the
/// gateway's (A42).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InvoiceRequest {
    pub order_id: String,
    pub amount_idr: i64,
    pub description: String,
    pub expires_at: DateTime<Utc>,
}

/// The gateway's answer: the string a client renders as a QR code, and when it dies.
///
/// `expires_at` is an absolute instant (A6). The countdown the checkout screen shows
/// is this value rendered against the client's clock, never a duration the client
/// invents.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Invoice {
    pub order_id: String,
    pub qr_string: String,
    pub expires_at: DateTime<Utc>,
    pub provider_payment_id: Option<String>,
}

/// The gateway's own view of an order, which the poller reads when a callback is
/// lost (A301).
///
/// `fee_idr` is the gateway's own fee where it reports one, and `None` where it does
/// not: the margin report must be able to say "unknown" rather than record a
/// plausible-looking rate (A304).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayStatus {
    Pending,
    Paid {
        paid_at: DateTime<Utc>,
        amount_idr: i64,
        provider_payment_id: Option<String>,
        fee_idr: Option<i64>,
    },
    Expired,
    Failed,
    Refunded,
}

/// The kinds of callback a gateway sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookKind {
    Paid,
    Expired,
    Failed,
    Refunded,
}

/// A **verified** callback: the fields the money path reads, plus the payload they
/// came from.
///
/// A value of this type has already passed the signature check, so a caller cannot
/// reach the amount check with an unverified body. `raw` is kept because
/// `webhook_events.payload` is what a human reads when the amount does not match
/// (A294), and because the gateway's own words are the only account of a disputed
/// payment we have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEvent {
    pub event_id: String,
    pub kind: WebhookKind,
    pub order_id: String,
    pub amount_idr: i64,
    pub currency: String,
    pub paid_at: Option<DateTime<Utc>>,
    pub provider_payment_id: Option<String>,
    pub fee_idr: Option<i64>,
    pub raw: serde_json::Value,
}

/// A gateway failure.
///
/// `Signature` and `UnknownOrder` are separate variants because the webhook route
/// answers them differently (401 and 404): collapsing them into one "invalid" would
/// turn that mapping into a string match. `NotConfigured` is a startup-visible state
/// — no gateway is wired — and must never be reported as a verification failure,
/// because a missing gateway is our fault and a bad signature is not.
#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("payment gateway transport failure: {0}")]
    Transport(String),
    #[error("malformed payment gateway message: {0}")]
    Invalid(String),
    #[error("the webhook signature did not verify")]
    Signature,
    #[error("the payment gateway does not know this order")]
    UnknownOrder,
    #[error("payment gateway error: {0}")]
    Provider(String),
    #[error("no payment gateway is configured")]
    NotConfigured,
}

/// Creates invoices, reads their status, verifies callbacks, and records refunds.
///
/// The server is the only caller (roadmap § 1): a client never talks to the gateway,
/// and never reports a payment as paid.
#[async_trait::async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn create_invoice(&self, req: InvoiceRequest) -> Result<Invoice, PaymentError>;

    async fn get_status(&self, order_id: &str) -> Result<GatewayStatus, PaymentError>;

    /// Verifies the callback against `raw_body` and parses it.
    ///
    /// `raw_body` must be the bytes as received. Re-encoding a parsed body produces
    /// different bytes (key order, whitespace, number formatting) and the signature
    /// will not verify — which is a false rejection, and the reason this parameter is
    /// `&[u8]`.
    fn parse_and_verify_webhook(
        &self,
        headers: &HeaderMap,
        raw_body: &[u8],
    ) -> Result<WebhookEvent, PaymentError>;

    /// Record-only for the MVP: refunds are processed outside the app and recorded
    /// in `payments` (spec § Payments).
    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError>;
}
