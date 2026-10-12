//! The `.ac.id` student discount flow (A299).
//!
//! Two routes and one rule: the discount applies only when the address is **verified**,
//! and the verification is a link mailed to that address. The rule that makes the endpoint
//! safe is the suffix list: without it, `POST /billing/student/verify` is a mail amplifier
//! — an authenticated account could ask us to send mail to any address it likes, at our
//! expense and with our reputation.
//!
//! The token store is the existing `email_verification_tokens` with a `purpose` column, so
//! one table serves both verifications and a token minted for one purpose cannot be spent
//! on the other. The mint and the hash are [`crate::auth::email_verify`]'s, not a second
//! copy: two token conventions in one codebase is how one of them ends up weaker.

use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use stvdy_core::traits::mailer::Email;

use crate::auth::email_verify::{hash_token, new_token};
use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct VerifyBody {
    pub email: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyOutcome {
    /// Whether a link was sent. `false` is not an error: telling a caller "that address is
    /// not a campus one" and "that address is already verified" would let the endpoint be
    /// used to probe which addresses exist.
    pub sent: bool,
    /// The address the link went to, echoed so the client can say "check your inbox at …".
    /// Only returned when a link was actually sent.
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub token: String,
}

/// `POST /billing/student/verify` — mail a link to the supplied address.
///
/// Requires a session (the caller must be a real account) *and* the suffix check. The two
/// together are what bound abuse: a session limits who can ask, the suffix limits what they
/// can ask for.
pub async fn request(
    state: &AppState,
    user: &AuthUser,
    body: VerifyBody,
) -> AppResult<Json<VerifyOutcome>> {
    let email = body.email.trim().to_ascii_lowercase();

    if !is_student_address(&email, &state.config.student_email_suffixes) {
        // Deliberately the same response shape as a successful send: a caller must not be
        // able to use this endpoint to learn which domains we accept, and the client
        // already knows the rule because it renders it.
        tracing::info!(%user.user_id, "a student verification was refused: not a campus address");
        return Ok(Json(VerifyOutcome {
            sent: false,
            email: None,
        }));
    }

    let token = new_token();
    let expires_at = Utc::now() + Duration::hours(state.config.auth.verify_ttl_hours);

    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a verification: {error}"))
    })?;

    // Only the previous *student* token is invalidated: an email-verification link the
    // account is still holding must not be killed by asking for a student link.
    sqlx::query(
        "update email_verification_tokens set used_at = now()
          where user_id = $1 and purpose = 'student_verify' and used_at is null",
    )
    .bind(user.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("invalidating old tokens: {error}")))?;

    sqlx::query(
        "insert into email_verification_tokens
             (user_id, token_hash, expires_at, purpose, target_email)
         values ($1, $2, $3, 'student_verify', $4)",
    )
    .bind(user.user_id)
    .bind(hash_token(&token))
    .bind(expires_at)
    // The address the link went to, so `confirm` stores the address that was actually
    // proved rather than the account's own.
    .bind(&email)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a token: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a token: {error}")))?;

    let base = state
        .config
        .web_origin
        .clone()
        .unwrap_or_else(|| "stvdy://billing/student".to_string());
    let link = if base.starts_with("http") {
        format!("{base}/billing/student?token={token}")
    } else {
        format!("{base}?token={token}")
    };

    let message = Email {
        to: email.clone(),
        subject: "Confirm your student email for the Stvdy discount".to_string(),
        text: format!(
            "Confirm this campus address and your plan price drops by 50%.\n\n{link}\n\n\
             The link expires in {} hours. If you did not ask for this, ignore this \
             message — nothing changes until the link is opened.",
            state.config.auth.verify_ttl_hours
        ),
        html: None,
    };

    // A mail failure is logged and reported as `sent: false` rather than as a 500: the
    // account is fine and the student can ask again, which is exactly what a 500 would
    // not communicate.
    if let Err(error) = state.mailer.send(message).await {
        tracing::error!(%error, %user.user_id, "sending the student verification mail failed");
        return Ok(Json(VerifyOutcome {
            sent: false,
            email: None,
        }));
    }

    tracing::info!(%user.user_id, "a student verification link was sent");
    Ok(Json(VerifyOutcome {
        sent: true,
        email: Some(email),
    }))
}

/// `POST /billing/student/confirm` — spend the token, set the flag and the address.
///
/// Single use is enforced by the `used_at is null` predicate in the UPDATE, so two
/// concurrent requests cannot both succeed: the loser updates zero rows. The `purpose`
/// predicate is the other half — a token minted to verify an *email* must not confirm a
/// student address, or the student discount would be reachable by verifying any address at
/// all.
pub async fn confirm(
    state: &AppState,
    user: &AuthUser,
    body: ConfirmBody,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a confirmation: {error}"))
    })?;

    // The address comes off the token row — the one the link was mailed to — and never from
    // the body or from `users.email`. That is what makes the suffix rule mean something: the
    // student proved control of *that* campus address, and storing the account's own address
    // instead would record the discount against an address the rule never accepted.
    let row: Option<(uuid::Uuid, Option<String>)> = sqlx::query_as(
        "update email_verification_tokens
            set used_at = now()
          where token_hash = $1 and purpose = 'student_verify'
            and user_id = $2 and used_at is null and expires_at > now()
      returning user_id, target_email",
    )
    .bind(hash_token(&body.token))
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("consuming a token: {error}")))?;

    let Some((user_id, target_email)) = row else {
        return Err(AppError::Validation(
            "this verification link is invalid or has expired".to_string(),
        ));
    };
    let Some(target_email) = target_email else {
        // A `student_verify` token with no target is one this code could not have minted.
        // Refusing is the safe direction: the alternative is recording whatever address the
        // account happens to hold and calling it proved.
        return Err(AppError::Validation(
            "this verification link is invalid or has expired".to_string(),
        ));
    };

    sqlx::query("update users set student_verified = true, student_email = $2 where id = $1")
        .bind(user_id)
        .bind(&target_email)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("marking a student verified: {error}"))
        })?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a confirmation: {error}"))
    })?;

    tracing::info!(%user_id, "a student address was verified");

    Ok(Json(serde_json::json!({
        "student_verified": true,
    })))
}

/// Whether an address is a campus one, by the configured suffix list.
///
/// The comparison is on the **end** of the domain, and it is done on the domain rather
/// than on the whole address: a naive `ends_with(".ac.id")` would accept
/// `student@ac.id.evil.example`, because the *address* ends with those characters. That
/// is the whole bug this function exists to not have.
pub fn is_student_address(email: &str, suffixes: &[String]) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || email.len() > 254 {
        return false;
    }
    suffixes.iter().any(|suffix| {
        let suffix = suffix.trim_start_matches('.');
        !suffix.is_empty() && (domain == suffix || domain.ends_with(&format!(".{suffix}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suffixes() -> Vec<String> {
        vec![".ac.id".to_string()]
    }

    #[test]
    fn a_campus_address_matches_and_a_lookalike_does_not() {
        assert!(is_student_address("ana@ui.ac.id", &suffixes()));
        assert!(is_student_address("ana@ac.id", &suffixes()));
        assert!(is_student_address("ANA@UI.AC.ID", &suffixes()));

        // The lookalike: the *address* ends with `.ac.id`, and the domain does not.
        assert!(!is_student_address("ana@ac.id.evil.example", &suffixes()));
        assert!(!is_student_address("ana@notac.id", &suffixes()));
        assert!(!is_student_address("ana@gmail.com", &suffixes()));
        // No domain at all, and an empty suffix, are both refusals rather than panics.
        assert!(!is_student_address("ana", &suffixes()));
        assert!(!is_student_address("ana@", &suffixes()));
        assert!(!is_student_address("ana@ui.ac.id", &["".to_string()]));
    }

    #[test]
    fn several_suffixes_are_any_of_them() {
        let list = vec![".ac.id".to_string(), ".edu".to_string()];
        assert!(is_student_address("ana@ui.ac.id", &list));
        assert!(is_student_address("ana@mit.edu", &list));
        assert!(!is_student_address("ana@mit.edu.example", &list));
    }
}
