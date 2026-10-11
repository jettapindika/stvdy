//! The period: local boundaries, the grant that cannot double, grace, and read-only.
//!
//! Every case here is about *time*, so every case pins the clock: "the counter rolls at
//! the user's own midnight" is a statement about an instant, and a test that ran at the
//! ambient time would pass for eleven months out of twelve and fail in December.
//!
//! The read-only half is in this file too because it is the same event: a downgrade is
//! what puts rows over a limit, and the rollover is what notices.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use stvdy_core::config::PlanId;
use stvdy_core::entitlements::Feature;
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::jobs::ClaimedJob;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::signed_up;

/// 2026-10-31T18:00:00Z, the instant the plan's counter test turns on.
///
/// Jakarta (UTC+7) entered November at 17:00Z; UTC does not enter it until 00:00Z on the
/// 1st. So at this instant the two users are in *different* months, which is what a
/// counter reset done with `date_trunc('month', now())` cannot see.
const BETWEEN_THE_BOUNDARIES: &str = "2026-10-31T18:00:00Z";

fn instant(raw: &str) -> DateTime<Utc> {
    raw.parse().expect("a valid timestamp")
}

fn router(harness: &TestState) -> axum::Router {
    stvdy_server::routes::router(harness.state.clone()).expect("the router builds")
}

async fn user_id(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the account exists")
}

async fn bucket(harness: &TestState, user_id: Uuid, bucket: &str) -> i64 {
    sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = $2",
    )
    .bind(user_id)
    .bind(bucket)
    .fetch_one(&harness.state.pool)
    .await
    .expect("the bucket is readable")
}

async fn ledger_rows(harness: &TestState, user_id: Uuid, reason: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*)::bigint from credit_ledger where user_id = $1 and reason = $2",
    )
    .bind(user_id)
    .bind(reason)
    .fetch_one(&harness.state.pool)
    .await
    .expect("the ledger is readable")
}

/// Gives the account `count` flashcards created at `at`, through a deck in a workspace.
///
/// The monthly counter is `flashcards + the cards in-flight decks have reserved`, so a
/// test that wrote only `flashcards` rows would be testing half the expression the
/// snapshot reads.
async fn seed_flashcards(harness: &TestState, user_id: Uuid, count: i64, at: DateTime<Utc>) {
    sqlx::query(
        "with w as (
           insert into workspaces (user_id, name, created_at)
           values ($1, 'period', $2) returning id),
         d as (
           insert into decks (user_id, workspace_id, title, source_ids, created_at)
           select $1, w.id, 'period', '{}'::uuid[], $2 from w returning id)
         insert into flashcards (user_id, deck_id, front, back, created_at)
         select $1, d.id, 'front ' || g, 'back ' || g, $2
           from d, generate_series(1, $3) as g",
    )
    .bind(user_id)
    .bind(at)
    .bind(count)
    .execute(&harness.state.pool)
    .await
    .expect("seeding flashcards");
}

/// A claim-shaped job for `kind`, as the worker would hand it to a handler.
fn job(kind: &str, user_id: Uuid) -> ClaimedJob {
    ClaimedJob {
        id: Uuid::now_v7(),
        kind: kind.to_string(),
        payload: json!({ "user_id": user_id }),
        attempts: 1,
    }
}

/// The counters roll on the user's own local boundary, not UTC.
///
/// `Asia/Jakarta` is UTC+7, so at [`BETWEEN_THE_BOUNDARIES`] the Jakarta month is already
/// November while UTC's is still October. Two accounts with the *same* rows from early
/// October therefore see different months: Jakarta's counter has rolled (the new month's
/// allowance is available), UTC's has not (the same rows still count against it).
///
/// This is the test that catches a counter reset done with `date_trunc('month', now())` —
/// which would reset both accounts — and a fixed offset applied to a zone that is not
/// UTC, which would reset the UTC account early.
#[sqlx::test(migrations = "../../migrations")]
async fn the_monthly_counter_rolls_at_the_users_own_midnight(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(Arc::new(FixedClock::at(instant(BETWEEN_THE_BOUNDARIES))));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);

    signed_up(&app, &harness, "jakarta@example.test").await;
    signed_up(&app, &harness, "utc@example.test").await;
    // Signup writes `Asia/Jakarta`; the second account is pinned to UTC so the two
    // accounts differ only in their zone.
    sqlx::query("update users set timezone = 'UTC' where email = 'utc@example.test'")
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let jakarta = user_id(&harness, "jakarta@example.test").await;
    let utc = user_id(&harness, "utc@example.test").await;
    // Early October, which is the current month for *both* accounts until Jakarta's
    // boundary passes at 2026-10-31T17:00Z.
    let at = instant("2026-10-05T00:00:00Z");

    // The free plan allows thirty cards a month, so thirty cards is exactly a full month.
    let allowance: i64 = harness
        .state
        .config
        .plans
        .get(PlanId::Free)
        .flashcards_per_month
        .max()
        .expect("free's card limit is a number") as i64;
    seed_flashcards(&harness, jakarta, allowance, at).await;
    seed_flashcards(&harness, utc, allowance, at).await;

    let jakarta_snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, jakarta)
        .await
        .expect("the snapshot is readable");
    assert_eq!(
        jakarta_snapshot.flashcards_this_month, 0,
        "Jakarta entered November at 17:00Z, so October's cards are in the past month"
    );
    let utc_snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, utc)
        .await
        .expect("the snapshot is readable");
    assert_eq!(
        utc_snapshot.flashcards_this_month, allowance,
        "UTC is still in October, so the same rows still count against it"
    );

    // And the decision the counter feeds: the Jakarta account gets the new month's
    // allowance, the UTC account does not yet.
    stvdy_server::entitlements::require_for(
        &harness.state,
        jakarta,
        Feature::FlashcardBatch { n: 10 },
    )
    .await
    .expect("Jakarta's month has rolled, so it has the new allowance");
    let refused = stvdy_server::entitlements::require_for(
        &harness.state,
        utc,
        Feature::FlashcardBatch { n: 10 },
    )
    .await
    .expect_err("a full month refuses the next batch");
    assert_eq!(refused.code(), "limit_reached");
}

/// The grant is once per period, whatever runs it.
///
/// Signup grants; the `monthly_grant` job then runs for the same local month. One ledger
/// row, not two (A152) — the double-grant the roadmap's own job note warns about, and one
/// that is reachable today because the signup grant already exists (A64).
///
/// The second half is what makes the guard a *period* guard rather than a permanent one:
/// in the next local month the same job grants, and the unspent balance from the previous
/// period expires first.
#[sqlx::test(migrations = "../../migrations")]
async fn the_signup_grant_and_the_job_cannot_double_grant(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "once@example.test").await;

    let user_id = user_id(&harness, "once@example.test").await;
    let allowance = harness
        .state
        .config
        .plans
        .get(PlanId::Free)
        .credits_per_month;
    assert_eq!(bucket(&harness, user_id, "monthly").await, allowance);
    assert_eq!(ledger_rows(&harness, user_id, "monthly_grant").await, 1);

    stvdy_server::jobs::handlers::grants::run(&harness.state, &job("monthly_grant", user_id))
        .await
        .expect("the job runs");

    assert_eq!(
        ledger_rows(&harness, user_id, "monthly_grant").await,
        1,
        "the job and the signup grant share a period_key, so the second is a no-op"
    );
    assert_eq!(bucket(&harness, user_id, "monthly").await, allowance);

    // Now the same job in the account's *next* local month.
    let next = stvdy_server::util::next_local_month_start("Asia/Jakarta", harness.clock.as_ref());
    let harness = harness.with_clock(Arc::new(FixedClock::at(next + Duration::hours(1))));

    stvdy_server::jobs::handlers::grants::run(&harness.state, &job("monthly_grant", user_id))
        .await
        .expect("the job runs in the new period");

    assert_eq!(
        ledger_rows(&harness, user_id, "monthly_grant").await,
        2,
        "a new period grants again: the guard is per period, not permanent"
    );
    assert_eq!(
        ledger_rows(&harness, user_id, "expire").await,
        1,
        "the previous period's unspent monthly balance expires at the boundary"
    );
    assert_eq!(bucket(&harness, user_id, "monthly").await, allowance);
}

/// A month's unspent monthly credits do not roll over (A153), and purchased ones do.
///
/// The balance alone cannot show this — the expiry and the new grant are the same size —
/// so the assertion is the ledger: a negative `expire` row for the old balance, and the
/// purchased bucket untouched.
#[sqlx::test(migrations = "../../migrations")]
async fn unspent_monthly_credits_expire_and_purchased_ones_do_not(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "rollover@example.test").await;

    let user_id = user_id(&harness, "rollover@example.test").await;
    let allowance = harness
        .state
        .config
        .plans
        .get(PlanId::Free)
        .credits_per_month;
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason)
         values ($1, 50, 'purchased', 'credit_pack')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let next = stvdy_server::util::next_local_month_start("Asia/Jakarta", harness.clock.as_ref());
    let harness = harness.with_clock(Arc::new(FixedClock::at(next + Duration::hours(1))));
    stvdy_server::jobs::handlers::grants::run(&harness.state, &job("monthly_grant", user_id))
        .await
        .expect("the job runs");

    let expired: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and reason = 'expire'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        expired, -allowance,
        "the whole unspent monthly balance expires at the boundary"
    );
    assert_eq!(bucket(&harness, user_id, "monthly").await, allowance);
    assert_eq!(
        bucket(&harness, user_id, "purchased").await,
        50,
        "purchased credits never expire"
    );

    // The expiry is once per period, like the grant: a second run cannot take the balance
    // twice.
    stvdy_server::jobs::handlers::grants::run(&harness.state, &job("monthly_grant", user_id))
        .await
        .expect("the job runs again");
    assert_eq!(ledger_rows(&harness, user_id, "expire").await, 1);
    assert_eq!(bucket(&harness, user_id, "monthly").await, allowance);
}

/// The rollover expires a lapsed grace window, recomputes the read-only flags, and
/// enqueues the next period's grant.
///
/// `reset_counters` does not zero a counter — counters are derived (A147). What it
/// changes is the subscription's status, and therefore the effective plan the read-only
/// flags come from.
#[sqlx::test(migrations = "../../migrations")]
async fn the_rollover_job_expires_a_lapsed_subscription_and_flags_the_over_limit_rows(
    pool: PgPool,
) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "lapsed@example.test").await;

    let user_id = user_id(&harness, "lapsed@example.test").await;
    // A power account whose grace window lapsed ten days ago, with three workspaces: the
    // free plan allows two, so one is over the limit.
    sqlx::query("update users set plan_id = 'power' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
           (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'power', 'grace', 'monthly', now() - interval '40 days', now() - interval '10 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    for (index, name) in ["oldest", "middle", "newest"].iter().enumerate() {
        sqlx::query(
            "insert into workspaces (user_id, name, created_at)
             values ($1, $2, now() - make_interval(days => $3))",
        )
        .bind(user_id)
        .bind(name)
        .bind(2 - index as i32)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    // A power account is over no limit, so nothing is read-only before the rollover.
    let before: i64 = sqlx::query_scalar(
        "select count(*)::bigint from workspaces where user_id = $1 and read_only",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(before, 0);

    stvdy_server::jobs::handlers::counters::run(&harness.state, &job("reset_counters", user_id))
        .await
        .expect("the rollover runs");

    let status: String = sqlx::query_scalar("select status from subscriptions where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        status, "expired",
        "a grace window past current_period_end + GRACE_PERIOD_DAYS is expired"
    );

    let read_only: Vec<(String, bool)> = sqlx::query_as(
        "select name, read_only from workspaces where user_id = $1 order by created_at",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        read_only,
        vec![
            ("oldest".to_string(), false),
            ("middle".to_string(), false),
            ("newest".to_string(), true),
        ],
        "the oldest stay writable and the newest is flagged, which is the reading a user \
         expects when they are told to delete something"
    );

    // The next period's grant is enqueued, with the same key and run time the scheduler
    // would have used.
    let queued: i64 = sqlx::query_scalar(
        "select count(*)::bigint from jobs
          where kind = 'monthly_grant' and payload->>'user_id' = $1",
    )
    .bind(user_id.to_string())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(queued, 1);
}

/// The scheduler enqueues once per account per local month, and a restart does not double
/// it: the `dedupe_key` is the month.
///
/// `run_at` is the account's *own* next local month start, computed through `chrono-tz`
/// because Postgres cannot do IANA arithmetic on a text timezone (A6).
#[sqlx::test(migrations = "../../migrations")]
async fn the_scheduler_enqueues_one_grant_per_user_per_local_month(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "schedule-jakarta@example.test").await;
    signed_up(&app, &harness, "schedule-utc@example.test").await;
    sqlx::query("update users set timezone = 'UTC' where email = 'schedule-utc@example.test'")
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let jakarta = user_id(&harness, "schedule-jakarta@example.test").await;
    let utc = user_id(&harness, "schedule-utc@example.test").await;

    let scheduled = stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the pass runs");
    assert_eq!(scheduled, 2, "one account, one boundary");

    let rows: Vec<(String, String, DateTime<Utc>)> = sqlx::query_as(
        "select kind, dedupe_key, run_at from jobs
          where dedupe_key like 'monthly_grant:%' order by dedupe_key",
    )
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "one grant per account");

    for (account, timezone) in [(jakarta, "Asia/Jakarta"), (utc, "UTC")] {
        let run_at = stvdy_server::util::next_local_month_start(timezone, harness.clock.as_ref());
        let month = stvdy_server::util::local_month_key(timezone, run_at);
        let key = format!("monthly_grant:{account}:{month}");
        let row = rows
            .iter()
            .find(|(_, candidate, _)| *candidate == key)
            .unwrap_or_else(|| panic!("no grant scheduled under {key}: {rows:?}"));
        assert_eq!(
            row.2, run_at,
            "the grant runs at the account's own local month start"
        );
    }

    // The rollover is scheduled too: expiring a grace window is part of the same boundary.
    let counters: i64 = sqlx::query_scalar(
        "select count(*)::bigint from jobs where dedupe_key like 'reset_counters:%'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(counters, 2);

    // A restart re-runs the pass; the accounts already have a job waiting, so nothing is
    // enqueued a second time.
    let scheduled = stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the second pass runs");
    assert_eq!(
        scheduled, 0,
        "a restart must not double-schedule an account"
    );
    let after: i64 = sqlx::query_scalar("select count(*)::bigint from jobs")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(after, 4, "two accounts, two kinds, no duplicates");
}
