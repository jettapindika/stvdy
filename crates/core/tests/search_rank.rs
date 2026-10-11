//! Ranking and dedupe. These are the properties the composite's output order rests
//! on: one paper is one result, a dated hit beats an undated one, and the same
//! input twice is the same bytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeZone, Utc};

use stvdy_core::search::rank::rank;
use stvdy_core::traits::search::{SearchHit, SourceKind};

fn date(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
}

fn hit(title: &str, url: &str, kind: SourceKind) -> SearchHit {
    SearchHit::new(title, url, "snippet", kind, "tier")
}

#[test]
fn an_academic_hit_outranks_a_web_hit() {
    let web = hit("w", "https://e.com/w", SourceKind::Web);
    let academic = hit("a", "https://e.com/a", SourceKind::Academic);

    let ranked = rank(vec![web, academic]);

    assert_eq!(ranked[0].source_kind, SourceKind::Academic);
    assert_eq!(ranked[1].source_kind, SourceKind::Web);
}

#[test]
fn a_hit_with_a_doi_outranks_one_without() {
    let mut with_doi = hit("d", "https://e.com/d", SourceKind::Web);
    with_doi.doi = Some("10.1000/x".to_string());
    let without_doi = hit("n", "https://e.com/n", SourceKind::Web);

    let ranked = rank(vec![without_doi, with_doi]);

    assert!(ranked[0].doi.is_some());
}

#[test]
fn a_doi_duplicate_collapses_to_the_academic_copy() {
    // The same paper at a publisher and at a repository. The DOI comparison must
    // ignore case and the `https://doi.org/` resolver prefix.
    let mut web = hit("short", "https://publisher.example/paper", SourceKind::Web);
    web.doi = Some("10.1000/ABC".to_string());

    let mut academic = hit(
        "A much fuller title",
        "https://repository.example/paper",
        SourceKind::Academic,
    );
    academic.doi = Some("https://doi.org/10.1000/abc".to_string());

    let ranked = rank(vec![web, academic]);

    assert_eq!(ranked.len(), 1, "one work must yield one result");
    assert_eq!(ranked[0].source_kind, SourceKind::Academic);
    assert!(ranked[0].doi.is_some(), "the survivor must keep the DOI");
    assert_eq!(ranked[0].title, "A much fuller title");
}

#[test]
fn a_url_duplicate_collapses_to_one_entry() {
    let tracked = hit("a", "https://e.com/a?utm_source=x", SourceKind::Web);
    let clean = hit("a", "https://e.com/a", SourceKind::Web);

    assert_eq!(rank(vec![tracked, clean]).len(), 1);
}

#[test]
fn a_dated_hit_sorts_above_an_undated_one_and_none_sorts_last() {
    let undated = hit("undated", "https://e.com/u", SourceKind::Web);
    let mut dated = hit("dated", "https://e.com/d", SourceKind::Web);
    dated.published_at = Some(date(2024, 1, 1));

    // The undated hit is first in the input, so a naive sort that treats `None` as
    // the minimum would leave it first and this assertion would fail.
    let ranked = rank(vec![undated, dated]);

    assert_eq!(ranked[0].title, "dated");
    assert_eq!(ranked[1].title, "undated");
    assert!(
        ranked[1].published_at.is_none(),
        "None must sort last, not first"
    );
}

#[test]
fn the_same_input_twice_gives_byte_identical_output() {
    let input = || {
        vec![
            hit("b", "https://e.com/b", SourceKind::Web),
            hit("a", "https://e.com/a", SourceKind::Academic),
            hit("c", "https://e.com/c?utm_source=x", SourceKind::Web),
        ]
    };

    let first = serde_json::to_string(&rank(input())).unwrap();
    let second = serde_json::to_string(&rank(input())).unwrap();

    assert_eq!(first, second);
}

#[test]
fn hits_equal_in_every_ranked_field_come_out_in_url_order() {
    let a = hit("t", "https://e.com/a", SourceKind::Web);
    let b = hit("t", "https://e.com/b", SourceKind::Web);

    // Same tier, same kind, no DOI, no date, no score: only the URL is left, and it
    // must decide so the order is total.
    let ranked = rank(vec![b, a]);

    assert_eq!(ranked[0].url, "https://e.com/a");
    assert_eq!(ranked[1].url, "https://e.com/b");
}

#[test]
fn an_earlier_tier_outranks_a_later_one() {
    let mut first = hit("first", "https://e.com/1", SourceKind::Web);
    first.provider = "openalex".to_string();
    let mut second = hit("second", "https://e.com/2", SourceKind::Web);
    second.provider = "tavily".to_string();

    let ranked = rank(vec![first, second]);

    assert_eq!(ranked[0].provider, "openalex");
}

#[test]
fn a_hit_with_an_unparseable_url_is_dropped_and_the_rest_still_rank() {
    let bad = hit("bad", "javascript:alert(1)", SourceKind::Academic);
    let good = hit("good", "https://e.com/g", SourceKind::Web);

    let ranked = rank(vec![bad, good]);

    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].title, "good");
}

#[test]
fn an_empty_input_is_an_empty_output() {
    assert!(rank(Vec::new()).is_empty());
}
