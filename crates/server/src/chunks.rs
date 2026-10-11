//! Chunk reads: the server-side text a citation points at.
//!
//! `chunks` has no `user_id`, so ownership is enforced through the join to
//! `sources`; a chunk of a tombstoned source is not readable even though its
//! row may still exist, because the source is gone from the user's view.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::get;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/chunks/{id}", get(get_chunk))
}

#[derive(Debug, Serialize)]
pub struct ChunkView {
    pub id: Uuid,
    pub source_id: Uuid,
    pub page: Option<i32>,
    pub idx: i32,
    pub content: String,
}

/// `GET /chunks/:id`
pub async fn get_chunk(
    State(state): State<AppState>,
    user: AuthUser,
    Path(chunk_id): Path<Uuid>,
) -> AppResult<Json<ChunkView>> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, Option<i32>, i32, String)>(
        "select c.id, c.source_id, c.page, c.idx, c.content
           from chunks c
           join sources s on s.id = c.source_id
          where c.id = $1 and s.user_id = $2 and s.deleted_at is null",
    )
    .bind(chunk_id)
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a chunk: {error}")))?
    .ok_or(AppError::NotFound)?;

    let (id, source_id, page, idx, content) = row;
    Ok(Json(ChunkView {
        id,
        source_id,
        page,
        idx,
        content,
    }))
}
