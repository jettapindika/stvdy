//! `crates/server/src/events.rs`: the CRUD, the window, and the four columns a client may
//! not write.
//!
//! The four calendar-owned columns get **one case each**, because a validator that checked
//! only `source` would pass a payload that sets `external_id` and silently break the next
//! incremental sync's `(connection_id, external_id)` match (A198).
//!
//! The window case is the other one worth having: an overlap and a containment filter agree
//! on every short event, so a test whose events are all inside the window passes with the
//! wrong comparison.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// A complete, valid event body, so each case can vary exactly one field.
fn body() -> serde_json::Value {
    serde_json::json!({
        "kind": "class",
        "title": "Kriptografi",
        "starts_at": "2026-11-02T09:00:00Z",
        "ends_at": "2026-11-02T11:00:00Z",
    })
}

async fn insert_event(
    harness: &TestState,
    user_id: Uuid,
    kind: &str,
    title: &str,
    starts_at: &str,
    ends_at: &str,
    source: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into events (id, user_id, kind, title, starts_at, ends_at, source)
         values ($1, $2, $3, $4, $5::timestamptz, $6::timestamptz, $7)",
    )
    .bind(id)
    .bind(user_id)
    .bind(kind)
    .bind(title)
    .bind(starts_at)
    .bind(ends_at)
    .bind(source)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The columns Phase 10 fills are not the client's to write (A198). Each one is its own
/// case, because a validator that checked only `source` would pass a payload that sets
/// `external_id` and silently break the next incremental sync's `(connection_id,
/// external_id)` match.
#[sqlx::test(migrations = "../../migrations")]
async fn a_client_cannot_set_the_columns_the_calendar_sync_owns(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "owned-events@example.test").await;

    for (field, value) in [
        ("source", serde_json::json!("google")),
        ("external_id", serde_json::json!("abc123")),
        ("etag", serde_json::json!("W/\"1\"")),
        ("connection_id", serde_json::json!(Uuid::now_v7())),
    ] {
        let mut request = body();
        request[field] = value;
        let (status, answer) = json(&app, "POST", "/events", &token, request).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field} must be refused: {answer}"
        );
        assert_eq!(answer["code"], "validation", "{field}: {answer}");
    }

    // And the permitted half: `source: "app"` with the other three absent is a normal
    // event, so the refusals above are the calendar's columns and not "source" itself.
    let mut request = body();
    request["source"] = serde_json::json!("app");
    let (status, answer) = json(&app, "POST", "/events", &token, request).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["event"]["source"], "app", "{answer}");

    // Nothing was written by the four refusals.
    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from events where user_id = (select id from users where email = $1)",
    )
    .bind("owned-events@example.test")
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "only the permitted event exists");
}

/// A Google-sourced event is readable and not writable, and the refusal says why.
#[sqlx::test(migrations = "../../migrations")]
async fn an_externally_sourced_event_cannot_be_edited_or_deleted(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "google-event@example.test").await;
    let user_id = user_id_of(&harness, "google-event@example.test").await;
    let event = insert_event(
        &harness,
        user_id,
        "class",
        "Dari Google",
        "2026-11-02T09:00:00Z",
        "2026-11-02T11:00:00Z",
        "google",
    )
    .await;

    let (status, answer) = json(
        &app,
        "GET",
        "/events?from=2026-11-01T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["events"].as_array().unwrap().len(),
        1,
        "an external event is still the student's to read: {answer}"
    );

    let (status, answer) = json(
        &app,
        "PATCH",
        &format!("/events/{event}"),
        &token,
        serde_json::json!({ "title": "Diubah" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "forbidden", "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|message| message.contains("calendar")),
        "the refusal must name the calendar, not read as a permissions bug: {answer}"
    );

    let (status, answer) = json(
        &app,
        "DELETE",
        &format!("/events/{event}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "forbidden", "{answer}");

    // Neither refusal changed the row.
    let title: String = sqlx::query_scalar("select title from events where id = $1")
        .bind(event)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(title, "Dari Google");
    let deleted: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select deleted_at from events where id = $1")
            .bind(event)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        deleted.is_none(),
        "an external event is not deleted by a client"
    );
}

/// `ends_at >= starts_at`; a deadline with equal ends is valid (A199).
#[sqlx::test(migrations = "../../migrations")]
async fn a_deadline_is_an_event_with_equal_ends(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "deadline@example.test").await;

    let (status, answer) = json(
        &app,
        "POST",
        "/events",
        &token,
        serde_json::json!({
            "kind": "deadline",
            "title": "Kumpulkan laporan",
            "starts_at": "2026-11-02T23:59:00Z",
            "ends_at": "2026-11-02T23:59:00Z",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["event"]["kind"], "deadline", "{answer}");

    let (status, answer) = json(
        &app,
        "POST",
        "/events",
        &token,
        serde_json::json!({
            "kind": "deadline",
            "title": "Terbalik",
            "starts_at": "2026-11-02T23:59:00Z",
            "ends_at": "2026-11-02T23:58:00Z",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
}

/// The window is an overlap, not a containment: the class that started yesterday and ends
/// tomorrow is on today's timeline, and a query that required containment would drop it from
/// the Today view with nothing on the page to say why.
#[sqlx::test(migrations = "../../migrations")]
async fn the_window_is_an_overlap(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "window@example.test").await;
    let user_id = user_id_of(&harness, "window@example.test").await;

    // Starts before the window and ends inside it.
    let long = insert_event(
        &harness,
        user_id,
        "study_block",
        "Blok panjang",
        "2026-10-31T20:00:00Z",
        "2026-11-02T10:00:00Z",
        "app",
    )
    .await;
    // Starts inside the window and ends after it.
    let spilling = insert_event(
        &harness,
        user_id,
        "class",
        "Kelas malam",
        "2026-11-02T22:00:00Z",
        "2026-11-03T02:00:00Z",
        "app",
    )
    .await;
    // Entirely inside.
    let inside = insert_event(
        &harness,
        user_id,
        "class",
        "Kriptografi",
        "2026-11-02T09:00:00Z",
        "2026-11-02T11:00:00Z",
        "app",
    )
    .await;
    // Entirely before the window.
    let before = insert_event(
        &harness,
        user_id,
        "exam",
        "Ujian lama",
        "2026-10-01T09:00:00Z",
        "2026-10-01T11:00:00Z",
        "app",
    )
    .await;
    // Touching the window's end exactly: `[from, to)`, so this is outside.
    let at_end = insert_event(
        &harness,
        user_id,
        "class",
        "Mulai tepat di akhir",
        "2026-11-03T00:00:00Z",
        "2026-11-03T01:00:00Z",
        "app",
    )
    .await;

    let (status, answer) = json(
        &app,
        "GET",
        "/events?from=2026-11-02T00:00:00Z&to=2026-11-03T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let ids: Vec<&str> = answer["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["id"].as_str().unwrap())
        .collect();
    for (name, id) in [
        ("the block that started before the window", long),
        ("the class that ends after the window", spilling),
        ("the event inside the window", inside),
    ] {
        assert!(
            ids.contains(&id.to_string().as_str()),
            "{name} is in the window: {answer}"
        );
    }
    for (name, id) in [
        ("the event before the window", before),
        ("the event starting exactly at the window's end", at_end),
    ] {
        assert!(
            !ids.contains(&id.to_string().as_str()),
            "{name} is not in the window: {answer}"
        );
    }

    // The order is `starts_at asc`, which is what a timeline renders.
    let starts: Vec<&str> = answer["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["starts_at"].as_str().unwrap())
        .collect();
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(starts, sorted, "the timeline is ordered: {answer}");
}

/// A `from` the caller omitted is a 400, not a full-table scan of every event the account
/// has ever had.
#[sqlx::test(migrations = "../../migrations")]
async fn a_window_without_a_start_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nowindow@example.test").await;

    let (status, answer) = json(&app, "GET", "/events", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
}

/// A kind outside the five is refused rather than stored as a string the UI cannot render.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_kind_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "kind@example.test").await;

    let mut request = body();
    request["kind"] = serde_json::json!("holiday");
    let (status, answer) = json(&app, "POST", "/events", &token, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");

    // Every documented kind is accepted, so the refusal above is the allowlist and not a
    // check that refuses everything.
    for kind in ["class", "exam", "study_block", "deadline", "external"] {
        let mut request = body();
        request["kind"] = serde_json::json!(kind);
        let (status, answer) = json(&app, "POST", "/events", &token, request).await;
        assert_eq!(status, StatusCode::CREATED, "{kind}: {answer}");
    }
}

/// A task link survives, and deleting the task nulls the event's link rather than deleting
/// the event (`on delete set null`).
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_linked_task_keeps_the_event(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "link@example.test").await;
    let user_id = user_id_of(&harness, "link@example.test").await;

    let task = Uuid::now_v7();
    sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, 'Tugas')")
        .bind(task)
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, answer) = json(
        &app,
        "POST",
        "/events",
        &token,
        serde_json::json!({
            "kind": "study_block",
            "title": "Belajar",
            "starts_at": "2026-11-02T09:00:00Z",
            "ends_at": "2026-11-02T10:00:00Z",
            "task_id": task,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    let event = answer["event"]["id"].as_str().unwrap().to_string();
    assert_eq!(answer["event"]["task_id"], task.to_string(), "{answer}");

    // A tombstone leaves the event readable and its link intact: the task is a row the
    // client has, and nulling the link would be a write on the student's behalf.
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/tasks/{task}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, answer) = json(
        &app,
        "GET",
        "/events?from=2026-11-02T00:00:00Z&to=2026-11-03T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["events"].as_array().unwrap().len(),
        1,
        "deleting the task must not delete the event: {answer}"
    );
    assert_eq!(answer["events"][0]["task_id"], task.to_string(), "{answer}");

    // And the foreign key itself: a real delete — what an account purge runs — nulls the
    // link rather than cascading into the event, which is what `on delete set null` means
    // and what migration 0013's comment claims.
    sqlx::query("delete from tasks where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let link: Option<Uuid> = sqlx::query_scalar("select task_id from events where id = $1")
        .bind(Uuid::parse_str(&event).unwrap())
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert!(
        link.is_none(),
        "the event survives with its link nulled, not deleted"
    );
}
