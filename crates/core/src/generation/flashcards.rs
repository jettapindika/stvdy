//! The flashcards JSON contract (`prompts/flashcards.v1.md`), parsed and resolved.

use uuid::Uuid;

use crate::citations::ContextLabels;
use crate::generation::GenerationError;

/// The most cards one reply may carry. Above the largest request the API accepts,
/// so a model that ignores `{{count}}` cannot store an unbounded deck.
pub const MAX_CARDS_PER_REPLY: usize = 60;

/// The reply, exactly as `prompts/flashcards.v1.md` documents it.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that
/// invents `hint` or `difficulty` produces a parse error and a retry rather than a
/// card with a silently dropped field.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardsFile {
    pub cards: Vec<GeneratedCard>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedCard {
    pub front: String,
    pub back: String,
    /// The chunk label the card came from, as shown inside `<sources>`.
    pub label: u32,
}

/// One card with its chunk resolved, or `chunk_id: None` when the label was not in
/// the context (A17: dropped, never guessed).
#[derive(Debug, Clone)]
pub struct ResolvedCard {
    pub front: String,
    pub back: String,
    pub chunk_id: Option<Uuid>,
    pub label: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ParseOutcome {
    pub cards: Vec<ResolvedCard>,
    /// Labels the model cited that were not in the context, deduplicated and
    /// sorted. Recorded rather than silently swallowed: a rate that climbs means
    /// the prompt or the model changed.
    pub dropped_labels: Vec<u32>,
}

/// Parses a model reply into cards.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite
/// the contract. It does **not** tolerate prose around the object: guessing which
/// part is the JSON is how a half-parsed deck ships.
pub fn parse_cards(raw: &str) -> Result<Vec<GeneratedCard>, GenerationError> {
    let text = strip_code_fence(raw);
    let file: CardsFile = serde_json::from_str(text).map_err(|error| {
        // `serde_json` reports both "not JSON" and "wrong shape" through the same
        // error type; the message distinguishes them, and the caller only needs to
        // know a retry is worth one attempt.
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    if file.cards.is_empty() {
        return Err(GenerationError::Empty);
    }
    if file.cards.len() > MAX_CARDS_PER_REPLY {
        return Err(GenerationError::Schema(format!(
            "the model returned {} cards, more than the {MAX_CARDS_PER_REPLY} allowed",
            file.cards.len()
        )));
    }
    Ok(file.cards)
}

/// ```` ```json … ``` ```` becomes `…`; anything else is returned trimmed.
fn strip_code_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let body = match rest.find('\n') {
        Some(index) => &rest[index + 1..],
        None => rest,
    };
    body.trim_end()
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(body)
}

/// Resolves each card's label against the labels the prompt actually sent, and
/// drops the cards that carry nothing to store.
pub fn resolve_cards(cards: Vec<GeneratedCard>, labels: &ContextLabels) -> ParseOutcome {
    let mut outcome = ParseOutcome::default();

    for card in cards {
        let front = card.front.trim().to_string();
        let back = card.back.trim().to_string();
        if front.is_empty() || back.is_empty() {
            continue;
        }

        let chunk_id = labels.chunk(card.label).map(|chunk| chunk.chunk_id);
        if chunk_id.is_none() && !outcome.dropped_labels.contains(&card.label) {
            outcome.dropped_labels.push(card.label);
        }

        outcome.cards.push(ResolvedCard {
            front,
            back,
            chunk_id,
            label: card.label,
        });
    }

    outcome.dropped_labels.sort_unstable();
    outcome
}
