//! Task 6: `poll_payment` and `reconcile_payments` (A301).
//!
//! The suite's central assertion is that **the client's poll and the sweep share one apply
//! function**: a sweep that runs after the checkout screen already saw the payment is a
//! no-op rather than a second activation. That is the defect this task exists to prevent,
//! and the only way to test it is to run both against one payment.
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

/// The **subscription period's** grant, not every positive ledger row: signup writes the
/// free allowance, so counting all of them would make every activation look like a double
/// grant.
async fn ledger_rows(harness: &TestState) -> i64 {
    sqlx::query_scalar(
        "select count(*) from credit_ledger where delta > 0 and period_key like 'sub:%'",
    )
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

/// The backstop: a student who paid and closed the tab is recovered by the sweep.
#[sqlx::test(migrations = "../../migrations")]
async fn the_sweep_recovers_a_payment_the_client_stopped_watching(pool: PgPool) {
    let (harness, _app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    // The gateway says paid; nobody is polling the screen any more.
    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: amount,
        provider_payment_id: Some("gw-poll".to_string()),
        fee_idr: Some(70),
    });

    let applied = stvdy_server::billing::poll::poll_pending(&harness.state)
        .await
        .unwrap();
    assert_eq!(applied, 1, "the sweep did not recover a paid payment");

    assert_eq!(
        payment_status(&harness, payment["id"].as_str().unwrap()).await,
        "paid"
    );
    assert_eq!(live_subscriptions(&harness).await, 1);
    assert_eq!(ledger_rows(&harness).await, 1);

    // The fee the gateway reported is on the row, and the event is recorded so the audit
    // says where the activation came from. `gateway_fee_idr` is INT4, so the read is
    // `Option<i32>` and the cast keeps sqlx from decoding it as anything else.
    let fee: Option<i32> =
        sqlx::query_scalar("select gateway_fee_idr::int from payments where id = $1::uuid")
            .bind(payment["id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(fee, Some(70));
    let source: Option<String> = sqlx::query_scalar(
        "select payload->>'source' from gateway_events where event_id like 'gateway:%'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(source.as_deref(), Some("poll"));
}

/// **The fast ingress**: the checkout screen's own poll asks the gateway, so a student who
/// has paid does not wait for the five-minute sweep. The bridge never pushes, so without
/// this the screen sits on "waiting for confirmation" in front of money that moved.
#[sqlx::test(migrations = "../../migrations")]
async fn the_checkout_poll_asks_the_gateway_and_activates(pool: PgPool) {
    let (harness, app, token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: amount,
        provider_payment_id: Some("gw-poll".to_string()),
        fee_idr: None,
    });

    let (status, polled) = json(
        &app,
        "GET",
        &format!("/billing/payments/{}", payment["id"].as_str().unwrap()),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(
        polled["status"], "paid",
        "the poll answered from the row without asking the gateway: {polled}"
    );
    assert_eq!(live_subscriptions(&harness).await, 1);

    // And the sweep that follows is a no-op — one apply function, one activation.
    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: amount,
        provider_payment_id: Some("gw-poll".to_string()),
        fee_idr: None,
    });
    let applied = stvdy_server::billing::poll::poll_pending(&harness.state)
        .await
        .unwrap();
    assert_eq!(applied, 0, "the sweep re-activated an already-paid payment");
    assert_eq!(live_subscriptions(&harness).await, 1);
    assert_eq!(ledger_rows(&harness).await, 1);
}

/// **The amount the settlement is checked against is the payable one.** The gateway adds a
/// unique offset so it can match a settlement by amount alone; the row keeps the price and
/// the payable apart, and the check reads the payable. Comparing against the price would put
/// every real payment in `review`.
#[sqlx::test(migrations = "../../migrations")]
async fn an_offset_invoice_activates_against_its_payable_amount(pool: PgPool) {
    let (harness, app, token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    // The double now behaves like the bridge: every invoice is priced at base + 137.
    harness.gateway.set_unique_offset(137);
    let (status, offset_payment) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "power", "billing_period": "monthly" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{offset_payment}");

    let price = offset_payment["amount_idr"].as_i64().unwrap();
    assert_eq!(
        offset_payment["payable_amount_idr"].as_i64().unwrap(),
        price + 137,
        "the client is not told the figure it must transfer: {offset_payment}"
    );

    // The gateway reports the amount that actually landed.
    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: price + 137,
        provider_payment_id: Some("gw-offset".to_string()),
        fee_idr: None,
    });
    let (_, polled) = json(
        &app,
        "GET",
        &format!(
            "/billing/payments/{}",
            offset_payment["id"].as_str().unwrap()
        ),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        polled["status"], "paid",
        "a settlement for the payable amount was refused: {polled}"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from subscriptions where status = 'active'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap(),
        1
    );

    // The other direction, and it is the one the column exists for: a settlement for the
    // *price* is not the amount this invoice asked for, so it is a human's problem rather
    // than an activation. Both numbers are on the row for them to read.
    let (_, second) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "power", "billing_period": "yearly" }),
    )
    .await;
    let second_price = second["amount_idr"].as_i64().unwrap();
    harness.gateway.push_status(GatewayStatus::Paid {
        paid_at: chrono::Utc::now(),
        amount_idr: second_price,
        provider_payment_id: Some("gw-short".to_string()),
        fee_idr: None,
    });
    let (_, polled) = json(
        &app,
        "GET",
        &format!("/billing/payments/{}", second["id"].as_str().unwrap()),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(polled["status"], "review", "{polled}");

    let reason: Option<String> =
        sqlx::query_scalar("select review_reason from payments where id = $1::uuid")
            .bind(second["id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let reason = reason.expect("a review names its reason");
    assert!(
        reason.contains(&(second_price + 137).to_string())
            && reason.contains(&second_price.to_string()),
        "the reason does not name both amounts: {reason}"
    );
    let _ = amount;
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
    // One account, one token: signing the same email up twice is a 409, which is what the
    // first version of this test did.
    let second_token = signed_up_on(&_app, &harness, "second@example.test", "web").await;
    let (_, second) = json(
        &_app,
        "POST",
        "/billing/checkout",
        &second_token,
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
    let (status, polled) = json(
        &_app,
        "GET",
        &format!("/billing/payments/{}", second["id"].as_str().unwrap()),
        &second_token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "expired", "{polled}");
}

/// A replay of one answer activates once, and leaves one audit row.
///
/// Two defences are in play and this asserts the outcome rather than which one fired: the
/// `gateway_events` unique key catches the *same* answer arriving twice (the client's poll
/// and the sweep can both be in flight), and the guarded `status = 'pending'` transition
/// catches the same payment reported under a **different** answer. A unique key cannot see
/// the second and a guarded update cannot see the first, which is why both exist (A293).
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_answer_activates_once(pool: PgPool) {
    let (harness, _app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();
    // One instant, so both answers are the same event: the event id is derived from the
    // settlement time, exactly as it is for a real gateway that reports one settlement.
    let paid_at = chrono::Utc::now();

    for _ in 0..2 {
        harness.gateway.push_status(GatewayStatus::Paid {
            paid_at,
            amount_idr: amount,
            provider_payment_id: Some("gw-replay".to_string()),
            fee_idr: None,
        });
        stvdy_server::billing::poll::poll_pending(&harness.state)
            .await
            .unwrap();
    }

    assert_eq!(
        payment_status(&harness, payment["id"].as_str().unwrap()).await,
        "paid"
    );
    assert_eq!(live_subscriptions(&harness).await, 1);
    assert_eq!(ledger_rows(&harness).await, 1, "a period was granted twice");

    let events: i64 = sqlx::query_scalar("select count(*) from gateway_events")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(events, 1, "a replay wrote a second audit row");
}

/// A gateway transaction we have no row for is a `review` flag, not silence.
#[sqlx::test(migrations = "../../migrations")]
async fn reconcile_flags_a_gateway_payment_we_never_recorded(pool: PgPool) {
    let (harness, _app, _token, _payment) = checkout(pool).await;

    // The gateway took money for an amount no payment of ours is priced at. There is no
    // order id to compare: the bridge's feed carries the *provider's* id, which is not
    // ours, so the amount that landed is the only join key there is.
    harness.gateway.push_transactions(vec![GatewayTransaction {
        transaction_id: "tx-someone-elses".to_string(),
        amount_idr: 9_999,
        paid_at: chrono::Utc::now(),
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
