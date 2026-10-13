//! The gopay-bridge wire suite: the requests we send, the responses we accept, and the
//! failures that must stay distinguishable.
//!
//! B1 is resolved, so this file is no longer a test of an invented contract — it is the
//! test of the **real bridge's** API, exercised against a stub HTTP server so a bridge
//! whose field is renamed fails here rather than in production. The three facts it pins are
//! the three the money path depends on: the bridge mints its own payment id, it assigns the
//! amount, and it answers with a bare transaction array.
//!
//! Test code may unwrap and panic; `#[sqlx::test]` is a macro harness, so the
//! relaxations in clippy.toml are not guaranteed to apply.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sqlx::PgPool;
use stvdy_core::traits::payment::{GatewayStatus, InvoiceRequest, PaymentError, PaymentProvider};
use stvdy_server::config::AppConfig;
use stvdy_server::integrations::payment::{PaymentGateway, QrisGateway};
use stvdy_server::testing::{TestState, root};

mod common;
use common::stub_http_json;

/// A bridge config pointing at `base_url`, built through the real env parser so the
/// validation path is the one that ships.
fn config_at(base_url: &str) -> AppConfig {
    AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", base_url),
            ("QRIS_MERCHANT_ID", "merchant-1"),
        ],
    )
    .expect("a bridge config with a base URL must load")
}

fn gateway_at(base_url: &str) -> QrisGateway {
    let config = config_at(base_url);
    QrisGateway::from_config(config.qris.as_ref().expect("a bridge config")).expect("a gateway")
}

fn request() -> InvoiceRequest {
    InvoiceRequest {
        order_id: "ord_1".to_string(),
        amount_idr: 5_000,
        description: "Stvdy Pro (monthly)".to_string(),
        expires_at: chrono::DateTime::parse_from_rfc3339("2026-10-12T09:15:00Z")
            .expect("a fixed instant")
            .with_timezone(&chrono::Utc),
    }
}

/// A created invoice: the path, the body, and the fact that **no credential travels** —
/// the bridge is unauthenticated by design, so a header here would be a secret sent to a
/// service that does not check it.
#[sqlx::test(migrations = "../../migrations")]
async fn the_invoice_request_is_the_bridges_shape_and_carries_no_credential(pool: PgPool) {
    let _ = pool;
    let stub = stub_http_json(
        200,
        serde_json::json!({
            "id": "gw-7f2a",
            "status": "pending",
            "amount": 5_000,
            "uniqueAmount": 5_137,
            "qrString": "00020101021226...",
            "reference": "ord_1",
            "createdAt": "2026-10-12T09:00:00Z",
            "expiresAt": "2026-10-12T09:15:00Z",
            "secondsLeft": 900,
        })
        .to_string(),
    )
    .await;

    let invoice = gateway_at(stub.base_url())
        .create_invoice(request())
        .await
        .expect("the stub answers a valid invoice");

    let sent = stub.requests();
    assert_eq!(sent.len(), 1, "one request, not a retry");
    assert_eq!(sent[0].method, "POST");
    assert_eq!(sent[0].path, "/payments");
    assert_eq!(
        sent[0].header("authorization"),
        None,
        "the bridge has no auth header; a credential sent to it would be a secret in a log"
    );

    let body: serde_json::Value =
        serde_json::from_str(&sent[0].body).expect("the body we sent is JSON");
    assert_eq!(body["amount"], 5_000);
    assert!(
        body["amount"].is_i64(),
        "an amount that arrives as a string is an amount a gateway cannot check"
    );
    // Our order id travels as the bridge's `reference`, never as its id: the bridge keys
    // its own routes on the id it mints.
    assert_eq!(body["reference"], "ord_1");
    assert_eq!(
        body["minutes"], 15,
        "the bridge takes a lifetime and computes the deadline itself"
    );

    // And the response was parsed, including the two fields that are not ours.
    assert_eq!(invoice.order_id, "ord_1");
    assert_eq!(invoice.qr_string, "00020101021226...");
    assert_eq!(
        invoice.provider_payment_id.as_deref(),
        Some("gw-7f2a"),
        "the bridge's id is the handle every later question is asked with"
    );
    assert_eq!(
        invoice.payable_amount_idr,
        Some(5_137),
        "the offset is the bridge's, and it is the figure the buyer must transfer"
    );
    assert_eq!(
        invoice.expires_at,
        chrono::DateTime::parse_from_rfc3339("2026-10-12T09:15:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        "the bridge's deadline is authoritative, not the one we computed before the call"
    );
}

/// The status route, keyed by the id the bridge minted — and the failures that must stay
/// apart, because the poller treats them differently.
#[sqlx::test(migrations = "../../migrations")]
async fn a_payment_is_read_by_the_bridges_own_id(pool: PgPool) {
    let _ = pool;

    let paid = stub_http_json(
        200,
        serde_json::json!({
            "id": "gw-7f2a",
            "status": "paid",
            "amount": 5_000,
            "uniqueAmount": 5_137,
            "qrString": "00020101021226...",
            "reference": "ord_1",
            "expiresAt": "2026-10-12T09:15:00Z",
            "transaction": {
                "id": "tx-1",
                "amount": 5_137,
                "time": "2026-10-12T09:10:00Z",
            },
        })
        .to_string(),
    )
    .await;

    let status = gateway_at(paid.base_url())
        .get_status("gw-7f2a")
        .await
        .expect("the stub answers a paid payment");

    assert_eq!(paid.requests()[0].method, "GET");
    assert_eq!(
        paid.requests()[0].path,
        "/payment/gw-7f2a",
        "the route is keyed by the bridge's id, not our order id"
    );

    match status {
        GatewayStatus::Paid {
            amount_idr,
            paid_at,
            provider_payment_id,
            fee_idr,
        } => {
            assert_eq!(
                amount_idr, 5_137,
                "what landed is the unique amount, not what we asked for"
            );
            assert_eq!(
                paid_at,
                chrono::DateTime::parse_from_rfc3339("2026-10-12T09:10:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                "the settlement instant is the bridge's; our clock would decide A295"
            );
            assert_eq!(provider_payment_id.as_deref(), Some("gw-7f2a"));
            assert_eq!(
                fee_idr, None,
                "the bridge reports no fee; a guess would corrupt the margin report"
            );
        }
        other => panic!("expected a paid status, got {other:?}"),
    }

    // The two dead states stay apart: `cancelled` is a person, `expired` is the clock, and
    // the row records which one it was.
    for (raw, expected) in [
        ("pending", GatewayStatus::Pending),
        ("expired", GatewayStatus::Expired),
        ("cancelled", GatewayStatus::Cancelled),
    ] {
        let stub = stub_http_json(200, serde_json::json!({ "status": raw }).to_string()).await;
        assert_eq!(
            gateway_at(stub.base_url())
                .get_status("gw-1")
                .await
                .unwrap(),
            expected
        );
    }

    // A payment the bridge does not know is `UnknownOrder`, not a transport failure: the
    // poller turns the first into `review` and would retry the second forever.
    let missing = stub_http_json(404, "{}").await;
    assert!(matches!(
        gateway_at(missing.base_url())
            .get_status("gw-missing")
            .await,
        Err(PaymentError::UnknownOrder)
    ));

    // A bridge error is `Provider`, which is retryable, and a malformed body is `Invalid`,
    // which is not — the two must not collapse into one.
    let broken = stub_http_json(500, "{}").await;
    assert!(matches!(
        gateway_at(broken.base_url()).get_status("gw-1").await,
        Err(PaymentError::Provider(_))
    ));
    let malformed = stub_http_json(200, "not json").await;
    assert!(matches!(
        gateway_at(malformed.base_url()).get_status("gw-1").await,
        Err(PaymentError::Invalid(_))
    ));
    // A status word the bridge does not have is `Invalid` too, rather than a default that
    // would read as "still pending" forever.
    let unknown = stub_http_json(
        200,
        serde_json::json!({ "status": "settlement" }).to_string(),
    )
    .await;
    assert!(matches!(
        gateway_at(unknown.base_url()).get_status("gw-1").await,
        Err(PaymentError::Invalid(_))
    ));

    // A `paid` answer with no transaction behind it is refused: the settlement instant is
    // the one fact A295's late-payment rule turns on, and substituting our clock for a
    // missing one would silently decide it.
    let bare_paid = stub_http_json(
        200,
        serde_json::json!({ "status": "paid", "uniqueAmount": 5_137 }).to_string(),
    )
    .await;
    assert!(matches!(
        gateway_at(bare_paid.base_url()).get_status("gw-1").await,
        Err(PaymentError::Invalid(_))
    ));
}

/// An invoice we cannot render or cannot check an amount against is `Invalid`, not an empty
/// string and not a default: the first would be a QR that encodes nothing and the second
/// would make every settlement mismatch.
#[sqlx::test(migrations = "../../migrations")]
async fn an_invoice_without_a_qr_or_a_payable_amount_is_refused(pool: PgPool) {
    let _ = pool;

    let no_qr = stub_http_json(
        200,
        serde_json::json!({
            "id": "gw-1",
            "uniqueAmount": 5_137,
            "reference": "ord_1",
            "expiresAt": "2026-10-12T09:15:00Z",
        })
        .to_string(),
    )
    .await;
    assert!(matches!(
        gateway_at(no_qr.base_url()).create_invoice(request()).await,
        Err(PaymentError::Invalid(_))
    ));

    let no_payable = stub_http_json(
        200,
        serde_json::json!({
            "id": "gw-1",
            "qrString": "00020101021226...",
            "reference": "ord_1",
            "expiresAt": "2026-10-12T09:15:00Z",
        })
        .to_string(),
    )
    .await;
    assert!(matches!(
        gateway_at(no_payable.base_url())
            .create_invoice(request())
            .await,
        Err(PaymentError::Invalid(_))
    ));

    // An empty QR string is not a valid one either: the client renders this verbatim, and a
    // blank payload is a blank screen rather than a payable code.
    let empty_qr = stub_http_json(
        200,
        serde_json::json!({
            "id": "gw-1",
            "qrString": "",
            "uniqueAmount": 5_137,
            "reference": "ord_1",
            "expiresAt": "2026-10-12T09:15:00Z",
        })
        .to_string(),
    )
    .await;
    assert!(
        matches!(
            gateway_at(empty_qr.base_url())
                .create_invoice(request())
                .await,
            Err(PaymentError::Invalid(_))
        ),
        "an empty QR payload must not reach the screen"
    );
}

/// A bridge with no `QRIS_MERCHANT_ID` must not boot: the reconciliation sweep is the only
/// thing that notices money the gateway took with no row behind it, and a sweep that cannot
/// name a merchant silently learns nothing (A301).
#[test]
fn a_bridge_with_no_merchant_id_fails_startup() {
    let error = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", "http://gopay-bridge:8787"),
        ],
    )
    .expect_err("a bridge with no merchant id must not boot");
    assert!(
        error.to_string().contains("QRIS_MERCHANT_ID"),
        "the boot error must name the key, got {error}"
    );

    // No gateway at all is a supported deployment: nothing to sell, and the checkout route
    // answers the honest 502 rather than minting an invoice against a double.
    let none = AppConfig::from_env_at(&root(), &[("APP_ENV", "dev"), ("QRIS_BASE_URL", "")])
        .expect("the default config loads");
    assert!(none.qris.is_none());

    // And a merchant id with no gateway is not a gateway: the config is driven by the base
    // URL, so a stray id changes nothing.
    let stray = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", ""),
            ("QRIS_MERCHANT_ID", "merchant-1"),
        ],
    )
    .expect("the default config loads");
    assert!(stray.qris.is_none());
}

/// An unset `QRIS_BASE_URL` is a supported deployment: the server has no gateway, and
/// `AppState::payment` says so with the honest 502 rather than minting an invoice against
/// a double.
///
/// The route-level half of this — `POST /billing/checkout` answering 502 `provider_error`
/// — is in `tests/billing.rs`, where the route exists (Task 4).
#[sqlx::test(migrations = "../../migrations")]
async fn a_deployment_with_no_gateway_has_none_and_says_so(pool: PgPool) {
    let harness = TestState::new(pool).without_gateway();

    assert!(harness.state.config.qris.is_none());
    let error = harness
        .state
        .payment()
        .err()
        .expect("a deployment with no gateway must refuse");
    assert_eq!(error.code(), "provider_error");
    assert_eq!(error.status(), axum::http::StatusCode::BAD_GATEWAY);
    assert!(harness.state.qris().is_err());
}

/// The transaction list the reconciliation sweep reads: a **bare array** of settled money,
/// bounded by the merchant and a window in minutes.
#[sqlx::test(migrations = "../../migrations")]
async fn the_transaction_list_is_the_bridges_bare_array(pool: PgPool) {
    let _ = pool;
    let stub = stub_http_json(
        200,
        serde_json::json!([
            {
                "id": "tx-1",
                "orderId": "gopay-99",
                "status": "settlement",
                "paymentType": "qris",
                "amount": 5_137,
                "realAmount": 5_037,
                "currency": "IDR",
                "time": "2026-10-12T09:10:00Z",
            },
            {
                "id": "tx-2",
                "orderId": "gopay-100",
                "status": "capture",
                "paymentType": "gopay",
                "amount": 10_004,
                "realAmount": 10_004,
                "currency": "IDR",
                "time": "2026-10-12T09:11:00Z",
            },
        ])
        .to_string(),
    )
    .await;

    let listed = gateway_at(stub.base_url())
        .recent_transactions(48)
        .await
        .expect("the stub answers a list");

    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].transaction_id, "tx-1");
    assert_eq!(
        listed[0].amount_idr, 5_137,
        "the gross amount is what the buyer transferred, which is the join key"
    );
    assert_eq!(
        listed[1].amount_idr, 10_004,
        "the fee is the provider's business; the join is on the gross"
    );

    // Hours become minutes, because the bridge bounds its own window that way, and the
    // merchant is named: a sweep with no merchant has nothing to compare against.
    let path = &stub.requests()[0].path;
    assert!(path.starts_with("/transactions?"), "{path}");
    assert!(path.contains("merchant_id=merchant-1"), "{path}");
    assert!(path.contains("minutes=2880"), "{path}");

    // The internal contract's wrapper is not this gateway's shape, so it is refused rather
    // than read as an empty list — an empty list would mean "the gateway has no
    // transactions", which is the one answer the sweep must not invent.
    let wrapped = stub_http_json(200, serde_json::json!({ "transactions": [] }).to_string()).await;
    assert!(matches!(
        gateway_at(wrapped.base_url()).recent_transactions(48).await,
        Err(PaymentError::Invalid(_))
    ));
}

/// The fee the gateway reports is used where it reports one, and the configured rate is
/// the fallback — `None` by default, which means "unknown, record nothing" (A304).
#[sqlx::test(migrations = "../../migrations")]
async fn the_fee_falls_back_to_the_configured_rate_only_when_configured(pool: PgPool) {
    let _ = pool;
    let stub = stub_http_json(200, "{}").await;
    let config = config_at(stub.base_url());
    let qris = config.qris.as_ref().unwrap().clone();

    assert_eq!(
        qris.fee_idr_for(10_000),
        None,
        "the shipped default records nothing rather than guessing a rate"
    );

    let config = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", stub.base_url()),
            ("QRIS_MERCHANT_ID", "merchant-1"),
            ("QRIS_FEE_PERCENT", "0.7"),
            ("QRIS_FEE_FIXED_IDR", "100"),
        ],
    )
    .expect("a fee rate loads");

    assert_eq!(
        config.qris.as_ref().unwrap().fee_idr_for(10_000),
        Some(100 + 70)
    );
    // A fixed fee with no percentage is still a known fee; a rate of zero with no fixed fee
    // is "unknown", which is a different fact from a real zero.
    let config = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", stub.base_url()),
            ("QRIS_MERCHANT_ID", "merchant-1"),
            ("QRIS_FEE_FIXED_IDR", "500"),
        ],
    )
    .expect("a fixed fee loads");
    assert_eq!(config.qris.as_ref().unwrap().fee_idr_for(10_000), Some(500));
}
