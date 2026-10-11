//! The search tiers and the composite that walks them (A3).
//!
//! Each tier is its own module and is skipped when unconfigured, so a deployment
//! without a key for one does not fail at the first query with a 401.
//!
//! The helpers here are the parts every tier needs and none of them owns: the
//! HTTP client shape, the status-code split, and the date and string reading that
//! the scholarly APIs make necessary (they publish dates in four formats and
//! abstracts as markup or as an inverted index).

pub mod academic;
pub mod composite;
pub mod exa;
pub mod searxng;
pub mod tavily;

pub use composite::{CompositeSearch, SearchOutcome};

use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use stvdy_core::traits::search::SearchError;

/// OpenAlex and Crossref both ask callers to identify themselves rather than send
/// a library default, and rate-limit the ones that do not.
pub(crate) const USER_AGENT: &str = "Stvdy/0.1 (+https://stvdy.app)";

/// One client shape for every tier: a timeout, because a slow provider must not
/// hold a chat turn open when the composite can move to the next tier, and the
/// app's user agent.
pub(crate) fn http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| anyhow::anyhow!("building the search http client: {error}"))
}

/// A response body as JSON, with a 4xx and a 5xx told apart: the first means our
/// request is wrong (a bad key, a bad parameter), the second means the tier is.
/// The composite treats both the same way and moves on, so the distinction exists
/// for the admin report rather than for control flow.
pub(crate) async fn json_body(
    response: reqwest::Response,
    tier: &str,
) -> Result<serde_json::Value, SearchError> {
    let status = response.status();
    if !status.is_success() {
        let detail: String = response
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        return Err(if status.is_client_error() {
            SearchError::Invalid(format!("{tier} answered {status}: {detail}"))
        } else {
            SearchError::Provider(format!("{tier} answered {status}: {detail}"))
        });
    }
    response
        .json()
        .await
        .map_err(|error| SearchError::Provider(format!("{tier}: decoding the response: {error}")))
}

/// Midnight UTC on `date`. The APIs give a publication *date*, not an instant, so
/// the time of day is not information that exists rather than something dropped.
pub(crate) fn midnight(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
}

/// The four date shapes the tiers actually send: RFC 3339, `2024-01-02`,
/// `2024-01` and a bare year. A partial date becomes the first of its period
/// rather than being dropped, because a year is still worth ranking on.
pub(crate) fn parse_date(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(date) = NaiveDate::parse_from_str(raw, format) {
            return Some(midnight(date));
        }
    }
    // `2024-01`, `2024/01` and `2024`. Splitting rather than a `%Y-%m` format
    // because chrono refuses to parse a date without a day.
    let mut parts = raw.split(['-', '/']);
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts
        .next()
        .and_then(|month| month.parse::<u32>().ok())
        .unwrap_or(1);
    NaiveDate::from_ymd_opt(year, month, 1).map(midnight)
}

/// A JSON integer that may have been serialized as a string, which DOAJ and ERIC
/// both do for years.
pub(crate) fn as_int(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
}

/// A DOI stored bare, without the resolver prefix.
///
/// OpenAlex sends `https://doi.org/10.x` and Crossref sends `10.x` for the same
/// paper, and `rank` dedupes on the DOI, so a prefix left in place would defeat
/// the dedupe it exists for.
pub(crate) fn doi_bare(raw: &str) -> String {
    let trimmed = raw.trim();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Whitespace collapsed to single spaces. arXiv wraps titles and summaries across
/// lines, so an unfolded snippet would carry newlines into a prompt.
pub(crate) fn collapse_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Markup removed, entities decoded.
///
/// Crossref abstracts are JATS XML, which is noise in a prompt. `<` is only a tag
/// opener when a letter, `/`, `!` or `?` follows, so an abstract writing
/// `p < 0.05` keeps its text instead of losing it to the next real tag.
pub(crate) fn strip_markup(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        let is_tag = ch == '<'
            && chars.peek().is_some_and(|next| {
                next.is_ascii_alphabetic() || *next == '/' || *next == '!' || *next == '?'
            });
        if is_tag {
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    unescape_xml(&out)
}

/// XML entities decoded, for the same reason as the tags: `&amp;` and `&lt;` in a
/// prompt are noise a reader has to decode by eye.
pub(crate) fn unescape_xml(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(ampersand) = rest.find('&') {
        out.push_str(&rest[..ampersand]);
        let tail = &rest[ampersand..];
        // An entity name is short: a `;` further away than this is prose.
        let end = tail
            .char_indices()
            .take(12)
            .find(|(_, ch)| *ch == ';')
            .map(|(index, _)| index);
        match end.and_then(|end| decode_entity(&tail[1..end])) {
            Some(decoded) => {
                out.push(decoded);
                rest = &tail[end.unwrap_or(0) + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "nbsp" => return Some(' '),
        _ => {}
    }
    let number = entity.strip_prefix('#')?;
    match number.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok().and_then(char::from_u32),
        None => number.parse::<u32>().ok().and_then(char::from_u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_doi_loses_its_resolver_prefix() {
        assert_eq!(doi_bare("https://doi.org/10.1234/AbC"), "10.1234/AbC");
        assert_eq!(doi_bare("doi: 10.1234/AbC"), "10.1234/AbC");
        assert_eq!(doi_bare("10.1234/AbC"), "10.1234/AbC");
    }

    #[test]
    fn the_four_date_shapes_all_parse() {
        assert_eq!(
            parse_date("2024-03-04T05:06:07Z"),
            Some(Utc.with_ymd_and_hms(2024, 3, 4, 5, 6, 7).unwrap())
        );
        assert_eq!(
            parse_date("2024-03-04"),
            Some(Utc.with_ymd_and_hms(2024, 3, 4, 0, 0, 0).unwrap())
        );
        assert_eq!(
            parse_date("2024-03"),
            Some(Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            parse_date("2024"),
            Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(parse_date("last Tuesday"), None);
    }

    #[test]
    fn markup_is_removed_but_a_comparison_operator_survives() {
        assert_eq!(
            strip_markup("<jats:p>Effect was real (p < 0.05).</jats:p>"),
            "Effect was real (p < 0.05)."
        );
        assert_eq!(
            strip_markup("Smith &amp; Jones &lt;b&gt;"),
            "Smith & Jones <b>"
        );
        assert_eq!(unescape_xml("A &#8212; B"), "A \u{2014} B");
    }

    #[test]
    fn an_unterminated_ampersand_is_text() {
        assert_eq!(unescape_xml("Q&A and R&D"), "Q&A and R&D");
        assert_eq!(
            unescape_xml("100% &notanentity; ok"),
            "100% &notanentity; ok"
        );
    }

    #[test]
    fn a_year_serialized_as_a_string_is_still_a_year() {
        assert_eq!(as_int(&serde_json::json!("2024")), Some(2024));
        assert_eq!(as_int(&serde_json::json!(2024)), Some(2024));
        assert_eq!(as_int(&serde_json::json!("")), None);
    }
}
