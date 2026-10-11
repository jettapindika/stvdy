//! Shared application state.

use std::sync::Arc;

use sqlx::PgPool;
use stvdy_core::traits::clock::Clock;
use stvdy_core::traits::embedder::Embedder;
use stvdy_core::traits::mailer::Mailer;
use stvdy_core::traits::storage::Storage;

use stvdy_core::traits::llm::LlmProvider;

use crate::config::AppConfig;
use crate::jobs::Queue;

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
    pub jobs: Queue,
}

impl AppState {
    pub fn plans(&self) -> &stvdy_core::config::PlansFile {
        &self.config.plans
    }
}
