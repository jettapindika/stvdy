//! First implementations of the provider traits (roadmap § Cross-Cutting
//! Contracts § 1). Each is selected by configuration, so a provider is a
//! deployment decision rather than a code change.

pub mod clock;
pub mod documents;
pub mod embedder;
pub mod fetch;
pub mod llm;
pub mod mailer;
pub mod payment;
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
    /// `None` only when the pinned tier is unconfigured. The composite needs no
    /// keys, so a default deployment has one.
    pub search: Option<Arc<search::CompositeSearch>>,
    /// The pooled Chromium, or `None` when none could be found or launched.
    pub renderer: Option<Arc<crate::render::chrome::Renderer>>,
    /// The page-fetch chain, for a user-supplied URL (A18/A34).
    ///
    /// Always present: the chain's last step is plain `reqwest`, which needs no
    /// key and no binary, so there is no deployment where a URL source has nowhere
    /// to go. A test can replace it (see `testing::TestState::with_fetcher`), which
    /// is how the URL path is exercised without a network.
    pub fetcher: Arc<fetch::ChainFetcher>,
    /// The raw-text fetch for a user-supplied ICS URL (A233).
    ///
    /// Always present and needs no configuration: it is the same guard as
    /// [`fetcher`](Self::fetcher) with no readability pass. A test replaces it with a fake
    /// (see `testing::TestState::with_ics_fetch`), because the real guard is exactly what
    /// refuses the local stub a route test would want to point at.
    pub ics_fetch: Arc<dyn crate::calendar::ics_in::IcsFetch>,
    /// The payment gateway (Phase 13).
    ///
    /// `None` when `QRIS_BASE_URL` is unset — a supported deployment with nothing to
    /// sell, like [`renderer`](Self::renderer) and [`llm`](Self::llm). It is
    /// **not** a mock: a deployment with no gateway must answer `provider_error`
    /// rather than mint an invoice against a double, because a double's QR string is
    /// not money and its callback signature is one we chose ourselves.
    pub payment: Option<Arc<dyn payment::PaymentGateway>>,
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

        // Built here rather than lazily so a misconfigured `SEARCH_PROVIDER` or a
        // missing key for a pinned tier is a startup failure, not a first-query
        // surprise. `None` means the pinned tier is unconfigured, which the config
        // loader already refuses, so in practice this is present.
        let search = search::CompositeSearch::new(&cfg.search, pool.clone()).map(Arc::new);
        if let Some(composite) = &search {
            tracing::info!(tiers = ?composite.tier_names(), "search composite ready");
        }

        // The pooled Chromium, launched at boot so a deployment that cannot render finds
        // out at startup rather than when a student asks for a PDF. `None` is a supported
        // state: the print step is the only thing that needs a browser, and a cheatsheet
        // whose blocks are stored can be re-rendered later.
        //
        // The sandbox is disabled only in dev, and only because a container without the
        // user-namespace support Chromium's sandbox needs is a deployment fact rather than
        // a preference. Outside dev an unsandboxed render is refused outright: the browser
        // is rendering model-influenced HTML.
        //
        // The renderer is given the origin its print assets are served from, because the
        // template's own references point at a CDN and a PDF must not depend on one being
        // reachable (A339). It is the loopback origin by default, so a single container
        // needs no configuration.
        let renderer = match crate::render::chrome::find_executable() {
            Some(executable) => {
                let allow_no_sandbox = cfg.env.is_dev();
                match crate::render::chrome::Renderer::launch(
                    Some(&executable),
                    allow_no_sandbox,
                    Some(&cfg.assets_base_url),
                ) {
                    Ok(renderer) => {
                        tracing::info!(%executable, "chromium renderer ready");
                        Some(Arc::new(renderer))
                    }
                    Err(error) => {
                        tracing::error!(%error, %executable, "chromium did not launch; PDF rendering is unavailable");
                        None
                    }
                }
            }
            None => {
                tracing::warn!("no Chromium found (set CHROME_PATH); PDF rendering is unavailable");
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
            search,
            renderer,
            fetcher: Arc::new(fetch::ChainFetcher::from_config(
                cfg.search
                    .exa_api_key
                    .as_ref()
                    .map(|key| key.expose_secret().to_string()),
            )?),
            ics_fetch: Arc::new(crate::calendar::ics_in::GuardedFetch),
            payment: payment(cfg)?,
        })
    }
}

/// The payment gateway, or `None` when no base URL is configured.
///
/// Built at boot rather than lazily so a gateway whose client cannot be constructed is
/// a startup failure, not a checkout-time surprise. `None` is a supported deployment:
/// checkout answers 502 `provider_error` and every other route is unaffected, which is
/// the same shape the renderer and the model provider take (A130).
fn payment(cfg: &AppConfig) -> anyhow::Result<Option<Arc<dyn payment::PaymentGateway>>> {
    let Some(qris) = &cfg.qris else {
        tracing::warn!("QRIS_BASE_URL is unset; checkout is unavailable");
        return Ok(None);
    };

    let gateway = payment::QrisGateway::from_config(qris)?;
    tracing::info!(
        base_url = %qris.base_url,
        scheme = qris.signature_scheme.as_str(),
        fee_percent = qris.fee_percent,
        "payment gateway ready"
    );
    Ok(Some(Arc::new(gateway)))
}
