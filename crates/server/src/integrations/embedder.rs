//! Embeddings behind `stvdy_core::traits::embedder::Embedder`.
//!
//! A2 fixes the model: `intfloat/multilingual-e5-base` through `fastembed`, 768
//! dims. The model card requires the `query: ` / `passage: ` prefixes, and
//! `fastembed` does not add them, so this impl does, at the boundary, on the
//! vector input only. Stored chunk text stays unprefixed.

use std::sync::{Arc, Mutex};

use stvdy_core::traits::embedder::{EmbedError, Embedder};

use crate::config::EmbedderConfig;

/// The real model. Construction is expensive (loads the ONNX graph), so it
/// happens once at boot and is shared.
pub struct FastembedEmbedder {
    model: Mutex<fastembed::TextEmbedding>,
    dim: usize,
    model_id: String,
}

impl FastembedEmbedder {
    pub fn new(cfg: &EmbedderConfig) -> anyhow::Result<Self> {
        let options =
            fastembed::TextInitOptions::new(fastembed::EmbeddingModel::MultilingualE5Base)
                .with_cache_dir(cfg.cache_dir.clone())
                .with_show_download_progress(false);

        let model = fastembed::TextEmbedding::try_new(options)
            .map_err(|error| anyhow::anyhow!("loading the embedding model: {error}"))?;

        Ok(Self {
            model: Mutex::new(model),
            dim: cfg.dim,
            model_id: cfg.model.clone(),
        })
    }
}

/// E5 is trained with these prefixes; a query embedded without `query: ` lands
/// in a different region of the space than the passage it should match, which
/// degrades retrieval silently rather than erroring.
pub const QUERY_PREFIX: &str = "query: ";
pub const PASSAGE_PREFIX: &str = "passage: ";

#[async_trait::async_trait]
impl Embedder for FastembedEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format!("{PASSAGE_PREFIX}{t}"))
            .collect();

        // `fastembed` is synchronous and CPU-bound; the server's ingestion path
        // already runs on a job worker, so this blocks that task rather than the
        // runtime's other tasks (spawn_blocking would copy the batch).
        let mut guard = self
            .model
            .lock()
            .map_err(|_| EmbedError::Provider("embedding model poisoned".to_string()))?;
        guard
            .embed(prefixed, None)
            .map_err(|error| EmbedError::Provider(error.to_string()))
    }
}

/// Query-side embedding, which is the only caller that needs the other prefix.
pub fn embed_query(_model: &dyn Embedder, text: &str) -> String {
    format!("{QUERY_PREFIX}{text}")
}

/// An HTTP embedder, for a deployment that would rather call a hosted model than
/// bake 1.06 GB into the image. Same trait, so nothing else changes.
pub struct HttpEmbedder {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model_id: String,
    dim: usize,
}

impl HttpEmbedder {
    pub fn new(base_url: String, api_key: Option<String>, model: String, dim: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model_id: model,
            dim,
        }
    }
}

#[async_trait::async_trait]
impl Embedder for HttpEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut request = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .json(&serde_json::json!({ "model": self.model_id, "input": texts }));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .map_err(|error| EmbedError::Provider(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(EmbedError::Provider(format!("{status}: {body}")));
        }

        #[derive(serde::Deserialize)]
        struct Embedding {
            embedding: Vec<f32>,
        }
        #[derive(serde::Deserialize)]
        struct Envelope {
            data: Vec<Embedding>,
        }

        let envelope: Envelope = response
            .json()
            .await
            .map_err(|error| EmbedError::Provider(error.to_string()))?;

        if envelope.data.len() != texts.len() {
            return Err(EmbedError::Invalid(format!(
                "asked for {} embeddings, got {}",
                texts.len(),
                envelope.data.len()
            )));
        }
        for item in &envelope.data {
            if item.embedding.len() != self.dim {
                return Err(EmbedError::Invalid(format!(
                    "embedder returned {} dims, expected {}",
                    item.embedding.len(),
                    self.dim
                )));
            }
        }
        Ok(envelope.data.into_iter().map(|d| d.embedding).collect())
    }
}

/// Wraps any embedder so the configured dimension is checked on every call
/// rather than only at boot: a provider that changes its output width under us
/// would otherwise corrupt the index one row at a time.
pub struct DimensionChecked {
    inner: Arc<dyn Embedder>,
}

impl DimensionChecked {
    pub fn new(inner: Arc<dyn Embedder>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl Embedder for DimensionChecked {
    fn dim(&self) -> usize {
        self.inner.dim()
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let vectors = self.inner.embed(texts).await?;
        for vector in &vectors {
            if vector.len() != self.dim() {
                return Err(EmbedError::Invalid(format!(
                    "embedder {} returned {} dims, expected {}",
                    self.model_id(),
                    vector.len(),
                    self.dim()
                )));
            }
        }
        Ok(vectors)
    }
}

/// A2's boot assertion: the configured dimension, the embedder's own dimension
/// and the database column's dimension must all agree. A mismatch aborts boot
/// instead of failing on the first ingest, because there is no widening cast and
/// the fix is a migration plus a full re-embed.
pub async fn assert_dimensions(
    pool: &sqlx::PgPool,
    embedder: &dyn Embedder,
    configured_dim: usize,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        embedder.dim() == configured_dim,
        "EMBEDDING_DIM={configured_dim} but embedder {} reports {} dims",
        embedder.model_id(),
        embedder.dim()
    );

    let column_dim: Option<i32> = sqlx::query_scalar(
        "select atttypmod from pg_attribute
          where attrelid = 'chunks'::regclass and attname = 'embedding'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| anyhow::anyhow!("reading the chunks.embedding dimension: {error}"))?;

    // pgvector stores the dimension in atttypmod for a plain vector(n) column.
    if let Some(raw) = column_dim {
        anyhow::ensure!(
            raw == configured_dim as i32,
            "chunks.embedding is vector({raw}) but EMBEDDING_DIM={configured_dim}"
        );
    }
    Ok(())
}
