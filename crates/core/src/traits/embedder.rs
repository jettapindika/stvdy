//! Text embedding capability.

/// Failure modes for embedding.
///
/// `Provider` is a transport or upstream failure and is retryable; `Invalid`
/// means the input or the configuration is wrong and a retry cannot help.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("{0}")]
    Provider(String),
    #[error("{0}")]
    Invalid(String),
}

/// Turns text into vectors.
///
/// `dim` MUST equal the dimension of `chunks.embedding`: a mismatch is only
/// discovered at insert time, so the server asserts it against the column at
/// boot rather than trusting configuration.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;

    fn model_id(&self) -> &str;

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}
