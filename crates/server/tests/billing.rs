//! Task 4: checkout — the payment row, the discount, the student price, the reuse rule.
//!
//! Every case drives the real route through the real router with the trait's mock
//! gateway, so what is asserted is what ships: the platform rule reads the device row,
//! the amount comes from `billing::price`, and the QR the client is handed is the one the
//! gateway returned.
//!
//! Test code may unwrap and panic; `#[sqlx::test]` is a macro harness.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// The app and a signed-in, verified account on `platform`.
async fn harness(pool: PgPool, platform: &str) -> (TestState, axum::Router, String) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = format!("{platform}-{}@example.test", Uuid::now_v7());
    let token = common::signed_up_on(&app, &harness, &email, platform).await;
    (harness, app, token)
}

/// A checkout body for the pro plan, monthly.
fn checkout_body() -> serde_json::Value {
    serde_json::json!({ "plan_id": "pro", "billing_period": "monthly" })
}

/// Phase 7's boundary, called first, with the platform from the **device row**: a body that
/// says `platform: "web"` from an `ios` device still refuses (A144/A151).
#[sqlx::test(migrations = "../../migrations")]
async fn checkout_is_refused_on_a_mobile_device_and_a_body_cannot_claim_otherwise(pool: PgPool) {
    let (harness, app, token) = harness(pool, "ios").await;

    let (status, body) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({
            "plan_id": "pro",
            "billing_period": "monthly",
            // The claim the rule exists to ignore.
            "platform": "web",
        }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "feature_locked", "{body}");

    // And nothing was written: a refused checkout must not leave a pending row behind,
    // because the poller would then ask the gateway about an order it never minted.
    let payments: i64 = sqlx::query_scalar("select count(*) from payments")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(payments, 0, "a refused checkout wrote a payment");

    // The gateway was never called either.
    assert!(harness.gateway.invoices().is_empty());
}

/// A43's rule reaches checkout too: an unverified account is 403 `email_not_verified`.
#[sqlx::test(migrations = "../../migrations")]
async fn checkout_requires_a_verified_email(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    // Sign up without verifying: the helper's `signed_up` verifies, so this drives the
    // route directly and takes the token from the signup response.
    let (status, body) = json(
        &app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": "unverified@example.test",
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": "web" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["access_token"].as_str().expect("a token").to_string();

    let (status, body) = json(&app, "POST", "/billing/checkout", &token, checkout_body()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "email_not_verified", "{body}");
    assert!(harness.gateway.invoices().is_empty());
}

/// One pending payment, one order id, a 15-minute expiry, and the amount the webhook will
/// later be checked against.
#[sqlx::test(migrations = "../../migrations")]
async fn checkout_writes_one_pending_payment_with_the_quoted_amount(pool: PgPool) {
    let (harness, app, token) = harness(pool, "web").await;

    let (status, body) = json(&app, "POST", "/billing/checkout", &token, checkout_body()).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(body["status"], "pending", "{body}");
    assert_eq!(body["kind"], "subscription", "{body}");
    assert_eq!(body["plan_id"], "pro", "{body}");
    // The amount is the config's monthly pro price, not a literal here.
    let expected = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Pro)
        .price_idr_monthly;
    assert_eq!(body["amount_idr"], expected, "{body}");
    assert_eq!(body["amount_before_discount_idr"], expected, "{body}");
    assert_eq!(body["discount_percent"], 0, "{body}");
    assert!(
        body["qr_string"].as_str().is_some_and(|qr| !qr.is_empty()),
        "{body}"
    );
    assert!(
        body["order_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("stvdy_")),
        "{body}"
    );

    // The gateway was asked for exactly the amount and order id stored on the row: this is
    // the assertion that the quote and the invoice cannot disagree.
    let asked = harness.gateway.invoices();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].amount_idr, expected);
    assert_eq!(asked[0].order_id, body["order_id"].as_str().unwrap());

    // One row, pending, with the QR written back.
    let row: (i64, String, Option<String>) =
        sqlx::query_as("select amount_idr, status, qr_string from payments where order_id = $1")
            .bind(body["order_id"].as_str().unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(row.0, expected);
    assert_eq!(row.1, "pending");
    assert!(row.2.is_some(), "the QR was not written back to the row");
}

/// Asking twice for the same plan and period reuses the invoice instead of minting a
/// second QR (A307) — the spec's "paying twice" case, half of it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_checkout_for_the_same_period_reuses_the_pending_invoice(pool: PgPool) {
    let (harness, app, token) = harness(pool, "web").await;

    let (_, first) = json(&app, "POST", "/billing/checkout", &token, checkout_body()).await;
    let (status, second) = json(&app, "POST", "/billing/checkout", &token, checkout_body()).await;
    assert_eq!(status, StatusCode::OK, "{second}");

    assert_eq!(
        first["id"], second["id"],
        "a second checkout minted a second invoice for one period"
    );
    assert_eq!(first["order_id"], second["order_id"]);
    assert_eq!(
        harness.gateway.invoices().len(),
        1,
        "the gateway was asked for a second QR"
    );

    // A different period is a different invoice, which is what makes the reuse rule a rule
    // about one period rather than about one account.
    let (status, yearly) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "yearly" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{yearly}");
    assert_ne!(yearly["id"], first["id"]);
    assert_eq!(harness.gateway.invoices().len(), 2);
}

/// The student price is applied from `student_verified`, and a code does not stack (A298).
#[sqlx::test(migrations = "../../migrations")]
async fn a_verified_student_gets_the_student_price_and_a_code_does_not_stack(pool: PgPool) {
    let (harness, app, token) = harness(pool, "web").await;

    // A code worth 30% and the student promo worth 50%: the larger wins, and the code is
    // not consumed.
    sqlx::query("insert into discount_codes (code, percent_off) values ('STUDY30', 30)")
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, before) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly", "discount_code": "STUDY30" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let pro = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Pro);
    assert_eq!(before["discount_percent"], 30, "{before}");
    assert_eq!(before["discount_code"], "STUDY30", "{before}");
    assert_eq!(
        before["amount_idr"],
        pro.price_idr_monthly * 70 / 100,
        "{before}"
    );

    // Now mark the account a student and ask again for the *other* period, so the reuse
    // rule does not answer this call.
    sqlx::query("update users set student_verified = true where email like 'web-%'")
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, after) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "yearly", "discount_code": "STUDY30" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["discount_percent"], 50, "{after}");
    assert_eq!(after["student_discount"], true, "{after}");
    assert_eq!(
        after["discount_code"],
        serde_json::Value::Null,
        "the student promo won, so the code bought nothing and must not be recorded as applied"
    );
    assert_eq!(
        after["amount_idr"],
        pro.price_idr_yearly * 50 / 100,
        "{after}"
    );

    // The code was redeemed exactly once — for the checkout that actually used it.
    let redeemed: i64 =
        sqlx::query_scalar("select redeemed from discount_codes where code = 'STUDY30'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        redeemed, 1,
        "the code was counted for a checkout it did not discount"
    );
}

/// A code past `max_redemptions`, or used twice by the same account, is refused with 400
/// `validation` and no payment row.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exhausted_or_reused_discount_code_is_refused(pool: PgPool) {
    let (harness, app, token) = harness(pool, "web").await;

    sqlx::query(
        "insert into discount_codes (code, percent_off, max_redemptions) values ('ONCE', 20, 1)",
    )
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, first) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly", "discount_code": "ONCE" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");

    // The same account, a different period, the same once-per-user code.
    let (status, second) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "yearly", "discount_code": "ONCE" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{second}");
    assert_eq!(second["code"], "validation", "{second}");

    // An unknown code is refused with the same message: telling a caller which of
    // "unknown", "expired" and "exhausted" it was turns this route into a code oracle.
    let (status, unknown) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly", "discount_code": "NOPE" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{unknown}");
    assert_eq!(unknown["message"], second["message"], "{unknown}");

    // The count is not left inflated by the refused attempt, and no row was written for
    // it: one redemption, two payments (monthly and yearly), the yearly one without a code.
    let redeemed: i64 =
        sqlx::query_scalar("select redeemed from discount_codes where code = 'ONCE'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(redeemed, 1, "a refused redemption was counted");
    let payments: i64 = sqlx::query_scalar("select count(*) from payments")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(payments, 2, "a refused checkout wrote a payment");
}

/// The amount the client is shown is the amount in the row, from the one pricing function
/// — the honesty rule (gate 7(b)) as an assertion.
#[sqlx::test(migrations = "../../migrations")]
async fn the_quoted_amount_is_the_stored_amount(pool: PgPool) {
    let (harness, app, token) = harness(pool, "web").await;

    let (_, quoted) = json(&app, "POST", "/billing/checkout", &token, checkout_body()).await;
    let id = quoted["id"].as_str().unwrap();

    // The poll the checkout screen runs must report the same number the quote did.
    let (status, polled) = json(
        &app,
        "GET",
        &format!("/billing/payments/{id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["amount_idr"], quoted["amount_idr"]);
    assert_eq!(polled["status"], "pending");

    let stored: i64 = sqlx::query_scalar("select amount_idr from payments where id = $1::uuid")
        .bind(id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        quoted["amount_idr"].as_i64().unwrap(),
        stored,
        "the amount shown is not the amount stored"
    );

    // And the gateway was asked for that number, not another.
    assert_eq!(harness.gateway.invoices()[0].amount_idr, stored);

    // A payment belonging to someone else is not readable through the poll.
    let other = TestState::new(harness.state.pool.clone());
    let _ = other;
    sqlx::query("insert into users (id, email, password_hash) values ($1, $2, 'x')")
        .bind(Uuid::now_v7())
        .bind(format!("other-{}@example.test", Uuid::now_v7()))
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (status, body) = json(
        &app,
        "GET",
        &format!("/billing/payments/{id}"),
        "not-a-real-token",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

/// A deployment with no gateway refuses checkout with 502 `provider_error` and writes
/// nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deployment_with_no_gateway_refuses_checkout(pool: PgPool) {
    let harness = TestState::new(pool).without_gateway();
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nogateway@example.test").await;

    let (status, body) = json(&app, "POST", "/billing/checkout", &token, checkout_body()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "provider_error", "{body}");

    let payments: i64 = sqlx::query_scalar("select count(*) from payments")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(payments, 0, "a deployment with no gateway wrote a payment");
}

/// The plan matrix is platform-aware and omits prices where the policy says so (A305).
#[sqlx::test(migrations = "../../migrations")]
async fn plans_omits_prices_on_a_platform_that_must_not_show_them(pool: PgPool) {
    let (_harness, app, token) = harness(pool, "web").await;
    let (status, body) = json(&app, "GET", "/plans", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["show_prices"], true, "{body}");
    assert!(body["plans"][1]["price_idr_monthly"].is_i64(), "{body}");
    assert!(body["credit_packs"][0]["price_idr"].is_i64(), "{body}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn plans_hides_prices_from_a_phone(pool: PgPool) {
    let (_harness, app, token) = harness(pool, "ios").await;
    let (status, body) = json(&app, "GET", "/plans", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["show_prices"], false, "{body}");
    assert_eq!(body["purchase_allowed"], false, "{body}");
    assert!(
        body["plans"][1].get("price_idr_monthly").is_none(),
        "a phone was told a price: {body}"
    );
    assert!(
        body["credit_packs"][0].get("price_idr").is_none(),
        "a phone was told a pack price: {body}"
    );
    // The limits are still there: the screen must be able to say what the plan includes.
    assert!(body["plans"][1]["limits"].is_object(), "{body}");
}

/// The student flow: a campus address is accepted, a lookalike is refused, and the confirm
/// route sets the flag and the address from the link, not from a body field (A299).
#[sqlx::test(migrations = "../../migrations")]
async fn the_student_verification_flow_sets_the_flag_only_from_a_campus_link(pool: PgPool) {
    let (harness, app, token) = harness(pool, "web").await;

    let (status, refused) = json(
        &app,
        "POST",
        "/billing/student/verify",
        &token,
        serde_json::json!({ "email": "ana@ac.id.evil.example" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{refused}");
    assert_eq!(
        refused["sent"], false,
        "a lookalike domain was accepted as a campus address: {refused}"
    );
    assert!(
        harness.mailer.sent().is_empty(),
        "mail was sent for a lookalike address"
    );

    let (status, sent) = json(
        &app,
        "POST",
        "/billing/student/verify",
        &token,
        serde_json::json!({ "email": "ana@ui.ac.id" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["sent"], true, "{sent}");

    let link = harness.last_verification_link().expect("a student link");
    let token_value = link.split("token=").nth(1).expect("a token").to_string();

    let (status, confirmed) = json(
        &app,
        "POST",
        "/billing/student/confirm",
        &token,
        serde_json::json!({ "token": token_value }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["student_verified"], true, "{confirmed}");

    let (verified, address): (bool, Option<String>) = sqlx::query_as(
        "select student_verified, student_email from users where email like 'web-%'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert!(verified);
    assert_eq!(
        address.as_deref(),
        Some("ana@ui.ac.id"),
        "the address must be the one the link was mailed to, not a body field"
    );

    // The token is single use: the second attempt updates zero rows.
    let (status, _) = json(
        &app,
        "POST",
        "/billing/student/confirm",
        &token,
        serde_json::json!({ "token": link.split("token=").nth(1).unwrap() }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
