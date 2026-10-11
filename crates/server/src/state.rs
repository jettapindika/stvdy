//! Shared application state.

use std::sync::Arc;

use sqlx::PgPool;

use crate::config::AppConfig;

/// Provider handles (LLM, embedder, search, push, payments, storage) are added
/// here by the phases that introduce them.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<AppConfig>,
}
