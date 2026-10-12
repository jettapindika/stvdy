//! Renewal reminders: four boundaries, once each, and copy the policy allows.
//!
//! Every assertion here is about the **enqueued rows**, not about a delivered notification:
//! Phase 12 owns the transport, and a test that waited for a push would be a test of that
//! slice rather than of this one. What this suite proves is that the producer emits the right
//! four rows, once each, and that the copy it hands to the delivery path cannot carry a
//! purchase call to action onto a phone.
//!
//! The clock is pinned, because "seven days before the period end, at the student's local
//! 09:00" is a statement about an instant: a boundary computed from the ambient time would
//! pass or fail depending on when the suite ran.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::jobs::ClaimedJob;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::signed_up;

/// 2026-10-11T02:00Z is 09:00 in Jakarta, which is the same instant the period suite pins.
/// The period end is then 2026-11-01T17:00Z — 2026-11-02T00:00 local — so all four boundaries
/// (26 Oct, 30 Oct, 1 Nov, 2 Nov, each at local 09:00) are still ahead of the clock and the
/// pass enqueues every one of them.
const NOW: &str = "2026-10-11T02:00:00Z";
const PERIOD_END: &str = "2026-11-01T17:00:00Z";

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

/// A paid account with a live subscription ending at [`PERIOD_END`].
async fn paid_account(harness: &TestState, email: &str) -> Uuid {
    let app = router(harness);
    signed_up(&app, harness, email).await;
    let user_id = user_id_of(harness, email).await;

    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', $2, $3)",
    )
    .bind(user_id)
    .bind(instant(NOW) - Duration::days(30))
    .bind(instant(PERIOD_END))
    .execute(&harness.state.pool)
    .await
    .unwrap();

    user_id
}

/// A claim-shaped job for one boundary, as the worker would hand it to the handler.
fn boundary_job(user_id: Uuid, days: i64) -> ClaimedJob {
    ClaimedJob {
        id: Uuid::now_v7(),
        kind: "renewal_reminder".to_string(),
        payload: json!({
            "user_id": user_id,
            "period_end": instant(PERIOD_END).to_rfc3339(),
            "days": days,
        }),
        attempts: 1,
    }
}

async fn reminder_rows(harness: &TestState, user_id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as("select dedupe_key, kind from reminders where user_id = $1 order by dedupe_key")
        .bind(user_id)
        .fetch_all(&harness.state.pool)
        .await
        .unwrap()
}

async fn push_jobs(harness: &TestState, user_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "select payload from jobs
          where kind = 'send_push' and payload->>'reminder_id' in
                (select id::text from reminders where user_id = $1)
          order by payload->>'reminder_id'",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap()
}

async fn scheduled_boundaries(harness: &TestState, user_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        // `payload->>'user_id'` is text, so the parameter is bound as text too: sqlx sends a
        // `Uuid` as a `uuid`, and Postgres has no `text = uuid` operator, so binding the id
        // itself would fail at prepare rather than at comparison.
        "select dedupe_key from jobs
          where kind = 'renewal_reminder' and payload->>'user_id' = $1
          order by dedupe_key",
    )
    .bind(user_id.to_string())
    .fetch_all(&harness.state.pool)
    .await
    .unwrap()
}

/// Four boundaries, four reminders, once each — asserted by the enqueued rows, so this test
/// does not depend on Phase 12's transport.
#[sqlx::test(migrations = "../../migrations")]
async fn the_seven_three_one_and_zero_day_reminders_each_fire_once(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(instant(NOW))));
    harness.seed_plans().await.unwrap();
    let user_id = paid_account(&harness, "renewing@example.test").await;

    stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the scheduler pass runs");

    let keys = scheduled_boundaries(&harness, user_id).await;
    assert_eq!(keys.len(), 4, "one job per boundary: {keys:?}");
    for days in ["7", "3", "1", "0"] {
        let suffix = format!(":{days}");
        assert!(
            keys.iter().any(|key| key.ends_with(&suffix)),
            "the {days}-day boundary is scheduled: {keys:?}"
        );
    }

    // Each boundary's own run writes its reminder and enqueues its delivery.
    for days in [7_i64, 3, 1, 0] {
        stvdy_server::jobs::handlers::renewal::run(&harness.state, &boundary_job(user_id, days))
            .await
            .expect("the reminder is produced");
    }

    let rows = reminder_rows(&harness, user_id).await;
    assert_eq!(rows.len(), 4, "four reminders, one per boundary: {rows:?}");
    assert!(
        rows.iter().all(|(_, kind)| kind == "renewal"),
        "every row is a renewal: {rows:?}"
    );
    assert_eq!(
        push_jobs(&harness, user_id).await.len(),
        4,
        "four deliveries"
    );

    // A second run of each boundary produces nothing: the reminder's dedupe key is the
    // watermark, exactly as the grant's period key is (A152/A300).
    for days in [7_i64, 3, 1, 0] {
        stvdy_server::jobs::handlers::renewal::run(&harness.state, &boundary_job(user_id, days))
            .await
            .expect("the second run is a no-op");
    }
    assert_eq!(
        reminder_rows(&harness, user_id).await.len(),
        4,
        "a second run adds no reminder"
    );
    assert_eq!(
        push_jobs(&harness, user_id).await.len(),
        4,
        "and no second delivery"
    );
}

/// A second scheduler pass in the same day enqueues nothing: the dedupe key is the
/// watermark (A300), exactly as the grant's period key is (A152).
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_pass_sends_nothing(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(instant(NOW))));
    harness.seed_plans().await.unwrap();
    let user_id = paid_account(&harness, "twopass@example.test").await;

    stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the first pass runs");
    let first = scheduled_boundaries(&harness, user_id).await;
    assert_eq!(first.len(), 4);

    stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the second pass runs");
    let second = scheduled_boundaries(&harness, user_id).await;

    assert_eq!(
        first, second,
        "the second pass enqueues nothing: the key is the record that the boundary is done"
    );
}

/// The mobile copy names the date and nothing to buy: no price, no link, no CTA
/// (`billing_policy`), and the same key renders the same sentence in Indonesian.
#[sqlx::test(migrations = "../../migrations")]
async fn the_reminder_copy_follows_billing_policy_on_a_phone(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(instant(NOW))));
    harness.seed_plans().await.unwrap();

    // One account whose only device is an iPhone, and one whose only device is a laptop.
    let phone = paid_account(&harness, "phone@example.test").await;
    let desktop = paid_account(&harness, "desktop@example.test").await;
    for user_id in [phone, desktop] {
        sqlx::query("update devices set platform = 'ios' where user_id = $1")
            .bind(user_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
    }
    sqlx::query("update devices set platform = 'macos' where user_id = $1")
        .bind(desktop)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    for user_id in [phone, desktop] {
        stvdy_server::jobs::handlers::renewal::run(&harness.state, &boundary_job(user_id, 3))
            .await
            .expect("the reminder is produced");
    }

    let phone_body = push_jobs(&harness, phone).await[0]["body"]
        .as_str()
        .expect("a body")
        .to_string();
    let desktop_body = push_jobs(&harness, desktop).await[0]["body"]
        .as_str()
        .expect("a body")
        .to_string();

    // The date is in both: the reminder still says what it is for.
    assert!(phone_body.contains("1 November 2026"), "{phone_body}");
    // And the phone's copy offers nothing to buy — no price, no link, no call to action.
    assert!(!phone_body.contains("/pricing"), "{phone_body}");
    assert!(!phone_body.contains("Renew"), "{phone_body}");
    assert!(!phone_body.contains("IDR"), "{phone_body}");
    assert!(!phone_body.contains("10,000"), "{phone_body}");

    // The desktop copy is the one that may carry the link, which is what makes the policy
    // check observable rather than decorative.
    assert!(desktop_body.contains("/pricing"), "{desktop_body}");

    // The i18n key travels beside the sentence, so the SPA's reminder feed renders the same
    // reminder from `en.json`/`id.json` rather than re-deriving the copy from the numbers.
    let payload = &push_jobs(&harness, phone).await[0];
    assert_eq!(payload["key"], "reminder.planEnding", "{payload}");
    assert_eq!(payload["plan_id"], "pro", "{payload}");
    assert!(payload["reminder_id"].is_string(), "{payload}");
}

/// Renewing moves the boundary, so the old period's reminders never fire.
#[sqlx::test(migrations = "../../migrations")]
async fn renewing_moves_the_boundary_and_cancels_the_old_reminders(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(instant(NOW))));
    harness.seed_plans().await.unwrap();
    let user_id = paid_account(&harness, "renewed@example.test").await;

    stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the scheduler pass runs");
    assert_eq!(scheduled_boundaries(&harness, user_id).await.len(), 4);

    // The student renews before any of the four fires: the period end moves forward.
    let new_end = instant(PERIOD_END) + Duration::days(30);
    sqlx::query("update subscriptions set current_period_end = $2 where user_id = $1")
        .bind(user_id)
        .bind(new_end)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    // The job for the *old* boundary still runs — the queue does not know the period moved —
    // and it must produce nothing: telling a student their plan is ending when they have
    // just renewed is worse than no reminder at all.
    for days in [7_i64, 3, 1, 0] {
        stvdy_server::jobs::handlers::renewal::run(&harness.state, &boundary_job(user_id, days))
            .await
            .expect("the stale boundary is a no-op");
    }

    assert!(
        reminder_rows(&harness, user_id).await.is_empty(),
        "a reminder for a period that no longer exists is not written"
    );
    assert!(
        push_jobs(&harness, user_id).await.is_empty(),
        "and nothing is delivered"
    );

    // The next pass schedules the new period's boundaries, keyed by the new end date.
    stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the next pass runs");
    let keys = scheduled_boundaries(&harness, user_id).await;
    assert!(
        keys.iter()
            .any(|key| key.contains(&new_end.date_naive().to_string())),
        "the new period's boundaries are scheduled: {keys:?}"
    );

    // The old period's rows are **still in the queue**, and that is the design: a queue does
    // not retract work, so the four stale boundaries remain until the worker claims them. What
    // makes them harmless is the handler, not their absence — which is exactly what the two
    // assertions above prove (`reminder_rows` and `push_jobs` are both empty after every one of
    // them has run). Asserting that the old keys are *gone* would be asserting something the
    // design does not promise, and the property that matters is that they produce nothing.
    let stale = keys
        .iter()
        .filter(|key| key.contains(&instant(PERIOD_END).date_naive().to_string()))
        .count();
    assert_eq!(
        stale, 4,
        "the old period's four boundaries are still queued and inert: {keys:?}"
    );

    // And the two key sets are disjoint, so the new period's reminders cannot collide with
    // the old ones (the period end is part of the key, A300).
    for key in &keys {
        if key.contains(&new_end.date_naive().to_string()) {
            assert!(
                !key.contains(&instant(PERIOD_END).date_naive().to_string()),
                "a key names one period: {key}"
            );
        }
    }
}

/// A free account has no period to end, so it is never reminded — and neither is an account
/// whose subscription has already lapsed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_free_account_is_never_reminded(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(instant(NOW))));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "free@example.test").await;
    let user_id = user_id_of(&harness, "free@example.test").await;

    stvdy_server::jobs::schedule::pass(&harness.state)
        .await
        .expect("the scheduler pass runs");

    assert!(
        scheduled_boundaries(&harness, user_id).await.is_empty(),
        "a free account has nothing to renew"
    );
    assert!(
        stvdy_server::jobs::handlers::renewal::run(&harness.state, &boundary_job(user_id, 7))
            .await
            .is_ok(),
        "a stale boundary for a free account is a no-op rather than a failure"
    );
    assert!(reminder_rows(&harness, user_id).await.is_empty());
}

/// The plan name reaches the copy, so the sentence names the plan the student actually has.
#[sqlx::test(migrations = "../../migrations")]
async fn the_copy_names_the_plan_the_account_is_on(pool: PgPool) {
    let harness =
        TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(instant(NOW))));
    harness.seed_plans().await.unwrap();
    let user_id = paid_account(&harness, "power@example.test").await;
    sqlx::query("update subscriptions set plan_id = 'power' where user_id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update users set plan_id = 'power' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    stvdy_server::jobs::handlers::renewal::run(&harness.state, &boundary_job(user_id, 0))
        .await
        .expect("the reminder is produced");

    let payload = &push_jobs(&harness, user_id).await[0];
    let title = payload["title"].as_str().expect("a title");
    assert!(title.contains("Power"), "{title}");
    assert!(
        title.contains("today"),
        "the 0-day copy says today: {title}"
    );
    assert_eq!(payload["plan_id"], "power", "{payload}");
}
