//! Task 6: `poll_payment` and `reconcile_payments` (A301).
//!
//! The suite's central assertion is that **the poller and the webhook share one apply
//! function**: a poll that lands after a webhook is a no-op rather than a second
//! activation. That is the defect this task exists to prevent, and the only way to test it
//! is to run both against one payment.
//!
//! Test code may unwrap and panic; `#[sqlx::test]` is a macro harness.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::payment::GatewayStatus;
use stvdy_server::integrations::payment::GatewayTransaction;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up_on};

/// A signed-in account with one pending `pro` monthly payment.
async fn checkout(pool: PgPool) -> (TestState, axum::Router, String, serde_json::Value) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up_on(&app, &harness, "poll@example.test", "web").await;

    let (status, payment) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payment}");
    (harness, app, token, payment)
}

async fn live_subscriptions(harness: &TestState) -> i64 {
    sqlx::query_scalar("select count(*) from subscriptions where status = 'active'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn ledger_rows(harness: &TestState) -> i64 {
    sqlx::query_scalar("select count(*) from credit_ledger where delta > 0")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn payment_status(harness: &TestState, id: &str) -> String {
    sqlx::query_scalar("select status from payments where id = $1::uuid")
        .bind(id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The spec's fallback: a webhook that was lost is recovered by asking the gateway.
#[sqlx::test(migrations = "../../migrations")]
async fn poll_payment_activates_a_payment_whose_webhook_was_lost(pool: PgPool) {
    let (harness, _app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    // The gateway says paid; no webhook ever arrived.
    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: amount,
        provider_payment_id: Some("gw-poll".to_string()),
        fee_idr: Some(70),
    });

    let applied = stvdy_server::billing::poll::poll_pending(&harness.state)
        .await
        .unwrap();
    assert_eq!(applied, 1, "the poller did not recover a paid payment");

    assert_eq!(
        payment_status(&harness, payment["id"].as_str().unwrap()).await,
        "paid"
    );
    assert_eq!(live_subscriptions(&harness).await, 1);
    assert_eq!(ledger_rows(&harness).await, 1);

    // The fee the gateway reported is on the row, and the poll's event is recorded so the
    // audit says where the activation came from.
    let fee: Option<i32> =
        sqlx::query_scalar("select gateway_fee_idr from payments where id = $1::uuid")
            .bind(payment["id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(fee, Some(70));
    let source: Option<String> = sqlx::query_scalar(
        "select payload->>'source' from webhook_events where event_id like 'poll:%'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(source.as_deref(), Some("poll"));
}

/// Still pending at the gateway → still pending here, and `expired` once the invoice's own
/// deadline passes. The row is never left `pending` forever.
#[sqlx::test(migrations = "../../migrations")]
async fn poll_payment_expires_an_invoice_the_gateway_says_is_dead(pool: PgPool) {
    let (harness, _app, _token, payment) = checkout(pool).await;

    // The gateway says expired.
    harness.gateway.push_status(GatewayStatus::Expired);
    let applied = stvdy_server::billing::poll::poll_pending(&harness.state)
        .await
        .unwrap();
    assert_eq!(applied, 0);
    assert_eq!(
        payment_status(&harness, payment["id"].as_str().unwrap()).await,
        "expired"
    );
    assert_eq!(live_subscriptions(&harness).await, 0);

    // A payment that is still pending at the gateway stays pending — it is not expired by
    // our clock alone, because the gateway's expiry is the authoritative one.
    let (_, second) = json(
        &_app,
        "POST",
        "/billing/checkout",
        &signed_up_on(&_app, &harness, "second@example.test", "web").await,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly" }),
    )
    .await;
    harness.gateway.push_status(GatewayStatus::Pending);
    stvdy_server::billing::poll::poll_pending(&harness.state)
        .await
        .unwrap();
    assert_eq!(
        payment_status(&harness, second["id"].as_str().unwrap()).await,
        "pending"
    );

    // And the client's own poll expires it once the deadline passes, so the screen stops
    // waiting even when the gateway never says anything.
    sqlx::query("update payments set expires_at = now() - interval '1 minute' where id = $1::uuid")
        .bind(second["id"].as_str().unwrap())
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let token = signed_up_on(&_app, &harness, "second@example.test", "web").await;
    let (status, polled) = json(
        &_app,
        "GET",
        &format!("/billing/payments/{}", second["id"].as_str().unwrap()),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "expired", "{polled}");
}

/// The poller and the webhook run the SAME apply, so a poll that lands after the webhook
/// is a no-op rather than a second activation.
#[sqlx::test(migrations = "../../migrations")]
async fn a_poll_after_a_webhook_changes_nothing(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    // The webhook gets there first.
    let body = serde_json::json!({
        "event_id": "evt_won_the_race",
        "kind": "paid",
        "order_id": payment["order_id"],
        "amount_idr": amount,
        "currency": "IDR",
        "paid_at": chrono::Utc::now().to_rfc3339(),
    })
    .to_string()
    .into_bytes();
    let headers = harness.gateway.sign(&body);
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/webhooks/qris")
        .header("content-type", "application/json");
    for (name, value) in headers.iter() {
        request = request.header(name, value);
    }
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        request.body(axum::body::Body::from(body)).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let before_subscriptions = live_subscriptions(&harness).await;
    let before_ledger = ledger_rows(&harness).await;

    // Now the poller asks the same question and gets the same answer. It must change
    // nothing: same apply function, same guarded transition.
    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: amount,
        provider_payment_id: Some("gw-poll".to_string()),
        fee_idr: None,
    });
    let applied = stvdy_server::billing::poll::poll_pending(&harness.state)
        .await
        .unwrap();

    assert_eq!(applied, 0, "the poll re-activated an already-paid payment");
    assert_eq!(live_subscriptions(&harness).await, before_subscriptions);
    assert_eq!(
        ledger_rows(&harness).await,
        before_ledger,
        "the poll granted a second period"
    );
}

/// A gateway transaction we have no row for is a `review` flag, not silence.
#[sqlx::test(migrations = "../../migrations")]
async fn reconcile_flags_a_gateway_payment_we_never_recorded(pool: PgPool) {
    let (harness, _app, _token, _payment) = checkout(pool).await;

    // The gateway knows two orders: ours, and one we have no row for.
    harness.gateway.push_transactions(vec![GatewayTransaction {
        order_id: "stvdy_someone_elses_order".to_string(),
        status: GatewayStatus::Paid {
            paid_at: chrono::Utc::now(),
            amount_idr: 9_999,
            provider_payment_id: None,
            fee_idr: None,
        },
        amount_idr: 9_999,
    }]);

    let flagged = stvdy_server::billing::poll::reconcile(&harness.state)
        .await
        .unwrap();
    // The unknown transaction is counted; our own pending payment is inside the ten-minute
    // grace and is not flagged, because an invoice created seconds ago is not a mismatch.
    assert_eq!(flagged, 1, "an unknown gateway transaction was not flagged");
}

/// One of ours the gateway does not know is the other flag.
#[sqlx::test(migrations = "../../migrations")]
async fn reconcile_flags_a_payment_the_gateway_does_not_know(pool: PgPool) {
    let (harness, _app, _token, payment) = checkout(pool).await;

    // Age the payment past the ten-minute grace, so it is a candidate.
    sqlx::query("update payments set created_at = now() - interval '1 hour' where id = $1::uuid")
        .bind(payment["id"].as_str().unwrap())
        .execute(&harness.state.pool)
        .await
        .unwrap();

    // The gateway lists nothing at all.
    harness.gateway.push_transactions(vec![]);

    let flagged = stvdy_server::billing::poll::reconcile(&harness.state)
        .await
        .unwrap();
    assert_eq!(
        flagged, 1,
        "a payment the gateway does not know was not flagged"
    );

    let (status, reason): (String, Option<String>) =
        sqlx::query_as("select status, review_reason from payments where id = $1::uuid")
            .bind(payment["id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "review");
    assert!(reason.is_some(), "a review names its reason");

    // A second sweep must not re-flag it into a different reason: the first reason is what
    // a human is already acting on.
    let first_reason = reason.clone();
    stvdy_server::billing::poll::reconcile(&harness.state)
        .await
        .unwrap();
    let (_, again): (String, Option<String>) =
        sqlx::query_as("select status, review_reason from payments where id = $1::uuid")
            .bind(payment["id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        again, first_reason,
        "a second sweep rewrote a review reason"
    );
}

/// The two kinds are singletons: a restart, a second instance or a double tick enqueues one
/// job, not two (A10's `dedupe_key` rule).
#[sqlx::test(migrations = "../../migrations")]
async fn the_payment_tick_is_idempotent(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    let first = stvdy_server::jobs::schedule::payment_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(first, 2, "the pass enqueues the poller and the sweep");
    let second = stvdy_server::jobs::schedule::payment_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(second, 2);

    for kind in ["poll_payment", "reconcile_payments"] {
        let queued: i64 = sqlx::query_scalar(
            "select count(*) from jobs where kind = $1 and status in ('queued', 'running')",
        )
        .bind(kind)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        assert_eq!(queued, 1, "{kind} was enqueued twice");
    }
}

/// A deployment with no gateway enqueues nothing: a job that would answer
/// `provider_error` every five minutes is noise in the failure log, not a signal.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deployment_with_no_gateway_enqueues_no_payment_jobs(pool: PgPool) {
    let harness = TestState::new(pool).without_gateway();
    harness.seed_plans().await.unwrap();

    let enqueued = stvdy_server::jobs::schedule::payment_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(enqueued, 0);

    let jobs: i64 = sqlx::query_scalar("select count(*) from jobs")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

/// The two dispatch arms exist: the worker's `match` on `kind` would otherwise fail these
/// jobs permanently as an unknown kind.
#[sqlx::test(migrations = "../../migrations")]
async fn the_worker_dispatches_both_payment_kinds(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    for kind in ["poll_payment", "reconcile_payments"] {
        stvdy_server::jobs::enqueue(
            &harness.state.pool,
            &harness.state.jobs,
            kind,
            serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    }

    common::drain_jobs(&harness).await;

    let failed: i64 = sqlx::query_scalar("select count(*) from jobs where status = 'failed'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(failed, 0, "a payment job failed");
    let _ = Uuid::now_v7();
}
