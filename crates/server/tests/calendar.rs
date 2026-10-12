//! The calendar's four surfaces: the feed out, the ICS import, the consent flow and
//! the two-way sync. Split by surface, not by module, because each has a failure the
//! others cannot produce: a feed a client refuses, a private host reached, a token in a
//! deep link, a sync that stopped being incremental.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use tower::ServiceExt;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// A request whose response headers and raw bytes are both read.
///
/// `common::json` parses the body and drops the headers, and the feed's contract *is* its
/// headers (`text/calendar`, `no-store`, `noindex`): a helper that only returned the body
/// would make the interesting half of the route unassertable.
async fn raw(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");

    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// One `events` row, written directly: the feed's job is to select rows, and going through
/// `POST /events` for each one would make this suite depend on Phase 9's validator.
#[allow(clippy::too_many_arguments)]
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

/// The feed validates, carries the caller's own rows, and a regenerated token retires the old
/// one. The last assertion is the one that matters: a revoked token that still works is not
/// revoked.
#[sqlx::test(migrations = "../../migrations")]
async fn the_feed_validates_and_a_regenerated_token_stops_working(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "feed@example.test").await;
    let user_id = user_id_of(&harness, "feed@example.test").await;

    // One of each kind the feed carries, one kind it does not, and one row that is not the
    // student's own (A241).
    insert_event(
        &harness,
        user_id,
        "exam",
        "UAS Jaringan Komputer",
        "2026-06-10T01:00:00Z",
        "2026-06-10T03:00:00Z",
        "app",
    )
    .await;
    insert_event(
        &harness,
        user_id,
        "study_block",
        "Blok belajar",
        "2026-06-11T09:00:00Z",
        "2026-06-11T10:00:00Z",
        "app",
    )
    .await;
    insert_event(
        &harness,
        user_id,
        "external",
        "Kuliah umum",
        "2026-06-12T09:00:00Z",
        "2026-06-12T10:00:00Z",
        "app",
    )
    .await;
    // A Google row: echoing it back would show the student's own Google event twice.
    insert_event(
        &harness,
        user_id,
        "class",
        "Rapat himpunan",
        "2026-06-13T09:00:00Z",
        "2026-06-13T10:00:00Z",
        "google",
    )
    .await;
    // A task with a due date, which the feed renders as an all-day deadline (A248).
    sqlx::query(
        "insert into tasks (id, user_id, title, due_at) values ($1, $2, $3, $4::timestamptz)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind("Kumpulkan laporan")
    .bind("2026-06-14T17:00:00Z")
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // The token does not exist until it is generated, and reading it says so rather than 404ing.
    let (status, body) = json(
        &app,
        "GET",
        "/calendar/ics-token",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["ics_feed_token"].is_null(), "{body}");

    let (status, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let first_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    assert!(
        first_token.len() >= 40,
        "a token must be long enough to be unguessable"
    );

    // Reading it back is the same value: the screen shows the link, it does not rotate it.
    let (_, read_back) = json(
        &app,
        "GET",
        "/calendar/ics-token",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(read_back["ics_feed_token"], first_token);

    let (status, headers, body) = raw(&app, "GET", &format!("/cal/{first_token}.ics"), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["content-type"], "text/calendar; charset=utf-8");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-robots-tag"], "noindex");
    assert!(body.contains("BEGIN:VCALENDAR"), "{body}");
    assert!(body.contains("UAS Jaringan Komputer"), "{body}");
    assert!(body.contains("Blok belajar"), "{body}");
    assert!(body.contains("Kumpulkan laporan"), "{body}");
    // `kind = 'external'` and every `source != 'app'` row are excluded (A241).
    assert!(!body.contains("Kuliah umum"), "{body}");
    assert!(!body.contains("Rapat himpunan"), "{body}");
    // The task's due date is an all-day deadline in the user's zone, not a timed event.
    assert!(body.contains("DTSTART;VALUE=DATE:20260615"), "{body}");
    // And the exam, which is an instant, is UTC.
    assert!(body.contains("DTSTART:20260610T010000Z"), "{body}");

    // Another account's token resolves to that account's rows and never this one's.
    let other = signed_up(&app, &harness, "other-feed@example.test").await;
    let (_, other_rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &other,
        serde_json::json!({}),
    )
    .await;
    let other_token = other_rotated["ics_feed_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(other_token, first_token);
    let (status, _, other_body) = raw(&app, "GET", &format!("/cal/{other_token}.ics"), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !other_body.contains("UAS Jaringan Komputer"),
        "{other_body}"
    );
    assert!(!other_body.contains("Kumpulkan laporan"), "{other_body}");

    // Regenerate retires the old token: this is the whole remedy A242 promises.
    let (_, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    let new_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    assert_ne!(new_token, first_token);
    assert_eq!(
        raw(&app, "GET", &format!("/cal/{first_token}.ics"), "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        raw(&app, "GET", &format!("/cal/{new_token}.ics"), "")
            .await
            .0,
        StatusCode::OK
    );

    // A token that is not a token is 404, not 500: the lookup is an index hit or a miss.
    assert_eq!(
        raw(&app, "GET", "/cal/nope.ics", "").await.0,
        StatusCode::NOT_FOUND
    );
    // And the documented suffix is required: one token has exactly one URL.
    assert_eq!(
        raw(&app, "GET", &format!("/cal/{new_token}"), "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        raw(&app, "GET", "/cal/.ics", "").await.0,
        StatusCode::NOT_FOUND
    );
}

/// The feed is a cache the client owns (A230), so two renders of an unchanged row must be
/// byte-identical. A `DTSTAMP` written from a clock reading, or a `UID` regenerated per
/// request, would make every poll a new document and every client re-import it — with no
/// error on either side.
#[sqlx::test(migrations = "../../migrations")]
async fn two_renders_of_an_unchanged_feed_are_byte_identical(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "stable-feed@example.test").await;
    let user_id = user_id_of(&harness, "stable-feed@example.test").await;

    let event = insert_event(
        &harness,
        user_id,
        "class",
        "Kriptografi",
        "2026-06-10T01:00:00Z",
        "2026-06-10T03:00:00Z",
        "app",
    )
    .await;
    let (_, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    let feed_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    let uri = format!("/cal/{feed_token}.ics");

    let (_, _, first) = raw(&app, "GET", &uri, "").await;
    let (_, _, second) = raw(&app, "GET", &uri, "").await;
    assert_eq!(first, second, "an unchanged feed must be byte-identical");
    assert!(first.contains(&format!("UID:{event}@stvdy.app")), "{first}");
    // A new row has never been edited, so `SEQUENCE:0` (A230).
    assert_eq!(sequence_of(&first), 0, "{first}");

    // An edit raises it. `updated_at` is touched by the trigger, so this is the same path a
    // real edit takes.
    sqlx::query("update events set title = $2 where id = $1")
        .bind(event)
        .bind("Kriptografi (revisi)")
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (_, _, edited) = raw(&app, "GET", &uri, "").await;
    assert_ne!(edited, first);
    assert!(edited.contains("Kriptografi (revisi)"), "{edited}");
    assert!(
        sequence_of(&edited) >= 1,
        "an edit must move SEQUENCE off zero: {edited}"
    );
}

/// The `SEQUENCE` value the feed carries, parsed rather than pattern-matched: the assertion
/// is that an edit *raised* it, and `!contains("SEQUENCE:0")` would also pass on a feed that
/// rendered a different integer for an unrelated reason.
fn sequence_of(ics: &str) -> i64 {
    ics.split("\r\n")
        .find_map(|line| line.strip_prefix("SEQUENCE:"))
        .expect("a SEQUENCE line")
        .trim()
        .parse()
        .expect("SEQUENCE is an integer")
}
