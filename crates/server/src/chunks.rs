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
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use stvdy_core::prompts::PromptChunk;

/// How many chunks one generation prompt may carry.
///
/// A prompt has a token budget, and the same sources must produce the same prompt
/// twice, so the selection is deterministic as well as bounded.
pub const GENERATION_CHUNK_LIMIT: i64 = 40;

/// The chunks a generation prompt is built from.
///
/// Round-robin across the selected sources: rank each chunk within its source, then
/// take rank 1 of every source, then rank 2 of every source, and so on. A plain
/// `order by source_id, idx limit 40` spends the whole budget on the source whose id
/// sorts first, so a generation built from several sources silently ignores all but one
/// of them whenever the first has more than `GENERATION_CHUNK_LIMIT` chunks, which is
/// the ordinary case for a textbook. Ordering by `(rank, source_id)` keeps the result
/// deterministic, so the same sources still produce the same prompt twice.
///
/// Shared by the deck and the quiz rather than copied: this selection was itself a bug
/// fix, and a second copy would be a second chance to reintroduce it. The caller maps
/// the error to its own type (a job's `Failure`, or a route's `AppError`).
pub async fn select_for_generation(
    pool: &PgPool,
    source_ids: &[Uuid],
    user_id: Uuid,
) -> Result<Vec<PromptChunk>, sqlx::Error> {
    let rows = sqlx::query(
        "select id, source_id, page, content, title
           from (
                select c.id, c.source_id, c.page, c.content, s.title,
                       row_number() over (partition by c.source_id order by c.idx) as rank
                  from chunks c
                  join sources s on s.id = c.source_id
                 where c.source_id = any($1) and s.user_id = $2
           ) ranked
          order by ranked.rank, ranked.source_id
          limit $3",
    )
    .bind(source_ids)
    .bind(user_id)
    .bind(GENERATION_CHUNK_LIMIT)
    .fetch_all(pool)
    .await?;

    // The label is the index plus one, which is what the composer writes into the
    // prompt and what the label resolver maps back.
    Ok(rows
        .iter()
        .enumerate()
        .map(|(index, row)| PromptChunk {
            label: index as u32 + 1,
            chunk_id: row.get("id"),
            source_id: row.get("source_id"),
            source_title: row.get("title"),
            page: row.get("page"),
            content: row.get("content"),
        })
        .collect())
}

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
    user: VerifiedUser,
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
