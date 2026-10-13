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
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use stvdy_core::entitlements::Feature;

use crate::auth::extract::VerifiedUser;
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
    /// The course code the student gave this course (`BIO 201`), or null. Optional
    /// by design: every surface that shows it falls back to the name, so an
    /// account that never sets one is complete rather than half-filled.
    pub course_code: Option<String>,
    /// The course's hue, 0-5, indexing the six categorical course colours. The
    /// server assigns it round-robin on create so two adjacent courses differ
    /// without the student choosing anything.
    pub color: i16,
    /// The next exam, as a date. A `NaiveDate` rather than an instant: an exam is
    /// on a day in the reader's calendar, and a `timestamptz` would render "5
    /// November" as the 4th for a Jakarta student.
    pub exam_date: Option<NaiveDate>,
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
    course_code: Option<String>,
    color: i16,
    exam_date: Option<NaiveDate>,
}

impl From<WorkspaceRow> for WorkspaceView {
    fn from(row: WorkspaceRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
            source_count: row.source_count,
            course_code: row.course_code,
            color: row.color,
            exam_date: row.exam_date,
        }
    }
}

/// `GET /workspaces`
///
/// **The column list below is repeated in `get_one`/`fetch_workspace` and in the
/// `create` read-back, and the three must stay identical.** sqlx 0.9's `SqlSafeStr`
/// bound accepts only literal SQL, so the list cannot be a `const` interpolated
/// into the statement; a shared string would have to be `AssertSqlSafe`'d, which
/// disables exactly the check that keeps a caller from reaching the query text.
/// The three copies are the price, and they are three lines apart in this file.
pub async fn list(
    State(state): State<AppState>,
    user: VerifiedUser,
) -> AppResult<Json<WorkspaceList>> {
    let rows = sqlx::query_as::<_, WorkspaceRow>(
        "select w.id, w.name, w.created_at, w.updated_at, w.course_code, w.color, w.exam_date,
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
    /// Optional, like every other course field: a workspace created from a quick
    /// capture has no code, and one created from a course form does.
    #[serde(default)]
    pub course_code: Option<String>,
    #[serde(default)]
    pub exam_date: Option<NaiveDate>,
}

/// `POST /workspaces`
///
/// `VerifiedUser` (A359). This route is the one that used to argue the other way — creating a
/// workspace spends nothing and costs no provider call, so A43's spending rule did not reach
/// it — and that argument was the hole: an unverified account could hold a workspace and use
/// every read route around it. The gate is now on the surface, not the price, so the workspace
/// is where it starts. `route_sweep.rs` carries the classification for every route rather than
/// applying the rule globally: the account's own administrative routes are still `AuthUser`.
pub async fn create(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<CreateRequest>,
) -> AppResult<(StatusCode, Json<WorkspaceResponse>)> {
    let name = validate_name(&body.name)?;
    let course_code = validate_course_code(body.course_code.as_deref())?;

    crate::entitlements::require(&state, &user, Feature::CreateWorkspace).await?;

    // The hue is assigned here rather than by the client, so two courses created
    // by two devices do not both pick the first colour. `count(*) % 6` over the
    // account's live workspaces is the ordinal of the row being inserted, and the
    // subquery is evaluated in the same statement as the insert so a concurrent
    // create cannot interleave between the count and the write.
    //
    // `on conflict do nothing` turns a duplicate client id into an empty result
    // rather than a 500, and does it without a check-then-insert race.
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "with hue as (
           select (count(*) % 6)::smallint as color
             from workspaces
            where user_id = $2 and deleted_at is null
         ), inserted as (
           insert into workspaces (id, user_id, name, course_code, exam_date, color)
           select coalesce($1, gen_random_uuid()), $2, $3, $4, $5, hue.color from hue
           on conflict (id) do nothing
           returning id, name, created_at, updated_at, course_code, color, exam_date
         )
         select i.id, i.name, i.created_at, i.updated_at, i.course_code, i.color,
                i.exam_date, 0::bigint as source_count
           from inserted i",
    )
    .bind(body.id)
    .bind(user.user_id)
    .bind(name)
    .bind(course_code)
    .bind(body.exam_date)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("creating a workspace: {error}")))?;

    let Some(row) = row else {
        return Err(AppError::Conflict(
            "a workspace with that id already exists".to_string(),
        ));
    };

    Ok((
        StatusCode::CREATED,
        Json(WorkspaceResponse {
            workspace: row.into(),
        }),
    ))
}

/// `GET /workspaces/:id`
pub async fn get_one(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<WorkspaceResponse>> {
    let row = fetch_workspace(&state, user.user_id, workspace_id).await?;
    Ok(Json(WorkspaceResponse {
        workspace: row.into(),
    }))
}

/// The fields a `PATCH` may change.
///
/// Every field is `Option` and every absent field is left alone, so a caller that
/// only renames a workspace does not silently clear its course code. `null` is
/// therefore meaningful and distinct from absent: it clears the field.
#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub name: String,
    #[serde(default, deserialize_with = "double_option")]
    pub course_code: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub exam_date: Option<Option<NaiveDate>>,
}

/// Distinguishes an absent JSON field from an explicit `null`.
///
/// Serde maps both to `None` for `Option<T>`, which would make "clear the exam
/// date" indistinguishable from "do not touch it" — so a rename would either wipe
/// the date or make it impossible to remove. This wrapper is the standard fix: the
/// outer `Option` is presence, the inner is the value.
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `PATCH /workspaces/:id`
///
/// A workspace the plan has put over the workspace limit is refused: read-only means the
/// row keeps every byte and stays readable and exportable, not that it keeps accepting
/// writes. Deleting it is still allowed — that is the remedy the refusal names.
pub async fn update(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<UpdateRequest>,
) -> AppResult<Json<WorkspaceResponse>> {
    let name = validate_name(&body.name)?;
    let course_code = match &body.course_code {
        Some(value) => validate_course_code(value.as_deref())?.map(str::to_string),
        None => None,
    };
    let set_course_code = body.course_code.is_some();
    let set_exam_date = body.exam_date.is_some();
    let exam_date = body.exam_date.flatten();

    crate::entitlements::require_writable_row(
        &state,
        &user,
        crate::entitlements::ReadOnlyRow::Workspace,
        workspace_id,
    )
    .await?;

    // `coalesce($4, course_code)` with a paired "was it sent" flag, rather than
    // building the statement from strings: the flag is a bound boolean, so the SQL
    // is a constant and there is no path where a caller controls the query text.
    let result = sqlx::query(
        "update workspaces
            set name = $3,
                course_code = case when $4 then $5 else course_code end,
                exam_date = case when $6 then $7 else exam_date end,
                updated_at = now()
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(workspace_id)
    .bind(user.user_id)
    .bind(name)
    .bind(set_course_code)
    .bind(course_code)
    .bind(set_exam_date)
    .bind(exam_date)
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
    user: VerifiedUser,
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
    // The same column list as `list`, so a freshly created or renamed workspace
    // reports the same numbers the list does.
    sqlx::query_as::<_, WorkspaceRow>(
        "select w.id, w.name, w.created_at, w.updated_at, w.course_code, w.color, w.exam_date,
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

/// A course code, trimmed and capped.
///
/// The cap is the column's (`workspaces_course_code_len`), and it is checked here
/// as well so the caller gets a `validation` error naming the field rather than a
/// database constraint violation surfaced as an internal error. An empty string is
/// treated as absent: a form that submits an untouched optional field should clear
/// the code, not store a blank one that renders as a gap.
fn validate_course_code(raw: Option<&str>) -> AppResult<Option<&str>> {
    let Some(value) = raw else { return Ok(None) };
    let code = value.trim();
    if code.is_empty() {
        return Ok(None);
    }
    if code.chars().count() > 16 {
        return Err(AppError::Validation(
            "course code cannot be longer than 16 characters".to_string(),
        ));
    }
    Ok(Some(code))
}
