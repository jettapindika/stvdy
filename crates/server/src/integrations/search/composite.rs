//! The composite walk (A3, A40).
//!
//! One `SearchProvider` shape in front of the tiers, tried in order until one
//! answers. Nothing here fails a request: a tier that errors or returns nothing is
//! recorded and the walk continues, so a chat turn with every tier down still
//! answers from the user's own sources.
//!
//! The cache is checked per tier, not once up front, because a cached result set
//! belongs to the tier that produced it: a hit on tier 1 must not be served as if
//! tier 2 had answered, and a tier with no cached answer is simply called.

use std::sync::Arc;
use std::time::Duration;

use secrecy::ExposeSecret;
use sqlx::PgPool;
use sqlx::types::Json;
use stvdy_core::search::normalize::dedupe_key;
use stvdy_core::search::rank::rank;
use stvdy_core::traits::search::{SearchHit, SearchProvider};

use crate::config::{SearchConfig, SearchTier};
use crate::integrations::search::academic;
use crate::integrations::search::exa::ExaSearch;
use crate::integrations::search::searxng::SearxngSearch;
use crate::integrations::search::tavily::TavilySearch;

/// What a composite search did, so one `usage_events` row can carry the tier that
/// answered plus the ones that did not (A3/A40).
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// The tier that returned results, if any did.
    pub answered_by: Option<String>,
    /// Tiers consulted and found empty, erroring, or unreachable. This is what
    /// makes silent degradation of the free path visible in the admin report.
    pub unresponsive: Vec<String>,
    /// Whether the results came from the cache, which decides whether the request
    /// is charged (A40).
    pub cache_hit: bool,
    /// The engines SearXNG reported as unresponsive for this search, empty unless
    /// the SearXNG tier was the one consulted (A3).
    pub unresponsive_engines: Vec<String>,
}

pub struct CompositeSearch {
    tiers: Vec<Arc<dyn SearchProvider>>,
    pool: PgPool,
    ttl: Duration,
    /// The SearXNG tier, kept typed as well as in `tiers` so its engine report can
    /// reach the outcome. The trait is frozen and has nowhere to put it, and a
    /// downcast would be worse than one named field.
    searxng: Option<Arc<SearxngSearch>>,
}

/// Drops hits whose URL cannot be offered to the user as a citation.
///
/// Applied before ranking and before caching, so an unusable URL can never reach a
/// citation and is never stored. See `fetch::is_citable_url` for why this is a
/// boundary concern rather than each consumer's own: the citation path never goes
/// through the fetcher.
fn drop_uncitable(hits: Vec<SearchHit>) -> Vec<SearchHit> {
    let before = hits.len();
    let kept: Vec<SearchHit> = hits
        .into_iter()
        .filter(|hit| crate::integrations::fetch::is_citable_url(&hit.url))
        .collect();

    if kept.len() != before {
        // Worth a log line: a tier returning unusable URLs is either broken or
        // hostile, and either way an operator wants to see it.
        tracing::warn!(
            dropped = before - kept.len(),
            "a search tier returned hits with URLs that cannot be cited"
        );
    }
    kept
}

impl CompositeSearch {
    /// Builds the tiers `SEARCH_PROVIDER` asks for, in A3's order: the general
    /// web first, the academic APIs behind it, then the paid fallbacks.
    ///
    /// **The web tier is first because the academic APIs match almost any word.**
    /// The walk stops at the first tier that returns anything, and a scholarly
    /// index answers a question about a Windows update error with an unrelated
    /// paper, so with the academic APIs first SearXNG was never consulted and a
    /// general question got no usable result. Academic hits are the better answer
    /// for a study app, which is why they are still walked — just after the tier
    /// that can answer a general question.
    ///
    /// `None` when a *pinned* tier is unconfigured, which is a different thing
    /// from the composite not having it: a deployment running `SEARCH_PROVIDER=exa`
    /// with no key asked for something it cannot serve, while a composite without
    /// a key is the normal case. `SEARCH_PROVIDER` is validated at boot for the
    /// same reason; this is the second line of defence.
    pub fn new(config: &SearchConfig, pool: PgPool) -> Option<Self> {
        let mut tiers: Vec<Arc<dyn SearchProvider>> = Vec::new();
        let mut searxng = None;

        if matches!(config.tier, SearchTier::Composite | SearchTier::Searxng) {
            match &config.searxng_url {
                Some(url) => {
                    let tier = Arc::new(SearxngSearch::new(url).ok()?);
                    searxng = Some(tier.clone());
                    tiers.push(tier);
                }
                None if config.tier == SearchTier::Searxng => return None,
                None => {}
            }
        }

        if matches!(config.tier, SearchTier::Composite | SearchTier::Academic) {
            tiers.extend(academic::providers().ok()?);
        }

        if matches!(config.tier, SearchTier::Composite | SearchTier::Exa) {
            match &config.exa_api_key {
                Some(key) => tiers.push(Arc::new(ExaSearch::new(key.expose_secret()).ok()?)),
                None if config.tier == SearchTier::Exa => return None,
                None => {}
            }
        }

        if matches!(config.tier, SearchTier::Composite | SearchTier::Tavily) {
            match &config.tavily_api_key {
                Some(key) => tiers.push(Arc::new(TavilySearch::new(key.expose_secret()).ok()?)),
                None if config.tier == SearchTier::Tavily => return None,
                None => {}
            }
        }

        Some(Self {
            tiers,
            pool,
            ttl: config.cache_ttl,
            searxng,
        })
    }

    /// A composite over an explicit tier list, which is what a test needs to script
    /// the walk without a network, a key or a search API.
    pub fn with_tiers(tiers: Vec<Arc<dyn SearchProvider>>, pool: PgPool, ttl: Duration) -> Self {
        Self {
            tiers,
            pool,
            ttl,
            searxng: None,
        }
    }

    /// Registers the SearXNG tier a second time, typed, so the outcome can carry
    /// the engines it reported as unresponsive.
    pub fn with_searxng(mut self, tier: Arc<SearxngSearch>) -> Self {
        self.searxng = Some(tier);
        self
    }

    /// The tiers this composite will consult, in order. Used by the admin report
    /// and by the boot log, which is how an operator sees what a deployment built.
    pub fn tier_names(&self) -> Vec<&'static str> {
        self.tiers.iter().map(|tier| tier.name()).collect()
    }

    /// Walks the tiers in order. Returns what happened rather than a `Result`: a
    /// search that every tier failed is still a successful search with no hits,
    /// because the chat turn that asked for it must answer (A3).
    pub async fn search(&self, q: &str, max_results: u8) -> SearchOutcome {
        let mut outcome = SearchOutcome::default();
        // Cleared first so a request that never reaches the SearXNG tier cannot
        // inherit the engine list of the request before it.
        if let Some(searxng) = &self.searxng {
            searxng.clear_unresponsive_engines();
        }

        // The cache is consulted for EVERY tier before any tier is called.
        //
        // Reading and writing per tier inside one loop looks equivalent and is not:
        // which tier answers is not stable, because a free API returns empty under
        // load without erroring. A repeat of the same query could then miss the
        // entry the previous answer wrote (cached under `crossref`, while
        // `openalex` is tried first and answers this time), call the network again,
        // and bill the user a second search for a question already answered. Found
        // by the smoke run, whose query was answered by two different tiers on two
        // consecutive identical calls.
        for tier in &self.tiers {
            let key = dedupe_key(q, tier.name(), max_results);
            if let Some(hits) = self.read_cache(&key).await {
                outcome.hits = hits;
                outcome.answered_by = Some(tier.name().to_string());
                outcome.cache_hit = true;
                return self.finish(outcome);
            }
        }

        for tier in &self.tiers {
            let key = dedupe_key(q, tier.name(), max_results);
            match tier.search(q, max_results).await {
                Ok(hits) if !hits.is_empty() => {
                    // Unusable URLs are dropped before ranking and before caching, so
                    // they can never become a citation and are never stored. A hit
                    // whose URL is a `javascript:` scheme or a literal private address
                    // is not a source the user can open, and offering it as one is how
                    // a search tier gets the user to click something they should not.
                    let hits = rank(drop_uncitable(hits));
                    self.write_cache(&key, tier.name(), &hits).await;
                    outcome.answered_by = Some(tier.name().to_string());
                    outcome.hits = hits;
                    break;
                }
                Ok(_) => outcome.unresponsive.push(tier.name().to_string()),
                Err(error) => {
                    tracing::warn!(tier = tier.name(), %error, "search tier did not answer");
                    outcome.unresponsive.push(tier.name().to_string());
                }
            }
        }

        self.finish(outcome)
    }

    /// Attaches the SearXNG engine report, which is per-instance state rather than
    /// per-search, and returns the outcome.
    fn finish(&self, mut outcome: SearchOutcome) -> SearchOutcome {
        if let Some(searxng) = &self.searxng {
            outcome.unresponsive_engines = searxng.unresponsive_engines();
        }
        outcome
    }

    /// A row past its `expires_at` is not served: the TTL is the freshness
    /// mechanism, so an expired row is the same as no row at all.
    async fn read_cache(&self, key: &str) -> Option<Vec<SearchHit>> {
        let row = sqlx::query_scalar::<_, Json<Vec<SearchHit>>>(
            "select results from search_cache where key = $1 and expires_at > now()",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await;
        match row {
            Ok(Some(Json(hits))) => Some(hits),
            Ok(None) => None,
            // A cache that cannot be read must not fail the search: the provider
            // call is the point and the cache is the optimization.
            Err(error) => {
                tracing::warn!(%error, "reading the search cache");
                None
            }
        }
    }

    async fn write_cache(&self, key: &str, provider: &str, hits: &[SearchHit]) {
        let result = sqlx::query(
            "insert into search_cache (key, provider, results, expires_at)
             values ($1, $2, $3, now() + make_interval(secs => $4))
             on conflict (key) do update
             set provider = excluded.provider,
                 results = excluded.results,
                 fetched_at = now(),
                 expires_at = excluded.expires_at",
        )
        .bind(key)
        .bind(provider)
        .bind(Json(hits))
        .bind(self.ttl.as_secs_f64())
        .execute(&self.pool)
        .await;
        if let Err(error) = result {
            // A write that fails costs the next request a provider call, which is
            // the pre-cache behaviour, so it is a warning rather than an error.
            tracing::warn!(%error, "writing the search cache");
        }
    }
}
