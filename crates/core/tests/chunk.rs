//! Chunker behaviour. The Indonesian case is not decoration: the roadmap's
//! language gate depends on this chunker splitting Indonesian sentences, which
//! use the same terminal punctuation but different spacing conventions.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::chunk::{ChunkConfig, chunk, heuristic_tokens};

/// Word count is the deterministic stand-in for the real tokenizer.
fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn small() -> ChunkConfig {
    ChunkConfig {
        target_tokens: 14,
        max_tokens: 16,
        overlap_tokens: 4,
    }
}

const ENGLISH: &str = "Photosynthesis converts light into chemical energy. \
Chlorophyll absorbs red and blue light. The reaction releases oxygen. \
Glucose stores the captured energy.";

const INDONESIAN: &str = "Selamat pagi! Apa kabar hari ini? \
Saya sedang belajar bahasa Indonesia. Apakah kamu juga belajar? \
Terima kasih banyak.";

fn assert_covers(chunks: &[String], sentences: &[&str]) {
    let joined = chunks.join(" ");
    for sentence in sentences {
        assert!(
            joined.contains(sentence),
            "sentence {sentence:?} missing from {joined:?}"
        );
    }
}

fn assert_within_max(chunks: &[String], cfg: &ChunkConfig) {
    for chunk in chunks {
        let tokens = words(chunk);
        assert!(
            tokens <= cfg.max_tokens,
            "chunk has {tokens} tokens, over max {}: {chunk:?}",
            cfg.max_tokens
        );
    }
}

#[test]
fn english_fixture_covers_every_sentence_and_respects_max() {
    let cfg = small();
    let chunks = chunk(ENGLISH, &cfg, &words);

    assert!(!chunks.is_empty());
    assert_within_max(&chunks, &cfg);
    assert_covers(
        &chunks,
        &[
            "Photosynthesis converts light into chemical energy.",
            "Chlorophyll absorbs red and blue light.",
            "The reaction releases oxygen.",
            "Glucose stores the captured energy.",
        ],
    );
}

#[test]
fn indonesian_fixture_covers_every_sentence() {
    let cfg = small();
    let chunks = chunk(INDONESIAN, &cfg, &words);

    assert!(
        chunks.len() >= 2,
        "five sentences must span more than one chunk"
    );
    assert_within_max(&chunks, &cfg);
    assert_covers(
        &chunks,
        &[
            "Selamat pagi!",
            "Apa kabar hari ini?",
            "Saya sedang belajar bahasa Indonesia.",
            "Apakah kamu juga belajar?",
            "Terima kasih banyak.",
        ],
    );
}

#[test]
fn overlap_appears_between_consecutive_chunks() {
    let cfg = ChunkConfig {
        target_tokens: 10,
        max_tokens: 12,
        overlap_tokens: 3,
    };
    let text = "Alpha beta gamma. Delta epsilon zeta. Eta theta iota. \
Kappa lambda mu. Nu xi omicron.";
    let chunks = chunk(text, &cfg, &words);

    assert!(chunks.len() >= 2);
    assert!(chunks[0].ends_with("Eta theta iota."));
    assert!(
        chunks[1].starts_with("Eta theta iota."),
        "the overlap must repeat the previous tail, got {:?}",
        chunks[1]
    );
}

#[test]
fn empty_input_yields_no_chunks() {
    let cfg = small();
    assert!(chunk("", &cfg, &words).is_empty());
    assert!(chunk("   \n\t  ", &cfg, &words).is_empty());
}

#[test]
fn chunking_is_deterministic() {
    let cfg = small();
    assert_eq!(chunk(ENGLISH, &cfg, &words), chunk(ENGLISH, &cfg, &words));
    assert_eq!(
        chunk(INDONESIAN, &cfg, &words),
        chunk(INDONESIAN, &cfg, &words)
    );
}

#[test]
fn long_sentence_is_split_without_breaking_words() {
    let cfg = ChunkConfig {
        target_tokens: 8,
        max_tokens: 10,
        overlap_tokens: 2,
    };
    let text = "one two three four five six seven eight nine ten \
eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty.";
    let chunks = chunk(text, &cfg, &words);

    assert!(chunks.len() >= 2, "an over-long sentence must split");
    assert_within_max(&chunks, &cfg);

    let original: Vec<&str> = text.split_whitespace().collect();
    let emitted: Vec<&str> = chunks.iter().flat_map(|c| c.split_whitespace()).collect();
    assert_eq!(emitted, original, "words must survive intact");
}

#[test]
fn heuristic_tokens_approximates_length() {
    assert_eq!(heuristic_tokens(""), 0);
    assert_eq!(heuristic_tokens("hello world"), 3);
    assert!(heuristic_tokens("a longer sentence with several words") > 5);
}
