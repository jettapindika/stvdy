//! Devices: the machines a user is signed in on.
//!
//! A device is the unit of "sign out that old laptop". Revoking one sets
//! `revoked_at` and kills its sessions in the same breath, because a revoked
//! device whose refresh token still works is a logout that did not happen.
//!
//! Every lookup is scoped by `user_id` and answers 404 for someone else's row:
//! whether a device id exists is not information this endpoint should leak.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, put};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Long enough for "Jetta's work MacBook (2023, 16-inch)" and short enough that
/// no list row has to truncate it.
const MAX_NAME_CHARS: usize = 120;

/// The push providers the native shells register with. Anything else is a typo
/// that would sit in `push_tokens` and never deliver, so it is rejected here.
const PUSH_PROVIDERS: &[&str] = &["apns", "fcm"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list))
        .route("/devices/{id}", patch(rename).delete(revoke))
        .route("/devices/{id}/push-token", put(put_push_token))
}

#[derive(Debug, Serialize)]
pub struct DeviceView {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    pub app_version: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    /// When this device last completed a sync pull (A180).
    ///
    /// Distinct from `last_seen_at`, which any authenticated request refreshes: the Devices
    /// screen's "Last synced" is a claim about the *data* on that device, and a device that
    /// signed in and never pulled has synced nothing. `sync::touch_last_sync` is the only
    /// writer, and it sets both — so this is `None` exactly until the device's first pull.
    pub last_sync_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// True for the device making the request, so the client can mark "this
    /// device" without guessing from a name it may have renamed.
    pub current: bool,
}

#[derive(Debug, Serialize)]
pub struct DeviceList {
    pub devices: Vec<DeviceView>,
}

#[derive(Debug, Serialize)]
pub struct DeviceResponse {
    pub device: DeviceView,
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: Uuid,
    name: String,
    platform: String,
    app_version: Option<String>,
    last_seen_at: Option<DateTime<Utc>>,
    last_sync_at: Option<DateTime<Utc>>,
    created_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

impl DeviceRow {
    fn into_view(self, current_device: Uuid) -> DeviceView {
        DeviceView {
            current: self.id == current_device,
            id: self.id,
            name: self.name,
            platform: self.platform,
            app_version: self.app_version,
            last_seen_at: self.last_seen_at,
            last_sync_at: self.last_sync_at,
            created_at: self.created_at,
            revoked_at: self.revoked_at,
        }
    }
}

/// `GET /devices`
///
/// Only live devices: a revoked one is signed out, and showing it forever would
/// make the delete button look broken. `devices_max` is counted the same way.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<DeviceList>> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "select id, name, platform, app_version, last_seen_at, last_sync_at, created_at,
                revoked_at
           from devices
          where user_id = $1 and revoked_at is null
          order by created_at asc nulls last, id",
    )
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing devices: {error}")))?;

    let devices = rows
        .into_iter()
        .map(|row| row.into_view(user.device_id))
        .collect();

    Ok(Json(DeviceList { devices }))
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}

/// `PATCH /devices/:id`
///
/// A device the plan has put over `devices_max` is refused here and in every spend: it may
/// read, and it must be revocable (that is the remedy), but renaming it is a write to a
/// row the plan has made read-only.
pub async fn rename(
    State(state): State<AppState>,
    user: AuthUser,
    Path(device_id): Path<Uuid>,
    Json(body): Json<RenameRequest>,
) -> AppResult<Json<DeviceResponse>> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::Validation(
            "device name cannot be empty".to_string(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Validation(format!(
            "device name must be {MAX_NAME_CHARS} characters or fewer"
        )));
    }

    crate::entitlements::require_writable_row(
        &state,
        &user,
        crate::entitlements::ReadOnlyRow::Device,
        device_id,
    )
    .await?;

    let row = sqlx::query_as::<_, DeviceRow>(
        "update devices set name = $3
          where id = $1 and user_id = $2 and revoked_at is null
      returning id, name, platform, app_version, last_seen_at, last_sync_at, created_at,
                revoked_at",
    )
    .bind(device_id)
    .bind(user.user_id)
    .bind(name)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("renaming a device: {error}")))?
    .ok_or(AppError::NotFound)?;

    Ok(Json(DeviceResponse {
        device: row.into_view(user.device_id),
    }))
}

/// `DELETE /devices/:id`. Revokes the device and every session it holds, so the
/// next request from that device is 401 and its local cache is wiped on contact.
pub async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    Path(device_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a transaction: {error}"))
        })?;

    let row = sqlx::query_as::<_, (Uuid,)>(
        "update devices set revoked_at = now()
          where id = $1 and user_id = $2 and revoked_at is null
      returning id",
    )
    .bind(device_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking a device: {error}")))?;

    let Some((revoked_id,)) = row else {
        return Err(AppError::NotFound);
    };

    sqlx::query(
        "update auth_sessions set revoked_at = now()
          where device_id = $1 and revoked_at is null",
    )
    .bind(revoked_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("revoking a device's sessions: {error}"))
    })?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a revoke: {error}")))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct PushTokenRequest {
    pub provider: String,
    pub token: String,
}

/// `PUT /devices/:id/push-token`. Upsert, because the OS rotates a push token
/// and the client re-registers the same device; a second row would double every
/// notification.
pub async fn put_push_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(device_id): Path<Uuid>,
    Json(body): Json<PushTokenRequest>,
) -> AppResult<StatusCode> {
    if !PUSH_PROVIDERS.contains(&body.provider.as_str()) {
        return Err(AppError::Validation(
            "provider must be \"apns\" or \"fcm\"".to_string(),
        ));
    }
    if body.token.trim().is_empty() {
        return Err(AppError::Validation(
            "push token cannot be empty".to_string(),
        ));
    }

    // Confirm the device is the caller's and live before writing: a token keyed
    // to a revoked device would never be sent to and would leak nothing, but the
    // 404 keeps the ownership rule uniform across this module.
    let owned = sqlx::query_scalar::<_, bool>(
        "select exists(
             select 1 from devices
              where id = $1 and user_id = $2 and revoked_at is null)",
    )
    .bind(device_id)
    .bind(user.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking a device: {error}")))?;

    if !owned {
        return Err(AppError::NotFound);
    }

    sqlx::query(
        "insert into push_tokens (device_id, provider, token, updated_at)
         values ($1, $2, $3, now())
         on conflict (device_id) do update
             set provider = excluded.provider,
                 token = excluded.token,
                 updated_at = now()",
    )
    .bind(device_id)
    .bind(&body.provider)
    .bind(&body.token)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("storing a push token: {error}")))?;

    Ok(StatusCode::NO_CONTENT)
}
