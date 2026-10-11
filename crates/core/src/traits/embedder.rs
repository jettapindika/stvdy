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

    /// How many tokens this model's tokenizer produces for `text`, excluding
    /// any special tokens.
    ///
    /// This belongs on the trait because the chunker's token budget is the
    /// *model's* budget (A2: 480 against the model's 512 `max_length`), and an
    /// approximation that undercounts makes the tail of a chunk unretrievable
    /// with no error: `fastembed` truncates silently. The default is the
    /// character heuristic, which is documented as undercounting; an
    /// implementation with a real tokenizer must override it.
    fn token_count(&self, text: &str) -> usize {
        crate::chunk::heuristic_tokens(text)
    }

    /// Embeds search queries, applying whatever convention this model requires.
    ///
    /// A separate method rather than a flag on `embed` because the convention
    /// is a property of the model, not of the caller: E5 puts a query and a
    /// passage in different regions of the space and needs the `query: `
    /// prefix, while another model may need none. Hardcoding the prefix at the
    /// call site is how a query silently lands in the wrong region and
    /// retrieval degrades with no error.
    ///
    /// The default is no convention, so an embedder with none needs no
    /// override.
    async fn embed_query(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.embed(texts).await
    }
}
