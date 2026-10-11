//! Credits: the spend's transaction, its daily cap, and cost attribution.
//!
//! These are unit-level facts about one function, so they run against the
//! database directly rather than through a route. The properties under test are
//! the ones a route cannot show: what happens under concurrency, and which row a
//! cost lands on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sqlx::PgPool;
use stvdy_core::config::{Limit, PlanId};
use stvdy_server::credits::{self, DailyCap};
use stvdy_server::testing::TestState;

mod common;
use common::signed_up;

/// A cap that has already been reached must refuse the spend *and* leave the
/// ledger untouched.
///
/// The cap lives inside the spend's transaction for this reason: a separate check
/// and spend can interleave, and a refusal that had already written a ledger row
/// would charge for the message it refused.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reached_cap_refuses_the_spend_and_writes_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cap@example.test").await;
    assert!(!token.is_empty());

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // A cap of zero is already reached, which keeps the test independent of the
    // plan's configured number.
    let cap = DailyCap {
        action: "chat_message",
        limit: Limit::Max(0),
        since: chrono::Utc::now() - chrono::Duration::hours(1),
        label: "chat messages today",
        uncached_only: false,
    };

    let result = credits::spend(
        &harness.state,
        user_id,
        PlanId::Free,
        "chat_message",
        1,
        [Some(cap), None],
    )
    .await;

    let error = result.expect_err("a reached cap must refuse the spend");
    assert_eq!(error.code(), "limit_reached");

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(after, before, "a refused spend must not write a ledger row");

    // And no usage row either, or the next request would count this refusal
    // against the cap.
    let events: i64 = sqlx::query_scalar("select count(*) from usage_events")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(events, 0, "a refused spend must not record usage");
}

/// A cap that has not been reached lets the spend through, and the usage row is
/// what the next call counts.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cap_with_room_lets_the_spend_through(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "room@example.test").await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let cap = DailyCap {
        action: "chat_message",
        limit: Limit::Max(3),
        since: chrono::Utc::now() - chrono::Duration::hours(1),
        label: "chat messages today",
        uncached_only: false,
    };

    let first = credits::spend(
        &harness.state,
        user_id,
        PlanId::Free,
        "chat_message",
        1,
        [Some(cap), None],
    )
    .await
    .expect("the first spend fits");

    // The second call sees the first's usage row, which is the mechanism the cap
    // depends on.
    let used: i64 = sqlx::query_scalar(
        "select count(*)::bigint from usage_events where user_id = $1 and action = $2",
    )
    .bind(user_id)
    .bind("chat_message")
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(used, 1);

    assert_eq!(first.remaining, first_balance(&harness, user_id).await);
}

async fn first_balance(harness: &TestState, user_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// A cost is attached to the row its own request created, not to whichever row
/// happens to be newest.
///
/// Two chats in flight finish in an arbitrary order. Looking up "the newest usage
/// row for this user and action" therefore attributes the second request's cost to
/// the first, and the margin report (A41) is wrong in a way nobody would notice.
#[sqlx::test(migrations = "../../migrations")]
async fn a_provider_cost_lands_on_its_own_usage_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "attribution@example.test").await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    // Two spends, as two concurrent chats would make.
    let first = credits::spend(
        &harness.state,
        user_id,
        PlanId::Free,
        "chat_message",
        1,
        [None, None],
    )
    .await
    .expect("the first spend");
    let second = credits::spend(
        &harness.state,
        user_id,
        PlanId::Free,
        "chat_message",
        1,
        [None, None],
    )
    .await
    .expect("the second spend");
    assert_ne!(first.usage_event_id, second.usage_event_id);

    // The *first* request finishes last and records its cost. With a
    // newest-row lookup that cost would land on the second row.
    credits::record_provider_usage(
        &harness.state.pool,
        first.usage_event_id,
        111,
        22,
        Some(0.00119),
        "base.v1",
    )
    .await
    .expect("recording usage");

    let rows: Vec<(uuid::Uuid, Option<i32>, Option<f64>)> = sqlx::query_as(
        "select id, tokens_in, provider_cost_usd::float8 from usage_events order by created_at",
    )
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();

    let first_row = rows
        .iter()
        .find(|(id, _, _)| *id == first.usage_event_id)
        .expect("the first row exists");
    let second_row = rows
        .iter()
        .find(|(id, _, _)| *id == second.usage_event_id)
        .expect("the second row exists");

    assert_eq!(
        first_row.1,
        Some(111),
        "the cost belongs to its own request"
    );
    assert_eq!(first_row.2, Some(0.00119));
    assert_eq!(
        second_row.1, None,
        "the other request's row must be untouched"
    );
    assert_eq!(second_row.2, None);
}

/// The monthly bucket is spent before the purchased one (A9): purchased credits
/// never expire, so spending them first would waste the grant that does.
#[sqlx::test(migrations = "../../migrations")]
async fn the_monthly_bucket_is_spent_before_the_purchased_one(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "buckets@example.test").await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    // Empty the monthly bucket exactly (a larger negative would drive it below
    // zero and make the whole balance negative, which is a different case), then
    // fund the purchased one.
    let grant = harness
        .state
        .config
        .plans
        .get(PlanId::Free)
        .credits_per_month;
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason)
         values ($1, $2, 'monthly', 'test'), ($1, 50, 'purchased', 'credit_pack')",
    )
    .bind(user_id)
    .bind(-grant)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // The monthly bucket is empty, so this must come out of the purchased one.
    credits::spend(
        &harness.state,
        user_id,
        PlanId::Free,
        "chat_message",
        3,
        [None, None],
    )
    .await
    .expect("the purchased bucket covers it");

    let purchased: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = 'purchased'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(purchased, 47);

    // With both buckets funded, the monthly one pays.
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason)
         values ($1, 100, 'monthly', 'monthly_grant')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    credits::spend(
        &harness.state,
        user_id,
        PlanId::Free,
        "chat_message",
        2,
        [None, None],
    )
    .await
    .expect("the monthly bucket covers it");

    let monthly: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = 'monthly'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        monthly, 98,
        "the monthly bucket paid, not the purchased one"
    );
    let purchased: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = 'purchased'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(purchased, 47, "the purchased bucket is untouched");
}

/// The spend takes the per-user advisory lock **before** it reads the ledger.
///
/// Asserted by holding the same lock on another connection and watching the spend
/// block, then releasing it. A timing-based test ("run two spends at once and
/// expect one to fail") does not work here: on a fast local Postgres the two
/// transactions usually serialize by luck, so it passes even with the lock
/// removed, which makes it worse than no test. This one fails deterministically
/// the moment the lock is not taken.
#[sqlx::test(migrations = "../../migrations")]
async fn the_spend_takes_the_lock_before_reading_the_ledger(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "locked@example.test").await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    // Hold the same lock on a separate connection. `pg_advisory_xact_lock` is
    // transaction-scoped, so this needs an explicit transaction rather than a bare
    // statement on a pooled connection.
    let mut holder = harness
        .state
        .pool
        .begin()
        .await
        .expect("a second connection");
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user_id.to_string())
        .execute(&mut *holder)
        .await
        .expect("the holder takes the lock");

    let state = harness.state.clone();
    let spending = tokio::spawn(async move {
        credits::spend(
            &state,
            user_id,
            PlanId::Free,
            "chat_message",
            1,
            [None, None],
        )
        .await
    });

    // While the lock is held the spend must not complete, and it must not have
    // written anything either: the point of taking the lock first is that the
    // ledger is not read until it is held.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(400), async {
            while !spending.is_finished() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .is_err(),
        "the spend completed while another connection held the lock, so it did not \
         take the lock before reading the ledger"
    );

    let written: i64 =
        sqlx::query_scalar("select count(*) from credit_ledger where user_id = $1 and delta < 0")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(written, 0, "nothing may be written while the lock is held");

    // Releasing the holder lets the spend through.
    holder.rollback().await.expect("release the lock");
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), spending)
        .await
        .expect("the spend proceeds once the lock is free")
        .expect("the task did not panic");
    assert!(result.is_ok(), "the spend must succeed: {result:?}");
}
