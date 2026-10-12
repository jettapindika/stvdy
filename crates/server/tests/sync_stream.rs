//! `GET /sync/stream`: the realtime hint.
//!
//! The stream never ends on its own, so the tests read frames off the live body rather
//! than buffering it with `oneshot` (which would wait forever). Comment heartbeats are
//! skipped by the frame parser, so a test can assert that *no* `seq` event arrived.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use tower::ServiceExt;
use uuid::Uuid;

mod common;

async fn account(app: &axum::Router, harness: &TestState, email: &str) -> (String, Uuid, Uuid) {
    let token = common::signed_up(app, harness, email).await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the user row");
    let device_id: Uuid = sqlx::query_scalar("select id from devices where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the device row");
    (token, user_id, device_id)
}

async fn insert_task(pool: &PgPool, user_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, $3)")
        .bind(id)
        .bind(user_id)
        .bind(title)
        .execute(pool)
        .await
        .expect("insert task");
    id
}

async fn open(app: &axum::Router, token: &str) -> BodyDataStream {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/sync/stream")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().into_data_stream()
}

/// Reads until `want` named events have arrived or `within` elapses.
///
/// A comment heartbeat has no `event:` line and is skipped, so a caller waiting for a
/// `seq` event is not woken by a keep-alive.
async fn read_events<S>(
    stream: &mut S,
    want: usize,
    within: Duration,
) -> Vec<(String, serde_json::Value)>
where
    S: futures::Stream<Item = Result<bytes::Bytes, axum::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + within;
    let mut buffer = String::new();
    let mut events = Vec::new();
    while events.len() < want {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => buffer.push_str(&String::from_utf8_lossy(&chunk)),
            Ok(Some(Err(error))) => panic!("the stream failed: {error}"),
            Ok(None) | Err(_) => break,
        }
        while let Some(index) = buffer.find("\n\n") {
            let frame = buffer[..index].to_string();
            buffer.drain(..index + 2);
            if let Some(event) = parse_frame(&frame) {
                events.push(event);
            }
        }
    }
    events
}

fn parse_frame(frame: &str) -> Option<(String, serde_json::Value)> {
    let mut name = None;
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim());
        }
    }
    let name = name?;
    Some((
        name,
        serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
    ))
}

/// The hint's first frame is the current `max(seq)`, so a device that reconnects after
/// a gap pulls at once instead of waiting for the next change (A181).
#[sqlx::test(migrations = "../../migrations")]
async fn the_stream_opens_with_the_current_seq(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "stream-open@example.test").await;

    insert_task(&harness.state.pool, user_id, "Tugas").await;
    let expected: i64 =
        sqlx::query_scalar("select coalesce(max(seq), 0) from change_log where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    let mut stream = open(&app, &token).await;
    let events = read_events(&mut stream, 1, Duration::from_secs(3)).await;
    assert_eq!(
        events.len(),
        1,
        "the stream must open with a frame: {events:?}"
    );
    assert_eq!(events[0].0, "seq");
    assert_eq!(events[0].1["seq"].as_i64(), Some(expected));
}

/// A change committed after the connection opens reaches the stream within the poll
/// interval, as `event: seq`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_later_change_is_emitted_as_a_seq_event(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "stream-later@example.test").await;

    let mut stream = open(&app, &token).await;
    let opening = read_events(&mut stream, 1, Duration::from_secs(3)).await;
    let opened_at = opening[0].1["seq"].as_i64().unwrap();

    insert_task(&harness.state.pool, user_id, "Tugas baru").await;

    let events = read_events(&mut stream, 1, Duration::from_secs(6)).await;
    assert_eq!(
        events.len(),
        1,
        "the new change must reach the stream: {events:?}"
    );
    assert_eq!(events[0].0, "seq");
    assert!(
        events[0].1["seq"].as_i64().unwrap() > opened_at,
        "the emitted seq must be the newer one: {events:?}"
    );
}

/// The hint carries the caller's own clock and nothing else: another account's change
/// does not move this stream.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_change_does_not_move_this_stream(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (a_token, _, _) = account(&app, &harness, "stream-a@example.test").await;
    let (_, b_user, _) = account(&app, &harness, "stream-b@example.test").await;

    let mut stream = open(&app, &a_token).await;
    let opening = read_events(&mut stream, 1, Duration::from_secs(3)).await;
    assert_eq!(opening.len(), 1, "{opening:?}");

    // B writes; A's stream must not emit a seq event for it. Two poll intervals is enough
    // to prove the point without waiting on the heartbeat.
    insert_task(&harness.state.pool, b_user, "B's task").await;
    let events = read_events(&mut stream, 1, Duration::from_secs(5)).await;
    assert!(
        events.is_empty(),
        "another account's change must not move this stream: {events:?}"
    );
}
