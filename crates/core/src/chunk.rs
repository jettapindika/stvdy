//! Sentence-safe chunking for embedding.
//!
//! The embedder truncates silently past its 512-token limit, so a chunk that
//! is too long loses its tail with no error. This module therefore guarantees
//! every chunk fits `max_tokens`, and it never cuts mid-word because a half
//! word embeds as noise.

use unicode_segmentation::UnicodeSegmentation;

/// Chunk sizes. Defaults are pinned to the embedding model's `max_length` of
/// 512 (A2), with a 50-token overlap so a fact split across a boundary is
/// still retrievable from one of the two chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkConfig {
    pub target_tokens: usize,
    pub max_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            target_tokens: 480,
            max_tokens: 512,
            overlap_tokens: 50,
        }
    }
}

/// Split `text` into sentence-safe chunks.
///
/// `count` is injected so the server can pass the real tokenizer and tests a
/// deterministic counter; the chunker never assumes a tokenization.
pub fn chunk(text: &str, cfg: &ChunkConfig, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    let sentences = split_long_sentences(text, cfg, count);
    if sentences.is_empty() {
        return Vec::new();
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut carry = String::new();
    let mut index = 0;
    while index < sentences.len() {
        let carry_tokens = if carry.is_empty() { 0 } else { count(&carry) };
        let mut body = carry.clone();
        let mut body_tokens = carry_tokens;
        let mut added = 0usize;
        let mut end = index;
        while end < sentences.len() {
            let tokens = count(&sentences[end]);
            if added == 0 {
                // A chunk always keeps its first sentence. If the carried
                // overlap would push it past the model limit, drop the
                // overlap: a chunk the model truncates is worse than a
                // missing one.
                if body_tokens + tokens > cfg.max_tokens {
                    body.clear();
                    body_tokens = 0;
                }
            } else if body_tokens + tokens > cfg.target_tokens {
                break;
            }
            if !body.is_empty() {
                body.push(' ');
            }
            body.push_str(&sentences[end]);
            body_tokens += tokens;
            added += 1;
            end += 1;
        }

        chunks.push(body.clone());
        if end >= sentences.len() {
            break;
        }
        carry = overlap_prefix(&body, cfg.overlap_tokens, count);
        index = end;
    }
    chunks
}

/// Cheap token approximation for when no tokenizer is wired in.
///
/// Four non-whitespace characters per token is the usual rule of thumb for BPE
/// English. It undercounts Indonesian, whose words are longer, so it is only a
/// fallback and the server passes the real tokenizer wherever size matters.
pub fn heuristic_tokens(text: &str) -> usize {
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    chars.div_ceil(4)
}

/// Sentences, trimmed and with empties dropped, each no longer than
/// `max_tokens`; an over-long sentence is split on word boundaries.
fn split_long_sentences(
    text: &str,
    cfg: &ChunkConfig,
    count: &dyn Fn(&str) -> usize,
) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    for sentence in split_sentences(text) {
        if count(sentence) > cfg.max_tokens {
            units.extend(split_on_words(sentence, cfg.max_tokens, count));
        } else {
            units.push(sentence.to_string());
        }
    }
    units
}

fn split_sentences(text: &str) -> Vec<&str> {
    text.split_sentence_bounds()
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

/// Greedy word packing that never breaks a word; a lone word longer than
/// `limit` is emitted whole because the alternative is a mid-word cut.
fn split_on_words(text: &str, limit: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;
    for word in text.split_whitespace() {
        let tokens = count(word);
        if !current.is_empty() && current_tokens + tokens > limit {
            pieces.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        current_tokens += tokens;
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// The tail of `body` to repeat at the start of the next chunk.
///
/// Whole trailing sentences are preferred; when the last sentence alone
/// exceeds the budget, its trailing words are used instead so a long sentence
/// still gets overlap.
fn overlap_prefix(body: &str, budget: usize, count: &dyn Fn(&str) -> usize) -> String {
    if budget == 0 {
        return String::new();
    }
    let sentences = split_sentences(body);
    let mut total = 0usize;
    let mut start = sentences.len();
    for index in (0..sentences.len()).rev() {
        let tokens = count(sentences[index]);
        if total + tokens > budget {
            break;
        }
        total += tokens;
        start = index;
    }
    if start < sentences.len() {
        return sentences[start..].join(" ");
    }

    let last = sentences.last().copied().unwrap_or(body);
    let words: Vec<&str> = last.split_whitespace().collect();
    let mut total = 0usize;
    let mut start = words.len();
    for index in (0..words.len()).rev() {
        let tokens = count(words[index]);
        if total + tokens > budget {
            break;
        }
        total += tokens;
        start = index;
    }
    words[start..].join(" ")
}
