//! The period rules after a payment: extension, proration, a downgrade that waits, and
//! the purchased bucket.
//!
//! Every case here is about *time*, so the period end is written from the test's own clock
//! rather than from `now()` in SQL: `upgrade_delta` prorates over `remaining_days`, and a
//! boundary computed by the database would be a fraction of a second short of the number
//! the assertion names.
//!
//! Three things this suite exists to catch, each invisible to a reading:
//!
//!  * an upgrade that moves `current_period_end`, which silently hands the student a free
//!    period on top of the proration they paid for;
//!  * a downgrade that takes effect immediately, which shortens time already paid for —
//!    the one thing A296 forbids;
//!  * a credit pack that lands in the monthly bucket, which the rollover then expires,
//!    taking away credits the student bought outright.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use stvdy_core::billing::BillingPeriod;
use stvdy_core::config::PlanId;
use stvdy_core::entitlements::SubStatus;
use stvdy_core::traits::payment::{WebhookEvent, WebhookKind};
use stvdy_server::auth::extract::AuthUser;
use stvdy_server::billing::apply::{Applied, apply_paid};
use stvdy_server::jobs::ClaimedJob;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::signed_up;

const PASSWORD: &str = "correct horse battery";

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

/// The caller the period functions see, read back from the row so the plan and the status
/// are the account's real ones rather than values the test hoped for.
async fn caller(harness: &TestState, user_id: Uuid, platform: &str) -> AuthUser {
    let (email, plan_id, timezone, student_verified): (String, String, String, bool) =
        sqlx::query_as(
            "select email, plan_id, timezone, student_verified from users where id = $1",
        )
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let status: Option<String> = sqlx::query_scalar(
        "select status from subscriptions
          where user_id = $1
          order by (status in ('active', 'grace')) desc, current_period_end desc
          limit 1",
    )
    .bind(user_id)
    .fetch_optional(&harness.state.pool)
    .await
    .unwrap();

    AuthUser {
        user_id,
        device_id: Uuid::now_v7(),
        session_id: Uuid::now_v7(),
        email,
        email_verified: true,
        plan_id,
        timezone,
        is_admin: false,
        student_verified,
        platform: platform.to_string(),
        subscription_status: SubStatus::parse(status.as_deref().unwrap_or("")),
    }
}

/// Gives the account a live subscription with an exact window, and `users.plan_id` to match.
async fn live_subscription(
    harness: &TestState,
    user_id: Uuid,
    plan: PlanId,
    status: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Uuid {
    sqlx::query("update users set plan_id = $2 where id = $1")
        .bind(user_id)
        .bind(plan.as_str())
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query_scalar(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, $3, 'monthly', $4, $5)
         returning id",
    )
    .bind(user_id)
    .bind(plan.as_str())
    .bind(status)
    .bind(start)
    .bind(end)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// A pending `payments` row, as checkout would have written it.
#[allow(clippy::too_many_arguments)]
async fn pending_payment(
    harness: &TestState,
    user_id: Uuid,
    subscription_id: Option<Uuid>,
    kind: &str,
    plan: Option<PlanId>,
    credits: Option<i32>,
    amount_idr: i64,
    expires_at: DateTime<Utc>,
) -> Uuid {
    sqlx::query_scalar(
        "insert into payments
            (user_id, subscription_id, kind, plan_id, billing_period, credits, provider,
             order_id, amount_idr, expires_at, status)
         values ($1, $2, $3, $4, 'monthly', $5, 'qris_gateway', $6, $7, $8, 'pending')
         returning id",
    )
    .bind(user_id)
    .bind(subscription_id)
    .bind(kind)
    .bind(plan.map(|plan| plan.as_str()))
    .bind(credits)
    .bind(format!("order-{}", Uuid::now_v7()))
    .bind(amount_idr)
    .bind(expires_at)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// A verified `paid` callback for `payment`, as the webhook would have parsed one.
fn paid_event(amount_idr: i64, paid_at: DateTime<Utc>) -> WebhookEvent {
    WebhookEvent {
        event_id: format!("evt-{}", Uuid::now_v7()),
        kind: WebhookKind::Paid,
        order_id: "order".to_string(),
        amount_idr,
        currency: "IDR".to_string(),
        paid_at: Some(paid_at),
        provider_payment_id: Some("gw-1".to_string()),
        fee_idr: Some(0),
        raw: json!({}),
    }
}

/// Confirms a payment through the one path the webhook and the poller share.
async fn confirm(harness: &TestState, payment_id: Uuid, event: &WebhookEvent) -> Applied {
    let mut conn = harness.state.pool.acquire().await.unwrap();
    apply_paid(&mut conn, &harness.state, payment_id, event)
        .await
        .expect("the apply runs")
}

async fn subscription_row(harness: &TestState, user_id: Uuid) -> (String, String, DateTime<Utc>) {
    sqlx::query_as(
        "select plan_id, status, current_period_end from subscriptions where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
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
    .unwrap()
}

/// Paying while there is live paid time extends from that end, so paying early never loses
/// days (A296).
#[sqlx::test(migrations = "../../migrations")]
async fn paying_while_active_extends_from_the_period_end(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "extend@example.test").await;
    let user_id = user_id_of(&harness, "extend@example.test").await;

    let now = harness.state.clock.now();
    let end = now + Duration::days(10);
    let subscription_id = live_subscription(
        &harness,
        user_id,
        PlanId::Pro,
        "active",
        now - Duration::days(20),
        end,
    )
    .await;

    let payment = pending_payment(
        &harness,
        user_id,
        Some(subscription_id),
        "subscription",
        Some(PlanId::Pro),
        None,
        10_000,
        now + Duration::minutes(15),
    )
    .await;

    let applied = confirm(&harness, payment, &paid_event(10_000, now)).await;
    assert!(
        matches!(applied, Applied::Extended { .. }),
        "a payment with live paid time extends: {applied:?}"
    );

    let (_, _, new_end) = subscription_row(&harness, user_id).await;
    assert_eq!(
        new_end,
        end + Duration::days(30),
        "the new period starts at the old end, so the ten remaining days are not lost"
    );
}

/// A payment with no live paid time starts a fresh period at `paid_at`: extending from an
/// end in the past would hand the student a period with days already spent.
#[sqlx::test(migrations = "../../migrations")]
async fn paying_after_expiry_starts_a_fresh_period_at_paid_at(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "fresh@example.test").await;
    let user_id = user_id_of(&harness, "fresh@example.test").await;

    let now = harness.state.clock.now();
    // The subscription lapsed 40 days ago, which is past the grace window, so the account
    // is on the free limits and `apply_payment` takes its "otherwise" branch.
    live_subscription(
        &harness,
        user_id,
        PlanId::Pro,
        "expired",
        now - Duration::days(70),
        now - Duration::days(40),
    )
    .await;

    let payment = pending_payment(
        &harness,
        user_id,
        None,
        "subscription",
        Some(PlanId::Pro),
        None,
        10_000,
        now + Duration::minutes(15),
    )
    .await;

    let applied = confirm(&harness, payment, &paid_event(10_000, now)).await;
    assert!(
        matches!(applied, Applied::Activated { .. }),
        "a lapsed account starts a fresh period: {applied:?}"
    );

    let (plan, status, end) = subscription_row(&harness, user_id).await;
    assert_eq!(plan, "pro");
    assert_eq!(status, "active", "paying reactivates the row");
    assert_eq!(
        end,
        now + Duration::days(30),
        "a fresh period runs from the payment, not from the stale end"
    );
}

/// An upgrade switches immediately and charges the prorated difference; the subscription's
/// end does not move, because the student has already paid for the days that remain.
#[sqlx::test(migrations = "../../migrations")]
async fn an_upgrade_switches_immediately_and_charges_the_prorated_difference(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "upgrade@example.test").await;
    let user_id = user_id_of(&harness, "upgrade@example.test").await;

    let now = harness.state.clock.now();
    let end = now + Duration::days(10);
    live_subscription(
        &harness,
        user_id,
        PlanId::Pro,
        "active",
        now - Duration::days(20),
        end,
    )
    .await;
    let who = caller(&harness, user_id, "macos").await;

    let view = stvdy_server::billing::periods::upgrade(
        &harness.state,
        &who,
        PlanId::Power,
        BillingPeriod::Monthly,
    )
    .await
    .expect("the upgrade opens an invoice");

    // (25000 - 10000) * 10 / 30 = 5000: the difference for the days that remain, which is
    // the same arithmetic `stvdy_core::billing::upgrade_delta` is tested on.
    assert_eq!(view.amount_idr, 5_000, "{view:?}");
    assert_eq!(view.plan_id.as_deref(), Some("power"));

    // The switch has not happened yet: until the money arrives the student is on the plan
    // they paid for, and moving the subscription now would make the payment row look like
    // a renewal.
    let (plan, _, still_end) = subscription_row(&harness, user_id).await;
    assert_eq!(
        plan, "pro",
        "checkout must not move the subscription's plan"
    );
    assert_eq!(still_end, end, "and must not move its end");

    let applied = confirm(&harness, view.id, &paid_event(5_000, now)).await;
    assert!(
        matches!(applied, Applied::Activated { .. }),
        "an upgrade activates rather than extending: {applied:?}"
    );

    let (plan, status, after_end) = subscription_row(&harness, user_id).await;
    assert_eq!(plan, "power", "the upgrade switched the plan");
    assert_eq!(status, "active");
    assert_eq!(
        after_end, end,
        "the period end must not move: the proration bought the remaining days at the \
         higher plan, not a second period"
    );

    // The entitlement decision sees power now, which is the point of the switch.
    let who = caller(&harness, user_id, "macos").await;
    assert_eq!(who.effective_plan(), PlanId::Power);
}

/// A downgrade is recorded, not applied: the plan changes at the period end and the paid
/// time is never shortened (A296).
#[sqlx::test(migrations = "../../migrations")]
async fn a_downgrade_waits_for_the_period_end_and_never_shortens_paid_time(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "downgrade@example.test").await;
    let user_id = user_id_of(&harness, "downgrade@example.test").await;

    let now = harness.state.clock.now();
    let end = now + Duration::days(10);
    live_subscription(
        &harness,
        user_id,
        PlanId::Power,
        "active",
        now - Duration::days(20),
        end,
    )
    .await;
    let who = caller(&harness, user_id, "macos").await;

    let change = stvdy_server::billing::periods::downgrade(
        &harness.state,
        &who,
        PlanId::Pro,
        BillingPeriod::Monthly,
    )
    .await
    .expect("the downgrade is recorded");

    assert!(change.pending, "a downgrade is a recorded intent");
    assert_eq!(
        change.effective_at, end,
        "it takes effect at the period end"
    );

    // Nothing about the account's paid time moved, and the plan is still the one paid for.
    let (plan, _, still_end) = subscription_row(&harness, user_id).await;
    assert_eq!(plan, "power", "the paid plan is untouched");
    assert_eq!(still_end, end, "and so is the paid time");
    let who = caller(&harness, user_id, "macos").await;
    assert_eq!(
        who.effective_plan(),
        PlanId::Power,
        "a recorded downgrade grants nothing away before the boundary"
    );

    // The rollover applies it once the period has ended. The end is moved into the past
    // rather than the clock moved forward, because `apply_pending_plan` compares against
    // the database's `now()`.
    sqlx::query("update subscriptions set current_period_end = $2 where user_id = $1")
        .bind(user_id)
        .bind(now - Duration::days(1))
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let applied = stvdy_server::billing::periods::apply_pending_plan(&harness.state, user_id)
        .await
        .expect("the rollover runs")
        .expect("a pending plan is applied");
    assert_eq!(applied, PlanId::Pro);

    let (plan, _, _) = subscription_row(&harness, user_id).await;
    assert_eq!(plan, "pro", "the subscription moved with the account");
    let stored: String = sqlx::query_scalar("select plan_id from users where id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, "pro", "and so did `users.plan_id`");

    let pending: Option<String> =
        sqlx::query_scalar("select pending_plan_id from subscriptions where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(pending.is_none(), "the pending columns are cleared");

    // A second call is a no-op: the rollover is idempotent.
    let again = stvdy_server::billing::periods::apply_pending_plan(&harness.state, user_id)
        .await
        .expect("the rollover runs again");
    assert!(again.is_none(), "a second rollover applies nothing");
}

/// The purchased bucket: a credit pack lands there, is never expired by the rollover, and
/// is spent after the monthly bucket (A9/A297).
#[sqlx::test(migrations = "../../migrations")]
async fn a_credit_pack_lands_in_the_purchased_bucket_and_survives_the_rollover(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "pack@example.test").await;
    let user_id = user_id_of(&harness, "pack@example.test").await;

    let now = harness.state.clock.now();
    // A live free account: the pack is bought with no subscription in the way.
    let payment = pending_payment(
        &harness,
        user_id,
        None,
        "credit_pack",
        None,
        Some(100),
        3_000,
        now + Duration::minutes(15),
    )
    .await;

    let applied = confirm(&harness, payment, &paid_event(3_000, now)).await;
    assert_eq!(applied, Applied::Credits { granted: 100 }, "{applied:?}");

    let monthly_before = bucket(&harness, user_id, "monthly").await;
    assert_eq!(
        bucket(&harness, user_id, "purchased").await,
        100,
        "the pack lands in the purchased bucket, not the monthly one"
    );

    // The ledger row names the payment that bought it, so a double grant is visible in the
    // report rather than looking like an ordinary top-up.
    let ref_id: Option<Uuid> = sqlx::query_scalar(
        "select ref_id from credit_ledger
          where user_id = $1 and reason = 'credit_pack' and bucket = 'purchased'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(ref_id, Some(payment), "the grant names its payment");

    // The rollover expires the *monthly* balance and leaves the purchase alone: credits
    // bought outright do not expire.
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    live_subscription(
        &harness,
        user_id,
        PlanId::Pro,
        "active",
        now - Duration::days(30),
        now - Duration::days(1),
    )
    .await;
    stvdy_server::jobs::handlers::counters::run(
        &harness.state,
        &ClaimedJob {
            id: Uuid::now_v7(),
            kind: "reset_counters".to_string(),
            payload: json!({ "user_id": user_id }),
            attempts: 1,
        },
    )
    .await
    .expect("the rollover runs");

    assert_eq!(
        bucket(&harness, user_id, "purchased").await,
        100,
        "the purchased bucket survives the period boundary"
    );
    assert!(
        bucket(&harness, user_id, "monthly").await <= monthly_before,
        "the monthly balance is not grown by a rollover that expired it"
    );
}

/// Two packs are two grants, and the ledger names each payment so a double grant is
/// visible rather than looking like an ordinary top-up.
#[sqlx::test(migrations = "../../migrations")]
async fn two_credit_packs_are_two_grants_each_naming_its_payment(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    signed_up(&app, &harness, "twopacks@example.test").await;
    let user_id = user_id_of(&harness, "twopacks@example.test").await;

    let now = harness.state.clock.now();
    let mut payments = Vec::new();
    for (credits, price) in [(100_i32, 3_000_i64), (500, 12_500)] {
        let payment = pending_payment(
            &harness,
            user_id,
            None,
            "credit_pack",
            None,
            Some(credits),
            price,
            now + Duration::minutes(15),
        )
        .await;
        confirm(&harness, payment, &paid_event(price, now)).await;
        payments.push(payment);
    }

    assert_eq!(bucket(&harness, user_id, "purchased").await, 600);

    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "select ref_id, delta from credit_ledger
          where user_id = $1 and reason = 'credit_pack'
          order by delta",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "two packs are two ledger rows");
    assert_eq!(rows[0].0, payments[0]);
    assert_eq!(rows[1].0, payments[1]);

    // A second report of the same payment grants nothing: the state transition guards it
    // (A293), and the period key would refuse a duplicate row even if it did not.
    let again = confirm(&harness, payments[0], &paid_event(3_000, now)).await;
    assert_eq!(again, Applied::AlreadyPaid, "a second report is a no-op");
    assert_eq!(
        bucket(&harness, user_id, "purchased").await,
        600,
        "and grants nothing"
    );
}
