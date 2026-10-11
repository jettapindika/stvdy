//! The academic tier: six scholarly APIs, tried in the order A3 pins them:
//! OpenAlex, Crossref, arXiv, DOAJ, ERIC, PubMed.
//!
//! All six are keyless, which is what gives a deployment with no paid fallback a
//! working web tier, and it is why this tier is first: it is the one that always
//! answers. Each maps its own response shape to `SearchHit`; the mapping functions
//! are `pub` and take the decoded body, so they can be tested against a real
//! response without a network.
//!
//! Three of these APIs are not what a guess would produce, and the code below says
//! so where it matters:
//!
//! - arXiv answers Atom XML, not JSON. The workspace has no XML crate and this is
//!   the only XML any tier receives, so `parse_arxiv` reads the six elements a hit
//!   needs directly rather than adding a dependency for one feed.
//! - OpenAlex has no abstract field. It ships `abstract_inverted_index`, an
//!   inverted index, because it may not redistribute the text.
//! - ERIC's JSON has no DOI, so `doi` is `None` for every ERIC hit and those hits
//!   dedupe on URL alone.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use stvdy_core::traits::search::{SearchError, SearchHit, SearchProvider, SourceKind};

use crate::integrations::search::{
    as_int, collapse_whitespace, doi_bare, http_client, json_body, midnight, parse_date,
    strip_markup, unescape_xml,
};

/// Scholarly APIs answer slowly under load, and a slow tier must not hold a chat
/// turn open: the composite moves to the next one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The six APIs in the order A3 pins, so the composite and a pinned `academic`
/// tier walk the same list.
pub fn providers() -> anyhow::Result<Vec<Arc<dyn SearchProvider>>> {
    // One client for all six: they are the same origin shape and share a pool.
    let client = http_client(REQUEST_TIMEOUT)?;
    Ok(vec![
        Arc::new(OpenAlex::new(client.clone())),
        Arc::new(Crossref::new(client.clone())),
        Arc::new(Arxiv::new(client.clone())),
        Arc::new(Doaj::new(client.clone())),
        Arc::new(Eric::new(client.clone())),
        Arc::new(PubMed::new(client)),
    ])
}

/// `max_results` reaches the APIs as a page size, and every one of them rejects
/// zero, so a caller asking for none gets one rather than a 400.
fn page_size(max_results: u8) -> String {
    max_results.max(1).to_string()
}

// ---------------------------------------------------------------------------
// OpenAlex (1a)
// ---------------------------------------------------------------------------

pub struct OpenAlex {
    client: reqwest::Client,
}

impl OpenAlex {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchProvider for OpenAlex {
    fn name(&self) -> &'static str {
        "openalex"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let per_page = page_size(max_results);
        // No `select`: a wrong field name in it is a 403 rather than a smaller
        // response, and the payload is not the expensive part of this call.
        let response = self
            .client
            .get("https://api.openalex.org/works")
            .query(&[("search", q), ("per-page", per_page.as_str())])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("openalex: {error}")))?;
        let value = json_body(response, "openalex").await?;
        Ok(parse_openalex(&value))
    }
}

/// OpenAlex `/works`. `title` is the current field name and `display_name` the one
/// it replaced, so both are read.
pub fn parse_openalex(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(results) = value.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results.iter().filter_map(openalex_hit).collect()
}

fn openalex_hit(work: &serde_json::Value) -> Option<SearchHit> {
    let title = work
        .get("title")
        .and_then(|v| v.as_str())
        .or_else(|| work.get("display_name").and_then(|v| v.as_str()))?;
    let location = work
        .get("primary_location")
        .filter(|v| !v.is_null())
        .or_else(|| work.get("best_oa_location").filter(|v| !v.is_null()));
    let url = location
        .and_then(|l| l.get("landing_page_url"))
        .and_then(|v| v.as_str())
        .or_else(|| work.get("doi").and_then(|v| v.as_str()))
        .or_else(|| work.get("id").and_then(|v| v.as_str()))?;

    let mut hit = SearchHit::new(
        title,
        url,
        openalex_abstract(work),
        SourceKind::Academic,
        "openalex",
    );
    hit.doi = work.get("doi").and_then(|v| v.as_str()).map(doi_bare);
    hit.published_at = work
        .get("publication_date")
        .and_then(|v| v.as_str())
        .and_then(parse_date);
    hit.license = location
        .and_then(|l| l.get("license"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    hit.score = work
        .get("relevance_score")
        .and_then(|v| v.as_f64())
        .map(|score| score as f32);
    Some(hit)
}

/// OpenAlex cannot redistribute the abstract, so it publishes it as an inverted
/// index of word to positions and reassembling it is the documented way to read
/// it. An absent index means the work has no abstract, not an empty one.
fn openalex_abstract(work: &serde_json::Value) -> String {
    let Some(index) = work
        .get("abstract_inverted_index")
        .and_then(|v| v.as_object())
    else {
        return String::new();
    };
    let mut positioned: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in index {
        for position in positions.as_array().into_iter().flatten() {
            if let Some(position) = position.as_u64() {
                positioned.push((position, word.as_str()));
            }
        }
    }
    positioned.sort_by_key(|(position, _)| *position);
    positioned
        .iter()
        .map(|(_, word)| *word)
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Crossref (1b)
// ---------------------------------------------------------------------------

pub struct Crossref {
    client: reqwest::Client,
}

impl Crossref {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchProvider for Crossref {
    fn name(&self) -> &'static str {
        "crossref"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let rows = page_size(max_results);
        let response = self
            .client
            .get("https://api.crossref.org/works")
            .query(&[("query", q), ("rows", rows.as_str())])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("crossref: {error}")))?;
        let value = json_body(response, "crossref").await?;
        Ok(parse_crossref(&value))
    }
}

/// Crossref wraps everything twice: the payload is `message`, and the works are
/// `message.items`. `title` and `container-title` are arrays, and only the first
/// element is the title.
pub fn parse_crossref(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(items) = value
        .get("message")
        .and_then(|message| message.get("items"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    items.iter().filter_map(crossref_hit).collect()
}

fn crossref_hit(item: &serde_json::Value) -> Option<SearchHit> {
    let title = item
        .get("title")
        .and_then(|v| v.as_array())
        .and_then(|titles| titles.first())
        .and_then(|v| v.as_str())?;
    let doi = item.get("DOI").and_then(|v| v.as_str()).map(doi_bare);
    let url = item
        .get("URL")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| doi.as_ref().map(|doi| format!("https://doi.org/{doi}")))?;
    let journal = item
        .get("container-title")
        .and_then(|v| v.as_array())
        .and_then(|names| names.first())
        .and_then(|v| v.as_str());
    let abstract_text = item
        .get("abstract")
        .and_then(|v| v.as_str())
        .map(strip_markup)
        .unwrap_or_default();
    // Many records have no abstract, and the journal is a better snippet than
    // nothing when they do not.
    let snippet = if abstract_text.trim().is_empty() {
        journal.unwrap_or_default().to_string()
    } else {
        abstract_text
    };

    let mut hit = SearchHit::new(title, url, snippet, SourceKind::Academic, "crossref");
    hit.doi = doi;
    hit.published_at = crossref_date(item);
    hit.license = item
        .get("license")
        .and_then(|v| v.as_array())
        .and_then(|licenses| licenses.first())
        .and_then(|license| license.get("URL"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    hit.score = item
        .get("score")
        .and_then(|v| v.as_f64())
        .map(|score| score as f32);
    Some(hit)
}

/// Crossref dates are `{"date-parts": [[2024, 1, 2]]}` under one of several keys,
/// and which key is present depends on the record: `published` is the online date,
/// `published-print` the issue, `issued` the earliest, `created` the deposit. They
/// are tried in that order because the online date is the one a reader sees.
fn crossref_date(item: &serde_json::Value) -> Option<DateTime<Utc>> {
    for key in ["published", "published-print", "issued", "created"] {
        let parts = item
            .get(key)
            .and_then(|v| v.get("date-parts"))
            .and_then(|v| v.as_array())
            .and_then(|sets| sets.first())
            .and_then(|v| v.as_array());
        if let Some(date) = parts.and_then(|parts| date_parts(parts.as_slice())) {
            return Some(date);
        }
    }
    None
}

fn date_parts(parts: &[serde_json::Value]) -> Option<DateTime<Utc>> {
    let year = parts.first().and_then(as_int)? as i32;
    let month = parts.get(1).and_then(as_int).unwrap_or(1) as u32;
    let day = parts.get(2).and_then(as_int).unwrap_or(1) as u32;
    NaiveDate::from_ymd_opt(year, month, day).map(midnight)
}

// ---------------------------------------------------------------------------
// arXiv (1c)
// ---------------------------------------------------------------------------

pub struct Arxiv {
    client: reqwest::Client,
}

impl Arxiv {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchProvider for Arxiv {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let max_results = page_size(max_results);
        let response = self
            .client
            .get("https://export.arxiv.org/api/query")
            .query(&[
                ("search_query", format!("all:{q}").as_str()),
                ("start", "0"),
                ("max_results", max_results.as_str()),
                ("sortBy", "relevance"),
            ])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("arxiv: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(SearchError::Provider(format!("arxiv answered {status}")));
        }
        let feed = response
            .text()
            .await
            .map_err(|error| SearchError::Provider(format!("arxiv: reading the feed: {error}")))?;
        Ok(parse_arxiv(&feed))
    }
}

/// arXiv answers an Atom feed. Only `<entry>` blocks are read, because the feed's
/// own `<title>` and `<id>` would otherwise become a hit.
pub fn parse_arxiv(feed: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut rest = feed;
    while let Some(start) = rest.find("<entry>") {
        let after = &rest[start + "<entry>".len()..];
        let Some(end) = after.find("</entry>") else {
            break;
        };
        if let Some(hit) = arxiv_hit(&after[..end]) {
            hits.push(hit);
        }
        rest = &after[end + "</entry>".len()..];
    }
    hits
}

fn arxiv_hit(entry: &str) -> Option<SearchHit> {
    let title = element_text(entry, "title")?;
    let id = element_text(entry, "id")?;
    let url = link_href(entry, "alternate").unwrap_or_else(|| id.clone());
    let snippet = element_text(entry, "summary").unwrap_or_default();

    let mut hit = SearchHit::new(title, url, snippet, SourceKind::Academic, "arxiv");
    hit.published_at = element_text(entry, "published")
        .as_deref()
        .and_then(parse_date);
    // Only some entries carry a journal DOI; a preprint usually has none.
    hit.doi = element_text(entry, "arxiv:doi").map(|doi| doi_bare(&doi));
    hit.license = link_href(entry, "license");
    Some(hit)
}

/// The text of the first `<name>...</name>`, whitespace collapsed. The tag may
/// carry attributes, so the text starts after its `>` rather than after the name.
fn element_text(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let start = xml.find(&open)?;
    let after_open = &xml[start + open.len()..];
    let text = &after_open[after_open.find('>')? + 1..];
    let close = format!("</{name}>");
    let end = text.find(&close)?;
    Some(collapse_whitespace(&unescape_xml(&text[..end])))
}

/// The `href` of the first `<link>` whose `rel` matches, which is how the abstract
/// page is told apart from the PDF and the license. A `link` is self-closing, so
/// it has attributes and no text.
fn link_href(xml: &str, rel: &str) -> Option<String> {
    let mut rest = xml;
    while let Some(start) = rest.find("<link") {
        let after = &rest[start + "<link".len()..];
        let end = after.find('>')?;
        let tag = &after[..end];
        if attribute(tag, "rel").as_deref() == Some(rel) {
            return attribute(tag, "href");
        }
        rest = &after[end..];
    }
    None
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)?;
    let after = &tag[start + needle.len()..];
    let end = after.find('"')?;
    Some(unescape_xml(&after[..end]))
}

// ---------------------------------------------------------------------------
// DOAJ (1d)
// ---------------------------------------------------------------------------

pub struct Doaj {
    client: reqwest::Client,
}

impl Doaj {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchProvider for Doaj {
    fn name(&self) -> &'static str {
        "doaj"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        // DOAJ takes the query in the *path*, so a raw space or `&` in a question
        // would split the URL rather than reach the search.
        let query: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
        let page_size = page_size(max_results);
        let response = self
            .client
            .get(format!("https://doaj.org/api/search/articles/{query}"))
            .query(&[("pageSize", page_size.as_str())])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("doaj: {error}")))?;
        let value = json_body(response, "doaj").await?;
        Ok(parse_doaj(&value))
    }
}

/// DOAJ nests the article under `bibjson`, and both the DOI and the full text URL
/// are entries in typed lists rather than named fields.
pub fn parse_doaj(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(results) = value.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results.iter().filter_map(doaj_hit).collect()
}

fn doaj_hit(record: &serde_json::Value) -> Option<SearchHit> {
    let bibjson = record.get("bibjson")?;
    let title = bibjson.get("title").and_then(|v| v.as_str())?;
    let doi = bibjson
        .get("identifier")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|identifier| identifier.get("type").and_then(|v| v.as_str()) == Some("doi"))
        .and_then(|identifier| identifier.get("id"))
        .and_then(|v| v.as_str())
        .map(doi_bare);
    let full_text = bibjson
        .get("link")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|link| link.get("type").and_then(|v| v.as_str()) == Some("fulltext"))
        .and_then(|link| link.get("url"))
        .and_then(|v| v.as_str());
    let url = full_text
        .map(str::to_string)
        .or_else(|| doi.as_ref().map(|doi| format!("https://doi.org/{doi}")))?;

    let mut hit = SearchHit::new(
        title,
        url,
        bibjson
            .get("abstract")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        SourceKind::Academic,
        "doaj",
    );
    hit.doi = doi;
    hit.published_at = doaj_date(bibjson);
    hit.license = bibjson
        .get("license")
        .and_then(|v| v.as_array())
        .and_then(|licenses| licenses.first())
        .and_then(|license| license.get("type"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(hit)
}

/// DOAJ splits the publication date into a year and a month, both strings, and a
/// month is optional.
fn doaj_date(bibjson: &serde_json::Value) -> Option<DateTime<Utc>> {
    let year = bibjson.get("year").and_then(as_int)? as i32;
    let month = bibjson.get("month").and_then(as_int).unwrap_or(1) as u32;
    NaiveDate::from_ymd_opt(year, month, 1).map(midnight)
}

// ---------------------------------------------------------------------------
// ERIC (1e)
// ---------------------------------------------------------------------------

pub struct Eric {
    client: reqwest::Client,
}

impl Eric {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchProvider for Eric {
    fn name(&self) -> &'static str {
        "eric"
    }

    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let rows = page_size(max_results);
        // No `fields`: the parameter is only useful for narrowing a large record,
        // and a name it does not accept is an error rather than a smaller payload.
        let response = self
            .client
            .get("https://api.ies.ed.gov/eric/")
            .query(&[("search", q), ("format", "json"), ("rows", rows.as_str())])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("eric: {error}")))?;
        let value = json_body(response, "eric").await?;
        Ok(parse_eric(&value))
    }
}

/// ERIC wraps its documents in `response.docs` and publishes only a publication
/// *year*, so every hit lands on the first of January.
pub fn parse_eric(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(docs) = value
        .get("response")
        .and_then(|response| response.get("docs"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    docs.iter().filter_map(eric_hit).collect()
}

fn eric_hit(doc: &serde_json::Value) -> Option<SearchHit> {
    let title = doc.get("title").and_then(|v| v.as_str())?;
    let id = doc.get("id").and_then(|v| v.as_str());
    // ERIC's own record page is always resolvable, so it is the fallback when the
    // record carries no external URL.
    let url = doc
        .get("url")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| id.map(|id| format!("https://eric.ed.gov/?id={id}")))?;

    let mut hit = SearchHit::new(
        title,
        url,
        doc.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        SourceKind::Academic,
        "eric",
    );
    // ERIC's JSON carries no DOI field, so an ERIC hit is never deduped by one.
    hit.published_at = doc
        .get("publicationdateyear")
        .and_then(as_int)
        .and_then(|year| NaiveDate::from_ymd_opt(year as i32, 1, 1))
        .map(midnight);
    Some(hit)
}

// ---------------------------------------------------------------------------
// PubMed (1f)
// ---------------------------------------------------------------------------

pub struct PubMed {
    client: reqwest::Client,
}

impl PubMed {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchProvider for PubMed {
    fn name(&self) -> &'static str {
        "pubmed"
    }

    /// Two calls, because `esearch` returns identifiers and nothing else: the
    /// title and the DOI are only in `esummary`.
    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError> {
        let retmax = page_size(max_results);
        let search = self
            .client
            .get("https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi")
            .query(&[
                ("db", "pubmed"),
                ("term", q),
                ("retmode", "json"),
                ("retmax", retmax.as_str()),
                ("sort", "relevance"),
            ])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("pubmed: {error}")))?;
        let found = json_body(search, "pubmed").await?;

        let ids: Vec<&str> = found
            .get("esearchresult")
            .and_then(|result| result.get("idlist"))
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids = ids.join(",");
        let summary = self
            .client
            .get("https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi")
            .query(&[("db", "pubmed"), ("retmode", "json"), ("id", ids.as_str())])
            .send()
            .await
            .map_err(|error| SearchError::Provider(format!("pubmed: {error}")))?;
        let value = json_body(summary, "pubmed").await?;
        Ok(parse_pubmed(&value))
    }
}

/// `esummary` puts the documents in an object keyed by uid and lists their order
/// separately in `uids`, so the array is walked and the object only indexed.
pub fn parse_pubmed(value: &serde_json::Value) -> Vec<SearchHit> {
    let Some(result) = value.get("result") else {
        return Vec::new();
    };
    let Some(uids) = result.get("uids").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    uids.iter()
        .filter_map(|uid| uid.as_str())
        .filter_map(|uid| result.get(uid))
        .filter_map(pubmed_hit)
        .collect()
}

fn pubmed_hit(doc: &serde_json::Value) -> Option<SearchHit> {
    let uid = doc.get("uid").and_then(|v| v.as_str())?;
    let title = doc.get("title").and_then(|v| v.as_str())?;
    let url = format!("https://pubmed.ncbi.nlm.nih.gov/{uid}/");
    // The journal is the only descriptive text in the summary response.
    let snippet = doc
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let mut hit = SearchHit::new(title, url, snippet, SourceKind::Academic, "pubmed");
    hit.doi = pubmed_doi(doc);
    // `sortpubdate` is `2024/01/02 00:00` and is the only field that always
    // carries a day, unlike `pubdate`, which is `2024 Jan` for most records.
    hit.published_at = doc
        .get("sortpubdate")
        .and_then(|v| v.as_str())
        .and_then(parse_date)
        .or_else(|| {
            doc.get("pubdate")
                .and_then(|v| v.as_str())
                .and_then(parse_date)
        });
    Some(hit)
}

/// `articleids` is the documented place for the DOI. `elocationid` carries it as
/// `doi: 10.xxxx` on the records that predate that field.
fn pubmed_doi(doc: &serde_json::Value) -> Option<String> {
    doc.get("articleids")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|identifier| identifier.get("idtype").and_then(|v| v.as_str()) == Some("doi"))
        .and_then(|identifier| identifier.get("idval"))
        .and_then(|v| v.as_str())
        .map(doi_bare)
        .or_else(|| {
            doc.get("elocationid")
                .and_then(|v| v.as_str())
                .filter(|value| value.starts_with("doi:"))
                .map(doi_bare)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn academic(hits: &[SearchHit]) -> bool {
        hits.iter()
            .all(|hit| hit.source_kind == SourceKind::Academic)
    }

    #[test]
    fn openalex_rebuilds_the_abstract_and_keeps_the_doi() {
        let body = serde_json::json!({
            "results": [{
                "id": "https://openalex.org/W1",
                "doi": "https://doi.org/10.1234/abc",
                "title": "A study of things",
                "display_name": "A study of things",
                "publication_date": "2021-05-06",
                "primary_location": {
                    "landing_page_url": "https://example.org/paper",
                    "license": "cc-by"
                },
                "abstract_inverted_index": {
                    "We": [0], "measured": [1], "things.": [2]
                },
                "relevance_score": 12.5
            }]
        });

        let hits = parse_openalex(&body);
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.title, "A study of things");
        assert_eq!(hit.url, "https://example.org/paper");
        assert_eq!(hit.snippet, "We measured things.");
        assert_eq!(hit.doi.as_deref(), Some("10.1234/abc"));
        assert_eq!(
            hit.published_at,
            Some(Utc.with_ymd_and_hms(2021, 5, 6, 0, 0, 0).unwrap())
        );
        assert_eq!(hit.license.as_deref(), Some("cc-by"));
        assert_eq!(hit.score, Some(12.5));
        assert_eq!(hit.provider, "openalex");
        assert!(academic(&hits));
    }

    #[test]
    fn openalex_falls_back_to_the_doi_url_and_an_absent_abstract() {
        let body = serde_json::json!({
            "results": [{
                "id": "https://openalex.org/W2",
                "doi": "https://doi.org/10.1234/def",
                "display_name": "Only the old field name",
                "primary_location": null,
                "abstract_inverted_index": null
            }]
        });

        let hits = parse_openalex(&body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Only the old field name");
        assert_eq!(hits[0].url, "https://doi.org/10.1234/def");
        assert_eq!(hits[0].snippet, "");
        assert_eq!(hits[0].published_at, None);
    }

    #[test]
    fn crossref_unwraps_the_message_and_reads_the_first_title() {
        let body = serde_json::json!({
            "message": {
                "items": [{
                    "DOI": "10.5555/xyz",
                    "title": ["A title with markup &amp; more"],
                    "URL": "https://doi.org/10.5555/xyz",
                    "container-title": ["Journal of Tests"],
                    "abstract": "<jats:p>The effect was real (p &lt; 0.05).</jats:p>",
                    "published": { "date-parts": [[2019, 7, 8]] },
                    "license": [{ "URL": "https://creativecommons.org/licenses/by/4.0/" }],
                    "score": 9.75
                }]
            }
        });

        let hits = parse_crossref(&body);
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.title, "A title with markup &amp; more");
        assert_eq!(hit.doi.as_deref(), Some("10.5555/xyz"));
        assert_eq!(hit.snippet, "The effect was real (p < 0.05).");
        assert_eq!(
            hit.published_at,
            Some(Utc.with_ymd_and_hms(2019, 7, 8, 0, 0, 0).unwrap())
        );
        assert_eq!(hit.score, Some(9.75));
        assert!(academic(&hits));
    }

    #[test]
    fn crossref_falls_back_to_the_journal_and_the_issued_date() {
        let body = serde_json::json!({
            "message": {
                "items": [{
                    "DOI": "10.5555/abc",
                    "title": ["No abstract"],
                    "container-title": ["Journal of Tests"],
                    "issued": { "date-parts": [[2018, 2]] }
                }]
            }
        });

        let hits = parse_crossref(&body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://doi.org/10.5555/abc");
        assert_eq!(hits[0].snippet, "Journal of Tests");
        assert_eq!(
            hits[0].published_at,
            Some(Utc.with_ymd_and_hms(2018, 2, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(hits[0].license, None);
    }

    #[test]
    fn arxiv_reads_entries_and_not_the_feed_header() {
        let feed = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>ArXiv Query: search_query=all:electron</title>
  <id>http://arxiv.org/api/query?search_query=all:electron</id>
  <entry>
    <id>http://arxiv.org/abs/2401.00001v1</id>
    <updated>2024-01-02T03:04:05Z</updated>
    <published>2024-01-02T03:04:05Z</published>
    <title>A long
  wrapped title</title>
    <summary>  An abstract
  that wraps.  </summary>
    <link href="http://arxiv.org/abs/2401.00001v1" rel="alternate" type="text/html"/>
    <link href="http://arxiv.org/pdf/2401.00001v1" rel="related" type="application/pdf" title="pdf"/>
    <arxiv:doi xmlns:arxiv="http://arxiv.org/schemas/atom">10.9999/arxiv.1</arxiv:doi>
  </entry>
</feed>"#;

        let hits = parse_arxiv(feed);
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.title, "A long wrapped title");
        assert_eq!(hit.url, "http://arxiv.org/abs/2401.00001v1");
        assert_eq!(hit.snippet, "An abstract that wraps.");
        assert_eq!(hit.doi.as_deref(), Some("10.9999/arxiv.1"));
        assert_eq!(
            hit.published_at,
            Some(Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap())
        );
        assert_eq!(hit.provider, "arxiv");
        assert!(academic(&hits));
    }

    #[test]
    fn arxiv_without_a_journal_doi_has_none() {
        let feed = r#"<feed><entry>
  <id>http://arxiv.org/abs/2401.00002v2</id>
  <published>2024-02-03T00:00:00Z</published>
  <title>Preprint</title>
  <summary>Text.</summary>
  <link href="http://arxiv.org/abs/2401.00002v2" rel="alternate"/>
</entry></feed>"#;

        let hits = parse_arxiv(feed);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doi, None);
        assert_eq!(hits[0].license, None);
    }

    #[test]
    fn doaj_reads_the_typed_identifier_and_link_lists() {
        let body = serde_json::json!({
            "total": 1,
            "results": [{
                "id": "abc123",
                "bibjson": {
                    "title": "Open access findings",
                    "abstract": "An abstract.",
                    "year": "2020",
                    "month": "6",
                    "identifier": [
                        { "type": "pissn", "id": "1234-5678" },
                        { "type": "doi", "id": "10.7777/doaj.1" }
                    ],
                    "link": [
                        { "type": "fulltext", "url": "https://example.org/full" }
                    ],
                    "license": [{ "type": "CC BY", "url": "https://example.org/licence" }]
                }
            }]
        });

        let hits = parse_doaj(&body);
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.title, "Open access findings");
        assert_eq!(hit.url, "https://example.org/full");
        assert_eq!(hit.doi.as_deref(), Some("10.7777/doaj.1"));
        assert_eq!(hit.license.as_deref(), Some("CC BY"));
        assert_eq!(
            hit.published_at,
            Some(Utc.with_ymd_and_hms(2020, 6, 1, 0, 0, 0).unwrap())
        );
        assert!(academic(&hits));
    }

    #[test]
    fn eric_has_no_doi_and_falls_back_to_its_own_record_page() {
        let body = serde_json::json!({
            "response": {
                "numFound": 1,
                "docs": [{
                    "id": "EJ1234567",
                    "title": "A study of education",
                    "description": "An abstract.",
                    "publicationdateyear": 2017
                }]
            }
        });

        let hits = parse_eric(&body);
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.url, "https://eric.ed.gov/?id=EJ1234567");
        assert_eq!(hit.doi, None);
        assert_eq!(hit.snippet, "An abstract.");
        assert_eq!(
            hit.published_at,
            Some(Utc.with_ymd_and_hms(2017, 1, 1, 0, 0, 0).unwrap())
        );
        assert!(academic(&hits));
    }

    #[test]
    fn pubmed_walks_the_uid_order_and_reads_the_doi_from_articleids() {
        let body = serde_json::json!({
            "result": {
                "uids": ["30000001", "30000002"],
                "30000001": {
                    "uid": "30000001",
                    "title": "First paper",
                    "source": "Nature",
                    "sortpubdate": "2022/03/04 00:00",
                    "articleids": [
                        { "idtype": "pubmed", "idval": "30000001" },
                        { "idtype": "doi", "idval": "10.1038/first" }
                    ]
                },
                "30000002": {
                    "uid": "30000002",
                    "title": "Second paper",
                    "source": "Science",
                    "pubdate": "2021 Jan",
                    "elocationid": "doi: 10.1126/second"
                }
            }
        });

        let hits = parse_pubmed(&body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "First paper");
        assert_eq!(hits[0].url, "https://pubmed.ncbi.nlm.nih.gov/30000001/");
        assert_eq!(hits[0].doi.as_deref(), Some("10.1038/first"));
        assert_eq!(hits[0].snippet, "Nature");
        assert_eq!(
            hits[0].published_at,
            Some(Utc.with_ymd_and_hms(2022, 3, 4, 0, 0, 0).unwrap())
        );
        assert_eq!(hits[1].doi.as_deref(), Some("10.1126/second"));
        // `pubdate` is `2021 Jan`, which is not a date any parser can keep a day
        // for, so the year is the most that can be said.
        assert_eq!(hits[1].published_at, None);
        assert!(academic(&hits));
    }

    #[test]
    fn an_empty_or_shapeless_body_yields_no_hits_rather_than_a_panic() {
        for body in [
            serde_json::json!({}),
            serde_json::json!({ "results": null }),
            serde_json::json!({ "results": [] }),
        ] {
            assert!(parse_openalex(&body).is_empty());
            assert!(parse_crossref(&body).is_empty());
            assert!(parse_doaj(&body).is_empty());
            assert!(parse_eric(&body).is_empty());
            assert!(parse_pubmed(&body).is_empty());
        }
        assert!(parse_arxiv("<feed></feed>").is_empty());
    }
}
