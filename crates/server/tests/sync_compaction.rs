//! `compact_change_log`: the two retention windows, the watermark they write, and the 410
//! that watermark makes answerable.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::jobs::handlers::compact::{Retention, compact_user};
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

const RETENTION: Retention = Retention {
    upsert_days: 90,
    tombstone_days: 180,
};

async fn account(app: &axum::Router, harness: &TestState, email: &str) -> (String, Uuid) {
    let token = signed_up(app, harness, email).await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the user row");
    (token, user_id)
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

async fn seq_of(pool: &PgPool, entity_id: Uuid) -> i64 {
    sqlx::query_scalar("select seq from change_log where entity_id = $1")
        .bind(entity_id)
        .fetch_one(pool)
        .await
        .expect("the row's seq")
}

async fn count_changes(pool: &PgPool, user_id: Uuid) -> i64 {
    sqlx::query_scalar("select count(*) from change_log where user_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("the change count")
}

async fn age(pool: &PgPool, user_id: Uuid, op: &str, days: i32) {
    sqlx::query(
        "update change_log set changed_at = now() - make_interval(days => $3)
          where user_id = $1 and op = $2",
    )
    .bind(user_id)
    .bind(op)
    .bind(days)
    .execute(pool)
    .await
    .expect("age the changes");
}

async fn watermark(pool: &PgPool, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "select coalesce(
                  (select min_retained_seq from change_log_watermark where user_id = $1), 0)",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("the watermark")
}

/// Upserts older than the retention window go; tombstones younger than the tombstone
/// window stay, because a device that has been offline for three months must be told
/// that a row was removed rather than silently keep it (A184).
#[sqlx::test(migrations = "../../migrations")]
async fn compaction_keeps_tombstones_longer_than_it_keeps_upserts(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (_, user_id) = account(&app, &harness, "windows@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Lama").await;
    sqlx::query("update tasks set deleted_at = now() where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    // Both changes are 100 days old: past the upsert window, inside the tombstone window.
    age(&harness.state.pool, user_id, "upsert", 100).await;
    age(&harness.state.pool, user_id, "delete", 100).await;

    let mut conn = harness.state.pool.acquire().await.unwrap();
    let written = compact_user(&mut conn, user_id, RETENTION).await.unwrap();
    assert!(
        written.is_some(),
        "something was removed, so a watermark is written"
    );

    let upserts: i64 =
        sqlx::query_scalar("select count(*) from change_log where user_id = $1 and op = 'upsert'")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let deletes: i64 =
        sqlx::query_scalar("select count(*) from change_log where user_id = $1 and op = 'delete'")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(upserts, 0, "an old upsert is redundant and goes");
    assert_eq!(
        deletes, 1,
        "an old tombstone stays: a device may still need it"
    );
}

/// The watermark is the first seq that survived, so a cursor at or above it is still
/// servable and a cursor below it is 410 (A165).
#[sqlx::test(migrations = "../../migrations")]
async fn the_watermark_is_the_first_seq_that_survived(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (_, user_id) = account(&app, &harness, "watermark@example.test").await;

    let first = insert_task(&harness.state.pool, user_id, "Satu").await;
    let second = insert_task(&harness.state.pool, user_id, "Dua").await;
    let third = insert_task(&harness.state.pool, user_id, "Tiga").await;

    let old_first = seq_of(&harness.state.pool, first).await;
    let surviving = seq_of(&harness.state.pool, third).await;
    assert!(old_first < surviving && seq_of(&harness.state.pool, second).await < surviving);

    // Age only the first two.
    sqlx::query(
        "update change_log set changed_at = now() - make_interval(days => 100)
          where user_id = $1 and entity_id = any($2)",
    )
    .bind(user_id)
    .bind(vec![first, second])
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let mut conn = harness.state.pool.acquire().await.unwrap();
    let written = compact_user(&mut conn, user_id, RETENTION).await.unwrap();
    assert_eq!(
        written,
        Some(surviving),
        "the watermark is the first seq that survived"
    );
    assert_eq!(watermark(&harness.state.pool, user_id).await, surviving);
}

/// An account whose history is entirely inside the window is untouched, and its
/// watermark stays 0 — which is what keeps a new device's first pull from being 410.
#[sqlx::test(migrations = "../../migrations")]
async fn an_account_inside_the_window_is_untouched(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (_, user_id) = account(&app, &harness, "recent@example.test").await;
    insert_task(&harness.state.pool, user_id, "Baru").await;

    let before = count_changes(&harness.state.pool, user_id).await;

    let mut conn = harness.state.pool.acquire().await.unwrap();
    let written = compact_user(&mut conn, user_id, RETENTION).await.unwrap();
    assert_eq!(written, None, "nothing was old enough to remove");

    assert_eq!(count_changes(&harness.state.pool, user_id).await, before);
    assert_eq!(watermark(&harness.state.pool, user_id).await, 0);
}

/// The job is a singleton: two enqueues in one day produce one row, because the unique
/// index on `jobs.dedupe_key` is the mechanism the roadmap names.
#[sqlx::test(migrations = "../../migrations")]
async fn the_job_is_a_singleton_per_day(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    stvdy_server::jobs::schedule::enqueue_compaction(&harness.state)
        .await
        .unwrap();
    stvdy_server::jobs::schedule::enqueue_compaction(&harness.state)
        .await
        .unwrap();

    let jobs: i64 =
        sqlx::query_scalar("select count(*) from jobs where kind = 'compact_change_log'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(jobs, 1, "a second enqueue in one day must be a no-op");
}

/// A cursor below the watermark gets 410 through the route, and a client that resets
/// to 0 then receives the whole surviving history.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_cursor_is_told_to_resync_and_cursor_zero_always_works(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id) = account(&app, &harness, "stale-cursor@example.test").await;

    let first = insert_task(&harness.state.pool, user_id, "Satu").await;
    let second = insert_task(&harness.state.pool, user_id, "Dua").await;
    let third = insert_task(&harness.state.pool, user_id, "Tiga").await;
    let stale = seq_of(&harness.state.pool, first).await;

    sqlx::query(
        "update change_log set changed_at = now() - make_interval(days => 100)
          where user_id = $1 and entity_id = any($2)",
    )
    .bind(user_id)
    .bind(vec![first, second])
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let mut conn = harness.state.pool.acquire().await.unwrap();
    compact_user(&mut conn, user_id, RETENTION).await.unwrap();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/sync/pull?cursor={stale}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["code"], "resync_required", "{body}");

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["id"] == third.to_string()),
        "the surviving history must arrive from zero: {body}"
    );
}

/// Another account's rows are not touched by this account's compaction, and one
/// account's watermark does not 410 another's cursor.
#[sqlx::test(migrations = "../../migrations")]
async fn compaction_is_scoped_to_one_account(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (_, a_user) = account(&app, &harness, "compact-a@example.test").await;
    let (_, b_user) = account(&app, &harness, "compact-b@example.test").await;

    insert_task(&harness.state.pool, a_user, "A").await;
    insert_task(&harness.state.pool, b_user, "B").await;
    age(&harness.state.pool, a_user, "upsert", 100).await;
    age(&harness.state.pool, b_user, "upsert", 100).await;

    let mut conn = harness.state.pool.acquire().await.unwrap();
    compact_user(&mut conn, a_user, RETENTION).await.unwrap();

    assert_eq!(count_changes(&harness.state.pool, a_user).await, 0);
    assert_eq!(
        count_changes(&harness.state.pool, b_user).await,
        1,
        "another account's history must be untouched"
    );
    assert_eq!(
        watermark(&harness.state.pool, b_user).await,
        0,
        "another account's watermark must not move"
    );
}
