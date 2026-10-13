//! Contract § 1's `PaymentProvider`, its DTOs, and the scripted gateway double.
//!
//! What this file exists to prove is narrow, because the gateway is polled: the trait
//! is object-safe behind `Arc<dyn PaymentProvider>`, the status queue is scripted
//! rather than constant, and `Invoice::payable_amount_idr` is optional — `None` means
//! "the gateway charges exactly what it was asked for", which is the case every
//! existing checkout test relies on.
//!
//! The signing machinery this file used to carry is gone with the callback path: a
//! gateway that never pushes has nothing to verify, so an HMAC double here would be
//! testing a mechanism the product no longer has.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use stvdy_core::traits::payment::{GatewayStatus, InvoiceRequest, PaymentError, PaymentProvider};
use stvdy_core::traits::testing::MockPayment;

#[tokio::test]
async fn the_payment_trait_is_object_safe_and_a_double_drives_every_method() {
    // Object safety, asserted by the compiler: a method taking `Self` or a generic
    // parameter would make this call impossible, and the server holds the trait
    // behind an `Arc<dyn PaymentProvider>`.
    fn drive(provider: &dyn PaymentProvider) -> &'static str {
        provider.name()
    }

    let mock = Arc::new(MockPayment::new());
    let provider: Arc<dyn PaymentProvider> = mock.clone();
    assert_eq!(drive(provider.as_ref()), "mock");
    assert_eq!(provider.name(), "mock");

    let expires_at = Utc.with_ymd_and_hms(2026, 10, 12, 3, 15, 0).unwrap();
    let request = InvoiceRequest {
        order_id: "ord-1".to_string(),
        amount_idr: 5_000,
        description: "Pro, monthly".to_string(),
        expires_at,
    };

    let invoice = provider.create_invoice(request.clone()).await.unwrap();
    assert_eq!(invoice.order_id, "ord-1");
    assert_eq!(
        invoice.expires_at, expires_at,
        "the invoice expiry is the server's instant, not the gateway's"
    );
    assert!(
        !invoice.qr_string.is_empty(),
        "the client renders this string; an empty one is a blank screen"
    );
    assert_eq!(
        invoice.provider_payment_id.as_deref(),
        Some("mock-ord-1"),
        "the gateway's own id is the handle every later status question is asked with"
    );
    assert_eq!(
        invoice.payable_amount_idr, None,
        "the double charges what it is asked for; only the real gateway adds an offset"
    );
    assert_eq!(
        mock.invoices(),
        vec![request],
        "the double records what it was asked for, so a checkout test can assert the amount"
    );

    let paid_at = Utc.with_ymd_and_hms(2026, 10, 12, 3, 5, 0).unwrap();
    let paid = GatewayStatus::Paid {
        paid_at,
        amount_idr: 5_000,
        provider_payment_id: Some("gw-9".to_string()),
        fee_idr: Some(150),
    };
    mock.push_status(paid.clone());
    assert_eq!(provider.get_status("mock-ord-1").await.unwrap(), paid);

    // The status queue is scripted, not a constant. Once it is empty, asking again
    // is an error naming the missing script rather than a silent `Pending`, so a
    // poller test that forgot to queue a status cannot pass by accident.
    match provider.get_status("mock-ord-1").await.unwrap_err() {
        PaymentError::Provider(message) => {
            assert!(message.contains("no queued status"), "got: {message}");
        }
        other => panic!("expected a Provider error, got {other:?}"),
    }

    // `refund_note` is record-only for the MVP; it records both halves of the note.
    provider.refund_note("ord-1", 5_000).await.unwrap();
    assert_eq!(mock.refund_notes(), vec![("ord-1".to_string(), 5_000)]);
}

/// The statuses are the contract's, and the two "the payment is dead" cases are
/// distinct values rather than one: `Expired` is the clock, `Cancelled` is a person,
/// and the row records which of them it was.
#[tokio::test]
async fn the_status_vocabulary_carries_the_dead_cases_apart() {
    let mock = MockPayment::new();
    for status in [
        GatewayStatus::Expired,
        GatewayStatus::Cancelled,
        GatewayStatus::Failed,
        GatewayStatus::Refunded,
    ] {
        mock.push_status(status.clone());
        assert_eq!(mock.get_status("mock-ord-1").await.unwrap(), status);
    }
}

/// A gateway that does not charge the requested amount says so in the invoice, and
/// the value survives serialisation: it is the figure the checkout screen prints and
/// the figure the settlement is checked against, so it cannot be a computed default.
#[test]
fn an_invoice_carries_the_gateways_payable_amount() {
    let invoice = stvdy_core::traits::payment::Invoice {
        order_id: "ord-1".to_string(),
        qr_string: "QRIS".to_string(),
        expires_at: Utc.with_ymd_and_hms(2026, 10, 12, 3, 15, 0).unwrap(),
        provider_payment_id: Some("gw-1".to_string()),
        payable_amount_idr: Some(5_137),
    };

    let encoded = serde_json::to_string(&invoice).unwrap();
    assert!(encoded.contains("\"payable_amount_idr\":5137"), "{encoded}");
    assert_eq!(
        serde_json::from_str::<stvdy_core::traits::payment::Invoice>(&encoded).unwrap(),
        invoice
    );
}
