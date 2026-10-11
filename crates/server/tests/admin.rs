//! Admin: the manual plan switch, the manual credit grant, and the margin scaffold.
//!
//! Three things this suite exists to catch, each invisible to a reading:
//!
//!  * a plan switch that writes `users.plan_id` but not the `subscriptions` row, which is
//!    how a comped account ends up on the free limits (A146) while the admin believes it
//!    was upgraded;
//!  * a manual grant that a double-click applies twice, which is what the period key and
//!    the partial unique index exist to make impossible (A152);
//!  * a margin report that reports `0` for revenue and conversion, which reads as a
//!    measurement when there is nothing to measure yet (A46/gate 7(a)).

// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Phase 0 Task 1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use stvdy_core::config::PlanId;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// A signed-up account whose `is_admin` is set. The flag is read from the row on every
/// request, so the token issued at signup works as an admin without re-login.
async fn admin_token(app: &axum::Router, harness: &TestState, email: &str) -> String {
    let token = signed_up(app, harness, email).await;
    sqlx::query("update users set is_admin = true where email = $1")
        .bind(email)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    token
}

/// A plan switch takes effect on the next request and recomputes the read-only flags,
/// because a downgrade and a switch are the same event from the data's point of view.
///
/// The account starts on `power` with three workspaces — over free's two — so switching to
/// free must flag exactly the newest one, and the *next* request must meet the free plan's
/// limit rather than the stored plan's.
#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_plan_switch_takes_effect_and_recomputes_read_only(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let admin = admin_token(&app, &harness, "admin@example.test").await;
    let target = signed_up(&app, &harness, "target@example.test").await;
    let target_id = user_id_of(&harness, "target@example.test").await;

    // Three workspaces, written directly: the API would refuse the third on free, and the
    // point of the case is the downgrade of an account that already has them.
    for index in 0..3 {
        sqlx::query("insert into workspaces (user_id, name) values ($1, $2)")
            .bind(target_id)
            .bind(format!("Workspace {index}"))
            .execute(&harness.state.pool)
            .await
            .unwrap();
    }

    // A switch to power: unlimited workspaces, so nothing is flagged.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/admin/users/{target_id}/plan"),
        &admin,
        serde_json::json!({ "plan": "power", "period_days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["plan"], "power", "{body}");

    // Both rows moved, which is the whole reason the handler writes two.
    let stored: String = sqlx::query_scalar("select plan_id from users where id = $1")
        .bind(target_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(stored, "power");
    let subscription: (String, String) = sqlx::query_as(
        "select plan_id, status from subscriptions
          where user_id = $1 and status in ('active', 'grace')",
    )
    .bind(target_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        subscription,
        ("power".to_string(), "active".to_string()),
        "a plan without a live subscription grants nothing (A146)"
    );

    let flagged: i64 = sqlx::query_scalar(
        "select count(*)::bigint from workspaces where user_id = $1 and read_only",
    )
    .bind(target_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(flagged, 0, "power allows unlimited workspaces");

    // The switch back down is the same event: free allows two, so the newest of the three
    // is flagged, and the account's next request meets the free limit.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/admin/users/{target_id}/plan"),
        &admin,
        serde_json::json!({ "plan": "free" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["read_only"]["workspaces"], 1, "{body}");

    let flagged: i64 = sqlx::query_scalar(
        "select count(*)::bigint from workspaces where user_id = $1 and read_only",
    )
    .bind(target_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(flagged, 1, "the newest over-limit workspace is read-only");

    let snapshot = stvdy_server::entitlements::snapshot_for(&harness.state, target_id)
        .await
        .unwrap();
    assert_eq!(snapshot.plan, PlanId::Free);

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &target,
        serde_json::json!({ "name": "One too many" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the switch takes effect on the next request: {body}"
    );
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 2, "{body}");
}

/// A manual grant is one `credit_ledger` row, and it is idempotent per admin-supplied
/// key, so a double-click cannot grant twice (A152).
#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_grant_writes_one_admin_ledger_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let admin = admin_token(&app, &harness, "admin-grant@example.test").await;
    signed_up(&app, &harness, "grantee@example.test").await;
    let grantee = user_id_of(&harness, "grantee@example.test").await;

    let key = "double-click-7f2a";
    let period_key = format!("admin:{key}");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/admin/users/{grantee}/credits"),
        &admin,
        serde_json::json!({ "credits": 50, "bucket": "monthly", "idempotency_key": key }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["granted"], true, "{body}");

    let row: (i32, String, String) = sqlx::query_as(
        "select delta, bucket, reason from credit_ledger
          where user_id = $1 and period_key = $2",
    )
    .bind(grantee)
    .bind(&period_key)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(row.0, 50);
    assert_eq!(row.1, "monthly");
    // `credits::grant` is the one grant primitive in this phase, and it writes the
    // monthly bucket under `monthly_grant`; the key is what makes the row once-only.
    assert_eq!(row.2, "monthly_grant");

    // The double-click: the same key is a no-op, so the balance does not move.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/admin/users/{grantee}/credits"),
        &admin,
        serde_json::json!({ "credits": 50, "bucket": "monthly", "idempotency_key": key }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["granted"], false,
        "the second call grants nothing: {body}"
    );

    let rows: i64 = sqlx::query_scalar(
        "select count(*)::bigint from credit_ledger where user_id = $1 and period_key = $2",
    )
    .bind(grantee)
    .bind(&period_key)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "one row per idempotency key");

    let granted: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and period_key = $2",
    )
    .bind(grantee)
    .bind(&period_key)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(granted, 50, "the balance moved once, not twice");
}

/// **The scaffolding is honest about what it cannot compute yet.** Provider cost splits
/// free from paid (join `usage_events` → `users.plan_id`), with BYOK in its own column
/// (A156). Revenue and conversion are `null` until Phase 13 writes `payments` rows —
/// a `0` there would be an invented number, and A46's tripwires are read off this report
/// (roadmap gate 7(a)).
#[sqlx::test(migrations = "../../migrations")]
async fn the_margin_report_splits_free_from_paid_and_nulls_what_it_cannot_compute(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let admin = admin_token(&app, &harness, "admin-margin@example.test").await;

    let _free = signed_up(&app, &harness, "free-margin@example.test").await;
    let free_id = user_id_of(&harness, "free-margin@example.test").await;
    let _paid = signed_up(&app, &harness, "paid-margin@example.test").await;
    let paid_id = user_id_of(&harness, "paid-margin@example.test").await;

    // The paid account is on pro, with a live subscription: the report reads the plan the
    // account is on, not the plan it signed up with.
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(paid_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(paid_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // One free call, one paid call, and one BYOK call on the paid account.
    for (user_id, cost, byok) in [
        (free_id, 0.25_f64, false),
        (paid_id, 1.50_f64, false),
        (paid_id, 0.10_f64, true),
    ] {
        sqlx::query(
            "insert into usage_events (user_id, action, credits, provider_cost_usd, byok)
             values ($1, 'chat_message', 1, $2, $3)",
        )
        .bind(user_id)
        .bind(cost)
        .bind(byok)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    let from = Utc::now() - Duration::days(1);
    let to = Utc::now() + Duration::days(1);
    let report = stvdy_server::admin::margin_report(&harness.state.pool, from, to)
        .await
        .unwrap();

    let close = |left: f64, right: f64| (left - right).abs() < 1e-9;
    assert!(
        close(report.provider_cost.free_usd, 0.25),
        "{:?}",
        report.provider_cost
    );
    assert!(
        close(report.provider_cost.paid_usd, 1.50),
        "{:?}",
        report.provider_cost
    );
    assert!(
        close(report.provider_cost.byok_usd, 0.10),
        "BYOK is its own column (A156): {:?}",
        report.provider_cost
    );

    // Per plan, the same split, and BYOK is not inside either tier's figure.
    let free_row = report
        .per_plan
        .iter()
        .find(|row| row.plan_id == "free")
        .expect("the free plan's row");
    assert!(close(free_row.provider_cost_usd, 0.25), "{free_row:?}");
    let paid_row = report
        .per_plan
        .iter()
        .find(|row| row.plan_id == "pro")
        .expect("the paid plan's row");
    assert!(close(paid_row.provider_cost_usd, 1.50), "{paid_row:?}");
    assert!(close(paid_row.byok_cost_usd, 0.10), "{paid_row:?}");

    // What cannot be computed yet is `null`, never `0`.
    assert!(report.revenue.is_none(), "no `payments` rows exist yet");
    assert!(
        report.conversion.is_none(),
        "a conversion of 0 would be an invented number (gate 7(a))"
    );

    // And the same shape over the wire.
    let (status, body) = json(
        &app,
        "GET",
        &format!(
            "/admin/margin?from={}&to={}",
            from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ),
        &admin,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["revenue"].is_null(), "{body}");
    assert!(body["conversion"].is_null(), "{body}");
    assert!(body["provider_cost"]["free_usd"].is_number(), "{body}");
}

/// Every admin route is 404 for a non-admin (A7).
///
/// 404, not 403: whether an admin endpoint exists is not something a non-admin needs to
/// learn. The admin reads the same routes successfully, so the 404s are the extractor and
/// not a broken table.
#[sqlx::test(migrations = "../../migrations")]
async fn a_non_admin_sees_not_found(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let user = signed_up(&app, &harness, "nonadmin@example.test").await;
    let _ = signed_up(&app, &harness, "victim@example.test").await;
    let target_id = user_id_of(&harness, "victim@example.test").await;
    let admin = admin_token(&app, &harness, "admin-routes@example.test").await;

    let requests: [(&str, String, serde_json::Value); 3] = [
        (
            "PATCH",
            format!("/admin/users/{target_id}/plan"),
            serde_json::json!({ "plan": "pro" }),
        ),
        (
            "POST",
            format!("/admin/users/{target_id}/credits"),
            serde_json::json!({ "credits": 10, "idempotency_key": "nonadmin" }),
        ),
        ("GET", "/admin/margin".to_string(), serde_json::Value::Null),
    ];

    for (method, uri, body) in &requests {
        let (status, body) = json(&app, method, uri, &user, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
        assert_eq!(body["code"], "not_found", "{method} {uri}: {body}");
    }

    // The admin is admitted, so the 404s above are the extractor and not a route that
    // does not exist.
    for (method, uri, body) in &requests {
        let (status, response) = json(&app, method, uri, &admin, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}: {response}");
    }
}
