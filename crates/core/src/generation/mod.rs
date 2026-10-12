//! Strict-JSON generation: the model's reply, turned into typed domain data.
//!
//! Every module here is a pure function of the reply text and the label map. The
//! retry-once rule lives in the caller (the server, which owns the provider), but
//! the *decision* it retries on — is this a parse failure — is made here, so the
//! rule is testable without a model.

pub mod breakdown;
pub mod capture;
pub mod cheatsheet;
pub mod flashcards;
pub mod quiz;
pub mod report;

#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("the model's reply was not valid JSON: {0}")]
    Json(String),
    #[error("the model's reply did not match the schema: {0}")]
    Schema(String),
    #[error("the model returned nothing to store")]
    Empty,
}

/// True when a second attempt is worth making.
///
/// A parse failure is worth one retry with a corrective message; an empty reply is
/// not, because the model answered the question it was asked and the answer was
/// "nothing", and retrying would spend a second call to be told the same thing.
///
/// The message is deliberately content-neutral: both the deck and the quiz contract
/// reuse this variant, so naming one of them ("no cards") would report the wrong thing
/// to the other's user.
pub fn is_retryable(error: &GenerationError) -> bool {
    matches!(error, GenerationError::Json(_) | GenerationError::Schema(_))
}

/// ```` ```json … ``` ```` becomes `…`; anything else is returned trimmed.
///
/// Shared by every strict-JSON contract, because a provider occasionally adds a fence
/// despite the module saying not to, and two copies of this rule would drift.
pub fn strip_code_fence(raw: &str) -> &str {
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
