//! The ICS import: `POST /calendar/ics`, the connection list, and the delete.
//!
//! The fetch is behind [`IcsFetch`] rather than called directly, and that seam is not
//! ceremony: the guard this URL must pass is *exactly* what refuses a local stub, so a test
//! that pointed the real fetcher at `http://127.0.0.1:…` and passed would be proving the
//! guard **absent** (A233). The refusal itself is asserted against `fetch_guarded_text`
//! directly in `crates/server/tests/ssrf.rs`, where a refusal is a success.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use stvdy_core::traits::search::FetchError;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::integrations::fetch::{GuardedText, Validators, fetch_guarded_text};
use crate::state::AppState;

/// What an ICS document is asked for as.
///
/// `text/calendar` first because it is the right answer, then the two things a faculty web
/// server actually sends for one (a `text/plain` dump and a generic octet stream). A server
/// that negotiates strictly would answer `406` to `text/calendar` alone, and a 406 on a feed
/// that opens fine in a browser is a support ticket with no visible cause.
const ICS_ACCEPT: &str = "text/calendar, text/plain;q=0.9, */*;q=0.8";

/// The fetch seam for a user-supplied ICS URL (A233).
#[async_trait::async_trait]
pub trait IcsFetch: Send + Sync {
    /// A conditional GET of `url`. A `304` is `Ok` with [`GuardedText::not_modified`] set.
    async fn get(
        &self,
        url: &str,
        validators: Option<&Validators>,
    ) -> Result<GuardedText, FetchError>;
}

/// The real one: the SSRF guard, and no readability pass.
pub struct GuardedFetch;

#[async_trait::async_trait]
impl IcsFetch for GuardedFetch {
    async fn get(
        &self,
        url: &str,
        validators: Option<&Validators>,
    ) -> Result<GuardedText, FetchError> {
        fetch_guarded_text(url, ICS_ACCEPT, validators).await
    }
}

#[derive(Debug, Deserialize)]
pub struct AddIcsRequest {
    pub url: String,
}

/// `POST /calendar/ics`. Validates the URL (A246), inserts a `provider = 'ics'` connection,
/// and enqueues the first `sync_calendar` (A238).
///
/// The URL is checked **before** the row is written, and checked again at fetch time by the
/// guard. The two do different jobs: this one is for the user — "that is not a URL we can
/// fetch" beats a connection that silently never syncs — while the fetch-time guard is the
/// protection, because DNS can change between the two.
///
/// More than one ICS connection per account is allowed, because a student can have two
/// faculties. The **same URL twice** is not, and is a 409 rather than a silent second
/// connection: two rows for one feed would import every class twice, and the student would
/// have no way to tell which row to delete.
pub async fn add(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<AddIcsRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let url = body.url.trim();
    // The same check ingestion uses (A18/A34), so a non-http scheme, an embedded credential
    // and a literal private address are refused here with the same wording as everywhere else
    // (A246). It does not resolve a hostname: DNS can change between this and the fetch, which
    // is why the fetch path guards again.
    crate::integrations::fetch::check_fetchable_literal(url)
        .map_err(|error| AppError::Validation(error.to_string()))?;

    let id: Uuid = sqlx::query_scalar(
        "insert into calendar_connections (user_id, provider, ics_url)
         select $1, 'ics', $2
          where not exists (
                select 1 from calendar_connections
                 where user_id = $1 and provider = 'ics' and ics_url = $2)
         returning id",
    )
    .bind(user.user_id)
    .bind(url)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("adding an ICS connection: {error}")))?
    .ok_or_else(|| AppError::Conflict("that ICS URL is already connected".to_string()))?;

    crate::jobs::schedule::enqueue_connection_sync(&state, user.user_id, id).await?;

    let connection = crate::calendar::sync::load(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    tracing::info!(user_id = %user.user_id, connection_id = %id, "ICS connection added");

    Ok((
        StatusCode::CREATED,
        Json(json!({ "connection": crate::calendar::sync::view(&connection) })),
    ))
}

/// `GET /calendar/connections` (A249). **Never** returns a token of any kind.
///
/// The spec lists connect/callback/ics/delete/sync/regenerate/feed and no list, so without
/// this a connection made in one page load is invisible in the next and the settings screen
/// cannot show its status — the same reasoning as A81.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let connections = crate::calendar::sync::load_for_user(&state.pool, user.user_id).await?;
    let views: Vec<Value> = connections
        .iter()
        .map(crate::calendar::sync::view)
        .collect();
    Ok(Json(json!({ "connections": views })))
}

/// `DELETE /calendar/connections/{id}`.
///
/// The connection's events are **tombstoned in the same transaction**, and that is the part a
/// plausible implementation gets wrong: `events.connection_id` is
/// `references calendar_connections on delete set null`, so deleting the connection alone
/// leaves orphan `source = 'ics'` rows that no sync will ever touch again and that a client
/// cannot edit or delete either (they are read-only by A198). The student would be stuck with
/// a timetable they cannot remove.
///
/// A tombstone rather than a hard delete, because `events` is `+sync`: a row removed outright
/// leaves every device showing the event forever, with no change-log entry to correct it.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a connection delete: {error}"))
    })?;

    let owned: Option<String> = sqlx::query_scalar(
        "select provider from calendar_connections where id = $1 and user_id = $2 for update",
    )
    .bind(id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a connection: {error}")))?;

    if owned.is_none() {
        return Err(AppError::NotFound);
    }

    // `updated_at` is touched by the trigger, which is also what makes the tombstone reach a
    // device with a `SEQUENCE` that moved.
    sqlx::query(
        "update events set deleted_at = now()
          where connection_id = $1 and deleted_at is null",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("removing a connection's events: {error}"))
    })?;

    sqlx::query("delete from calendar_connections where id = $1 and user_id = $2")
        .bind(id)
        .bind(user.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting a connection: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a connection delete: {error}"))
    })?;

    tracing::info!(user_id = %user.user_id, connection_id = %id, "calendar connection removed");
    Ok(StatusCode::NO_CONTENT)
}
