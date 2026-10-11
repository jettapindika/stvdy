//! Citation label resolution, including the golden test the roadmap asks for:
//! a reply citing a fabricated label must drop it, and the failure message must
//! name the fabricated label so a regression reads as a fact, not a guess.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use uuid::Uuid;

use stvdy_core::citations::{
    Citation, ContextLabels, LabeledChunk, LabeledWeb, resolve, streamable_end, strip_unknown,
};

fn labels(chunk_labels: &[u32]) -> ContextLabels {
    ContextLabels::new(
        chunk_labels
            .iter()
            .map(|&label| LabeledChunk {
                label,
                chunk_id: Uuid::from_u128(label as u128),
                source_id: Uuid::from_u128(9000 + label as u128),
                page: Some(label as i32),
            })
            .collect(),
        Vec::new(),
    )
}

fn labels_with_web() -> ContextLabels {
    ContextLabels::new(
        vec![LabeledChunk {
            label: 1,
            chunk_id: Uuid::from_u128(1),
            source_id: Uuid::from_u128(9001),
            page: Some(3),
        }],
        vec![LabeledWeb {
            label: 1,
            url: "https://example.test/tcp".to_string(),
            title: "TCP overview".to_string(),
        }],
    )
}

/// The golden test: the context has three chunks, the reply cites `[1]` (real)
/// and `[7]` (fabricated). Exactly one citation survives, and `[7]` is gone
/// from both the citations and the stripped text.
#[test]
fn a_fabricated_label_is_dropped_from_citations_and_text() {
    let context = labels(&[1, 2, 3]);
    let reply = "TCP uses a three-way handshake [1]. The RFC says so [7].";

    let citations = resolve(reply, &context);
    assert!(
        !citations.iter().any(
            |c| matches!(c, Citation::Chunk { chunk_id, .. } if *chunk_id == Uuid::from_u128(7))
        ),
        "fabricated label [7] must not resolve to a citation: {citations:?}"
    );
    assert_eq!(
        citations.len(),
        1,
        "only [1] is real, so exactly one citation must survive, got {citations:?}"
    );
    assert_eq!(
        citations[0],
        Citation::Chunk {
            chunk_id: Uuid::from_u128(1),
            source_id: Uuid::from_u128(9001),
            page: Some(1),
        }
    );

    let stripped = strip_unknown(reply, &context);
    assert!(
        !stripped.contains("[7]"),
        "fabricated label [7] must be removed from the text, got {stripped:?}"
    );
    assert!(
        stripped.contains("[1]"),
        "the real label [1] must be left exactly as written, got {stripped:?}"
    );
}

#[test]
fn a_label_repeated_three_times_yields_one_citation() {
    let context = labels(&[1, 2]);
    let reply = "First [2]. Again [2]. And once more [2].";

    let citations = resolve(reply, &context);

    assert_eq!(citations.len(), 1, "one chunk cited thrice is one citation");
    assert_eq!(
        citations[0],
        Citation::Chunk {
            chunk_id: Uuid::from_u128(2),
            source_id: Uuid::from_u128(9002),
            page: Some(2),
        }
    );
}

#[test]
fn a_label_past_the_context_is_dropped() {
    let context = labels(&[1, 2]);

    let citations = resolve("See [10].", &context);
    assert!(citations.is_empty(), "label [10] is outside the context");

    let stripped = strip_unknown("See [10].", &context);
    assert!(!stripped.contains("[10]"), "got {stripped:?}");
}

#[test]
fn citations_are_in_first_appearance_order() {
    let context = labels(&[1, 2, 3]);

    let citations = resolve("Later point [3], earlier point [1].", &context);

    assert_eq!(citations.len(), 2);
    assert_eq!(
        citations[0],
        Citation::Chunk {
            chunk_id: Uuid::from_u128(3),
            source_id: Uuid::from_u128(9003),
            page: Some(3),
        }
    );
    assert_eq!(
        citations[1],
        Citation::Chunk {
            chunk_id: Uuid::from_u128(1),
            source_id: Uuid::from_u128(9001),
            page: Some(1),
        }
    );
}

#[test]
fn web_labels_resolve_and_are_case_insensitive() {
    let context = labels_with_web();

    let citations = resolve("The RFC covers it [W1].", &context);
    assert_eq!(
        citations,
        vec![Citation::Web {
            url: "https://example.test/tcp".to_string(),
            title: "TCP overview".to_string(),
        }]
    );

    let lower = resolve("The RFC covers it [w1].", &context);
    assert_eq!(
        lower.len(),
        1,
        "the W in a web label is case-insensitive, got {lower:?}"
    );

    let stripped = strip_unknown("The RFC covers it [w1].", &context);
    assert!(
        stripped.contains("[w1]"),
        "a known label is left exactly as written, got {stripped:?}"
    );
}

#[test]
fn an_unknown_web_label_is_dropped() {
    let context = labels_with_web();

    let citations = resolve("From the web [W9].", &context);
    assert!(citations.is_empty());
    assert!(!strip_unknown("From the web [W9].", &context).contains("[W9]"));
}

#[test]
fn malformed_markers_are_left_alone_and_never_panic() {
    let context = labels(&[1, 2]);

    // Each of these is not a valid label at all, so it is text, not a citation.
    let text = "a [ b [1 c [] d [W] e [abc] f [0] g [01] h [1000] i";
    let stripped = strip_unknown(text, &context);

    assert_eq!(stripped, text, "malformed markers must be untouched");
    assert!(resolve(text, &context).is_empty());
}

#[test]
fn a_lone_bracket_does_not_panic() {
    let context = labels(&[1]);

    assert!(resolve("[", &context).is_empty());
    assert_eq!(strip_unknown("[", &context), "[");
    assert_eq!(strip_unknown("", &context), "");
    assert!(resolve("", &context).is_empty());
}

#[test]
fn labels_are_not_matched_across_text_boundaries() {
    let context = labels(&[1]);

    // `[1]` is known and stays; `[1x]` is not a label and stays.
    let text = "keep [1] and [1x]";
    assert_eq!(strip_unknown(text, &context), text);
}

#[test]
fn an_empty_context_drops_every_label() {
    let context = ContextLabels::default();

    assert!(context.is_empty());
    assert_eq!(context.chunk_count(), 0);
    assert!(context.chunk(1).is_none());
    assert!(context.web(1).is_none());

    assert!(resolve("Everything [1] is [W1] dropped.", &context).is_empty());
    // The intent is that no label survives; the spacing is the removal seam, and
    // asserting the double space it used to leave would pin an artifact rather
    // than the behaviour. A reader must see a clean sentence.
    let stripped = strip_unknown("Everything [1] is [W1] dropped.", &context);
    assert_eq!(stripped, "Everything is dropped.");
    assert!(!stripped.contains('['), "no marker may survive: {stripped}");
    assert!(!stripped.contains("  "), "no doubled space: {stripped:?}");
}

#[test]
fn a_dropped_label_before_punctuation_does_not_leave_a_space() {
    let context = labels_with_web();

    // The exact shape a model produces: a citation, then the sentence's period.
    // Without tidying, the period is preceded by the space the marker occupied,
    // which reads as a typo in an otherwise clean answer.
    assert_eq!(
        strip_unknown("a real claim [1] and a false one [9].", &context),
        "a real claim [1] and a false one."
    );
    assert_eq!(
        strip_unknown("one [9], two [9]; three [9]: four [9]!", &context),
        "one, two; three: four!"
    );
}

#[test]
fn a_kept_label_is_left_exactly_as_written() {
    let context = labels_with_web();

    // The tidy pass must not touch a marker that stays, or the client would have
    // to re-find labels it was already given.
    let text = "grounded [1] and grounded again [1].";
    assert_eq!(strip_unknown(text, &context), text);
}

#[test]
fn context_labels_expose_their_entries() {
    let context = labels_with_web();

    assert!(!context.is_empty());
    assert_eq!(context.chunk_count(), 1);
    assert_eq!(context.chunk(1).unwrap().page, Some(3));
    assert_eq!(context.web(1).unwrap().title, "TCP overview");
    assert!(context.chunk(2).is_none());
}

#[test]
fn a_complete_marker_is_streamable_but_a_partial_one_is_not() {
    // Nothing to hold back: no bracket at all.
    assert_eq!(streamable_end("a plain sentence"), "a plain sentence".len());

    // A complete marker is safe: the caller can validate it and drop it.
    assert_eq!(streamable_end("claim [1]"), "claim [1]".len());

    // A marker still being assembled is not: `[9` may yet become `[9]`, which
    // must be validated rather than shown. The space before the bracket is held
    // too, because `strip_unknown` swallows it with the marker and an emitted
    // byte cannot be taken back.
    assert_eq!(streamable_end("claim [9"), "claim".len());
    assert_eq!(streamable_end("claim ["), "claim".len());
    assert_eq!(streamable_end("claim [W"), "claim".len());
    // With no preceding space there is nothing extra to hold.
    assert_eq!(streamable_end("claim[9"), "claim".len());
    assert_eq!(streamable_end("[9"), 0);

    // A prose bracket with no close within a marker's length is emitted rather
    // than stalling the stream for the rest of the answer.
    let long = "see the note [see appendix for the full derivation";
    assert_eq!(streamable_end(long), long.len());
}

#[test]
fn a_dropped_marker_takes_its_preceding_space_and_nothing_else() {
    let context = labels_with_web();

    // The exact case the browser run caught: a fabricated label must not reach
    // the user, and the sentence must read cleanly afterwards.
    assert_eq!(strip_unknown("a claim [9] and", &context), "a claim and");
    assert_eq!(strip_unknown("a claim [9].", &context), "a claim.");
    assert_eq!(strip_unknown("one [9], two", &context), "one, two");
    assert_eq!(strip_unknown("word [9] word", &context), "word word");

    // A trailing space is never trimmed: the streaming path depends on it, or the
    // next fragment would be joined to this one.
    assert_eq!(strip_unknown("Mitochondria ", &context), "Mitochondria ");
    assert_eq!(strip_unknown("Mitochondria", &context), "Mitochondria");

    // A known label survives byte for byte, and the space before it stays.
    assert_eq!(
        strip_unknown("a claim [1] and", &context),
        "a claim [1] and"
    );
}

#[test]
fn the_streamed_text_equals_the_stored_text() {
    let context = labels_with_web();
    let answer = "ATP is made here [1], and also [9]. The rest is ordinary.";

    // The stored copy.
    let stored = strip_unknown(answer, &context);

    // The streamed copy, fragment by fragment through the same hold-back the
    // server uses.
    let mut emitted = String::new();
    let mut cursor = 0;
    for (index, _) in answer.char_indices() {
        let end = index + 1;
        let safe = streamable_end(&answer[..end]);
        if safe > cursor {
            emitted.push_str(&strip_unknown(&answer[cursor..safe], &context));
            cursor = safe;
        }
    }
    emitted.push_str(&strip_unknown(&answer[cursor..], &context));

    assert_eq!(
        emitted, stored,
        "the user must not watch the text change when the answer is finalised"
    );
    assert!(!emitted.contains("[9]"), "{emitted:?}");
    assert!(emitted.contains("[1]"), "{emitted:?}");
}

#[test]
fn the_space_before_a_marker_is_held_when_the_two_arrive_separately() {
    let context = labels_with_web();
    let answer = "ATP is made here [1], and also [9].";

    // The boundary the joined-text test cannot see: a provider that emits
    // `"and also "` as one token and `"[9]."` as the next. The space is emitted
    // before the marker is ever visible, so the hold-back must keep a trailing
    // space rather than only looking at the unmatched bracket.
    let fragments = ["ATP is made here ", "[1], ", "and also ", "[9]."];

    let mut emitted = String::new();
    let mut seen = String::new();
    // The cursor is an index into `seen`, not `emitted.len()`: `strip_unknown`
    // removes bytes, so the two differ and using the wrong one silently skips
    // text.
    let mut cursor = 0;
    for fragment in fragments {
        seen.push_str(fragment);
        let safe = streamable_end(&seen);
        if safe > cursor {
            emitted.push_str(&strip_unknown(&seen[cursor..safe], &context));
            cursor = safe;
        }
    }
    // The final flush, as the route does at the end of the stream.
    emitted.push_str(&strip_unknown(&seen[cursor..], &context));

    assert_eq!(
        emitted,
        strip_unknown(answer, &context),
        "a space emitted before its marker arrives is what breaks the equality"
    );
    assert!(!emitted.contains("[9]"), "{emitted:?}");
    assert!(emitted.contains("[1]"), "{emitted:?}");
}
