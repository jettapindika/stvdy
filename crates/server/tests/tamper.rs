//! Every `docs/THREAT_MODEL.md` §5 row marked "**Fails.**", as a case asserting the
//! **exact** error code.
//!
//! §5 is the normative list — "the table to consult before adding any check" — so the
//! list is the input, not a memory: [`every_section_5_fails_row_has_a_case`] reads the
//! document, takes every row whose outcome begins "**Fails.**", and asserts each one is
//! named by a case in this file. Adding a "fails" row without a case fails there, which is
//! the only way the document and the suite stay the same claim.
//!
//! Four of these are the rows the roadmap calls "easy to assume rather than verify", and
//! they are why the suite exists: a client push of `Message` and of `Source`, the derived
//! card fields, the forged plan claim, and the `ios` checkout.
//!
//! Nothing here asserts "did not panic" or a bare status: every refusal is asserted by its
//! exact `code`, and the counted ones by their `limit` and `used`, because a bare 422
//! proves only that something failed.

// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Phase 0 Task 1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use chrono::Utc;
use secrecy::ExposeSecret;
use sqlx::PgPool;
use stvdy_core::config::PlanId;
use stvdy_core::entitlements::{Credits, Feature, PlanFacts, SubStatus, UsageSnapshot};
use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::auth::extract::AuthUser;
use stvdy_server::integrations::search::composite::CompositeSearch;
use stvdy_server::testing::TestState;
use stvdy_sync::proto::{Entity, Mutation, MutationResult, Op};
use uuid::Uuid;

mod common;
use common::{json, multipart, post_json, signed_up, sse_events};

/// The repository root, for the document the doc-driven case reads.
fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A caller built by hand. The pure boundaries take an `AuthUser`, so the platform a
/// decision sees is a field here exactly as the extractor fills it from `devices.platform`
/// (A144).
fn caller(plan_id: &str, platform: &str) -> AuthUser {
    AuthUser {
        user_id: Uuid::now_v7(),
        device_id: Uuid::now_v7(),
        session_id: Uuid::now_v7(),
        email: "tamper@example.test".to_string(),
        email_verified: true,
        plan_id: plan_id.to_string(),
        timezone: "Asia/Jakarta".to_string(),
        is_admin: false,
        student_verified: false,
        platform: platform.to_string(),
        subscription_status: SubStatus::None,
    }
}

/// A free caller's snapshot, for the pure push decision.
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

/// The wire code `decide` answered, so the assertion is on the value Phase 8 serialises.
fn rejection_code(result: Result<(), MutationResult>) -> String {
    match result {
        Err(MutationResult::Rejected { code, .. }) => code,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn decide(entity: Entity, payload: serde_json::Value) -> Result<(), MutationResult> {
    stvdy_server::sync::push::decide(
        &stvdy_server::testing::plans(),
        &PlanFacts {
            upgrade_offerable: true,
        },
        &free_snapshot(),
        false,
        &upsert(entity, payload),
    )
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// How many searches consumed the daily allowance: the uncached ones (A40).
async fn uncached_searches(harness: &TestState, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "select count(*)::bigint from usage_events
          where user_id = $1 and action = 'web_search' and cache_hit = false",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// Signs up with an explicit platform, because `signed_up` hardcodes macOS.
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

// ---------------------------------------------------------------------------------------
// Unlock a paid feature by editing UI state
// ---------------------------------------------------------------------------------------

/// Editing UI state cannot unlock a paid feature: the decision reads the plan from the
/// database, per request (A22), so a client that renders the feature anyway meets the
/// server's refusal on its next call.
///
/// One fact, two codes: on a desktop client the refusal names the plan that has the
/// feature (`plan_required`); on iOS/Android the same feature is `feature_locked`,
/// because the store rules forbid a purchase call to action there (A143). The positive
/// control at the end is a feature the free plan *does* have, so the loop cannot pass by
/// refusing everything.
#[sqlx::test(migrations = "../../migrations")]
async fn editing_ui_state_cannot_unlock_a_paid_feature(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    let locked = [
        Feature::Byok,
        Feature::GoogleCalendar,
        Feature::AgentAction,
        Feature::QuickCapture,
        Feature::TaskBreakdown,
        Feature::PlanGeneration,
        Feature::AdvancedAnalytics,
        Feature::ExportFormat(stvdy_core::config::ExportFormat::Pdf),
    ];

    let desktop = caller("free", "macos");
    for feature in locked {
        let error = stvdy_server::entitlements::require(&harness.state, &desktop, feature)
            .await
            .expect_err("a free account does not have this feature");
        assert_eq!(error.status(), StatusCode::FORBIDDEN, "{feature:?}");
        assert_eq!(error.code(), "plan_required", "{feature:?}");
    }

    let ios = caller("free", "ios");
    for feature in locked {
        let error = stvdy_server::entitlements::require(&harness.state, &ios, feature)
            .await
            .expect_err("a free account does not have this feature");
        assert_eq!(error.status(), StatusCode::FORBIDDEN, "{feature:?}");
        assert_eq!(error.code(), "feature_locked", "{feature:?}");
    }

    // The free plan's own features are granted, so the refusals above are the plan and
    // not a decision that refuses everything. `CreateWorkspace` is counted rather than
    // priced, so this caller (which has no ledger rows) is admitted on the count alone.
    stvdy_server::entitlements::require(&harness.state, &desktop, Feature::CreateWorkspace)
        .await
        .expect("a free account may create a workspace within its limit");
}

// ---------------------------------------------------------------------------------------
// Edit the JWT to add a plan or raise limits
// ---------------------------------------------------------------------------------------

/// The extra claims a modified client would add to its access token.
#[derive(Debug, serde::Serialize)]
struct ForgedClaims {
    sub: Uuid,
    did: Uuid,
    sid: Uuid,
    iat: i64,
    exp: i64,
    jti: Uuid,
    /// The claims a client would hope the server reads.
    plan: String,
    devices_max: u32,
    limits: serde_json::Value,
}

/// A token with a `plan` claim changes nothing: the plan is deliberately not a claim
/// (A22), so the numbers prove the claim was ignored rather than that something failed.
///
/// The account is free and at its two-workspace limit. The forged token is signed with the
/// server's own secret — so it *authenticates* — and carries `plan: "power"`,
/// `devices_max: 10` and a `limits` object. The refusal is still `limit_reached` with
/// `limit == 2` and `used == 2`, which is the free plan's number: a bare 422 would not
/// distinguish "the claim was ignored" from "the request was malformed".
#[sqlx::test(migrations = "../../migrations")]
async fn a_forged_plan_claim_in_the_token_changes_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "forger@example.test").await;

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

    // The real token, decoded so the forged one can name the same session and device.
    let claims = stvdy_server::auth::tokens::decode_jwt(&harness.state.config, &token)
        .expect("the real token decodes");
    let forged = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &ForgedClaims {
            sub: claims.sub,
            did: claims.did,
            sid: claims.sid,
            iat: Utc::now().timestamp(),
            exp: Utc::now().timestamp() + 3600,
            jti: Uuid::now_v7(),
            plan: "power".to_string(),
            devices_max: 10,
            limits: serde_json::json!({ "workspaces": "unlimited" }),
        },
        &jsonwebtoken::EncodingKey::from_secret(
            harness.state.config.jwt_secret.expose_secret().as_bytes(),
        ),
    )
    .expect("signing the forged token");

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &forged,
        serde_json::json!({ "name": "One too many" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the forged token must authenticate and still meet the free plan's limit: {body}"
    );
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(
        body["limit"], 2,
        "the free plan's limit, not the claim: {body}"
    );
    assert_eq!(body["used"], 2, "{body}");

    // The same request with the real token is refused identically, so the refusal is the
    // plan and not the forgery.
    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "One too many" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["limit"], 2, "{body}");
}

// ---------------------------------------------------------------------------------------
// Grant credits locally, or replay a grant
// ---------------------------------------------------------------------------------------

/// A client cannot grant itself credits: the balance is `sum(delta)` over `credit_ledger`,
/// written only by the server (A9), and a body carrying `credits`, `balance` or `plan`
/// moves nothing.
///
/// The assertion is the ledger's exact delta — the turn costs `chat_message = 1` and
/// nothing else — because a 200 would not distinguish "the fields were ignored" from "the
/// server granted 9999".
#[sqlx::test(migrations = "../../migrations")]
async fn a_client_cannot_grant_itself_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        "Gravity is a force.".to_string(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "selfgrant@example.test").await;
    let (workspace_id, _) =
        common::workspace_with_text(&app, &harness, &token, "Physics", "Notes", "F = ma").await;

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = (
             select id from users where email = 'selfgrant@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({
            "message": "what is gravity?",
            "credits": 9999,
            "balance": 9999,
            "plan": "power"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = (
             select id from users where email = 'selfgrant@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        before - after,
        stvdy_server::credits::chat_message_cost(&harness.state),
        "the turn costs exactly the chat_message price and nothing else"
    );

    let injected: i64 =
        sqlx::query_scalar("select count(*)::bigint from credit_ledger where delta = 9999")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(injected, 0, "no body field became a ledger row");

    // And the admin grant route is not a way in for a non-admin: `AdminUser` answers 404
    // (A7), so whether the endpoint exists is not something a non-admin learns.
    let user_id = user_id_of(&harness, "selfgrant@example.test").await;
    let (status, body) = json(
        &app,
        "POST",
        &format!("/admin/users/{user_id}/credits"),
        &token,
        serde_json::json!({ "credits": 9999, "idempotency_key": "self" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found", "{body}");
}

// ---------------------------------------------------------------------------------------
// Call the LLM / search / Google directly from the app
// ---------------------------------------------------------------------------------------

/// No response body carries a provider key.
///
/// The control is "no provider key exists on any device" (spec § PLATFORMS AND
/// ARCHITECTURE), and the server half of that is that nothing it answers with echoes the
/// key. A distinctive key is seeded into the configuration — asserted present, so the
/// negative assertion is not vacuous — and the swept routes plus a provider error are
/// driven with it.
#[sqlx::test(migrations = "../../migrations")]
async fn no_response_body_carries_a_provider_key(pool: PgPool) {
    const KEY: &str = "stvdy-provider-key-2f9c4a1e-do-not-leak";

    let mut harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    let mut config = (*harness.state.config).clone();
    config.llm = Some(stvdy_server::config::LlmConfig {
        base_url: "https://llm.invalid/v1".to_string(),
        api_key: KEY.to_string(),
        model: "test-model".to_string(),
    });
    harness.state.config = Arc::new(config);
    // The seeded key is really in the config: without this the assertions below could
    // pass while the key was never present to leak.
    assert_eq!(
        harness
            .state
            .config
            .llm
            .as_ref()
            .expect("the seeded provider config")
            .api_key,
        KEY
    );

    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "noleak@example.test").await;
    let (workspace_id, _) =
        common::workspace_with_text(&app, &harness, &token, "Notes", "Notes", "hello").await;

    // The provider is deliberately absent from `AppState` even though the config names
    // one, so the chat route answers its provider error: the error path is where a key is
    // most likely to be interpolated into a message.
    let (status, provider_error) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{provider_error}");
    assert_eq!(provider_error["code"], "provider_error", "{provider_error}");

    let mut bodies = vec![provider_error.to_string()];
    for (method, uri) in [
        ("GET", "/usage".to_string()),
        ("GET", "/workspaces".to_string()),
        ("GET", "/devices".to_string()),
        ("GET", "/auth/me".to_string()),
    ] {
        let (status, body) = json(&app, method, &uri, &token, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        bodies.push(body.to_string());
    }

    assert!(!bodies.is_empty());
    for body in &bodies {
        assert!(
            !body.contains(KEY),
            "a response body carried the provider key: {body}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Forge an assistant message with fabricated citations
// ---------------------------------------------------------------------------------------

/// `Message` is rejected with `read_only_entity`: it is persisted by the chat route, so
/// accepting a push of one would skip the credit spend and let a client forge assistant
/// turns with fabricated citations (roadmap § Sync protocol; A17).
#[test]
fn a_client_push_of_a_message_is_rejected() {
    let result = decide(
        Entity::Message,
        serde_json::json!({
            "role": "assistant",
            "content": "The mitochondria is the powerhouse of the cell [1].",
            "citations": [{ "label": 1, "chunk_id": Uuid::now_v7() }]
        }),
    );
    assert_eq!(rejection_code(result), "read_only_entity");
}

// ---------------------------------------------------------------------------------------
// Mark a source `ready`, or skip ingestion
// ---------------------------------------------------------------------------------------

/// `Source` is rejected: `status`, `file_key` and `read_only` are server-owned, so a
/// client must not be able to mark a source `ready` or skip the storage-quota check
/// (roadmap § Sync protocol).
#[test]
fn a_client_push_of_a_source_is_rejected() {
    let result = decide(
        Entity::Source,
        serde_json::json!({
            "status": "ready",
            "file_key": "users/00000000-0000-0000-0000-000000000000/sources/x.pdf",
            "file_size": 0
        }),
    );
    assert_eq!(rejection_code(result), "read_only_entity");
}

// ---------------------------------------------------------------------------------------
// Upload beyond the storage quota
// ---------------------------------------------------------------------------------------

/// The quota is checked server-side **before** the presign, so a refused upload leaves
/// nothing behind.
///
/// The refusal's `limit` is the plan's storage quota in bytes and its `used` is the sum
/// the upload path itself computes — the live bytes plus the file being refused — and the
/// source count is unchanged, which is what proves the check ran before the insert rather
/// than after it.
#[sqlx::test(migrations = "../../migrations")]
async fn an_upload_beyond_the_storage_quota_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quota@example.test").await;
    let user_id = user_id_of(&harness, "quota@example.test").await;

    let (status, created) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Full" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let workspace_id = created["workspace"]["id"].as_str().unwrap().to_string();

    let free = harness.state.config.plans.get(PlanId::Free);
    let quota = i64::from(free.storage_mb) * 1024 * 1024;
    // Two originals fill the free quota exactly, and stay under `sources_total` so the
    // storage axis is the one that refuses.
    let half = quota / 2;
    for index in 0..2 {
        sqlx::query(
            "insert into sources
                (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
             values ($1, $2, $3, 'text', $4, $5, $6, 'text/plain', 'ready')",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(Uuid::parse_str(&workspace_id).unwrap())
        .bind(format!("Filler {index}"))
        .bind(format!("users/{user_id}/sources/filler-{index}.txt"))
        .bind(half)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    let live: i64 = sqlx::query_scalar(
        "select coalesce(sum(file_size), 0)::bigint from sources
          where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live, quota, "the free quota is exactly full");
    let before: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let text = "a note";
    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", "One too many", None),
        ("text", text, None),
    ]);
    let (status, refused) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "limit_reached", "{refused}");
    assert_eq!(refused["limit"], quota, "{refused}");
    // The engine reports the sum it compared: the live bytes plus the file it refused.
    // (The refusal is `existing + new > quota`, so `used` is that total, not the live
    // bytes alone — the number the client renders is the one the comparison used.)
    assert_eq!(refused["used"], live + text.len() as i64, "{refused}");

    let after: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "the refusal happened before the source row was written"
    );
}

// ---------------------------------------------------------------------------------------
// Write `flashcards.ease/interval/due` directly
// ---------------------------------------------------------------------------------------

/// A push of a card carrying `ease`, `interval_days` or `due_at` is `read_only_entity`:
/// those are the output of the server-side SM-2 replay, so a client must not schedule its
/// own reviews (roadmap § Conflict rules).
///
/// The allowlist refuses a `flashcards` push first — the entity is not client-writable
/// either — and the derived-field guard is the second line behind it, so both the payload
/// and the entity are named here.
#[test]
fn a_push_of_derived_card_fields_is_rejected() {
    assert!(!Entity::Flashcard.is_client_writable());
    let result = decide(
        Entity::Flashcard,
        serde_json::json!({
            "front": "What is the powerhouse of the cell?",
            "back": "The mitochondrion",
            "ease": 2.9,
            "interval_days": 9999,
            "due_at": "2030-01-01T00:00:00Z"
        }),
    );
    assert_eq!(rejection_code(result), "read_only_entity");
}

// ---------------------------------------------------------------------------------------
// Exceed `web_searches_per_day` / `chat_msgs_per_day`
// ---------------------------------------------------------------------------------------

/// The counters are `usage_events`, server-side, and the refusal reports the cap and the
/// count.
///
/// Chat: the 21st message on free is `limit_reached` with `limit == 20`, `used == 20`.
/// Search: the 4th **uncached** search is `limit_reached` with `limit == 3`, `used == 3`,
/// and a cache hit does not consume the allowance (A40/A78) — a repeat of a question
/// already answered is the case the cache exists for.
#[sqlx::test(migrations = "../../migrations")]
async fn exceeding_a_daily_cap_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    let composite = CompositeSearch::with_tiers(
        vec![Arc::new(StubWebTier)],
        harness.state.pool.clone(),
        std::time::Duration::from_secs(3600),
    );
    let harness = harness
        .with_search(Arc::new(composite))
        .with_llm(Arc::new(ScriptedLlm::new(vec![
            "An answer.".to_string(),
            "An answer.".to_string(),
            "An answer.".to_string(),
        ])));

    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    // --- the chat cap -------------------------------------------------------------------
    let chat_token = signed_up(&app, &harness, "chatcap@example.test").await;
    let chat_user = user_id_of(&harness, "chatcap@example.test").await;
    let (workspace_id, _) =
        common::workspace_with_text(&app, &harness, &chat_token, "Chat", "Notes", "the material")
            .await;

    let chat_cap = harness
        .state
        .config
        .plans
        .get(PlanId::Free)
        .chat_msgs_per_day
        .max()
        .expect("free caps chat messages") as i64;
    for _ in 0..chat_cap {
        sqlx::query(
            "insert into usage_events (user_id, action, credits) values ($1, 'chat_message', 1)",
        )
        .bind(chat_user)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &chat_token,
        serde_json::json!({ "message": "one too many" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], chat_cap, "{body}");
    assert_eq!(body["used"], chat_cap, "{body}");

    // --- the search cap -----------------------------------------------------------------
    let search_token = signed_up(&app, &harness, "searchcap@example.test").await;
    let search_user = user_id_of(&harness, "searchcap@example.test").await;
    let (workspace_id, _) = common::workspace_with_text(
        &app,
        &harness,
        &search_token,
        "Search",
        "Notes",
        "the material",
    )
    .await;

    let search_cap = i64::from(
        harness
            .state
            .config
            .plans
            .get(PlanId::Free)
            .web_searches_per_day,
    );
    // Two uncached searches already ran; the third is the last the free plan allows.
    for _ in 0..(search_cap - 1) {
        sqlx::query(
            "insert into usage_events (user_id, action, credits, cache_hit)
             values ($1, 'web_search', 2, false)",
        )
        .bind(search_user)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    // The last slot is spent by a real, uncached search.
    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &search_token,
        serde_json::json!({ "message": "attention is all you need", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");
    assert_eq!(uncached_searches(&harness, search_user).await, search_cap);

    // The same query again is a cache hit: it answers, and it does not consume the
    // allowance, so the cap is still at `search_cap`.
    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &search_token,
        serde_json::json!({ "message": "attention is all you need", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");
    assert_eq!(
        uncached_searches(&harness, search_user).await,
        search_cap,
        "a cache hit must not consume the daily allowance (A40)"
    );

    // A fourth, different search is the refusal.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &search_token,
        serde_json::json!({ "message": "a completely different question", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], search_cap, "{body}");
    assert_eq!(body["used"], search_cap, "{body}");
}

/// A tier that returns one hit, so a web turn has something to cite without a network.
struct StubWebTier;

#[async_trait::async_trait]
impl SearchProvider for StubWebTier {
    fn name(&self) -> &'static str {
        "stub_web"
    }

    async fn search(&self, _query: &str, _max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        Ok(vec![SearchHit::new(
            "Attention Is All You Need",
            "https://example.test/recurrence",
            "The dominant sequence transduction models are based on complex recurrent networks.",
            SourceKind::Academic,
            "stub_web",
        )])
    }
}

// ---------------------------------------------------------------------------------------
// Enable checkout on iOS/Android
// ---------------------------------------------------------------------------------------

/// The server refuses `platform in (ios, android)` (A24), and **the platform is the
/// device row, never a body field** (A144): a request whose body says `platform: "web"`
/// from an `ios` device still refuses.
///
/// The route-level half drives the export-format refusal, which is the one route that
/// consults the platform today: an `ios` device gets `feature_locked` whatever the body
/// claims, and a macOS device gets `plan_required` even when its body claims `ios`.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ios_checkout_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let policy = harness.state.config.billing_policy.clone();

    let refused = stvdy_server::billing::checkout_allowed(&policy, "ios")
        .expect_err("an ios device must not be sold to");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.code(), "feature_locked");
    let refused = stvdy_server::billing::checkout_allowed(&policy, "android")
        .expect_err("an android device must not be sold to");
    assert_eq!(refused.code(), "feature_locked");
    assert!(stvdy_server::billing::checkout_allowed(&policy, "macos").is_ok());

    // The platform a decision sees comes from `devices.platform`, so a body that claims
    // otherwise changes nothing in either direction.
    let ios = signup_on(&app, &harness, "ios-checkout@example.test", "ios").await;
    let macos = signed_up(&app, &harness, "macos-checkout@example.test").await;
    let missing = Uuid::now_v7();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{missing}/pdf"),
        &ios,
        serde_json::json!({ "platform": "web" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an ios device claiming to be web must still refuse: {body}"
    );
    assert_eq!(body["code"], "feature_locked", "{body}");
    assert!(body["min_plan"].is_null(), "{body}");

    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{missing}/pdf"),
        &macos,
        serde_json::json!({ "platform": "ios" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a macOS device claiming to be ios is still offered the plan: {body}"
    );
    assert_eq!(body["code"], "plan_required", "{body}");
}

// ---------------------------------------------------------------------------------------
// Read another user's rows
// ---------------------------------------------------------------------------------------

/// Every query is scoped by `user_id`, so another account's row is `not_found` — the
/// exact code the client maps to "it may have been deleted, or it belongs to another
/// account" — rather than a 403 that confirms the row exists.
///
/// The owner reads the same rows successfully, so the 404s are about ownership and not a
/// broken route.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_rows_are_not_readable(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let alice = signed_up(&app, &harness, "alice@example.test").await;
    let bob = signed_up(&app, &harness, "bob@example.test").await;
    let bob_id = user_id_of(&harness, "bob@example.test").await;

    let workspace = Uuid::now_v7();
    let deck = Uuid::now_v7();
    let quiz = Uuid::now_v7();
    let source = Uuid::now_v7();
    let chunk = Uuid::now_v7();

    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'Bob''s course')")
        .bind(workspace)
        .bind(bob_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into decks (id, user_id, workspace_id, title, source_ids, status, requested_count)
         values ($1, $2, $3, 'Bob''s deck', array[$4]::uuid[], 'ready', 5)",
    )
    .bind(deck)
    .bind(bob_id)
    .bind(workspace)
    .bind(source)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into quizzes
            (id, user_id, workspace_id, title, difficulty, status, source_ids,
             requested_count, question_types)
         values ($1, $2, $3, 'Bob''s quiz', 'medium', 'ready', array[$4]::uuid[], 5, 'mcq')",
    )
    .bind(quiz)
    .bind(bob_id)
    .bind(workspace)
    .bind(source)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into sources
            (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
         values ($1, $2, $3, 'text', 'Bob''s source', $4, 16, 'text/plain', 'ready')",
    )
    .bind(source)
    .bind(bob_id)
    .bind(workspace)
    .bind(format!("users/{bob_id}/sources/{source}.txt"))
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into chunks (id, source_id, idx, content, content_hash, embedder)
         values ($1, $2, 0, 'Bob''s material', 'hash', 'test')",
    )
    .bind(chunk)
    .bind(source)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let routes = [
        format!("/workspaces/{workspace}"),
        format!("/decks/{deck}"),
        format!("/quizzes/{quiz}"),
        format!("/sources/{source}/status"),
        format!("/chunks/{chunk}"),
    ];

    for uri in &routes {
        let (status, body) = json(&app, "GET", uri, &alice, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert_eq!(body["code"], "not_found", "{uri}: {body}");
    }

    // The owner reads the same rows, so the 404s above are about ownership.
    for uri in &routes {
        let (status, body) = json(&app, "GET", uri, &bob, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{uri} (as its owner): {body}");
    }
}

// ---------------------------------------------------------------------------------------
// Exceed the provider-cost ceiling
// ---------------------------------------------------------------------------------------

/// The plan's provider-cost ceiling stops the spend, through the real route.
///
/// The ceiling is enforced inside the spend's own transaction (`credits::spend_in`), so
/// this drives the HTTP path rather than the pure engine: an account whose month has
/// already cost the plan's whole ceiling gets `422 limit_reached` naming the plan whose
/// ceiling is higher, and **nothing** is written — no ledger row and no second usage row.
///
/// A bare 422 would not distinguish "the ceiling stopped it" from "the request was
/// malformed", which is why the status, the code and `min_plan` are all asserted. The
/// ledger and usage assertions are the other half: a refusal that still charged would be
/// a refusal an attacker could not care less about.
///
/// `crates/server/tests/credits.rs` owns the deeper suite (the ceiling's exact comparison,
/// the BYOK exclusion, the higher plan's ceiling); this case exists because §5's row needs
/// a case in this file, and it is the one an HTTP client can reach.
#[sqlx::test(migrations = "../../migrations")]
async fn an_account_at_its_provider_cost_ceiling_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // The chat route resolves its provider *before* the spend (a server with no model
    // configured could never succeed on retry, so charging for it would take a credit for
    // nothing). A stub is therefore what gets this request as far as the ceiling check —
    // and the stub is never called, because the refusal happens first.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec!["An answer.".to_string()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "ceiling@example.test").await;
    let user_id = user_id_of(&harness, "ceiling@example.test").await;
    let (workspace_id, _) =
        common::workspace_with_text(&app, &harness, &token, "Ceiling", "Notes", "the material")
            .await;

    // The ceiling, read from the loaded matrix rather than written as a literal: a
    // retuned `config/plans.toml` must move this case with it.
    let ceiling = *harness
        .state
        .config
        .plans
        .provider_cost_ceiling_usd
        .get(&PlanId::Free)
        .expect("every plan has a ceiling (the config validates it)");
    sqlx::query(
        "insert into usage_events (user_id, action, credits, provider_cost_usd)
         values ($1, 'chat_message', 0, $2)",
    )
    .bind(user_id)
    .bind(ceiling)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let balance_before = balance_of(&harness, user_id).await;
    let events_before = usage_events_of(&harness, user_id).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "one more" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(
        body["min_plan"], "pro",
        "the refusal must name the plan whose ceiling is higher: {body}"
    );
    // Dollars are not plan-matrix numbers: a client that rendered these would read "the
    // limit is 0.5 and you have 0.5" (A148).
    assert!(body["limit"].is_null(), "{body}");
    assert!(body["used"].is_null(), "{body}");

    assert_eq!(
        balance_of(&harness, user_id).await,
        balance_before,
        "a spend refused by the ceiling must not write a ledger row"
    );
    assert_eq!(
        usage_events_of(&harness, user_id).await,
        events_before,
        "a spend refused by the ceiling must not record usage"
    );
}

/// The account's credit balance, as `sum(delta)` over the ledger (A9).
async fn balance_of(harness: &TestState, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// How many `usage_events` rows the account has, so "nothing was recorded" is countable.
async fn usage_events_of(harness: &TestState, user_id: Uuid) -> i64 {
    sqlx::query_scalar("select count(*)::bigint from usage_events where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------------------
// Farm signups past the per-IP limit
// ---------------------------------------------------------------------------------------

/// A signup farm from one address is refused, and the address is the **connection's**.
///
/// The limiter is database-backed (A154), so this drives real signups: the first
/// `SIGNUP_PER_IP_PER_DAY` succeed and the next is `429 rate_limited`. Every request
/// carries `x-forwarded-for` naming a different address, and the refusal proves the header
/// bought nothing — the peer address comes from `ConnectInfo`, which is what a listener
/// inserts and what a client cannot forge. That is the tamper half: a modified client's
/// cheapest move against a per-IP limit is to claim a second IP.
///
/// `crates/server/tests/abuse.rs::the_per_ip_signup_limit_trips_and_logs` owns the deeper
/// suite (the four fields of the log line, the database count, the public-domain
/// exemption); this case exists because §5's row needs a case in this file.
#[sqlx::test(migrations = "../../migrations")]
async fn signing_up_past_the_per_ip_limit_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let ip = "203.0.113.7";
    let limit = stvdy_server::ratelimit::SIGNUP_PER_IP_PER_DAY;
    for index in 0..limit {
        let (status, body) = signup_from(&app, ip, &format!("tamper{index}@ugm.ac.id")).await;
        assert_eq!(status, StatusCode::CREATED, "attempt {index}: {body}");
    }

    let (status, refused) = signup_from(&app, ip, "one-too-many@ugm.ac.id").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["code"], "rate_limited", "{refused}");
}

/// A signup carrying an explicit peer address, and a **false** `x-forwarded-for`.
///
/// The limiter reads the address from the connection (`ConnectInfo`), never from a
/// header, so supplying one the way a listener would is the only way to test a second IP
/// — and setting the header to a third address is the assertion that it is ignored.
async fn signup_from(app: &axum::Router, ip: &str, email: &str) -> (StatusCode, serde_json::Value) {
    let body = serde_json::to_vec(&serde_json::json!({
        "email": email,
        "password": "correct horse battery",
        "device": { "name": "Laptop", "platform": "macos" }
    }))
    .expect("serialize");
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/auth/signup")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "198.51.100.200")
        .body(axum::body::Body::from(body))
        .expect("request");
    let address = ip.parse::<std::net::IpAddr>().expect("a peer address");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
            address, 4242,
        )));

    let response = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

// ---------------------------------------------------------------------------------------
// The document is the input
// ---------------------------------------------------------------------------------------

/// Every §5 "Fails." row, by the exact text of its `Attempt` cell, and the case here that
/// covers it.
///
/// The pairing is the point: a row added to §5 without a case fails
/// [`every_section_5_fails_row_has_a_case`], and a case whose row is gone fails too, so
/// the document and the suite cannot drift into two different claims.
const COVERED: &[(&str, &str)] = &[
    (
        "Unlock a paid feature by editing UI state",
        "editing_ui_state_cannot_unlock_a_paid_feature",
    ),
    (
        "Edit the JWT to add a plan or raise limits",
        "a_forged_plan_claim_in_the_token_changes_nothing",
    ),
    (
        "Grant credits locally, or replay a grant",
        "a_client_cannot_grant_itself_credits",
    ),
    (
        "Call the LLM / search / Google directly from the app",
        "no_response_body_carries_a_provider_key",
    ),
    (
        "Forge an assistant message with fabricated citations",
        "a_client_push_of_a_message_is_rejected",
    ),
    (
        "Mark a source `ready`, or skip ingestion",
        "a_client_push_of_a_source_is_rejected",
    ),
    (
        "Upload beyond the storage quota",
        "an_upload_beyond_the_storage_quota_is_refused",
    ),
    (
        "Write `flashcards.ease/interval/due` directly",
        "a_push_of_derived_card_fields_is_rejected",
    ),
    (
        "Exceed `web_searches_per_day` / `chat_msgs_per_day`",
        "exceeding_a_daily_cap_is_refused",
    ),
    (
        "Enable checkout on iOS/Android",
        "an_ios_checkout_is_refused",
    ),
    (
        "Read another user's rows",
        "another_users_rows_are_not_readable",
    ),
    (
        "Exceed the provider-cost ceiling",
        "an_account_at_its_provider_cost_ceiling_is_refused",
    ),
    (
        "Farm signups past the per-IP limit",
        "signing_up_past_the_per_ip_limit_is_refused",
    ),
];

/// The `Attempt` cells of every §5 row whose outcome begins "**Fails.**".
fn fails_rows(document: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut in_section = false;
    for line in document.lines() {
        if line.starts_with("## ") {
            in_section = line.starts_with("## 5.");
            continue;
        }
        if !in_section || !line.starts_with("| ") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // cells[0] is the empty text before the leading `|`.
        let (Some(attempt), Some(outcome)) = (cells.get(1), cells.get(2)) else {
            continue;
        };
        if outcome.starts_with("**Fails.**") {
            rows.push((*attempt).to_string());
        }
    }
    rows
}

/// §5 is the normative list, so the list is the input, not a memory.
///
/// This reads `docs/THREAT_MODEL.md`, takes every row of §5 whose outcome begins
/// "**Fails.**", and asserts each one is named by a case in this file. Adding a "fails"
/// row without a case fails here, which is the only way the document and the test stay the
/// same claim.
#[test]
fn every_section_5_fails_row_has_a_case() {
    let document = std::fs::read_to_string(root().join("docs/THREAT_MODEL.md"))
        .expect("docs/THREAT_MODEL.md must be readable");
    let rows = fails_rows(&document);
    assert_eq!(
        rows.len(),
        13,
        "§5 must carry 13 \"Fails.\" rows; found {rows:?}"
    );

    for row in &rows {
        assert!(
            COVERED.iter().any(|(attempt, _)| attempt == row),
            "§5 row {row:?} has no case in tests/tamper.rs"
        );
    }

    let source = include_str!("tamper.rs");
    for (attempt, case) in COVERED {
        assert!(
            rows.iter().any(|row| row == attempt),
            "this table names a row §5 no longer has: {attempt:?}"
        );
        assert!(
            source.contains(&format!("fn {case}(")),
            "{case} is named for {attempt:?} but is not a case in this file"
        );
    }
}
