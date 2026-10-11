//! Phase 3's exit gate, end to end through the chat route.
//!
//! The roadmap's acceptance is "cited results with URLs **and DOIs**" and "a
//! repeated query bills once". Both are asserted here against the real route and
//! the real ledger, with a stub tier standing in for the network: the tiers
//! themselves are covered by `tests/search.rs`, and what this file checks is that
//! their results reach the user and the ledger intact.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::http::StatusCode;
use chrono::Utc;
use sqlx::PgPool;
use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::integrations::search::composite::CompositeSearch;
use stvdy_server::testing::TestState;

mod common;
use common::{SseEvent, json, signed_up, sse_events, workspace_with_text};

/// A tier that returns one academic hit with a DOI, and counts its calls so the
/// cache can be observed.
struct StubTier {
    calls: Arc<AtomicUsize>,
    doi: Option<String>,
}

#[async_trait::async_trait]
impl SearchProvider for StubTier {
    fn name(&self) -> &'static str {
        "stub_academic"
    }

    async fn search(&self, _q: &str, _max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);

        let mut hit = SearchHit::new(
            "Attention Is All You Need",
            "https://arxiv.org/abs/1706.03762",
            "The dominant sequence transduction models are based on complex recurrent networks.",
            SourceKind::Academic,
            "stub_academic",
        );
        hit.doi = self.doi.clone();
        hit.published_at = Some(Utc::now());
        Ok(vec![hit])
    }
}

/// Places the stub composite in the state, so a web turn has something to search.
fn with_stub(harness: TestState, doi: Option<&str>) -> (TestState, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let tier = Arc::new(StubTier {
        calls: calls.clone(),
        doi: doi.map(str::to_string),
    });
    let composite = CompositeSearch::with_tiers(
        vec![tier],
        harness.state.pool.clone(),
        std::time::Duration::from_secs(3600),
    );
    (harness.with_search(Arc::new(composite)), calls)
}

const NOTES: &str = "Photosynthesis converts light into chemical energy in the chloroplast.";

/// The gate: a web turn cites a URL **and** a DOI, and the citation arrives as the
/// structured web form rather than as a chunk citation.
#[sqlx::test(migrations = "../../migrations")]
async fn a_web_turn_cites_a_url_and_a_doi(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cite@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let (harness, calls) = with_stub(harness, Some("10.48550/arXiv.1706.03762"));
    // The answer cites the web result the stub returned.
    let llm = Arc::new(ScriptedLlm::new(vec![
        "Transformers replaced recurrence [W1].".to_string(),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "what replaced recurrence?", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the tier was consulted once"
    );

    let citations = events
        .iter()
        .find(|event| event.kind() == "citations")
        .map(|event| event.json()["citations"].clone())
        .expect("a citations event");
    let array = citations.as_array().expect("an array");
    assert_eq!(array.len(), 1, "one web citation: {citations}");

    // The gate is URLs *and* DOIs, so both are asserted by name.
    assert_eq!(array[0]["type"], "web");
    assert_eq!(array[0]["url"], "https://arxiv.org/abs/1706.03762");
    assert_eq!(array[0]["title"], "Attention Is All You Need");
    assert_eq!(
        array[0]["doi"], "10.48550/arXiv.1706.03762",
        "the DOI must survive from the tier to the citation: {citations}"
    );

    // The prompt carried the web block, so the model was actually given the result.
    let requests = llm.requests();
    let system = &requests[0].messages[0].content;
    assert!(
        system.contains("[W1]"),
        "the web result must be labelled in the prompt: {system}"
    );
    assert!(
        system.contains("10.48550/arXiv.1706.03762"),
        "the DOI must reach the model: {system}"
    );

    // The stored turn agrees, and says it used the web.
    let (_, thread) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let assistant = &thread["messages"][1];
    assert_eq!(assistant["used_web"], true);
    assert_eq!(assistant["citations"][0]["type"], "web");
    assert_eq!(
        assistant["citations"][0]["doi"],
        "10.48550/arXiv.1706.03762"
    );

    let done_used_web = events
        .iter()
        .find(|event| event.kind() == "done")
        .map(|event| event.json()["used_web"].clone())
        .expect("a done event");
    assert_eq!(done_used_web, true);
}

/// The second half of the gate: the same query twice bills one search.
#[sqlx::test(migrations = "../../migrations")]
async fn a_repeated_query_bills_one_search_and_reports_a_cache_hit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cache@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let (harness, calls) = with_stub(harness, Some("10.48550/arXiv.1706.03762"));
    let llm = Arc::new(ScriptedLlm::new(vec![
        "Transformers replaced recurrence [W1].".to_string(),
        "Transformers replaced recurrence [W1].".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let chat_cost = harness.state.config.plans.credits.chat_message;
    let search_cost = harness.state.config.plans.credits.web_search;
    let allowance = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .credits_per_month;

    let body = serde_json::json!({ "message": "what replaced recurrence?", "use_web": true });
    let uri = format!("/workspaces/{workspace_id}/chat");

    let (first_status, first) = sse_events(&app, &uri, &token, body.clone()).await;
    assert_eq!(first_status, StatusCode::OK, "{first:?}");

    let (second_status, second) = sse_events(&app, &uri, &token, body).await;
    assert_eq!(second_status, StatusCode::OK, "{second:?}");

    // One provider call for two identical queries. This is the A40 acceptance
    // criterion, and the assertion is on the *provider*, not on our own counter.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the second identical query must be served from the cache"
    );

    let spent = |events: &[SseEvent]| -> i64 {
        events
            .iter()
            .find(|event| event.kind() == "usage")
            .map(|event| event.json()["credits_spent"].as_i64().unwrap())
            .expect("a usage event")
    };
    assert_eq!(
        spent(&first),
        chat_cost + search_cost,
        "the first turn pays for the chat and the search"
    );
    assert_eq!(
        spent(&second),
        chat_cost,
        "the cached turn pays for the chat only"
    );

    let ledger: i64 =
        sqlx::query_scalar("select coalesce(sum(delta), 0)::bigint from credit_ledger")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        ledger,
        allowance - (chat_cost * 2) - search_cost,
        "the ledger must agree with what the two turns reported"
    );

    // Both searches wrote a row, and the second says it was a cache hit: without
    // that column a cache hit and a never-run search are indistinguishable.
    let rows: Vec<bool> = sqlx::query_scalar(
        "select cache_hit from usage_events where action = 'web_search' order by created_at",
    )
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![false, true], "the second search is a cache hit");

    // The tier that answered is recorded, so the admin report can see which path
    // is actually serving.
    let tier: Option<String> = sqlx::query_scalar(
        "select search_tier from usage_events where action = 'web_search' order by created_at limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(tier.as_deref(), Some("stub_academic"));
}

/// A tier with no DOI must not invent one.
#[sqlx::test(migrations = "../../migrations")]
async fn a_web_hit_without_a_doi_cites_without_one(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nodoi@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let (harness, _) = with_stub(harness, None);
    let llm = Arc::new(ScriptedLlm::new(vec!["A web claim [W1].".to_string()]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "anything", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let citations = events
        .iter()
        .find(|event| event.kind() == "citations")
        .map(|event| event.json()["citations"].clone())
        .expect("a citations event");
    let array = citations.as_array().unwrap();
    assert_eq!(array.len(), 1);
    assert_eq!(array[0]["url"], "https://arxiv.org/abs/1706.03762");
    assert!(
        array[0]["doi"].is_null(),
        "no DOI was provided, so none may appear: {citations}"
    );
}

/// `use_web: false` must not search at all, which is also what keeps the toggle
/// honest.
#[sqlx::test(migrations = "../../migrations")]
async fn a_turn_without_the_toggle_does_not_search(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "off@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let (harness, calls) = with_stub(harness, Some("10.48550/arXiv.1706.03762"));
    let llm = Arc::new(ScriptedLlm::new(vec!["Grounded [1].".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "anything" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    assert_eq!(calls.load(Ordering::SeqCst), 0, "no search may run");
    // And the prompt tells the model web search is off, so it has no reason to
    // claim otherwise.
    let system = &llm.requests()[0].messages[0].content;
    assert!(system.contains("Web search: off"), "{system}");
}
