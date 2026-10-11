//! The SVG allowlist, and the id prefixing that keeps two diagrams apart.
//!
//! Two properties, both of which the template's own comments state as the renderer's
//! contract.
//!
//! **The allowlist is closed.** An element or attribute outside it drops the whole block
//! rather than being stripped: a diagram with its `<script>` removed is not the diagram the
//! model drew, and rendering a mutilated version of it is a quiet lie about the content.
//!
//! **Every `id` and every `url(#…)` is prefixed per block.** Every id inside an inline SVG
//! is document-global, so two diagrams that both define `arrowhead` collide and the second
//! silently reuses the first's marker: the arrow comes out the wrong colour or shape, which
//! reads as a CSS problem rather than an id one. This was one of the five defects the
//! template's first render surfaced.

/// The elements the template's stylesheet is written for, copied from
/// `prompts/cheatsheet.v1.md` so the prompt and the enforcement cannot drift.
pub const ALLOWED_ELEMENTS: &[&str] = &[
    "svg", "g", "rect", "circle", "ellipse", "line", "polyline", "polygon", "path", "text",
    "tspan", "defs", "marker", "title",
];

/// The attributes the template's stylesheet uses, and nothing else.
///
/// `id` and `class` are here because the template styles by class and the renderer
/// prefixes ids; every `on*` attribute, `href`, `style` and `xlink:*` is absent, which is
/// what makes the list a guard rather than a formality.
pub const ALLOWED_ATTRIBUTES: &[&str] = &[
    "viewBox",
    "d",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "width",
    "height",
    "points",
    "fill",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "fill-opacity",
    "stroke-opacity",
    "transform",
    "font-size",
    "font-family",
    "font-weight",
    "text-anchor",
    "dominant-baseline",
    "marker-end",
    "marker-start",
    "markerWidth",
    "markerHeight",
    "orient",
    "refX",
    "refY",
    "id",
    "class",
    "role",
    "aria-label",
];

/// Why a block was refused. The text is what the drop is logged and reported with, so it
/// names the offending element or attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgRejection {
    pub reason: String,
}

/// Validates the SVG and rewrites its ids with `prefix`.
///
/// Returns the prefixed source, or the reason it was refused. The scanner is hand-written
/// rather than a full XML parser on purpose: the input is a small subset, and a scanner
/// that rejects anything it does not recognise is the property wanted. A parser accepts
/// more than the allowlist does, which is the opposite of a guard.
pub fn check_and_prefix(svg: &str, prefix: &str) -> Result<String, SvgRejection> {
    let reject = |reason: String| Err(SvgRejection { reason });
    let mut out = String::with_capacity(svg.len() + 64);
    let bytes = svg.as_bytes();
    let mut index = 0usize;
    // Collected first, then applied, so a rewrite cannot be scanned again as input.
    let mut ids: Vec<String> = Vec::new();

    while index < bytes.len() {
        let Some(open) = svg[index..].find('<') else {
            // Trailing text outside any tag: SVG has no bare text nodes at this level, so
            // anything here is malformed.
            let rest = svg[index..].trim();
            if rest.is_empty() {
                break;
            }
            return reject(format!("text outside any element: {rest:?}"));
        };
        out.push_str(&svg[index..index + open]);
        let start = index + open;

        // A comment is not part of the allowlist, and passing one through is how a
        // commented-out `<script>` survives review.
        if svg[start..].starts_with("<!--") {
            return reject("comments are not allowed".to_string());
        }
        if svg[start..].starts_with("<!") || svg[start..].starts_with("<?") {
            return reject("a declaration or doctype is not allowed".to_string());
        }

        let Some(close) = svg[start..].find('>') else {
            return reject("an unterminated tag".to_string());
        };
        let tag = &svg[start..start + close + 1];
        let inner = &tag[1..tag.len() - 1];

        let (is_closing, body) = match inner.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, inner),
        };
        let self_closing = body.ends_with('/');
        let body = body.trim_end_matches('/').trim();

        let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
        let name = &body[..name_end];
        if name.is_empty() {
            return reject("an empty tag".to_string());
        }
        if !ALLOWED_ELEMENTS.contains(&name) {
            return reject(format!("<{name}> is not in the allowlist"));
        }

        if is_closing {
            out.push_str(tag);
            index = start + close + 1;
            continue;
        }

        let attributes = &body[name_end..];
        let mut rewritten = format!("<{name}");
        let mut rest = attributes.trim();
        while !rest.is_empty() {
            let name_len = rest
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(rest.len());
            let attribute = &rest[..name_len];
            if attribute.is_empty() {
                return reject(format!("a malformed attribute in <{name}>: {rest:?}"));
            }
            if !ALLOWED_ATTRIBUTES.contains(&attribute) {
                return reject(format!("{attribute:?} on <{name}> is not in the allowlist"));
            }
            rest = rest[name_len..].trim_start();

            // A valueless attribute is not something this subset uses, and accepting one
            // would mean deciding what it means.
            let Some(value_body) = rest.strip_prefix('=') else {
                return reject(format!("{attribute:?} on <{name}> has no value"));
            };
            let value_body = value_body.trim_start();
            let Some(quote) = value_body
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
            else {
                return reject(format!("{attribute:?} on <{name}> is not quoted"));
            };
            let after = &value_body[1..];
            let Some(end) = after.find(quote) else {
                return reject(format!("{attribute:?} on <{name}> is unterminated"));
            };
            let value = &after[..end];

            if attribute == "id" {
                // Recorded now, applied after the walk: the reference may appear before
                // the definition (a `<line>` can precede its `<marker>`'s `<defs>`).
                ids.push(value.to_string());
                rewritten.push_str(&format!(" {attribute}=\"{prefix}-{value}\""));
            } else {
                // `url(#name)` and `href`-style references inside an attribute value are
                // rewritten to the prefixed id. `url(#…)` is the only form this subset
                // uses; a bare `#name` in `marker-end` without `url()` is not valid SVG.
                let value = rewrite_url_refs(value, prefix);
                rewritten.push_str(&format!(" {attribute}=\"{value}\""));
            }
            rest = after[end + 1..].trim_start();
        }

        rewritten.push_str(if self_closing { "/>" } else { ">" });
        out.push_str(&rewritten);
        index = start + close + 1;
    }

    if !out.contains("<svg") {
        return reject("no <svg> element".to_string());
    }
    Ok(out)
}

/// Rewrites every `url(#name)` in an attribute value to the prefixed form.
fn rewrite_url_refs(value: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(value.len() + 16);
    let mut rest = value;
    while let Some(start) = rest.find("url(#") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        match after.find(')') {
            Some(end) => {
                out.push_str(&format!("url(#{prefix}-{})", &after[..end]));
                rest = &after[end + 1..];
            }
            None => {
                // An unterminated reference: left as-is rather than guessed at, and the
                // attribute allowlist has already accepted the attribute itself.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}
