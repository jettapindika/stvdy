//! Email verification.
//!
//! A43 makes a verified email a precondition for *spending* (chat, generation,
//! search, ingestion), not for holding an account. The token is single-use and
//! expiring, and only its hash is stored, for the same reason as a refresh
//! token: the database should not contain a usable credential.

use chrono::{Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use stvdy_core::traits::mailer::Email;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn new_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Issues a verification token and mails the link. Any earlier unused token for
/// the same user is invalidated, so a second "resend" does not leave two live
/// credentials in the mailbox.
pub async fn send_verification(state: &AppState, user_id: Uuid, email: &str) -> AppResult<()> {
    let token = new_token();
    let expires_at = Utc::now() + Duration::hours(state.config.auth.verify_ttl_hours);

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a transaction: {error}"))
        })?;

    sqlx::query(
        "update email_verification_tokens set used_at = now()
          where user_id = $1 and used_at is null",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("invalidating old tokens: {error}")))?;

    sqlx::query(
        "insert into email_verification_tokens (user_id, token_hash, expires_at)
         values ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(hash_token(&token))
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a token: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a token: {error}")))?;

    // The link points at the web build when one is configured, because that is
    // where the route exists; otherwise the native scheme handles it.
    let base = state
        .config
        .web_origin
        .clone()
        .unwrap_or_else(|| "stvdy://auth/verify".to_string());
    let link = if base.starts_with("http") {
        format!("{base}/verify-email?token={token}")
    } else {
        format!("{base}?token={token}")
    };

    let message = Email {
        to: email.to_string(),
        subject: "Verify your Stvdy email".to_string(),
        text: format!(
            "Confirm this address to finish setting up your account.\n\n{link}\n\n\
             The link expires in {} hours. If you did not sign up, ignore this message.",
            state.config.auth.verify_ttl_hours
        ),
        html: None,
    };

    // A mail failure must not lose the token: the account exists and a resend
    // can be requested. Surfacing it as a 500 would make signup look broken.
    if let Err(error) = state.mailer.send(message).await {
        tracing::error!(%error, %user_id, "sending the verification mail failed");
    }
    Ok(())
}

/// Consumes a verification token. Returns the user it belonged to.
///
/// Single use is enforced by the `used_at is null` predicate in the UPDATE, so
/// two concurrent requests cannot both succeed: the loser updates zero rows.
pub async fn consume(pool: &PgPool, token: &str) -> AppResult<Uuid> {
    let hash = hash_token(token);

    let row = sqlx::query(
        "update email_verification_tokens
            set used_at = now()
          where token_hash = $1 and used_at is null and expires_at > now()
      returning user_id",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("consuming a token: {error}")))?;

    let Some(row) = row else {
        return Err(AppError::Validation(
            "this verification link is invalid or has expired".to_string(),
        ));
    };
    let user_id: Uuid = row.get("user_id");

    sqlx::query(
        "update users set email_verified = true, email_verified_at = now()
          where id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("marking an email verified: {error}")))?;

    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_url_safe_and_never_repeats() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert!(!a.contains('+') && !a.contains('/') && !a.contains('='));
    }

    #[test]
    fn only_the_hash_is_stored() {
        let token = new_token();
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(&token));
    }
}
