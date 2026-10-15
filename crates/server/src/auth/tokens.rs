//! Access and refresh tokens.
//!
//! Access tokens are short-lived JWTs and deliberately carry **no plan claim**
//! (A22): a token is a bearer of identity, never of entitlement, so editing it
//! grants nothing. Refresh tokens are opaque random strings; only their SHA-256
//! is stored, so a database leak does not hand over usable sessions.
//!
//! Rotation is the security property that matters here. Every refresh inserts a
//! new row in the same `family_id` and revokes the presented one. Presenting an
//! already-revoked row means the token was stolen and used after the legitimate
//! client rotated it, so the whole family dies and the user signs in again.

use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::error::{AppError, AppResult};

/// What an access token asserts. `did` (device id) and `sid` (session family)
/// are what let a revoked device be refused without a database round trip for
/// every claim, and `jti` makes a token individually identifiable in logs
/// without logging the token itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: Uuid,
    pub did: Uuid,
    pub sid: Uuid,
    pub iat: i64,
    pub exp: i64,
    pub jti: Uuid,
}

#[derive(Debug, Clone)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_expires_at: DateTime<Utc>,
    pub device_id: Uuid,
}

fn encode_jwt(config: &AppConfig, claims: &AccessClaims) -> AppResult<String> {
    let header = jsonwebtoken::Header {
        typ: Some("JWT".to_string()),
        ..Default::default()
    };
    jsonwebtoken::encode(
        &header,
        claims,
        &jsonwebtoken::EncodingKey::from_secret(config.jwt_secret.expose_secret().as_bytes()),
    )
    .map_err(|error| AppError::Internal(anyhow::anyhow!("signing an access token: {error}")))
}

pub fn decode_jwt(config: &AppConfig, token: &str) -> AppResult<AccessClaims> {
    let validation = jsonwebtoken::Validation::default();
    match jsonwebtoken::decode::<AccessClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(config.jwt_secret.expose_secret().as_bytes()),
        &validation,
    ) {
        Ok(data) => Ok(data.claims),
        // An expired token is worth distinguishing: the client refreshes on
        // `token_expired` and gives up on `unauthorized`.
        Err(error) => {
            if matches!(
                error.kind(),
                jsonwebtoken::errors::ErrorKind::ExpiredSignature
            ) {
                Err(AppError::TokenExpired)
            } else {
                Err(AppError::Unauthorized)
            }
        }
    }
}

/// 32 bytes of OS randomness, base64url without padding. Long enough that the
/// hash stored in the database cannot be brute-forced.
fn new_refresh_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn hash_refresh_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Issues a fresh access + refresh pair for an existing device, starting a new
/// session family. Used by signup, login and Google login.
pub async fn issue_pair(
    pool: &PgPool,
    config: &AppConfig,
    user_id: Uuid,
    device_id: Uuid,
) -> AppResult<TokenPair> {
    issue_pair_in_family(pool, config, user_id, device_id, Uuid::now_v7()).await
}

async fn issue_pair_in_family(
    pool: &PgPool,
    config: &AppConfig,
    user_id: Uuid,
    device_id: Uuid,
    family_id: Uuid,
) -> AppResult<TokenPair> {
    let now = Utc::now();
    let access_expires_at = now + Duration::minutes(config.auth.access_ttl_min);
    let refresh_expires_at = now + Duration::days(config.auth.refresh_ttl_days);

    let claims = AccessClaims {
        sub: user_id,
        did: device_id,
        sid: family_id,
        iat: now.timestamp(),
        exp: access_expires_at.timestamp(),
        jti: Uuid::now_v7(),
    };
    let access_token = encode_jwt(config, &claims)?;

    let refresh_token = new_refresh_token();
    sqlx::query(
        "insert into auth_sessions
           (user_id, device_id, refresh_token_hash, family_id, expires_at)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(device_id)
    .bind(hash_refresh_token(&refresh_token))
    .bind(family_id)
    .bind(refresh_expires_at)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a session: {error}")))?;

    Ok(TokenPair {
        access_token,
        refresh_token,
        access_expires_at,
        refresh_expires_at,
        device_id,
    })
}

/// Rotates a refresh token. Returns the new pair, or an error that the caller
/// maps to 401.
pub async fn rotate(
    pool: &PgPool,
    config: &AppConfig,
    refresh_token: &SecretString,
) -> AppResult<TokenPair> {
    let hash = hash_refresh_token(refresh_token.expose_secret());

    let row = sqlx::query(
        "select id, user_id, device_id, family_id, expires_at, revoked_at
           from auth_sessions
          where refresh_token_hash = $1
          for update",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("looking up a session: {error}")))?;

    let Some(row) = row else {
        // Unknown token: either a forgery or a session already compacted away.
        return Err(AppError::Unauthorized);
    };

    let user_id: Uuid = row.get("user_id");
    let device_id: Uuid = row.get("device_id");
    let family_id: Uuid = row.get("family_id");
    let expires_at: DateTime<Utc> = row.get("expires_at");
    let revoked_at: Option<DateTime<Utc>> = row.get("revoked_at");

    if revoked_at.is_some() {
        // Reuse of a rotated token: the family is compromised. Revoking all of
        // it is the whole point of rotation, so this is not a warning.
        revoke_family(pool, family_id).await?;
        tracing::warn!(%user_id, %family_id, "refresh token reuse detected; family revoked");
        return Err(AppError::Unauthorized);
    }

    if expires_at <= Utc::now() {
        return Err(AppError::Unauthorized);
    }

    // The device may have been revoked (or the account deleted) since login.
    let device_ok: Option<bool> =
        sqlx::query_scalar("select revoked_at is null from devices where id = $1 and user_id = $2")
            .bind(device_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("checking the device: {error}")))?;

    if device_ok != Some(true) {
        revoke_family(pool, family_id).await?;
        return Err(AppError::Unauthorized);
    }

    sqlx::query("update auth_sessions set revoked_at = now() where refresh_token_hash = $1")
        .bind(&hash)
        .execute(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("rotating a session: {error}")))?;

    issue_pair_in_family(pool, config, user_id, device_id, family_id).await
}

pub async fn revoke_family(pool: &PgPool, family_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "update auth_sessions set revoked_at = now()
          where family_id = $1 and revoked_at is null",
    )
    .bind(family_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking a session family: {error}")))?;
    Ok(())
}

/// Revokes the family a refresh token belongs to, if it is known. Idempotent, so
/// logout twice is not an error.
pub async fn revoke_by_refresh_token(pool: &PgPool, refresh_token: &SecretString) -> AppResult<()> {
    let hash = hash_refresh_token(refresh_token.expose_secret());
    sqlx::query(
        "update auth_sessions set revoked_at = now()
          where family_id = (select family_id from auth_sessions where refresh_token_hash = $1)
            and revoked_at is null",
    )
    .bind(&hash)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking a session: {error}")))?;
    Ok(())
}

pub async fn revoke_all_for_user(pool: &PgPool, user_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "update auth_sessions set revoked_at = now()
          where user_id = $1 and revoked_at is null",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking sessions: {error}")))?;
    Ok(())
}

pub async fn revoke_all_for_device(pool: &PgPool, device_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "update auth_sessions set revoked_at = now()
          where device_id = $1 and revoked_at is null",
    )
    .bind(device_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking device sessions: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_token_is_long_and_never_repeats() {
        let a = new_refresh_token();
        let b = new_refresh_token();
        assert_ne!(a, b);
        assert!(a.len() >= 42, "32 bytes base64url is 43 characters");
        assert!(!a.contains('+') && !a.contains('/') && !a.contains('='));
    }

    #[test]
    fn hashing_is_stable_and_not_reversible_by_inspection() {
        let hash = hash_refresh_token("token");
        assert_eq!(hash, hash_refresh_token("token"));
        assert_ne!(hash, hash_refresh_token("other"));
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("token"));
    }
}
