//! The margin report, and the admin payment tools.
//!
//! **The attribution case is the point of this suite.** The roadmap's sketch joins
//! `usage_events` to `users.plan_id`, and that join is wrong in exactly the month the report
//! exists to examine: the month a student converts, their earlier spend that month is
//! attributed to the paid tier, so the free tier's cost is understated by exactly the cost of
//! converting. `users.plan_id` cannot do better — it holds only the *current* plan — which is
//! why Phase 13 writes `usage_events.plan_id` at write time (A303) and why
//! [`a_user_who_converted_in_july_keeps_junes_cost_in_the_free_column`] fails if the query
//! falls back to it.
//!
//! Two more rules the report is read for: a month with no data is `null` rather than `0`
//! (gate 7(a) — a zero is an invented number and a tripwire reads it as a measurement), and
//! BYOK is its own column rather than blended into either tier (A156).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json as request, signed_up};

fn instant(raw: &str) -> DateTime<Utc> {
    raw.parse().expect("a valid timestamp")
}

fn router(harness: &TestState) -> axum::Router {
    stvdy_server::routes::router(harness.state.clone()).expect("the router builds")
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// A signed-up account whose `is_admin` is set, and its id.
async fn admin(harness: &TestState, email: &str) -> (Uuid, String) {
    let app = router(harness);
    let token = signed_up(&app, harness, email).await;
    sqlx::query("update users set is_admin = true where email = $1")
        .bind(email)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    (user_id_of(harness, email).await, token)
}

/// An account with no admin flag, on the free plan.
async fn member(harness: &TestState, email: &str) -> Uuid {
    let app = router(harness);
    signed_up(&app, harness, email).await;
    user_id_of(harness, email).await
}

/// One `usage_events` row with an explicit instant and an explicit plan of record.
///
/// `plan_id` is written here rather than left to the account's current plan, because that is
/// the whole difference this suite is about: the row carries the plan it was spent under.
async fn usage(
    harness: &TestState,
    user_id: Uuid,
    plan: &str,
    cost: f64,
    byok: bool,
    at: DateTime<Utc>,
) {
    sqlx::query(
        "insert into usage_events
            (user_id, action, credits, plan_id, provider_cost_usd, byok, created_at)
         values ($1, 'chat_message', 1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(plan)
    .bind(cost)
    .bind(byok)
    .bind(at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// One `payments` row, as the money path would have written it.
async fn paid(
    harness: &TestState,
    user_id: Uuid,
    plan: &str,
    amount_idr: i64,
    fee_idr: i64,
    at: DateTime<Utc>,
) {
    sqlx::query(
        "insert into payments
            (user_id, kind, plan_id, billing_period, provider, order_id, amount_idr,
             gateway_fee_idr, expires_at, status, paid_at)
         values ($1, 'subscription', $2, 'monthly', 'qris_gateway', $3, $4, $5, $6, 'paid', $7)",
    )
    .bind(user_id)
    .bind(plan)
    .bind(format!("order-{}", Uuid::now_v7()))
    .bind(amount_idr)
    .bind(fee_idr)
    .bind(at + Duration::minutes(15))
    .bind(at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// The month a report row is for, from the wire shape.
fn month<'a>(body: &'a Value, month: &str) -> &'a Value {
    body["months"]
        .as_array()
        .expect("months is an array")
        .iter()
        .find(|row| row["month"] == month)
        .unwrap_or_else(|| panic!("a row for {month}: {body}"))
}

fn plan_row<'a>(month: &'a Value, plan: &str) -> &'a Value {
    month["per_plan"]
        .as_array()
        .expect("per_plan is an array")
        .iter()
        .find(|row| row["plan_id"] == plan)
        .unwrap_or_else(|| panic!("a row for {plan}: {month}"))
}

/// The month's provider cost split three ways: `(free, paid, byok)`.
///
/// Computed from the per-plan rows rather than read from a field, because that is how the
/// report carries it now: a tier's figure is the sum of its plans, and a second top-level
/// figure would be a second answer to the same question.
fn split(month: &Value) -> (f64, f64, f64) {
    let mut free = 0.0;
    let mut paid = 0.0;
    let mut byok = 0.0;
    for row in month["per_plan"].as_array().expect("per_plan is an array") {
        let cost = row["provider_cost_usd"].as_f64().unwrap_or(0.0);
        if row["plan_id"] == "free" {
            free += cost;
        } else {
            paid += cost;
        }
        byok += row["byok_cost_usd"].as_f64().unwrap_or(0.0);
    }
    (free, paid, byok)
}

async fn margin(app: &axum::Router, token: &str, from: &str, to: &str) -> (StatusCode, Value) {
    request(
        app,
        "GET",
        &format!("/admin/margin?from={from}&to={to}"),
        token,
        json!(null),
    )
    .await
}

/// The headline acceptance: free and paid provider cost are separate columns, per plan, per
/// month — the thing A46's tripwires cannot be read from a blend.
#[sqlx::test(migrations = "../../migrations")]
async fn the_margin_report_splits_free_and_paid_provider_cost_per_plan_and_month(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "margin-admin@example.test").await;

    let free = member(&harness, "margin-free@example.test").await;
    let pro = member(&harness, "margin-pro@example.test").await;
    let power = member(&harness, "margin-power@example.test").await;

    // June, all three tiers, each with a cost of its own.
    let june = instant("2026-06-15T03:00:00Z");
    usage(&harness, free, "free", 0.25, false, june).await;
    usage(&harness, pro, "pro", 1.50, false, june).await;
    usage(&harness, power, "power", 4.00, false, june).await;
    // And a BYOK call on the power account: its own column, inside neither tier (A156).
    usage(&harness, power, "power", 0.10, true, june).await;

    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let june_row = month(&body, "2026-06");
    assert_eq!(
        plan_row(june_row, "free")["provider_cost_usd"],
        0.25,
        "{body}"
    );
    assert_eq!(
        plan_row(june_row, "pro")["provider_cost_usd"],
        1.50,
        "{body}"
    );
    assert_eq!(
        plan_row(june_row, "power")["provider_cost_usd"],
        4.00,
        "BYOK is not inside the tier's figure: {body}"
    );
    assert_eq!(plan_row(june_row, "power")["byok_cost_usd"], 0.10, "{body}");

    // The split is also derivable per month, because that is the resolution the tripwires
    // read: free is its own figure and BYOK is inside neither tier.
    let (free_usd, paid_usd, byok_usd) = split(june_row);
    assert!((free_usd - 0.25).abs() < 1e-9, "{body}");
    assert!((paid_usd - 5.50).abs() < 1e-9, "{body}");
    assert!((byok_usd - 0.10).abs() < 1e-9, "{body}");
}

/// **The attribution case.** A user who was free in June and paid in July keeps June's cost in
/// the free column (A303). Seeded with `usage_events.plan_id`, this is the test that fails if
/// the query falls back to `users.plan_id`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_user_who_converted_in_july_keeps_junes_cost_in_the_free_column(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "convert-admin@example.test").await;

    let converter = member(&harness, "converter@example.test").await;

    // June: on the free plan, spending. The row says `free`.
    usage(
        &harness,
        converter,
        "free",
        0.40,
        false,
        instant("2026-06-20T03:00:00Z"),
    )
    .await;

    // July: the student pays, and the account becomes pro. This is the moment the roadmap's
    // join goes wrong: `users.plan_id` now says `pro`, and a read-time join would re-attribute
    // the June row to pro.
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(converter)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    usage(
        &harness,
        converter,
        "pro",
        2.00,
        false,
        instant("2026-07-05T03:00:00Z"),
    )
    .await;

    // June's report, run *after* the conversion — which is the whole point: the report is read
    // at the end of the quarter, months after the conversion happened.
    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let june_row = month(&body, "2026-06");
    assert_eq!(
        plan_row(june_row, "free")["provider_cost_usd"],
        0.40,
        "June's cost stays in the free column after a July conversion: {body}"
    );
    // And nothing leaked into pro: a read-time join would put 0.40 here.
    let (free_usd, paid_usd, _) = split(june_row);
    assert!(
        (paid_usd - 0.0).abs() < 1e-9,
        "the converting student's June spend is not attributed to the paid tier: {body}"
    );
    assert!(
        (free_usd - 0.40).abs() < 1e-9,
        "the free-tier figure the report exists for is intact: {body}"
    );

    // July's report has the paid spend, attributed to pro.
    let (_, july) = margin(&app, &token, "2026-07-01T00:00:00Z", "2026-08-01T00:00:00Z").await;
    assert_eq!(
        plan_row(month(&july, "2026-07"), "pro")["provider_cost_usd"],
        2.00,
        "{july}"
    );
}

/// The `coalesce` fallback: a row written before Phase 13 has no `plan_id` of its own, so the
/// report reads the account's current plan rather than dropping the cost. Wrong-by-approximation
/// on old history is the documented behaviour; wrong-by-omission would understate every tier.
#[sqlx::test(migrations = "../../migrations")]
async fn a_row_with_no_plan_of_record_falls_back_to_the_accounts_current_plan(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "legacy-admin@example.test").await;

    let legacy = member(&harness, "legacy@example.test").await;
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(legacy)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    // No `plan_id` on the row: this is what pre-Phase-13 history looks like.
    sqlx::query(
        "insert into usage_events (user_id, action, credits, provider_cost_usd, created_at)
         values ($1, 'chat_message', 1, 0.75, $2)",
    )
    .bind(legacy)
    .bind(instant("2026-06-10T03:00:00Z"))
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        plan_row(month(&body, "2026-06"), "pro")["provider_cost_usd"],
        0.75,
        "a row with no plan of record falls back to the account's plan rather than being \
         dropped: {body}"
    );
}

/// Utilization and conversion are derived from that split, and the two conversion ratios are
/// both present because A46's 4% is one of them (A304).
#[sqlx::test(migrations = "../../migrations")]
async fn free_tier_utilization_and_conversion_are_derived_from_the_split(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "util-admin@example.test").await;

    // Four free accounts, two of which pay: conversion_of_free = 2/4 = 0.5, and
    // conversion_of_total = 2/6 with the two paid accounts counted.
    let mut free_accounts = Vec::new();
    for index in 0..4 {
        free_accounts.push(member(&harness, &format!("util-free-{index}@example.test")).await);
    }
    for index in 0..2 {
        let user_id = member(&harness, &format!("util-paid-{index}@example.test")).await;
        sqlx::query("update users set plan_id = 'pro' where id = $1")
            .bind(user_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
        // A live subscription, because that is what "paying" means to the report (A146): a
        // stored `plan_id` with no live subscription grants no paid limits, so counting it as
        // a paying account would overstate conversion by exactly the accounts that never paid.
        sqlx::query(
            "insert into subscriptions
                (user_id, plan_id, status, billing_period, current_period_start,
                 current_period_end)
             values ($1, 'pro', 'active', 'monthly', $2, $3)",
        )
        .bind(user_id)
        .bind(instant("2026-06-01T00:00:00Z"))
        .bind(instant("2026-07-01T00:00:00Z"))
        .execute(&harness.state.pool)
        .await
        .unwrap();
        paid(
            &harness,
            user_id,
            "pro",
            10_000,
            250,
            instant("2026-06-05T03:00:00Z"),
        )
        .await;
    }

    // Free tier: 60 credits granted to each of the four, 18 spent in total. The figure the
    // report carries is the tier's own ratio — total spent over total granted — so this is 18
    // of 240, and it equals A46's "≈17 of 60" only because every free account is granted the
    // same allowance. Reading it as one account's share would report 30% where the tier used
    // 7.5%, which is the mistake this assertion exists to pin down.
    let at = instant("2026-06-12T03:00:00Z");
    for user_id in &free_accounts {
        sqlx::query(
            "insert into credit_ledger (user_id, delta, bucket, reason, period_key, plan_id, created_at)
             values ($1, 60, 'monthly', 'monthly_grant', $2, 'free', $3)",
        )
        .bind(user_id)
        .bind(format!("monthly:2026-06:{user_id}"))
        .bind(at)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }
    // 18 spent across the four, in the monthly bucket.
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason, plan_id, created_at)
         values ($1, -18, 'monthly', 'usage:chat_message', 'free', $2)",
    )
    .bind(free_accounts[0])
    .bind(at)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let june_row = month(&body, "2026-06");
    assert_eq!(june_row["free_accounts"], 4, "{body}");
    assert_eq!(june_row["paying"], 2, "{body}");
    let utilization = june_row["free_utilization"]
        .as_f64()
        .expect("a utilization figure");
    assert!(
        (utilization - 0.075).abs() < 1e-9,
        "18 of 240 credits across the four free accounts is 7.5%: {body}"
    );
    let of_free = june_row["conversion_of_free"]
        .as_f64()
        .expect("conversion_of_free");
    assert!(
        (of_free - 0.5).abs() < 1e-9,
        "2 paying of 4 free is 0.5: {body}"
    );
    let of_total = june_row["conversion_of_total"]
        .as_f64()
        .expect("conversion_of_total");
    assert!(
        (of_total - 2.0 / 6.0).abs() < 1e-9,
        "2 paying of 6 accounts is a third: {body}"
    );

    // Revenue and the gateway fee come from `payments`, per plan and per month.
    assert_eq!(june_row["revenue_idr"], 20_000, "{body}");
    assert_eq!(june_row["gateway_fee_idr"], 500, "{body}");
    assert_eq!(plan_row(june_row, "pro")["revenue_idr"], 20_000, "{body}");
}

/// A month with no rows is `null`, never `0`: a zero would be an invented number and a
/// tripwire would read it as a real measurement (gate 7(a)).
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_month_is_null_and_the_fx_rate_used_is_printed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "empty-admin@example.test").await;

    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let june_row = month(&body, "2026-06");
    assert!(
        june_row["free_utilization"].is_null(),
        "no grants means no utilization, not 0%: {body}"
    );
    assert!(
        june_row["conversion_of_free"].is_null(),
        "no accounts means no conversion, not 0%: {body}"
    );
    assert!(june_row["conversion_of_total"].is_null(), "{body}");
    assert_eq!(june_row["revenue_idr"], 0, "a sum of no rows is 0 IDR");
    assert_eq!(june_row["paying"], 0, "a count of no rows is 0");
    assert!(
        june_row["per_plan"]
            .as_array()
            .expect("an array")
            .is_empty(),
        "no spend means no per-plan rows rather than zero-cost ones: {body}"
    );

    // The FX rate is printed as the input it is (A41/A304): a reader has to be able to check
    // the gross-margin arithmetic against the rate it used.
    assert!(
        body["idr_per_usd"].as_i64().unwrap_or(0) > 0,
        "the rate used is printed: {body}"
    );
    assert!(
        body["as_of"].is_string() || june_row["as_of"].is_string(),
        "the report says when it was computed: {body}"
    );
}

/// BYOK cost is its own column, and it is not inside either tier's cost (A156).
#[sqlx::test(migrations = "../../migrations")]
async fn byok_cost_is_its_own_column(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "byok-admin@example.test").await;

    let power = member(&harness, "byok-margin@example.test").await;
    sqlx::query("update users set plan_id = 'power' where id = $1")
        .bind(power)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let at = instant("2026-06-15T03:00:00Z");
    usage(&harness, power, "power", 1.20, false, at).await;
    usage(&harness, power, "power", 0.30, true, at).await;

    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let row = plan_row(month(&body, "2026-06"), "power");
    assert_eq!(row["provider_cost_usd"], 1.20, "{body}");
    assert_eq!(row["byok_cost_usd"], 0.30, "{body}");
    let (_, paid_usd, byok_usd) = split(month(&body, "2026-06"));
    assert!(
        (paid_usd - 1.20).abs() < 1e-9,
        "BYOK is excluded from the paid tier rather than blended in: {body}"
    );
    assert!((byok_usd - 0.30).abs() < 1e-9, "{body}");
    // Gross margin is revenue minus *our* cost, so the BYOK column does not reduce it.
    let rate = body["idr_per_usd"].as_f64().unwrap_or(0.0);
    assert_eq!(
        row["gross_margin_idr"].as_i64().unwrap_or(0),
        -((1.20 * rate).round() as i64),
        "gross margin counts our cost only: {body}"
    );
}

/// The tripwires are booleans computed from the numbers, and the two-month one is null when
/// the range has one month.
#[sqlx::test(migrations = "../../migrations")]
async fn the_tripwires_are_computed_and_the_two_month_one_is_null_without_a_previous(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "tripwire-admin@example.test").await;

    // One free account that spends far past 30% of its grant, and no paying account at all:
    // utilization trips and conversion trips, in the direction A46 cares about.
    let free = member(&harness, "tripwire-free@example.test").await;
    let at = instant("2026-06-12T03:00:00Z");
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason, period_key, plan_id, created_at)
         values ($1, 60, 'monthly', 'monthly_grant', $2, 'free', $3)",
    )
    .bind(free)
    .bind(format!("monthly:2026-06:{free}"))
    .bind(at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason, plan_id, created_at)
         values ($1, -40, 'monthly', 'usage:chat_message', 'free', $2)",
    )
    .bind(free)
    .bind(at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    usage(&harness, free, "free", 0.30, false, at).await;

    // A one-month range: "two months running" cannot be computed from one.
    let (status, body) = margin(&app, &token, "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tripwires = &body["tripwires"];
    assert_eq!(
        tripwires["free_utilization_over_30"], true,
        "40 of 60 is over 30%: {body}"
    );
    assert_eq!(
        tripwires["conversion_under_3"], true,
        "no paying accounts is under 3%: {body}"
    );
    assert_eq!(
        tripwires["free_spend_over_quarter_of_revenue"], true,
        "free spend with no revenue at all is over a quarter of it: {body}"
    );
    assert!(
        tripwires["plan_loss_two_months_running"].is_null(),
        "one month cannot say 'two months running': {body}"
    );

    // A two-month range with a previous month of profit makes the same tripwire answerable.
    usage(
        &harness,
        free,
        "free",
        0.01,
        false,
        instant("2026-05-12T03:00:00Z"),
    )
    .await;
    let (_, two_months) =
        margin(&app, &token, "2026-05-01T00:00:00Z", "2026-07-01T00:00:00Z").await;
    assert!(
        two_months["tripwires"]["plan_loss_two_months_running"].is_boolean(),
        "with a previous month the tripwire is answerable: {two_months}"
    );
}

/// Marking paid runs the webhook's own apply, so it activates once and a second call changes
/// nothing (A308).
#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_payment_paid_activates_once(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "mark-admin@example.test").await;

    let student = member(&harness, "mark-student@example.test").await;
    let payment: Uuid = sqlx::query_scalar(
        "insert into payments
            (user_id, kind, plan_id, billing_period, provider, order_id, amount_idr,
             expires_at, status)
         values ($1, 'subscription', 'pro', 'monthly', 'qris_gateway', 'manual-order', 10000,
                 now() + interval '15 minutes', 'pending')
         returning id",
    )
    .bind(student)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = request(
        &app,
        "POST",
        &format!("/admin/payments/{payment}/mark-paid"),
        &token,
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, status_after, plan): (Uuid, String, String) = sqlx::query_as(
        "select p.id, p.status, u.plan_id from payments p
           join users u on u.id = p.user_id
          where p.id = $1",
    )
    .bind(payment)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(status, payment);
    assert_eq!(status_after, "paid");
    assert_eq!(plan, "pro", "the plan is active after a manual mark");

    let subscription: String =
        sqlx::query_scalar("select status from subscriptions where user_id = $1")
            .bind(student)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(subscription, "active");

    // A second call changes nothing: the state transition is guarded, exactly as it is for a
    // replayed webhook (A293/A308).
    let (status, second) = request(
        &app,
        "POST",
        &format!("/admin/payments/{payment}/mark-paid"),
        &token,
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let subscriptions: i64 =
        sqlx::query_scalar("select count(*)::bigint from subscriptions where user_id = $1")
            .bind(student)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(subscriptions, 1, "one subscription, not two");
    let end: DateTime<Utc> =
        sqlx::query_scalar("select current_period_end from subscriptions where user_id = $1")
            .bind(student)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        end < harness.state.clock.now() + Duration::days(31),
        "and the period was not extended a second time"
    );
}

/// A refund records the row and leaves the subscription alone.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refund_records_the_row_and_does_not_delete_the_subscription(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "refund-admin@example.test").await;

    let student = member(&harness, "refund-student@example.test").await;
    let payment: Uuid = sqlx::query_scalar(
        "insert into payments
            (user_id, kind, plan_id, billing_period, provider, order_id, amount_idr,
             gateway_fee_idr, expires_at, status, paid_at)
         values ($1, 'subscription', 'pro', 'monthly', 'qris_gateway', 'refund-order', 10000,
                 250, now() + interval '15 minutes', 'paid', now())
         returning id",
    )
    .bind(student)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(student)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = request(
        &app,
        "POST",
        &format!("/admin/payments/{payment}/refund"),
        &token,
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status_after, fee): (String, Option<i32>) =
        sqlx::query_as("select status, gateway_fee_idr from payments where id = $1")
            .bind(payment)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(status_after, "refunded");
    assert_eq!(
        fee,
        Some(0),
        "a refunded payment's fee is zeroed rather than left to overstate our cost"
    );

    // The subscription is untouched: a refund is a money fact, and revoking a plan a student
    // may already be using is a decision a human makes, not a side effect of a row update.
    let subscription: String =
        sqlx::query_scalar("select status from subscriptions where user_id = $1")
            .bind(student)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(subscription, "active", "the subscription is left alone");

    // And the row still exists: nothing is deleted (A308).
    let rows: i64 = sqlx::query_scalar("select count(*)::bigint from payments where id = $1")
        .bind(payment)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

/// The review queue A294/A301 feed: `GET /admin/payments?status=review`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_review_queue_lists_the_payments_that_need_a_human(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (_, token) = admin(&harness, "queue-admin@example.test").await;

    let student = member(&harness, "queue-student@example.test").await;
    for (order, reason) in [
        ("queue-a", Some("amount 9000 does not match 10000")),
        ("queue-b", None),
    ] {
        sqlx::query(
            "insert into payments
                (user_id, kind, plan_id, billing_period, provider, order_id, amount_idr,
                 expires_at, status, review_reason)
             values ($1, 'subscription', 'pro', 'monthly', 'qris_gateway', $2, 10000,
                     now() + interval '15 minutes', $3, $4)",
        )
        .bind(student)
        .bind(order)
        .bind(if reason.is_some() {
            "review"
        } else {
            "pending"
        })
        .bind(reason)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    let (status, body) = request(
        &app,
        "GET",
        "/admin/payments?status=review",
        &token,
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["payments"].as_array().expect("a payments array");
    assert_eq!(
        rows.len(),
        1,
        "only the flagged row is in the queue: {body}"
    );
    assert_eq!(rows[0]["order_id"], "queue-a", "{body}");
    assert_eq!(
        rows[0]["review_reason"], "amount 9000 does not match 10000",
        "the reason travels with the row, because it is what the human reads: {body}"
    );
}

/// Every admin payment route is 404 for a non-admin (A7).
#[sqlx::test(migrations = "../../migrations")]
async fn a_non_admin_sees_not_found(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);

    let student = member(&harness, "nonadmin@example.test").await;
    let token = {
        let (_, token) = {
            let app = router(&harness);
            let token = signed_up(&app, &harness, "nonadmin2@example.test").await;
            (0, token)
        };
        token
    };
    let payment: Uuid = sqlx::query_scalar(
        "insert into payments
            (user_id, kind, plan_id, billing_period, provider, order_id, amount_idr,
             expires_at, status)
         values ($1, 'subscription', 'pro', 'monthly', 'qris_gateway', 'nonadmin-order', 10000,
                 now() + interval '15 minutes', 'pending')
         returning id",
    )
    .bind(student)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    for (method, path) in [
        ("GET", "/admin/payments?status=review".to_string()),
        ("POST", format!("/admin/payments/{payment}/mark-paid")),
        ("POST", format!("/admin/payments/{payment}/refund")),
        ("GET", "/admin/margin".to_string()),
    ] {
        let (status, body) = request(&app, method, &path, &token, json!(null)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} must be invisible to a non-admin: {body}"
        );
        assert_eq!(body["code"], "not_found", "{method} {path}: {body}");
    }
}
