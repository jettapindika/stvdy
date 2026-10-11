//! Phase 2 chat: retrieval, citation validation, streaming, persistence.
//!
//! The suite runs against `ScriptedLlm`, so it needs no network and no provider
//! key. The property it leans on hardest is that the double **records the request
//! it received**: a test that only checks the response path cannot tell whether
//! the prompt actually contained the labels, which is exactly the assumption the
//! Phase 1 defect was made of.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{SseEvent, drain_jobs, json, multipart, post_json, signed_up, sse_events};

/// A workspace with one ingested text source, ready to answer from.
async fn workspace_with_source(
    app: &axum::Router,
    harness: &TestState,
    token: &str,
    text: &str,
) -> (String, String) {
    let (status, workspace) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": "Chat workspace" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workspace}");
    let workspace_id = workspace["workspace"]["id"].as_str().unwrap().to_string();

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", "Cell notes", None),
        ("text", text, None),
    ]);
    let (status, created) = post_json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // The ingest job runs to completion so retrieval has something to find.
    drain_jobs(harness).await;

    (
        workspace_id,
        created["source"]["id"].as_str().unwrap().to_string(),
    )
}

const NOTES: &str = "Mitochondria are the powerhouse of the cell. They generate most of \
the cell's supply of adenosine triphosphate through oxidative phosphorylation. The citric \
acid cycle feeds electrons into the electron transport chain across the inner membrane.";

fn chat_body(message: &str) -> serde_json::Value {
    serde_json::json!({ "message": message })
}

/// The happy path, and the exit gate's first half: a cited answer streams.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cited_answer_streams_and_is_persisted(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "chat@example.test").await;
    let (workspace_id, _source_id) = workspace_with_source(&app, &harness, &token, NOTES).await;

    // The model answers with a citation to label 1, which the server sent.
    let llm = Arc::new(ScriptedLlm::new(vec![
        "Mitochondria produce ATP through oxidative phosphorylation [1].".to_string(),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("How do mitochondria make energy?"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    // Order is part of the contract: tokens, then citations, then usage, then done.
    let kinds: Vec<&str> = events.iter().map(SseEvent::kind).collect();
    assert!(kinds.contains(&"token"), "no token event: {kinds:?}");
    assert!(
        kinds.contains(&"citations"),
        "no citations event: {kinds:?}"
    );
    assert!(kinds.contains(&"usage"), "no usage event: {kinds:?}");
    assert!(kinds.contains(&"done"), "no done event: {kinds:?}");
    assert!(
        kinds.iter().position(|k| *k == "citations").unwrap()
            < kinds.iter().position(|k| *k == "done").unwrap(),
        "citations must arrive before done"
    );

    let streamed: String = events
        .iter()
        .filter(|event| event.kind() == "token")
        .filter_map(|event| event.json()["t"].as_str().map(str::to_string))
        .collect();
    assert!(streamed.contains("oxidative phosphorylation"), "{streamed}");

    // The citations event resolves to a real chunk of this workspace.
    let citations = events
        .iter()
        .find(|event| event.kind() == "citations")
        .map(|event| event.json()["citations"].clone())
        .expect("a citations event");
    let array = citations.as_array().expect("an array");
    assert_eq!(array.len(), 1, "expected one citation: {citations}");
    assert_eq!(array[0]["type"], "chunk");
    // A pasted-text source has no page, so the citation's page is null. A PDF
    // would carry one; the point here is that the citation resolves to the real
    // chunk rather than to a page number the server invented.
    assert!(
        array[0]["page"].is_null(),
        "text sources have no page: {citations}"
    );
    let cited_source = array[0]["source_id"].as_str().unwrap();

    // And the prompt the model actually received carried the label block. This is
    // the assertion that a response-only test cannot make.
    let requests = llm.requests();
    assert_eq!(requests.len(), 1, "the provider was called once");
    let system = &requests[0].messages[0].content;
    assert!(
        system.contains("[1]"),
        "the prompt must label the chunks: {system}"
    );
    assert!(
        system.contains("Mitochondria"),
        "the prompt must carry the retrieved text"
    );
    assert!(
        !system.contains("{{"),
        "no prompt variable may survive composition"
    );

    // Persisted, with the resolved citation and not the raw label.
    let (status, thread) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "a user turn and an assistant turn");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(
        messages[1]["citations"][0]["source_id"].as_str().unwrap(),
        cited_source
    );
    // The stored text has the label stripped to a form the user can act on, and
    // never contains a raw marker for a chunk that was not sent.
    assert!(messages[1]["content"].as_str().unwrap().contains("[1]"));

    // The usage event's remaining matches the ledger.
    let ledger: i64 =
        sqlx::query_scalar("select coalesce(sum(delta), 0)::bigint from credit_ledger")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let reported = events
        .iter()
        .find(|event| event.kind() == "usage")
        .map(|event| event.json()["credits_remaining"].as_i64().unwrap())
        .expect("a usage event");
    assert_eq!(
        reported, ledger,
        "the reported balance must be the ledger sum"
    );
}

/// The exit gate's second half: the same account on another device sees the
/// thread. This is what "persisted" has to mean.
#[sqlx::test(migrations = "../../migrations")]
async fn the_thread_is_visible_from_a_second_device(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "two-devices@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let llm = Arc::new(ScriptedLlm::new(vec![
        "ATP is made in the mitochondria [1].".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("Where is ATP made?"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A second sign-in is a second device of the same account.
    let (status, second) = json(
        &app,
        "POST",
        "/auth/login",
        "",
        serde_json::json!({
            "email": "two-devices@example.test",
            "password": "correct horse battery",
            "device": { "name": "Phone", "platform": "ios" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let second_token = second["access_token"].as_str().unwrap().to_string();
    assert_ne!(second_token, token);

    let (status, thread) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &second_token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(
        thread["messages"].as_array().unwrap().len(),
        2,
        "the second device sees both turns"
    );
}

/// The roadmap's risk for this phase: citation hallucination. A fabricated label
/// must be dropped, not resolved to something plausible.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fabricated_citation_label_is_dropped(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "fabricate@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    // Label 9 was never sent: the context has one chunk, labelled [1].
    let llm = Arc::new(ScriptedLlm::new(vec![
        "A claim from nowhere [9] and a real one [1].".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("Tell me about the cell"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let citations = events
        .iter()
        .find(|event| event.kind() == "citations")
        .map(|event| event.json()["citations"].clone())
        .expect("a citations event");
    let array = citations.as_array().unwrap();
    assert_eq!(array.len(), 1, "only the real label survives: {citations}");
    // A pasted-text source has no page, so the citation's page is null rather
    // than an invented number.
    assert!(array[0]["page"].is_null(), "{citations}");

    // The unknown marker is gone from the stored text too: a user must not see a
    // citation they cannot open.
    let (_, thread) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let stored = thread["messages"][1]["content"].as_str().unwrap();
    assert!(
        !stored.contains("[9]"),
        "the fabricated label must not survive in the stored text: {stored}"
    );
    assert!(stored.contains("[1]"), "the real label stays: {stored}");
}

/// A43: chat is one of the four actions an unverified account cannot take.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unverified_account_cannot_chat(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, body) = json(
        &app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": "unverified-chat@example.test",
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["access_token"].as_str().unwrap().to_string();

    let (status, workspace) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Unverified" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let workspace_id = workspace["workspace"]["id"].as_str().unwrap().to_string();

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("hello"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{rejected}");
    assert_eq!(rejected["code"], "email_not_verified");

    // Nothing spent, nothing written.
    let events: i64 = sqlx::query_scalar("select count(*) from usage_events")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let messages: i64 = sqlx::query_scalar("select count(*) from messages")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!((events, messages), (0, 0));
}

/// A spent-out account is refused with 402, and the refusal is clean: no message
/// was written, so the client can retry after topping up without a duplicate turn.
#[sqlx::test(migrations = "../../migrations")]
async fn a_spent_out_account_is_refused_without_writing_anything(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "broke@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    // Drain the balance: the free plan's grant is spent one message at a time, so
    // a single ledger row is simpler and deterministic.
    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason) values ($1, $2, 'monthly', 'test')",
    )
    .bind(user_id)
    .bind(-1_000)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let llm = Arc::new(ScriptedLlm::new(vec!["unused".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("anything"),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{rejected}");
    assert_eq!(rejected["code"], "insufficient_credits");

    let messages: i64 = sqlx::query_scalar("select count(*) from messages")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(messages, 0, "a refused request writes no message");
    assert!(
        llm.requests().is_empty(),
        "the provider must not be called when the request is refused"
    );
}

/// Another account's workspace is a 404, never a 403: existence must not leak.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_workspace_is_not_chattable(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let owner = signed_up(&app, &harness, "owner-chat@example.test").await;
    let intruder = signed_up(&app, &harness, "intruder-chat@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &owner, NOTES).await;

    let llm = Arc::new(ScriptedLlm::new(vec!["nope".to_string()]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &intruder,
        chat_body("read it"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "chat leaked");

    let (status, _) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &intruder,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the message list leaked");
}

/// The daily cap is per the user's local day, and it stops the request before the
/// provider is called.
#[sqlx::test(migrations = "../../migrations")]
async fn the_daily_message_cap_trips(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "daily@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let cap = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .chat_msgs_per_day
        .max()
        .expect("the free plan caps daily messages");

    // Plenty of credits, so the cap is what trips rather than the balance.
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason) values ($1, 500, 'monthly', 'test')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // `usage_events` rows are what the counter reads, so the cap is simulated by
    // writing them rather than by sending `cap` real messages.
    for _ in 0..cap {
        sqlx::query(
            "insert into usage_events (user_id, action, credits) values ($1, 'chat_message', 1)",
        )
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    let llm = Arc::new(ScriptedLlm::new(vec!["unused".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("one more"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{rejected}");
    assert_eq!(rejected["code"], "limit_reached");
    assert_eq!(rejected["min_plan"], "pro");
    assert_eq!(rejected["limit"], cap as i64);
    assert!(llm.requests().is_empty());
}

/// A workspace with no sources still composes a prompt, and the model is told
/// there is nothing to ground on rather than being handed an empty block.
#[sqlx::test(migrations = "../../migrations")]
async fn a_workspace_with_no_sources_still_composes_a_prompt(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "empty@example.test").await;

    let (_, workspace) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Empty" }),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().unwrap().to_string();

    let llm = Arc::new(ScriptedLlm::new(vec![
        "This isn't covered in your selected sources.".to_string(),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("What is entropy?"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    let system = &requests[0].messages[0].content;
    assert!(
        system.contains("<sources>"),
        "the sources block must be present even when empty"
    );
    // With nothing retrieved there is nothing to cite, and the event still
    // arrives so a client can tell "no citations" from "the stream died".
    let citations = events
        .iter()
        .find(|event| event.kind() == "citations")
        .map(|event| event.json()["citations"].clone())
        .expect("a citations event");
    assert_eq!(citations.as_array().unwrap().len(), 0);
}

/// A provider failure surfaces as the documented error event, not as a dropped
/// stream, and the user's message is still stored so the turn is not lost.
#[sqlx::test(migrations = "../../migrations")]
async fn a_provider_failure_is_reported_as_an_error_event(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "provider-down@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let llm = Arc::new(ScriptedLlm::failing("upstream refused the connection"));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("hello"),
    )
    .await;

    // The stream has not started, so this is an ordinary HTTP error with the
    // documented shape rather than a mid-stream event.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "provider_error");

    // The user's turn is stored: the spend happened and the message is theirs.
    let messages: Vec<String> = sqlx::query_scalar("select role from messages order by created_at")
        .fetch_all(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        messages,
        vec!["user".to_string()],
        "no half-answer is stored"
    );
}

/// A workspace that does not exist is a 404 before any credit is spent.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_workspace_is_not_found_and_costs_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "missing@example.test").await;

    let llm = Arc::new(ScriptedLlm::new(vec!["unused".to_string()]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = json(
        &app,
        "POST",
        &format!("/workspaces/{}/chat", uuid::Uuid::now_v7()),
        &token,
        chat_body("hello"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let ledger: i64 =
        sqlx::query_scalar("select coalesce(sum(delta), 0)::bigint from credit_ledger")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    // The baseline is the signup grant, not zero: the assertion is that the
    // request spent nothing, not that the account has nothing.
    let allowance = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .credits_per_month;
    assert_eq!(
        ledger, allowance,
        "nothing may be spent on a request that failed a check"
    );
}

/// The message body is validated before anything else, so an empty message is not
/// a way to spend a credit.
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_message_is_rejected_before_spending(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "empty-msg@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let llm = Arc::new(ScriptedLlm::new(vec!["unused".to_string()]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("   "),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(rejected["code"], "validation");

    let ledger: i64 =
        sqlx::query_scalar("select coalesce(sum(delta), 0)::bigint from credit_ledger")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let allowance = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .credits_per_month;
    assert_eq!(
        ledger, allowance,
        "an empty message must not spend a credit"
    );
}

/// The `source_ids` filter narrows retrieval, which is the spec's "optionally
/// limited to user-selected sources".
#[sqlx::test(migrations = "../../migrations")]
async fn the_source_filter_narrows_retrieval(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "filter@example.test").await;

    let (_, workspace) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Filtered" }),
    )
    .await;
    let workspace_id = workspace["workspace"]["id"].as_str().unwrap().to_string();

    // Two sources that both mention the query term, so only the filter can
    // decide which one is retrieved.
    let mut source_ids = Vec::new();
    for (title, text) in [
        (
            "Alpha",
            "Photosynthesis converts light into chemical energy in chloroplasts.",
        ),
        (
            "Beta",
            "Photosynthesis in cyanobacteria happens in the thylakoid membrane.",
        ),
    ] {
        let (content_type, body) = multipart(&[
            ("kind", "text", None),
            ("title", title, None),
            ("text", text, None),
        ]);
        let (status, created) = post_json(
            &app,
            "POST",
            &format!("/workspaces/{workspace_id}/sources"),
            &token,
            Some(&content_type),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        source_ids.push(created["source"]["id"].as_str().unwrap().to_string());
    }

    drain_jobs(&harness).await;

    let llm = Arc::new(ScriptedLlm::new(vec!["Answer [1].".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({
            "message": "photosynthesis",
            "source_ids": [source_ids[1]],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let requests = llm.requests();
    let system = &requests[0].messages[0].content;
    assert!(
        system.contains("cyanobacteria"),
        "the selected source must be in the context: {system}"
    );
    assert!(
        !system.contains("chloroplasts"),
        "the unselected source must not be: {system}"
    );
}

/// The streamed text must never contain a marker the server has rejected.
///
/// This is the invariant `docs/API.md` states and the browser run caught being
/// false: the token loop used to yield every token verbatim and strip only the
/// persisted copy, so a fabricated `[9]` was on screen from the token that carried
/// it until `done` replaced the live turn. Asserted on the token events, which is
/// what the user actually sees while the answer arrives.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fabricated_marker_never_reaches_the_streamed_text(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "stream-labels@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    // Label 9 was never sent. The answer is scripted as one string, which the
    // double streams as a single token, so this also covers the case where the
    // whole marker arrives at once rather than one character at a time.
    let llm = Arc::new(ScriptedLlm::new(vec![
        "A grounded claim [1] and a fabricated one [9].".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("Tell me about the cell"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let streamed: String = events
        .iter()
        .filter(|event| event.kind() == "token")
        .filter_map(|event| event.json()["t"].as_str().map(str::to_string))
        .collect();

    assert!(
        !streamed.contains("[9]"),
        "the streamed text must not contain a rejected marker: {streamed:?}"
    );
    assert!(
        streamed.contains("[1]"),
        "a label that was sent must still reach the user: {streamed:?}"
    );
    assert!(
        streamed.contains("grounded claim") && streamed.contains("fabricated one"),
        "no ordinary text may be lost to the hold-back: {streamed:?}"
    );
    // The hold-back must not have swallowed the sentence's end either.
    assert!(streamed.trim_end().ends_with('.'), "{streamed:?}");
}

/// A marker split across two tokens is held back until it is complete, then
/// either emitted or dropped. The client never sees a half-marker.
#[sqlx::test(migrations = "../../migrations")]
async fn a_marker_split_across_tokens_is_never_shown_partial(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "split-marker@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    // Explicit fragments, because the double's default tokenisation splits on
    // whitespace and so can never put a marker across a boundary. A real provider
    // does, which is the case the hold-back exists for.
    let llm = Arc::new(ScriptedLlm::with_tokens(vec![
        "The claim [".to_string(),
        "9".to_string(),
        "] is not grounded.".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("anything"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let streamed: String = events
        .iter()
        .filter(|event| event.kind() == "token")
        .filter_map(|event| event.json()["t"].as_str().map(str::to_string))
        .collect();

    assert!(
        !streamed.contains("[9]"),
        "a split marker must not appear once assembled: {streamed:?}"
    );
    // The answer has no known labels, so no bracket may survive at all: not the
    // assembled marker, and not the partial one the hold-back is holding.
    assert!(
        !streamed.contains('['),
        "no partial or rejected marker may reach the user: {streamed:?}"
    );
    assert!(
        streamed.contains("The claim") && streamed.contains("not grounded"),
        "no ordinary text may be lost to the hold-back: {streamed:?}"
    );

    // The route slices per *token* while the core simulation slices per
    // character, so the equality property has to be asserted here too: a
    // per-character proof does not cover a different partition, and two earlier
    // attempts at this were reasoned-correct and then failed on the real one.
    let (_, thread) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let stored = thread["messages"][1]["content"]
        .as_str()
        .expect("the assistant turn");

    assert_eq!(
        streamed, stored,
        "the user must not watch the text change when the answer is finalised"
    );
}

/// A spent-out account's 402 carries the plan-matrix sense of `limit`/`used`,
/// which is "not applicable", and names the plan that would lift it. Putting the
/// action's cost and the balance in those fields would make the client render
/// "the limit is 1, you have 59".
#[sqlx::test(migrations = "../../migrations")]
async fn a_spent_out_account_gets_an_upgrade_hint_and_no_confusing_numbers(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "hint@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason) values ($1, -1000, 'monthly', 'test')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let llm = Arc::new(ScriptedLlm::new(vec!["unused".to_string()]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("anything"),
    )
    .await;

    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{rejected}");
    assert_eq!(rejected["code"], "insufficient_credits");
    assert_eq!(
        rejected["min_plan"], "pro",
        "a spent-out free user must be told which plan has a bigger allowance"
    );
    assert!(
        rejected["limit"].is_null() && rejected["used"].is_null(),
        "these fields are the plan-matrix sense, not this action's cost and balance: {rejected}"
    );
    // The numbers the user needs are in the message.
    let message = rejected["message"].as_str().unwrap();
    assert!(message.contains("1 credit"), "{message}");
}

/// A server with no model configured cannot succeed on retry, so it must not
/// charge for the attempt. The shipped `.env.example` has no `LLM_BASE_URL`, so
/// this is the default state of a fresh checkout.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unconfigured_provider_refuses_before_spending(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // `TestState` defaults to `llm: None`, which is the case under test.
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "no-provider@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("hello"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{rejected}");
    assert_eq!(rejected["code"], "provider_error");

    // Nothing spent, nothing written: the check runs before both.
    let ledger: i64 =
        sqlx::query_scalar("select coalesce(sum(delta), 0)::bigint from credit_ledger")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let allowance = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .credits_per_month;
    assert_eq!(ledger, allowance, "an unconfigured server must not spend");

    let messages: i64 = sqlx::query_scalar("select count(*) from messages")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(messages, 0, "and must not leave a turn with no answer");
}

/// `use_web` is refused until Phase 3 rather than accepted and ignored: accepting
/// it would tell the model it has web context, store `used_web: true`, and let the
/// model claim to have searched, none of which happened.
#[sqlx::test(migrations = "../../migrations")]
async fn use_web_is_refused_until_the_search_arm_exists(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "web@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    let llm = Arc::new(ScriptedLlm::new(vec!["unused".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, rejected) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "look this up", "use_web": true }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(rejected["code"], "validation");
    assert!(llm.requests().is_empty(), "the model must not be called");

    let ledger: i64 =
        sqlx::query_scalar("select coalesce(sum(delta), 0)::bigint from credit_ledger")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let allowance = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .credits_per_month;
    assert_eq!(ledger, allowance, "a refused request must not spend");

    let messages: i64 = sqlx::query_scalar("select count(*) from messages")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(messages, 0, "and must not store a turn claiming web use");
}

/// An empty answer must still terminate the stream. Without a terminator a client
/// waits for a `done` that never arrives, because `done` is the end of the turn.
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_answer_still_terminates_the_stream(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "empty-answer@example.test").await;
    let (workspace_id, _) = workspace_with_source(&app, &harness, &token, NOTES).await;

    // Whitespace only: nothing is persisted, so `done` cannot be sent, but the
    // stream must still say what happened.
    let llm = Arc::new(ScriptedLlm::new(vec!["   ".to_string()]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        chat_body("anything"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let kinds: Vec<&str> = events.iter().map(SseEvent::kind).collect();
    assert!(
        kinds.contains(&"error") || kinds.contains(&"done"),
        "every exit path must emit exactly one terminator, got {kinds:?}"
    );
    if let Some(error) = events.iter().find(|event| event.kind() == "error") {
        assert_eq!(error.json()["code"], "provider_error");
    }

    // And no assistant turn was stored for an answer that had nothing in it.
    let messages: Vec<String> = sqlx::query_scalar("select role from messages order by created_at")
        .fetch_all(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(messages, vec!["user".to_string()]);
}
