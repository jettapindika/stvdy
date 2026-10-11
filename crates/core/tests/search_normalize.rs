//! URL and query normalisation. These are the properties the dedupe and the A40
//! cache key rest on: two links to one document must be one key, and two spellings
//! of one question must be one cache entry.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::search::normalize::{dedupe_key, normalize_query, normalize_url};

#[test]
fn a_tracked_url_and_a_clean_one_normalise_to_the_same_string() {
    let tracked = normalize_url("HTTP://Example.COM/a/?utm_source=x#frag");
    let clean = normalize_url("https://example.com/a/");

    assert_eq!(tracked.as_deref(), Some("https://example.com/a"));
    assert_eq!(tracked, clean);
}

#[test]
fn the_scheme_and_host_are_lowercased_but_the_path_is_not() {
    // Paths are case-sensitive on most servers, so `/Page` and `/page` are two
    // resources and must not collapse.
    assert_eq!(
        normalize_url("https://E.COM/Page").as_deref(),
        Some("https://e.com/Page")
    );
}

#[test]
fn every_tracking_parameter_is_stripped_and_a_meaningful_one_survives() {
    assert_eq!(
        normalize_url("https://e.com/s?q=rust&utm_source=x").as_deref(),
        Some("https://e.com/s?q=rust")
    );

    let tags = [
        "utm_source=x",
        "utm_medium=x",
        "utm_campaign=x",
        "utm_content=x",
        "utm_term=x",
        "fbclid=x",
        "gclid=x",
        "mc_cid=x",
        "mc_eid=x",
        "ref=x",
        "source=x",
    ];
    for tag in tags {
        let raw = format!("https://e.com/p?{tag}");
        assert_eq!(
            normalize_url(&raw).as_deref(),
            Some("https://e.com/p"),
            "{tag} must be stripped"
        );
    }
}

#[test]
fn the_fragment_and_a_trailing_slash_are_dropped_but_the_root_survives() {
    assert_eq!(
        normalize_url("https://e.com/a#section").as_deref(),
        Some("https://e.com/a")
    );
    assert_eq!(
        normalize_url("https://e.com/a/").as_deref(),
        Some("https://e.com/a")
    );
    assert_eq!(
        normalize_url("https://e.com/").as_deref(),
        Some("https://e.com/")
    );
    // No path at all is the root, so it keys the same as the root with a slash.
    assert_eq!(
        normalize_url("https://e.com").as_deref(),
        Some("https://e.com/")
    );
}

#[test]
fn a_non_http_url_is_none() {
    for raw in [
        "javascript:alert(1)",
        "mailto:a@b.c",
        "data:text/html,x",
        "",
    ] {
        assert_eq!(normalize_url(raw), None, "{raw} must not normalise");
    }
}

#[test]
fn normalize_query_collapses_whitespace_and_lowercases() {
    assert_eq!(normalize_query("  What \t Is\nX?  "), "what is x?");
}

#[test]
fn the_cache_key_is_stable_for_equivalent_queries() {
    assert_eq!(
        dedupe_key("What  Is X?", "searxng", 8),
        dedupe_key("what is x?", "searxng", 8)
    );
}

#[test]
fn the_cache_key_separates_provider_and_max_results() {
    let base = dedupe_key("what is x", "searxng", 8);

    assert_ne!(base, dedupe_key("what is x", "exa", 8));
    assert_ne!(base, dedupe_key("what is x", "searxng", 5));
}
