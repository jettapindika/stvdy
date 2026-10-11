//! Strict-JSON generation: the model's reply, turned into typed domain data.
//!
//! Every module here is a pure function of the reply text and the label map. The
//! retry-once rule lives in the caller (the server, which owns the provider), but
//! the *decision* it retries on — is this a parse failure — is made here, so the
//! rule is testable without a model.

pub mod flashcards;

#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("the model's reply was not valid JSON: {0}")]
    Json(String),
    #[error("the model's reply did not match the schema: {0}")]
    Schema(String),
    #[error("the model returned no cards")]
    Empty,
}

/// True when a second attempt is worth making.
///
/// A parse failure is worth one retry with a corrective message; an empty deck is
/// not, because the model answered the question it was asked and the answer was
/// "no cards", and retrying would spend a second call to be told the same thing.
pub fn is_retryable(error: &GenerationError) -> bool {
    matches!(error, GenerationError::Json(_) | GenerationError::Schema(_))
}
