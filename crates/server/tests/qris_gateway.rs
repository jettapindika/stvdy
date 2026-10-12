//! The QRIS wire suite: the request we send, the responses we accept, the three
//! signature schemes, and the two idempotency defences the money path depends on.
//!
//! B1 (the gateway's identity and docs) is unresolved, so the wire tests exercise the
//! **internal contract** — the field names in `integrations/payment/mapping.rs` and the
//! schemes in `signature.rs`. The day the docs arrive, the fixtures change; nothing else
//! in this phase does, because every behavioural test runs against the trait's mock.
//!
//! Test code may unwrap and panic; `#[sqlx::test]` is a macro harness, so the
//! relaxations in clippy.toml are not guaranteed to apply.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sqlx::PgPool;
use stvdy_core::traits::payment::{InvoiceRequest, PaymentError, PaymentProvider};
use stvdy_server::config::{AppConfig, SignatureScheme};
use stvdy_server::integrations::payment::{PaymentGateway, QrisGateway};
use stvdy_server::testing::{TestState, root};

mod common;
use common::stub_http_json;

/// A gateway config pointing at `base_url`, built through the real env parser so the
/// validation path is the one that ships.
fn config_at(base_url: &str, scheme: &str) -> AppConfig {
    AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", base_url),
            ("QRIS_API_KEY", "test-key"),
            ("QRIS_CALLBACK_TOKEN", "test-callback-token"),
            ("QRIS_SIGNATURE_SCHEME", scheme),
        ],
    )
    .expect("a gateway config with a base URL must load")
}

fn gateway_at(base_url: &str) -> QrisGateway {
    let config = config_at(base_url, "hmac_sha256_hex");
    QrisGateway::from_config(config.qris.as_ref().expect("a gateway config")).expect("a gateway")
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

/// The request we send is the internal contract, and B1 changes this test's fixtures and
/// nothing else — which is the whole point of isolating the mapping.
#[sqlx::test(migrations = "../../migrations")]
async fn the_gateway_sends_the_order_id_and_amount_the_contract_names(pool: PgPool) {
    let _ = pool;
    let stub = stub_http_json(
        200,
        serde_json::json!({
            "order_id": "ord_1",
            "qr_string": "00020101021226...",
            "expires_at": "2026-10-12T09:15:00Z",
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
    assert_eq!(sent[0].path, "/invoices");
    assert_eq!(
        sent[0].header("authorization"),
        Some("Bearer test-key"),
        "the API key travels in the auth header and nowhere else"
    );

    let body: serde_json::Value =
        serde_json::from_str(&sent[0].body).expect("the body we sent is JSON");
    assert_eq!(body["order_id"], "ord_1");
    assert_eq!(body["amount_idr"], 5_000);
    assert!(
        body["amount_idr"].is_i64(),
        "an amount that arrives as a string is an amount a gateway cannot check"
    );

    // And the response was parsed, not merely accepted.
    assert_eq!(invoice.order_id, "ord_1");
    assert_eq!(invoice.qr_string, "00020101021226...");
    assert_eq!(invoice.expires_at, request().expires_at);
}

/// The response shape we accept, against a stub HTTP server, so a vendor whose field is
/// named differently fails here rather than in production.
#[sqlx::test(migrations = "../../migrations")]
async fn the_gateway_parses_a_sandbox_shaped_invoice_and_status_response(pool: PgPool) {
    let _ = pool;

    let paid = stub_http_json(
        200,
        serde_json::json!({
            "status": "paid",
            "paid_at": "2026-10-12T09:10:00Z",
            "amount_idr": 5_000,
            "provider_payment_id": "gw_1",
            "fee_idr": 75,
        })
        .to_string(),
    )
    .await;

    let status = gateway_at(paid.base_url())
        .get_status("ord_1")
        .await
        .expect("the stub answers a paid status");

    assert_eq!(paid.requests()[0].method, "GET");
    assert_eq!(paid.requests()[0].path, "/invoices/ord_1");
    match status {
        stvdy_core::traits::payment::GatewayStatus::Paid {
            amount_idr,
            fee_idr,
            provider_payment_id,
            ..
        } => {
            assert_eq!(amount_idr, 5_000);
            assert_eq!(fee_idr, Some(75));
            assert_eq!(provider_payment_id.as_deref(), Some("gw_1"));
        }
        other => panic!("expected a paid status, got {other:?}"),
    }

    // An order the gateway does not know is `UnknownOrder`, not a transport failure: the
    // poller turns the first into `review` and would retry the second forever.
    let missing = stub_http_json(404, "{}").await;
    assert!(matches!(
        gateway_at(missing.base_url())
            .get_status("ord_missing")
            .await,
        Err(PaymentError::UnknownOrder)
    ));

    // A gateway error is `Provider`, which is retryable, and a malformed body is
    // `Invalid`, which is not — the two must not collapse into one.
    let broken = stub_http_json(500, "{}").await;
    assert!(matches!(
        gateway_at(broken.base_url()).get_status("ord_1").await,
        Err(PaymentError::Provider(_))
    ));
    let malformed = stub_http_json(200, "not json").await;
    assert!(matches!(
        gateway_at(malformed.base_url()).get_status("ord_1").await,
        Err(PaymentError::Invalid(_))
    ));

    // A response that is valid JSON but missing the QR string is `Invalid` too: an
    // invoice a student cannot pay must not become an empty QR.
    let no_qr = stub_http_json(
        200,
        serde_json::json!({ "order_id": "ord_1", "expires_at": "2026-10-12T09:15:00Z" })
            .to_string(),
    )
    .await;
    assert!(matches!(
        gateway_at(no_qr.base_url()).create_invoice(request()).await,
        Err(PaymentError::Invalid(_))
    ));
}

/// Each scheme verifies its own shape through the **real gateway**, and rejects the
/// others. Three schemes with one test between them would let a copy-paste make two of
/// them the same.
#[sqlx::test(migrations = "../../migrations")]
async fn every_signature_scheme_verifies_its_own_shape_and_rejects_the_others(pool: PgPool) {
    let _ = pool;
    let body = serde_json::json!({
        "event_id": "evt_1",
        "kind": "paid",
        "order_id": "ord_1",
        "amount_idr": 5_000,
        "currency": "IDR",
        "paid_at": "2026-10-12T09:10:00Z",
    })
    .to_string();

    let secret = b"test-callback-token";
    let hex_value = hex::encode(hmac_sha256(secret, body.as_bytes()));
    let base64_value = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(hmac_sha256(secret, body.as_bytes()))
    };

    for (scheme, header_value) in [
        ("hmac_sha256_hex", hex_value.as_str()),
        ("hmac_sha256_base64", base64_value.as_str()),
        ("shared_token", "test-callback-token"),
    ] {
        let config = config_at("http://127.0.0.1:1", scheme);
        let gateway =
            QrisGateway::from_config(config.qris.as_ref().expect("a gateway config")).unwrap();
        let headers = signed_headers("X-Callback-Signature", header_value);

        gateway
            .parse_and_verify_webhook(&headers, body.as_bytes())
            .unwrap_or_else(|error| panic!("{scheme} must verify its own shape: {error}"));

        // The same headers over a different body. For the two HMAC schemes this is the
        // assertion that matters, because a gateway that signed nothing would pass the
        // check above. `shared_token` deliberately does not bind the body — a shared token
        // is not a signature, which is exactly why it is the scheme an operator should stop
        // using the day B1 names something better.
        if scheme != "shared_token" {
            let tampered = body.replace("5000", "1");
            assert!(
                matches!(
                    gateway.parse_and_verify_webhook(&headers, tampered.as_bytes()),
                    Err(PaymentError::Signature)
                ),
                "{scheme} must reject a body swapped under a valid header"
            );
        }

        // A header from another secret does not verify either.
        let foreign = signed_headers(
            "X-Callback-Signature",
            &hex::encode(hmac_sha256(b"another-secret", body.as_bytes())),
        );
        assert!(
            matches!(
                gateway.parse_and_verify_webhook(&foreign, body.as_bytes()),
                Err(PaymentError::Signature)
            ),
            "{scheme} must reject a signature it did not make"
        );

        // And a missing header is a failure, never an empty-string match.
        assert!(matches!(
            gateway.parse_and_verify_webhook(&axum::http::HeaderMap::new(), body.as_bytes()),
            Err(PaymentError::Signature)
        ));
    }
}

/// A scheme we do not know must stop the process, because the alternative is a default
/// that verifies everything.
#[test]
fn an_unknown_signature_scheme_fails_startup() {
    let error = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", "https://gateway.example"),
            ("QRIS_API_KEY", "k"),
            ("QRIS_CALLBACK_TOKEN", "t"),
            ("QRIS_SIGNATURE_SCHEME", "md5"),
        ],
    )
    .expect_err("an unknown scheme must not boot");
    assert!(
        error.to_string().contains("QRIS_SIGNATURE_SCHEME"),
        "the boot error must name the key, got {error}"
    );

    // A gateway with no callback token must not boot either: the token is the HMAC key,
    // and an empty one verifies anything an attacker can compute.
    let error = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("QRIS_BASE_URL", "https://gateway.example"),
            ("QRIS_API_KEY", "k"),
            ("QRIS_CALLBACK_TOKEN", ""),
        ],
    )
    .expect_err("a gateway with no callback token must not boot");
    assert!(error.to_string().contains("QRIS_CALLBACK_TOKEN"), "{error}");

    // The scheme is parsed even with no gateway at all: a typo is an operator who believes
    // a scheme is in force.
    assert!(
        AppConfig::from_env_at(
            &root(),
            &[
                ("APP_ENV", "dev"),
                ("QRIS_BASE_URL", ""),
                ("QRIS_SIGNATURE_SCHEME", "hmac"),
            ],
        )
        .is_err()
    );

    // The three documented spellings parse, and no gateway means no `qris` config.
    for scheme in SignatureScheme::ALL {
        assert_eq!(SignatureScheme::parse(scheme.as_str()), Some(scheme));
    }
    let default = AppConfig::from_env_at(&root(), &[("APP_ENV", "dev"), ("QRIS_BASE_URL", "")])
        .expect("the default config loads");
    assert!(default.qris.is_none());
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

/// The transactions list the reconciliation sweep reads, against the wire.
#[sqlx::test(migrations = "../../migrations")]
async fn the_transaction_list_is_read_from_the_gateway(pool: PgPool) {
    let _ = pool;
    let stub = stub_http_json(
        200,
        serde_json::json!({
            "transactions": [
                {
                    "order_id": "ord_1",
                    "status": "paid",
                    "amount_idr": 5_000,
                    "paid_at": "2026-10-12T09:10:00Z",
                },
                { "order_id": "ord_2", "status": "pending", "amount_idr": 10_000 },
            ]
        })
        .to_string(),
    )
    .await;

    let listed = gateway_at(stub.base_url())
        .recent_transactions(48)
        .await
        .expect("the stub answers a list");

    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].order_id, "ord_1");
    assert_eq!(listed[1].order_id, "ord_2");
    assert_eq!(
        stub.requests()[0].path,
        "/transactions?since_hours=48",
        "the window is what bounds the sweep"
    );
}

/// The fee the gateway reports is used where it reports one, and the configured rate is
/// the fallback — `None` by default, which means "unknown, record nothing" (A304).
#[sqlx::test(migrations = "../../migrations")]
async fn the_fee_falls_back_to_the_configured_rate_only_when_configured(pool: PgPool) {
    let _ = pool;
    let stub = stub_http_json(200, "{}").await;
    let config = config_at(stub.base_url(), "hmac_sha256_hex");
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
            ("QRIS_API_KEY", "k"),
            ("QRIS_CALLBACK_TOKEN", "t"),
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
            ("QRIS_API_KEY", "k"),
            ("QRIS_CALLBACK_TOKEN", "t"),
            ("QRIS_FEE_FIXED_IDR", "500"),
        ],
    )
    .expect("a fixed fee loads");
    assert_eq!(config.qris.as_ref().unwrap().fee_idr_for(10_000), Some(500));
}

fn hmac_sha256(secret: &[u8], message: &[u8]) -> Vec<u8> {
    use hmac::digest::KeyInit;
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(secret).expect("a key");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

fn signed_headers(name: &str, value: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
        axum::http::HeaderValue::from_str(value).expect("a header value"),
    );
    headers
}
