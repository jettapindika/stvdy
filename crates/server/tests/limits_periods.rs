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
use common::{json as request, multipart, post_json, signed_up};

/// The signup password `common::signed_up` uses, so a test can sign the same account in
/// again and land on the *same* device row (`resolve_device` matches on name and platform).
const PASSWORD: &str = "correct horse battery";

/// A token for an account that already exists, on the device named `Laptop` (macos).
///
/// The device matters: a device over the plan's `devices_max` is read-only and refuses every
/// spend, so a test that signed in on a fresh device would be testing that refusal rather
/// than the row it means to write.
async fn sign_in(app: &axum::Router, email: &str) -> String {
    let (status, body) = request(
        app,
        "POST",
        "/auth/login",
        "",
        json!({
            "email": email,
            "password": PASSWORD,
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    body["access_token"]
        .as_str()
        .expect("an access token")
        .to_string()
}

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
    let after: i64 = sqlx::query_scalar(
        "select count(*)::bigint from jobs
          where kind in ('monthly_grant', 'reset_counters')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    // Scoped to the boundary kinds: a pass also enqueues the daily change-log compaction,
    // which is a single global job rather than a per-account boundary one (A184), and its
    // own suite asserts that it is enqueued once.
    assert_eq!(after, 4, "two accounts, two kinds, no duplicates");
}
/// A claim-shaped caller for a specific device, as the extractor would build it.
///
/// Built by hand because the subject here is what the decision does with the device it is
/// handed; the extractor is the auth suite's subject.
fn caller_for(
    user_id: Uuid,
    device_id: Uuid,
    timezone: &str,
) -> stvdy_server::auth::extract::AuthUser {
    stvdy_server::auth::extract::AuthUser {
        user_id,
        device_id,
        session_id: Uuid::now_v7(),
        email: "caller@example.test".to_string(),
        email_verified: true,
        plan_id: "free".to_string(),
        timezone: timezone.to_string(),
        is_admin: false,
        student_verified: false,
        platform: "macos".to_string(),
        subscription_status: stvdy_core::entitlements::SubStatus::None,
    }
}

/// Seeds a live subscription for the account, so the effective plan is the stored one.
async fn subscribe(harness: &TestState, user_id: Uuid, plan: &str, ends_in_days: i64) {
    sqlx::query(
        "insert into subscriptions
           (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly',
                 now() - make_interval(days => 30),
                 now() + make_interval(days => $3))",
    )
    .bind(user_id)
    .bind(plan)
    .bind(ends_in_days as i32)
    .execute(&harness.state.pool)
    .await
    .expect("seeding a subscription");
}

/// Inserts `count` live devices for the account, all created at `at` plus a second each so
/// the ranking is deterministic, and returns their ids oldest-first.
async fn seed_devices(
    harness: &TestState,
    user_id: Uuid,
    count: i64,
    at: DateTime<Utc>,
) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for index in 0..count {
        let id = Uuid::now_v7();
        sqlx::query(
            "insert into devices (id, user_id, name, platform, created_at)
             values ($1, $2, $3, 'macos', $4)",
        )
        .bind(id)
        .bind(user_id)
        .bind(format!("Device {index}"))
        .bind(at + Duration::seconds(index))
        .execute(&harness.state.pool)
        .await
        .expect("seeding a device");
        ids.push(id);
    }
    ids
}

/// Inserts `count` live sources in `workspace_id`, created at `at` plus a second each.
async fn seed_sources(
    harness: &TestState,
    user_id: Uuid,
    workspace_id: Uuid,
    count: i64,
    bytes: i64,
    at: DateTime<Utc>,
) {
    for index in 0..count {
        sqlx::query(
            "insert into sources
               (user_id, workspace_id, kind, title, file_key, file_size, mime, status, created_at)
             values ($1, $2, 'text', $3, $4, $5, 'text/plain', 'ready', $6)",
        )
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("Source {index}"))
        .bind(format!("users/{user_id}/sources/{index}.txt"))
        .bind(bytes)
        .bind(at + Duration::seconds(index))
        .execute(&harness.state.pool)
        .await
        .expect("seeding a source");
    }
}

/// The downgrade's shape: a power account with more rows than free allows.
///
/// Returns the caller's workspace (the oldest, so it stays writable), the read-only
/// workspace and source, and the ids of the devices.
struct Downgrade {
    user_id: Uuid,
    device_id: Uuid,
    writable_workspace: Uuid,
    read_only_workspace: Uuid,
    read_only_source: Uuid,
    over_limit_devices: Vec<Uuid>,
}

/// Builds the account: three workspaces, thirty sources, six devices on power, then the
/// subscription expires and the flags are recomputed against free's limits (2 / 20 / 2).
async fn downgraded_account(harness: &TestState, email: &str) -> Downgrade {
    let app = router(harness);
    signed_up(&app, harness, email).await;
    let user_id = user_id(harness, email).await;

    // The signed-up device is the oldest, so the caller keeps a writable device and the
    // refusals below are about the row being written, not about the caller's own device.
    let signed_up_device: Uuid = sqlx::query_scalar("select id from devices where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the signup device");
    sqlx::query("update devices set created_at = now() - interval '100 days' where id = $1")
        .bind(signed_up_device)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let at = instant("2026-09-01T00:00:00Z");
    let extra_devices = seed_devices(harness, user_id, 5, at).await;
    let mut devices = vec![signed_up_device];
    devices.extend(extra_devices);

    sqlx::query("update users set plan_id = 'power' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    subscribe(harness, user_id, "power", 10).await;

    // Three workspaces, oldest first; the newest is the one the downgrade flags.
    let mut workspaces = Vec::new();
    for index in 0..3 {
        let id = Uuid::now_v7();
        sqlx::query(
            "insert into workspaces (id, user_id, name, created_at)
             values ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(user_id)
        .bind(format!("Workspace {index}"))
        .bind(at + Duration::seconds(index))
        .execute(&harness.state.pool)
        .await
        .unwrap();
        workspaces.push(id);
    }

    // Thirty sources in the newest workspace, so ten of them are over free's twenty.
    seed_sources(harness, user_id, workspaces[2], 30, 1024, at).await;
    let read_only_source: Uuid = sqlx::query_scalar(
        "select id from sources where user_id = $1 order by created_at desc limit 1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // Nothing is flagged while power is active, and the effective plan is power because the
    // subscription is live (A146).
    let before = stvdy_server::entitlements::recompute_read_only(
        &harness.state.pool,
        &harness.state.config.plans,
        user_id,
    )
    .await
    .expect("the recompute runs");
    assert_eq!(before, Default::default(), "power is over no limit");

    // The subscription expires: the same event the rollover job handles, applied directly
    // so this suite tests the downgrade rather than the clock.
    sqlx::query("update subscriptions set status = 'expired' where user_id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    Downgrade {
        user_id,
        device_id: signed_up_device,
        writable_workspace: workspaces[0],
        read_only_workspace: workspaces[2],
        read_only_source,
        over_limit_devices: devices[2..].to_vec(),
    }
}

/// **The downgrade test.** A power account with 3 workspaces, 30 sources and 6 devices whose
/// subscription expires keeps every row and every byte: 3 workspaces, 30 sources, 6 devices,
/// all readable, all exportable. What changes is that the over-limit rows are `read_only` and
/// a new one is refused.
///
/// The alternative — deleting or hiding the over-limit data — is what the spec forbids
/// ("never delete user data") and what makes a downgrade a data-loss event.
#[sqlx::test(migrations = "../../migrations")]
async fn a_downgrade_keeps_the_data_and_flags_the_over_limit_rows(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let downgrade = downgraded_account(&harness, "downgrade@example.test").await;

    let report = stvdy_server::entitlements::recompute_read_only(
        &harness.state.pool,
        &harness.state.config.plans,
        downgrade.user_id,
    )
    .await
    .expect("the recompute runs");

    // Free allows 2 workspaces, 20 sources and 2 devices, so 1 / 10 / 4 rows are over.
    assert_eq!(report.workspaces, 1);
    assert_eq!(report.sources, 10);
    assert_eq!(report.devices, 4);

    // Every row is still there, and every byte: nothing was deleted or hidden.
    let counts: (i64, i64, i64) = (
        sqlx::query_scalar("select count(*)::bigint from workspaces where user_id = $1")
            .bind(downgrade.user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap(),
        sqlx::query_scalar("select count(*)::bigint from sources where user_id = $1")
            .bind(downgrade.user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap(),
        sqlx::query_scalar("select count(*)::bigint from devices where user_id = $1")
            .bind(downgrade.user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap(),
    );
    assert_eq!(
        counts,
        (3, 30, 6),
        "a downgrade must keep every row: 3 workspaces, 30 sources, 6 devices"
    );

    // The oldest stay writable and the newest are flagged.
    let workspaces: Vec<(String, bool)> = sqlx::query_as(
        "select name, read_only from workspaces where user_id = $1 order by created_at",
    )
    .bind(downgrade.user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        workspaces,
        vec![
            ("Workspace 0".to_string(), false),
            ("Workspace 1".to_string(), false),
            ("Workspace 2".to_string(), true),
        ]
    );

    // And a new one is refused: the write path consults the flag, it is not a UI affordance.
    let app = router(&harness);
    let token = sign_in(&app, "downgrade@example.test").await;
    let (content_type, body) = multipart(&[("kind", "text", None), ("text", "notes", None)]);
    let (status, body) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{}/sources", downgrade.read_only_workspace),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "an upload into a read-only workspace must be refused: {body}"
    );
    assert_eq!(body["code"], "limit_reached");
    assert_eq!(
        body["limit"], 2,
        "the refusal carries the workspaces limit, not the sources one: {body}"
    );
    assert_eq!(body["min_plan"], "pro");
    let sources_after: i64 =
        sqlx::query_scalar("select count(*)::bigint from sources where user_id = $1")
            .bind(downgrade.user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(sources_after, 30, "a refused upload writes nothing");

    // The workspace that is *not* over the limit is still writable: the flag is per row, not
    // per account, which is what makes "delete the newest one" a remedy rather than a
    // punishment. The upload itself is refused by `sources_total` (30 of 20), so what this
    // asserts is that the refusal is the sources one and not the read-only one.
    let (content_type, body) = multipart(&[("kind", "text", None), ("text", "notes", None)]);
    let (status, body) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{}/sources", downgrade.writable_workspace),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert_eq!(body["code"], "limit_reached");
    assert_eq!(
        body["limit"], 20,
        "a writable workspace refuses on the sources count, not on the workspace's flag: {body}"
    );
}

/// Reading an over-limit workspace, source and device still works; writing to one does not.
///
/// The spec's word is "viewable, exportable": a downgrade that made the data unreadable
/// would be indistinguishable from a delete.
#[sqlx::test(migrations = "../../migrations")]
async fn over_limit_rows_stay_readable(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let downgrade = downgraded_account(&harness, "readable@example.test").await;
    stvdy_server::entitlements::recompute_read_only(
        &harness.state.pool,
        &harness.state.config.plans,
        downgrade.user_id,
    )
    .await
    .expect("the recompute runs");

    let app = router(&harness);
    let token = sign_in(&app, "readable@example.test").await;

    // Reads of the over-limit rows: the list, the row itself, and the sources inside it.
    for path in [
        "/workspaces".to_string(),
        format!("/workspaces/{}", downgrade.read_only_workspace),
        format!("/workspaces/{}/sources", downgrade.read_only_workspace),
        "/devices".to_string(),
        format!("/sources/{}/status", downgrade.read_only_source),
    ] {
        let (status, body) = request(&app, "GET", &path, &token, serde_json::Value::Null).await;
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "GET {path} must stay readable: {body}"
        );
    }

    // Writes to the read-only rows are refused: the workspace's own rename, and the device's.
    let (status, body) = request(
        &app,
        "PATCH",
        &format!("/workspaces/{}", downgrade.read_only_workspace),
        &token,
        json!({ "name": "renamed" }),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert_eq!(body["code"], "limit_reached");

    let flagged_device = downgrade.over_limit_devices[0];
    let (status, body) = request(
        &app,
        "PATCH",
        &format!("/devices/{flagged_device}"),
        &token,
        json!({ "name": "renamed" }),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert_eq!(body["code"], "limit_reached");
    assert_eq!(
        body["limit"],
        harness.state.config.plans.get(PlanId::Free).devices_max as i64,
        "the refusal carries the device limit: {body}"
    );

    // The device's *own* refusal: it may read, but it may not spend, and the code says the
    // remedy is to sign another device out rather than to buy (A143).
    let caller = caller_for(downgrade.user_id, flagged_device, "Asia/Jakarta");
    let refused =
        stvdy_server::entitlements::require(&harness.state, &caller, Feature::ChatMessage)
            .await
            .expect_err("a read-only device may not spend");
    assert_eq!(refused.code(), "feature_locked");
    assert_eq!(refused.status(), axum::http::StatusCode::FORBIDDEN);
    match &refused {
        stvdy_server::error::AppError::Entitlement { min_plan, .. } => {
            assert_eq!(*min_plan, None, "there is no plan to sell this device")
        }
        other => panic!("expected a feature_locked refusal, got {other:?}"),
    }

    // The caller's own (oldest, writable) device is unaffected: the refusal is about the
    // device the request came from, not about the account.
    let caller = caller_for(downgrade.user_id, downgrade.device_id, "Asia/Jakarta");
    stvdy_server::entitlements::require(&harness.state, &caller, Feature::ChatMessage)
        .await
        .expect("the writable device still spends");

    // And the remedy works: revoking the flagged device is allowed, and it frees the slot.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/devices/{flagged_device}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
}

/// `devices_max` counts live devices only, so revoking one is the remedy the refusal
/// suggests — and a device over the new limit is flagged rather than logged out.
#[sqlx::test(migrations = "../../migrations")]
async fn the_extra_devices_are_flagged_not_revoked(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let downgrade = downgraded_account(&harness, "devices@example.test").await;
    stvdy_server::entitlements::recompute_read_only(
        &harness.state.pool,
        &harness.state.config.plans,
        downgrade.user_id,
    )
    .await
    .expect("the recompute runs");

    // Flagged, not revoked: the session survives, which is what "read-only" has to mean if
    // the remedy is for the *user* to choose which device to sign out.
    let live: Vec<(bool, bool)> = sqlx::query_as(
        "select read_only, revoked_at is not null from devices
          where user_id = $1 order by created_at",
    )
    .bind(downgrade.user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live.len(), 6, "no device was deleted");
    assert!(
        live.iter().all(|(_, revoked)| !revoked),
        "a downgrade must not sign a device out"
    );
    assert_eq!(
        live.iter().filter(|(read_only, _)| *read_only).count(),
        4,
        "free allows two live devices, so the other four are flagged"
    );

    // Revoking one is the remedy: the live count drops and the recompute flags one fewer.
    sqlx::query("update devices set revoked_at = now() where id = $1")
        .bind(downgrade.over_limit_devices[0])
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let report = stvdy_server::entitlements::recompute_read_only(
        &harness.state.pool,
        &harness.state.config.plans,
        downgrade.user_id,
    )
    .await
    .expect("the recompute runs");
    assert_eq!(
        report.devices, 3,
        "a revoked device frees its slot, so one fewer row is over the limit"
    );

    // And a revoked device is not flagged: it is signed out, and the flag on it would be a
    // statement about a device that can no longer make a request.
    let revoked_flagged: bool = sqlx::query_scalar("select read_only from devices where id = $1")
        .bind(downgrade.over_limit_devices[0])
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert!(!revoked_flagged);
}

/// Storage is `sum(file_size)` over live sources, and the check is existing + new (A157).
///
/// One number for both: the sum the snapshot reports is the sum the upload decision adds to,
/// so a user cannot be shown one figure and refused against another.
#[sqlx::test(migrations = "../../migrations")]
async fn storage_is_summed_over_live_sources_before_the_upload(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "storage@example.test").await;
    let user_id = user_id(&harness, "storage@example.test").await;

    let workspace: Uuid = sqlx::query_scalar(
        "insert into workspaces (user_id, name) values ($1, 'storage') returning id",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // Ten sources of 19 MiB — under the free plan's 20 MB per-file ceiling — plus one
    // tombstoned source that must not count.
    seed_sources(
        &harness,
        user_id,
        workspace,
        10,
        19 * 1024 * 1024,
        instant("2026-09-01T00:00:00Z"),
    )
    .await;
    sqlx::query(
        "insert into sources
           (user_id, workspace_id, kind, title, file_key, file_size, mime, status, deleted_at)
         values ($1, $2, 'text', 'deleted', 'k', 1048576, 'text/plain', 'ready', now())",
    )
    .bind(user_id)
    .bind(workspace)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let summed = stvdy_server::entitlements::storage_bytes(&harness.state.pool, user_id)
        .await
        .expect("storage is readable");
    assert_eq!(
        summed,
        10 * 19 * 1024 * 1024,
        "storage is the originals of live sources only"
    );

    let snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, user_id)
        .await
        .expect("the snapshot is readable");
    assert_eq!(
        snapshot.storage_bytes, summed,
        "the snapshot and the storage helper are the same number"
    );

    // The upload check is existing + new, and the quota is a ceiling: a file that fits
    // exactly is admitted, one byte more is refused with the numbers.
    let quota = i64::from(harness.state.config.plans.get(PlanId::Free).storage_mb) * 1024 * 1024;
    let caller = caller_for(user_id, Uuid::now_v7(), "Asia/Jakarta");
    stvdy_server::entitlements::require(
        &harness.state,
        &caller,
        Feature::UploadSource {
            size_bytes: quota - summed,
        },
    )
    .await
    .expect("a file that fits the quota exactly is admitted");

    let refused = stvdy_server::entitlements::require(
        &harness.state,
        &caller,
        Feature::UploadSource {
            size_bytes: quota - summed + 1,
        },
    )
    .await
    .expect_err("one byte over the quota is refused");
    assert_eq!(refused.code(), "limit_reached");
    match &refused {
        stvdy_server::error::AppError::Entitlement { limit, used, .. } => {
            assert_eq!(*limit, Some(quota), "the quota is the limit it names");
            assert_eq!(
                *used,
                Some(quota + 1),
                "used is existing + new, the number the upload would leave behind"
            );
        }
        other => panic!("expected a limit_reached refusal, got {other:?}"),
    }
}
