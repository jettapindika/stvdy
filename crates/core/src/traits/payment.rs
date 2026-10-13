//! Payment gateway capability.
//!
//! **The gateway is polled, and this contract has no callback method because of it.**
//! B1 is resolved: the gateway is a self-hosted QRIS bridge that derives a dynamic QRIS
//! from the merchant's static payload and reports the outcome only through its own
//! status route — it never pushes. So the poller is the single ingress and
//! [`billing::apply::apply_paid`](crate::billing) is the single activation path, which
//! is what makes a payment recovered by the client's poll and one recovered by the
//! five-minute sweep indistinguishable.
//!
//! **`payable_amount_idr` exists because the gateway assigns a unique amount.** The
//! bridge adds an offset in `[1, 999]` to the amount we ask for, so that it can match a
//! settlement by amount alone without a per-order callback. That offset is the one
//! number in the money path the server does not compute, and it is why
//! [`Invoice`] carries it: the amount quoted to the student, the amount the QR encodes
//! and the amount the settlement is checked against have to be the same figure, and
//! only the gateway knows it.

use chrono::{DateTime, Utc};

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
    /// The id the **gateway** minted, which is the handle every later question about
    /// this payment is asked with. It is not `order_id`: our order id is only the
    /// reference the gateway stores beside the payment.
    pub provider_payment_id: Option<String>,
    /// The amount the buyer must actually transfer, when it differs from
    /// [`InvoiceRequest::amount_idr`].
    ///
    /// `None` means the gateway charges the requested amount exactly, and the caller
    /// falls back to `amount_idr`. The gateway's number is never allowed to *lower*
    /// what is charged: it is the buyer-facing figure for one invoice, and the row
    /// keeps the price the server computed.
    pub payable_amount_idr: Option<i64>,
}

/// The gateway's own view of a payment, which the poller reads (A301).
///
/// `fee_idr` is the gateway's own fee where it reports one, and `None` where it does
/// not: the margin report must be able to say "unknown" rather than record a
/// plausible-looking rate (A304).
///
/// The vocabulary is the contract's, and a gateway need not produce every variant:
/// the QRIS bridge answers `Pending`, `Paid`, `Expired` and `Cancelled` only, and
/// `Failed` and `Refunded` stay because a second gateway may say them.
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
    /// The gateway cancelled the invoice. Distinct from `Expired` because the reason
    /// differs — an operator or the buyer stopped it, rather than the clock — and the
    /// row records the gateway's own word for it.
    Cancelled,
    Failed,
    Refunded,
}

/// The kinds of event the money path applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GatewayEventKind {
    Paid,
    Expired,
    Failed,
    Refunded,
}

/// What the gateway said about one payment, as the money path reads it.
///
/// A value of this type comes from a **status query**, never from a request body:
/// this gateway has no callbacks, so there is no unverified payload that could reach
/// the amount check. `raw` is kept because `gateway_events.payload` is what a human
/// reads when the amount does not match (A294), and because the gateway's own words
/// are the only account of a disputed payment we have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayEvent {
    pub event_id: String,
    pub kind: GatewayEventKind,
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
/// `UnknownOrder` is separate from `Transport` because the two are answered
/// differently: an order the gateway has never heard of is a fact to record, while a
/// transport failure is an outage to retry. `NotConfigured` is a startup-visible state
/// — no gateway is wired — and must never be reported as a transport failure, because
/// a missing gateway is our fault and a slow one is not.
#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("payment gateway transport failure: {0}")]
    Transport(String),
    #[error("malformed payment gateway message: {0}")]
    Invalid(String),
    #[error("the payment gateway does not know this order")]
    UnknownOrder,
    #[error("payment gateway error: {0}")]
    Provider(String),
    #[error("no payment gateway is configured")]
    NotConfigured,
}

/// Creates invoices, reads their status, and records refunds.
///
/// The server is the only caller (roadmap § 1): a client never talks to the gateway,
/// and never reports a payment as paid.
#[async_trait::async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn create_invoice(&self, req: InvoiceRequest) -> Result<Invoice, PaymentError>;

    /// The gateway's view of one payment, by the id **the gateway minted** —
    /// [`Invoice::provider_payment_id`].
    ///
    /// It is deliberately not our `order_id`: the gateway keys its status route on its
    /// own identifier, and passing our order id would ask about a payment that does
    /// not exist under that name.
    async fn get_status(&self, provider_payment_id: &str) -> Result<GatewayStatus, PaymentError>;

    /// Record-only for the MVP: refunds are processed outside the app and recorded
    /// in `payments` (spec § Payments).
    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError>;
}
