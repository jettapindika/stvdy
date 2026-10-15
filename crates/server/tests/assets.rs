//! The print assets: the template's CDN references, and the route that replaces them.
//!
//! The defect this suite exists for is the one the tree had until Phase 15: the template's
//! own comment says the renderer rewrites `ASSET_BASE` to a local mount, and no code did.
//! The markup loaded KaTeX and the typefaces from `cdn.jsdelivr.net` and
//! `fonts.googleapis.com`, so a container with no egress rendered every formula as literal
//! `$…$` — which the `data-math-ready` guard turns into a failed job, i.e. a PDF nobody can
//! download (A339, A340, `docs/RELEASE.md`).
//!
//! The two halves fail differently and are both needed:
//!
//!  * the rewrite test is a pure string assertion, so it runs everywhere and fails the
//!    moment the template gains a fifth remote reference;
//!  * the route test proves the rewritten URL actually resolves to bytes with the content
//!    type the browser needs. A 200 with the wrong MIME type is a stylesheet that does not
//!    apply, which is the same silent failure one step later.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// `#[sqlx::test]` is a macro harness, so clippy.toml's test relaxations are not guaranteed
// to apply to the generated code.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use stvdy_core::render::CHEATSHEET_V1;
use stvdy_server::render::chrome::rewrite_asset_base;
use tower::ServiceExt;

/// A base with a non-default port on purpose: a rewrite that hard-coded 8080 would pass
/// with the default and fail here.
const BASE: &str = "http://127.0.0.1:9123";

fn app(pool: PgPool) -> axum::Router {
    stvdy_server::routes::router(stvdy_server::testing::state(pool)).expect("the router builds")
}

/// Every absolute URL in a document, as written in `src`/`href`/`url(...)` attributes.
///
/// Scanned rather than listed, because the property under test is "no third-party origin
/// survives", and a hand-written list of the four URLs the template carries today would
/// pass while a fifth was added.
fn absolute_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let bytes = html.as_bytes();
    let mut at = 0;
    while let Some(found) = html[at..].find("https://") {
        let start = at + found;
        let end = bytes[start..]
            .iter()
            .position(|byte| {
                matches!(
                    *byte,
                    b'"' | b'\'' | b')' | b' ' | b'\n' | b'\r' | b'\t' | b'>' | b';'
                )
            })
            .map(|offset| start + offset)
            .unwrap_or(bytes.len());
        urls.push(html[start..end].to_string());
        at = end;
    }
    urls
}

/// The rewrite points every CDN reference at the server's own origin, and the four URLs the
/// template carries all land on files that are actually vendored.
#[test]
fn the_template_has_no_remote_asset_reference_after_the_rewrite() {
    let rewritten = rewrite_asset_base(CHEATSHEET_V1, BASE);

    assert!(
        !rewritten.contains("cdn.jsdelivr.net"),
        "a KaTeX reference survived the rewrite"
    );
    assert!(
        !rewritten.contains("fonts.googleapis.com"),
        "the typeface stylesheet reference survived the rewrite"
    );
    assert!(
        !rewritten.contains("fonts.gstatic.com"),
        "the preconnect hint to a third-party font origin survived the rewrite"
    );

    for path in [
        "katex/katex.min.css",
        "katex/katex.min.js",
        "katex/contrib/auto-render.min.js",
        "fonts.css",
    ] {
        assert!(
            rewritten.contains(&format!("{BASE}/assets/{path}")),
            "the rewritten document must reference {path}"
        );
    }
}

/// Every absolute URL in the rewritten document is one the server itself serves.
///
/// This is the assertion that catches a *fifth* remote reference: the template may gain one
/// at any time (a font weight, a new KaTeX plugin), and without this the render would go
/// back to depending on a third-party origin for exactly one asset — the failure that is
/// invisible until a document is printed somewhere with no egress.
#[test]
fn the_rewritten_document_references_only_the_server() {
    let rewritten = rewrite_asset_base(CHEATSHEET_V1, BASE);

    let remote: Vec<_> = absolute_urls(&rewritten)
        .into_iter()
        .filter(|url| !url.starts_with(BASE))
        .collect();

    assert!(
        remote.is_empty(),
        "these references do not point at the server: {remote:?}"
    );

    // And the count is not zero, which is what makes the assertion above non-vacuous: a
    // rewrite that stripped every reference would also pass it.
    let source = absolute_urls(CHEATSHEET_V1);
    assert!(
        source.len() >= 4,
        "the template should still carry its CDN fallback: {source:?}"
    );
}

/// The rewrite is idempotent, so a document that already points at the server is unchanged.
///
/// Worth pinning because the function is applied on every render: a second pass that
/// re-prefixed the base would produce `/assets/http://127.0.0.1:9123/assets/…`.
#[test]
fn the_rewrite_is_idempotent() {
    let once = rewrite_asset_base(CHEATSHEET_V1, BASE);
    let twice = rewrite_asset_base(&once, BASE);

    assert_eq!(once, twice);
}

/// The route serves KaTeX and the fonts, with the content type the browser needs.
///
/// The MIME type is the substance: a browser refuses a stylesheet served as
/// `application/octet-stream` under a strict CSP, and refuses a font served as text — and
/// the failure is a document in a fallback face with no error anywhere.
#[sqlx::test(migrations = "../../migrations")]
async fn the_asset_route_serves_katex_and_the_fonts(pool: PgPool) {
    for (path, content_type, needle) in [
        ("/assets/katex/katex.min.css", "text/css", ".katex"),
        ("/assets/katex/katex.min.js", "javascript", "katex"),
        (
            "/assets/katex/contrib/auto-render.min.js",
            "javascript",
            "renderMathInElement",
        ),
        ("/assets/fonts.css", "text/css", "@font-face"),
    ] {
        let response = app(pool.clone())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK, "{path}");

        let header = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            header.contains(content_type),
            "{path} must be served as {content_type}, not {header}"
        );

        // A version-carrying path, so a stale stylesheet is impossible and the cache can be
        // permanent.
        let cache = response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(cache.contains("immutable"), "{path}: {cache}");

        let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(needle), "{path} served the wrong bytes");
    }
}

/// A woff2 is served as a font, byte for byte.
///
/// The bytes are compared to the vendored file rather than merely being non-empty: a route
/// that served the stylesheet for every path would pass a "did we get bytes" check.
#[sqlx::test(migrations = "../../migrations")]
async fn the_asset_route_serves_a_font_byte_for_byte(pool: PgPool) {
    let response = app(pool)
        .oneshot(
            Request::builder()
                .uri("/assets/fonts/inter-latin-c940764593.woff2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("font/woff2"),
    );

    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let vendored = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts/inter-latin-c940764593.woff2"),
    )
    .expect("the vendored font is in the tree");
    assert_eq!(body.as_ref(), vendored.as_slice());
    assert_eq!(&vendored[..4], b"wOF2", "and it is a woff2");
}

/// A path outside the served set is a 404, and so is a traversal.
///
/// The directory holds `checksums.txt` and `README.md`, which are for the release engineer
/// and not for the HTTP surface; and the traversal case is the one that would matter if the
/// handler ever resolved a path on disk instead of looking it up in the embedded map.
#[sqlx::test(migrations = "../../migrations")]
async fn the_asset_route_refuses_what_it_does_not_serve(pool: PgPool) {
    for path in [
        "/assets/checksums.txt",
        "/assets/README.md",
        "/assets/katex/../checksums.txt",
        "/assets/../../Cargo.toml",
        "/assets/nope.woff2",
    ] {
        let response = app(pool.clone())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
