//! `crates/server/src/reminders.rs`: registration, the feed, and the ack.
//!
//! The first case is the exit gate's reminder half. Two devices computing the same due time an
//! hour apart must produce **one** row, and the row must keep the **first** `fire_at` — which
//! is what a key derived from the fire instant fails, and it fails in the direction that is
//! invisible in a single-device test: two rows, two notifications.
//!
//! The ack case is the other direction of the same story: an ack is per device, so silencing a
//! reminder on the phone in the student's hand must not silence it on the laptop they will open
//! in an hour.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// A second device for the same account, which is what "two devices" means here.
async fn login(app: &axum::Router, email: &str, name: &str) -> serde_json::Value {
    let (status, body) = json(
        app,
        "POST",
        "/auth/login",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": name, "platform": "ios" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn token_of(body: &serde_json::Value) -> String {
    body["access_token"]
        .as_str()
        .expect("access token")
        .to_string()
}

fn device_of(body: &serde_json::Value) -> Uuid {
    Uuid::parse_str(body["device_id"].as_str().expect("device id")).unwrap()
}

/// Creates a task due at `due_at` and returns its id.
async fn a_task(app: &axum::Router, token: &str, due_at: &str) -> Uuid {
    let (status, body) = json(
        app,
        "POST",
        "/tasks",
        token,
        serde_json::json!({ "title": "Essay", "due_at": due_at }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["task"]["id"].as_str().expect("task id")).unwrap()
}

/// Sets the caller's quiet hours.
async fn set_quiet(app: &axum::Router, token: &str, start: i64, end: i64) {
    let (status, body) = json(
        app,
        "PUT",
        "/settings",
        token,
        serde_json::json!({ "quiet_start_hour": start, "quiet_end_hour": end }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn reminder_rows(harness: &TestState) -> Vec<(String, chrono::DateTime<chrono::Utc>)> {
    sqlx::query_as("select dedupe_key, fire_at from reminders order by dedupe_key")
        .fetch_all(&harness.state.pool)
        .await
        .unwrap()
}

/// The exit gate's reminder half. Two devices compute the same due time an hour apart, and
/// there is one row — whose `fire_at` is the FIRST one, which a fire-time key would fail.
///
/// **The clock is pinned, and it has to be.** `TestState::new` pins it to `Utc::now()`, and
/// this case's fixtures are absolute instants — `08:00Z` and `09:00Z` on 2026-10-12 — because
/// the dedupe key embeds the *local date* of the fire instant, so a relative fixture would
/// assert a different key depending on the day it ran. Against a real `now` those instants are
/// in the past, and `reminders.rs`'s registration window refuses anything more than
/// `CLOCK_SKEW_MIN` behind it: this suite went red on 2026-10-12 at 09:05 UTC for exactly that
/// reason, with `fire_at must be no earlier than 5 minutes ago`. Pinning is the same remedy the
/// quiet-hours case below already documents. `08:00Z` is chosen so both fixture instants are
/// inside the window (`earliest` is then 07:55Z, and the second device's *earlier* 08:00Z must
/// be accepted for the row to keep the first one) and so both land on the same Jakarta date.
#[sqlx::test(migrations = "../../migrations")]
async fn two_devices_asking_for_the_same_reminder_produce_one_row(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(Arc::new(FixedClock::at(
        "2026-10-12T08:00:00Z".parse().unwrap(),
    )));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let email = "reminders@example.test";
    let first = signed_up(&app, &harness, email).await;
    let second = login(&app, email, "Jetta's iPhone").await;
    let second_token = token_of(&second);

    let due = "2026-10-12T10:00:00Z";
    let task = a_task(&app, &first, due).await;

    // The first device computes 09:00Z; the second is an hour out of step and computes
    // 08:00Z. Both are inside the window, both are for the same task on the same local date
    // in Jakarta (the 12th is the 12th for both), so both must key to the same string.
    let (status, first_answer) = json(
        &app,
        "POST",
        "/reminders",
        &first,
        serde_json::json!({
            "kind": "task_due", "subject_id": task, "fire_at": "2026-10-12T09:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first_answer}");
    assert_eq!(first_answer["applied"], true, "{first_answer}");

    let (status, second_answer) = json(
        &app,
        "POST",
        "/reminders",
        &second_token,
        serde_json::json!({
            "kind": "task_due", "subject_id": task, "fire_at": "2026-10-12T08:00:00Z"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the second device's identical reminder is not a conflict: {second_answer}"
    );
    assert_eq!(second_answer["applied"], false, "{second_answer}");
    assert_eq!(
        second_answer["reminder_id"], first_answer["reminder_id"],
        "both devices must be handed the same row: {second_answer}"
    );

    let rows = reminder_rows(&harness).await;
    assert_eq!(rows.len(), 1, "one reminder, not two: {rows:?}");
    assert_eq!(rows[0].0, format!("task_due:{task}:2026-10-12"));
    assert_eq!(
        rows[0].1.to_rfc3339(),
        "2026-10-12T09:00:00+00:00",
        "the row keeps the first fire_at: a later write must not move it"
    );
}

/// Another account's task is a 404, never a reminder about a stranger's deadline.
///
/// **The clock is pinned for the same reason as the case above**, and this one is worth
/// reading for a second fact: the failure it produced when the fixture aged out was a
/// **400, not the 404 the test expects**. Body validation runs before the ownership lookup,
/// so a stale `fire_at` short-circuits ahead of the check this test exists to assert — which
/// is the right order (validate the body, then authorize) and the wrong thing to leave the
/// assertion resting on. The pin makes the request reach the ownership check.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reminder_for_another_users_task_is_not_found(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(Arc::new(FixedClock::at(
        "2026-10-12T08:00:00Z".parse().unwrap(),
    )));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let alice = signed_up(&app, &harness, "alice-rem@example.test").await;
    let bob = signed_up(&app, &harness, "bob-rem@example.test").await;
    let bob_task = a_task(&app, &bob, "2026-10-12T10:00:00Z").await;

    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &alice,
        serde_json::json!({
            "kind": "task_due", "subject_id": bob_task, "fire_at": "2026-10-12T09:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // And the kind has to match the subject: a `task_due` naming an event id is the same 404,
    // because the row it would fire from does not exist in the table it names.
    let (status, body) = json(
        &app,
        "POST",
        "/events",
        &alice,
        serde_json::json!({
            "kind": "class", "title": "Lecture", "starts_at": "2026-10-12T10:00:00Z",
            "ends_at": "2026-10-12T11:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let event = Uuid::parse_str(body["event"]["id"].as_str().expect("event id")).unwrap();

    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &alice,
        serde_json::json!({
            "kind": "task_due", "subject_id": event, "fire_at": "2026-10-12T09:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // The matching kind works, so the two 404s above are about ownership and table and not
    // about the route being broken.
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &alice,
        serde_json::json!({
            "kind": "event_soon", "subject_id": event, "fire_at": "2026-10-12T09:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    assert!(reminder_rows(&harness).await.len() == 1);
}

/// The window is bounded in both directions: a fire time last week, and one five years out,
/// are both 400 with a message naming the bound.
#[sqlx::test(migrations = "../../migrations")]
async fn the_fire_window_is_bounded(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "window@example.test").await;

    // A task far enough out that the subject itself is never the reason a reminder is
    // refused, so each case below fails on the fire window alone.
    let task = a_task(&app, &token, "2027-10-12T10:00:00Z").await;

    let now = harness.state.clock.now();
    let past = (now - chrono::Duration::days(7)).to_rfc3339();
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &token,
        serde_json::json!({ "kind": "task_due", "subject_id": task, "fire_at": past }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("no earlier than")),
        "the message must name the bound: {body}"
    );

    let far = (now + chrono::Duration::days(365 * 5)).to_rfc3339();
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &token,
        serde_json::json!({ "kind": "task_due", "subject_id": task, "fire_at": far }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("days ahead")),
        "the message must name the bound: {body}"
    );

    // Inside the window in both directions is accepted: five minutes of clock skew behind, and
    // thirteen days ahead, are both ordinary.
    for offset in [-chrono::Duration::minutes(4), chrono::Duration::days(13)] {
        let (status, body) = json(
            &app,
            "POST",
            "/reminders",
            &token,
            serde_json::json!({
                "kind": "task_due", "subject_id": task, "fire_at": (now + offset).to_rfc3339()
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
}

/// An ack is per device, so the same reminder still shows on the student's other phone.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ack_is_per_device(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let email = "ack@example.test";
    let first = signed_up(&app, &harness, email).await;
    let second = login(&app, email, "Jetta's iPhone").await;
    let second_token = token_of(&second);
    let second_device = device_of(&second);

    let now = harness.state.clock.now();
    // Due already, so the feed carries it: a reminder that is not yet due is not in the feed
    // and an ack test against one would pass for the wrong reason.
    let fire_at = (now - chrono::Duration::minutes(1)).to_rfc3339();
    let task = a_task(&app, &first, "2026-10-12T10:00:00Z").await;

    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &first,
        serde_json::json!({ "kind": "task_due", "subject_id": task, "fire_at": fire_at }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let reminder = Uuid::parse_str(body["reminder_id"].as_str().unwrap()).unwrap();

    // Both devices see it.
    for token in [&first, &second_token] {
        let (status, body) = json(&app, "GET", "/reminders", token, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["reminders"].as_array().unwrap().len(), 1, "{body}");
    }

    // The first device acknowledges, and it is gone from that device's feed.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/reminders/{reminder}/ack"),
        &first,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = json(&app, "GET", "/reminders", &first, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["reminders"].as_array().unwrap().is_empty(),
        "an acked reminder is off this device's feed: {body}"
    );

    let (status, body) = json(
        &app,
        "GET",
        "/reminders",
        &second_token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["reminders"].as_array().unwrap().len(),
        1,
        "the other phone has not seen it yet: {body}"
    );

    // A double tap is a no-op rather than a second delivery row: the ack's unique index is
    // what makes a retry safe, and a retried job that recorded two deliveries would report
    // two sends for one reminder.
    let (status, _) = json(
        &app,
        "POST",
        &format!("/reminders/{reminder}/ack"),
        &first,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let web_deliveries: i64 = sqlx::query_scalar(
        "select count(*) from reminder_deliveries where reminder_id = $1 and channel = 'web'",
    )
    .bind(reminder)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(web_deliveries, 1, "one ack, one delivery row");

    // And another account cannot ack it.
    let stranger = signed_up(&app, &harness, "stranger-rem@example.test").await;
    let (status, body) = json(
        &app,
        "POST",
        &format!("/reminders/{reminder}/ack"),
        &stranger,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // The delivery is keyed to the device that acked, not to the account.
    let acking_device: Uuid = sqlx::query_scalar(
        "select device_id from reminder_deliveries where reminder_id = $1 and channel = 'web'",
    )
    .bind(reminder)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_ne!(acking_device, second_device);
}

/// A quiet-hours deferral is visible: the row's `fire_at` moved and its key did not.
#[sqlx::test(migrations = "../../migrations")]
async fn a_quiet_hours_deferral_moves_the_time_and_keeps_the_key(pool: PgPool) {
    // The clock is pinned one minute before the instant the client reports. `TestState::new`
    // pins it to `Utc::now()`, and this case's whole subject is a fire time that is in the
    // *past* for any real "now" — the client computed 22:30 Jakarta yesterday evening. An
    // unpinned clock makes the registration window refuse it, which is the window working
    // rather than the deferral failing.
    let harness = TestState::new(pool).with_clock(Arc::new(FixedClock::at(
        "2026-10-11T15:29:00Z".parse().unwrap(),
    )));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quiet@example.test").await;

    // 22:00→07:00 Jakarta. A reminder computed for 22:30 local is inside the window.
    set_quiet(&app, &token, 22, 7).await;

    let task = a_task(&app, &token, "2026-10-12T10:00:00Z").await;
    // 15:30Z is 22:30 in Jakarta (UTC+7).
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &token,
        serde_json::json!({
            "kind": "task_due", "subject_id": task, "fire_at": "2026-10-11T15:30:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (key, fire_at): (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("select dedupe_key, fire_at from reminders")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    // The key is the subject and the local date the client asked for, not the instant it
    // landed on.
    assert_eq!(key, format!("task_due:{task}:2026-10-11"));
    assert_eq!(
        fire_at.to_rfc3339(),
        "2026-10-12T00:00:00+00:00",
        "07:00 the next morning Jakarta is midnight UTC"
    );

    // The same task registered again a minute off — what the second device computes — lands
    // on the same local date, so it is the same key and still one row. A deferral must not
    // open a second registration path.
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &token,
        serde_json::json!({
            "kind": "task_due", "subject_id": task, "fire_at": "2026-10-11T15:31:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], false, "{body}");
    assert_eq!(reminder_rows(&harness).await.len(), 1);

    // A different local date is a different reminder, and it is *not* deferred — 09:00
    // Jakarta is outside the 22:00→07:00 window — which is the other half of the rule.
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &token,
        serde_json::json!({
            "kind": "task_due", "subject_id": task, "fire_at": "2026-10-12T02:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let rows = reminder_rows(&harness).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().any(|(key, _)| key.ends_with(":2026-10-12")));
}
