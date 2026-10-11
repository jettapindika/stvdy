//! Phase 3 Task 3: the tiers and the composite.
//!
//! The real tiers are all unreachable from this machine (no SearXNG container and
//! no paid keys), so the walk is tested against doubles that implement the same
//! frozen `SearchProvider` trait, and the one tier whose wire format can be
//! reproduced locally (SearXNG's JSON) is tested against a stub HTTP server.
//!
//! Test code may unwrap and panic; `#[sqlx::test]` is a macro harness, so the
//! relaxations in clippy.toml are not guaranteed to apply.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use secrecy::SecretString;
use sqlx::PgPool;
use sqlx::types::Json;
use stvdy_core::search::normalize::dedupe_key;
use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};
use stvdy_server::config::{SearchConfig, SearchTier};
use stvdy_server::integrations::search::CompositeSearch;
use stvdy_server::integrations::search::searxng::SearxngSearch;
use stvdy_server::testing::TestState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A tier that returns scripted hits and counts how many times it was asked. The
/// count is the assertion in most of these tests: it is what distinguishes "the
/// walk stopped" and "the cache answered" from "it happened to work".
struct StubTier {
    name: &'static str,
    hits: Vec<SearchHit>,
    calls: AtomicUsize,
}

impl StubTier {
    fn new(name: &'static str, hits: Vec<SearchHit>) -> Self {
        Self {
            name,
            hits,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SearchProvider for StubTier {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn search(&self, _q: &str, _max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.hits.clone())
    }
}

/// A tier that fails the way a stopped SearXNG or a rate-limited API does.
struct FailingTier {
    name: &'static str,
    calls: AtomicUsize,
}

impl FailingTier {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SearchProvider for FailingTier {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn search(&self, _q: &str, _max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(SearchError::Unavailable(format!("{} is down", self.name)))
    }
}

fn hit(title: &str, url: &str, source_kind: SourceKind, provider: &str) -> SearchHit {
    SearchHit::new(
        title,
        url,
        format!("snippet for {title}"),
        source_kind,
        provider,
    )
}

fn composite(pool: &PgPool, tiers: Vec<Arc<dyn SearchProvider>>) -> CompositeSearch {
    CompositeSearch::with_tiers(tiers, pool.clone(), Duration::from_secs(3600))
}

fn search_config(
    tier: SearchTier,
    searxng_url: Option<&str>,
    exa_key: bool,
    tavily_key: bool,
) -> SearchConfig {
    SearchConfig {
        tier,
        searxng_url: searxng_url.map(str::to_string),
        exa_api_key: exa_key.then(|| SecretString::from("test-exa-key".to_string())),
        tavily_api_key: tavily_key.then(|| SecretString::from("test-tavily-key".to_string())),
        cache_ttl: Duration::from_secs(3600),
        max_results: 8,
    }
}

/// The row count of the two tables the cache tests assert on. Literal SQL because
/// sqlx 0.9 refuses a runtime-built string, and a table name cannot be bound.
async fn cache_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from search_cache")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn user_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from users")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_user(pool: &PgPool, email: &str) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query("insert into users (id, email, password_hash) values ($1, $2, 'x')")
        .bind(id)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// A one-endpoint HTTP server speaking just enough of the protocol for a tier to
/// read one JSON response, so the SearXNG mapping can be exercised against the
/// wire without the real service.
async fn serve_json(body: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = [0u8; 2048];
            let _ = socket.read(&mut buffer).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (format!("http://{address}"), handle)
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_tier_with_results_stops_the_walk(pool: PgPool) {
    let harness = TestState::new(pool);
    let first = Arc::new(StubTier::new(
        "first",
        vec![hit(
            "First",
            "https://example.org/1",
            SourceKind::Academic,
            "first",
        )],
    ));
    let second = Arc::new(StubTier::new(
        "second",
        vec![hit(
            "Second",
            "https://example.org/2",
            SourceKind::Web,
            "second",
        )],
    ));
    let search = composite(&harness.state.pool, vec![first.clone(), second.clone()]);

    let outcome = search.search("a query", 8).await;

    assert_eq!(first.calls(), 1);
    assert_eq!(
        second.calls(),
        0,
        "the second tier must never be called once the first answers"
    );
    assert_eq!(outcome.answered_by.as_deref(), Some("first"));
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].title, "First");
    assert!(!outcome.cache_hit);
    assert!(outcome.unresponsive.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_erroring_tier_is_recorded_and_the_next_one_is_tried(pool: PgPool) {
    let harness = TestState::new(pool);
    let broken = Arc::new(FailingTier::new("broken"));
    let good = Arc::new(StubTier::new(
        "good",
        vec![hit(
            "Good",
            "https://example.org/g",
            SourceKind::Web,
            "good",
        )],
    ));
    let search = composite(&harness.state.pool, vec![broken.clone(), good.clone()]);

    let outcome = search.search("a query", 8).await;

    assert_eq!(broken.calls(), 1);
    assert_eq!(good.calls(), 1);
    assert_eq!(outcome.unresponsive, vec!["broken".to_string()]);
    assert_eq!(outcome.answered_by.as_deref(), Some("good"));
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].title, "Good");
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_tier_failing_is_an_empty_outcome_and_not_an_error(pool: PgPool) {
    let harness = TestState::new(pool);
    let broken = Arc::new(FailingTier::new("broken"));
    // A tier that answers successfully with nothing is as unresponsive as one that
    // errors, and must be recorded the same way.
    let empty = Arc::new(StubTier::new("empty", Vec::new()));
    let search = composite(&harness.state.pool, vec![broken.clone(), empty.clone()]);

    let outcome = search.search("a query", 8).await;

    assert!(outcome.hits.is_empty());
    assert_eq!(
        outcome.unresponsive,
        vec!["broken".to_string(), "empty".to_string()],
        "every tier must be named so the admin report can see the free path degrade"
    );
    assert!(outcome.answered_by.is_none());
    assert!(!outcome.cache_hit);
    assert_eq!(broken.calls(), 1);
    assert_eq!(empty.calls(), 1);
    assert_eq!(
        cache_rows(&harness.state.pool).await,
        0,
        "an empty result set must not be cached"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_same_query_twice_calls_the_tier_once(pool: PgPool) {
    let harness = TestState::new(pool);
    let tier = Arc::new(StubTier::new(
        "only",
        vec![hit(
            "Cached",
            "https://example.org/c",
            SourceKind::Web,
            "only",
        )],
    ));
    let search = composite(&harness.state.pool, vec![tier.clone()]);

    let first = search.search("cache me", 8).await;
    let second = search.search("cache me", 8).await;
    // A different spelling of the same question: the key is built from the
    // normalised query, so this must be the same row.
    let third = search.search("  CACHE   me ", 8).await;

    assert_eq!(
        tier.calls(),
        1,
        "the A40 acceptance criterion: a repeated query is a cache hit"
    );
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert!(third.cache_hit);
    assert_eq!(second.answered_by.as_deref(), Some("only"));
    assert_eq!(second.hits.len(), 1);
    assert_eq!(second.hits[0].title, "Cached");
    assert_eq!(cache_rows(&harness.state.pool).await, 1);

    let key: String = sqlx::query_scalar("select key from search_cache")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(key, dedupe_key("cache me", "only", 8));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_cache_row_is_not_served(pool: PgPool) {
    let harness = TestState::new(pool);
    let tier = Arc::new(StubTier::new(
        "only",
        vec![hit(
            "Fresh",
            "https://example.org/f",
            SourceKind::Web,
            "only",
        )],
    ));
    let search = composite(&harness.state.pool, vec![tier.clone()]);

    let key = dedupe_key("stale query", "only", 8);
    let stale = vec![hit(
        "Stale",
        "https://example.org/s",
        SourceKind::Web,
        "only",
    )];
    sqlx::query(
        "insert into search_cache (key, provider, results, expires_at)
         values ($1, $2, $3, now() - interval '1 hour')",
    )
    .bind(key.as_str())
    .bind("only")
    .bind(Json(serde_json::to_value(&stale).unwrap()))
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let outcome = search.search("stale query", 8).await;

    assert_eq!(
        tier.calls(),
        1,
        "an expired row must not satisfy the request"
    );
    assert!(!outcome.cache_hit);
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].title, "Fresh");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_cache_is_global_and_not_per_user(pool: PgPool) {
    let harness = TestState::new(pool);
    // Two accounts, because the property under test is that the second one's
    // identical query is served from the first one's row (A40). A search carries
    // no user at all, so the only way the cache could be per-user is a column.
    insert_user(&harness.state.pool, "alice@example.test").await;
    insert_user(&harness.state.pool, "bob@example.test").await;

    let tier = Arc::new(StubTier::new(
        "only",
        vec![hit(
            "Shared",
            "https://example.org/s",
            SourceKind::Web,
            "only",
        )],
    ));
    let search = composite(&harness.state.pool, vec![tier.clone()]);

    let first = search.search("shared question", 8).await;
    let second = search.search("shared question", 8).await;

    assert_eq!(tier.calls(), 1);
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert_eq!(cache_rows(&harness.state.pool).await, 1);
    assert_eq!(user_rows(&harness.state.pool).await, 2);

    let scoped_columns: i64 = sqlx::query_scalar(
        "select count(*) from information_schema.columns
         where table_name = 'search_cache'
           and column_name in ('user_id', 'workspace_id', 'device_id')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        scoped_columns, 0,
        "A40: the cache is global, so it carries no user scope"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_composite_ranks_what_it_returns(pool: PgPool) {
    let harness = TestState::new(pool);
    // A web tier that also surfaced a scholarly link, which is what SearXNG does
    // in practice, and in the tier's own order the web page comes first.
    let tier = Arc::new(StubTier::new(
        "web",
        vec![
            hit(
                "A blog post",
                "https://blog.example.org/a",
                SourceKind::Web,
                "web",
            ),
            hit(
                "A paper",
                "https://example.org/paper",
                SourceKind::Academic,
                "web",
            ),
        ],
    ));
    let search = composite(&harness.state.pool, vec![tier]);

    let outcome = search.search("ranking", 8).await;

    assert_eq!(outcome.hits.len(), 2);
    assert_eq!(
        outcome.hits[0].source_kind,
        SourceKind::Academic,
        "rank() must put an academic hit above a web one"
    );
    assert_eq!(outcome.hits[0].title, "A paper");
    assert_eq!(outcome.hits[1].title, "A blog post");
}

#[sqlx::test(migrations = "../../migrations")]
async fn searxng_maps_results_and_surfaces_its_unresponsive_engines(pool: PgPool) {
    let harness = TestState::new(pool);
    let body = r#"{
      "query": "a query",
      "results": [
        {
          "url": "https://example.org/a",
          "title": "A result",
          "content": "A snippet.",
          "publishedDate": null,
          "score": 0.7,
          "engine": "brave"
        }
      ],
      "unresponsive_engines": [["duckduckgo", "Suspended: too many requests"]]
    }"#
    .to_string();
    let (base_url, server) = serve_json(body).await;

    let tier = Arc::new(SearxngSearch::new(&base_url).unwrap());
    let search = composite(&harness.state.pool, vec![tier.clone()]).with_searxng(tier.clone());

    let outcome = search.search("a query", 8).await;
    server.abort();

    assert_eq!(outcome.answered_by.as_deref(), Some("searxng"));
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].title, "A result");
    assert_eq!(outcome.hits[0].url, "https://example.org/a");
    assert_eq!(outcome.hits[0].source_kind, SourceKind::Web);
    assert_eq!(outcome.hits[0].provider, "searxng");
    assert_eq!(
        outcome.unresponsive_engines,
        vec!["duckduckgo: Suspended: too many requests".to_string()],
        "the engine list must reach the outcome, not be dropped"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_searxng_answer_is_ok_and_still_reports_the_engines(pool: PgPool) {
    let harness = TestState::new(pool);
    let body = r#"{
      "results": [],
      "unresponsive_engines": [["startpage", "timeout"], ["mojeek", "timeout"]]
    }"#
    .to_string();
    let (base_url, server) = serve_json(body).await;

    let tier = Arc::new(SearxngSearch::new(&base_url).unwrap());
    let fallback = Arc::new(StubTier::new(
        "fallback",
        vec![hit(
            "Fallback",
            "https://example.org/f",
            SourceKind::Web,
            "fallback",
        )],
    ));
    let search = composite(&harness.state.pool, vec![tier.clone(), fallback.clone()])
        .with_searxng(tier.clone());

    let outcome = search.search("a query", 8).await;
    server.abort();

    assert_eq!(outcome.answered_by.as_deref(), Some("fallback"));
    assert_eq!(outcome.unresponsive, vec!["searxng".to_string()]);
    assert_eq!(
        outcome.unresponsive_engines,
        vec![
            "startpage: timeout".to_string(),
            "mojeek: timeout".to_string()
        ]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_pinned_tier_constructs_only_that_tier(pool: PgPool) {
    let harness = TestState::new(pool);
    // `SEARCH_PROVIDER=exa` must build one tier and nothing else, so a deployment
    // that asked for one thing cannot quietly answer from another (decision 6).
    let exa = CompositeSearch::new(
        &search_config(SearchTier::Exa, None, true, false),
        harness.state.pool.clone(),
    )
    .expect("a pinned exa tier with a key");
    assert_eq!(exa.tier_names(), vec!["exa"]);

    let tavily = CompositeSearch::new(
        &search_config(SearchTier::Tavily, None, false, true),
        harness.state.pool.clone(),
    )
    .expect("a pinned tavily tier with a key");
    assert_eq!(tavily.tier_names(), vec!["tavily"]);

    let academic = CompositeSearch::new(
        &search_config(SearchTier::Academic, None, false, false),
        harness.state.pool.clone(),
    )
    .expect("the academic tier is keyless");
    assert_eq!(
        academic.tier_names(),
        vec!["openalex", "crossref", "arxiv", "doaj", "eric", "pubmed"],
        "the academic tier is six APIs in A3's order"
    );

    let searxng = CompositeSearch::new(
        &search_config(
            SearchTier::Searxng,
            Some("http://127.0.0.1:8088"),
            false,
            false,
        ),
        harness.state.pool.clone(),
    )
    .expect("a pinned searxng tier with a url");
    assert_eq!(searxng.tier_names(), vec!["searxng"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_pinned_tier_that_is_unconfigured_is_none(pool: PgPool) {
    let harness = TestState::new(pool);
    // A deployment that asked for a tier it has no key for is broken, and the
    // composite says so rather than booting into a tier that cannot answer.
    assert!(
        CompositeSearch::new(
            &search_config(SearchTier::Exa, None, false, false),
            harness.state.pool.clone()
        )
        .is_none()
    );
    assert!(
        CompositeSearch::new(
            &search_config(SearchTier::Tavily, None, false, false),
            harness.state.pool.clone()
        )
        .is_none()
    );
    assert!(
        CompositeSearch::new(
            &search_config(SearchTier::Searxng, None, false, false),
            harness.state.pool.clone()
        )
        .is_none()
    );

    // The composite is the opposite case: no keys and no URL is the normal
    // deployment, and it still walks the academic tier.
    let composite = CompositeSearch::new(
        &search_config(SearchTier::Composite, None, false, false),
        harness.state.pool.clone(),
    )
    .expect("the composite needs no keys");
    assert_eq!(composite.tier_names().len(), 6);
}

#[tokio::test]
async fn an_unreachable_searxng_is_unavailable() {
    // A port with nothing listening on it: the shape a stopped container produces.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let tier = SearxngSearch::new(&format!("http://{address}")).unwrap();
    let error = tier.search("a query", 8).await.unwrap_err();

    assert!(
        matches!(error, SearchError::Unavailable(_)),
        "a stopped SearXNG must be Unavailable, got {error:?}"
    );
}
