//! Devices, workspaces and chunks: the account's machines, its containers, and
//! the text a citation points at.
//!
//! The through-line is ownership. Every route here is scoped by `user_id`, and
//! the tests assert the *observable* consequence rather than the SQL: a row
//! that belongs to someone else is a 404 (never a 403, so existence does not
//! leak), a revoked device's token stops working on the very next request, and
//! a workspace delete tombstones its sources in the same transaction instead of
//! letting them resurface through a source endpoint.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Phase 0 Task 1).
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use stvdy_core::config::PlanId;
use stvdy_server::testing::TestState;
use tower::ServiceExt;
use uuid::Uuid;

/// One request helper for the whole suite. `token` is `Option` because the auth
/// routes are the only ones called without a bearer.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body)).expect("request"))
        .await
        .expect("response");

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    let value: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

async fn json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(
        app,
        method,
        uri,
        token,
        Some("application/json"),
        serde_json::to_vec(&body).expect("serialize"),
    )
    .await
}

async fn get(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    send(app, "GET", uri, Some(token), None, Vec::new()).await
}

async fn delete(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    send(app, "DELETE", uri, Some(token), None, Vec::new()).await
}

/// A multipart body with one `kind` field and one payload field.
fn multipart(fields: &[(&str, &str, Option<&[u8]>)]) -> (String, Vec<u8>) {
    let boundary = "----stvdy-test-boundary";
    let mut body = Vec::new();
    for (name, value, file) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(bytes) => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{value}\"\r\n\
                         Content-Type: application/pdf\r\n\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(bytes);
            }
            None => {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                body.extend_from_slice(value.as_bytes());
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Signs up, verifies the email, and returns the full auth response so a test
/// can read `access_token`, `device_id` and `user.id` from one place.
async fn signup(
    app: &axum::Router,
    harness: &TestState,
    email: &str,
    device_name: &str,
) -> serde_json::Value {
    let (status, body) = json(
        app,
        "POST",
        "/auth/signup",
        None,
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": device_name, "platform": "macos" }
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
        None,
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    body
}

/// A second device for an existing account: the login path is what a returning
/// user takes, and it is what makes `GET /devices` a list rather than a single
/// row.
async fn login(app: &axum::Router, email: &str, device_name: &str) -> serde_json::Value {
    let (status, body) = json(
        app,
        "POST",
        "/auth/login",
        None,
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": device_name, "platform": "web" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn create_workspace(app: &axum::Router, token: &str, name: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/workspaces",
        Some(token),
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["workspace"]["id"]
        .as_str()
        .expect("workspace id")
        .to_string()
}

fn token_of(body: &serde_json::Value) -> String {
    body["access_token"]
        .as_str()
        .expect("access token")
        .to_string()
}

fn device_of(body: &serde_json::Value) -> String {
    body["device_id"].as_str().expect("device id").to_string()
}

fn user_of(body: &serde_json::Value) -> uuid::Uuid {
    uuid::Uuid::parse_str(body["user"]["id"].as_str().expect("user id")).expect("uuid")
}

/// `GET /devices` lists every live device, marks exactly the caller as
/// `current`, and drops a revoked one from the list.
#[sqlx::test(migrations = "../../migrations")]
async fn listing_devices_marks_only_the_caller_current_and_hides_revoked(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let signed_up = signup(&app, &harness, "devices@example.test", "Signup Mac").await;
    let signup_device = device_of(&signed_up);
    let signed_in = login(&app, "devices@example.test", "Login Mac").await;
    let login_token = token_of(&signed_in);
    let login_device = device_of(&signed_in);
    assert_ne!(signup_device, login_device);

    // The returning device sees both machines and is the only `current` one.
    let (status, body) = get(&app, "/devices", &login_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let devices = body["devices"].as_array().expect("devices array");
    assert_eq!(devices.len(), 2, "both devices must be listed: {body}");
    let current: Vec<&serde_json::Value> = devices
        .iter()
        .filter(|device| device["current"] == true)
        .collect();
    assert_eq!(current.len(), 1, "exactly one device is current: {body}");
    assert_eq!(current[0]["id"], login_device.as_str());

    // Revoke the other device through the API, then confirm it is gone from the
    // list rather than shown as a tombstone.
    let (status, body) = delete(&app, &format!("/devices/{signup_device}"), &login_token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = get(&app, "/devices", &login_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let devices = body["devices"].as_array().expect("devices array");
    assert_eq!(devices.len(), 1, "a revoked device is not listed: {body}");
    assert_eq!(devices[0]["id"], login_device.as_str());
    assert_eq!(devices[0]["current"], true);
}

/// The Devices screen's "Last synced" traces to a row, and it is not `last_seen_at`.
///
/// The two are different claims: `last_seen_at` refreshes on any authenticated request,
/// while `last_sync_at` is written only by a completed sync pull (A180). A screen that
/// showed `last_seen_at` would say "synced 2 minutes ago" for a device that has never pulled
/// anything, which is the one thing the honesty gate forbids — a number that does not trace
/// to the work it names.
#[sqlx::test(migrations = "../../migrations")]
async fn last_synced_is_absent_until_the_device_actually_pulls(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let signed_up = signup(&app, &harness, "synced@example.test", "Fresh Mac").await;
    let token = token_of(&signed_up);
    let device = device_of(&signed_up);

    // A fresh device has been seen — signup itself did that — and has synced nothing.
    let (status, body) = get(&app, "/devices", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = &body["devices"][0];
    assert_eq!(listed["id"], device.as_str());
    assert!(
        !listed["last_seen_at"].is_null(),
        "signup is a request, so the device has been seen: {body}"
    );
    assert!(
        listed["last_sync_at"].is_null(),
        "a device that has never pulled must not report a sync time: {body}"
    );

    // One pull, and now it has.
    let (status, body) = get(&app, "/sync/pull?cursor=0", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = get(&app, "/devices", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body["devices"][0]["last_sync_at"].is_null(),
        "a completed pull is what sets it (A180): {body}"
    );
}

/// A rename is scoped: another user's device id is a 404, your own rename
/// persists, and an empty or over-long name is rejected before it is written.
#[sqlx::test(migrations = "../../migrations")]
async fn renaming_a_device_is_scoped_and_validated(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let alice = signup(&app, &harness, "alice@example.test", "Alice Mac").await;
    let alice_token = token_of(&alice);
    let alice_device = device_of(&alice);
    let bob = signup(&app, &harness, "bob@example.test", "Bob Mac").await;
    let bob_device = device_of(&bob);

    // Bob's device id exists, but not for Alice: 404, so existence does not leak.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/devices/{bob_device}"),
        Some(&alice_token),
        serde_json::json!({ "name": "Stolen" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");

    // Renaming her own device persists.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/devices/{alice_device}"),
        Some(&alice_token),
        serde_json::json!({ "name": "Renamed Mac" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["device"]["name"], "Renamed Mac");
    assert_eq!(body["device"]["id"], alice_device.as_str());

    let (_, list) = get(&app, "/devices", &alice_token).await;
    assert_eq!(list["devices"][0]["name"], "Renamed Mac");

    // An empty name is a validation error, not a silent blank row.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/devices/{alice_device}"),
        Some(&alice_token),
        serde_json::json!({ "name": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation");

    // And so is a name longer than the 120-character ceiling.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/devices/{alice_device}"),
        Some(&alice_token),
        serde_json::json!({ "name": "x".repeat(121) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation");
}

/// Registering a push token twice for the same device updates the one row; a
/// provider the shells do not use is rejected.
#[sqlx::test(migrations = "../../migrations")]
async fn a_push_token_upserts_per_device_and_rejects_unknown_providers(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "push@example.test", "Phone").await;
    let token = token_of(&account);
    let device = device_of(&account);

    let (status, body) = json(
        &app,
        "PUT",
        &format!("/devices/{device}/push-token"),
        Some(&token),
        serde_json::json!({ "provider": "apns", "token": "token-one" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // The OS rotated the token: the client re-registers the same device.
    let (status, body) = json(
        &app,
        "PUT",
        &format!("/devices/{device}/push-token"),
        Some(&token),
        serde_json::json!({ "provider": "fcm", "token": "token-two" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let rows: Vec<(String, String)> =
        sqlx::query_as("select provider, token from push_tokens where device_id = $1")
            .bind(uuid::Uuid::parse_str(&device).unwrap())
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1, "the second registration must not add a row");
    assert_eq!(rows[0].0, "fcm");
    assert_eq!(rows[0].1, "token-two");

    // A provider neither shell uses would sit undeliverable, so it is refused.
    let (status, body) = json(
        &app,
        "PUT",
        &format!("/devices/{device}/push-token"),
        Some(&token),
        serde_json::json!({ "provider": "webpush", "token": "token-three" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation");
}

/// Revoking a device kills the access token that device is holding: the very
/// next request is 401. This is the Devices screen's promise.
#[sqlx::test(migrations = "../../migrations")]
async fn revoking_a_device_locks_out_its_access_token(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "revoke-device@example.test", "Old Laptop").await;
    let token = token_of(&account);
    let device = device_of(&account);

    let (status, _) = get(&app, "/auth/me", &token).await;
    assert_eq!(status, StatusCode::OK, "the token works before the revoke");

    let (status, body) = delete(&app, &format!("/devices/{device}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = get(&app, "/auth/me", &token).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a revoked device must be locked out immediately: {body}"
    );
}

/// The workspace cap is read from `plans.toml`, not hardcoded, and the error
/// carries everything the upgrade prompt needs.
#[sqlx::test(migrations = "../../migrations")]
async fn the_workspace_limit_trips_at_the_configured_count(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "limits@example.test", "Laptop").await;
    let token = token_of(&account);

    // The expected number comes from the loaded config, so retuning the file
    // cannot leave a stale literal behind.
    let limit = harness.state.config.plans.get(PlanId::Free).workspaces;
    let max = limit.max().expect("free workspaces are limited");

    for index in 0..max {
        create_workspace(&app, &token, &format!("Workspace {index}")).await;
    }

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        Some(&token),
        serde_json::json!({ "name": "One too many" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached");
    assert_eq!(body["min_plan"], "pro");
    assert_eq!(body["limit"], i64::from(max));
    assert_eq!(body["used"], i64::from(max));
}

/// A8: a client-supplied id is stored as given, and reusing it is a conflict
/// rather than a second row.
#[sqlx::test(migrations = "../../migrations")]
async fn a_client_supplied_workspace_id_is_honoured_and_duplicates_conflict(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "client-id@example.test", "Laptop").await;
    let token = token_of(&account);
    let id = uuid::Uuid::now_v7().to_string();

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        Some(&token),
        serde_json::json!({ "name": "Offline", "id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["workspace"]["id"], id.as_str());

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        Some(&token),
        serde_json::json!({ "name": "Again", "id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict");

    // The first row is the one that survives.
    let (status, body) = get(&app, &format!("/workspaces/{id}"), &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workspace"]["name"], "Offline");
}

/// Workspaces are per account: another user's id is invisible through the list
/// and answers 404 on every verb.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_workspace_is_never_visible(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let alice = signup(&app, &harness, "ws-alice@example.test", "Alice Mac").await;
    let alice_token = token_of(&alice);
    let bob = signup(&app, &harness, "ws-bob@example.test", "Bob Mac").await;
    let bob_token = token_of(&bob);

    let bob_workspace = create_workspace(&app, &bob_token, "Bob's notes").await;

    let (status, body) = get(&app, "/workspaces", &alice_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|workspace| workspace["id"] != bob_workspace.as_str()),
        "Bob's workspace must not appear in Alice's list: {body}"
    );

    let (status, body) = get(&app, &format!("/workspaces/{bob_workspace}"), &alice_token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/workspaces/{bob_workspace}"),
        Some(&alice_token),
        serde_json::json!({ "name": "Stolen" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = delete(&app, &format!("/workspaces/{bob_workspace}"), &alice_token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // It still exists for its owner, which is what makes the 404 a scoping
    // decision rather than a missing row.
    let (status, _) = get(&app, &format!("/workspaces/{bob_workspace}"), &bob_token).await;
    assert_eq!(status, StatusCode::OK);
}

/// Deleting a workspace tombstones its sources in the same transaction, so the
/// source endpoints cannot outlive the workspace that owned them.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_workspace_tombstones_its_sources(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "cascade@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Cascade").await;

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", "Notes", None),
        ("text", "Photosynthesis stores energy in glucose.", None),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        Some(&token),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source = created["source"]["id"]
        .as_str()
        .expect("source id")
        .to_string();

    let (status, body) = delete(&app, &format!("/workspaces/{workspace}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // The source's own status endpoint no longer sees it.
    let (status, body) = get(&app, &format!("/sources/{source}/status"), &token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a tombstoned source must not be readable: {body}"
    );

    // And the workspace that keyed it is gone too.
    let (status, body) = get(&app, &format!("/workspaces/{workspace}/sources"), &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// Chunks are scoped through their source's owner, and a readable chunk returns
/// exactly the stored text, page and index.
#[sqlx::test(migrations = "../../migrations")]
async fn chunks_are_scoped_and_return_their_stored_fields(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let alice = signup(&app, &harness, "chunk-alice@example.test", "Alice Mac").await;
    let alice_token = token_of(&alice);
    let alice_user = user_of(&alice);
    let alice_workspace = create_workspace(&app, &alice_token, "Alice").await;

    let bob = signup(&app, &harness, "chunk-bob@example.test", "Bob Mac").await;
    let bob_token = token_of(&bob);
    let bob_user = user_of(&bob);
    let bob_workspace = create_workspace(&app, &bob_token, "Bob").await;

    let alice_chunk = seed_source_and_chunk(
        &harness.state.pool,
        alice_user,
        uuid::Uuid::parse_str(&alice_workspace).unwrap(),
        "Alice's stored text",
        7,
        3,
    )
    .await;
    let bob_chunk = seed_source_and_chunk(
        &harness.state.pool,
        bob_user,
        uuid::Uuid::parse_str(&bob_workspace).unwrap(),
        "Bob's stored text",
        1,
        0,
    )
    .await;

    let (status, body) = get(&app, &format!("/chunks/{alice_chunk}"), &alice_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"], "Alice's stored text");
    assert_eq!(body["page"], 7);
    assert_eq!(body["idx"], 3);

    // Bob's chunk is invisible to Alice, and vice versa: 404, never 403.
    let (status, body) = get(&app, &format!("/chunks/{bob_chunk}"), &alice_token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = get(&app, &format!("/chunks/{alice_chunk}"), &bob_token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// `source_count` counts live sources only, so it falls when a source is
/// tombstoned.
#[sqlx::test(migrations = "../../migrations")]
async fn source_count_counts_live_sources_only(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "count@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Counting").await;

    let count = |body: &serde_json::Value, workspace: &str| -> i64 {
        body["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == workspace)
            .expect("the workspace is listed")["source_count"]
            .as_i64()
            .expect("source_count")
    };

    let (_, body) = get(&app, "/workspaces", &token).await;
    assert_eq!(
        count(&body, &workspace),
        0,
        "a fresh workspace has no sources"
    );

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", "Notes", None),
        (
            "text",
            "The Krebs cycle completes the oxidation of glucose.",
            None,
        ),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        Some(&token),
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source = created["source"]["id"]
        .as_str()
        .expect("source id")
        .to_string();

    let (_, body) = get(&app, "/workspaces", &token).await;
    assert_eq!(count(&body, &workspace), 1, "the live source is counted");

    let (status, body) = delete(&app, &format!("/sources/{source}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, body) = get(&app, "/workspaces", &token).await;
    assert_eq!(
        count(&body, &workspace),
        0,
        "a tombstoned source drops out of the count"
    );
}

/// Inserts a live source and one chunk directly, so a chunk test can pin `page`
/// and `idx` without running the ingestion pipeline. Returns the chunk id.
async fn seed_source_and_chunk(
    pool: &PgPool,
    user_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
    content: &str,
    page: i32,
    idx: i32,
) -> uuid::Uuid {
    let source_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into sources (id, user_id, workspace_id, kind, title, status)
         values ($1, $2, $3, 'text', 'Seeded', 'ready')",
    )
    .bind(source_id)
    .bind(user_id)
    .bind(workspace_id)
    .execute(pool)
    .await
    .unwrap();

    let chunk_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into chunks (id, source_id, page, idx, content, content_hash, embedder)
         values ($1, $2, $3, $4, $5, $6, 'hash-embedder-768')",
    )
    .bind(chunk_id)
    .bind(source_id)
    .bind(page)
    .bind(idx)
    .bind(content)
    .bind(format!("hash-{chunk_id}"))
    .execute(pool)
    .await
    .unwrap();

    chunk_id
}

/// A workspace's course identity: the code, the hue and the exam date.
///
/// The three fields exist because the design's sidebar and course cards present a
/// workspace as a course, and none of them can be derived from the name. Two
/// behaviours here are the ones a later edit is most likely to break:
///
/// 1. **The hue is assigned round-robin and never repeats until the six are used.**
///    A client that picked its own colour would give two devices the same one, and a
///    constant would make every course in the rail identical.
/// 2. **`PATCH` distinguishes an absent field from an explicit `null`.** Serde maps
///    both to `None` for a plain `Option<T>`, which would make "rename this course"
///    silently clear its exam date, and would make clearing one impossible.
#[sqlx::test(migrations = "../../migrations")]
async fn a_workspace_carries_its_course_identity(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "course@example.test", "Laptop").await;
    let token = token_of(&account);

    // The hue wrap needs seven workspaces, and the free plan allows two. The plan is
    // raised directly rather than through checkout because this test is about the
    // colour assignment, not about billing — and it needs the subscription row as
    // well, because `effective_plan` honours `users.plan_id` only while a period is
    // active (A146).
    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("course@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // The code and the exam date are given at create; the hue is the server's.
    let (status, first) = json(
        &app,
        "POST",
        "/workspaces",
        Some(&token),
        serde_json::json!({ "name": "Cell biology", "course_code": "BIO 201", "exam_date": "2026-11-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let first_id = first["workspace"]["id"].as_str().unwrap().to_string();
    assert_eq!(first["workspace"]["course_code"], "BIO 201");
    assert_eq!(first["workspace"]["exam_date"], "2026-11-05");
    assert_eq!(
        first["workspace"]["color"], 0,
        "the first course takes hue 0"
    );

    let (status, second) = json(
        &app,
        "POST",
        "/workspaces",
        Some(&token),
        serde_json::json!({ "name": "Statistics" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let second_id = second["workspace"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        second["workspace"]["color"], 1,
        "a second course takes the next hue, so two courses are told apart by colour"
    );
    assert!(
        second["workspace"]["course_code"].is_null(),
        "a course with no code is null, not an empty string"
    );

    // Six courses wrap, so the seventh reuses hue 0 rather than falling out of the
    // range the client indexes with.
    for index in 2..7 {
        let (status, body) = json(
            &app,
            "POST",
            "/workspaces",
            Some(&token),
            serde_json::json!({ "name": format!("Course {index}") }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let expected = index % 6;
        assert_eq!(body["workspace"]["color"], expected, "course {index}");
    }

    // A rename that does not mention the course fields leaves them alone.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/workspaces/{first_id}"),
        Some(&token),
        serde_json::json!({ "name": "Cell biology II" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["workspace"]["name"], "Cell biology II");
    assert_eq!(
        body["workspace"]["course_code"], "BIO 201",
        "an absent course_code is left alone, not cleared"
    );
    assert_eq!(body["workspace"]["exam_date"], "2026-11-05");

    // An explicit null clears it, which is what makes the field removable at all.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/workspaces/{first_id}"),
        Some(&token),
        serde_json::json!({ "name": "Cell biology II", "exam_date": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["workspace"]["exam_date"].is_null(),
        "an explicit null clears the exam date"
    );

    // The list ships the same three fields, because the sidebar reads the list
    // rather than one workspace at a time.
    let (status, body) = get(&app, "/workspaces", &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = body["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == first_id.as_str())
        .expect("the renamed course is listed");
    assert_eq!(row["course_code"], "BIO 201");
    assert_eq!(row["color"], 0);
    assert!(row["exam_date"].is_null());

    // A code past the column's cap is a validation error naming the field, not a
    // constraint violation surfaced as a 500.
    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        Some(&token),
        serde_json::json!({ "name": "Too long", "course_code": "ABCDEFGHIJKLMNOPQ" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");

    let _ = second_id;
}

/// Deleting a workspace tombstones every content row it owns, in the same transaction.
///
/// This is not tidiness. The entitlement counters filter on each content table's **own**
/// `deleted_at`, not the workspace's, so a deck left live under a deleted workspace keeps
/// counting against the student's monthly limit while being unreachable — every list route
/// calls `owned_name` first. The student would be charged for content they cannot open and
/// cannot delete, because the workspace that owned it is gone.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_workspace_tombstones_its_content(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "content-cascade@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Cascade").await;

    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("content-cascade@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let workspace_id = Uuid::parse_str(&workspace).unwrap();

    // One live row per content type. The column lists differ because the required
    // columns do: `decks.source_ids` has no default, and `cheatsheets.scope` is
    // `not null` with no default either.
    sqlx::query(
        "insert into decks (user_id, workspace_id, title, source_ids, status)
         values ($1, $2, 'live', array[]::uuid[], 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into quizzes (user_id, workspace_id, title, status)
         values ($1, $2, 'live', 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into reports (user_id, workspace_id, title, status)
         values ($1, $2, 'live', 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into cheatsheets (user_id, workspace_id, title, scope, status)
         values ($1, $2, 'live', 'UTS Jaringan', 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // A card under the deck, so the child sweep is exercised too.
    sqlx::query(
        "insert into flashcards (user_id, deck_id, front, back)
         select $1, id, 'front', 'back' from decks where workspace_id = $2",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = delete(&app, &format!("/workspaces/{workspace}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Four literal statements rather than one `format!` loop: `query_scalar` takes
    // `impl SqlSafeStr`, which only `&'static str` implements, so a table name
    // interpolated at runtime is a compile error by design.
    let live_decks: i64 = sqlx::query_scalar(
        "select count(*) from decks where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_decks, 0, "decks must be tombstoned with the workspace");

    let live_quizzes: i64 = sqlx::query_scalar(
        "select count(*) from quizzes where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_quizzes, 0, "quizzes must be tombstoned with the workspace");

    let live_reports: i64 = sqlx::query_scalar(
        "select count(*) from reports where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_reports, 0, "reports must be tombstoned with the workspace");

    let live_cheatsheets: i64 = sqlx::query_scalar(
        "select count(*) from cheatsheets where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        live_cheatsheets, 0,
        "cheatsheets must be tombstoned with the workspace"
    );

    let live_cards: i64 = sqlx::query_scalar(
        "select count(*) from flashcards f
           join decks d on d.id = f.deck_id
          where d.workspace_id = $1 and f.deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_cards, 0, "a deck's cards go with the deck");
}

/// Bulk delete tombstones every id in one transaction, and skips ids that are not the
/// workspace's.
///
/// The skip is the contract, not an oversight: a bulk delete runs against a list the client
/// rendered, and a row another device already deleted is a normal race, not an error worth
/// showing. What must not happen is the whole batch failing over one stale id — that would
/// leave the other items undeleted and the button apparently broken.
#[sqlx::test(migrations = "../../migrations")]
async fn bulk_delete_tombstones_the_batch_and_skips_stale_ids(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "bulk@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Bulk").await;

    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("bulk@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let workspace_id = Uuid::parse_str(&workspace).unwrap();

    let mut deck_ids = Vec::new();
    for index in 0..3 {
        let id: Uuid = sqlx::query_scalar(
            "insert into decks (user_id, workspace_id, title, source_ids, status)
             values ($1, $2, $3, array[]::uuid[], 'ready') returning id",
        )
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("deck {index}"))
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        deck_ids.push(id);
    }
    let quiz_id: Uuid = sqlx::query_scalar(
        "insert into quizzes (user_id, workspace_id, title, source_ids, status)
         values ($1, $2, 'quiz', array[]::uuid[], 'ready') returning id",
    )
    .bind(user_id)
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // A deck in *another* workspace of the same account: it must survive, because the
    // route's guard is the workspace as well as the user.
    let other_workspace = create_workspace(&app, &token, "Other").await;
    let other_id: Uuid = sqlx::query_scalar(
        "insert into decks (user_id, workspace_id, title, source_ids, status)
         values ($1, $2, 'elsewhere', array[]::uuid[], 'ready') returning id",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(&other_workspace).unwrap())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/bulk-delete"),
        Some(&token),
        serde_json::json!({
            "deck_ids": [
                deck_ids[0].to_string(),
                deck_ids[1].to_string(),
                // A nil uuid that matches no row, and a real id in another workspace:
                // both are skipped rather than failing the batch.
                Uuid::nil().to_string(),
                other_id.to_string(),
            ],
            "quiz_ids": [quiz_id.to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let live_decks: i64 = sqlx::query_scalar(
        "select count(*) from decks where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_decks, 1, "the third deck was not in the batch");

    let other_live: i64 =
        sqlx::query_scalar("select count(*) from decks where id = $1 and deleted_at is null")
            .bind(other_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(other_live, 1, "another workspace's deck must not be reachable");

    let live_quizzes: i64 = sqlx::query_scalar(
        "select count(*) from quizzes where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_quizzes, 0);

    // An empty batch is a client bug, not a no-op.
    let (status, _) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/bulk-delete"),
        Some(&token),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// `PATCH /workspaces/:id/content-order` sets `sort_order` from the array index, and the
/// list route honours it.
///
/// Ordering by `created_at desc, id` was the old behaviour, and it is wrong for this feature
/// for a specific reason: `created_at` defaults to `now()`, and a generation that inserts
/// several rows in one transaction gives them the same microsecond, so the tiebreak fell
/// through to a random uuid. A student who reorders a list must see the order they chose.
#[sqlx::test(migrations = "../../migrations")]
async fn content_order_sets_the_list_order(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "reorder@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Order").await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("reorder@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let workspace_id = Uuid::parse_str(&workspace).unwrap();

    let mut ids = Vec::new();
    for index in 0..3 {
        let id: Uuid = sqlx::query_scalar(
            "insert into decks (user_id, workspace_id, title, source_ids, status)
             values ($1, $2, $3, array[]::uuid[], 'ready') returning id",
        )
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("deck {index}"))
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        ids.push(id);
    }

    // Reverse the creation order.
    let reversed: Vec<String> = ids.iter().rev().map(Uuid::to_string).collect();
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/workspaces/{workspace}/content-order"),
        Some(&token),
        serde_json::json!({ "deck_ids": reversed }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace}/decks"),
        Some(&token),
        serde_json::Value::Null,
    )
    .await;
    let titles: Vec<&str> = list["decks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|deck| deck["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["deck 2", "deck 1", "deck 0"], "{list}");
}
