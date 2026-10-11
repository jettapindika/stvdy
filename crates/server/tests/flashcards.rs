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

/// The refund and the claim that zeroes `credits_spent` are one transaction, so a
/// failure between them cannot charge a user for a deck they never received with
/// nothing left for a retry to fix.
///
/// The ledger insert is forced to fail with a `user_id` that violates
/// `credit_ledger`'s foreign key; the point under test is the rollback, not the
/// trigger. Without the atomic fix the claim has already committed and
/// `credits_spent` is 0, which is exactly the unrecoverable state.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_refund_rolls_the_claim_back(pool: PgPool) {
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
        credits_spent, 4,
        "a failed refund must leave the charge refundable, not zeroed"
    );
    assert_eq!(
        status, "generating",
        "the claim must roll back with the refund it could not complete"
    );

    let refunds: i64 =
        sqlx::query_scalar("select count(*)::bigint from credit_ledger where reason = 'refund'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(refunds, 0, "no refund row was written");
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
