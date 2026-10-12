//! The SSRF guard, per range.
//!
//! Ingestion accepts an arbitrary URL from a user, so an unguarded fetch is a
//! primitive for reading the server's own network and its cloud metadata endpoint
//! (A18/A34). This suite exists because the guard is the kind of code that looks
//! obviously right and is wrong in one range nobody tested: the classic misses are
//! carrier-grade NAT, an IPv4-mapped IPv6 address, and the metadata address being
//! treated as "just a link-local address we never see".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::IpAddr;

use stvdy_server::integrations::fetch::{is_public_ip, parse_target};

fn ip(raw: &str) -> IpAddr {
    raw.parse()
        .unwrap_or_else(|_| panic!("{raw} must parse as an IP"))
}

/// Every address family and range the guard must refuse.
#[test]
fn private_and_reserved_ranges_are_refused() {
    let refused = [
        // Loopback, in both spellings.
        "127.0.0.1",
        "127.1.2.3",
        "::1",
        // RFC 1918.
        "10.0.0.1",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        // Link-local, including the cloud metadata endpoint. This is the one that
        // turns an unguarded fetch into a credential theft.
        "169.254.169.254",
        "169.254.0.1",
        "fe80::1",
        // Carrier-grade NAT: the range `is_private()` does not cover.
        "100.64.0.1",
        "100.127.255.255",
        // IETF protocol assignments and the documentation ranges.
        "192.0.0.1",
        "192.0.2.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        // Reserved.
        "240.0.0.1",
        "255.255.255.255",
        // Unspecified and multicast.
        "0.0.0.0",
        "224.0.0.1",
        "::",
        "ff02::1",
        // Unique local.
        "fc00::1",
        "fd00:1234::1",
        // An IPv4-mapped address reaches an IPv4 target, so it is judged as one.
        // Missing this is the classic way a v6 check is bypassed.
        "::ffff:169.254.169.254",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        // 6to4 wraps an IPv4 address, here a private one.
        "2002:0a00:0001::1",
    ];

    for raw in refused {
        assert!(
            !is_public_ip(ip(raw)),
            "{raw} must be refused: fetching it is an SSRF primitive"
        );
    }
}

/// And the guard must not refuse the internet.
#[test]
fn public_addresses_are_allowed() {
    let allowed = [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "172.32.0.1",     // just outside 172.16/12
        "100.63.255.255", // just below 100.64/10
        "100.128.0.1",    // just above it
        "2606:4700:4700::1111",
        "2002:0808:0808::1", // 6to4 wrapping 8.8.8.8
    ];

    for raw in allowed {
        assert!(is_public_ip(ip(raw)), "{raw} is a public address");
    }
}

/// Only http and https, and nothing that carries credentials to whatever answers.
#[test]
fn non_http_schemes_and_embedded_credentials_are_refused() {
    for raw in [
        "javascript:alert(1)",
        "mailto:a@b.example",
        "data:text/html,<h1>x</h1>",
        "ftp://example.com/file",
        "file:///etc/passwd",
        "gopher://example.com",
    ] {
        assert!(
            parse_target(raw).is_err(),
            "{raw} must be refused before any fetch is attempted"
        );
    }

    // Credentials in the URL would be sent to whatever the host resolves to.
    assert!(
        parse_target("https://user:pass@example.com/").is_err(),
        "a URL with embedded credentials must be refused"
    );
    assert!(parse_target("https://user@example.com/").is_err());

    // A truly empty authority cannot be fetched. Note `https:///path` is NOT this
    // case: the url crate folds it to host `path`, which then fails at DNS like any
    // other unknown name, so it needs no special handling here.
    assert!(parse_target("https://").is_err());
    assert!(parse_target("http://").is_err());
    assert!(parse_target("not a url").is_err());
    assert!(parse_target("").is_err());

    // The ordinary cases pass.
    assert!(parse_target("https://example.com/paper").is_ok());
    assert!(parse_target("http://example.com:8080/x?y=1#z").is_ok());
}

/// The chain must stop on a refusal rather than handing the URL to the next
/// fetcher, which is what would turn one fetcher's policy into a suggestion.
#[tokio::test]
async fn the_chain_refuses_a_scheme_it_will_not_fetch() {
    use stvdy_core::traits::search::{FetchError, PageFetcher};
    use stvdy_server::integrations::fetch::ChainFetcher;

    // An empty chain still refuses, because the scheme check happens before the
    // walk. That ordering is the point: a refusal is not something a later
    // fetcher can fix.
    let chain = ChainFetcher::new(Vec::new());
    let error = chain
        .fetch("javascript:alert(1)")
        .await
        .expect_err("a javascript URL must be refused");

    match error {
        FetchError::Refused(reason) => assert!(
            reason.contains("http"),
            "the refusal must say what is allowed: {reason}"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A chain with nothing in it reports that, rather than a refusal, for a URL it
/// would otherwise have tried.
#[tokio::test]
async fn an_empty_chain_reports_that_it_has_nothing_to_try() {
    use stvdy_core::traits::search::{FetchError, PageFetcher};
    use stvdy_server::integrations::fetch::ChainFetcher;

    let chain = ChainFetcher::new(Vec::new());
    let error = chain
        .fetch("https://example.com/")
        .await
        .expect_err("nothing to try");

    match error {
        FetchError::Provider(reason) => assert!(
            reason.contains("no page fetcher"),
            "the message must say why: {reason}"
        ),
        other => panic!("expected a provider failure, got {other:?}"),
    }
}

/// A literal private address in the URL is refused by the resolver, before any
/// connection, which is the case that needs no DNS to be dangerous.
#[tokio::test]
async fn a_literal_private_host_is_refused_by_the_resolver() {
    use stvdy_server::integrations::fetch::resolve_public;

    for raw in ["127.0.0.1", "169.254.169.254", "10.1.2.3", "::1", "fd00::1"] {
        let error = resolve_public(raw, 80)
            .await
            .expect_err("a private literal must be refused");
        assert!(
            error.to_string().contains("public address"),
            "{raw}: {error}"
        );
    }

    // A public literal resolves to itself.
    let resolved = resolve_public("93.184.216.34", 443)
        .await
        .expect("a public literal is allowed");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].port(), 443);
}

/// A search hit whose URL cannot be opened must never become a citation.
///
/// The citation path does not go through the fetcher: a hit's URL is written into
/// `Citation::Web` and rendered as a link the user clicks, which the shell then
/// opens in their browser. So a hostile or compromised search tier could return the
/// metadata endpoint or this server's own loopback address and have it offered as a
/// citable source. Filtering at the tier boundary is what makes the property true
/// for every consumer at once.
#[test]
fn only_a_url_that_could_be_opened_is_citable() {
    use stvdy_server::integrations::fetch::is_citable_url;

    for refused in [
        // The two with real consequences: the cloud metadata endpoint and a service
        // on the user's own machine.
        "http://169.254.169.254/latest/meta-data/",
        "http://127.0.0.1:8099/workspaces",
        "http://localhost/",
        "http://10.0.0.1/admin",
        "http://192.168.1.1/",
        "http://[::1]/",
        "http://[fd00::1]/",
        // Not a URL a browser should be handed from a citation.
        "javascript:alert(1)",
        "data:text/html,<script>x</script>",
        "file:///etc/passwd",
        // Not a URL at all.
        "",
        "not a url",
        // Credentials would be sent to whatever answers.
        "https://user:pass@example.com/",
    ] {
        assert!(
            !is_citable_url(refused),
            "{refused} must not be offered as a citation"
        );
    }

    for allowed in [
        "https://arxiv.org/abs/1706.03762",
        "https://doi.org/10.5281/zenodo.19225200",
        "http://example.com:8080/paper?q=1#frag",
        // A hostname is left to the fetch path, which resolves it; this check is
        // syntactic plus a literal-address test, and that residual is documented.
        "https://a-hostname-that-could-resolve-anywhere.example/paper",
    ] {
        assert!(is_citable_url(allowed), "{allowed} is a citable URL");
    }
}

/// The raw text fetch shares the guard with the page fetcher (A233). A second implementation
/// of the guard is the failure this asserts against, and it is the failure that matters here:
/// the ICS path exists because `ChainFetcher` runs readability, so the temptation is to write
/// a small `reqwest` call beside it — which would be an unguarded fetch of a URL a user typed.
///
/// Every case is asserted **before any network work happens**, so this suite needs no network
/// and no stub: `parse_target` and `resolve_public` refuse these without a socket.
#[tokio::test]
async fn the_guarded_text_fetch_refuses_a_private_host_and_a_non_http_scheme() {
    use stvdy_server::integrations::fetch::fetch_guarded_text;
    // `FetchError` is `stvdy_core`'s, re-exported through the trait module rather than through
    // `integrations::fetch`: it is the trait's error type, so importing it from where it is
    // defined is what keeps the two crates' error contract visible.
    use stvdy_core::traits::search::FetchError;

    for url in [
        "http://127.0.0.1/feed.ics",
        "http://169.254.169.254/feed.ics",
        "http://[::ffff:169.254.169.254]/feed.ics",
        "http://10.0.0.1/feed.ics",
        "http://100.64.0.1/feed.ics",
        "file:///etc/passwd",
        "https://user:pw@example.com/feed.ics",
    ] {
        let refused = fetch_guarded_text(url, "text/calendar", None).await;
        assert!(
            matches!(
                refused,
                Err(FetchError::Refused(_) | FetchError::Invalid(_))
            ),
            "{url} must be refused, got {refused:?}"
        );
    }
}
