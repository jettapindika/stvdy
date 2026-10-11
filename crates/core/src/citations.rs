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
/// Known labels are left exactly as written (including `[w1]` lowercased), and
/// a body that is not a valid label at all is left untouched: rewriting
/// `[abc]` would be guessing at the author's intent.
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
        output.push_str(&text[cursor..marker.start]);
        cursor = marker.end;
    }
    output.push_str(&text[cursor..]);
    output
}
