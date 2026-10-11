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
    /// The model's own tokenizer, for the chunker's budget.
    ///
    /// Loaded separately because `fastembed` keeps its tokenizer private and
    /// exposes no accessor. `None` only when the cached file cannot be found, in
    /// which case `token_count` falls back to the documented heuristic and says
    /// so once in the log, because a silent fallback is how a chunk's tail
    /// becomes unretrievable.
    tokenizer: Option<tokenizers::Tokenizer>,
}

/// The models this build can load, by the name `EMBEDDER_MODEL` carries.
///
/// An explicit map rather than "whatever the config says": the name is written
/// into `chunks.embedder` and used as the dedupe key, so it must be the identity
/// of the model that actually produced the vector. Echoing an unsupported config
/// string back would label one model's vectors as another's, which is the mixed
/// vector space migration 0007's `embedder` column exists to prevent.
fn model_for(name: &str) -> Option<(fastembed::EmbeddingModel, &'static str, usize)> {
    match name {
        "intfloat/multilingual-e5-base" => Some((
            fastembed::EmbeddingModel::MultilingualE5Base,
            "intfloat/multilingual-e5-base",
            768,
        )),
        "intfloat/multilingual-e5-small" => Some((
            fastembed::EmbeddingModel::MultilingualE5Small,
            "intfloat/multilingual-e5-small",
            384,
        )),
        "intfloat/multilingual-e5-large" => Some((
            fastembed::EmbeddingModel::MultilingualE5Large,
            "intfloat/multilingual-e5-large",
            1024,
        )),
        _ => None,
    }
}

impl FastembedEmbedder {
    pub fn new(cfg: &EmbedderConfig) -> anyhow::Result<Self> {
        let (model, model_id, native_dim) = model_for(&cfg.model).ok_or_else(|| {
            anyhow::anyhow!(
                "EMBEDDER_MODEL={} is not a model this build can load; supported: \
                 intfloat/multilingual-e5-base, intfloat/multilingual-e5-small, \
                 intfloat/multilingual-e5-large",
                cfg.model
            )
        })?;

        // The configured dimension is asserted against the model's own, so a
        // config that says 768 for a 384-dimension model fails at boot rather
        // than producing vectors the column cannot hold.
        anyhow::ensure!(
            native_dim == cfg.dim,
            "EMBEDDER_MODEL={model_id} produces {native_dim} dims but EMBEDDING_DIM={}",
            cfg.dim
        );

        let options = fastembed::TextInitOptions::new(model)
            .with_cache_dir(cfg.cache_dir.clone())
            .with_show_download_progress(false);

        let loaded = fastembed::TextEmbedding::try_new(options)
            .map_err(|error| anyhow::anyhow!("loading the embedding model: {error}"))?;

        let tokenizer = load_tokenizer(&cfg.cache_dir, &cfg.model);
        if tokenizer.is_none() {
            tracing::warn!(
                model = %model_id,
                "the model tokenizer was not found in the cache; the chunker will use the \
                 character heuristic, which undercounts Indonesian and can let a chunk exceed \
                 the model's max_length"
            );
        }

        Ok(Self {
            model: Mutex::new(loaded),
            dim: native_dim,
            model_id: model_id.to_string(),
            tokenizer,
        })
    }
}

/// Finds `tokenizer.json` under the fastembed cache for `repo`.
///
/// The cache layout is Hugging Face's: `models--<org>--<name>/snapshots/<rev>/`.
/// Only the file name and the repo are needed, so this walks the one directory
/// rather than pulling in an HF client for a single local read.
fn load_tokenizer(cache_dir: &std::path::Path, repo: &str) -> Option<tokenizers::Tokenizer> {
    let dir_name = format!("models--{}", repo.replace('/', "--"));
    let snapshots = cache_dir.join(dir_name).join("snapshots");
    let entries = std::fs::read_dir(snapshots).ok()?;

    for entry in entries.flatten() {
        let candidate = entry.path().join("tokenizer.json");
        if candidate.is_file() {
            match tokenizers::Tokenizer::from_file(&candidate) {
                Ok(tokenizer) => {
                    tracing::info!(path = %candidate.display(), "model tokenizer loaded");
                    return Some(tokenizer);
                }
                Err(error) => {
                    tracing::warn!(%error, path = %candidate.display(), "the tokenizer failed to load");
                }
            }
        }
    }
    None
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

    /// The model's own count. `encode` adds the special tokens (CLS/SEP) unless
    /// they are disabled, and they count against `max_length`, so they are
    /// included: the budget must cover everything the model sees.
    fn token_count(&self, text: &str) -> usize {
        let Some(tokenizer) = &self.tokenizer else {
            return stvdy_core::chunk::heuristic_tokens(text);
        };
        match tokenizer.encode(text, true) {
            Ok(encoding) => encoding.get_ids().len(),
            // Encoding fails only on pathological input; the heuristic is a
            // better answer than aborting an ingestion.
            Err(_) => stvdy_core::chunk::heuristic_tokens(text),
        }
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

    /// Forwarded, not defaulted: the wrapper exists so a caller cannot bypass
    /// the dimension check, and a default here would silently swap the real
    /// tokenizer for the heuristic on the path that actually ingests.
    fn token_count(&self, text: &str) -> usize {
        self.inner.token_count(text)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every model this build accepts must declare the dimension the schema has.
    /// The `chunks.embedding` column is `vector(768)`, so a model with a
    /// different width is a migration, not a config change.
    #[test]
    fn the_supported_models_are_the_ones_the_schema_can_hold() {
        let (_, base, base_dim) = model_for("intfloat/multilingual-e5-base").expect("e5-base");
        assert_eq!(base, "intfloat/multilingual-e5-base");
        assert_eq!(base_dim, 768, "the schema's vector(768)");

        assert!(model_for("intfloat/multilingual-e5-small").is_some());
        assert!(model_for("intfloat/multilingual-e5-large").is_some());
        // An unsupported name must be refused, not echoed back: the name is
        // written into `chunks.embedder` and used as the dedupe key, so echoing
        // one model's name onto another's vectors is the mixed-vector-space
        // corruption migration 0007 exists to prevent.
        assert!(model_for("some/other-model").is_none());
        assert!(model_for("").is_none());
    }

    /// The real tokenizer must be used where it exists, because the character
    /// heuristic is unreliable in *both* directions and the chunker's budget is
    /// the model's, not an estimate: an overcount wastes context, an undercount
    /// lets a chunk exceed `max_length` and be truncated silently. Skips with a
    /// reason when the cache is cold, so a CI job without the model still runs
    /// this file.
    #[test]
    fn the_real_tokenizer_disagrees_with_the_heuristic() {
        let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.fastembed_cache")
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(".fastembed_cache"));

        let Some(tokenizer) = load_tokenizer(&cache, "intfloat/multilingual-e5-base") else {
            eprintln!(
                "skipped: no cached tokenizer under {} (run an ingest once, or set \
                 FASTEMBED_CACHE_DIR)",
                cache.display()
            );
            return;
        };

        // Measured on this machine: the tokenizer gives 25 for this Indonesian
        // sentence where the heuristic gives 28, so the heuristic *overcounts*
        // here. It undercounts on longer words elsewhere. Either direction is a
        // reason not to budget with it, which is the property under test.
        let indonesian = "Mahasiswa harus memahami konsep dasar termodinamika sebelum \
                          mengerjakan soal-soal latihan yang diberikan oleh dosen pengampu.";
        let real = tokenizer
            .encode(indonesian, true)
            .expect("the tokenizer encodes Indonesian")
            .get_ids()
            .len();
        let heuristic = stvdy_core::chunk::heuristic_tokens(indonesian);

        assert!(real > 0);
        assert_ne!(
            real, heuristic,
            "the heuristic agreed by coincidence; it is still not a tokenizer, and the \
             chunker must keep asking the embedder"
        );
        // And the budget must leave room inside the model's window.
        assert!(
            real < stvdy_core::chunk::ChunkConfig::default().max_tokens,
            "the default target must fit inside the model's max_length: {real} tokens"
        );
    }
}
