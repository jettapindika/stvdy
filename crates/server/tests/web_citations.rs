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
use stvdy_server::integrations::search::searxng::SearxngSearch;
use stvdy_server::testing::TestState;

mod common;
use common::{SseEvent, json, signed_up, sse_events, workspace_with_text};

/// A tier that returns one academic hit with a DOI, and counts its calls so the
/// cache can be observed.
///
/// Its hit URL varies with the query, so two *different* queries are two cache
/// misses and each consumes the daily search allowance.
struct StubTier {
    calls: Arc<AtomicUsize>,
    doi: Option<String>,
}

#[async_trait::async_trait]
impl SearchProvider for StubTier {
    fn name(&self) -> &'static str {
        "stub_academic"
    }

    async fn search(&self, q: &str, _max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);

        let mut hit = SearchHit::new(
            "Attention Is All You Need",
            // Per-query, so a different query is a different cache key and therefore
            // a real search.
            format!("https://arxiv.org/abs/1706.03762?q={}", q.len()),
            "The dominant sequence transduction models are based on complex recurrent networks.",
            SourceKind::Academic,
            "stub_academic",
        );
        let _ = q;
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

    // The gate is URLs *and* DOIs, so both are asserted. The stub appends the query
    // length to its URL so distinct queries are distinct cache keys, so the
    // assertion is on the address rather than the whole string.
    assert_eq!(array[0]["type"], "web");
    let url = array[0]["url"].as_str().unwrap();
    assert!(
        url.starts_with("https://arxiv.org/abs/1706.03762"),
        "the citation must carry the hit's URL: {url}"
    );
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

/// The A3 health signal on the row is SearXNG's **engine** list, not the list of
/// tiers that failed to answer.
///
/// The two are one word apart in the struct and were swapped at the call site: the
/// column took `WebContext::unresponsive` (tiers) and the engine list was dropped,
/// so a tier answering with a dead engine looked perfectly healthy in the margin
/// report. The composite here is a failing tier *followed by* a stub SearXNG that
/// reports one dead engine, so the two lists differ and only one of them can be
/// what lands in the row.
#[sqlx::test(migrations = "../../migrations")]
async fn the_search_row_carries_the_engines_searxng_reported(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "engines@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let body = serde_json::json!({
        "query": "anything",
        "results": [{
            "url": "https://example.org/a",
            "title": "A result",
            "content": "A snippet.",
            "publishedDate": null,
            "score": 0.7,
            "engine": "google cse"
        }],
        "unresponsive_engines": [["wikipedia", "Suspended: too many requests"]]
    })
    .to_string();
    let server = common::stub_http_json(200, body).await;
    let searxng = Arc::new(SearxngSearch::new(server.base_url()).unwrap());

    // The failing tier is first, so `unresponsive` (tiers) is `["broken"]` while the
    // engine list is the stub's.
    let composite = CompositeSearch::with_tiers(
        vec![Arc::new(FailingTier), searxng.clone()],
        harness.state.pool.clone(),
        std::time::Duration::from_secs(3600),
    )
    .with_searxng(searxng);

    let llm = Arc::new(ScriptedLlm::new(vec!["A web claim [W1].".to_string()]));
    let harness = harness.with_llm(llm).with_search(Arc::new(composite));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "anything", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let (tier, engines): (Option<String>, Vec<String>) = sqlx::query_as(
        "select search_tier, unresponsive_engines from usage_events
          where action = 'web_search' order by created_at limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    assert_eq!(tier.as_deref(), Some("searxng"));
    assert_eq!(
        engines,
        vec!["wikipedia: Suspended: too many requests".to_string()],
        "the row must carry SearXNG's engine list, not the tier that failed"
    );
}

/// A tier that never answers, so the composite's tier list is non-empty.
struct FailingTier;

#[async_trait::async_trait]
impl SearchProvider for FailingTier {
    fn name(&self) -> &'static str {
        "broken"
    }

    async fn search(&self, _q: &str, _max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        Err(SearchError::Unavailable(
            "the stub tier is down".to_string(),
        ))
    }
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
    assert!(
        array[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://arxiv.org/abs/1706.03762")
    );
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

/// `web_searches_per_day` is enforced, and a cache hit does not consume it.
///
/// Two things this pins that are easy to get wrong. The cap has to exist at all:
/// it is configured in `plans.toml` (3 free, 30 pro, 100 power) and a limit nothing
/// reads is a limit that does not exist, which is exactly what happened to
/// `sources_total` in Phase 1. And it must count **uncached** searches only: a
/// repeat of a question already answered costs no credit, so charging the daily
/// allowance for it would make the free tier's three searches unusable for a
/// student who revises, which is the opposite of what the cache is for (A40).
#[sqlx::test(migrations = "../../migrations")]
async fn the_daily_search_cap_counts_uncached_searches_only(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "search-cap@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let (harness, calls) = with_stub(harness, Some("10.48550/arXiv.1706.03762"));
    // Enough credits that the cap is what refuses, not the balance.
    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason) values ($1, 500, 'monthly', 'test')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let llm = Arc::new(ScriptedLlm::new(vec!["An answer [W1].".to_string(); 20]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let cap = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .web_searches_per_day;
    let uri = format!("/workspaces/{workspace_id}/chat");

    // Distinct queries, so each is a miss and each consumes one of the allowance.
    for index in 0..cap {
        let (status, events) = sse_events(
            &app,
            &uri,
            &token,
            serde_json::json!({ "message": format!("distinct question {index}"), "use_web": true }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "search {index} of {cap}: {events:?}"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), cap as usize);

    // The next distinct query is refused, and the refusal names the plan.
    let (status, rejected) = json(
        &app,
        "POST",
        &uri,
        &token,
        serde_json::json!({ "message": "one more distinct question", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{rejected}");
    assert_eq!(rejected["code"], "limit_reached");
    assert_eq!(rejected["min_plan"], "pro");
    assert_eq!(rejected["limit"], cap as i64);

    // A repeat of an already-cached query still works: it costs nothing and must not
    // consume the allowance it already spent.
    let before = calls.load(Ordering::SeqCst);
    let (status, events) = sse_events(
        &app,
        &uri,
        &token,
        serde_json::json!({ "message": "distinct question 0", "use_web": true }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a cached repeat must still answer after the cap is reached: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "a cached repeat must not call the provider again"
    );

    // And the uncached count is exactly the cap, not cap + 1: the repeat did not add
    // to it.
    let uncached: i64 = sqlx::query_scalar(
        "select count(*)::bigint from usage_events
          where action = 'web_search' and cache_hit = false",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(uncached, cap as i64);
}

/// A search that is refused by the cap spends no credit for it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_search_refused_by_the_cap_does_not_charge_for_it(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cap-charge@example.test").await;
    let (workspace_id, _) =
        workspace_with_text(&app, &harness, &token, "Notes", "Notes", NOTES).await;

    let (harness, _) = with_stub(harness, None);
    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    // A cap already reached, simulated with usage rows, and a healthy balance.
    let cap = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .web_searches_per_day;
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason) values ($1, 500, 'monthly', 'test')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    for _ in 0..cap {
        sqlx::query(
            "insert into usage_events (user_id, action, credits, cache_hit)
             values ($1, 'web_search', 2, false)",
        )
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
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
        serde_json::json!({ "message": "a distinct question", "use_web": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{rejected}");

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "a refused turn must not charge, because the whole spend is one transaction"
    );
}
