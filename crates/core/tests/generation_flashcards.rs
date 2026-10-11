//! The flashcards JSON contract: parsing a model reply and resolving its labels.
//!
//! A provider is not available here and must not be: parsing and label resolution
//! are pure, and they are where the two defects that reach a user live (a reply
//! that is nearly JSON, and a card citing a label that was never sent).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::generation::GenerationError;
use stvdy_core::generation::flashcards::{MAX_CARDS_PER_REPLY, parse_cards, resolve_cards};
use stvdy_core::generation::is_retryable;
use uuid::Uuid;

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
                page: None,
            },
        ],
        vec![],
    )
}

#[test]
fn a_plain_object_parses() {
    let cards = parse_cards(
        r#"{"cards":[{"front":"Apa itu TCP?","back":"Protokol transport.","label":1}]}"#,
    )
    .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].label, 1);
}

#[test]
fn a_fenced_reply_parses_because_a_provider_sometimes_adds_one() {
    let raw = "```json\n{\"cards\":[{\"front\":\"a\",\"back\":\"b\",\"label\":1}]}\n```";
    assert_eq!(parse_cards(raw).unwrap().len(), 1);
}

#[test]
fn prose_around_the_object_is_not_guessed_at() {
    let raw = "Here are your cards:\n{\"cards\":[]}";
    // serde classifies leading prose as a syntax error, not a schema error, so this
    // is `Json` and not `Schema`. What matters for the caller is that both are
    // retryable and neither is guessed at.
    assert!(matches!(parse_cards(raw), Err(GenerationError::Json(_))));
}

#[test]
fn an_extra_field_is_rejected_rather_than_ignored() {
    let raw = r#"{"cards":[{"front":"a","back":"b","label":1,"hint":"c"}]}"#;
    assert!(matches!(parse_cards(raw), Err(GenerationError::Schema(_))));
}

#[test]
fn an_empty_deck_is_an_error_not_an_empty_success() {
    assert!(matches!(
        parse_cards(r#"{"cards":[]}"#),
        Err(GenerationError::Empty)
    ));
}

#[test]
fn a_label_that_was_never_sent_is_dropped_and_recorded() {
    let cards = parse_cards(
        r#"{"cards":[
             {"front":"a","back":"b","label":1},
             {"front":"c","back":"d","label":99},
             {"front":"e","back":"f","label":99}
           ]}"#,
    )
    .unwrap();
    let outcome = resolve_cards(cards, &labels());
    // The card survives; only its link is lost.
    assert_eq!(outcome.cards.len(), 3);
    assert_eq!(outcome.cards[0].chunk_id, Some(Uuid::from_u128(11)));
    // Dropped, never rewritten to a nearby label, and counted once.
    assert_eq!(outcome.cards[1].chunk_id, None);
    assert_eq!(outcome.dropped_labels, vec![99]);
}

#[test]
fn a_card_with_an_empty_side_is_dropped() {
    let cards = parse_cards(
        r#"{"cards":[{"front":"  ","back":"b","label":1},{"front":"c","back":"","label":1}]}"#,
    )
    .unwrap();
    let outcome = resolve_cards(cards, &labels());
    assert!(outcome.cards.is_empty(), "{:?}", outcome.cards);
}

/// Pins both sides of the cap: the limit itself is accepted, one card more is
/// refused. A truncating implementation would return `Ok` with fewer cards, so
/// the passing case asserts the count too.
#[test]
fn a_reply_at_the_cap_parses_and_one_over_is_refused() {
    let card = |index: usize| {
        serde_json::json!({
            "front": format!("front {index}"),
            "back": format!("back {index}"),
            "label": 1,
        })
    };
    let at_cap = serde_json::to_string(&serde_json::json!({
        "cards": (0..MAX_CARDS_PER_REPLY).map(card).collect::<Vec<_>>(),
    }))
    .unwrap();
    assert_eq!(parse_cards(&at_cap).unwrap().len(), MAX_CARDS_PER_REPLY);

    let over_cap = serde_json::to_string(&serde_json::json!({
        "cards": (0..=MAX_CARDS_PER_REPLY).map(card).collect::<Vec<_>>(),
    }))
    .unwrap();
    assert!(matches!(
        parse_cards(&over_cap),
        Err(GenerationError::Schema(_))
    ));
}

/// Only an empty deck is not retryable. Task 4 branches on exactly this to choose
/// between a second billed call and a permanent failure, so inverting the match
/// must fail a test.
#[test]
fn only_an_empty_deck_is_not_retryable() {
    assert!(is_retryable(&GenerationError::Json("bad".to_string())));
    assert!(is_retryable(&GenerationError::Schema("bad".to_string())));
    assert!(!is_retryable(&GenerationError::Empty));
}
