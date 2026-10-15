//! Phase 4 flashcards: generation, the JSON contract, review replay, due cards.
//!
//! Runs against `ScriptedLlm`, so the JSON the model returns is scripted and the
//! suite asserts on both halves of the exchange: what the prompt contained (via
//! `requests()`) and what was stored.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::Clock;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;
use uuid::Uuid;

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

/// A deck that cannot be created must not keep the charge.
///
/// The insert is the last step after the spend, so a rejected insert (here a title
/// carrying a NUL byte, which JSON can express and Postgres text cannot) would
/// otherwise leave credits spent with no deck, no job and no refund.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_that_cannot_be_created_refunds_the_charge(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nul@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'nul@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // The NUL passes the title's length check and reaches the insert, where Postgres
    // rejects it.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::json!({
            "title": "bad\u{0}title",
            "source_ids": [source_id],
            "count": 10,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'nul@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "a deck that was never created must not keep the credits"
    );

    // And no deck was left behind for the job to pick up.
    let decks: i64 = sqlx::query_scalar("select count(*) from decks")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(decks, 0);
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

    // Joined to the deck rather than filtered on `action`: a query that filters on
    // the value it then asserts proves nothing. This asserts that the row the deck
    // names is the row the generation wrote.
    let (prompt_version, tokens_in, deck_usage_event): (
        Option<String>,
        Option<i32>,
        Option<uuid::Uuid>,
    ) = sqlx::query_as(
        "select ue.prompt_version, ue.tokens_in, d.usage_event_id
           from usage_events ue
           join decks d on d.usage_event_id = ue.id
          where ue.action = 'generate_flashcards'
            and ue.user_id = (select id from users where email = 'usage@example.test')
          order by ue.created_at desc limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        prompt_version.as_deref(),
        Some("flashcards.v1"),
        "a model call must be attributable to a prompt version"
    );
    // The calls happened and were billed, so the counts are recorded. `0` and NULL
    // are different things here: the `ScriptedLlm` reports no cost, but a NULL would
    // say the row was never attributed to a call at all.
    assert_eq!(
        tokens_in,
        Some(0),
        "a failed generation must still record what its calls reported"
    );
    assert!(
        deck_usage_event.is_some(),
        "the deck must name the row it charged"
    );
}

/// A second job for a deck that is already finished must not write a second set of
/// cards. The deck's status is the idempotency marker.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_job_for_a_finished_deck_writes_no_more_cards(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "twice@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 5).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    // The same job again, as a duplicate claim (an overlapping deploy plus the boot
    // requeue sweep) would produce.
    stvdy_server::jobs::enqueue(
        &harness.state.pool,
        &harness.state.jobs,
        "generate_flashcards",
        serde_json::json!({ "deck_id": deck_id }),
        None,
    )
    .await
    .unwrap();
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
    assert_eq!(
        deck["deck"]["card_count"], 1,
        "a re-claimed deck must not be written twice: {deck}"
    );
    assert_eq!(deck["cards"].as_array().unwrap().len(), 1, "{deck}");
}

/// A delivered deck must not be refundable.
///
/// The refund guard is `credits_spent > 0`, and `insert_cards` zeroes it as part of
/// the claim. Without that, a second worker that loaded the deck while it was still
/// `generating`, called the model, and then failed could flip an already-`ready`,
/// card-bearing deck to `failed` and refund a generation the user received.
#[sqlx::test(migrations = "../../migrations")]
async fn a_delivered_deck_cannot_be_refunded(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "delivered@example.test").await;
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
    assert_eq!(deck["deck"]["card_count"], 1, "{deck}");

    let deck_uuid = Uuid::parse_str(&deck_id).unwrap();
    let user_id: Uuid =
        sqlx::query_scalar("select id from users where email = 'delivered@example.test'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    let ledger_before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // Exactly what the loser of a claim race reaches: it loaded the deck while it
    // was `generating`, called the model, failed for its own reason, and still
    // carries the charge it read.
    stvdy_server::jobs::handlers::generate::fail_and_refund(
        &harness.state,
        deck_uuid,
        user_id,
        4,
        "the loser of a claim race failed for its own reason",
    )
    .await;

    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        deck["deck"]["status"], "ready",
        "a delivered deck must not be flipped to failed: {deck}"
    );
    assert_eq!(deck["deck"]["card_count"], 1, "{deck}");

    let ledger_after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        ledger_after, ledger_before,
        "a delivered generation must not be refunded"
    );

    let credits_spent: i32 = sqlx::query_scalar("select credits_spent from decks where id = $1")
        .bind(deck_uuid)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(credits_spent, 0, "the charge stays consumed");
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

/// The exit gate: two clients reviewing one card produce the state a local replay
/// of the same rows produces, whatever order the rows arrived in.
///
/// The two reviews are sent **later-first**, which is what a second device whose
/// clock ran ahead produces. That order is the whole test: a route that never
/// re-reads the history it has already stored — one that applied the arriving
/// review to the card's stored `ease`/`interval_days`, or dated the result from the
/// incoming `reviewed_at` — would end at `ten_min_ago + 6 days`, while a full replay
/// ends at `now + 6 days`. In chronological arrival order the two agree, so the gate
/// would not see the difference.
///
/// The expected state comes from `stvdy_core::sm2::replay` run here over the same
/// two reviews, and from hand-derived literals beside it, so the assertion is
/// against the SM-2 rules rather than against the code under test. The review ids in
/// the local replay are arbitrary: the route mints them when the body has no `id`,
/// and the replay orders by `(reviewed_at, id)`, so the ids only decide anything if
/// two reviews share a timestamp.
#[sqlx::test(migrations = "../../migrations")]
async fn reviews_from_two_devices_replay_to_identical_state(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol transport.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "two@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
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
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    // Derived from the injected clock, not from an absolute literal: the route
    // refuses a `reviewed_at` more than 24 h ahead of that clock, so a hard-coded
    // date would make this gate fail on a machine whose clock is behind. Truncated
    // to whole seconds because `timestamptz` has microsecond resolution and rounds,
    // so a sub-second instant would come back a value this test cannot predict.
    let now = chrono::DateTime::from_timestamp(harness.clock.now().timestamp(), 0).unwrap();
    let ten_min_ago = now - chrono::Duration::minutes(10);

    // Device B's clock runs ten minutes ahead of device A's, so its later review
    // arrives first.
    let (status, later) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        serde_json::json!({ "rating": "easy", "reviewed_at": now.to_rfc3339() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{later}");
    assert_eq!(later["applied"], true, "{later}");

    let (status, earlier) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        serde_json::json!({ "rating": "good", "reviewed_at": ten_min_ago.to_rfc3339() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{earlier}");
    assert_eq!(
        earlier["applied"], true,
        "a review that arrives late is still a new row: {earlier}"
    );

    // The stored rows are the two that were sent, ordered the way the replay orders
    // them — so the replay input is the stored fact, not a second copy of what this
    // test meant to send.
    let rows: Vec<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "select rating, reviewed_at from flashcard_reviews
          where flashcard_id = $1::uuid order by reviewed_at asc, id asc",
    )
    .bind(&card_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![("good".to_string(), ten_min_ago), ("easy".to_string(), now)],
        "the earlier review sorts first whatever order the rows arrived in"
    );

    let created_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select created_at from flashcards where id = $1::uuid")
            .bind(&card_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let local = stvdy_core::sm2::replay(
        created_at,
        &[
            stvdy_core::sm2::Review {
                id: uuid::Uuid::from_u128(1),
                rating: stvdy_core::sm2::Rating::Good,
                reviewed_at: ten_min_ago,
            },
            stvdy_core::sm2::Review {
                id: uuid::Uuid::from_u128(2),
                rating: stvdy_core::sm2::Rating::Easy,
                reviewed_at: now,
            },
        ],
    );

    // The response to the second request — the earlier review — and the row a
    // third device would fetch.
    let responded: (f32, i32, chrono::DateTime<chrono::Utc>) = (
        earlier["card"]["ease"].as_f64().unwrap() as f32,
        earlier["card"]["interval_days"].as_i64().unwrap() as i32,
        earlier["card"]["due_at"].as_str().unwrap().parse().unwrap(),
    );
    let persisted: (f32, i32, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("select ease, interval_days, due_at from flashcards where id = $1::uuid")
            .bind(&card_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    for (label, stored) in [("response", responded), ("row", persisted)] {
        assert!(
            (stored.0 - local.ease).abs() < 1e-5,
            "{label}: {stored:?} vs {local:?}"
        );
        assert_eq!(stored.1, local.interval_days, "{label}: {stored:?}");
        assert_eq!(stored.2, local.due_at, "{label}: {stored:?}");
    }

    // The literals are the point: `replay` is the function the route calls, so a
    // change to the SM-2 rules would leave a `replay`-only comparison green. Two
    // successful reviews give 1 day then 6, and ease moves 2.5 -> 2.5 (`good` is
    // ease-neutral at q=4) -> 2.6 (`easy`); the schedule is dated from the *last*
    // review in `(reviewed_at, id)` order, which is the later one. An
    // implementation that dated it from the arriving review would land ten minutes
    // earlier, on `ten_min_ago + 6 days`.
    let (ease, interval_days, due_at) = persisted;
    assert_eq!(interval_days, 6, "two successful reviews: {interval_days}");
    assert!((ease - 2.6).abs() < 1e-5, "{ease}");
    assert_eq!(
        due_at,
        now + chrono::Duration::days(6),
        "the schedule is dated from the last review in (reviewed_at, id) order"
    );
    // Implied by the `assert_eq!` above, and kept deliberately: it names the
    // incremental answer at the failure point. If the route dated the schedule
    // from the review that happened to arrive last, this is the value it would
    // have produced, and the message says so rather than leaving the reader to
    // work out what `ten_min_ago + 6 days` means.
    assert_ne!(
        due_at,
        ten_min_ago + chrono::Duration::days(6),
        "the schedule must not be dated from the review that happened to arrive last"
    );
}

/// A review is an append-only fact keyed by a client-generated id, so replaying
/// the same id — a retry after a lost response — applies once and leaves the card
/// exactly where the first request left it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_review_id_applies_once(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "retry@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
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
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    let review_id = uuid::Uuid::from_u128(0x5eed);
    let review = serde_json::json!({ "id": review_id.to_string(), "rating": "good" });

    let (status, first) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        review.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["applied"], true, "{first}");
    // From the rules, not from the response: one successful review is the first
    // rung of the ladder. Without this, a route that inserted the review row and
    // then skipped the card update would pass, since the two `due_at`s it compares
    // below would simply be the card's untouched value.
    assert_eq!(
        first["card"]["interval_days"], 1,
        "the first review must move the card: {first}"
    );
    let due_at = first["card"]["due_at"].clone();

    let (status, second) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        review,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["applied"], false,
        "the same id must not be applied twice: {second}"
    );
    assert_eq!(
        second["card"]["due_at"], due_at,
        "a replayed review must not move the card: {second}"
    );

    // One row, and it is the row the client named. The count is of the card's
    // rows, so it is not the query that decides the value asserted.
    let stored: Vec<(uuid::Uuid,)> =
        sqlx::query_as("select id from flashcard_reviews where flashcard_id = $1::uuid")
            .bind(&card_id)
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, vec![(review_id,)], "one review row per id");
}

/// A card reviewed `good` leaves the queue until tomorrow: the queue is a filter
/// on `due_at`, and the deck still lists the card with its new schedule.
#[sqlx::test(migrations = "../../migrations")]
async fn the_due_queue_excludes_a_card_scheduled_for_later(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "due@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
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
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    // A card that has never been reviewed is due at the instant it was created.
    let (status, due) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}/due"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{due}");
    assert_eq!(
        due["cards"].as_array().unwrap().len(),
        1,
        "a fresh card is due: {due}"
    );

    // No `reviewed_at`: the server's clock, so `due_at` is a day after `now`.
    let (status, reviewed) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        serde_json::json!({ "rating": "good" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reviewed}");
    // SM-2's first successful interval, from the spec.
    assert_eq!(reviewed["card"]["interval_days"], 1, "{reviewed}");

    let (status, due) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}/due"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{due}");
    assert!(
        due["cards"].as_array().unwrap().is_empty(),
        "a card scheduled for tomorrow is not due: {due}"
    );

    // And the card is still in the deck, carrying the schedule that keeps it out
    // of the queue rather than having vanished.
    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let card = &deck["cards"][0];
    assert_eq!(card["id"], card_id, "{deck}");
    assert_eq!(card["interval_days"], 1, "{deck}");
    let card_due_at: chrono::DateTime<chrono::Utc> =
        card["due_at"].as_str().unwrap().parse().unwrap();
    let queue_now: chrono::DateTime<chrono::Utc> = due["now"].as_str().unwrap().parse().unwrap();
    assert!(
        card_due_at > queue_now,
        "the queue is empty because the card is not due yet: {card_due_at} vs {queue_now}"
    );
}

/// A card belongs to one account: every route that reaches it checks ownership, so
/// a stranger sees 404 rather than a deck, a queue or a card to review.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_card_is_not_found(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "owner@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
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
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    let stranger = signed_up(&app, &harness, "stranger@example.test").await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &stranger,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}/due"),
        &stranger,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &stranger,
        serde_json::json!({ "rating": "good" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/decks"),
        &stranger,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // The owner still reaches it, so the 404s are about ownership and not about a
    // card or deck id that does not resolve at all.
    let (status, _) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // And the refused review was not stored: the count is of every review row.
    let reviews: i64 = sqlx::query_scalar("select count(*) from flashcard_reviews")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(reviews, 0, "a refused review must not be stored");
}

/// The guard rail inspects the *template*, never the data interpolated into it.
///
/// A workspace named `{{Project}}` and a source containing `{{user_name}}` are both
/// data: the marker-looking text must survive into the prompt verbatim and the deck
/// must generate. While the guard scanned the rendered text, either one made
/// composition return `Invalid`, which `generate` maps to `Failure::permanent` — the
/// deck failed, the credits came back, and that material could never produce a deck.
#[sqlx::test(migrations = "../../migrations")]
async fn a_marker_looking_workspace_and_source_still_generate(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu three-way handshake?",
        "Membuka koneksi TCP.",
        1,
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "braces@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(
        &app,
        &harness,
        &token,
        "{{Project}}",
        "{{cards}} notes",
        "Render {{user_name}} to show the deck. TCP memakai three-way handshake.",
    )
    .await;

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
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
    assert_eq!(
        deck["deck"]["status"], "ready",
        "a marker-looking workspace and source must still generate: {deck}"
    );
    assert!(
        deck["deck"]["card_count"].as_i64().unwrap() >= 1,
        "the deck must carry at least one card: {deck}"
    );

    // The prompt that produced it carried the data verbatim, not an error.
    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    let prompt = &requests[0].messages[0].content;
    assert!(prompt.contains("{{Project}}"), "{prompt}");
    assert!(prompt.contains("{{user_name}}"), "{prompt}");
}

/// The 201 body is the same `DeckJson` every other route serialises, so it carries
/// `created_at` — the SPA's `Deck` declares it, and the hand-built body that
/// omitted it made that a false type assertion.
#[sqlx::test(migrations = "../../migrations")]
async fn the_created_deck_body_carries_every_deck_field(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "created@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 5).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let deck = &body["deck"];
    for field in [
        "id",
        "workspace_id",
        "title",
        "source_ids",
        "status",
        "error",
        "requested_count",
        "card_count",
        "created_at",
    ] {
        assert!(
            deck.get(field).is_some(),
            "the 201 body must carry {field}: {deck}"
        );
    }
    assert!(
        deck["created_at"].is_string(),
        "created_at must be the real timestamp, not null: {deck}"
    );
}

/// Every workspace-scoped query in the generation path is owner-scoped, and the
/// chunk load is one of them. The route validates ownership when it creates the
/// deck, so this constructs the row directly: the defence is against a future
/// caller that skips that check. Without the owner predicate the model would be
/// handed another account's material and the deck would generate from it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_cannot_read_another_accounts_chunks(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Stolen?", "Yes.", 1)]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    // Account A owns the source the deck will name.
    let owner = signed_up(&app, &harness, "chunk-owner@example.test").await;
    let (_, foreign_source) =
        workspace_with_text(&app, &harness, &owner, "Jaringan", "Modul 3", INDONESIAN).await;

    // Account B has their own workspace and a deck that names A's source.
    let stranger = signed_up(&app, &harness, "chunk-stranger@example.test").await;
    let (workspace_b, _) = workspace_with_text(
        &app,
        &harness,
        &stranger,
        "Sendiri",
        "Modul 1",
        "Isi sendiri.",
    )
    .await;

    let (user_b,): (Uuid,) = sqlx::query_as("select id from users where email = $1")
        .bind("chunk-stranger@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let deck_id: Uuid = sqlx::query_scalar(
        "insert into decks (user_id, workspace_id, title, source_ids, status, requested_count)
         values ($1, $2, 'Curi', array[$3::uuid], 'generating', 1)
         returning id",
    )
    .bind(user_b)
    .bind(Uuid::parse_str(&workspace_b).unwrap())
    .bind(Uuid::parse_str(&foreign_source).unwrap())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into jobs (kind, payload)
         values ('generate_flashcards', jsonb_build_object('deck_id', $1::text))",
    )
    .bind(deck_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    drain_jobs(&harness).await;

    assert!(
        llm.requests().is_empty(),
        "the model must not be called for a deck whose only source belongs to another account"
    );
    let cards: i64 =
        sqlx::query_scalar("select count(*)::bigint from flashcards where deck_id = $1")
            .bind(deck_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(cards, 0, "a foreign source's text must not become a deck");

    let status: String = sqlx::query_scalar("select status from decks where id = $1")
        .bind(deck_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        status, "failed",
        "the deck must fail rather than stay generating"
    );
}

/// A failed refund must leave a terminal deck, and the boot sweep must recover it.
///
/// CONTRACT CHANGE. This test used to pin the opposite: that a failed refund insert
/// rolled the whole claim back, leaving the deck `generating` with its charge
/// intact "so a later call could still refund". Nothing ever made that later call —
/// `fail_and_refund` only runs on the final attempt, no retry is scheduled after it,
/// and the SPA polls a `generating` deck every 1.5 s without end — so the rollback
/// traded a visible failure for a silent, permanent one. The contract is now: the
/// status change commits even when the refund cannot be written, `credits_spent`
/// stays charged (the pending-refund marker), and `credits::refund_pending`
/// recovers it at the next boot, exactly once.
///
/// The ledger insert is forced to fail with a `user_id` that violates
/// `credit_ledger`'s foreign key: the failure is injected through the refund's own
/// insert, which is the real infrastructure failure the recovery exists for.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_refund_leaves_a_terminal_deck_and_the_boot_sweep_recovers_it(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "atomic@example.test").await;
    let (workspace_id, _source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (user_id,): (Uuid,) = sqlx::query_as("select id from users where email = $1")
        .bind("atomic@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let deck_id: Uuid = sqlx::query_scalar(
        "insert into decks
            (user_id, workspace_id, title, source_ids, status, requested_count, credits_spent)
         values ($1, $2, 'Jaringan', '{}'::uuid[], 'generating', 10, 4)
         returning id",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(&workspace_id).unwrap())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // A user id that does not exist violates the ledger's foreign key, so the
    // refund insert fails.
    stvdy_server::jobs::handlers::generate::fail_and_refund(
        &harness.state,
        deck_id,
        Uuid::from_u128(0xdead),
        4,
        "boom",
    )
    .await;

    let (status, credits_spent): (String, i32) =
        sqlx::query_as("select status, credits_spent from decks where id = $1")
            .bind(deck_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        status, "failed",
        "a failed refund must still leave the deck terminal, never generating"
    );
    assert_eq!(
        credits_spent, 4,
        "the charge stays pending so the boot sweep can recover it"
    );

    let refunds_before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where reason = 'refund'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(refunds_before, 0, "the failed insert wrote no refund row");

    // The next boot recovers the pending charge.
    let recovered = stvdy_server::credits::refund_pending(
        &harness.state.pool,
        stvdy_server::credits::RefundTarget::Deck,
    )
    .await
    .unwrap();
    assert_eq!(
        recovered, 1,
        "the sweep must recover exactly the pending charge"
    );

    let (status, credits_spent): (String, i32) =
        sqlx::query_as("select status, credits_spent from decks where id = $1")
            .bind(deck_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed", "recovery must not resurrect the deck");
    assert_eq!(credits_spent, 0, "the sweep zeroes the charge it refunds");

    let refunded: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where reason = 'refund'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(refunded, 4, "the user gets back exactly what was charged");

    // Idempotent: a second boot must not refund the same deck twice.
    let recovered_again = stvdy_server::credits::refund_pending(
        &harness.state.pool,
        stvdy_server::credits::RefundTarget::Deck,
    )
    .await
    .unwrap();
    assert_eq!(
        recovered_again, 0,
        "a recovered charge is not refunded twice"
    );
    let refunded_again: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where reason = 'refund'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(refunded_again, 4, "the ledger moved exactly once");
}

/// The studio panel's cost copy and its `max` come from the plan matrix, not from
/// a literal in the SPA. Serving them with the decks the panel already fetches
/// keeps the two in step: a config change cannot leave the UI quoting a stale
/// price or a stale request cap.
#[sqlx::test(migrations = "../../migrations")]
async fn the_deck_cost_descriptor_matches_the_config(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cost@example.test").await;
    let (workspace_id, _source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let cost = &body["cost"];
    assert_eq!(
        cost["per_batch"].as_i64().unwrap(),
        harness.state.config.plans.credits.flashcard_batch_10,
        "the quoted batch price must be the plan matrix's"
    );
    assert_eq!(
        cost["batch_size"].as_i64().unwrap(),
        stvdy_server::flashcards::routes::CARDS_PER_BATCH
    );
    assert_eq!(
        cost["max_cards"].as_u64().unwrap(),
        u64::from(stvdy_server::flashcards::routes::MAX_REQUESTED_CARDS)
    );
    assert!(
        body["decks"].is_array(),
        "the existing shape stays valid for a client that ignores cost: {body}"
    );
}

/// The monthly cap counts the cards a deck has *requested*, not only the ones that
/// have already been written.
///
/// Free allows 30 a month. The deck's cards do not exist yet at request time: the
/// job writes them later. Counting only `flashcards` rows therefore reads zero while
/// a deck is still generating, so a second request sees a full allowance and the cap
/// can be exceeded simply by asking twice in a row. The whole point of
/// `enforce_batch` is to refuse a batch that does not fit, which it cannot do if the
/// first batch is invisible to it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_still_generating_counts_against_the_monthly_limit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reserve@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    // The whole allowance in one deck. The job is deliberately not drained, so no
    // card row exists yet and the request is still in flight.
    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 30).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // One more card does not fit, whether or not the first deck has finished.
    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a deck still generating must count against the month: {body}"
    );
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 30, "{body}");
}

/// A deck built from several sources must draw on **all** of them.
///
/// `load_chunks` used to take its bounded window in `(source_id, idx)` order, which
/// spends the whole budget on whichever source sorts first. A deck from a 500-chunk
/// textbook and a one-page summary therefore carried the textbook alone: the student
/// selected two sources and silently got cards from one. The chunks are now taken
/// round-robin, so the second chunk in the prompt comes from a different source than
/// the first.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_draws_on_every_selected_source(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Q?", "A.", 1)]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "spread@example.test").await;

    let (status, workspace) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Two sources" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workspace}");
    let workspace_id = workspace["workspace"]["id"].as_str().unwrap().to_string();
    let user_id: Uuid =
        sqlx::query_scalar("select id from users where email = 'spread@example.test'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    // The sources and their chunks are inserted directly: ingestion is Phase 1's
    // subject, and this test needs a source with more chunks than another, which a
    // pasted paragraph cannot produce.
    let mut source_ids = Vec::new();
    for (title, texts) in [
        (
            "Big textbook",
            vec!["ALPHA one", "ALPHA two", "ALPHA three"],
        ),
        ("One-page summary", vec!["BETA only"]),
    ] {
        let source_id: Uuid = sqlx::query_scalar(
            "insert into sources (user_id, workspace_id, kind, title, status)
             values ($1, $2, 'text', $3, 'ready') returning id",
        )
        .bind(user_id)
        .bind(Uuid::parse_str(&workspace_id).unwrap())
        .bind(title)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        for (index, text) in texts.iter().enumerate() {
            sqlx::query(
                "insert into chunks (source_id, page, idx, content, content_hash, embedder)
                 values ($1, null, $2, $3, $4, 'test')",
            )
            .bind(source_id)
            .bind(index as i32)
            .bind(text)
            .bind(format!("{source_id}-{index}"))
            .execute(&harness.state.pool)
            .await
            .unwrap();
        }
        source_ids.push(source_id);
    }

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::json!({
            "title": "Both",
            "source_ids": source_ids,
            "count": 1,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    drain_jobs(&harness).await;

    // The prompt is where the spread is observable: the first two entries of the
    // `<sources>` block must come from different sources. Under the old ordering the
    // second was "ALPHA two".
    let requests = llm.requests();
    assert_eq!(requests.len(), 1, "one generation, one call");
    let prompt = &requests[0].messages[0].content;
    let first = prompt
        .find("ALPHA one")
        .expect("the first source's first chunk");
    let second = prompt
        .find("ALPHA two")
        .expect("the first source's second chunk");
    let other = prompt.find("BETA only").expect("the second source's chunk");
    assert!(
        other < second,
        "the second source must contribute before the first source's second chunk: \
         alpha1={first} beta={other} alpha2={second}"
    );
}

/// The same source twice is one source, not an invalid request.
///
/// The readiness check compares `count(*)` (rows) with `source_ids.len()` (the
/// request), so a repeat made them disagree and the request was refused with "every
/// source must be one of this workspace's" -- which was false: the source was fine,
/// the array had a duplicate.
#[sqlx::test(migrations = "../../migrations")]
async fn a_repeated_source_id_is_deduplicated_rather_than_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Q?", "A.", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "dup@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::json!({
            "title": "Repeated",
            "source_ids": [source_id, source_id],
            "count": 1,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a duplicate id is not an invalid source: {body}"
    );
    // And the stored deck does not carry the repeat into every later read.
    assert_eq!(
        body["deck"]["source_ids"].as_array().unwrap().len(),
        1,
        "{body}"
    );
}

/// Undo: a review deleted inside the window, with the card replayed.
///
/// **The assertion that matters is the equality at the end.** Undo is not "put the
/// numbers back" — it is a delete plus a replay of the *remaining* history, so the
/// state after an undo must equal the state the card would have had if the review
/// had never been written. That is the property a second implementation (an inverse
/// of SM-2) could not preserve, which is why this test compares against a card that
/// took the other path rather than against a remembered pair of numbers.
///
/// The window is checked from both sides: a review older than five minutes is a 409
/// (the review exists; it is too late), and a second undo of the same review is a
/// 404, so a retried tap is not an error.
#[sqlx::test(migrations = "../../migrations")]
async fn a_review_can_be_undone_inside_the_window_and_not_after(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // Two decks over one source, each with one card, so two cards start identical.
    let llm = Arc::new(ScriptedLlm::new(vec![
        reply("Apa fungsi three-way handshake?", "Membuka koneksi TCP.", 1),
        reply("Apa fungsi three-way handshake?", "Membuka koneksi TCP.", 1),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "undo@example.test").await;

    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, first) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    let (_, second) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    drain_jobs(&harness).await;

    let deck_of = |body: &serde_json::Value| body["deck"]["id"].as_str().unwrap().to_string();
    let deck_a = deck_of(&first);
    let deck_b = deck_of(&second);

    // The card's state as the API reports it: the three fields `replay` writes.
    async fn state_of(
        app: &axum::Router,
        token: &str,
        deck: &str,
    ) -> (serde_json::Value, serde_json::Value, serde_json::Value) {
        let (_, body) = json(
            app,
            "GET",
            &format!("/decks/{deck}"),
            token,
            serde_json::Value::Null,
        )
        .await;
        (
            body["cards"][0]["ease"].clone(),
            body["cards"][0]["interval_days"].clone(),
            body["cards"][0]["due_at"].clone(),
        )
    }

    let card_a = {
        let (_, body) = json(
            &app,
            "GET",
            &format!("/decks/{deck_a}"),
            &token,
            serde_json::Value::Null,
        )
        .await;
        body["cards"][0]["id"].as_str().unwrap().to_string()
    };
    let card_b = {
        let (_, body) = json(
            &app,
            "GET",
            &format!("/decks/{deck_b}"),
            &token,
            serde_json::Value::Null,
        )
        .await;
        body["cards"][0]["id"].as_str().unwrap().to_string()
    };

    let now = chrono::DateTime::from_timestamp(harness.clock.now().timestamp(), 0).unwrap();

    // The card's state *before* any review, which is what an undo has to restore —
    // "the state it would have had if the review had never been written". Captured
    // rather than derived, because deriving it would need a second implementation of
    // SM-2's initial state, which is the thing this route exists to avoid.
    let before_review = state_of(&app, &token, &deck_a).await;

    // The same rating at the same instant on both cards, so they start identical.
    for card in [&card_a, &card_b] {
        let (status, body) = json(
            &app,
            "POST",
            &format!("/flashcards/{card}/review"),
            &token,
            serde_json::json!({ "rating": "easy", "reviewed_at": now.to_rfc3339() }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let review_id: Uuid =
        sqlx::query_scalar("select id from flashcard_reviews where flashcard_id = $1")
            .bind(Uuid::parse_str(&card_a).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    // Older than the window: a 409 that says why, not a 404 pretending it is gone.
    sqlx::query("update flashcard_reviews set reviewed_at = $2 where id = $1")
        .bind(review_id)
        .bind(now - chrono::Duration::minutes(6))
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (status, late) = json(
        &app,
        "DELETE",
        &format!("/flashcard-reviews/{review_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{late}");

    // Inside the window it succeeds, and the card lands in exactly the state of the
    // card whose review was never written — which is what replaying buys over
    // inverting the algorithm.
    sqlx::query("update flashcard_reviews set reviewed_at = $2 where id = $1")
        .bind(review_id)
        .bind(now - chrono::Duration::minutes(1))
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (status, undone) = json(
        &app,
        "DELETE",
        &format!("/flashcard-reviews/{review_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    assert_eq!(undone["undone"], true, "{undone}");

    assert_eq!(
        state_of(&app, &token, &deck_a).await,
        before_review,
        "after an undo the card must be in the state it would have had if the review had never been written"
    );
    // And the card that was *not* undone still holds its review, which is what makes
    // the assertion above about the undo rather than about reviews never landing.
    assert_ne!(
        state_of(&app, &token, &deck_b).await,
        before_review,
        "a review that was not undone must still be applied"
    );

    // A second undo is a 404: the review is gone, so a retried tap is not an error.
    let (status, again) = json(
        &app,
        "DELETE",
        &format!("/flashcard-reviews/{review_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{again}");

    // And another account cannot reach it at all.
    let other = signed_up(&app, &harness, "other@example.test").await;
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/flashcard-reviews/{review_id}"),
        &other,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A deleted deck is gone from the list, and its cards are tombstoned with it.
///
/// The test creates a deck, generates its cards, deletes it, and then checks:
///
///  * The DELETE returns 204 (no content).
///  * The deck no longer appears in `GET /workspaces/:id/decks`.
///  * `GET /decks/:id` returns 404 — `owned_deck` excludes tombstones.
///  * A second DELETE is also 404 (idempotent from the caller's side).
///  * A stranger cannot delete it (404, same as any scoped read).
#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_deck_is_gone_from_the_list_and_its_cards_are_tombstoned(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa itu TCP?",
        "Protokol.",
        1,
    )]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "deleter@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    // Sanity: the deck is listed and has one card.
    let (status, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["decks"].as_array().unwrap().len(), 1);
    assert_eq!(list["decks"][0]["card_count"], 1);

    // A stranger cannot delete it.
    let stranger = signed_up(&app, &harness, "stranger@example.test").await;
    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/decks/{deck_id}"),
        &stranger,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Delete it.
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The deck is gone from the list.
    let (status, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["decks"].as_array().unwrap().len(), 0);

    // GET the deck itself returns 404.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A second delete is 404 (idempotent).
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The cards are tombstoned in the database (not visible, but rows remain).
    let live_cards: i64 = sqlx::query_scalar(
        "select count(*) from flashcards where deck_id = $1::uuid and deleted_at is null",
    )
    .bind(&deck_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_cards, 0, "cards must be tombstoned with the deck");

    let all_cards: i64 =
        sqlx::query_scalar("select count(*) from flashcards where deck_id = $1::uuid")
            .bind(&deck_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        all_cards, 1,
        "card rows must survive as tombstones, not be hard-deleted"
    );
}

/// Deleting a deck that is still generating does not refund the credits.
///
/// The credits were spent when the generation was requested. Deleting is the
/// student's choice; it does not undo the cost. The test creates a deck
/// (status `generating`, `credits_spent > 0`), deletes it without draining
/// the job, and asserts:
///
///  * The ledger balance is unchanged (no refund row was written).
///  * `credits_spent` on the tombstoned row is 0 — so `fail_and_refund`, which
///    claims on `credits_spent > 0` without checking `deleted_at`, cannot
///    refund a deck the student deliberately deleted.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_generating_deck_does_not_refund_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // A scripted reply that will never be consumed: the job is never drained.
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Q", "A", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nodelete-refund@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    // Snapshot the ledger balance before the deck is created.
    let before_create: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'nodelete-refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["deck"]["status"], "generating");

    // The deck was charged. Snapshot the balance after creation.
    let after_create: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'nodelete-refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert!(
        after_create < before_create,
        "creating the deck must spend credits: before={before_create}, after={after_create}"
    );

    // The deck is still generating (job not drained). Delete it.
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The ledger balance must be unchanged from after_create: no refund row.
    let after_delete: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'nodelete-refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        after_delete, after_create,
        "deleting a generating deck must not refund credits"
    );

    // credits_spent must be zeroed on the tombstoned row, so fail_and_refund
    // cannot refund it later.
    let credits_spent: i32 =
        sqlx::query_scalar("select credits_spent from decks where id = $1::uuid")
            .bind(&deck_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        credits_spent, 0,
        "credits_spent must be zeroed so fail_and_refund cannot refund a deleted deck"
    );
}
