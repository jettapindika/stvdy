//! `crates/server/src/jobs/handlers/reminders.rs`: derive once, claim once, deliver once.
//!
//! Three cases here are the ones the design exists for, and each catches a different way a
//! reminder fires twice or never:
//!
//!  * `a_due_reminder_is_delivered_once` — the claim. A second run of the same job must send
//!    nothing, which is what makes a retried job and a second instance safe.
//!  * `push_goes_to_the_most_recently_active_device` — one device, not all of them (A285), and
//!    the *most recent* one rather than whichever the query happened to return.
//!  * `an_unregistered_token_is_deleted_and_the_next_device_is_tried` — a dead token is
//!    deleted and the phone that is merely asleep is not, which is the distinction
//!    `PushError::Unregistered` exists for.
//!
//! `ScriptedPush::sends()` counts **attempts, including failed ones**, because a delivery row
//! records an attempt. So the assertions below read `sends()` for attempts and the
//! `reminder_deliveries` table for outcomes, rather than inferring one from the other.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use stvdy_core::traits::push::{PushError, PushPlatform};
use stvdy_core::traits::testing::FixedClock;
use stvdy_core::traits::testing::ScriptedPush;
use stvdy_server::integrations::push::PushRouter;
use stvdy_server::jobs::{ClaimedJob, handlers};
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::json;

/// A verified account **and its device**, because half of these cases are about which device a
/// push went to and `common::signed_up` hands back only the token.
async fn signup(
    app: &axum::Router,
    harness: &TestState,
    email: &str,
    name: &str,
) -> (String, Uuid) {
    let (status, body) = json(
        app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": name, "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let link = harness
        .last_verification_link()
        .expect("a verification link");
    let token = link.split("token=").nth(1).expect("token").to_string();
    let (status, _) = json(
        app,
        "POST",
        "/auth/verify-email",
        "",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    (
        body["access_token"]
            .as_str()
            .expect("access token")
            .to_string(),
        Uuid::parse_str(body["device_id"].as_str().expect("device id")).unwrap(),
    )
}

/// A clock pinned to an instant, so "due" is a fact rather than a race.
fn clock_at(at: &str) -> Arc<FixedClock> {
    Arc::new(FixedClock::at(at.parse::<DateTime<Utc>>().unwrap()))
}

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

async fn set_channels(app: &axum::Router, token: &str, channels: serde_json::Value) {
    let (status, body) = json(
        app,
        "PUT",
        "/settings",
        token,
        serde_json::json!({ "reminder_channels": channels }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Puts the account on a paid plan, subscription and all.
///
/// `users.plan_id` alone is not enough: `AuthUser::effective_plan` honours it only while the
/// subscription is active or in grace (A146), so a row without a subscription reads as free.
/// `email` is pro-and-up and `telegram`/`discord` are power-only, so the cases that exercise
/// those channels need this.
async fn grant_plan(harness: &TestState, user_id: Uuid, plan: &str) {
    for statement in [
        "update users set plan_id = $2 where id = $1",
        "insert into subscriptions
             (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly', now(), now() + interval '30 days')",
    ] {
        sqlx::query(statement)
            .bind(user_id)
            .bind(plan)
            .execute(&harness.state.pool)
            .await
            .unwrap();
    }
}

/// Registers a push token for the calling device, as the native shell does.
async fn register_push_token(app: &axum::Router, token: &str, device: Uuid, provider: &str) {
    let (status, body) = json(
        app,
        "PUT",
        &format!("/devices/{device}/push-token"),
        token,
        serde_json::json!({ "provider": provider, "token": format!("tok-{device}") }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

fn send_reminder_job(user_id: Uuid) -> ClaimedJob {
    ClaimedJob {
        id: Uuid::now_v7(),
        kind: "send_reminder".to_string(),
        payload: serde_json::json!({ "user_id": user_id }),
        attempts: 1,
    }
}

fn send_push_job(reminder_id: Uuid) -> ClaimedJob {
    ClaimedJob {
        id: Uuid::now_v7(),
        kind: "send_push".to_string(),
        payload: serde_json::json!({ "reminder_id": reminder_id }),
        attempts: 1,
    }
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn device_of(app: &axum::Router, email: &str, name: &str) -> (String, Uuid) {
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
    (
        body["access_token"].as_str().unwrap().to_string(),
        Uuid::parse_str(body["device_id"].as_str().unwrap()).unwrap(),
    )
}

async fn delivery_rows(harness: &TestState) -> Vec<(String, Option<Uuid>, String, Option<String>)> {
    sqlx::query_as(
        "select channel, device_id, status, reason from reminder_deliveries order by channel, device_id",
    )
    .fetch_all(&harness.state.pool)
    .await
    .unwrap()
}

/// A due reminder is delivered once, and the second run sends nothing: the claim is atomic
/// and the job's own `dedupe_key` is a second layer (A285).
#[sqlx::test(migrations = "../../migrations")]
async fn a_due_reminder_is_delivered_once(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "deliver-once@example.test";
    let (token, _device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;
    // `email` is pro-and-up on the matrix, so the channel list below needs a paid plan.
    grant_plan(&harness, user_id, "pro").await;

    // Due at 09:00Z, so the task reminder fires at 08:00Z — now.
    a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    set_channels(&app, &token, serde_json::json!(["web", "email"])).await;
    let baseline_emails = harness.mailer.sent().len();

    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .expect("the derivation");

    let reminder: (Uuid, String) =
        sqlx::query_as("select id, status from reminders order by created_at limit 1")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(reminder.1, "sent", "the claim moved it to sent");

    // The claim stamps `sent_at` from the **injected** clock, not the database's `now()`.
    // A claim stamped with `now()` would record a `sent_at` from a different clock than the one
    // that decided the reminder was due, which is the two-clock split a sibling found in
    // `calendar_pass` — and it makes this assertion impossible, which is the tell.
    let sent_at: DateTime<Utc> = sqlx::query_scalar("select sent_at from reminders where id = $1")
        .bind(reminder.0)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        sent_at,
        harness.state.clock.now(),
        "the reminder is sent at the instant the sweep ran"
    );

    // One `send_push` job, and running it delivers on both channels.
    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_push'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1, "one delivery job for one claimed reminder");

    handlers::reminders::deliver(&harness.state, &send_push_job(reminder.0))
        .await
        .expect("the delivery");
    let first = delivery_rows(&harness).await;
    assert_eq!(first.len(), 2, "{first:?}");
    assert!(
        first.iter().all(|(_, _, status, _)| status == "sent"),
        "{first:?}"
    );

    // The second run of the *same* delivery job sends nothing: every channel is settled.
    handlers::reminders::deliver(&harness.state, &send_push_job(reminder.0))
        .await
        .expect("a re-run is a no-op");
    assert_eq!(
        delivery_rows(&harness).await.len(),
        2,
        "a re-run must not add a delivery"
    );

    // And the second derivation claims nothing, because the reminder is no longer pending.
    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .expect("the second derivation");
    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_push'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1, "a sent reminder is not claimed again");
    assert_eq!(delivery_rows(&harness).await.len(), 2);

    // The email went through the real mailer double, so the channel is not merely recorded.
    // A delta, not a total: `RecordingMailer` also captured the signup verification link, and
    // asserting on the total would be asserting about the fixture rather than the reminder.
    assert_eq!(
        harness.mailer.sent().len() - baseline_emails,
        1,
        "one email for one reminder"
    );
}

/// Push goes to the single most recently active device with a token (A285), which is the
/// spec's "deliver to the most recently active device".
#[sqlx::test(migrations = "../../migrations")]
async fn push_goes_to_the_most_recently_active_device(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "push-device@example.test";
    let (token, device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;

    let (second_token, second_device) = device_of(&app, email, "Jetta's iPhone").await;
    register_push_token(&app, &token, device, "apns").await;
    register_push_token(&app, &second_token, second_device, "apns").await;

    // The second device is the most recently active: a sync pull is what proves liveness.
    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &second_token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let scripted = Arc::new(ScriptedPush::new(PushPlatform::Apns));
    let harness = harness.with_push(Arc::new(PushRouter::with(Some(scripted.clone()), None)));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    set_channels(&app, &token, serde_json::json!(["push"])).await;

    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();
    let reminder: Uuid = sqlx::query_scalar("select id from reminders limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    handlers::reminders::deliver(&harness.state, &send_push_job(reminder))
        .await
        .unwrap();

    // Exactly one send, to the most recently active device's token.
    let sends = scripted.sends();
    assert_eq!(sends.len(), 1, "push goes to one device, not all of them");
    assert_eq!(
        sends[0].0,
        format!("tok-{second_device}"),
        "the most recently active device is the one that synced last"
    );
    // The payload carries the reminder id, which is what the client uses to suppress a
    // notification it has already shown (A285's fallback half).
    assert_eq!(sends[0].1.data["reminder_id"], reminder.to_string());
    assert_eq!(sends[0].1.data["kind"], "task_due");
    assert_eq!(
        sends[0].1.collapse_key.as_deref(),
        Some(format!("reminder-{reminder}").as_str()),
        "a reminder that fires once should read once"
    );

    // The delivery row names the device, which is what makes "delivered to device B" a fact.
    let rows = delivery_rows(&harness).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, Some(second_device));
    assert_eq!(rows[0].2, "sent");
}

/// A provider whose *first* token is dead and whose later ones work.
///
/// `ScriptedPush` cannot express this: it either succeeds or fails for every token, so with it
/// "the next device is tried" and "both tokens were dead" are the same observation. This double
/// is what makes the assertion sharp — a dead token must delete exactly one row and leave the
/// live one alone, which is the whole distinction `PushError::Unregistered` exists for.
struct FirstTokenDead {
    dead: String,
    sends: std::sync::Mutex<Vec<String>>,
}

impl FirstTokenDead {
    /// The tokens attempted, in order. A poisoned lock is read through rather than unwrapped:
    /// a test double has no business turning a peer thread's panic into its own.
    fn attempted(&self) -> Vec<String> {
        match self.sends.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[async_trait::async_trait]
impl stvdy_core::traits::push::PushProvider for FirstTokenDead {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Apns
    }

    async fn send(
        &self,
        token: &str,
        _msg: stvdy_core::traits::push::PushMessage,
    ) -> Result<(), PushError> {
        match self.sends.lock() {
            Ok(mut guard) => guard.push(token.to_string()),
            Err(poisoned) => poisoned.into_inner().push(token.to_string()),
        }
        if token == self.dead {
            Err(PushError::Unregistered)
        } else {
            Ok(())
        }
    }
}

/// A dead token is deleted and the next device is tried, because a dead token is not an
/// outage.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unregistered_token_is_deleted_and_the_next_device_is_tried(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "dead-token@example.test";
    let (token, first_device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;

    let (second_token, second_device) = device_of(&app, email, "Jetta's iPhone").await;
    register_push_token(&app, &token, first_device, "apns").await;
    register_push_token(&app, &second_token, second_device, "apns").await;

    // The first device is the most recently active, and its token is dead.
    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let scripted = Arc::new(FirstTokenDead {
        dead: format!("tok-{first_device}"),
        sends: std::sync::Mutex::new(Vec::new()),
    });
    let harness = harness.with_push(Arc::new(PushRouter::with(Some(scripted.clone()), None)));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    set_channels(&app, &token, serde_json::json!(["push"])).await;

    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();
    let reminder: Uuid = sqlx::query_scalar("select id from reminders limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    handlers::reminders::deliver(&harness.state, &send_push_job(reminder))
        .await
        .expect("the second device's send succeeded, so this is not a total failure");

    // Both devices were tried, in most-recently-active order, and the first device's token row
    // is gone while the second's is untouched.
    assert_eq!(
        scripted.attempted(),
        vec![
            format!("tok-{first_device}"),
            format!("tok-{second_device}")
        ],
        "a dead token means try the next device, in order"
    );
    let remaining: Vec<Uuid> = sqlx::query_scalar("select device_id from push_tokens")
        .fetch_all(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        remaining,
        vec![second_device],
        "only the dead token is deleted"
    );

    // The delivery row names the device that actually took the push, so the record says what
    // happened rather than what was attempted.
    let rows = delivery_rows(&harness).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, Some(second_device));
    assert_eq!(rows[0].2, "sent");

    // A *transport* failure must not delete anything: that is the whole distinction, and it is
    // asserted separately because it is the one a caller can get wrong in the direction that
    // silently unregisters a phone that was merely asleep.
    let transport = Arc::new(ScriptedPush::failing(
        PushPlatform::Apns,
        PushError::Transport("the vendor is unreachable".to_string()),
    ));
    let harness = harness.with_push(Arc::new(PushRouter::with(Some(transport), None)));

    let reminder2 = Uuid::now_v7();
    sqlx::query(
        "insert into reminders (id, user_id, kind, subject_key, local_date, fire_at, dedupe_key)
         values ($1, $2, 'task_due', 'subject-2', '2026-10-12', now(), 'task_due:subject-2:2026-10-12')",
    )
    .bind(reminder2)
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    // Every attempt fails, so the job reports a transient failure: a reminder that reached
    // nobody is retryable, and a silent `Ok` would lose it.
    handlers::reminders::deliver(&harness.state, &send_push_job(reminder2))
        .await
        .expect_err("every attempted channel failed, so the job is retryable");

    let still_there: i64 = sqlx::query_scalar("select count(*) from push_tokens")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        still_there, 1,
        "an outage must not unregister a device whose phone was merely asleep"
    );
}

/// Quiet hours defer; the delivery happens after the window with the same key.
#[sqlx::test(migrations = "../../migrations")]
async fn quiet_hours_defer_rather_than_drop(pool: PgPool) {
    // 2026-10-11T15:30Z is 22:30 in Jakarta, inside a 22:00→07:00 window.
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-11T15:30:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "quiet-jobs@example.test";
    let (token, _device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;

    let (status, body) = json(
        &app,
        "PUT",
        "/settings",
        &token,
        serde_json::json!({ "quiet_start_hour": 22, "quiet_end_hour": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Due at 16:30Z, so the task reminder would fire at 15:30Z — inside the window.
    a_task(&app, &token, "2026-10-11T16:30:00Z").await;

    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();

    let (key, fire_at, status, skip_reason): (String, DateTime<Utc>, String, Option<String>) =
        sqlx::query_as("select dedupe_key, fire_at, status, skip_reason from reminders")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    // Deferred to 07:00 the next morning Jakarta, which is midnight UTC, and still pending.
    assert_eq!(fire_at.to_rfc3339(), "2026-10-12T00:00:00+00:00");
    assert_eq!(status, "pending", "a deferral is not a skip");
    assert_eq!(skip_reason, None);
    assert!(key.ends_with(":2026-10-11"), "the key does not move: {key}");

    // Nothing is claimed now: the reminder is not due yet, which is the whole point of a
    // deferral rather than a delivery inside the quiet window.
    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_push'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 0, "a deferred reminder is not delivered early");

    // The same account's next sweep, after the window, claims it — and its key is unchanged.
    let harness = harness.with_clock(clock_at("2026-10-12T00:30:00Z"));
    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();

    let (after_key, after_status): (String, String) =
        sqlx::query_as("select dedupe_key, status from reminders")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(after_key, key, "the key survived the deferral");
    assert_eq!(after_status, "sent");
}

/// Email and in-app ride the same run, and a channel with no transport is recorded rather
/// than silently skipped (A288).
#[sqlx::test(migrations = "../../migrations")]
async fn every_enabled_channel_is_attempted_and_recorded(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "channels-jobs@example.test";
    let (token, _device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;

    // A plan that grants telegram and discord, and a settings row that selects them. The
    // account is put on power so the settings write is not refused — the plan matrix is the
    // authority and `PUT /settings` checks it (A201).
    grant_plan(&harness, user_id, "power").await;
    set_channels(
        &app,
        &token,
        serde_json::json!(["web", "email", "telegram", "discord"]),
    )
    .await;
    let baseline_emails = harness.mailer.sent().len();

    a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();
    let reminder: Uuid = sqlx::query_scalar("select id from reminders limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    handlers::reminders::deliver(&harness.state, &send_push_job(reminder))
        .await
        .unwrap();

    let rows = delivery_rows(&harness).await;
    let channels: Vec<&str> = rows
        .iter()
        .map(|(channel, _, _, _)| channel.as_str())
        .collect();
    assert_eq!(
        channels,
        vec!["discord", "email", "telegram", "web"],
        "every enabled channel is recorded: {rows:?}"
    );
    for (channel, device, status, reason) in &rows {
        match channel.as_str() {
            "web" | "email" => assert_eq!(status, "sent", "{channel} must deliver"),
            "telegram" | "discord" => {
                assert_eq!(status, "skipped", "{channel}");
                assert_eq!(
                    reason.as_deref(),
                    Some("channel_not_implemented"),
                    "a Power user who selected one must see a status rather than silence"
                );
            }
            other => panic!("unexpected channel {other}"),
        }
        assert_eq!(*device, None, "an account-level channel has no device");
    }
    assert_eq!(
        harness.mailer.sent().len() - baseline_emails,
        1,
        "one reminder email, and the signup link is not counted"
    );

    // `push` was not enabled, so it is absent rather than recorded as a skip: the settings row
    // is the list of channels the student chose.
    assert!(!channels.contains(&"push"), "{rows:?}");
}

/// One channel failing does not re-send the ones that worked: the job returns Ok, and the
/// transient failure is only a transient failure when every channel failed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_failure_does_not_resend_the_rest(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "partial@example.test";
    let (token, device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;
    grant_plan(&harness, user_id, "pro").await;
    register_push_token(&app, &token, device, "apns").await;
    set_channels(&app, &token, serde_json::json!(["web", "email", "push"])).await;

    // The vendor is unreachable: a transport failure, so nothing is deleted and the reminder
    // is retryable — but the channels that worked must not be re-attempted.
    let scripted = Arc::new(ScriptedPush::failing(
        PushPlatform::Apns,
        PushError::Transport("the vendor is unreachable".to_string()),
    ));
    let harness = harness.with_push(Arc::new(PushRouter::with(Some(scripted.clone()), None)));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();
    let reminder: Uuid = sqlx::query_scalar("select id from reminders limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    handlers::reminders::deliver(&harness.state, &send_push_job(reminder))
        .await
        .expect("one channel failing is not every channel failing");

    let rows = delivery_rows(&harness).await;
    assert_eq!(rows.len(), 3, "{rows:?}");
    let web = rows.iter().find(|(c, _, _, _)| c == "web").unwrap();
    assert_eq!(web.2, "sent");
    let email_row = rows.iter().find(|(c, _, _, _)| c == "email").unwrap();
    assert_eq!(email_row.2, "sent");
    let push = rows.iter().find(|(c, _, _, _)| c == "push").unwrap();
    assert_eq!(push.2, "failed");
    assert!(
        push.3.is_none(),
        "a failure carries its error, not a reason"
    );

    // The retry re-attempts only push: web and email are settled, so the retry's "every
    // channel failed" is about the one channel it tried. That is the point of returning a
    // transient failure per *attempted* channel rather than per configured one — a settled
    // channel cannot make a retry look successful, and it cannot be re-sent either.
    let before_emails = harness.mailer.sent().len();
    let retry = handlers::reminders::deliver(&harness.state, &send_push_job(reminder)).await;
    assert!(
        retry.is_err(),
        "the only attempted channel failed, so the retry is retryable"
    );
    assert_eq!(
        harness.mailer.sent().len(),
        before_emails,
        "a settled channel must not be re-sent"
    );
    assert_eq!(
        scripted.sends().len(),
        2,
        "only the failed channel is re-attempted"
    );
    let rows = delivery_rows(&harness).await;
    assert_eq!(
        rows.len(),
        3,
        "a retry updates its own row rather than adding one: {rows:?}"
    );
}

/// The tick is idempotent: two passes in the same window enqueue one job.
#[sqlx::test(migrations = "../../migrations")]
async fn the_tick_is_idempotent_within_its_window(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signup(&app, &harness, "tick@example.test", "Mac").await;

    let first = stvdy_server::jobs::schedule::reminder_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(first, 1, "one account, one derivation");
    let second = stvdy_server::jobs::schedule::reminder_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(
        second, 0,
        "the account already has one queued in this window"
    );

    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_reminder'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1, "one job, not two");

    // The next window is a new key, so the pass runs again — which is what makes the cadence
    // the ticker rather than the key.
    let later = harness.with_clock(clock_at("2026-10-12T08:05:00Z"));
    let third = stvdy_server::jobs::schedule::reminder_pass(&later.state)
        .await
        .unwrap();
    assert_eq!(third, 1, "the next window enqueues again");
    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_reminder'")
        .fetch_one(&later.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 2);
}

/// A payload with no readable `reminder_id` is a **permanent** failure, not a transient one.
///
/// A malformed payload will never become well-formed, and a transient failure would retry it
/// five times with exponential backoff and then park it — a retry storm for a message that was
/// never going to arrive.
#[sqlx::test(migrations = "../../migrations")]
async fn a_malformed_payload_fails_permanently(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    for payload in [
        serde_json::json!({}),
        serde_json::json!({ "reminder_id": "not-a-uuid" }),
        serde_json::json!({ "reminder_id": null }),
    ] {
        let job = ClaimedJob {
            id: Uuid::now_v7(),
            kind: "send_push".to_string(),
            payload: payload.clone(),
            attempts: 1,
        };
        let failure = handlers::reminders::deliver(&harness.state, &job)
            .await
            .expect_err("a payload with no reminder_id must fail");
        assert!(
            !failure.retryable,
            "a malformed payload is permanent, not retryable: {payload} -> {failure}"
        );
    }

    // A well-formed id for a row that does not exist is a *success*: the reminder is gone
    // (the account was deleted, or the row was pruned), and no retry could find it.
    let job = send_push_job(Uuid::now_v7());
    handlers::reminders::deliver(&harness.state, &job)
        .await
        .expect("a vanished reminder is not a failure");

    // And a `send_reminder` with no user id is permanent for the same reason.
    let job = ClaimedJob {
        id: Uuid::now_v7(),
        kind: "send_reminder".to_string(),
        payload: serde_json::json!({}),
        attempts: 1,
    };
    let failure = handlers::reminders::run(&harness.state, &job)
        .await
        .expect_err("a payload with no user_id must fail");
    assert!(!failure.retryable, "{failure}");
}

/// A producer that is not the engine owns its own copy: the payload's `title`/`body` are sent
/// verbatim (A300's renewal reminders ride this path).
#[sqlx::test(migrations = "../../migrations")]
async fn a_producers_own_copy_is_sent_verbatim(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "producer@example.test";
    let (token, device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;
    register_push_token(&app, &token, device, "apns").await;
    set_channels(&app, &token, serde_json::json!(["push"])).await;

    let scripted = Arc::new(ScriptedPush::new(PushPlatform::Apns));
    let harness = harness.with_push(Arc::new(PushRouter::with(Some(scripted.clone()), None)));

    // A renewal-shaped row: `kind` is free text, and the producer supplies the copy.
    let reminder = Uuid::now_v7();
    sqlx::query(
        "insert into reminders (id, user_id, kind, subject_key, local_date, fire_at, dedupe_key)
         values ($1, $2, 'renewal', 'subscription-1', '2026-10-12', now(),
                 'renewal_reminder:subscription-1:2026-11-01:7')",
    )
    .bind(reminder)
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let job = ClaimedJob {
        id: Uuid::now_v7(),
        kind: "send_push".to_string(),
        payload: serde_json::json!({
            "reminder_id": reminder,
            "title": "Your plan ends on 1 November.",
            "body": "Renew to keep your Power features."
        }),
        attempts: 1,
    };
    handlers::reminders::deliver(&harness.state, &job)
        .await
        .unwrap();

    let sends = scripted.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].1.title, "Your plan ends on 1 November.");
    assert_eq!(sends[0].1.body, "Renew to keep your Power features.");
    assert_eq!(sends[0].1.data["kind"], "renewal");
    assert_eq!(sends[0].1.data["reminder_id"], reminder.to_string());

    // And a row with no copy falls back to the engine's own text rather than an empty string,
    // which is what a producer that forgot to supply one would otherwise send.
    let bare = Uuid::now_v7();
    sqlx::query(
        "insert into reminders (id, user_id, kind, subject_key, local_date, fire_at, dedupe_key)
         values ($1, $2, 'renewal', 'subscription-2', '2026-10-12', now(),
                 'renewal_reminder:subscription-2:2026-11-01:7')",
    )
    .bind(bare)
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    handlers::reminders::deliver(&harness.state, &send_push_job(bare))
        .await
        .unwrap();
    let sends = scripted.sends();
    assert_eq!(sends.len(), 2);
    assert!(
        !sends[1].1.title.is_empty() && !sends[1].1.body.is_empty(),
        "a producer with no copy must not send an empty notification"
    );
}

/// A reminder is derived once across a re-run even when a client registered it first: the
/// engine's `on conflict (dedupe_key) do nothing` keeps the client's row and its fire time.
#[sqlx::test(migrations = "../../migrations")]
async fn a_client_registered_reminder_is_not_re_derived(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "client-first@example.test";
    let (token, _device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;

    let task = a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    // The client computed 08:30Z rather than the engine's 08:00Z.
    let (status, body) = json(
        &app,
        "POST",
        "/reminders",
        &token,
        serde_json::json!({
            "kind": "task_due", "subject_id": task, "fire_at": "2026-10-12T08:30:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();

    let rows: Vec<(String, DateTime<Utc>, String)> =
        sqlx::query_as("select dedupe_key, fire_at, origin from reminders")
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1, "one reminder, not two: {rows:?}");
    assert_eq!(
        rows[0].1.to_rfc3339(),
        "2026-10-12T08:30:00+00:00",
        "the client's row keeps its fire time"
    );
    assert_eq!(rows[0].2, "client");

    // It is not due yet at 08:00Z, so nothing was claimed.
    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_push'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 0);

    // And at 08:31Z the sweep claims the client's own row.
    let later = harness.with_clock(clock_at("2026-10-12T08:31:00Z"));
    handlers::reminders::run(&later.state, &send_reminder_job(user_id))
        .await
        .unwrap();
    let queued: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'send_push'")
        .fetch_one(&later.state.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1);
}

/// `devices.last_seen_at` is what "the most recently active device" reads, and the reminder
/// path never writes it: a reminder is not evidence that a device is alive.
///
/// This is the distinction the two columns exist for. `last_seen_at` refreshes when a device
/// *syncs* (`sync::touch_last_sync`), and `last_sync_at` is the Devices screen's "Last synced".
/// A push sent to a phone that is switched off must not make it look like the most recently
/// active device, or every reminder after the first would go to whichever phone was last
/// pushed to rather than the one the student is holding.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reminder_does_not_mark_a_device_seen(pool: PgPool) {
    let harness = TestState::new(pool).with_clock(clock_at("2026-10-12T08:00:00Z"));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let email = "last-seen@example.test";
    let (token, device) = signup(&app, &harness, email, "Mac").await;
    let user_id = user_id_of(&harness, email).await;
    register_push_token(&app, &token, device, "apns").await;

    let before: Option<DateTime<Utc>> =
        sqlx::query_scalar("select last_seen_at from devices where id = $1")
            .bind(device)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(before.is_some(), "signup records the device");

    let scripted = Arc::new(ScriptedPush::new(PushPlatform::Apns));
    let harness = harness.with_push(Arc::new(PushRouter::with(Some(scripted.clone()), None)));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    a_task(&app, &token, "2026-10-12T09:00:00Z").await;
    set_channels(&app, &token, serde_json::json!(["push"])).await;
    handlers::reminders::run(&harness.state, &send_reminder_job(user_id))
        .await
        .unwrap();
    let reminder: Uuid = sqlx::query_scalar("select id from reminders limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    handlers::reminders::deliver(&harness.state, &send_push_job(reminder))
        .await
        .unwrap();
    assert_eq!(scripted.sends().len(), 1, "the push went somewhere");

    let after: Option<DateTime<Utc>> =
        sqlx::query_scalar("select last_seen_at from devices where id = $1")
            .bind(device)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(before, after, "a reminder job does not mark a device seen");

    // A sync pull does write it, which is the one writer the column has.
    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let synced: Option<DateTime<Utc>> =
        sqlx::query_scalar("select last_seen_at from devices where id = $1")
            .bind(device)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        synced >= before,
        "a completed pull is what refreshes it: {before:?} -> {synced:?}"
    );
}
