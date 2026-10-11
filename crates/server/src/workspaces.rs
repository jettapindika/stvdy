//! Workspaces: one per course or subject, the container every source lives in.
//!
//! Deletes are tombstones (`deleted_at`) rather than `DELETE`, because a sync
//! client has to be able to tell "gone" from "never seen". The tombstone is the
//! whole subtree: a workspace delete also tombstones its sources in the same
//! transaction, so the source list can never outlive its workspace.
//!
//! Every read is scoped by `user_id` and answers 404 for another user's row.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use stvdy_core::entitlements::Feature;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces", get(list).post(create))
        .route(
            "/workspaces/{id}",
            get(get_one).patch(update).delete(delete_one),
        )
}

#[derive(Debug, Serialize)]
pub struct WorkspaceView {
    pub id: Uuid,
    pub name: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    /// Live sources only: a tombstoned source is not part of the workspace's
    /// visible contents, and the list screen shows this number next to the name.
    pub source_count: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceList {
    pub workspaces: Vec<WorkspaceView>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceResponse {
    pub workspace: WorkspaceView,
}

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
    created_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    source_count: i64,
}

impl From<WorkspaceRow> for WorkspaceView {
    fn from(row: WorkspaceRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
            source_count: row.source_count,
        }
    }
}

/// `GET /workspaces`
pub async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<WorkspaceList>> {
    let rows = sqlx::query_as::<_, WorkspaceRow>(
        "select w.id, w.name, w.created_at, w.updated_at,
                (select count(*) from sources s
                  where s.workspace_id = w.id and s.deleted_at is null) as source_count
           from workspaces w
          where w.user_id = $1 and w.deleted_at is null
          order by w.updated_at desc, w.id",
    )
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing workspaces: {error}")))?;

    let workspaces = rows.into_iter().map(WorkspaceView::from).collect();
    Ok(Json(WorkspaceList { workspaces }))
}

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    pub name: String,
    /// A8: a client may supply the id so it can create the row offline and sync
    /// it later without the server assigning a second one.
    #[serde(default)]
    pub id: Option<Uuid>,
}

/// `POST /workspaces`
///
/// `AuthUser`, not `VerifiedUser`: creating a workspace spends nothing and costs no
/// provider call, so A43 — which gates *spending* — does not apply. The sweep table
/// records that decision per route rather than applying it globally.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<WorkspaceResponse>)> {
    let name = validate_name(&body.name)?;

    crate::entitlements::require(&state, &user, Feature::CreateWorkspace).await?;

    // `on conflict do nothing` turns a duplicate client id into an empty result
    // rather than a 500, and does it without a check-then-insert race.
    let row = sqlx::query_as::<_, (Uuid, String, Option<DateTime<Utc>>, DateTime<Utc>)>(
        "insert into workspaces (id, user_id, name)
         values (coalesce($1, gen_random_uuid()), $2, $3)
         on conflict (id) do nothing
         returning id, name, created_at, updated_at",
    )
    .bind(body.id)
    .bind(user.user_id)
    .bind(name)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("creating a workspace: {error}")))?;

    let Some((id, name, created_at, updated_at)) = row else {
        return Err(AppError::Conflict(
            "a workspace with that id already exists".to_string(),
        ));
    };

    Ok((
        StatusCode::CREATED,
        Json(WorkspaceResponse {
            workspace: WorkspaceView {
                id,
                name,
                created_at,
                updated_at,
                source_count: 0,
            },
        }),
    ))
}

/// `GET /workspaces/:id`
pub async fn get_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<WorkspaceResponse>> {
    let row = fetch_workspace(&state, user.user_id, workspace_id).await?;
    Ok(Json(WorkspaceResponse {
        workspace: row.into(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub name: String,
}

/// `PATCH /workspaces/:id`
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<UpdateRequest>,
) -> AppResult<Json<WorkspaceResponse>> {
    let name = validate_name(&body.name)?;

    let result = sqlx::query(
        "update workspaces set name = $3, updated_at = now()
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(workspace_id)
    .bind(user.user_id)
    .bind(name)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("renaming a workspace: {error}")))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    let row = fetch_workspace(&state, user.user_id, workspace_id).await?;
    Ok(Json(WorkspaceResponse {
        workspace: row.into(),
    }))
}

/// `DELETE /workspaces/:id`. Tombstones the workspace and its sources together:
/// the source list is keyed by workspace, so leaving its rows live would let a
/// deleted workspace's material resurface through a source endpoint.
pub async fn delete_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a transaction: {error}"))
        })?;

    let row = sqlx::query_as::<_, (Uuid,)>(
        "update workspaces set deleted_at = now(), updated_at = now()
          where id = $1 and user_id = $2 and deleted_at is null
      returning id",
    )
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting a workspace: {error}")))?;

    let Some((deleted_id,)) = row else {
        return Err(AppError::NotFound);
    };

    // Collect the source ids first so their chunks and objects can be reclaimed.
    // Tombstoning the sources without this frees the user's storage *quota*
    // (`enforce_storage` sums `file_size` over live sources) while the embeddings
    // and the objects stay forever, which a delete loop could churn.
    let source_ids: Vec<Uuid> =
        sqlx::query_scalar("select id from sources where workspace_id = $1 and deleted_at is null")
            .bind(deleted_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!(
                    "listing a workspace's sources for deletion: {error}"
                ))
            })?;

    sqlx::query(
        "update sources set deleted_at = now(), updated_at = now()
          where workspace_id = $1 and deleted_at is null",
    )
    .bind(deleted_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "tombstoning a workspace's sources: {error}"
        ))
    })?;

    // Inside the transaction, so the chunk count and the quota cannot disagree.
    let keys = crate::sources::routes::reclaim_chunks(&mut *tx, &source_ids)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("dropping a workspace's chunks: {error}"))
        })?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a delete: {error}")))?;

    // After the commit: a storage failure must not roll back a deletion the user
    // asked for.
    crate::sources::routes::delete_objects(&state, &keys).await;

    Ok(StatusCode::NO_CONTENT)
}

/// The workspace's name, or 404 when it is not this user's.
///
/// Every workspace-scoped route needs this, and a second copy is how one of them
/// eventually forgets the `user_id` predicate, which is a cross-tenant read.
pub async fn owned_name(state: &AppState, user_id: Uuid, workspace_id: Uuid) -> AppResult<String> {
    let name: Option<String> = sqlx::query_scalar(
        "select name from workspaces
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the workspace: {error}")))?;

    name.ok_or(AppError::NotFound)
}

async fn fetch_workspace(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<WorkspaceRow> {
    // The same correlated-subquery shape as `list`, so a freshly created or
    // renamed workspace reports the same `source_count` the list does.
    sqlx::query_as::<_, WorkspaceRow>(
        "select w.id, w.name, w.created_at, w.updated_at,
                (select count(*) from sources s
                  where s.workspace_id = w.id and s.deleted_at is null) as source_count
           from workspaces w
          where w.id = $1 and w.user_id = $2 and w.deleted_at is null",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a workspace: {error}")))?
    .ok_or(AppError::NotFound)
}

fn validate_name(raw: &str) -> AppResult<&str> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AppError::Validation(
            "workspace name cannot be empty".to_string(),
        ));
    }
    Ok(name)
}
