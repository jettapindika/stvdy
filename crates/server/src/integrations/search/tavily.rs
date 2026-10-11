//! Tavily, the second fallback API (A3). `POST https://api.tavily.com/search`
//! with `Authorization: Bearer tvly-...`.
//!
//! Tavily has no publication date at all, which A3 measured as null in 20 of 20
//! results, so `published_at` stays `None` rather than being guessed from anything
//! else in the response. Nothing downstream may assume a web hit is dated.

use std::time::Duration;

use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};

use crate::integrations::search::{http_client, json_body};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct TavilySearch {
    client: reqwest::Client,
    api_key: String,
}

impl TavilySearch {
    pub fn new(api_key: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: http_client(REQUEST_TIMEOUT)?,
            api_key: api_key.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl SearchProvider for TavilySearch {
    fn name(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let response = self
            .client
            .post("https://api.tavily.com/search")
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "query": q,
                "max_results": max_results.max(1),
                // `basic` rather than `advanced`: this is the last tier, so its job
                // is to answer at all, and `advanced` costs more per search.
                "search_depth": "basic",
            }))
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("tavily: {error}")))?;
        let value = json_body(response, "tavily").await?;
        Ok(parse_tavily(&value))
    }
}

/// Tavily's `results[]`: `title`, `url`, `content`, `score`. There is no
/// publication date field, so none is invented.
pub fn parse_tavily(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(results) = value.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results.iter().filter_map(tavily_hit).collect()
}

fn tavily_hit(result: &serde_json::Value) -> Option<SearchHit> {
    let url = result.get("url").and_then(|v| v.as_str())?;
    let title = result.get("title").and_then(|v| v.as_str()).unwrap_or(url);

    let mut hit = SearchHit::new(
        title,
        url,
        result
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        SourceKind::Web,
        "tavily",
    );
    hit.score = result
        .get("score")
        .and_then(|v| v.as_f64())
        .map(|score| score as f32);
    Some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_have_no_publication_date() {
        let body = serde_json::json!({
            "query": "rust",
            "response_time": 1.2,
            "results": [{
                "title": "A page",
                "url": "https://example.org/a",
                "content": "The page text.",
                "score": 0.91
            }]
        });

        let hits = parse_tavily(&body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "A page");
        assert_eq!(hits[0].url, "https://example.org/a");
        assert_eq!(hits[0].snippet, "The page text.");
        assert_eq!(hits[0].source_kind, SourceKind::Web);
        assert_eq!(hits[0].provider, "tavily");
        assert_eq!(hits[0].score, Some(0.91));
        assert_eq!(hits[0].published_at, None);
    }

    #[test]
    fn a_shapeless_body_yields_no_hits() {
        assert!(parse_tavily(&serde_json::json!({})).is_empty());
        assert!(parse_tavily(&serde_json::json!({ "results": [] })).is_empty());
    }
}
