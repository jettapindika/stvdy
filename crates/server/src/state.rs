//! Shared application state.

use std::sync::Arc;

use sqlx::PgPool;
use stvdy_core::traits::clock::Clock;
use stvdy_core::traits::embedder::Embedder;
use stvdy_core::traits::mailer::Mailer;
use stvdy_core::traits::storage::Storage;

use stvdy_core::traits::llm::LlmProvider;

use crate::config::AppConfig;
use crate::integrations::fetch::ChainFetcher;
use crate::integrations::search::composite::CompositeSearch;
use crate::jobs::Queue;
use crate::render::chrome::Renderer;

/// Provider handles and the job queue. Built once at startup by
/// `integrations::Providers::build` plus `jobs::spawn_worker`, so a handler
/// never has to reach for a global.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<AppConfig>,
    pub storage: Arc<dyn Storage>,
    pub embedder: Arc<dyn Embedder>,
    pub mailer: Arc<dyn Mailer>,
    pub clock: Arc<dyn Clock>,
    /// `None` when no model provider is configured: chat answers
    /// `provider_error` rather than failing at the first request with a
    /// transport error that looks like an outage.
    pub llm: Option<Arc<dyn LlmProvider>>,
    /// `None` only when the pinned tier is unconfigured. The composite itself
    /// needs no keys, so this is present on a default deployment.
    pub search: Option<Arc<CompositeSearch>>,
    /// The pooled Chromium, or `None` when no browser could be found.
    ///
    /// `None` is a supported state and not a degraded one: the only thing that needs a
    /// browser is the *print* step, and a cheatsheet without a PDF is still a ready
    /// document with its blocks stored (the re-render endpoint exists precisely so the
    /// print can be repeated later). A report's PDF export is the one route that answers
    /// `502 provider_error` without it, because there is nothing else it could answer.
    pub renderer: Option<Arc<Renderer>>,
    /// The page fetcher for a user-supplied URL.
    ///
    /// **Every** fetch of a URL a user gave us goes through this, never a bare
    /// `reqwest` call: the chain is what carries the SSRF guard (A18/A34), and a
    /// second fetch path would be a second place to forget it. Present on every
    /// deployment, because the chain's last step needs no key and no binary.
    pub fetcher: Arc<ChainFetcher>,
    pub jobs: Queue,
}

impl AppState {
    pub fn plans(&self) -> &stvdy_core::config::PlansFile {
        &self.config.plans
    }
}
