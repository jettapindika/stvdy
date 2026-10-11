//! Citation label resolution.
//!
//! The model is given chunks as `[1]`, `[2]`, ... and web results as `[W1]`,
//! `[W2]`, ... rather than raw UUIDs, for two reasons: labels cost a few tokens
//! instead of ~40 per citation, and a model cannot hallucinate a plausible
//! label that happens to name another user's chunk, because the map is
//! per-request and only contains what this request sent. A label outside that
//! map is dropped, never rewritten: a reply must not show the user a citation
//! they cannot open. Pure functions, so the drop rule is unit tested.

use uuid::Uuid;

/// A chunk the server sent the model, with the label it was given.
#[derive(Debug, Clone)]
pub struct LabeledChunk {
    pub label: u32,
    pub chunk_id: Uuid,
    pub source_id: Uuid,
    pub page: Option<i32>,
}

/// A web result the server sent the model, with its label.
#[derive(Debug, Clone)]
pub struct LabeledWeb {
    pub label: u32,
    pub url: String,
    pub title: String,
}

/// The per-request label map. Fields are private so the only way to look a
/// label up is through the accessors, which return `None` for an unknown label
/// rather than a default: an absent label must be droppable, not fabricatable.
#[derive(Debug, Clone, Default)]
pub struct ContextLabels {
    chunks: Vec<LabeledChunk>,
    web: Vec<LabeledWeb>,
}

impl ContextLabels {
    pub fn new(chunks: Vec<LabeledChunk>, web: Vec<LabeledWeb>) -> Self {
        Self { chunks, web }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.web.is_empty()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Resolves a chunk label. Used by the prompt builder and by `resolve`.
    pub fn chunk(&self, label: u32) -> Option<&LabeledChunk> {
        self.chunks.iter().find(|entry| entry.label == label)
    }

    /// Resolves a web label. Phase 3 fills this side; the shape is fixed now so
    /// the resolver does not need a second pass when it lands.
    pub fn web(&self, label: u32) -> Option<&LabeledWeb> {
        self.web.iter().find(|entry| entry.label == label)
    }
}

/// A resolved citation, stored in `messages.citations` in this shape.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Citation {
    Chunk {
        chunk_id: Uuid,
        source_id: Uuid,
        page: Option<i32>,
    },
    Web {
        url: String,
        title: String,
    },
}

/// A marker found in the text: a syntactically valid label, before the map is
/// consulted. `start`/`end` are byte offsets, `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Marker {
    start: usize,
    end: usize,
    label: Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Label {
    Chunk(u32),
    Web(u32),
}

/// Finds every syntactically valid label marker in `text`.
///
/// Only ASCII `[`, `]`, `W` and digits participate, so byte offsets are always
/// char boundaries and the scan cannot panic on a lone `[` or a multi-byte
/// character next to one.
fn scan(text: &str) -> Vec<Marker> {
    let bytes = text.as_bytes();
    let mut markers = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        // A closing bracket may be absent (a lone `[`): then this is not a
        // marker and the scan just moves on.
        let Some(close_offset) = text[index + 1..].find(']') else {
            index += 1;
            continue;
        };
        let end = index + 1 + close_offset;
        let body = &text[index + 1..end];
        match parse_label(body) {
            Some(label) => {
                markers.push(Marker {
                    start: index,
                    end: end + 1,
                    label,
                });
                index = end + 1;
            }
            None => index += 1,
        }
    }

    markers
}

/// Parses a marker body into a label. `None` means the body is not a valid
/// label at all (`""`, `W`, `abc`, `0`, `1000`), as opposed to a valid label the
/// current context does not contain.
fn parse_label(body: &str) -> Option<Label> {
    let (is_web, digits) = match body.as_bytes().first() {
        Some(b'w') | Some(b'W') => (true, &body[1..]),
        _ => (false, body),
    };

    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // No leading zero: `[0]` and `[01]` are not labels the composer ever
    // emits, so treating them as labels would only invent citations.
    if digits.starts_with('0') {
        return None;
    }

    let value: u32 = digits.parse().ok()?;
    if is_web {
        // `[W1]`..`[W99]`.
        (1..=99).contains(&value).then_some(Label::Web(value))
    } else {
        // `[1]`..`[999]`.
        (1..=999).contains(&value).then_some(Label::Chunk(value))
    }
}

fn is_known(label: Label, labels: &ContextLabels) -> bool {
    match label {
        Label::Chunk(value) => labels.chunk(value).is_some(),
        Label::Web(value) => labels.web(value).is_some(),
    }
}

/// Citations in first-appearance order, unknown labels dropped, each label once.
///
/// A repeated label yields one citation: the same chunk cited three times is
/// one citation, and the UI would otherwise render three identical chips.
pub fn resolve(text: &str, labels: &ContextLabels) -> Vec<Citation> {
    let mut citations: Vec<Citation> = Vec::new();
    let mut emitted: Vec<Label> = Vec::new();

    for marker in scan(text) {
        if emitted.contains(&marker.label) {
            continue;
        }
        let citation = match marker.label {
            Label::Chunk(value) => labels.chunk(value).map(|entry| Citation::Chunk {
                chunk_id: entry.chunk_id,
                source_id: entry.source_id,
                page: entry.page,
            }),
            Label::Web(value) => labels.web(value).map(|entry| Citation::Web {
                url: entry.url.clone(),
                title: entry.title.clone(),
            }),
        };
        if let Some(citation) = citation {
            emitted.push(marker.label);
            citations.push(citation);
        }
    }

    citations
}

/// Removes every *unknown* label marker from `text`, so a reply cannot show a
/// citation the user cannot open.
///
/// Known labels are left exactly as written (including `[w1]` lowercased), and a
/// body that is not a valid label at all is left untouched: rewriting `[abc]`
/// would be guessing at the author's intent.
///
/// Dropping a marker also drops the single space immediately before it, so
/// `"and also [9]."` becomes `"and also."` rather than `"and also ."`. That rule
/// is deliberately the *only* whitespace rule: tidying the seam afterwards would
/// trim trailing spaces, and the streaming path must not lose one, because the
/// next fragment would then be joined to this one. One rule means the streamed
/// text and the stored text are byte-identical, so the user does not watch
/// punctuation move when the answer is finalised.
pub fn strip_unknown(text: &str, labels: &ContextLabels) -> String {
    let markers = scan(text);
    if markers.is_empty() {
        return text.to_string();
    }

    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for marker in markers {
        if is_known(marker.label, labels) {
            continue;
        }
        // The space before the marker, if there is one, and only when the marker
        // is not glued to the word that follows it.
        //
        // `"A [9]B"` is the case that matters: swallowing unconditionally joins
        // two words into `"AB"`, because the marker sat between a space and a
        // word character and both of its neighbours are content. The old
        // whitespace-tidying pass got this right by collapsing the seam instead,
        // which is why the rule has to be about the *neighbours* rather than
        // about the whitespace alone.
        let mut start = marker.start;
        let followed_by_a_word = text[marker.end..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric);
        if start > cursor && text[..start].ends_with(' ') && !followed_by_a_word {
            start -= 1;
        }
        output.push_str(&text[cursor..start]);
        cursor = marker.end;
    }
    output.push_str(&text[cursor..]);
    output
}

/// The largest marker the scanner recognises: `[999]` and `[W99]` are both five
/// bytes, with a byte of margin for a wider label later.
pub const MAX_MARKER_BYTES: usize = 8;

/// How many markers in `text` name something the server did not send.
///
/// Exposed rather than left to the caller to infer: the resolved citation count is
/// the number that *survived*, so logging it as "dropped" reports the inverse of
/// the thing anyone would want to measure, and a hallucination rate computed from
/// it would be wrong in the direction that looks reassuring.
pub fn unknown_count(text: &str, labels: &ContextLabels) -> usize {
    scan(text)
        .into_iter()
        .filter(|marker| !is_known(marker.label, labels))
        .count()
}

/// How much of `text` is safe to show the user right now.
///
/// Returns the index up to which every citation marker is *complete*, so the
/// caller never emits a marker that is still being assembled. The only ambiguous
/// tail is a `[` with no `]` after it; that is held back, but never more than a
/// marker's worth, because a prose `[` (a footnote, a code snippet) would
/// otherwise stall the stream for the rest of the answer.
///
/// This exists because streaming raw tokens leaks citations: a model that writes
/// `[9]` for a label the server never sent would show the user a marker the
/// server has already decided to drop, and the user would then watch it vanish
/// when the persisted copy replaced the live turn.
pub fn streamable_end(text: &str) -> usize {
    let mut end = text.len();

    // Never emit a trailing space on its own. The next byte may be a marker whose
    // preceding space `strip_unknown` has to swallow, and an emitted byte cannot
    // be taken back: emitting the space early is what made the streamed sentence
    // read "and also ." where the stored one reads "and also.".
    if text.ends_with(' ') {
        end -= 1;
    }

    // Hold an unterminated marker, and the space before it, so a complete marker
    // is always validated as a unit.
    if let Some(open) = text.rfind('[')
        && !text[open..].contains(']')
    {
        let held = text.len() - open;
        if held <= MAX_MARKER_BYTES {
            end = end.min(hold_from(text, open));
        }
    }

    // Hold a *complete* marker that sits at the very end of the text.
    //
    // `strip_unknown` decides whether to swallow the space before a marker by
    // looking at the byte after it, and at the end of a slice that byte does not
    // exist yet. Emitting the marker now would decide "nothing follows" and
    // swallow a space the stored text keeps, so the two copies would differ by
    // exactly the bug this whole hold-back exists to prevent. One more token is
    // enough to know.
    if text.ends_with(']')
        && let Some(open) = text.rfind('[')
        && text.len() - open <= MAX_MARKER_BYTES
    {
        end = end.min(hold_from(text, open));
    }

    end
}

/// Where a hold-back must start for a marker whose `[` is at `open`: the bracket
/// itself, or the space before it, because `strip_unknown` may swallow that space
/// with the marker.
fn hold_from(text: &str, open: usize) -> usize {
    if open > 0 && text[..open].ends_with(' ') {
        open - 1
    } else {
        open
    }
}
