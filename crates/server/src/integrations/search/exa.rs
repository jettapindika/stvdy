//! Exa, the first fallback API (A3). `POST https://api.exa.ai/search` with the key
//! in `x-api-key`, and the response carries a publication date, which is what
//! makes this tier worth its price when the free ones are thin.

use std::time::Duration;

use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};

use crate::integrations::search::{http_client, json_body, parse_date};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// What the API is asked to return per result. Exa charges for content, and the
/// snippet only has to survive the context budget, so it is bounded here rather
/// than fetched whole and truncated later.
const MAX_CHARACTERS: u32 = 1200;

pub struct ExaSearch {
    client: reqwest::Client,
    api_key: String,
}

impl ExaSearch {
    pub fn new(api_key: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: http_client(REQUEST_TIMEOUT)?,
            api_key: api_key.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl SearchProvider for ExaSearch {
    fn name(&self) -> &'static str {
        "exa"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", &self.api_key)
            .json(&serde_json::json!({
                "query": q,
                "numResults": max_results.max(1),
                // Without `contents` a result is a title and a URL with no text,
                // which is not a snippet.
                "contents": { "text": { "maxCharacters": MAX_CHARACTERS } },
            }))
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("exa: {error}")))?;
        let value = json_body(response, "exa").await?;
        Ok(parse_exa(&value))
    }
}

/// Exa's `results[]`: `title`, `url`, `publishedDate`, `text`, `highlights`,
/// `score`. `publishedDate` is the reason this tier exists, so it is read into
/// `published_at` rather than ignored.
pub fn parse_exa(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(results) = value.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results.iter().filter_map(exa_hit).collect()
}

fn exa_hit(result: &serde_json::Value) -> Option<SearchHit> {
    let url = result.get("url").and_then(|v| v.as_str())?;
    let title = result.get("title").and_then(|v| v.as_str()).unwrap_or(url);
    let snippet = result
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| {
            result
                .get("highlights")
                .and_then(|v| v.as_array())
                .and_then(|highlights| highlights.first())
                .and_then(|v| v.as_str())
        })
        .unwrap_or_default();

    let mut hit = SearchHit::new(title, url, snippet, SourceKind::Web, "exa");
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn results_carry_a_published_date_and_a_score() {
        let body = serde_json::json!({
            "requestId": "abc",
            "results": [
                {
                    "title": "A page",
                    "url": "https://example.org/a",
                    "publishedDate": "2023-06-01",
                    "text": "The page text.",
                    "score": 0.42
                },
                {
                    "title": "No text",
                    "url": "https://example.org/b",
                    "highlights": ["A highlight."],
                    "publishedDate": null
                }
            ]
        });

        let hits = parse_exa(&body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "A page");
        assert_eq!(hits[0].snippet, "The page text.");
        assert_eq!(hits[0].source_kind, SourceKind::Web);
        assert_eq!(hits[0].provider, "exa");
        assert_eq!(hits[0].score, Some(0.42));
        assert_eq!(
            hits[0].published_at,
            Some(Utc.with_ymd_and_hms(2023, 6, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(hits[1].snippet, "A highlight.");
        assert_eq!(hits[1].published_at, None);
    }

    #[test]
    fn a_shapeless_body_yields_no_hits() {
        assert!(parse_exa(&serde_json::json!({})).is_empty());
        assert!(parse_exa(&serde_json::json!({ "results": [] })).is_empty());
    }
}
