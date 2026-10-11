//! Phase 4 flashcards: generation, the JSON contract, review replay, due cards.
//!
//! Runs against `ScriptedLlm`, so the JSON the model returns is scripted and the
//! suite asserts on both halves of the exchange: what the prompt contained (via
//! `requests()`) and what was stored.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{drain_jobs, json, signed_up, workspace_with_text};

const INDONESIAN: &str = "TCP memakai three-way handshake untuk membuka koneksi. \
UDP tidak memakai handshake sehingga lebih cepat tetapi tidak menjamin urutan paket.";

/// One valid reply, as `prompts/flashcards.v1.md` documents it.
fn reply(front: &str, back: &str, label: u32) -> String {
    serde_json::json!({ "cards": [{ "front": front, "back": back, "label": label }] }).to_string()
}

async fn deck_created(
    app: &axum::Router,
    workspace_id: &str,
    token: &str,
    source_id: &str,
    count: u32,
) -> (StatusCode, serde_json::Value) {
    json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/decks"),
        token,
        serde_json::json!({
            "title": "Jaringan",
            "source_ids": [source_id],
            "count": count,
        }),
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_generates_cards_linked_to_their_chunk(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa fungsi three-way handshake?",
        "Membuka koneksi TCP.",
        1,
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "deck@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["deck"]["status"], "generating");

    drain_jobs(&harness).await;

    let (status, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deck}");
    assert_eq!(deck["deck"]["status"], "ready", "{deck}");
    assert_eq!(deck["deck"]["card_count"], 1);
    let card = &deck["cards"][0];
    assert_eq!(card["front"], "Apa fungsi three-way handshake?");
    // The card links to a real chunk of the Indonesian source.
    assert!(!card["chunk_id"].is_null(), "{card}");

    // And the prompt that produced it carried the label block and the count.
    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    let prompt = &requests[0].messages[0].content;
    assert!(prompt.contains("[1]"), "no label block: {prompt}");
    assert!(prompt.contains("10 cards requested"), "{prompt}");
    assert!(
        prompt.contains("the same language as the source material"),
        "{prompt}"
    );
    assert!(requests[0].json_mode, "generation must ask for JSON");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_invalid_reply_is_retried_once_and_then_succeeds(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "Sure! Here are your cards.".to_string(),
        reply("Apa itu UDP?", "Protokol tanpa koneksi.", 1),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "retry@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 5).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(deck["deck"]["status"], "ready", "{deck}");
    assert_eq!(llm.requests().len(), 2, "the second attempt must happen");
    // The retry carries the bad reply and a correction, not the same prompt twice.
    let second = &llm.requests()[1].messages;
    // The retry carries the original system prompt, the bad reply as an assistant
    // turn, and the correction. Three messages, not a second copy of the system
    // prompt: repeating the instruction unchanged is what makes a model answer the
    // same way again.
    assert_eq!(second.len(), 3, "{second:?}");
    assert_eq!(second[1].content, "Sure! Here are your cards.");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_generation_that_fails_for_good_refunds_the_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "not json".to_string(),
        "still not json".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "refund@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(deck["deck"]["status"], "failed", "{deck}");
    assert!(
        deck["deck"]["error"].as_str().unwrap().contains("JSON"),
        "the error must say what happened: {deck}"
    );

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(after, before, "the 4 credits must come back exactly once");
}

/// A reply whose cards all resolve to nothing must not become a paid, empty deck.
///
/// `resolve_cards` is infallible: it drops a card with an empty front or back and
/// records nothing, so a reply of whitespace-only cards parses cleanly and resolves
/// to zero. Without the check in `generate`, `insert_cards` would mark the deck
/// `ready` with no cards and keep the charge.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reply_with_only_empty_cards_fails_and_refunds(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        r#"{"cards":[{"front":" ","back":"","label":1}]}"#.to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "empty@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'empty@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(deck["deck"]["status"], "failed", "{deck}");
    assert_eq!(deck["deck"]["card_count"], 0);
    assert!(
        deck["deck"]["error"]
            .as_str()
            .unwrap()
            .contains("no usable cards"),
        "the error must say what happened: {deck}"
    );

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'empty@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(after, before, "an empty deck must not keep the credits");
}

/// A failed generation leaves an honest `usage_events` row: the action happened, the
/// provider was called, and the prompt version is recorded. A row that vanishes when
/// the job fails would hide a real cost from the margin report (A41).
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_generation_still_records_its_usage_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "not json".to_string(),
        "still not json".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "usage@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    let _deck_id = body["deck"]["id"].as_str().unwrap();
    drain_jobs(&harness).await;

    let (action, prompt_version): (String, Option<String>) = sqlx::query_as(
        "select action, prompt_version from usage_events
          where user_id = (select id from users where email = 'usage@example.test')
            and action = 'generate_flashcards'
          order by created_at desc limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(action, "generate_flashcards");
    assert_eq!(
        prompt_version.as_deref(),
        Some("flashcards.v1"),
        "a model call must be attributable to a prompt version"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_card_whose_label_was_not_sent_is_stored_without_a_chunk(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        99,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "label@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 5).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    // The deck is still usable: a fabricated label costs the link, not the card.
    assert_eq!(deck["deck"]["status"], "ready", "{deck}");
    assert!(deck["cards"][0]["chunk_id"].is_null(), "{deck}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_monthly_card_limit_is_enforced(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "limit@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    // Free allows 30 a month; 31 in one request is refused before anything is spent.
    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 31).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 30);
}
