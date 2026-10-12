//! `crates/server/src/settings.rs`: the partial write, the quiet hours and the plan's
//! channels.
//!
//! The channel case is here rather than in the route sweep, and that is deliberate: a
//! `Swept` row's refusal is driven by `Fixture::call`, which has no per-row body, so a swept
//! `PUT /settings` could only assert what an *empty* body does — which refuses nothing. The
//! honest classification is `ReadOnly` plus a real test here (A209).
//!
//! The quiet-hours case is the other one worth having: `22 -> 7` is the normal window, so a
//! naive `start < end` check rejects exactly the case the feature exists for.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// A live subscription is part of the fixture, not decoration: the decision honours
/// `users.plan_id` only while a subscription grants paid limits (A146).
async fn upgrade_to(harness: &TestState, email: &str, plan: &str) {
    let user_id = user_id_of(harness, email).await;
    sqlx::query("update users set plan_id = $2 where id = $1")
        .bind(user_id)
        .bind(plan)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .bind(plan)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// A free plan's channels are `[web, push]`, so `telegram` is 403 `plan_required` with
/// `min_plan: "power"` — the only plan that grants it. The check is the engine's; a
/// `plans.get(..).reminder_channels` read here would fail the route sweep's third test and
/// be a second source of truth (A201).
#[sqlx::test(migrations = "../../migrations")]
async fn a_channel_the_plan_does_not_grant_is_refused_with_the_plan_that_does(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "channels@example.test").await;

    // The row is read first so it exists: the refusal must leave a *stored* row alone, and a
    // row that was never there would make "unchanged" a vacuous claim.
    let (status, before) = json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{before}");

    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "reminder_channels": ["web", "telegram"] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "plan_required", "{answer}");
    assert_eq!(answer["min_plan"], "power", "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "the refusal explains itself: {answer}"
    );

    // The refusal preceded the write, so nothing changed: a write that saved the channel and
    // then refused would leave the account with a channel it cannot receive on.
    let (status, after) = json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(
        after["settings"]["reminder_channels"], before["settings"]["reminder_channels"],
        "the refusal must leave the row exactly as it was"
    );
}

/// The granted set saves and comes back, in the order the client sent.
#[sqlx::test(migrations = "../../migrations")]
async fn a_granted_channel_set_saves(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "pro-channels@example.test").await;
    upgrade_to(&harness, "pro-channels@example.test", "pro").await;

    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "reminder_channels": ["push", "web", "email"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["settings"]["reminder_channels"],
        serde_json::json!(["push", "web", "email"]),
        "the set round-trips in the order it was sent: {answer}"
    );

    // And it is the stored row, not just the response.
    let (status, answer) = json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["settings"]["reminder_channels"],
        serde_json::json!(["push", "web", "email"]),
        "{answer}"
    );

    // A channel the *matrix* does not know is refused by the payload parse, before the plan
    // is ever consulted: an unknown channel is a typo, not an upgrade.
    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "reminder_channels": ["carrier_pigeon"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
}

/// Quiet hours wrap midnight, so `22 -> 7` is valid and the *membership* test is what a
/// consumer uses. A naive `start < end` check rejects the normal case (A202).
#[sqlx::test(migrations = "../../migrations")]
async fn quiet_hours_may_wrap_midnight(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quiet@example.test").await;

    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "quiet_start_hour": 22, "quiet_end_hour": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["settings"]["quiet_start_hour"], 22, "{answer}");
    assert_eq!(answer["settings"]["quiet_end_hour"], 7, "{answer}");

    // One hour alone is half a window: 400, because the other half would be a stored value
    // the client did not send.
    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "quiet_start_hour": 22 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
    assert!(
        answer["message"].as_str().unwrap().contains("pair"),
        "the refusal explains the pair rule: {answer}"
    );

    // The out-of-range hour the migration's constraint would reject reaches the caller as a
    // 400 with a code, never as a 500 from the constraint.
    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "quiet_start_hour": 24, "quiet_end_hour": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");

    // And clearing the pair is an explicit null on both, which is the only way to say "no
    // quiet hours" once they have been set.
    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "quiet_start_hour": null, "quiet_end_hour": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer["settings"]["quiet_start_hour"].is_null(), "{answer}");
    assert!(answer["settings"]["quiet_end_hour"].is_null(), "{answer}");
}

/// A partial write leaves everything it did not mention alone, and a second write with one
/// field does not reset the rest — the failure a full-replace PUT would cause when a stale
/// tab saves one section.
#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_write_does_not_reset_the_rest(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "partial-settings@example.test").await;
    upgrade_to(&harness, "partial-settings@example.test", "power").await;

    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({
            "study_start_hour": 7,
            "study_end_hour": 21,
            "max_study_min_per_day": 300,
            "block_len_min": 45,
            "break_min": 15,
            "reminder_channels": ["web", "push", "email", "telegram", "discord"],
            "quiet_start_hour": 23,
            "quiet_end_hour": 6,
            "language": "id",
            "theme": "dark",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    // One field, and only that field moves.
    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "theme": "light" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let settings = &answer["settings"];
    assert_eq!(settings["theme"], "light", "{answer}");
    for (field, expected) in [
        ("study_start_hour", serde_json::json!(7)),
        ("study_end_hour", serde_json::json!(21)),
        ("max_study_min_per_day", serde_json::json!(300)),
        ("block_len_min", serde_json::json!(45)),
        ("break_min", serde_json::json!(15)),
        ("quiet_start_hour", serde_json::json!(23)),
        ("quiet_end_hour", serde_json::json!(6)),
        ("language", serde_json::json!("id")),
    ] {
        assert_eq!(settings[field], expected, "{field} was reset: {answer}");
    }
    assert_eq!(
        settings["reminder_channels"],
        serde_json::json!(["web", "push", "email", "telegram", "discord"]),
        "the channels were reset: {answer}"
    );
}

/// The timezone is on `users` and this route is the only writer (A203), so it round-trips
/// and an unknown zone is refused rather than stored as a string `chrono-tz` will silently
/// read as UTC.
#[sqlx::test(migrations = "../../migrations")]
async fn the_timezone_round_trips_and_an_unknown_zone_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "zone@example.test").await;
    let user_id = user_id_of(&harness, "zone@example.test").await;

    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "timezone": "Asia/Makassar" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["timezone"], "Asia/Makassar", "{answer}");

    // It is written to `users`, which is where the reminder and planner maths read it from.
    let stored: String = sqlx::query_scalar("select timezone from users where id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, "Asia/Makassar");

    let (status, answer) = json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["timezone"], "Asia/Makassar", "{answer}");

    let (status, answer) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "timezone": "Mars/Olympus" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");

    // The refusal did not store it: a zone `chrono-tz` cannot read becomes UTC, which puts
    // every date on the Today view on the wrong day for the student.
    let stored: String = sqlx::query_scalar("select timezone from users where id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, "Asia/Makassar");
}

/// The out-of-range values the migration's constraints would reject reach the caller as a
/// 400 with a code, never as a 500 from a constraint violation.
#[sqlx::test(migrations = "../../migrations")]
async fn out_of_range_study_hours_are_a_validation_error(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "range@example.test").await;

    for (name, body) in [
        (
            "start after end",
            serde_json::json!({ "study_start_hour": 23, "study_end_hour": 7 }),
        ),
        (
            "start equal to end",
            serde_json::json!({ "study_start_hour": 8, "study_end_hour": 8 }),
        ),
        (
            "start out of range",
            serde_json::json!({ "study_start_hour": 24, "study_end_hour": 23 }),
        ),
        (
            "end out of range",
            serde_json::json!({ "study_start_hour": 8, "study_end_hour": 25 }),
        ),
        (
            "daily limit too small",
            serde_json::json!({ "max_study_min_per_day": 5 }),
        ),
        (
            "block length too long",
            serde_json::json!({ "block_len_min": 300 }),
        ),
        ("break too long", serde_json::json!({ "break_min": 90 })),
        ("unknown theme", serde_json::json!({ "theme": "solarized" })),
        ("unknown language", serde_json::json!({ "language": "fr" })),
        (
            "a field the client does not own",
            serde_json::json!({ "user_id": Uuid::now_v7() }),
        ),
    ] {
        let (status, answer) = json(&app, "PUT", "/settings", &token, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{name} must be a 400, got {status}: {answer}"
        );
        assert_eq!(answer["code"], "validation", "{name}: {answer}");
    }
}

/// A read creates the row rather than answering "nothing", so the screen has one shape.
#[sqlx::test(migrations = "../../migrations")]
async fn a_first_read_returns_the_defaults(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "defaults@example.test").await;
    let user_id = user_id_of(&harness, "defaults@example.test").await;

    // No settings row exists yet: signup does not create one.
    let before: i64 =
        sqlx::query_scalar("select count(*)::bigint from user_settings where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(before, 0, "the fixture starts with no settings row");

    let (status, answer) = json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let settings = &answer["settings"];
    assert_eq!(settings["study_start_hour"], 8, "{answer}");
    assert_eq!(settings["study_end_hour"], 22, "{answer}");
    assert_eq!(settings["max_study_min_per_day"], 240, "{answer}");
    assert_eq!(settings["block_len_min"], 50, "{answer}");
    assert_eq!(settings["break_min"], 10, "{answer}");
    assert_eq!(
        settings["reminder_channels"],
        serde_json::json!(["web"]),
        "{answer}"
    );
    assert_eq!(settings["language"], "en", "{answer}");
    assert_eq!(settings["theme"], "system", "{answer}");
    assert!(
        settings["quiet_start_hour"].is_null() && settings["quiet_end_hour"].is_null(),
        "the quiet hours default to unset: {answer}"
    );
    assert_eq!(answer["timezone"], "Asia/Jakarta", "{answer}");

    // The row now exists, and a second read does not create a second `change_log` entry:
    // the read is idempotent, so a screen the student merely opened does not sync.
    let after: i64 =
        sqlx::query_scalar("select count(*)::bigint from user_settings where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(after, 1);

    let (status, _) = json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let changes: i64 = sqlx::query_scalar(
        "select count(*)::bigint from change_log where user_id = $1 and entity = 'user_settings'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        changes, 1,
        "a second read must not write a second change_log row"
    );
}
