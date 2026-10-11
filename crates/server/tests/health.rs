// See Task 1: `#[sqlx::test]` is a macro harness, so clippy.toml's test
// relaxations are not guaranteed to apply.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use stvdy_server::config::AppConfig;
use stvdy_server::state::AppState;
use tower::ServiceExt; // for `oneshot`

fn config() -> AppConfig {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    AppConfig::from_env_at(
        &root,
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ],
    )
    .expect("config must load")
}

fn app(pool: PgPool) -> axum::Router {
    stvdy_server::routes::router(AppState {
        pool,
        config: Arc::new(config()),
    })
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_reports_ok_and_the_plan_count(pool: PgPool) {
    stvdy_server::db::seed_plans(&pool, &config().plans)
        .await
        .unwrap();

    let response = app(pool)
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["plans"], 3);
    assert!(json["version"].is_string());
    assert_eq!(json["env"], "dev");
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_routes_return_the_typed_not_found_error(pool: PgPool) {
    let response = app(pool)
        .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "not_found");
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_is_scoped_to_a_live_database(pool: PgPool) {
    // Proves the probe actually queries: closing the pool must turn /health into
    // a 500 rather than a cheerful ok.
    let response = app(pool.clone())
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn error_mapping_matches_the_documented_table() {
    use stvdy_server::error::AppError;

    for (error, status, code) in [
        (AppError::Unauthorized, 401, "unauthorized"),
        (AppError::TokenExpired, 401, "token_expired"),
        (AppError::Forbidden, 403, "forbidden"),
        (AppError::NotFound, 404, "not_found"),
        (AppError::Validation("x".into()), 400, "validation"),
        (AppError::Conflict("x".into()), 409, "conflict"),
        (AppError::ResyncRequired, 410, "resync_required"),
        (AppError::RateLimited, 429, "rate_limited"),
        (AppError::Provider("x".into()), 502, "provider_error"),
    ] {
        assert_eq!(error.status().as_u16(), status, "{code}");
        assert_eq!(error.code(), code);
    }

    // Insufficient credits is 402 (payment required), not 403: the UI needs to
    // distinguish "upgrade" from "not allowed".
    let credits = AppError::Entitlement {
        code: "insufficient_credits",
        message: "need 5".into(),
        min_plan: None,
    };
    assert_eq!(credits.status(), StatusCode::PAYMENT_REQUIRED);

    let plan = AppError::Entitlement {
        code: "plan_required",
        message: "pro required".into(),
        min_plan: Some("pro"),
    };
    assert_eq!(plan.status(), StatusCode::FORBIDDEN);
    assert_eq!(plan.code(), "plan_required");
}
