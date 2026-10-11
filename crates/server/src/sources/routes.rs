//! Source upload and reads.
//!
//! One endpoint takes both shapes of a source, because the client's two paths
//! (a PDF from the file picker, pasted text) differ only in the payload. The
//! upload is validated against the plan *before* the bytes are stored: a quota
//! check after `put` would let an over-quota file occupy the bucket, and the
//! cleanup path is the one that gets forgotten.

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use stvdy_core::entitlements::Feature;

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::integrations::storage as storage_integration;
use crate::state::AppState;

use super::status::status;

/// Kinds Phase 1 accepts. PPTX/DOCX/URL/syllabus are Phase 7 work (roadmap
/// Phase 7 lists them explicitly); rejecting them with a clear message is
/// better than accepting a file that silently produces no text.
const SUPPORTED_KINDS: &[&str] = &["pdf", "text"];

/// One gigabyte is a ceiling that no plan allows, so it exists only to stop a
/// malicious client from streaming forever before the plan check runs.
const HARD_MAX_BYTES: usize = 1024 * 1024 * 1024;

pub fn routes(body_limit_bytes: usize) -> Router<AppState> {
    Router::new()
        .route("/workspaces/{id}/sources", get(list).post(create))
        .route("/sources/{id}/status", get(status))
        .route("/sources/{id}/file", get(file_url))
        .route("/sources/{id}", delete(remove))
        // The local storage backend's signed URL points here. It is
        // deliberately not behind `AuthUser`: the signature *is* the
        // authorization, which is what makes it equivalent to an S3 presigned
        // URL from the client's point of view.
        .route("/objects/{id}", get(download))
        // Axum's default body limit is 2 MB, which would reject an upload that
        // every plan allows and report it as a multipart parse error rather than
        // as a plan limit. The cap is passed in by `routes::router`, which has
        // the loaded config, so the *plan* check is what produces the precise
        // error; this only stops a body no plan could ever accept.
        .layer(DefaultBodyLimit::max(body_limit_bytes))
}

/// The largest file any plan accepts, plus multipart framing overhead.
///
/// Computed from the **loaded** `PlansFile` rather than by re-reading the file:
/// a container image has no `config/` directory at the compile-time path, so a
/// re-read would fall back to a default and silently cap a Pro or Power upload
/// at the free plan's limit.
pub fn max_upload_bytes(plans: &stvdy_core::config::PlansFile) -> usize {
    let largest = stvdy_core::config::PlanId::ALL
        .iter()
        .map(|plan| plans.get(*plan).max_file_mb)
        .max()
        .unwrap_or(20);
    // 1 MiB of slack for the boundary and the other form fields.
    largest as usize * 1024 * 1024 + 1024 * 1024
}

#[derive(Debug, serde::Deserialize)]
pub struct DownloadQuery {
    pub exp: u64,
    pub sig: String,
}

/// `GET /objects/:id?exp=&sig=`: streams an original for the local backend.
pub async fn download(
    State(state): State<AppState>,
    Path(source_id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<DownloadQuery>,
) -> AppResult<axum::response::Response> {
    if state.config.storage.backend != crate::config::StorageBackend::Local {
        // On S3 the client talks to the bucket, so this route is not part of
        // the contract and must not become a second, unsigned read path.
        return Err(AppError::NotFound);
    }

    use secrecy::ExposeSecret;
    storage_integration::verify_local_signature(
        state.config.storage.sign_key.expose_secret().as_bytes(),
        &source_id.to_string(),
        query.exp,
        &query.sig,
    )
    .map_err(|error| match error {
        // An expired link is a 410 with its own code: the client's remedy is to
        // ask for a fresh URL, which it cannot infer from a 403.
        storage_integration::SignatureError::Expired => AppError::LinkExpired,
        storage_integration::SignatureError::Mismatch => AppError::Forbidden,
    })?;

    let row = sqlx::query_as::<_, (Option<String>, String, Option<String>)>(
        "select file_key, kind, mime from sources
          where id = $1 and deleted_at is null",
    )
    .bind(source_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a source: {error}")))?
    .ok_or(AppError::NotFound)?;

    let (key, _kind, mime) = row;
    let key = key.ok_or(AppError::NotFound)?;

    let bytes = state
        .storage
        .get(&key)
        .await
        .map_err(|error| AppError::Provider(format!("reading the object: {error}")))?;

    let content_type = mime.unwrap_or_else(|| "application/octet-stream".to_string());
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, content_type),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment".to_string(),
            ),
            // A signed URL is short-lived, so caching it beyond its own expiry
            // would only serve a stale copy to the next holder of the link.
            (
                axum::http::header::CACHE_CONTROL,
                "private, no-store".to_string(),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Debug, Serialize)]
pub struct SourceView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    pub title: String,
    pub url: Option<String>,
    pub file_size: Option<i64>,
    pub mime: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct SourceRow {
    id: Uuid,
    workspace_id: Uuid,
    kind: String,
    title: String,
    url: Option<String>,
    file_size: Option<i64>,
    mime: Option<String>,
    status: String,
    error: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

impl From<SourceRow> for SourceView {
    fn from(row: SourceRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            kind: row.kind,
            title: row.title,
            url: row.url,
            file_size: row.file_size,
            mime: row.mime,
            status: row.status,
            error: row.error,
            created_at: row.created_at,
        }
    }
}

/// Confirms the workspace exists and belongs to the caller. A 404 rather than a
/// 403: whether someone else's workspace id exists is not information this
/// endpoint should give away.
pub(crate) async fn owned_workspace(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
) -> AppResult<()> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "select id from workspaces where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a workspace: {error}")))?;

    found.map(|_| ()).ok_or(AppError::NotFound)
}

/// `GET /workspaces/:id/sources`
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    owned_workspace(&state, user.user_id, workspace_id).await?;

    let rows = sqlx::query_as::<_, SourceRow>(
        "select id, workspace_id, kind, title, url, file_size, mime, status, error, created_at
           from sources
          where workspace_id = $1 and user_id = $2 and deleted_at is null
          order by created_at desc",
    )
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing sources: {error}")))?;

    let sources: Vec<SourceView> = rows.into_iter().map(SourceView::from).collect();
    Ok(Json(serde_json::json!({ "sources": sources })))
}

/// `POST /workspaces/:id/sources` (multipart: `kind`, then `file` or `text`).
///
/// Takes `VerifiedUser`, not `AuthUser`: ingestion is one of the four things A43
/// keeps locked until the address is confirmed ("chat, generation, search and
/// ingestion stay locked until `email_verified`"), and it is the first route in
/// the project that spends provider money on embedding calls.
pub async fn create(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    owned_workspace(&state, user.user_id, workspace_id).await?;

    let mut kind: Option<String> = None;
    let mut title: Option<String> = None;
    let mut file: Option<(String, String, Vec<u8>)> = None;
    let mut text: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Validation(format!("reading the upload: {error}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "kind" => {
                kind = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| AppError::Validation(error.to_string()))?
                        .trim()
                        .to_lowercase(),
                );
            }
            "title" => {
                title = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| AppError::Validation(error.to_string()))?,
                );
            }
            "text" => {
                text = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| AppError::Validation(error.to_string()))?,
                );
            }
            "file" => {
                let filename = field.file_name().unwrap_or("upload.pdf").to_string();
                let mime = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|error| AppError::Validation(format!("reading the file: {error}")))?;
                if bytes.len() > HARD_MAX_BYTES {
                    return Err(AppError::Validation("the file is too large".to_string()));
                }
                file = Some((filename, mime, bytes.to_vec()));
            }
            _ => {}
        }
    }

    let kind = kind.ok_or_else(|| AppError::Validation("kind is required".to_string()))?;
    if !SUPPORTED_KINDS.contains(&kind.as_str()) {
        return Err(AppError::Validation(format!(
            "kind must be one of {}",
            SUPPORTED_KINDS.join(", ")
        )));
    }

    let (body, mime, default_title): (Vec<u8>, String, String) = match kind.as_str() {
        "pdf" => {
            let (filename, mime, bytes) =
                file.ok_or_else(|| AppError::Validation("a pdf source needs a file".to_string()))?;
            // The extension is the check, not the client's content type: a
            // browser sends whatever the OS guessed.
            if !filename.to_lowercase().ends_with(".pdf") {
                return Err(AppError::Validation(
                    "pdf sources must be .pdf files".to_string(),
                ));
            }
            let title = title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| filename.clone());
            (bytes, mime, title)
        }
        "text" => {
            let body = text.ok_or_else(|| {
                AppError::Validation("a text source needs the text field".to_string())
            })?;
            if body.trim().is_empty() {
                return Err(AppError::Validation("the text is empty".to_string()));
            }
            let title = title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| first_line_title(&body));
            (body.into_bytes(), "text/plain".to_string(), title)
        }
        other => return Err(AppError::Validation(format!("unsupported kind {other}"))),
    };

    let size = body.len() as i64;
    // One decision for three axes: `max_file_mb` (a ceiling, so a 20 MB file fits a
    // 20 MB plan), `sources_total` (a count, so the check is `used < max`), and
    // `storage_mb` (a running total, so the check is `existing + new <= quota`). The
    // engine checks them in that order and refuses with the same 422 body the Phase 1
    // shim produced, so no client sees a different shape.
    //
    // `sources_total` is a separate axis from storage on purpose: a text source is a few
    // KB, so without it one account could create unbounded sources while staying far
    // under the storage cap.
    crate::entitlements::require(&state, &user, Feature::UploadSource { size_bytes: size }).await?;

    let source_id = Uuid::now_v7();
    let key = object_key(user.user_id, source_id, &kind);
    state
        .storage
        .put(&key, bytes::Bytes::from(body), &mime)
        .await
        .map_err(|error| AppError::Provider(format!("storing the source: {error}")))?;

    sqlx::query(
        "insert into sources
           (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
         values ($1, $2, $3, $4, $5, $6, $7, $8, 'pending')",
    )
    .bind(source_id)
    .bind(user.user_id)
    .bind(workspace_id)
    .bind(&kind)
    .bind(&default_title)
    .bind(&key)
    .bind(size)
    .bind(&mime)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording the source: {error}")))?;

    crate::jobs::enqueue(
        &state.pool,
        &state.jobs,
        "ingest_source",
        serde_json::json!({ "source_id": source_id }),
        None,
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing ingestion: {error}")))?;

    tracing::info!(%source_id, %workspace_id, kind = %kind, size, "source accepted");

    let row = sqlx::query_as::<_, SourceRow>(
        "select id, workspace_id, kind, title, url, file_size, mime, status, error, created_at
           from sources where id = $1",
    )
    .bind(source_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reloading the source: {error}")))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "source": SourceView::from(row) })),
    ))
}

/// The object key. Prefixed by user so a key can never be guessed into another
/// account's prefix, and deterministic so a retry overwrites rather than
/// duplicates.
fn object_key(user_id: Uuid, source_id: Uuid, kind: &str) -> String {
    let extension = match kind {
        "pdf" => "pdf",
        _ => "txt",
    };
    format!("users/{user_id}/sources/{source_id}.{extension}")
}

/// A title from pasted text: the first non-empty line, trimmed to something a
/// list row can render.
fn first_line_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Untitled note");
    let trimmed: String = line.chars().take(80).collect();
    if trimmed.is_empty() {
        "Untitled note".to_string()
    } else {
        trimmed
    }
}

/// Drops the chunks of the given sources and deletes their stored objects.
///
/// Shared by `DELETE /sources/:id` and `DELETE /workspaces/:id`, because the two
/// paths must reclaim identically: `enforce_storage` sums `file_size` over live
/// sources, so a path that tombstones a source without dropping its chunks frees
/// the user's *quota* while the embeddings (roughly 6 KiB per chunk, A47) and the
/// object stay forever. That is churnable: two workspaces and a delete loop would
/// keep quota at zero and storage unbounded.
///
/// The chunks are deleted in the caller's transaction so the count and the quota
/// cannot disagree; the objects are deleted afterwards, outside it, because a
/// storage failure must not roll back a deletion the user asked for.
pub(crate) async fn reclaim_chunks<'e, E>(
    executor: E,
    source_ids: &[Uuid],
) -> Result<Vec<String>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    if source_ids.is_empty() {
        return Ok(Vec::new());
    }

    let keys: Vec<String> = sqlx::query_scalar(
        "delete from chunks where source_id = any($1)
      returning (select file_key from sources where id = source_id)",
    )
    .bind(source_ids)
    .fetch_all(executor)
    .await?;

    Ok(keys.into_iter().filter(|key| !key.is_empty()).collect())
}

/// Deletes stored objects, logging rather than failing.
///
/// A leftover object is a cleanup job's problem, not a failed request: the row
/// is already gone from the user's view.
pub(crate) async fn delete_objects(state: &AppState, keys: &[String]) {
    for key in keys {
        if let Err(error) = state.storage.delete(key).await {
            tracing::warn!(%error, %key, "deleting a stored object failed");
        }
    }
}

/// `DELETE /sources/:id`: tombstone, drop the chunks, and delete the object.
///
/// The object is deleted after the row is tombstoned, so a storage failure
/// leaves a recoverable row rather than a source whose bytes are gone but which
/// still appears in the list.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(source_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a delete: {error}")))?;

    let row = sqlx::query_as::<_, (Option<String>,)>(
        "update sources set deleted_at = now()
          where id = $1 and user_id = $2 and deleted_at is null
      returning file_key",
    )
    .bind(source_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting a source: {error}")))?;

    let Some((file_key,)) = row else {
        return Err(AppError::NotFound);
    };

    reclaim_chunks(&mut *tx, &[source_id])
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("dropping chunks: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a delete: {error}")))?;

    let keys: Vec<String> = file_key.into_iter().collect();
    delete_objects(&state, &keys).await;

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /sources/:id/file`: a short-lived URL for the original.
pub async fn file_url(
    State(state): State<AppState>,
    user: AuthUser,
    Path(source_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let row = sqlx::query_as::<_, (Option<String>,)>(
        "select file_key from sources
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(source_id)
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a source: {error}")))?;

    let key = row.and_then(|(key,)| key).ok_or(AppError::NotFound)?;

    let url = storage_integration::source_file_url(
        &state.config.storage,
        state.storage.as_ref(),
        &source_id.to_string(),
        &key,
    )
    .await
    .map_err(|error| AppError::Provider(format!("signing a URL: {error}")))?;

    let expires_at = Utc::now()
        + chrono::Duration::from_std(state.config.storage.presign_ttl)
            .unwrap_or_else(|_| chrono::Duration::minutes(15));

    Ok(Json(
        serde_json::json!({ "url": url, "expires_at": expires_at }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_comes_from_the_first_non_empty_line() {
        assert_eq!(
            first_line_title("\n\n  Cell biology  \nnotes"),
            "Cell biology"
        );
        assert_eq!(first_line_title("   "), "Untitled note");
        assert_eq!(first_line_title(""), "Untitled note");
    }

    #[test]
    fn a_title_is_bounded() {
        let long = "x".repeat(500);
        assert_eq!(first_line_title(&long).chars().count(), 80);
    }

    #[test]
    fn an_object_key_is_scoped_to_its_owner() {
        let user = Uuid::now_v7();
        let source = Uuid::now_v7();
        let key = object_key(user, source, "pdf");
        assert!(key.starts_with(&format!("users/{user}/sources/")));
        assert!(key.ends_with(".pdf"));
        assert!(object_key(user, source, "text").ends_with(".txt"));
    }
}
