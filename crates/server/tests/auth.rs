//! Phase 1 auth: signup, verification, login, refresh rotation, logout.
//!
//! The rotation tests are the point of the file. Refresh-token reuse detection
//! is the property that makes a stolen token survivable, and it is invisible to
//! a happy-path test: it only shows up when an *already rotated* token is
//! presented again.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Phase 0 Task 1).
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use tower::ServiceExt;

fn json_body(value: serde_json::Value) -> Body {
    Body::from(serde_json::to_vec(&value).expect("serialize"))
}

async fn post(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(json_body(body))
                .expect("request"),
        )
        .await
        .expect("response");

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let value: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

async fn post_auth(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(json_body(body))
                .expect("request"),
        )
        .await
        .expect("response");

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let value: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

async fn get_auth(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let value: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

fn signup_body(email: &str, device: &str) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": "correct horse battery",
        "device": { "name": device, "platform": "macos", "app_version": "0.1.0" }
    })
}

/// The full happy path: signup creates an unverified account and mails a link,
/// the link verifies it, and the token pair works.
#[sqlx::test(migrations = "../../migrations")]
async fn signup_verifies_by_email_and_issues_a_working_token_pair(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, body) = post(
        &app,
        "/auth/signup",
        signup_body("a@example.test", "Laptop"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["user"]["email"], "a@example.test");
    assert_eq!(
        body["user"]["email_verified"], false,
        "a new account is unverified: A43 gates spending on it"
    );
    assert_eq!(body["user"]["plan_id"], "free");

    let access = body["access_token"]
        .as_str()
        .expect("access token")
        .to_string();
    let refresh = body["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_string();
    assert_ne!(access, refresh);

    // The verification mail went to the recording mailer, not to a real mailbox.
    let link = harness
        .last_verification_link()
        .expect("the dev sink captured a verification link");
    let token = link
        .split("token=")
        .nth(1)
        .expect("a token in the link")
        .to_string();
    assert!(!token.is_empty());

    // The refresh token is not stored in the clear.
    let stored: String = sqlx::query_scalar("select refresh_token_hash from auth_sessions")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_ne!(stored, refresh, "only the hash is stored");
    assert_eq!(stored.len(), 64);

    // Unverified accounts can still read their own profile.
    let (status, me) = get_auth(&app, "/auth/me", &access).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email_verified"], false);

    let (status, verified) = post(
        &app,
        "/auth/verify-email",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{verified}");
    assert_eq!(verified["verified"], true);

    let (_, me) = get_auth(&app, "/auth/me", &access).await;
    assert_eq!(me["email_verified"], true);
    assert!(
        me["email_verified_at"].is_string(),
        "the timestamp is recorded, not only the boolean"
    );

    // A consumed token cannot be replayed.
    let (status, _) = post(
        &app,
        "/auth/verify-email",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Signup is where an attacker would farm accounts, so the two cheap checks are
/// asserted: a duplicate address is refused, and the address is normalized.
#[sqlx::test(migrations = "../../migrations")]
async fn signup_rejects_a_duplicate_address_including_a_different_case(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = post(&app, "/auth/signup", signup_body("dup@example.test", "One")).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = post(&app, "/auth/signup", signup_body("dup@example.test", "Two")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict");

    // Without normalization these would be two accounts.
    let (status, _) = post(
        &app,
        "/auth/signup",
        signup_body("DUP@Example.TEST", "Three"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_rejects_a_wrong_password_and_an_unknown_address_identically(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    post(
        &app,
        "/auth/signup",
        signup_body("known@example.test", "One"),
    )
    .await;

    let (wrong_status, wrong) = post(
        &app,
        "/auth/login",
        serde_json::json!({
            "email": "known@example.test",
            "password": "not the password",
            "device": { "name": "Two", "platform": "web" }
        }),
    )
    .await;

    let (unknown_status, unknown) = post(
        &app,
        "/auth/login",
        serde_json::json!({
            "email": "nobody@example.test",
            "password": "not the password",
            "device": { "name": "Two", "platform": "web" }
        }),
    )
    .await;

    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_status, StatusCode::UNAUTHORIZED);
    // Identical bodies: the response must not distinguish "no such account".
    assert_eq!(wrong, unknown);
}

/// The security property: rotating a refresh token invalidates the presented
/// one, and replaying it kills the whole family.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_refresh_token_kills_its_whole_family(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (_, signup) = post(
        &app,
        "/auth/signup",
        signup_body("rotate@example.test", "One"),
    )
    .await;
    let first_refresh = signup["refresh_token"].as_str().unwrap().to_string();

    let (status, rotated) = post(
        &app,
        "/auth/refresh",
        serde_json::json!({ "refresh_token": first_refresh }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let second_refresh = rotated["refresh_token"].as_str().unwrap().to_string();
    assert_ne!(first_refresh, second_refresh);

    // The rotated (revoked) token is presented again: a theft signal.
    let (status, _) = post(
        &app,
        "/auth/refresh",
        serde_json::json!({ "refresh_token": first_refresh }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The legitimate client's live token dies too: the family is compromised.
    let (status, _) = post(
        &app,
        "/auth/refresh",
        serde_json::json!({ "refresh_token": second_refresh }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "reuse must revoke the family, not only the replayed token"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn logout_all_revokes_every_session_of_the_account(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (_, first) = post(
        &app,
        "/auth/signup",
        signup_body("multi@example.test", "One"),
    )
    .await;
    let (_, second) = post(
        &app,
        "/auth/login",
        serde_json::json!({
            "email": "multi@example.test",
            "password": "correct horse battery",
            "device": { "name": "Two", "platform": "web" }
        }),
    )
    .await;

    let second_refresh = second["refresh_token"].as_str().unwrap().to_string();
    let first_access = first["access_token"].as_str().unwrap().to_string();

    let (status, _) = post_auth(
        &app,
        "/auth/logout-all",
        &first_access,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The other device's refresh token is dead, and so is the access token: the
    // extractor checks the session family on every request.
    let (status, _) = post(
        &app,
        "/auth/refresh",
        serde_json::json!({ "refresh_token": second_refresh }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get_auth(&app, "/auth/me", &first_access).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn signup_validation_rejects_a_bad_email_a_short_password_and_a_bad_platform(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    for body in [
        serde_json::json!({
            "email": "not-an-email",
            "password": "correct horse battery",
            "device": { "name": "One", "platform": "macos" }
        }),
        serde_json::json!({
            "email": "short@example.test",
            "password": "tiny",
            "device": { "name": "One", "platform": "macos" }
        }),
        serde_json::json!({
            "email": "platform@example.test",
            "password": "correct horse battery",
            "device": { "name": "One", "platform": "linux" }
        }),
    ] {
        let (status, body) = post(&app, "/auth/signup", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {body}");
        assert_eq!(body["code"], "validation");
    }
}

/// A JWT is a bearer of identity, not of entitlement (A22): editing a claim must
/// not change what the server believes.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tampered_access_token_is_rejected(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (_, signup) = post(&app, "/auth/signup", signup_body("jwt@example.test", "One")).await;
    let access = signup["access_token"].as_str().unwrap().to_string();

    let mut parts: Vec<&str> = access.split('.').collect();
    assert_eq!(parts.len(), 3, "a JWT has three parts");
    // Flip a byte in the payload: the signature must no longer verify.
    let payload = parts[1].to_string();
    let mut mutated = payload.clone().into_bytes();
    let last = mutated.len() - 1;
    mutated[last] = if mutated[last] == b'A' { b'B' } else { b'A' };
    let mutated = String::from_utf8(mutated).unwrap();
    parts[1] = &mutated;
    let tampered = parts.join(".");

    let (status, _) = get_auth(&app, "/auth/me", &tampered).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // And a token signed with the wrong key is not accepted either.
    let (status, _) = get_auth(&app, "/auth/me", "not.a.token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A43's device cap is per plan, and the error must name the plan that lifts it
/// so the UI can show a precise prompt.
#[sqlx::test(migrations = "../../migrations")]
async fn the_device_limit_trips_with_an_upgrade_hint(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let max = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .devices_max;

    // Signup created one device; log in until the limit is reached.
    post(
        &app,
        "/auth/signup",
        signup_body("devices@example.test", "One"),
    )
    .await;
    for index in 1..max {
        let (status, body) = post(
            &app,
            "/auth/login",
            serde_json::json!({
                "email": "devices@example.test",
                "password": "correct horse battery",
                "device": { "name": format!("Device {index}"), "platform": "web" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "device {index} of {max}: {body}");
    }

    let (status, body) = post(
        &app,
        "/auth/login",
        serde_json::json!({
            "email": "devices@example.test",
            "password": "correct horse battery",
            "device": { "name": "One too many", "platform": "web" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached");
    assert_eq!(body["min_plan"], "pro");
    assert_eq!(body["limit"], max as i64);
    assert_eq!(body["used"], max as i64);

    // Revoking a device makes room, which is what the error message suggests.
    let device_id: uuid::Uuid =
        sqlx::query_scalar("select id from devices where name = 'Device 1'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    sqlx::query("update devices set revoked_at = now() where id = $1")
        .bind(device_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = post(
        &app,
        "/auth/login",
        serde_json::json!({
            "email": "devices@example.test",
            "password": "correct horse battery",
            "device": { "name": "Replacement", "platform": "web" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A revoked device's access token stops working immediately. This is the
/// Devices screen's promise, and it is why the extractor joins `devices`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_revoked_device_is_locked_out_on_its_next_request(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (_, signup) = post(
        &app,
        "/auth/signup",
        signup_body("revoke@example.test", "Laptop"),
    )
    .await;
    let access = signup["access_token"].as_str().unwrap().to_string();
    let device_id = signup["device_id"].as_str().unwrap().to_string();

    let (status, _) = get_auth(&app, "/auth/me", &access).await;
    assert_eq!(status, StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/devices/{device_id}"))
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let (status, _) = get_auth(&app, "/auth/me", &access).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// No secret may reach a log line. Asserted on the source, because a runtime log
/// capture in a test would only cover the paths the test happens to exercise.
#[test]
fn no_token_or_link_is_logged() {
    let sources = [
        include_str!("../src/auth/tokens.rs"),
        include_str!("../src/auth/email_verify.rs"),
        include_str!("../src/auth/google.rs"),
        include_str!("../src/auth/routes.rs"),
    ];
    for source in sources {
        for line in source.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("tracing::") && !trimmed.contains("tracing::") {
                continue;
            }
            assert!(
                !trimmed.contains("refresh_token")
                    && !trimmed.contains("access_token")
                    && !trimmed.contains("token =")
                    && !trimmed.contains("link"),
                "a credential must not be a log field: {trimmed}"
            );
        }
    }
}

/// The ordinary sequence sign up, sign out, sign in, sign out, sign in must not
/// consume five device slots on one machine. Before device reuse it hit
/// `devices_max = 2` on free with no explanation, which is a support ticket
/// rather than a feature.
#[sqlx::test(migrations = "../../migrations")]
async fn re_signing_in_from_the_same_machine_reuses_its_device(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, first) = post(
        &app,
        "/auth/signup",
        signup_body("again@example.test", "Laptop"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let first_device = first["device_id"].as_str().unwrap().to_string();

    // Three more sign-ins from the same machine, each preceded by a sign-out,
    // which is the shape that used to leak.
    for round in 0..3 {
        let (status, body) = post(
            &app,
            "/auth/login",
            serde_json::json!({
                "email": "again@example.test",
                "password": "correct horse battery",
                "device": { "name": "Laptop", "platform": "macos" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "round {round}: {body}");
        assert_eq!(
            body["device_id"], first_device,
            "round {round}: the same machine must reuse its device row"
        );

        let refresh = body["refresh_token"].as_str().unwrap().to_string();
        let (status, _) = post(
            &app,
            "/auth/logout",
            serde_json::json!({ "refresh_token": refresh }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    let devices: i64 = sqlx::query_scalar("select count(*) from devices where revoked_at is null")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(devices, 1, "four sign-ins from one machine are one device");
}

/// A *different* machine still consumes a slot, and the cap still trips: reuse
/// must not have quietly disabled `devices_max`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_new_machine_still_counts_against_the_limit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let max = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .devices_max;

    post(
        &app,
        "/auth/signup",
        signup_body("machines@example.test", "Machine 1"),
    )
    .await;
    for index in 2..=max {
        let (status, body) = post(
            &app,
            "/auth/login",
            serde_json::json!({
                "email": "machines@example.test",
                "password": "correct horse battery",
                "device": { "name": format!("Machine {index}"), "platform": "macos" }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "machine {index}: {body}");
    }

    let (status, body) = post(
        &app,
        "/auth/login",
        serde_json::json!({
            "email": "machines@example.test",
            "password": "correct horse battery",
            "device": { "name": "Machine too many", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached");
}
