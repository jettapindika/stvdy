//! The server's decision path, asserted through its own interface.
//!
//! Three things this suite exists to catch, each of which is invisible to a reading:
//!
//!  * the snapshot and the daily cap counting **different rows**, which is how a user is
//!    told "20 of 20" and then allowed the 21st (or refused the 20th);
//!  * a locked feature answered with the same code on every platform, which is how an iOS
//!    build ends up rendering a purchase call to action the App Store forbids (A143);
//!  * `users.plan_id` granting paid limits after the subscription expired (A146).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use stvdy_core::config::{ExportFormat, PlanId};
use stvdy_core::entitlements::{
    Credits, EntitlementError, Feature, PlanFacts, SubStatus, UsageSnapshot,
};
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::auth::extract::AuthUser;
use stvdy_server::error::AppError;
use stvdy_server::testing::TestState;
use stvdy_sync::proto::{Entity, Mutation, MutationResult, Op};
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// The instant every test in this file runs at, so a boundary case is a fixed
/// boundary rather than whatever time of day the suite happens to start.
///
/// 12:00 UTC is chosen because it makes the two zones below straddle a day
/// boundary: `Pacific/Kiritimati` (+14) is already on the next local day while
/// `America/New_York` (-4 in June) is still on the previous UTC date.
fn clock_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
}

/// A caller with no session, built by hand: the extractor is the auth suite's subject,
/// and these tests are about what the decision does with the fields it is handed.
fn caller(user_id: Uuid, plan_id: &str, timezone: &str, platform: &str) -> AuthUser {
    AuthUser {
        user_id,
        device_id: Uuid::now_v7(),
        session_id: Uuid::now_v7(),
        email: "caller@example.test".to_string(),
        email_verified: true,
        plan_id: plan_id.to_string(),
        timezone: timezone.to_string(),
        is_admin: false,
        student_verified: false,
        platform: platform.to_string(),
        subscription_status: SubStatus::None,
    }
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// One `chat_message` usage row at an explicit instant, which is what lets the test put a
/// row on each side of a day boundary.
async fn chat_row(harness: &TestState, user_id: Uuid, at: DateTime<Utc>) {
    sqlx::query(
        "insert into usage_events (user_id, action, credits, created_at)
         values ($1, 'chat_message', 1, $2)",
    )
    .bind(user_id)
    .bind(at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// The snapshot's number and the daily cap's number are **the same number**, on a day
/// boundary that is the user's own.
///
/// Two accounts see the same three rows and disagree, because their local days start at
/// different instants. A UTC-midnight boundary would count the 05:00Z row for both, which
/// is exactly the bug A6 names: a student in Jakarta is refused at 07:00 local because the
/// server counted a UTC day.
///
/// Then the cap itself is driven: with the snapshot at the cap, the very next spend is
/// refused `limit_reached` with `used` equal to the number the snapshot reports. That is
/// the assertion that the two cannot drift — a snapshot reading a different table than the
/// cap would show one number and enforce another.
#[sqlx::test(migrations = "../../migrations")]
async fn the_snapshot_counts_the_same_rows_the_daily_cap_counts(pool: PgPool) {
    let mut harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    harness.state.clock = Arc::new(FixedClock::at(clock_at()));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    // `Pacific/Kiritimati` is +14: at 12:00Z its local day began at 10:00Z.
    // `America/New_York` is -4 in June: its local day began at 04:00Z.
    let ahead = signed_up(&app, &harness, "ahead@example.test").await;
    let behind = signed_up(&app, &harness, "behind@example.test").await;
    assert!(!ahead.is_empty() && !behind.is_empty());

    let ahead_id = user_id_of(&harness, "ahead@example.test").await;
    let behind_id = user_id_of(&harness, "behind@example.test").await;
    for (id, zone) in [
        (ahead_id, "Pacific/Kiritimati"),
        (behind_id, "America/New_York"),
    ] {
        sqlx::query("update users set timezone = $2 where id = $1")
            .bind(id)
            .bind(zone)
            .execute(&harness.state.pool)
            .await
            .unwrap();
    }

    let cap = harness
        .state
        .config
        .plans
        .get(PlanId::Free)
        .chat_msgs_per_day
        .max()
        .expect("the free plan caps daily messages") as i64;

    // Three rows after the -4 account's local midnight and before the +14 account's.
    let straddle = Utc.with_ymd_and_hms(2026, 6, 15, 5, 0, 0).unwrap();
    // And rows inside both accounts' local days.
    let today = Utc.with_ymd_and_hms(2026, 6, 15, 11, 0, 0).unwrap();
    for _ in 0..(cap - 3) {
        chat_row(&harness, ahead_id, today).await;
        chat_row(&harness, behind_id, today).await;
    }
    for _ in 0..3 {
        chat_row(&harness, ahead_id, straddle).await;
        chat_row(&harness, behind_id, straddle).await;
    }

    let ahead_user = caller(ahead_id, "free", "Pacific/Kiritimati", "macos");
    let behind_user = caller(behind_id, "free", "America/New_York", "macos");

    let ahead_snapshot = stvdy_server::entitlements::snapshot(&harness.state, &ahead_user)
        .await
        .unwrap();
    let behind_snapshot = stvdy_server::entitlements::snapshot(&harness.state, &behind_user)
        .await
        .unwrap();

    // The same rows, two answers: the straddling rows are yesterday for the account whose
    // local day began at 10:00Z and today for the one whose day began at 04:00Z. A
    // UTC-midnight boundary would report the cap for both.
    assert_eq!(
        ahead_snapshot.chat_msgs_today,
        cap - 3,
        "the +14 account's local day holds only the 11:00Z rows"
    );
    assert_eq!(
        behind_snapshot.chat_msgs_today, cap,
        "the -4 account's local day holds the 05:00Z rows too"
    );

    // The cap's own check, on the cap's own boundary, sees the same number: the account
    // the snapshot calls short of the cap still spends, which it would not if the cap
    // counted a UTC day.
    stvdy_server::credits::spend(
        &harness.state,
        ahead_id,
        PlanId::Free,
        "chat_message",
        1,
        [
            Some(stvdy_server::credits::daily_chat_cap(
                &harness.state,
                &ahead_user,
            )),
            None,
        ],
    )
    .await
    .expect("three rows before the local day start must not fill the cap");

    // Now the +14 account's local day is full, and the refusal reports the same `used`
    // the snapshot does. A snapshot reading a different table than the cap would show one
    // number and enforce another.
    //
    // The spend above wrote its own `usage_events` row, so the count is one higher than
    // the rows this test inserted: the top-up reads the snapshot rather than assuming,
    // which is also what makes the assertion below a real one.
    let after_spend = stvdy_server::entitlements::snapshot(&harness.state, &ahead_user)
        .await
        .unwrap()
        .chat_msgs_today;
    assert_eq!(
        after_spend,
        cap - 2,
        "the spend records the message it paid for"
    );
    for _ in 0..(cap - after_spend) {
        chat_row(&harness, ahead_id, today).await;
    }
    let ahead_snapshot = stvdy_server::entitlements::snapshot(&harness.state, &ahead_user)
        .await
        .unwrap();
    assert_eq!(ahead_snapshot.chat_msgs_today, cap);

    let error = stvdy_server::credits::spend(
        &harness.state,
        ahead_id,
        PlanId::Free,
        "chat_message",
        1,
        [
            Some(stvdy_server::credits::daily_chat_cap(
                &harness.state,
                &ahead_user,
            )),
            None,
        ],
    )
    .await
    .expect_err("the cap is full according to the snapshot, so the spend must be refused");

    assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.code(), "limit_reached");
    match error {
        AppError::Entitlement { limit, used, .. } => {
            assert_eq!(used, Some(ahead_snapshot.chat_msgs_today));
            assert_eq!(limit, Some(cap));
        }
        other => panic!("expected an entitlement error, got {other:?}"),
    }
}

/// A locked feature is `feature_locked` on iOS and `plan_required` on macOS (A143).
///
/// One fact, two codes, and the difference is a store rule rather than a plan rule: the
/// App Store forbids a purchase call to action, so the iOS build must not be told which
/// plan to buy. The platform is read from the **device row** (A144), which is why this
/// drives the real route: the account's signup named its platform, and the request body
/// cannot claim a different one.
#[sqlx::test(migrations = "../../migrations")]
async fn a_locked_feature_is_feature_locked_on_ios_and_plan_required_on_macos(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    // A cheatsheet PDF is a `pdf` export, which the free plan does not include.
    let cheatsheet_id = Uuid::now_v7();

    let macos = signed_up(&app, &harness, "desktop@example.test").await;
    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{cheatsheet_id}/pdf"),
        &macos,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");
    assert_eq!(
        body["min_plan"], "pro",
        "a desktop client may be told which plan has it: {body}"
    );

    // The same account on iOS: the same refusal, but with no plan to sell.
    let ios = signup_on(&app, &harness, "phone@example.test", "ios").await;
    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{cheatsheet_id}/pdf"),
        &ios,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "feature_locked", "{body}");
    assert!(
        body["min_plan"].is_null(),
        "an iOS build must not be told which plan to buy: {body}"
    );

    // The device row is the authority, not the request: the iOS account's own token
    // carries the platform it signed up with.
    let platform: String = sqlx::query_scalar(
        "select platform from devices where user_id = $1 and revoked_at is null",
    )
    .bind(user_id_of(&harness, "phone@example.test").await)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(platform, "ios");
}

/// `users.plan_id = 'pro'` with an expired subscription gets the free limits (A146).
///
/// The row is never edited by anything: the subscription's status is what decides, so an
/// expiry takes effect on the next request instead of waiting for an admin or a job. The
/// second half flips the status back to `active` and asserts the pro limits return, so the
/// test cannot pass by refusing everything.
#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_subscription_drops_to_the_free_limits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "lapsed@example.test").await;
    let user_id = user_id_of(&harness, "lapsed@example.test").await;

    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let subscription: Uuid = sqlx::query_scalar(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'expired', 'monthly', now() - interval '60 days', now() - interval '1 day')
         returning id",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // Free allows two workspaces; pro allows unlimited. Two already exist, so the third
    // is the assertion: the stored `plan_id` must not be what the decision reads.
    for index in 0..2 {
        let (status, body) = json(
            &app,
            "POST",
            "/workspaces",
            &token,
            serde_json::json!({ "name": format!("Workspace {index}") }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, user_id)
        .await
        .unwrap();
    assert_eq!(
        snapshot.plan,
        PlanId::Free,
        "an expired subscription must not honour the stored plan"
    );
    assert_eq!(snapshot.subscription_status, SubStatus::Expired);

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "One too many" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 2, "{body}");
    assert_eq!(body["used"], 2, "{body}");
    assert_eq!(body["min_plan"], "pro", "{body}");

    // Reactivating the same row returns the pro limits, without touching `users.plan_id`.
    sqlx::query("update subscriptions set status = 'active' where id = $1")
        .bind(subscription)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, user_id)
        .await
        .unwrap();
    assert_eq!(snapshot.plan, PlanId::Pro);
    assert_eq!(snapshot.subscription_status, SubStatus::Active);

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Now it fits" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// **`users.plan_id = 'power'` with no `subscriptions` row at all is free, and this test
/// names that decision so a later phase cannot change it silently.**
///
/// The rule is one rule, not two: `effective_plan` honours the stored plan only while the
/// subscription status grants paid limits, and `SubStatus::None` (no row) does not. A
/// genuinely free account has no row either, so "no row" and "expired" reach the same
/// verdict by the same comparison, which is why there is no special case to forget.
///
/// `SubStatus::None` is still a distinct *value* from `Expired` — only one of them has a
/// period that ended, and `GET /usage` shows the difference — but the decision does not
/// treat them differently, and it must not: an exception that granted paid limits without a
/// live subscription is exactly the bypass the status check exists to close. A comped
/// account belongs to Phase 13's admin plan switch, which writes a subscription row, rather
/// than to a second rule here.
#[sqlx::test(migrations = "../../migrations")]
async fn a_plan_with_no_subscription_row_grants_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nosub@example.test").await;
    let user_id = user_id_of(&harness, "nosub@example.test").await;

    sqlx::query("update users set plan_id = 'power' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, user_id)
        .await
        .unwrap();
    assert_eq!(snapshot.subscription_status, SubStatus::None);
    assert_eq!(snapshot.plan, PlanId::Free);

    // And through a route, not just the snapshot: power allows unlimited workspaces and free
    // allows two, so the third request is the assertion that the stored `plan_id` bought
    // nothing.
    for index in 0..2 {
        let (status, body) = json(
            &app,
            "POST",
            "/workspaces",
            &token,
            serde_json::json!({ "name": format!("Workspace {index}") }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "One too many" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a stored plan with no live subscription must not grant paid limits: {body}"
    );
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 2, "{body}");
}

/// The mapping the response bodies depend on, asserted against the roadmap's § 5 table.
///
/// `From<EntitlementError>` is the plan-free form of the mapping, and the handlers use it
/// through `entitlements::refusal`, which adds the caller's plan so a spent-out account can
/// be told which plan raises the allowance.
#[test]
fn the_entitlement_errors_map_to_the_roadmaps_status_and_code() {
    let status_of = |error: EntitlementError| AppError::from(error).status();
    let code_of = |error: EntitlementError| AppError::from(error).code();

    let plan_required = EntitlementError::PlanRequired {
        feature: Feature::ExportFormat(ExportFormat::Pdf),
        min_plan: Some(PlanId::Pro),
    };
    assert_eq!(status_of(plan_required), StatusCode::FORBIDDEN);
    assert_eq!(code_of(plan_required), "plan_required");

    let locked = EntitlementError::FeatureLocked {
        feature: Feature::Byok,
    };
    assert_eq!(status_of(locked), StatusCode::FORBIDDEN);
    assert_eq!(code_of(locked), "feature_locked");

    let reached = EntitlementError::LimitReached {
        feature: Feature::CreateWorkspace,
        limit: 2,
        used: 2,
        min_plan: Some(PlanId::Pro),
    };
    assert_eq!(status_of(reached), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(code_of(reached), "limit_reached");

    let short = EntitlementError::InsufficientCredits {
        needed: 6,
        available: 0,
    };
    assert_eq!(status_of(short), StatusCode::PAYMENT_REQUIRED);
    assert_eq!(code_of(short), "insufficient_credits");
}

/// The five keys are always present, whatever the refusal, because the client's error
/// mapper reads them unconditionally.
#[tokio::test]
async fn every_refusal_serializes_all_five_keys() {
    use axum::response::IntoResponse;

    let bodies = [
        AppError::from(EntitlementError::PlanRequired {
            feature: Feature::Byok,
            min_plan: Some(PlanId::Power),
        }),
        AppError::from(EntitlementError::FeatureLocked {
            feature: Feature::Byok,
        }),
        AppError::from(EntitlementError::LimitReached {
            feature: Feature::CreateWorkspace,
            limit: 2,
            used: 2,
            min_plan: None,
        }),
        AppError::from(EntitlementError::InsufficientCredits {
            needed: 1,
            available: 0,
        }),
    ];

    for error in bodies {
        let response = error.into_response();
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, 64 * 1024).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        for key in ["code", "message", "min_plan", "limit", "used"] {
            assert!(
                value.get(key).is_some(),
                "{parts:?} is missing {key}: {value}"
            );
        }
    }
}

/// Signs up with an explicit device platform, because `signed_up` hardcodes macOS and the
/// platform is the whole point of the A143 case.
async fn signup_on(app: &axum::Router, harness: &TestState, email: &str, platform: &str) -> String {
    let (status, created) = json(
        app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": "Phone", "platform": platform }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let link = harness
        .last_verification_link()
        .expect("a verification link");
    let token = link.split("token=").nth(1).expect("token").to_string();
    let (status, body) = json(
        app,
        "POST",
        "/auth/verify-email",
        "",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    created["access_token"]
        .as_str()
        .expect("an access token")
        .to_string()
}

// --- Task 9: the usage view, and the two boundaries later phases inherit ---------------

/// A free caller's snapshot, built by hand: the push and checkout boundaries are pure, so
/// they are asserted without a database.
fn free_snapshot() -> UsageSnapshot {
    UsageSnapshot {
        plan: PlanId::Free,
        subscription_status: SubStatus::None,
        credits: Credits::default(),
        storage_bytes: 0,
        workspaces: 0,
        sources: 0,
        devices: 1,
        chat_msgs_today: 0,
        web_searches_today: 0,
        flashcards_this_month: 0,
        quizzes_this_month: 0,
        reports_this_month: 0,
        syllabus_imports: 0,
    }
}

fn upsert(entity: Entity, payload: serde_json::Value) -> Mutation {
    Mutation {
        mutation_id: Uuid::now_v7(),
        entity,
        op: Op::Upsert,
        id: Uuid::now_v7(),
        payload: Some(payload),
        client_ts: Utc::now(),
    }
}

fn rejection_code(result: Result<(), MutationResult>) -> String {
    match result {
        Err(MutationResult::Rejected { code, .. }) => code,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

/// `GET /usage` is the only source of the numbers the UI shows (A159), and every number in
/// it is computed: no placeholder, no `0` standing in for "unknown".
///
/// The assertions are against the database, not against the view: the credit buckets are
/// the ledger's sums, the limit counts are the rows that exist, and the storage figure is
/// the sum the upload path itself reads.
#[sqlx::test(migrations = "../../migrations")]
async fn the_usage_view_reports_only_computed_numbers(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "view@example.test").await;
    let user_id = user_id_of(&harness, "view@example.test").await;

    let (status, body) = json(&app, "GET", "/usage", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["plan"], "free");
    assert_eq!(body["name"], "Free");
    assert_eq!(body["subscription"]["status"], "none");
    assert!(body["subscription"]["period_end"].is_null());

    // The buckets are the ledger's own sums, not literals.
    let monthly: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = 'monthly'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    let purchased: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and bucket = 'purchased'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(body["credits"]["monthly"], monthly);
    assert_eq!(body["credits"]["purchased"], purchased);
    assert_eq!(body["credits"]["total"], monthly + purchased);
    assert!(monthly > 0, "signup grants the first month's allowance");

    // The device the account signed up with is the count, and the plan's limit is the
    // config's value.
    assert_eq!(body["limits"]["devices_max"]["used"], 1);
    assert_eq!(
        body["limits"]["devices_max"]["limit"],
        harness.state.config.plans.get(PlanId::Free).devices_max
    );

    // A count that is not zero is the count: the view cannot pass by reporting zeroes.
    for index in 0..2 {
        let (status, created) = json(
            &app,
            "POST",
            "/workspaces",
            &token,
            serde_json::json!({ "name": format!("Workspace {index}") }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
    }
    let (status, body) = json(&app, "GET", "/usage", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let workspaces: i64 = sqlx::query_scalar(
        "select count(*)::bigint from workspaces where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(body["limits"]["workspaces"]["used"], workspaces);
    assert_eq!(body["limits"]["workspaces"]["used"], 2);
    assert_eq!(body["limits"]["workspaces"]["limit"], 2);

    // Storage is the byte sum the upload path checks, reported in bytes rather than a
    // rounded megabyte count.
    let storage: i64 = sqlx::query_scalar(
        "select coalesce(sum(file_size), 0)::bigint from sources
          where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(body["storage_bytes"], storage);

    // An unlimited plan reports `null` for the limit rather than a sentinel. Pro allows
    // unlimited workspaces.
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (status, body) = json(&app, "GET", "/usage", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["plan"], "pro");
    assert!(
        body["limits"]["workspaces"]["limit"].is_null(),
        "an unlimited plan reports null, not a sentinel: {body}"
    );
}

/// The cost the client renders is the cost the server charges, from one function
/// (roadmap gate 7(d)).
#[sqlx::test(migrations = "../../migrations")]
async fn the_usage_costs_are_the_costs_the_routes_charge(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "costs@example.test").await;

    let (status, body) = json(&app, "GET", "/usage", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // One function builds the block, so the view's table is `credits::costs` verbatim.
    assert_eq!(
        body["costs"],
        stvdy_server::credits::costs(&harness.state.config.plans)
    );

    // And the number the view quotes is the number the chat route charges: both read
    // `plans.credits.chat_message`, so a retuned config moves them together.
    let charged = stvdy_server::credits::chat_message_cost(&harness.state);
    assert_eq!(body["costs"]["chat_message"], charged);
    assert_eq!(
        body["costs"]["web_search"],
        stvdy_server::credits::search_cost(&harness.state)
    );
}

/// The USD ceiling figures are not in the user-facing view (A159); the pause is.
#[sqlx::test(migrations = "../../migrations")]
async fn the_usage_view_does_not_leak_unit_economics(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "economics@example.test").await;

    let (status, body) = json(&app, "GET", "/usage", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The pause is reported, and it is the only thing the view says about the ceiling.
    let provider_cost = body["provider_cost"]
        .as_object()
        .expect("provider_cost is an object");
    let keys: std::collections::BTreeSet<&str> = provider_cost.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from(["paused", "resets_at"]),
        "the view names the pause and the reset instant, never the dollars: {body}"
    );
    assert!(body["provider_cost"]["paused"].is_boolean());
    assert!(body["provider_cost"]["resets_at"].is_string());

    // Nothing in the payload is denominated in dollars. The ceiling's figures are
    // admin-only, and a user reading "0.50" would be reading our unit economics.
    let text = body.to_string().to_lowercase();
    assert!(
        !text.contains("usd"),
        "no dollar figure reaches the user-facing view: {body}"
    );
}

/// Phase 8 inherits this exact decision: a `Message` push is `read_only_entity` (A150).
///
/// The allowlist is the first guard, and the whole offline-write scope is asserted from
/// both directions so the test cannot pass by refusing everything: every server-owned
/// entity is refused, and every pushable one is accepted.
#[test]
fn the_push_decision_rejects_what_the_allowlist_rejects() {
    let plans = stvdy_server::testing::plans();
    let facts = PlanFacts {
        upgrade_offerable: true,
    };
    let snapshot = free_snapshot();

    for entity in [
        Entity::Message,
        Entity::Source,
        Entity::Workspace,
        Entity::Deck,
        Entity::Flashcard,
        Entity::Quiz,
        Entity::QuizQuestion,
        Entity::Report,
        Entity::Cheatsheet,
        Entity::Goal,
        Entity::PendingAction,
    ] {
        assert!(!entity.is_client_writable(), "{entity:?}");
        let result = stvdy_server::sync::push::decide(
            &plans,
            &facts,
            &snapshot,
            false,
            &upsert(entity, serde_json::json!({ "status": "ready" })),
        );
        assert_eq!(
            rejection_code(result),
            "read_only_entity",
            "a push of {entity:?} must be refused"
        );
    }

    // The other direction: the six offline-write entities are accepted, so the refusals
    // above are the allowlist and not a decision that refuses everything.
    for entity in [
        Entity::Task,
        Entity::Event,
        Entity::FlashcardReview,
        Entity::QuizAttempt,
        Entity::StudySession,
        Entity::UserSettings,
    ] {
        assert!(entity.is_client_writable(), "{entity:?}");
        stvdy_server::sync::push::decide(
            &plans,
            &facts,
            &snapshot,
            false,
            &upsert(entity, serde_json::json!({ "title": "Read chapter 4" })),
        )
        .expect("a pushable entity must be accepted");
    }
}

/// Phase 13 inherits this: `ios` and `android` are refused, and a body that claims
/// `platform: "web"` changes nothing (A144/A151).
///
/// The route-level half — an `ios` device whose request body says `platform: "web"` still
/// gets `feature_locked` — is driven through a real route in `tests/tamper.rs`; this
/// asserts the policy function itself and the fact the route relies on, that the platform
/// a decision sees is the device row's field and not a request field.
#[test]
fn checkout_is_refused_for_a_mobile_platform_and_a_body_cannot_claim_otherwise() {
    let policy = stvdy_server::testing::config().billing_policy;

    for mobile in ["ios", "android"] {
        let error = stvdy_server::billing::checkout_allowed(&policy, mobile)
            .expect_err("a mobile platform must not be sold to");
        assert_eq!(error.status(), StatusCode::FORBIDDEN);
        assert_eq!(error.code(), "feature_locked");
    }

    for desktop in ["web", "macos", "windows"] {
        assert!(
            stvdy_server::billing::checkout_allowed(&policy, desktop).is_ok(),
            "{desktop}"
        );
    }

    // An `ios` device's `AuthUser` offers no upgrade, whatever a body claims: the field is
    // filled from `devices.platform` by the extractor (A144).
    let ios = caller(Uuid::now_v7(), "free", "UTC", "ios");
    assert!(!ios.upgrade_offerable(&policy));
    let desktop = caller(Uuid::now_v7(), "free", "UTC", "macos");
    assert!(desktop.upgrade_offerable(&policy));
}
