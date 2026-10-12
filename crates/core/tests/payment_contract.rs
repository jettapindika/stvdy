//! Contract § 1's `PaymentProvider`, its DTOs, and the mock gateway that signs.
//!
//! Two things this file exists to prove. First, the trait is the only thing every
//! later task in the phase calls, and it is object-safe behind
//! `Arc<dyn PaymentProvider>`, so the real QRIS adapter — whose vendor is still
//! unknown (B1) — is a drop-in for the double. Second, and the reason the double
//! computes a real HMAC rather than returning a canned event: the webhook suite in a
//! later slice asserts that a tampered body is *rejected*. A mock that could not fail
//! would make that whole suite vacuous, which is the same defect class as a
//! determinism fixture whose inputs were all distinguishable.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use http::HeaderMap;
use serde_json::json;
use sha2::Sha256;
use stvdy_core::traits::payment::{
    GatewayStatus, InvoiceRequest, PaymentError, PaymentProvider, WebhookKind,
};
use stvdy_core::traits::testing::MockPayment;

/// The header the mock signs with. Spelled out here rather than imported, so a
/// rename in the double is a compile error in this file instead of a test that
/// quietly asserts nothing.
const SIGNATURE_HEADER: &str = "x-mock-signature";

/// The secret `MockPayment::new` uses.
const SECRET: &[u8] = b"mock-secret";

/// A `paid` callback, serialised once so the bytes that are signed are the bytes
/// that are parsed.
fn paid_body(amount_idr: i64, order_id: &str) -> Vec<u8> {
    json!({
        "event_id": format!("evt-{order_id}-{amount_idr}"),
        "kind": "paid",
        "order_id": order_id,
        "amount_idr": amount_idr,
        "currency": "IDR",
        "paid_at": "2026-10-12T03:05:00Z",
        "provider_payment_id": "gw-9",
        "fee_idr": 150,
    })
    .to_string()
    .into_bytes()
}

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
    assert_eq!(provider.get_status("ord-1").await.unwrap(), paid);

    // The status queue is scripted, not a constant. Once it is empty, asking again
    // is an error naming the missing script rather than a silent `Pending`, so a
    // poller test that forgot to queue a status cannot pass by accident.
    match provider.get_status("ord-1").await.unwrap_err() {
        PaymentError::Provider(message) => {
            assert!(message.contains("no queued status"), "got: {message}");
        }
        other => panic!("expected a Provider error, got {other:?}"),
    }

    let body = paid_body(5_000, "ord-1");
    let headers = mock.sign(&body);
    let event = provider.parse_and_verify_webhook(&headers, &body).unwrap();
    assert_eq!(event.event_id, "evt-ord-1-5000");
    assert_eq!(event.kind, WebhookKind::Paid);
    assert_eq!(event.order_id, "ord-1");
    assert_eq!(event.amount_idr, 5_000);
    assert_eq!(event.currency, "IDR");
    assert_eq!(event.paid_at, Some(paid_at));
    assert_eq!(event.provider_payment_id.as_deref(), Some("gw-9"));
    assert_eq!(event.fee_idr, Some(150));
    assert_eq!(
        event.raw["kind"], "paid",
        "the raw payload is kept, because the audit trail is the gateway's own words"
    );

    // `refund_note` is record-only for the MVP; it records both halves of the note.
    provider.refund_note("ord-1", 5_000).await.unwrap();
    assert_eq!(mock.refund_notes(), vec![("ord-1".to_string(), 5_000)]);
}

/// A callback carries no payment fields until it is a `paid` one: `expired` and
/// `failed` have neither a `paid_at` nor a fee, and requiring them would make the
/// parser reject exactly the callbacks that tell us to stop waiting.
#[tokio::test]
async fn a_callback_without_the_payment_fields_still_parses() {
    let mock = MockPayment::new();
    let body = json!({
        "event_id": "evt-expired-1",
        "kind": "expired",
        "order_id": "ord-1",
        "amount_idr": 5_000,
        "currency": "IDR",
    })
    .to_string()
    .into_bytes();

    let event = mock
        .parse_and_verify_webhook(&mock.sign(&body), &body)
        .unwrap();
    assert_eq!(event.kind, WebhookKind::Expired);
    assert_eq!(event.paid_at, None);
    assert_eq!(event.fee_idr, None);
    assert_eq!(event.provider_payment_id, None);
}

/// The mock signs the bytes it is given and rejects any other signature, so a
/// webhook test that passes is a webhook test that actually verified something.
#[tokio::test]
async fn the_mock_verifies_its_own_signature_and_rejects_a_forged_one() {
    let mock = MockPayment::new();
    let body = paid_body(5_000, "ord-1");
    let headers = mock.sign(&body);

    assert_eq!(
        mock.parse_and_verify_webhook(&headers, &body)
            .unwrap()
            .amount_idr,
        5_000,
        "the happy path first, so the rejections below are not passing vacuously"
    );

    // The signature is a real HMAC-SHA256 over these exact bytes, recomputed here
    // with the primitives rather than compared against the double's own helper.
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(SECRET).unwrap();
    mac.update(&body);
    let expected = hex::encode(mac.finalize().into_bytes());
    assert_eq!(
        headers.get(SIGNATURE_HEADER).unwrap().to_str().unwrap(),
        expected,
        "a constant here would pass every test that only checks the happy path"
    );

    // The assertion that matters: swap the BODY and keep the HEADERS. The forged
    // body is itself valid JSON with a different amount, so only the signature can
    // catch it.
    let forged = paid_body(1, "ord-1");
    assert!(matches!(
        mock.parse_and_verify_webhook(&headers, &forged),
        Err(PaymentError::Signature)
    ));

    // Re-serialising the same event is not the same bytes, which is the whole
    // reason the trait takes `&[u8]` instead of a parsed body: a framework that
    // parses and re-encodes before verifying has already destroyed the evidence.
    let mut respaced = body.clone();
    respaced.push(b'\n');
    assert!(matches!(
        mock.parse_and_verify_webhook(&headers, &respaced),
        Err(PaymentError::Signature)
    ));

    // Another gateway's secret cannot sign for this one.
    let other = MockPayment::with_secret(b"not-the-secret");
    assert!(matches!(
        mock.parse_and_verify_webhook(&other.sign(&body), &body),
        Err(PaymentError::Signature)
    ));

    // No header at all is a failed verification, never a pass.
    assert!(matches!(
        mock.parse_and_verify_webhook(&HeaderMap::new(), &body),
        Err(PaymentError::Signature)
    ));

    // A correctly signed body that is not an event is `Invalid`, not `Signature` and
    // not a panic: the two failures stay distinguishable, because one is an attack
    // and the other is a malformed message.
    let garbage = b"not json at all".to_vec();
    assert!(matches!(
        mock.parse_and_verify_webhook(&mock.sign(&garbage), &garbage),
        Err(PaymentError::Invalid(_))
    ));

    let wrong_shape = json!({ "event_id": "evt-1" }).to_string().into_bytes();
    assert!(matches!(
        mock.parse_and_verify_webhook(&mock.sign(&wrong_shape), &wrong_shape),
        Err(PaymentError::Invalid(_))
    ));

    // Verification happens *before* parsing: a tampered body that is also malformed
    // must report the signature, or the route's 401/400 mapping would follow the
    // attacker's choice of bytes.
    let mut tampered_garbage = b"not json at all".to_vec();
    tampered_garbage.push(b'!');
    assert!(matches!(
        mock.parse_and_verify_webhook(&headers, &tampered_garbage),
        Err(PaymentError::Signature)
    ));
}
