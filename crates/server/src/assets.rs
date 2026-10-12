//! The print assets, embedded in the binary and served from the server's own origin.
//!
//! The cheatsheet template needs KaTeX and two typefaces. Until Phase 15 it loaded all
//! three from third-party origins, which made a printed document depend on a CDN being
//! reachable — and the failure was not an error but a plausible-looking PDF full of literal
//! `$…$` (A339, `docs/RELEASE.md`). `render::chrome::rewrite_asset_base` points the
//! template's references here before the page reaches Chromium; this module is what answers
//! them.
//!
//! **Embedded, not read from disk.** `rust-embed` compiles `assets/` into the binary, so a
//! file missing from the repository is a build failure rather than a 404 at print time. The
//! image also carries a copy at `ASSETS_DIR` for the operator, and that copy is redundant
//! on purpose: a release engineer needs to see what shipped.
//!
//! **Public, and that is the correct class.** The browser that requests these is headless
//! Chromium rendering a document with no credentials, and every file here is a vendored
//! upstream release with no user data in it.

use axum::Router;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::state::AppState;

/// The vendored assets. The path is relative to this crate's manifest directory, so it
/// resolves to the repository's `assets/`.
#[derive(rust_embed::Embed)]
#[folder = "../../assets"]
struct PrintAssets;

/// A year, because the URL names a version and the bytes behind a given path never change.
///
/// `immutable` is what stops a browser revalidating on every print: the request comes from
/// headless Chromium with a fresh profile each launch, so the cache is not doing much work
/// here — but a *browser* print of the same document is the other consumer, and there a
/// revalidation per asset per print is the difference between instant and a second.
const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The route table.
///
/// The path is a literal rather than a constant, because `crates/server/tests/route_sweep.rs`
/// reads the argument text of every `.route(…)` call to check that each registered route is
/// classified, and it requires a string literal — a `const` would make the extractor panic
/// and the sweep would stop covering anything. The path is asserted in three places anyway:
/// the sweep's `Exempt` row, the assets suite, and the PDF acceptance test that serves the
/// handler on its own loopback port.
pub fn routes() -> Router<AppState> {
    Router::new().route("/assets/{*path}", get(serve))
}

/// Serves one embedded asset.
///
/// The content type is decided by the extension and an unknown one is a 404 rather than a
/// guess: `checksums.txt` and `README.md` live in the same directory, and neither belongs on
/// the HTTP surface. A wrong content type is the failure worth naming — a browser refuses a
/// stylesheet served as `application/octet-stream`, and the symptom is a document in a
/// fallback face with nothing in any log.
pub async fn serve(Path(path): Path<String>) -> Response {
    let Some(content_type) = content_type(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(asset) = PrintAssets::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        asset.data,
    )
        .into_response()
}

/// The content type for a served path, or `None` for anything not in the served set.
///
/// An explicit match rather than `mime_guess`: the three extensions here are the whole
/// surface, and a crate that answered `application/octet-stream` for an extension it did not
/// know would turn "we do not serve this" into "we served it wrongly".
fn content_type(path: &str) -> Option<&'static str> {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("css") => Some("text/css; charset=utf-8"),
        // The template loads both as `defer`red classic scripts, and `text/javascript` is
        // what a browser accepts for that under any CSP.
        Some("js") => Some("text/javascript; charset=utf-8"),
        Some("woff2") => Some("font/woff2"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_content_type_is_decided_by_the_extension() {
        assert_eq!(
            content_type("katex/katex.min.css"),
            Some("text/css; charset=utf-8")
        );
        assert_eq!(content_type("fonts.css"), Some("text/css; charset=utf-8"));
        assert_eq!(
            content_type("katex/contrib/auto-render.min.js"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            content_type("katex/fonts/KaTeX_Main-Regular.woff2"),
            Some("font/woff2")
        );
    }

    /// The two files that live in `assets/` but are not part of the served set.
    #[test]
    fn the_release_records_are_not_served() {
        assert_eq!(content_type("checksums.txt"), None);
        assert_eq!(content_type("README.md"), None);
        assert_eq!(content_type("nope"), None);
    }
}
