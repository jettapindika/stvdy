//! Task 5: the webhook — verified, idempotent, amount-checked, one transaction.
//!
//! Every case posts a body the mock gateway **really signs** and drives the real route, so
//! a test that passes is a test whose verification ran. The two idempotency cases are the
//! point of the suite and they are separate tests because they defend different replays
//! (A293): the same event id twice, and the same payment under a **new** event id.
//!
//! Test code may unwrap and panic; `#[sqlx::test]` is a macro harness.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up_on};

/// A signed-in account, and a pending payment for `pro` monthly, created through the real
/// checkout route so the row is exactly what production would write.
async fn checkout(pool: PgPool) -> (TestState, axum::Router, String, serde_json::Value) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up_on(&app, &harness, "payer@example.test", "web").await;

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

/// A paid callback for `payment`, signed by the mock gateway.
fn paid_event(payment: &serde_json::Value, event_id: &str, amount_idr: i64) -> (String, Vec<u8>) {
    let body = serde_json::json!({
        "event_id": event_id,
        "kind": "paid",
        "order_id": payment["order_id"],
        "amount_idr": amount_idr,
        "currency": "IDR",
        "paid_at": chrono::Utc::now().to_rfc3339(),
        "provider_payment_id": format!("gw-{event_id}"),
    })
    .to_string();
    (body.clone(), body.into_bytes())
}

/// Posts a signed body to the webhook.
async fn post_webhook(
    app: &axum::Router,
    harness: &TestState,
    body: &[u8],
) -> (StatusCode, serde_json::Value) {
    let headers = harness.gateway.sign(body);
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/webhooks/qris")
        .header("content-type", "application/json");
    for (name, value) in headers.iter() {
        request = request.header(name, value);
    }
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        request.body(axum::body::Body::from(body.to_vec())).unwrap(),
    )
    .await
    .unwrap();

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

async fn ledger_rows(harness: &TestState) -> i64 {
    sqlx::query_scalar("select count(*) from credit_ledger where delta > 0")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn live_subscriptions(harness: &TestState) -> i64 {
    sqlx::query_scalar("select count(*) from subscriptions where status = 'active'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The exit gate's happy path, and the second device: after the webhook, a token minted on
/// another device sees the paid plan, because the plan is not a JWT claim (A22) and
/// entitlements load per request.
#[sqlx::test(migrations = "../../migrations")]
async fn a_signed_paid_webhook_activates_the_plan_and_the_second_device_sees_it(pool: PgPool) {
    let (harness, app, token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    let (_, body) = paid_event(&payment, "evt_1", amount);
    let (status, outcome) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(outcome["duplicate"], false, "{outcome}");
    assert_eq!(outcome["status"], "activated", "{outcome}");

    // The row is paid, with the fee recorded from the event and the subscription linked.
    let (status_str, paid_at, subscription_id): (
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<Uuid>,
    ) = sqlx::query_as("select status, paid_at, subscription_id from payments where id = $1::uuid")
        .bind(payment["id"].as_str().unwrap())
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(status_str, "paid");
    assert!(paid_at.is_some(), "a paid payment has no paid_at");
    assert!(
        subscription_id.is_some(),
        "the payment was not linked to a subscription"
    );
    assert_eq!(live_subscriptions(&harness).await, 1);

    // The period's allowance was granted exactly once.
    assert_eq!(ledger_rows(&harness).await, 1);

    // The second device: a token minted after the webhook sees the paid plan, because
    // entitlements are loaded per request rather than carried in the token.
    let other_token = signed_up_on(&app, &harness, "payer@example.test", "macos").await;
    let (status, me) = json(&app, "GET", "/usage", &other_token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        me["plan"], "pro",
        "the second device does not see the paid plan: {me}"
    );

    // The client's own poll sees `paid`, which is what stops the countdown.
    let (status, polled) = json(
        &app,
        "GET",
        &format!("/billing/payments/{}", payment["id"].as_str().unwrap()),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "paid", "{polled}");
}

/// The same event id twice: one activation, one credit grant, and the second answer says
/// `duplicate` (A293, the `webhook_events` half).
#[sqlx::test(migrations = "../../migrations")]
async fn the_same_event_id_twice_activates_once(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();
    let (_, body) = paid_event(&payment, "evt_same", amount);

    let (status, first) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["duplicate"], false, "{first}");

    let (status, second) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["duplicate"], true,
        "the second delivery of one event was not recognised: {second}"
    );

    // One activation, one grant, and the period did not move.
    assert_eq!(live_subscriptions(&harness).await, 1);
    assert_eq!(ledger_rows(&harness).await, 1);
    let events: i64 =
        sqlx::query_scalar("select count(*) from webhook_events where event_id = 'evt_same'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(events, 1, "the same event id left two audit rows");
}

/// The same payment under a **new** event id: still one activation (A293, the guarded
/// transition half). This is the case a real gateway produces on retry, and the one a
/// unique-key-only defence misses.
#[sqlx::test(migrations = "../../migrations")]
async fn the_same_payment_under_a_new_event_id_still_activates_once(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    let (_, first_body) = paid_event(&payment, "evt_a", amount);
    let (status, first) = post_webhook(&app, &harness, &first_body).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["status"], "activated", "{first}");

    // A fresh event id, the same order: the unique key cannot see this one, so the guarded
    // transition is the defence under test.
    let (_, second_body) = paid_event(&payment, "evt_b", amount);
    let (status, second) = post_webhook(&app, &harness, &second_body).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["status"], "already_paid",
        "a second report of one payment was applied again: {second}"
    );

    assert_eq!(live_subscriptions(&harness).await, 1);
    assert_eq!(
        ledger_rows(&harness).await,
        1,
        "a second report of one payment granted a second period"
    );
    // Both events are recorded — the audit is honest about what arrived — but only one
    // activation happened.
    let events: i64 = sqlx::query_scalar("select count(*) from webhook_events")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(events, 2);
}

/// Over- and under-payment both land in `review` with the two numbers recorded, and the
/// subscription is untouched (A294).
#[sqlx::test(migrations = "../../migrations")]
async fn a_mismatched_amount_lands_in_review_and_activates_nothing(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    for (event_id, paid) in [("evt_over", amount + 1), ("evt_under", amount - 1)] {
        let (_, body) = paid_event(&payment, event_id, paid);
        let (status, outcome) = post_webhook(&app, &harness, &body).await;
        assert_eq!(status, StatusCode::OK, "{outcome}");
        assert_eq!(outcome["status"], "review", "{outcome}");
    }

    // The row is `review` with both numbers in the reason, and no subscription exists.
    let (status_str, reason): (String, Option<String>) =
        sqlx::query_as("select status, review_reason from payments where id = $1::uuid")
            .bind(payment["id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(status_str, "review");
    let reason = reason.expect("a review names its reason");
    assert!(
        reason.contains(&amount.to_string()) && reason.contains(&(amount + 1).to_string()),
        "the reason must name both numbers, got {reason:?}"
    );
    assert_eq!(
        live_subscriptions(&harness).await,
        0,
        "a mismatch activated a plan"
    );
    assert_eq!(ledger_rows(&harness).await, 0, "a mismatch granted credits");

    // The raw payload is kept, which is what a human reads when the amount does not match.
    let payloads: i64 =
        sqlx::query_scalar("select count(*) from webhook_events where payload is not null")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(payloads, 2);
}

/// Paid before expiry, webhook after it: honoured, because the lateness is ours.
#[sqlx::test(migrations = "../../migrations")]
async fn a_payment_made_before_expiry_is_honoured_after_it(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    // The invoice is already expired; the gateway says the money moved before it died.
    let expired = chrono::Utc::now() - chrono::Duration::minutes(1);
    sqlx::query("update payments set expires_at = $2 where id = $1::uuid")
        .bind(payment["id"].as_str().unwrap())
        .bind(expired)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let body = serde_json::json!({
        "event_id": "evt_late_but_in_time",
        "kind": "paid",
        "order_id": payment["order_id"],
        "amount_idr": amount,
        "currency": "IDR",
        "paid_at": (expired - chrono::Duration::seconds(30)).to_rfc3339(),
    })
    .to_string()
    .into_bytes();

    let (status, outcome) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(
        outcome["status"], "activated",
        "a payment made in time was refused because our confirmation was late: {outcome}"
    );
    assert_eq!(live_subscriptions(&harness).await, 1);
}

/// Paid after expiry: `review`, never automatic (A295).
#[sqlx::test(migrations = "../../migrations")]
async fn a_payment_made_after_expiry_goes_to_review(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();

    let expired = chrono::Utc::now() - chrono::Duration::minutes(5);
    sqlx::query("update payments set expires_at = $2 where id = $1::uuid")
        .bind(payment["id"].as_str().unwrap())
        .bind(expired)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let body = serde_json::json!({
        "event_id": "evt_too_late",
        "kind": "paid",
        "order_id": payment["order_id"],
        "amount_idr": amount,
        "currency": "IDR",
        // Paid a minute *after* the code expired.
        "paid_at": (expired + chrono::Duration::minutes(1)).to_rfc3339(),
    })
    .to_string()
    .into_bytes();

    let (status, outcome) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(outcome["status"], "review", "{outcome}");
    assert_eq!(live_subscriptions(&harness).await, 0);
}

/// An order id we do not have is 404 `not_found` and writes no webhook event, so a forged
/// order id cannot even leave a row behind.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_order_id_is_not_found_and_writes_nothing(pool: PgPool) {
    let (harness, app, _token, _payment) = checkout(pool).await;

    let body = serde_json::json!({
        "event_id": "evt_forged_order",
        "kind": "paid",
        "order_id": "stvdy_forged0000000000000000000000000",
        "amount_idr": 10_000,
        "currency": "IDR",
        "paid_at": chrono::Utc::now().to_rfc3339(),
    })
    .to_string()
    .into_bytes();

    let (status, outcome) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{outcome}");
    assert_eq!(outcome["code"], "not_found", "{outcome}");

    let events: i64 = sqlx::query_scalar("select count(*) from webhook_events")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(events, 0, "a forged order id left an audit row behind");
}

/// A bad signature is 401 `unauthorized`, writes no `webhook_events` row and touches no
/// payment — the §5 "forge a webhook" row, as a case (A309).
#[sqlx::test(migrations = "../../migrations")]
async fn a_forged_signature_is_unauthorized_and_changes_nothing(pool: PgPool) {
    let (harness, app, _token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();
    let (_, body) = paid_event(&payment, "evt_forged", amount);

    // Sign with a *different* secret: a real gateway's signature, but not ours.
    let other = stvdy_core::traits::testing::MockPayment::with_secret(b"another-gateway");
    let headers = other.sign(&body);

    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/webhooks/qris")
        .header("content-type", "application/json");
    for (name, value) in headers.iter() {
        request = request.header(name, value);
    }
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        request.body(axum::body::Body::from(body.clone())).unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // One byte appended, the same headers: the body no longer matches the tag. This is the
    // assertion that makes the raw-bytes handoff real.
    let mut tampered = body.clone();
    tampered.push(b' ');
    let (status, _) = post_webhook(&app, &harness, &tampered).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let events: i64 = sqlx::query_scalar("select count(*) from webhook_events")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(events, 0, "a forged signature wrote an audit row");
    let status_str: String = sqlx::query_scalar("select status from payments where id = $1::uuid")
        .bind(payment["id"].as_str().unwrap())
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        status_str, "pending",
        "a forged signature touched the payment"
    );
}

/// The webhook arriving before the client polls: the poll reads `paid`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_webhook_that_arrives_first_is_visible_to_the_poll(pool: PgPool) {
    let (harness, app, token, payment) = checkout(pool).await;
    let amount = payment["amount_idr"].as_i64().unwrap();
    let (_, body) = paid_event(&payment, "evt_first", amount);

    let (status, outcome) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");

    let (status, polled) = json(
        &app,
        "GET",
        &format!("/billing/payments/{}", payment["id"].as_str().unwrap()),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "paid", "{polled}");
    assert!(polled["paid_at"].is_string(), "{polled}");
}

/// A credit pack activates credits in the `purchased` bucket, once, naming its payment
/// (A297).
#[sqlx::test(migrations = "../../migrations")]
async fn a_credit_pack_grants_once_and_the_ledger_names_its_payment(pool: PgPool) {
    let (harness, app, _token, _payment) = checkout(pool).await;

    // The first pack in the config, named by its credit amount.
    let pack = harness.state.config.plans.credit_packs.packs[0].clone();
    let (status, payment) = json(
        &app,
        "POST",
        "/billing/checkout",
        &signed_up_on(&app, &harness, "buyer@example.test", "web").await,
        serde_json::json!({ "credit_pack": pack.credits }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payment}");
    assert_eq!(payment["kind"], "credit_pack", "{payment}");
    assert_eq!(payment["amount_idr"], pack.price_idr, "{payment}");
    assert_eq!(payment["credits"], pack.credits, "{payment}");

    let (_, body) = paid_event(&payment, "evt_pack", pack.price_idr);
    let (status, outcome) = post_webhook(&app, &harness, &body).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(outcome["status"], "credits", "{outcome}");

    let (bucket, reason, ref_id, delta): (String, String, Option<Uuid>, i32) = sqlx::query_as(
        "select bucket, reason, ref_id, delta from credit_ledger
          where reason = 'credit_pack'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(bucket, "purchased");
    assert_eq!(delta, pack.credits as i32);
    assert_eq!(
        ref_id.map(|id| id.to_string()).as_deref(),
        payment["id"].as_str(),
        "the ledger row must name the payment that bought it"
    );

    // A second report of the same payment is a no-op, so no second pack.
    let (_, again) = paid_event(&payment, "evt_pack_2", pack.price_idr);
    let (status, outcome) = post_webhook(&app, &harness, &again).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    let packs: i64 =
        sqlx::query_scalar("select count(*) from credit_ledger where reason = 'credit_pack'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(packs, 1, "a second report granted a second pack");
}
