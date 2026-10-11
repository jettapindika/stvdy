//! Web and academic search capability, and page fetching.
//!
//! Two traits, because the two jobs have different failure semantics. A search
//! tier that fails is *never* fatal for the request: the composite records it and
//! moves on, and a query that every tier failed still produces an answer grounded
//! in the user's own sources (A3). A page fetch that fails is a failure of the one
//! thing the caller asked for, so it surfaces.

use chrono::{DateTime, Utc};

/// What a hit is, which drives ranking and citation rendering rather than a badge
/// in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Web,
    Academic,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Real only for the academic tier and Exa. Tavily and SearXNG returned null
    /// in 20 of 20 measured results (A3), so nothing may assume it is present, and
    /// a "recent sources" filter must read this field rather than a timestamp it
    /// invents.
    pub published_at: Option<DateTime<Utc>>,
    pub source_kind: SourceKind,
    /// A real DOI where the API provides one. It is the only stable identifier
    /// across hosts, which is why dedupe checks it before the URL.
    pub doi: Option<String>,
    /// The tier that produced this hit. Used for ranking (tier order breaks ties)
    /// and for the admin view of which tier is actually answering.
    pub provider: String,
    /// The provider's own relevance score, when it reports one. Not comparable
    /// across providers, so it only breaks ties within one tier.
    pub score: Option<f32>,
    pub license: Option<String>,
}

impl SearchHit {
    /// A hit with the fields a provider is most likely to fill, so an
    /// implementation does not have to name all eight.
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
        source_kind: SourceKind,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            published_at: None,
            source_kind,
            doi: None,
            provider: provider.into(),
            score: None,
            license: None,
        }
    }
}

/// A tier's failure. Every variant is recoverable from the request's point of
/// view, which is why none of them aborts a chat turn.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// Transport or upstream failure: a timeout, a 500, a rate limit. Worth
    /// retrying later, and the composite moves on now.
    #[error("{0}")]
    Provider(String),
    /// The request itself is wrong: a missing key, a bad parameter.
    #[error("{0}")]
    Invalid(String),
    /// The tier is configured but not reachable, which is what a stopped SearXNG
    /// looks like. Distinct from `Provider` only so the admin report can tell
    /// "we broke it" from "they broke it".
    #[error("{0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError>;
}

/// A fetched page, for "save to workspace" and for expanding a citation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FetchedPage {
    /// Where the fetch ended up, which differs from what was asked for after a
    /// redirect. Stored as the source's URL, so a citation points at the page the
    /// text actually came from.
    pub final_url: String,
    pub title: String,
    pub text: String,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("{0}")]
    Provider(String),
    /// The URL is not one this server will fetch. A distinct variant because the
    /// answer is "no" rather than "try again": it covers a non-http scheme and a
    /// target inside a private network, which is an SSRF attempt rather than a
    /// transient failure (A18/A34).
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Invalid(String),
}

#[async_trait::async_trait]
pub trait PageFetcher: Send + Sync {
    fn name(&self) -> &'static str;

    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError>;
}
