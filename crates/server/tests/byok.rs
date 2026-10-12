//! BYOK: the key at rest, the seam, and the two rules that keep it honest.
//!
//! The chat half of this suite runs against a **stub HTTP server** rather than a scripted
//! trait double, and that is deliberate: the BYOK provider is built by `provider_for` from
//! the account's stored endpoint and key, so a test that installed a double would prove the
//! seam exists while proving nothing about whether the stored key is what reaches the wire.
//! The stub records what it was sent, so the assertion is on the request the provider actually
//! made.
//!
//! Three things this suite exists to catch, each invisible to a reading:
//!
//!  * a response that carries the key — the one secret this feature must never return;
//!  * a BYOK turn that spends credits, which charges a student twice for one answer;
//!  * a BYOK turn that the provider-cost ceiling pauses, which pauses an account for spending
//!    someone else's money (A156).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use chrono::Duration;
use serde_json::json;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json as request, signed_up, stub_http_json, workspace_with_text};

/// A distinctive key string, so "the key is absent from this body" is an assertion about a
/// value that could not appear by accident.
const SECRET: &str = "sk-byok-9f3c1a7e4b2d8f6a0c5e1d9b3a7f2c4e";

/// One `[OI]`-compatible SSE answer carrying content and a usage block, which is the shape
/// `OpenAiCompatLlm::stream` parses.
fn sse_answer(text: &str, cost: f64) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{}}],\"usage\":{{\"prompt_tokens\":11,\"completion_tokens\":4,\
         \"cost\":{cost}}}}}\n\n\
         data: [DONE]\n\n"
    )
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

/// A signed-up account on `plan` with a live subscription, on a device of `platform`.
async fn account(harness: &TestState, email: &str, plan: &str, platform: &str) -> (Uuid, String) {
    let app = router(harness);
    let token = signed_up(&app, harness, email).await;
    let user_id = user_id_of(harness, email).await;

    sqlx::query("update users set plan_id = $2 where id = $1")
        .bind(user_id)
        .bind(plan)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .bind(plan)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query("update devices set platform = $2 where user_id = $1")
        .bind(user_id)
        .bind(platform)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    (user_id, token)
}

async fn store_key(
    app: &axum::Router,
    token: &str,
    key: &str,
    base_url: &str,
) -> (StatusCode, serde_json::Value) {
    request(
        app,
        "PUT",
        "/settings/api-key",
        token,
        json!({ "key": key, "base_url": base_url, "model": "byok-model" }),
    )
    .await
}

/// Drives the chat route as a **plain JSON** request, for the cases that refuse before the
/// stream starts.
///
/// A refusal is an HTTP status with the documented error envelope, not an SSE frame: the
/// route never opens the stream, so `sse_events` would return an empty event list and an
/// assertion on `events[0]` would panic rather than report the refusal.
async fn chat_json(
    app: &axum::Router,
    workspace_id: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    request(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        token,
        json!({ "message": "What absorbs light?" }),
    )
    .await
}

async fn ledger_total(harness: &TestState, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// A free or pro account cannot store a key: BYOK is the Power plan's feature (A302), and the
/// refusal is `plan_required` (403) rather than `feature_locked`, because on a desktop the
/// remedy is to buy the plan that grants it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_free_or_pro_account_cannot_store_a_key(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);

    for plan in ["free", "pro"] {
        let (user_id, token) = account(
            &harness,
            &format!("{plan}-byok@example.test"),
            plan,
            "macos",
        )
        .await;
        let (status, body) = store_key(&app, &token, SECRET, "https://api.example.test/v1").await;

        assert_eq!(status, StatusCode::FORBIDDEN, "{plan}: {body}");
        assert_eq!(body["code"], "plan_required", "{plan}: {body}");
        // The remedy names the plan that would grant it, derived from the matrix.
        assert_eq!(body["min_plan"], "power", "{plan}: {body}");

        // And nothing was written: a refused request must not leave ciphertext behind.
        let stored: i64 =
            sqlx::query_scalar("select count(*)::bigint from user_api_keys where user_id = $1")
                .bind(user_id)
                .fetch_one(&harness.state.pool)
                .await
                .unwrap();
        assert_eq!(stored, 0, "{plan}: a refused key is not stored");
    }
}

/// The response never carries the key, and the column is ciphertext: a distinctive key string
/// is absent from every body and from `key_enc`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_key_is_encrypted_at_rest_and_never_returned(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (user_id, token) = account(&harness, "byok-store@example.test", "power", "macos").await;

    let (status, body) = store_key(&app, &token, SECRET, "https://api.example.test/v1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["set"], true, "{body}");
    assert_eq!(body["base_url"], "https://api.example.test/v1", "{body}");
    assert_eq!(body["model"], "byok-model", "{body}");
    assert!(
        !body.to_string().contains(SECRET),
        "the write response must not echo the key: {body}"
    );

    // The read never carries it either.
    let (status, read) = request(&app, "GET", "/settings/api-key", &token, json!(null)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["set"], true, "{read}");
    assert!(
        !read.to_string().contains(SECRET),
        "the read response must not carry the key: {read}"
    );

    // The column is ciphertext: the plaintext is not present in the stored bytes, and the
    // row decrypts back to it.
    let stored: Vec<u8> =
        sqlx::query_scalar("select key_enc from user_api_keys where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        !stored
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes()),
        "the plaintext must not be in `key_enc`"
    );
    // A random nonce per write, so two writes of the same key differ — a deterministic nonce
    // under one key is the failure AES-GCM does not survive.
    store_key(&app, &token, SECRET, "https://api.example.test/v1").await;
    let rewritten: Vec<u8> =
        sqlx::query_scalar("select key_enc from user_api_keys where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_ne!(stored, rewritten, "each write uses a fresh nonce");

    // And the provider the account gets is built from that key: the stub sees it.
    let stub = stub_http_json(200, sse_answer("Hi", 0.002)).await;
    store_key(&app, &token, SECRET, stub.base_url()).await;
    let (workspace_id, _) = workspace_with_text(
        &app,
        &harness,
        &token,
        "byok",
        "Photosynthesis",
        "Chlorophyll absorbs light.",
    )
    .await;
    let (status, _) = common::sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        json!({ "message": "What absorbs light?" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let sent = stub.requests();
    assert_eq!(sent.len(), 1, "the provider reached the account's endpoint");
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {SECRET}").as_str()),
        "the stored key is what the request authenticates with"
    );

    // Removing it is idempotent, and the description says so afterwards.
    let (status, removed) = request(&app, "DELETE", "/settings/api-key", &token, json!(null)).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["set"], false, "{removed}");
    let (_, again) = request(&app, "DELETE", "/settings/api-key", &token, json!(null)).await;
    assert_eq!(again["set"], false, "a second removal is a no-op");
}

/// A BYOK call spends no credits and is recorded with `byok = true` and its provider cost, so
/// the margin report can exclude it (A156/A302).
#[sqlx::test(migrations = "../../migrations")]
async fn a_byok_call_consumes_no_credits_and_is_recorded_as_byok(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (user_id, token) = account(&harness, "byok-chat@example.test", "power", "macos").await;

    let stub = stub_http_json(200, sse_answer("Chlorophyll", 0.003)).await;
    store_key(&app, &token, SECRET, stub.base_url()).await;

    let (workspace_id, _) = workspace_with_text(
        &app,
        &harness,
        &token,
        "byok-chat",
        "Photosynthesis",
        "Chlorophyll absorbs light.",
    )
    .await;

    let before = ledger_total(&harness, user_id).await;
    let (status, events) = common::sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        json!({ "message": "What absorbs light?" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        events.iter().any(|event| event.kind() == "done"),
        "the turn completed on the account's own provider"
    );

    // No credits moved. The signup grant is the only ledger activity, so the total is
    // unchanged and there is no spend row at all.
    assert_eq!(
        ledger_total(&harness, user_id).await,
        before,
        "a BYOK turn spends nothing"
    );
    let spends: i64 = sqlx::query_scalar(
        "select count(*)::bigint from credit_ledger
          where user_id = $1 and delta < 0 and reason = 'usage:chat_message'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(spends, 0, "and writes no spend row");

    // The cost is still recorded, and flagged, because the margin report shows BYOK in its
    // own column rather than blending it into ours.
    let (byok, cost, plan_id): (bool, Option<f64>, Option<String>) = sqlx::query_as(
        "select byok, provider_cost_usd::float8, plan_id from usage_events
          where user_id = $1 and action = 'chat_message'
          order by created_at desc limit 1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert!(byok, "the row is recorded as BYOK");
    assert_eq!(cost, Some(0.003), "the provider's cost is still recorded");
    assert_eq!(
        plan_id.as_deref(),
        Some("power"),
        "and attributed to the plan it happened under (A303)"
    );
}

/// The ceiling does not pause a BYOK account: it is spending someone else's money.
#[sqlx::test(migrations = "../../migrations")]
async fn the_ceiling_does_not_pause_a_byok_account(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // A **server** provider, because the route resolves it before it reaches the ceiling: with
    // no `LLM_BASE_URL` the non-BYOK half would answer 502 `provider_error` and the test would
    // be asserting the wrong refusal. Installing one is what makes "the ceiling refuses" a
    // statement about the ceiling.
    let scripted = std::sync::Arc::new(ScriptedLlm::new(vec!["Chlorophyll.".to_string()]));
    let harness = harness.with_llm(scripted);
    let app = router(&harness);
    let (user_id, token) = account(&harness, "byok-ceiling@example.test", "power", "macos").await;

    // The account is far past its ceiling — power's is 6.00 USD.
    sqlx::query(
        "insert into usage_events (user_id, action, credits, provider_cost_usd, byok)
         values ($1, 'chat_message', 1, 99.0, false)",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (workspace_id, _) = workspace_with_text(
        &app,
        &harness,
        &token,
        "ceiling",
        "Photosynthesis",
        "Chlorophyll absorbs light.",
    )
    .await;

    // Without a key, the ceiling refuses: this is the assertion that keeps the BYOK case
    // from being vacuous, because it shows the same account and the same route do refuse.
    let (status, body) = chat_json(&app, &workspace_id, &token).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an account past its ceiling is refused on the server's provider: {body:?}"
    );
    assert_eq!(body["code"], "limit_reached", "{body:?}");

    // With a key, the same request succeeds: the ceiling bounds *our* provider bill, and a
    // call on the student's key is not ours (A156).
    let stub = stub_http_json(200, sse_answer("Chlorophyll", 0.004)).await;
    store_key(&app, &token, SECRET, stub.base_url()).await;

    let (status, events) = common::sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        json!({ "message": "What absorbs light?" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the ceiling does not pause a BYOK account: {events:?}"
    );
    assert!(
        events.iter().any(|event| event.kind() == "done"),
        "and the turn completes"
    );
}

/// A phone cannot set a key: web/desktop only, the same rule as checkout.
#[sqlx::test(migrations = "../../migrations")]
async fn a_mobile_device_cannot_set_a_key(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (user_id, token) = account(&harness, "byok-phone@example.test", "power", "ios").await;

    let (status, body) = store_key(&app, &token, SECRET, "https://api.example.test/v1").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        body["code"], "feature_locked",
        "a phone is refused with the same code checkout uses (A143): {body}"
    );

    let stored: i64 =
        sqlx::query_scalar("select count(*)::bigint from user_api_keys where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, 0, "and nothing is written");

    // The rule is the platform, not the plan: the same account on a desktop is accepted.
    sqlx::query("update devices set platform = 'macos' where user_id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (status, body) = store_key(&app, &token, SECRET, "https://api.example.test/v1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A downgraded Power account keeps its key and quietly uses the server's provider: deleting a
/// secret is not something a plan change should do, and re-entering a key after a temporary
/// downgrade would be a support ticket.
#[sqlx::test(migrations = "../../migrations")]
async fn a_downgraded_account_keeps_its_key_and_uses_the_server_provider(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let (user_id, token) = account(&harness, "byok-downgrade@example.test", "power", "macos").await;

    store_key(&app, &token, SECRET, "https://api.example.test/v1").await;

    // The subscription drops to pro, so the plan no longer grants BYOK.
    sqlx::query("update subscriptions set plan_id = 'pro' where user_id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    // The key is still there and still described — it is the student's, and a downgrade does
    // not destroy it.
    let (_, described) = request(&app, "GET", "/settings/api-key", &token, json!(null)).await;
    assert_eq!(described["set"], true, "{described}");

    // But it is not used: `provider_for` answers `None` when the plan does not grant BYOK, so
    // the turn falls to the server's provider — which this test state has none of, and that
    // refusal is exactly the proof that the key was not used.
    let (workspace_id, _) = workspace_with_text(
        &app,
        &harness,
        &token,
        "downgraded",
        "Photosynthesis",
        "Chlorophyll absorbs light.",
    )
    .await;
    let (status, body) = chat_json(&app, &workspace_id, &token).await;
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "with no server provider configured the turn is refused: {body:?}"
    );
    assert_eq!(body["code"], "provider_error", "{body:?}");

    // And a downgraded account cannot *store* a new one, because the gate is the plan.
    let (status, body) = store_key(&app, &token, SECRET, "https://api.example.test/v1").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");
}

/// A key with no endpoint at all is refused rather than stored: a key with nowhere to send it
/// is a configuration the student has to fix, and storing it would make every turn fail
/// later with a worse message.
#[sqlx::test(migrations = "../../migrations")]
async fn a_key_with_no_endpoint_is_refused_on_use(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // The same server provider as the ceiling case, for the same reason: the route resolves
    // the provider before `provider_for` can refuse, so with none configured the request would
    // be refused for the wrong reason and the assertion would pass without testing the key.
    let scripted = std::sync::Arc::new(ScriptedLlm::new(vec!["Chlorophyll.".to_string()]));
    let harness = harness.with_llm(scripted);
    let app = router(&harness);
    let (_, token) = account(&harness, "byok-noendpoint@example.test", "power", "macos").await;

    // The test config has no `LLM_BASE_URL`, so a key with no `base_url` has nowhere to go.
    let (status, body) = request(
        &app,
        "PUT",
        "/settings/api-key",
        &token,
        json!({ "key": SECRET }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["base_url"].is_null(), "{body}");

    let (workspace_id, _) = workspace_with_text(
        &app,
        &harness,
        &token,
        "noendpoint",
        "Photosynthesis",
        "Chlorophyll absorbs light.",
    )
    .await;
    let (status, body) = chat_json(&app, &workspace_id, &token).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body:?}");
    assert_eq!(body["code"], "provider_error", "{body:?}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("endpoint"),
        "the refusal says what to fix: {body:?}"
    );
    let _ = Duration::seconds(0);
}
