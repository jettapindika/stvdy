//! The pure renderer: escaping, the SVG allowlist, id prefixing, and determinism.
//!
//! Every case here is one of the template's own stated contracts, or one of the five
//! defects its first render surfaced. None of them needs Chromium: the renderer is a
//! function of `(blocks, template)`, which is what makes a re-render deterministic and
//! this suite runnable without a browser.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use uuid::Uuid;

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::render::blocks::parse_blocks;
use stvdy_core::render::svg::check_and_prefix;
use stvdy_core::render::{Cover, cheatsheet_v1, escape, render_html};

fn labels() -> ContextLabels {
    ContextLabels::new(
        vec![
            LabeledChunk {
                label: 1,
                chunk_id: Uuid::from_u128(11),
                source_id: Uuid::from_u128(1),
                page: Some(3),
            },
            LabeledChunk {
                label: 2,
                chunk_id: Uuid::from_u128(22),
                source_id: Uuid::from_u128(1),
                page: Some(4),
            },
        ],
        vec![],
    )
}

fn cover() -> Cover {
    Cover {
        course: "Jaringan Komputer".to_string(),
        exam: "UTS".to_string(),
        exam_date: "2026-11-01".to_string(),
        source_count: 2,
        // Passed in, never read from a clock: the renderer has no clock, and a test can
        // pin it.
        generated_at: "2026-10-12 07:00".to_string(),
        lang: "id".to_string(),
    }
}

/// Every SVG marker id in the rendered document, in document order.
///
/// Read out of the HTML rather than asserted against a spelling: the property under test is
/// that the ids are distinct and that each reference resolves, not what the prefix looks
/// like.
fn marker_ids(html: &str) -> Vec<String> {
    html.match_indices("id=\"")
        .map(|(at, marker)| {
            let rest = &html[at + marker.len()..];
            rest[..rest.find('"').expect("a closed attribute")].to_string()
        })
        .filter(|id| id.contains("arrowhead"))
        .collect()
}

/// A document with one of each interesting kind.
///
/// Generic over the item type so a call site can pass either a literal or a built
/// `String` without an `.as_str()` dance at every one.
fn document<S: AsRef<str>>(blocks: &[S]) -> String {
    let joined = blocks
        .iter()
        .map(|block| block.as_ref())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"title":"Jaringan","subtitle":"Ringkasan","course":"JK","exam":"UTS","chapters":[
             {{"title":"Lapisan","sections":[{{"title":"Model","blocks":[{joined}]}}]}}]}}"#
    )
}

fn text_block(text: &str) -> String {
    format!(r#"{{"kind":"text","text":{text:?},"labels":[1]}}"#)
}

/// Generation produces data, never markup. A field containing a tag is escaped and shown
/// literally, and that is asserted rather than assumed: the failure mode is a source
/// document containing `<script>` becoming markup in a document the student prints.
#[test]
fn markup_in_a_field_is_escaped_not_interpreted() {
    let blocks = parse_blocks(&document(&[text_block(
        "<script>alert(1)</script> & <b>bold</b>",
    )]))
    .unwrap();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .unwrap()
        .html;

    assert!(
        html.contains("&lt;script&gt;"),
        "the tag must be escaped: {html}"
    );
    assert!(
        !html.contains("<script>alert"),
        "no tag from a field may reach the page as markup"
    );
    assert!(html.contains("&amp;"), "an ampersand must be escaped too");
}

/// The escaping covers the cover fields too, which come from the request rather than the
/// model and are just as untrusted.
#[test]
fn a_cover_field_is_escaped_too() {
    let blocks = parse_blocks(&document(&[text_block("ok")])).unwrap();
    let mut cover = cover();
    cover.course = "<img src=x onerror=alert(1)>".to_string();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover)
        .unwrap()
        .html;

    assert!(
        !html.contains("<img src=x"),
        "the cover must be escaped: {html}"
    );
    assert!(html.contains("&lt;img"), "{html}");
}

/// The SVG allowlist is closed. An element outside it drops the whole block rather than
/// being stripped: a diagram with its `<script>` removed is not the diagram the model drew,
/// and rendering a mutilated version is a quiet lie about the content.
#[test]
fn a_disallowed_svg_element_drops_the_block_and_records_why() {
    let svg =
        r#"<svg viewBox="0 0 10 10"><script>alert(1)</script><rect width="1" height="1"/></svg>"#;
    let blocks = parse_blocks(&document(&[format!(
        r#"{{"kind":"svg","svg":{svg:?},"labels":[1]}}"#
    )]))
    .unwrap();
    let rendered = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover()).unwrap();

    assert!(
        !rendered.html.contains("alert(1)"),
        "the refused block's payload must not survive; the template's own KaTeX script is \
         not the block's"
    );
    assert_eq!(
        rendered.dropped.len(),
        1,
        "the drop is recorded, not silent"
    );
    assert!(
        rendered.dropped[0].reason.contains("script"),
        "the reason names the element: {:?}",
        rendered.dropped[0]
    );
    assert_eq!(rendered.dropped[0].kind, "svg");
    assert_eq!(rendered.dropped[0].path, "chapter 1 section 1 block 1");
}

/// An attribute outside the allowlist is refused, which is what stops an `on*` handler or
/// an external `href` even when the element itself is permitted.
#[test]
fn a_disallowed_svg_attribute_is_refused() {
    for (svg, expected) in [
        (
            r#"<svg viewBox="0 0 10 10"><rect width="1" height="1" onclick="alert(1)"/></svg>"#,
            "onclick",
        ),
        (
            r#"<svg viewBox="0 0 10 10"><image href="https://example.test/x.png"/></svg>"#,
            "image",
        ),
        (
            r#"<svg viewBox="0 0 10 10"><rect style="fill:red" width="1" height="1"/></svg>"#,
            "style",
        ),
    ] {
        let error = check_and_prefix(svg, "b1").unwrap_err();
        assert!(
            error.reason.contains(expected),
            "expected {expected:?} in {:?}",
            error.reason
        );
    }
}

/// Two diagrams in one section must not collide either.
///
/// Asserted on the *property* (distinct ids, each reference resolving to its own
/// definition) rather than on the prefix's spelling, so a future change to how the prefix is
/// built does not make this a test of the naming.
#[test]
fn two_diagrams_in_one_section_do_not_share_element_ids() {
    let svg = r#"<svg viewBox="0 0 10 10"><defs><marker id="arrowhead"/></defs><line x1="0" y1="0" x2="1" y2="1" marker-end="url(#arrowhead)"/></svg>"#;
    let blocks = parse_blocks(&document(&[
        format!(r#"{{"kind":"svg","svg":{svg:?},"labels":[1]}}"#),
        format!(r#"{{"kind":"svg","svg":{svg:?},"labels":[1]}}"#),
    ]))
    .unwrap();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .unwrap()
        .html;

    let ids = marker_ids(&html);
    assert_eq!(ids.len(), 2, "both diagrams emitted a marker: {ids:?}");
    assert_ne!(ids[0], ids[1], "the two ids must differ: {ids:?}");
    for id in &ids {
        assert!(
            html.contains(&format!("url(#{id})")),
            "the reference for {id} must resolve to it"
        );
    }
    assert!(
        !html.contains("id=\"arrowhead\""),
        "no unprefixed id may remain: {html}"
    );
}

/// A comment is not part of the allowlist, and passing one through is how a
/// commented-out `<script>` survives review.
#[test]
fn an_svg_comment_is_refused() {
    let svg =
        r#"<svg viewBox="0 0 10 10"><!-- <script>x</script> --><rect width="1" height="1"/></svg>"#;
    assert!(check_and_prefix(svg, "b1").is_err());
}

/// The renderer is pure. This is the property `POST /cheatsheets/:id/re-render` depends on:
/// a template fix re-renders every existing document for free, and the result must be
/// identical for identical input or the document changes under a student who already
/// printed it.
#[test]
fn rendering_is_deterministic() {
    let blocks = parse_blocks(&document(&[
        text_block("Satu."),
        r#"{"kind":"concept","term":"Enkapsulasi","text":"Menambah header.","labels":[1,2]}"#
            .to_string(),
        r#"{"kind":"formula","latex":"\\eta = L/(L+H)","caption":"Efisiensi","legend":[{"symbol":"L","meaning":"payload","unit":"byte"}],"labels":[1]}"#
            .to_string(),
    ]))
    .unwrap();

    let first = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover()).unwrap();
    let second = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover()).unwrap();

    assert_eq!(
        first.html, second.html,
        "a re-render must be byte-identical"
    );
}

/// Labels resolve to citation markers, and a label that was never sent is dropped rather
/// than rendered as a dangling reference (A17).
#[test]
fn labels_resolve_to_citations_and_unknown_ones_are_dropped() {
    let blocks = parse_blocks(&document(&[
        r#"{"kind":"text","text":"Dua klaim.","labels":[1,2]}"#.to_string(),
        r#"{"kind":"text","text":"Label yang tidak ada.","labels":[9]}"#.to_string(),
    ]))
    .unwrap();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .unwrap()
        .html;

    assert!(html.contains("[1]"), "{html}");
    assert!(html.contains("[2]"), "{html}");
    assert!(
        !html.contains("[9]"),
        "a label that was never sent must not render: {html}"
    );
    // The text itself survives: the block loses only its marker.
    assert!(html.contains("Label yang tidak ada."), "{html}");
}

/// The table of contents is generated from the same indices the chapters emit, so it
/// cannot drift from the document, and its anchors resolve.
#[test]
fn the_table_of_contents_anchors_match_the_chapter_ids() {
    let blocks = parse_blocks(
        r#"{"title":"T","subtitle":"S","chapters":[
            {"title":"Satu","sections":[{"title":"A","blocks":[{"kind":"text","text":"x","labels":[]}]},
                                        {"title":"B","blocks":[{"kind":"text","text":"y","labels":[]}]}]},
            {"title":"Dua","sections":[{"title":"C","blocks":[{"kind":"text","text":"z","labels":[]}]}]}]}"#,
    )
    .unwrap();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .unwrap()
        .html;

    for anchor in ["#ch1", "#ch2", "#ch1-1", "#ch1-2", "#ch2-1"] {
        assert!(
            html.contains(&format!("href=\"{anchor}\"")),
            "the TOC must link {anchor}: {html}"
        );
        assert!(
            html.contains(&format!("id=\"{}\"", anchor.trim_start_matches('#'))),
            "and the anchor must exist: {anchor}"
        );
    }
}

/// The formula delimiters are the renderer's, so a model that wraps its LaTeX in `$$`
/// itself does not produce `$$$$`, which KaTeX renders as nothing.
#[test]
fn the_renderer_owns_the_formula_delimiters() {
    let blocks = parse_blocks(&document(&[
        r#"{"kind":"formula","latex":"$$x = 1$$","legend":[{"symbol":"x","meaning":"a","unit":null}],"labels":[]}"#
            .to_string(),
    ]))
    .unwrap();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .unwrap()
        .html;

    assert!(html.contains("$$x = 1$$"), "{html}");
    assert!(!html.contains("$$$$"), "no doubled delimiters: {html}");
}

/// The escaping function itself, stated directly, because every field in the document
/// depends on it.
#[test]
fn escaping_covers_the_five_characters_that_matter() {
    assert_eq!(escape("&"), "&amp;");
    assert_eq!(escape("<"), "&lt;");
    assert_eq!(escape(">"), "&gt;");
    assert_eq!(escape("\""), "&quot;");
    assert_eq!(escape("'"), "&#39;");
    assert_eq!(escape("a & b < c"), "a &amp; b &lt; c");
    // Order matters: `&` must be escaped before the others or `&lt;` becomes `&amp;lt;`.
    assert_eq!(escape("<"), "&lt;");
}

/// A document with no chapters is the model saying it has nothing, which is `Empty` and is
/// not retried.
#[test]
fn an_empty_document_is_empty_and_a_malformed_one_is_json() {
    use stvdy_core::generation::{GenerationError, is_retryable};

    let empty = parse_blocks(r#"{"title":"T","subtitle":"S","chapters":[]}"#).unwrap_err();
    assert!(matches!(empty, GenerationError::Empty));
    assert!(!is_retryable(&empty));

    let malformed = parse_blocks("not json").unwrap_err();
    assert!(matches!(malformed, GenerationError::Json(_)));
    assert!(is_retryable(&malformed));
}

/// The per-block rules the type cannot express, each named with its index so a retry can
/// point at the block that was wrong.
#[test]
fn the_block_rules_are_enforced_with_their_index() {
    for (raw, expected) in [
        // A table row of the wrong width: a mis-set table the template cannot recover from.
        (
            r#"{"kind":"table","headers":["a","b"],"rows":[["1"]],"labels":[]}"#,
            "row 0 has 1 cells against 2 headers",
        ),
        // A formula with no legend is the one a student cannot use under pressure.
        (
            r#"{"kind":"formula","latex":"x","legend":[],"labels":[]}"#,
            "no legend",
        ),
        // A question whose likelihood is not one of the three the template styles.
        (
            r#"{"kind":"question","prompt":"p","likelihood":"maybe","answer":{"steps":["s"],"final":"f"},"labels":[]}"#,
            "likelihood must be high, medium or low",
        ),
        // A question with no steps teaches nothing: the reasoning is the point.
        (
            r#"{"kind":"question","prompt":"p","likelihood":"high","answer":{"steps":[],"final":"f"},"labels":[]}"#,
            "no steps",
        ),
        // More columns than fit a page.
        (
            r#"{"kind":"table","headers":["1","2","3","4","5","6","7","8","9"],"rows":[],"labels":[]}"#,
            "more than the 8",
        ),
    ] {
        let error = parse_blocks(&document(&[raw.to_string()])).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(expected),
            "expected {expected:?} in {message:?}"
        );
        assert!(
            message.contains("chapter 0 section 0 block 0"),
            "the index must name the block: {message:?}"
        );
    }
}

/// A drifted prompt fails loudly rather than storing a block with a silently dropped field.
#[test]
fn an_unknown_field_or_kind_is_refused() {
    let unknown_field = parse_blocks(&document(&[
        r#"{"kind":"text","text":"x","labels":[],"hint":"look here"}"#,
    ]))
    .unwrap_err();
    assert!(
        unknown_field.to_string().contains("hint"),
        "{unknown_field}"
    );

    let unknown_kind =
        parse_blocks(&document(&[r#"{"kind":"essay","text":"x","labels":[]}"#])).unwrap_err();
    let message = unknown_kind.to_string();
    assert!(message.contains("essay"), "{message}");
}

/// Two sections that each contain a diagram at the same block position must not collide.
///
/// This is the case the earlier per-block test could not see: `document()` puts every block
/// in one section, so its two diagrams landed at block indices 1 and 2 and were distinct
/// whatever the prefix was. The real document has one diagram in section 1 and another in
/// section 2, where the block index restarts, so a prefix built from the block index alone
/// emits `b1` twice: the second diagram's `url(#b1-arrow)` then resolves to the first's
/// marker and the arrow comes out the wrong colour, with no error anywhere. The PDF
/// acceptance test cannot see it either, because it asserts page count, header, footer and
/// no raw `$`, none of which observe marker identity.
#[test]
fn two_sections_with_a_diagram_each_do_not_share_element_ids() {
    let svg = r#"<svg viewBox="0 0 10 10"><defs><marker id="arrowhead"/></defs><line x1="0" y1="0" x2="1" y2="1" marker-end="url(#arrowhead)"/></svg>"#;
    let raw = format!(
        r#"{{"title":"T","subtitle":"S","chapters":[{{"title":"Bab","sections":[
             {{"title":"Satu","blocks":[{{"kind":"svg","svg":{svg:?},"labels":[]}}]}},
             {{"title":"Dua","blocks":[{{"kind":"svg","svg":{svg:?},"labels":[]}}]}}]}}]}}"#
    );
    let blocks = parse_blocks(&raw).unwrap();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .unwrap()
        .html;

    let ids: Vec<&str> = html
        .match_indices("id=\"")
        .map(|(at, marker)| {
            let rest = &html[at + marker.len()..];
            &rest[..rest.find('"').expect("a closed attribute")]
        })
        .filter(|id| id.contains("arrowhead"))
        .collect();

    assert_eq!(ids.len(), 2, "both diagrams emitted their marker: {ids:?}");
    assert_ne!(
        ids[0], ids[1],
        "two diagrams in different sections must not share an id: {ids:?}"
    );
    // And each reference resolves to its own definition, which is the property that
    // actually matters: a marker defined once and referenced twice draws the same arrow.
    for id in &ids {
        assert!(
            html.contains(&format!("url(#{id})")),
            "the reference for {id} must resolve to it: {html}"
        );
    }
}
