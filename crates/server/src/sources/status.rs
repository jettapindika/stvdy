//! `GET /sources/:id/status`: what the client polls while ingestion runs.
//!
//! Progress is a status plus a chunk count rather than a percentage: extraction
//! and embedding have no meaningful intermediate steps, and a fabricated
//! percentage is a worse experience than an honest "processing".
//!
//! The route is registered by `sources::routes`, which owns the source paths.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct StatusView {
    pub id: Uuid,
    pub status: String,
    pub chunk_count: i64,
    pub error: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// `GET /sources/:id/status`
pub async fn status(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(source_id): Path<Uuid>,
) -> AppResult<Json<StatusView>> {
    let row = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<DateTime<Utc>>, i64)>(
        "select s.id, s.status, s.error, s.updated_at,
                (select count(*) from chunks c where c.source_id = s.id) as chunk_count
           from sources s
          where s.id = $1 and s.user_id = $2 and s.deleted_at is null",
    )
    .bind(source_id)
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a source status: {error}")))?
    .ok_or(AppError::NotFound)?;

    let (id, status, error, updated_at, chunk_count) = row;
    Ok(Json(StatusView {
        id,
        status,
        chunk_count,
        error,
        updated_at,
    }))
}
