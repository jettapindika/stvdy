//! `GET /sync/pull`: the caller's own change log, page by page.
//!
//! Every read on this path is scoped by `user_id` **in the same statement that reads
//! it** — never by a filter applied afterwards. That is this phase's answer to its
//! named risk ("the phase most likely to leak cross-tenant data"): a `where` clause a
//! later edit can move into Rust is a bug waiting to happen, while a `where user_id =
//! $1` in the statement cannot be forgotten by the caller.

use std::collections::HashMap;
use std::time::Instant;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgConnection;
use stvdy_sync::proto::{Change, CursorKind, Entity, Op, PullQuery, PullResponse, is_stale};
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::touch_last_sync;

/// The three optional query parameters. `cursor` and `offset` are validated together by
/// [`PullQuery::parse`], so this struct is only the wire shape.
#[derive(Debug, Deserialize)]
pub struct PullParams {
    cursor: Option<i64>,
    offset: Option<i64>,
    limit: Option<i64>,
}

/// One page of `change_log` rows, before the payloads are loaded.
pub struct Page {
    /// `data` is filled by the handler once the payloads are read.
    pub changes: Vec<Change>,
    /// The cursor the client should send next: the pass's anchor on an initial page,
    /// the last seq on an incremental one.
    pub next_cursor: i64,
    pub has_more: bool,
}

/// `GET /sync/pull`.
pub async fn pull(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<PullParams>,
) -> AppResult<Json<PullResponse>> {
    let started = Instant::now();
    let query = PullQuery::parse(params.cursor, params.offset, params.limit)
        .map_err(|error| AppError::Validation(error.to_string()))?;

    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning the pull transaction: {error}"))
    })?;

    // A cursor older than retained history is told to resync from zero (A165/A166). The
    // comparison is against the per-user watermark, never `min(seq)`: seq is global, so
    // a new account's first change can be seq 500 and `min(seq)` would 410 its very
    // first request.
    let watermark = min_retained_seq(&mut tx, user.user_id).await?;
    if is_stale(query.cursor, watermark) {
        return Err(AppError::ResyncRequired);
    }

    let mut page = load_page(&mut tx, user.user_id, &query).await?;

    // One payload load per entity present in the page. Tombstones carry no data, and an
    // `upsert` whose row is gone loads nothing and is emitted as `data: null`, which the
    // client applies as a delete (A188) — so a `change_log` row whose `entity_id` is
    // another account's can never become a cross-tenant read.
    let mut ids_by_entity: HashMap<Entity, Vec<Uuid>> = HashMap::new();
    for change in &page.changes {
        if change.op == Op::Upsert {
            ids_by_entity
                .entry(change.entity)
                .or_default()
                .push(change.id);
        }
    }
    let mut payloads: HashMap<(Entity, Uuid), Value> = HashMap::new();
    for (entity, ids) in &ids_by_entity {
        for (id, payload) in load_payload(&mut tx, user.user_id, *entity, ids).await? {
            payloads.insert((*entity, id), payload);
        }
    }
    for change in &mut page.changes {
        if change.op == Op::Upsert {
            change.data = payloads.remove(&(change.entity, change.id));
        }
    }

    touch_last_sync(&mut tx, user.device_id)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("recording the last sync: {error}")))?;
    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing the pull transaction: {error}"))
    })?;

    // The roadmap requires sync pull latency to be logged; one line with the facts that
    // matter for a slow or an empty pull.
    tracing::info!(
        user_id = %user.user_id,
        device_id = %user.device_id,
        rows = page.changes.len(),
        cursor = query.cursor,
        has_more = page.has_more,
        latency_ms = started.elapsed().as_millis() as u64,
        "sync pull"
    );

    Ok(Json(PullResponse {
        changes: page.changes,
        next_cursor: page.next_cursor,
        has_more: page.has_more,
    }))
}

/// The page of `change_log` rows this cursor asks for.
///
/// `Initial` anchors at the pass's `max(seq)` so the pass set cannot change under it
/// (A164); `Incremental` walks `seq > cursor` ascending. Both are scoped by `user_id` in
/// the same statement — never filtered afterwards — because that is this phase's named
/// risk.
pub async fn load_page(conn: &mut PgConnection, user_id: Uuid, q: &PullQuery) -> AppResult<Page> {
    match q.kind() {
        CursorKind::Initial { offset } => load_initial(conn, user_id, q, offset).await,
        CursorKind::Incremental { after } => load_incremental(conn, user_id, q, after).await,
    }
}

/// A page of the initial pass, ordered so a parent lands before its child.
///
/// The rank expression is built from [`Entity::dependency_rank`] rather than written
/// out here, so the SQL order and the wire order cannot become two lists.
async fn load_initial(
    conn: &mut PgConnection,
    user_id: Uuid,
    q: &PullQuery,
    offset: i64,
) -> AppResult<Page> {
    // The anchor bounds the pass set. `None` means the first page of a pass, so the
    // anchor is the account's current `max(seq)`; any other value is the anchor a
    // previous page returned, which is what makes every later page see the same set.
    let anchor = match q.anchor() {
        Some(anchor) => anchor,
        None => max_seq(conn, user_id).await?,
    };

    let rank = rank_expression();
    let sql = format!(
        "select seq, entity, entity_id, op from change_log
          where user_id = $1 and seq <= $2
          order by {rank}, seq
          offset $3 limit $4"
    );
    // `limit + 1` so a full page can be told from the last page without a second query.
    // The SQL is assembled from `Entity`'s own table names and ranks — no caller input
    // reaches it — which is what `AssertSqlSafe` asserts.
    let rows: Vec<(i64, String, Uuid, String)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .bind(anchor)
        .bind(offset)
        .bind(q.limit + 1)
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading an initial page: {error}")))?;

    page_from_rows(rows, anchor, q.limit)
}

/// A page of the incremental walk: `seq > cursor`, ascending.
async fn load_incremental(
    conn: &mut PgConnection,
    user_id: Uuid,
    q: &PullQuery,
    after: i64,
) -> AppResult<Page> {
    let rows: Vec<(i64, String, Uuid, String)> = sqlx::query_as(
        "select seq, entity, entity_id, op from change_log
          where user_id = $1 and seq > $2
          order by seq
          limit $3",
    )
    .bind(user_id)
    .bind(after)
    .bind(q.limit + 1)
    .fetch_all(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading an incremental page: {error}")))?;

    // On an empty page the cursor stays where it was, so a client polling a quiet
    // account does not jump its cursor to an unrelated seq.
    let next_cursor = rows.last().map(|row| row.0).unwrap_or(after);
    page_from_rows(rows, next_cursor, q.limit)
}

/// Turns the fetched rows into a page, using the extra row to decide `has_more`.
///
/// An entity or an op the protocol does not know is a 500, never a skipped row (A169):
/// it can only mean the trigger array and `Entity::ALL` have drifted, and a silent skip
/// would be permanent loss on every device.
fn page_from_rows(
    mut rows: Vec<(i64, String, Uuid, String)>,
    next_cursor: i64,
    limit: i64,
) -> AppResult<Page> {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let mut changes = Vec::with_capacity(rows.len());
    for (seq, entity, id, op) in rows {
        let entity = Entity::from_table_name(&entity).ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "change_log holds an entity the protocol does not know: {entity}"
            ))
        })?;
        let op = match op.as_str() {
            "upsert" => Op::Upsert,
            "delete" => Op::Delete,
            other => {
                return Err(AppError::Internal(anyhow::anyhow!(
                    "change_log holds an op the protocol does not know: {other}"
                )));
            }
        };
        changes.push(Change {
            seq,
            entity,
            id,
            op,
            data: None,
        });
    }
    Ok(Page {
        changes,
        next_cursor,
        has_more,
    })
}

/// The current row for each id, restricted to the entity's payload allowlist (A168).
///
/// `where user_id = $1 and id = any($2)` is the whole defence: an id that belongs to
/// another account loads nothing, and the caller emits `data: null` (A188).
pub async fn load_payload(
    conn: &mut PgConnection,
    user_id: Uuid,
    entity: Entity,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Value>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    // A positive column list built from the allowlist, so a column added to a table and
    // not to the allowlist is simply not shipped — the safe direction. `jsonb_build_object`
    // keeps every column in one jsonb value, which decodes straight into the wire `data`.
    let pairs = entity
        .payload_columns()
        .iter()
        .map(|column| format!("'{column}', t.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let table = entity.table_name();

    // The SQL names a table and its allowlisted columns, all from `Entity` — no caller
    // input reaches it — which is what `AssertSqlSafe` asserts.
    let rows: Vec<(Uuid, Value)> = if entity == Entity::UserSettings {
        // Keyed by user_id: the row *is* the account, and that is what
        // `log_change_by_user` writes into `change_log.entity_id` (A179).
        let sql = format!(
            "select t.user_id as key, jsonb_build_object({pairs}) as payload
               from {table} t where t.user_id = $1"
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .fetch_all(&mut *conn)
            .await
    } else {
        let sql = format!(
            "select t.id as key, jsonb_build_object({pairs}) as payload
               from {table} t where t.user_id = $1 and t.id = any($2)"
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .bind(ids)
            .fetch_all(&mut *conn)
            .await
    }
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a payload: {error}")))?;

    Ok(rows.into_iter().collect())
}

/// The per-user watermark compaction writes, or 0 when nothing has been compacted.
pub async fn min_retained_seq(conn: &mut PgConnection, user_id: Uuid) -> AppResult<i64> {
    sqlx::query_scalar(
        "select coalesce(
                  (select min_retained_seq from change_log_watermark where user_id = $1), 0)",
    )
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("reading the compaction watermark: {error}"))
    })
}

/// The account's current `max(seq)`, or 0 when it has no changes.
async fn max_seq(conn: &mut PgConnection, user_id: Uuid) -> AppResult<i64> {
    sqlx::query_scalar("select coalesce(max(seq), 0) from change_log where user_id = $1")
        .bind(user_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the account's seq: {error}")))
}

/// The `case entity … end` that turns a table name into its dependency rank.
///
/// Built from [`Entity::dependency_rank`] so the SQL order and the wire rank are one
/// list. An unknown entity falls to the end; it can only reach here if the trigger array
/// and `Entity::ALL` have drifted, which `crates/server/tests/schema.rs` makes
/// impossible.
fn rank_expression() -> String {
    let arms = Entity::ALL
        .iter()
        .map(|entity| {
            format!(
                "when '{}' then {}",
                entity.table_name(),
                entity.dependency_rank()
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("case entity {arms} else 999 end")
}
