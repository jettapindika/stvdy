//! The payment gateway client, and the one capability the server needs on top of the
//! core contract.
//!
//! B1 is resolved: the gateway is a self-hosted QRIS bridge, and
//! [`mapping`](self::mapping) is the single file its wire shape lives in. Everything
//! here is transport, plus the two facts the rest of the money path reads:
//!
//!  * the bridge is **polled**, so [`PaymentGateway::recent_transactions`] exists for
//!    the reconciliation sweep, which needs to see money the gateway received and we
//!    have no row for (A301). `PaymentProvider` is a committed contract and has no
//!    listing method; adding one for a single caller would change it for every
//!    implementation, so the method lives here and the test double provides it.
//!  * the bridge assigns the **amount**, so the invoice's `payable_amount_idr` is what
//!    the buyer transfers and what a settlement is matched against.

pub mod mapping;
pub mod qris;

pub use qris::QrisGateway;

use chrono::{DateTime, Utc};

use stvdy_core::traits::payment::{PaymentError, PaymentProvider};

/// One settled transaction as the gateway reports it, for the reconciliation sweep.
///
/// There is no status field, and that is not an omission: the bridge's feed route is
/// queried with `statuses=settlement,capture`, so every entry it returns is money that
/// arrived. A `status` here could only ever repeat that.
///
/// `amount_idr` is the gross amount and it is the **join key**, because the feed's own
/// `orderId` is the provider's and not ours. It is the same figure the bridge matches
/// its own settlements by, and the same figure `payments.payable_amount_idr` holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTransaction {
    pub transaction_id: String,
    pub amount_idr: i64,
    pub paid_at: DateTime<Utc>,
}

/// The capability this server needs **on top of** the contract's trait.
#[async_trait::async_trait]
pub trait PaymentGateway: PaymentProvider {
    /// The gateway's settled transactions from the last `since_hours`.
    async fn recent_transactions(
        &self,
        since_hours: i64,
    ) -> Result<Vec<GatewayTransaction>, PaymentError>;
}
