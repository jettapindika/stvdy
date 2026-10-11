//! Liveness and readiness.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::error::AppError;
use crate::state::AppState;

/// Liveness plus a cheap readiness probe: the pool must answer.
pub async fn health(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    sqlx::query_scalar::<_, i32>("select 1")
        .fetch_one(&state.pool)
        .await
        .map_err(|e| AppError::Internal(anyhow::Error::from(e)))?;

    Ok(Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "env": if state.config.env.is_dev() { "dev" } else { "prod" },
    })))
}

pub async fn not_found() -> AppError {
    AppError::NotFound
}
