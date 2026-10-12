//! The QRIS gateway: the wire, and the one file B1 changes.
//!
//! `crates/core/src/traits/payment.rs` is the **contract** — the trait, its DTOs and
//! `PaymentError` — and slice A landed it. This module is the only implementation the
//! product ships, plus the server-side capability the contract does not carry.
//!
//! **B1 is unresolved**, so the design is the one that survives the docs arriving: the
//! field names live in [`mapping`] (the only file that changes), the signature scheme
//! is a closed set parsed at boot ([`signature`], A292), and every value is env-driven
//! ([`crate::config::QrisConfig`]). Every behavioural test in this phase runs against
//! [`stvdy_core::traits::testing::MockPayment`], so the suite that proves idempotency,
//! amount-checking and the period rules keeps passing on the day B1 resolves.

pub mod mapping;
pub mod qris;
pub mod signature;

pub use qris::QrisGateway;

use chrono::{DateTime, Utc};

use stvdy_core::traits::payment::{GatewayStatus, PaymentError, PaymentProvider};

/// One transaction as the gateway reports it, for the reconciliation sweep (A301).
///
/// `status` uses the same [`GatewayStatus`] vocabulary a status query answers with, so
/// the sweep and the poller agree about what "paid" means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTransaction {
    pub order_id: String,
    pub status: GatewayStatus,
    pub amount_idr: i64,
}

/// The capability this server needs **on top of** the contract's trait.
///
/// `PaymentProvider` is committed and does not have a listing method, but
/// `reconcile_payments` has to see the gateway's recent transactions to notice a payment
/// we have no row for. Adding the method to the core trait would change a committed
/// contract for one caller, so it lives here — one method, and the mock used in tests
/// provides it through [`crate::testing::TestGateway`].
#[async_trait::async_trait]
pub trait PaymentGateway: PaymentProvider {
    /// The gateway's transactions from the last `since_hours`.
    async fn recent_transactions(
        &self,
        since_hours: i64,
    ) -> Result<Vec<GatewayTransaction>, PaymentError>;
}

/// The instant a [`GatewayStatus`] says the money moved, if it says one.
///
/// One reader for the two callers that need it (the webhook's late-payment rule and the
/// poller's), so A295's "paid before the expiry" test cannot mean two things.
pub fn paid_at(status: &GatewayStatus) -> Option<DateTime<Utc>> {
    match status {
        GatewayStatus::Paid { paid_at, .. } => Some(*paid_at),
        GatewayStatus::Pending
        | GatewayStatus::Expired
        | GatewayStatus::Failed
        | GatewayStatus::Refunded => None,
    }
}

/// The fee a [`GatewayStatus`] reports, if it reports one.
pub fn fee_idr(status: &GatewayStatus) -> Option<i64> {
    match status {
        GatewayStatus::Paid { fee_idr, .. } => *fee_idr,
        _ => None,
    }
}
