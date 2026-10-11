//! Shared application state.

use std::sync::Arc;

use sqlx::PgPool;
use stvdy_core::traits::clock::Clock;
use stvdy_core::traits::embedder::Embedder;
use stvdy_core::traits::mailer::Mailer;
use stvdy_core::traits::storage::Storage;

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
    pub jobs: Queue,
}

impl AppState {
    pub fn plans(&self) -> &stvdy_core::config::PlansFile {
        &self.config.plans
    }
}
