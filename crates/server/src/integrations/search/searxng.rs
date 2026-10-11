//! SearXNG, the second tier (A3): a self-hosted metasearch service, so it needs
//! no key and no quota, at the price of being only as good as the engines it can
//! reach from where it runs.
//!
//! That price is the reason this module is more than a mapping. SearXNG answers
//! `unresponsive_engines` alongside its results, and with the tuned engine list
//! from A3 (a handful of engines, some of which are blocked or rate-limited from
//! any given host) an empty `results` usually means "most of the engines did not
//! answer" rather than "the web had nothing". The frozen `SearchProvider` trait has
//! nowhere to put that list, so the tier keeps the last answer and the composite
//! copies it into the outcome that becomes the `usage_events` row.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};

use crate::integrations::search::{http_client, json_body, parse_date};

/// SearXNG is a local service, so a slow answer means it is wedged and the walk
/// should move on rather than wait for the academic tier's timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SearxngSearch {
    client: reqwest::Client,
    base_url: String,
    unresponsive: Arc<Mutex<Vec<String>>>,
}

impl SearxngSearch {
    pub fn new(base_url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: http_client(REQUEST_TIMEOUT)?,
            base_url: base_url.trim_end_matches('/').to_string(),
            unresponsive: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// The engines the last search reported as unresponsive, in SearXNG's order.
    pub fn unresponsive_engines(&self) -> Vec<String> {
        self.engines().clone()
    }

    /// Cleared before each walk so a request that never reaches this tier cannot
    /// inherit the previous one's engine list.
    pub fn clear_unresponsive_engines(&self) {
        self.engines().clear();
    }

    fn engines(&self) -> MutexGuard<'_, Vec<String>> {
        // A panic elsewhere while the lock was held does not make the list wrong,
        // and losing the engine report would be worse than taking it back.
        self.unresponsive
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait::async_trait]
impl SearchProvider for SearxngSearch {
    fn name(&self) -> &'static str {
        "searxng"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        // A stopped or unreachable SearXNG is the case the whole free path is
        // designed around, so it is `Unavailable` and the composite moves on.
        let response = self
            .client
            .get(format!("{}/search", self.base_url))
            .query(&[("q", q), ("format", "json")])
            .send()
            .await
            .map_err(|error| {
                SearchError::Unavailable(format!("searxng at {}: {error}", self.base_url))
            })?;

        let value = json_body(response, "searxng").await?;
        *self.engines() = parse_unresponsive_engines(&value);

        // The JSON API has no result-count parameter, so the cap is applied here.
        let mut hits = parse_searxng(&value);
        hits.truncate(max_results.max(1) as usize);
        Ok(hits)
    }
}

/// SearXNG's `results[]`: `url`, `title`, `content`, `publishedDate`, `score`.
/// `publishedDate` is null for most engines (A3 measured null in 20 of 20), so a
/// missing one is the normal case and not a defect to work around.
pub fn parse_searxng(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(results) = value.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results.iter().filter_map(searxng_hit).collect()
}

fn searxng_hit(result: &serde_json::Value) -> Option<SearchHit> {
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
        "searxng",
    );
    hit.published_at = result
        .get("publishedDate")
        .and_then(|v| v.as_str())
        .and_then(parse_date);
    hit.score = result
        .get("score")
        .and_then(|v| v.as_f64())
        .map(|score| score as f32);
    Some(hit)
}

/// SearXNG lists a failed engine as `[name, reason]`, and the reason is kept
/// because "rate limited" and "no results" call for different fixes. A bare name
/// is accepted too: the shape has changed across versions.
pub fn parse_unresponsive_engines(value: &serde_json::Value) -> Vec<String> {
    let Some(engines) = value.get("unresponsive_engines").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    engines
        .iter()
        .filter_map(|engine| match engine {
            serde_json::Value::String(name) => Some(name.clone()),
            serde_json::Value::Array(parts) => {
                let name = parts.first()?.as_str()?;
                Some(match parts.get(1).and_then(|v| v.as_str()) {
                    Some(reason) => format!("{name}: {reason}"),
                    None => name.to_string(),
                })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn results_map_to_web_hits() {
        let body = serde_json::json!({
            "query": "rust",
            "number_of_results": 2,
            "results": [
                {
                    "url": "https://example.org/a",
                    "title": "A result",
                    "content": "A snippet.",
                    "publishedDate": "2023-04-05T00:00:00",
                    "score": 0.8,
                    "engine": "brave"
                },
                {
                    "url": "https://example.org/b",
                    "title": "No date",
                    "content": "Another.",
                    "publishedDate": null,
                    "score": 0.4
                }
            ]
        });

        let hits = parse_searxng(&body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "A result");
        assert_eq!(hits[0].url, "https://example.org/a");
        assert_eq!(hits[0].snippet, "A snippet.");
        assert_eq!(hits[0].source_kind, SourceKind::Web);
        assert_eq!(hits[0].provider, "searxng");
        assert_eq!(hits[0].score, Some(0.8));
        assert_eq!(
            hits[0].published_at,
            Some(Utc.with_ymd_and_hms(2023, 4, 5, 0, 0, 0).unwrap())
        );
        assert_eq!(hits[1].published_at, None);
    }

    #[test]
    fn an_unresponsive_engine_keeps_its_reason() {
        let body = serde_json::json!({
            "results": [],
            "unresponsive_engines": [
                ["duckduckgo", "Suspended: too many requests"],
                ["startpage", "timeout"],
                "mojeek"
            ]
        });

        assert!(parse_searxng(&body).is_empty());
        assert_eq!(
            parse_unresponsive_engines(&body),
            vec![
                "duckduckgo: Suspended: too many requests".to_string(),
                "startpage: timeout".to_string(),
                "mojeek".to_string(),
            ]
        );
    }

    #[test]
    fn a_response_without_the_field_reports_no_engines() {
        assert!(parse_unresponsive_engines(&serde_json::json!({ "results": [] })).is_empty());
    }
}
