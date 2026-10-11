//! First implementations of the provider traits (roadmap § Cross-Cutting
//! Contracts § 1). Each is selected by configuration, so a provider is a
//! deployment decision rather than a code change.

pub mod clock;
pub mod documents;
pub mod embedder;
pub mod fetch;
pub mod llm;
pub mod mailer;
pub mod search;
pub mod storage;

use std::sync::Arc;

use secrecy::ExposeSecret;
use stvdy_core::traits::clock::Clock;
use stvdy_core::traits::embedder::Embedder;
use stvdy_core::traits::mailer::Mailer;
use stvdy_core::traits::storage::Storage;

use crate::config::{AppConfig, EmbedderProvider};

pub struct Providers {
    pub storage: Arc<dyn Storage>,
    pub embedder: Arc<dyn Embedder>,
    pub mailer: Arc<dyn Mailer>,
    pub clock: Arc<dyn Clock>,
    /// Present only for the S3 backend, so `main` can create the bucket.
    s3: Option<Arc<storage::S3Storage>>,
    /// `None` when `LLM_BASE_URL` is unset.
    pub llm: Option<Arc<dyn stvdy_core::traits::llm::LlmProvider>>,
}

impl Providers {
    /// The S3 backend, when configured. `None` for the local backend, which
    /// needs no bucket.
    pub fn storage_as_s3(&self) -> Option<&storage::S3Storage> {
        self.s3.as_deref()
    }
}

impl Providers {
    /// Builds every provider and performs the boot assertions that would
    /// otherwise fail silently at first use: the storage backend is reachable,
    /// and the embedding dimension agrees with the database column (A2).
    pub async fn build(cfg: &AppConfig, pool: &sqlx::PgPool) -> anyhow::Result<Self> {
        let (storage, s3): (Arc<dyn Storage>, Option<Arc<storage::S3Storage>>) =
            match cfg.storage.backend {
                crate::config::StorageBackend::Local => (storage::from_config(&cfg.storage)?, None),
                crate::config::StorageBackend::S3 => {
                    let s3 = Arc::new(storage::S3Storage::new(cfg.storage.clone())?);
                    (s3.clone(), Some(s3))
                }
            };
        let mailer = mailer::from_config(&cfg.mailer)?;

        // Cheap checks first, in this order on purpose. The embedding model is a
        // 1.06 GB load (A2 pre-bakes it into the image precisely so a cold start
        // does not fetch it), so a misconfigured bucket or a missing pdfium path
        // must fail in seconds rather than after a multi-minute download. The
        // Phase 1 smoke run paid that cost once and it is the cost CI would pay
        // on every cold start.
        if let Some(s3) = &s3 {
            s3.ensure_bucket().await?;
        }
        documents::init(cfg.pdfium_lib_path.as_deref())?;

        let embedder: Arc<dyn Embedder> = match cfg.embedder.provider {
            EmbedderProvider::Fastembed => {
                Arc::new(embedder::FastembedEmbedder::new(&cfg.embedder)?)
            }
            EmbedderProvider::Http => {
                let base_url =
                    cfg.embedder.base_url.clone().ok_or_else(|| {
                        anyhow::anyhow!("EMBEDDER=http requires EMBEDDER_BASE_URL")
                    })?;
                Arc::new(embedder::HttpEmbedder::new(
                    base_url,
                    cfg.embedder
                        .api_key
                        .as_ref()
                        .map(|key| key.expose_secret().to_string()),
                    cfg.embedder.model.clone(),
                    cfg.embedder.dim,
                    cfg.embedder.query_prefix.clone(),
                ))
            }
        };
        let embedder: Arc<dyn Embedder> =
            Arc::new(embedder::DimensionChecked::new(embedder.clone()));

        embedder::assert_dimensions(pool, embedder.as_ref(), cfg.embedder.dim).await?;

        let llm: Option<Arc<dyn stvdy_core::traits::llm::LlmProvider>> = match &cfg.llm {
            Some(llm_config) => Some(Arc::new(llm::OpenAiCompatLlm::new(llm_config)?)),
            None => {
                tracing::warn!("LLM_BASE_URL is unset; chat and generation are unavailable");
                None
            }
        };

        Ok(Self {
            storage,
            embedder,
            mailer,
            clock: Arc::new(clock::SystemClock),
            s3,
            llm,
        })
    }
}
