//! Sign-in with Google (A48).
//!
//! Standard Authorization Code + PKCE, server-side, using the same `oauth2`
//! crate and the same deep-link return path the Calendar integration needs. The
//! flow ends by issuing our *own* access + refresh pair through
//! `auth::tokens::issue_pair`, so nothing downstream can tell how the user
//! signed in.
//!
//! Two pieces are deliberately not the obvious implementation:
//!
//! 1. **The `state` parameter is encrypted, not just signed.** It carries the
//!    PKCE verifier, and a signed-but-readable `state` would hand that verifier
//!    to the browser, which is the party PKCE exists to distrust. AES-GCM under
//!    `ENCRYPTION_KEY_HEX` keeps it server-side while staying stateless, so it
//!    works with more than one instance.
//! 2. **The `id_token` is verified here, not by `oauth2`.** `oauth2` 5.0.0 has
//!    no JWT or JWKS support, so it can hand back an `id_token` but cannot check
//!    its signature. Verification is `jsonwebtoken` against Google's JWKS, plus
//!    the issuer, audience, expiry and nonce checks that are the actual security
//!    property.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use secrecy::ExposeSecret;
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use super::oauth_state::{
    Purpose, StatePayload, decrypt_state, encrypt_state, hash, pkce_pair, random_token,
    state_expiry,
};
use crate::auth::{extract::AuthUser, tokens};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_URI: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUERS: &[&str] = &["https://accounts.google.com", "accounts.google.com"];

/// How long an issued login ticket stays valid. Short: the app exchanges it
/// immediately on the deep link.
const TICKET_TTL_MINUTES: i64 = 5;
/// JWKS cache lifetime. Google rotates keys with a long overlap, so a stale set
/// is only a problem for a key created in the last few minutes, which the
/// refresh-on-miss path below covers.
const JWKS_TTL: Duration = Duration::from_secs(3600);

/// The `state` codec, the PKCE pair and the hash live in [`super::oauth_state`] (A235): the
/// calendar consent flow needs the same four things, and a second AES-GCM `state` codec is a
/// second place to get the nonce handling wrong. What stays here is the *payload* this flow
/// puts in it.
fn state_payload(query: &StartQuery, verifier: String, nonce: String) -> StatePayload {
    StatePayload {
        purpose: Purpose::Login,
        verifier,
        nonce,
        exp: state_expiry(),
        // A login has no account yet: resolving one is what the callback does.
        user_id: None,
        device_name: query
            .device_name
            .clone()
            .unwrap_or_else(|| "Google sign-in".to_string()),
        device_platform: query
            .device_platform
            .clone()
            .unwrap_or_else(|| "web".to_string()),
        app_version: query.app_version.clone(),
    }
}

#[derive(Debug, Deserialize)]
pub struct StartQuery {
    pub device_name: Option<String>,
    pub device_platform: Option<String>,
    pub app_version: Option<String>,
}

/// The Google authorization URL to redirect to. 404 when Google login is not
/// configured: a half-configured client must not produce a redirect that fails
/// at Google with a confusing error.
pub async fn start(state: &AppState, query: &StartQuery) -> AppResult<String> {
    let Some(google) = &state.config.google_login else {
        return Err(AppError::NotFound);
    };

    let (verifier, challenge) = pkce_pair();
    let nonce = random_token();
    let payload = state_payload(query, verifier, nonce.clone());
    let state_param = encrypt_state(state, &payload)?;

    let url = url::Url::parse_with_params(
        GOOGLE_AUTH_ENDPOINT,
        &[
            ("client_id", google.client_id.as_str()),
            ("redirect_uri", google.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("state", state_param.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            // A refresh token is not needed for login, so none is requested.
            ("access_type", "online"),
            ("prompt", "select_account"),
        ],
    )
    .map_err(|error| AppError::Internal(anyhow::anyhow!("building the Google URL: {error}")))?;

    Ok(url.to_string())
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Verifies `state`, exchanges the code, verifies the `id_token`, resolves or
/// creates the account, and returns the deep link the browser should follow.
/// It carries a single-use ticket, never a token.
pub async fn callback(state: &AppState, query: &CallbackQuery) -> AppResult<String> {
    let Some(google) = &state.config.google_login else {
        return Err(AppError::NotFound);
    };

    if let Some(error) = &query.error {
        // The user declined, or Google refused. Not an internal error.
        return Err(AppError::Validation(format!(
            "Google sign-in was cancelled ({error})"
        )));
    }

    let state_param = query
        .state
        .as_deref()
        .ok_or_else(|| AppError::Validation("missing OAuth state".to_string()))?;
    let payload = decrypt_state(state, Purpose::Login, state_param)?;
    let code = query
        .code
        .as_deref()
        .ok_or_else(|| AppError::Validation("missing authorization code".to_string()))?;

    let token_response = exchange_code(google, code, &payload.verifier).await?;

    let id_token = token_response
        .get("id_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Provider("Google returned no id_token".to_string()))?;

    let claims = verify_id_token(google, id_token, &payload.nonce).await?;

    let user_id = resolve_user(state, &claims).await?;

    // A ticket, not a token pair: a deep link lands in browser history and the
    // OS URL handler, so it must not carry a credential.
    let ticket = random_token();
    sqlx::query(
        "insert into login_tickets
           (user_id, ticket_hash, device_name, device_platform, app_version, expires_at)
         values ($1, $2, $3, $4, $5, now() + make_interval(mins => $6))",
    )
    .bind(user_id)
    .bind(hash(&ticket))
    .bind(&payload.device_name)
    .bind(&payload.device_platform)
    .bind(&payload.app_version)
    .bind(TICKET_TTL_MINUTES)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("issuing a login ticket: {error}")))?;

    tracing::info!(%user_id, "Google sign-in completed");

    Ok(format!("stvdy://auth/callback?ticket={ticket}"))
}

#[derive(Debug, Deserialize)]
pub struct ExchangeRequest {
    pub ticket: String,
    pub device: super::routes::DeviceInfo,
}

/// `POST /auth/google/exchange`. Turns a ticket into the same token pair
/// `POST /auth/login` returns. This is where the device is actually created, so
/// a browser round trip cannot consume a device slot on its own.
pub async fn exchange(
    state: &AppState,
    body: &ExchangeRequest,
) -> AppResult<super::routes::AuthResponse> {
    let row = sqlx::query(
        "update login_tickets
            set used_at = now()
          where ticket_hash = $1 and used_at is null and expires_at > now()
      returning user_id",
    )
    .bind(hash(body.ticket.trim()))
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("consuming a login ticket: {error}")))?;

    let Some(row) = row else {
        return Err(AppError::Unauthorized);
    };
    let user_id: Uuid = row.get("user_id");

    let user = sqlx::query_as::<_, super::routes::LoginRow>(
        "select id, email, password_hash, email_verified, email_verified_at, plan_id,
                timezone, student_verified, created_at
           from users where id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the account: {error}")))?
    .ok_or(AppError::Unauthorized)?;

    // Through the same resolver as a password login, so the Google path is not
    // an unlimited-device backdoor around A43's `devices_max` control.
    let device_id = super::routes::resolve_device(state, user.id, &body.device).await?;
    let pair = tokens::issue_pair(&state.pool, &state.config, user.id, device_id).await?;

    Ok(super::routes::AuthResponse {
        user: super::routes::user_view(
            user.id,
            user.email,
            user.email_verified,
            user.email_verified_at,
            user.plan_id,
            user.timezone,
            user.student_verified,
            user.created_at,
        ),
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        expires_at: pair.access_expires_at,
        device_id,
    })
}

/// What Google asserts about the user, after verification.
/// The claims this code reads. `aud`, `iss` and `exp` are *checked by
/// `Validation`* rather than by hand, but they are declared so the deserialized
/// shape matches the token and a future reader can see what is verified.
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    email: String,
    email_verified: Option<bool>,
    nonce: Option<String>,
    #[allow(dead_code)]
    aud: String,
    #[allow(dead_code)]
    iss: String,
    #[allow(dead_code)]
    exp: i64,
}

async fn exchange_code(
    google: &crate::config::GoogleLoginConfig,
    code: &str,
    verifier: &str,
) -> AppResult<serde_json::Value> {
    let client = reqwest::Client::new();
    let response = client
        .post(GOOGLE_TOKEN_ENDPOINT)
        .form(&[
            ("client_id", google.client_id.as_str()),
            ("client_secret", google.client_secret.expose_secret()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", google.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("Google token exchange: {error}")))?;

    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| AppError::Provider(format!("Google token response: {error}")))?;

    if !status.is_success() {
        // The body may name the reason (invalid_grant, redirect_uri_mismatch),
        // which is the difference between a five-minute fix and an afternoon.
        let reason = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(AppError::Provider(format!(
            "Google rejected the code exchange: {reason}"
        )));
    }
    Ok(body)
}

/// Cached JWKS. A `Mutex<Option<(Instant, JwkSet)>>` rather than a `LazyLock`
/// because the set must be refreshable: a new signing key would otherwise be
/// rejected until the process restarted.
static JWKS_CACHE: Mutex<Option<(Instant, jsonwebtoken::jwk::JwkSet)>> = Mutex::new(None);

async fn fetch_jwks(force: bool) -> AppResult<jsonwebtoken::jwk::JwkSet> {
    if !force {
        let guard = JWKS_CACHE
            .lock()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("the JWKS cache is poisoned")))?;
        if let Some((fetched_at, set)) = guard.as_ref()
            && fetched_at.elapsed() < JWKS_TTL
        {
            return Ok(set.clone());
        }
    }

    let set: jsonwebtoken::jwk::JwkSet = reqwest::get(GOOGLE_JWKS_URI)
        .await
        .map_err(|error| AppError::Provider(format!("fetching Google's JWKS: {error}")))?
        .json()
        .await
        .map_err(|error| AppError::Provider(format!("parsing Google's JWKS: {error}")))?;

    let mut guard = JWKS_CACHE
        .lock()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("the JWKS cache is poisoned")))?;
    *guard = Some((Instant::now(), set.clone()));
    Ok(set)
}

/// Verifies the `id_token` and returns its claims.
///
/// This is the step `oauth2` 5.0.0 cannot do, and it is the whole security
/// argument for trusting the `sub` claim. `email_verified` is required to be
/// true: linking on an unverified address is an account-takeover path (A48).
async fn verify_id_token(
    google: &crate::config::GoogleLoginConfig,
    id_token: &str,
    expected_nonce: &str,
) -> AppResult<IdTokenClaims> {
    use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};

    let header = decode_header(id_token)
        .map_err(|error| AppError::Provider(format!("the id_token header: {error}")))?;
    let kid = header
        .kid
        .ok_or_else(|| AppError::Provider("the id_token has no key id".to_string()))?;

    let mut jwks = fetch_jwks(false).await?;
    let mut key = jwks.find(&kid).cloned();
    if key.is_none() {
        // A key we have never seen: refresh once, in case it was just rotated.
        jwks = fetch_jwks(true).await?;
        key = jwks.find(&kid).cloned();
    }
    let key = key.ok_or_else(|| AppError::Provider("unknown id_token signing key".to_string()))?;
    let decoding_key = DecodingKey::from_jwk(&key)
        .map_err(|error| AppError::Provider(format!("the JWKS key: {error}")))?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[google.client_id.as_str()]);
    validation.set_issuer(GOOGLE_ISSUERS);
    validation.validate_exp = true;

    let claims = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
        .map_err(|error| {
            tracing::warn!(%error, "rejecting a Google id_token");
            AppError::Unauthorized
        })?
        .claims;

    if claims.nonce.as_deref() != Some(expected_nonce) {
        // A missing or mismatched nonce means this token was not minted for the
        // request we started, so it must not be accepted.
        return Err(AppError::Unauthorized);
    }
    if claims.email_verified != Some(true) {
        return Err(AppError::Validation(
            "Google has not verified this email address".to_string(),
        ));
    }
    if claims.email.trim().is_empty() || claims.sub.trim().is_empty() {
        return Err(AppError::Validation(
            "Google returned an incomplete profile".to_string(),
        ));
    }
    Ok(claims)
}

/// Links or creates the account.
///
/// Order matters and is the security rule from A48: `google_sub` first, then a
/// *verified* email match. An account that already has a password is never
/// taken over silently: the user has to sign in and link explicitly, which a
/// later phase's settings screen provides.
async fn resolve_user(state: &AppState, claims: &IdTokenClaims) -> AppResult<Uuid> {
    let email = super::routes::normalize_email(&claims.email)?;

    if let Some(existing) = sqlx::query_scalar::<_, Uuid>(
        "select id from users where google_sub = $1 and deleted_at is null",
    )
    .bind(&claims.sub)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("looking up a Google account: {error}")))?
    {
        return Ok(existing);
    }

    let by_email = sqlx::query(
        "select id, password_hash, google_sub from users where email = $1 and deleted_at is null",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("looking up an account: {error}")))?;

    if let Some(row) = by_email {
        let user_id: Uuid = row.get("id");
        let password_hash: Option<String> = row.get("password_hash");
        let google_sub: Option<String> = row.get("google_sub");

        if google_sub.is_none() && password_hash.is_some() {
            // Account-takeover refusal. A signed-in linking step is the way in,
            // and saying so is better than a bare 409.
            return Err(AppError::Conflict(
                "this email already has a password. Sign in with it, then connect Google in settings."
                    .to_string(),
            ));
        }

        // Google-only account, or one already linked to this subject: attach the
        // sub and let it through.
        sqlx::query("update users set google_sub = $2 where id = $1 and google_sub is null")
            .bind(user_id)
            .bind(&claims.sub)
            .execute(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("linking Google: {error}")))?;
        return Ok(user_id);
    }

    let user_id = Uuid::now_v7();
    sqlx::query(
        "insert into users (id, email, password_hash, google_sub, email_verified, email_verified_at, timezone)
         values ($1, $2, null, $3, true, now(), 'Asia/Jakarta')",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&claims.sub)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("creating a Google account: {error}")))?;

    // Same grant as a password signup: the first period starts now.
    crate::credits::grant_monthly(
        &state.pool,
        user_id,
        crate::credits::monthly_allowance(state, stvdy_core::config::PlanId::Free),
    )
    .await?;

    tracing::info!(%user_id, "account created through Google");
    Ok(user_id)
}

/// `POST /auth/google/set-password`. A Google-only account can only set a
/// password after its email is verified (A48 rule 3): otherwise the reset path
/// becomes a takeover vector.
pub async fn set_password(state: &AppState, user: AuthUser, new_password: &str) -> AppResult<()> {
    if !user.email_verified {
        return Err(AppError::EmailNotVerified);
    }
    let hash = super::password::hash(new_password)?;
    sqlx::query("update users set password_hash = $2 where id = $1")
        .bind(user.user_id)
        .bind(&hash)
        .execute(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("setting a password: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// What stays here after the codec moved to `super::oauth_state`: the payload this flow
    /// builds, which is the part the calendar flow must *not* share. The codec's own tests
    /// (round-trip, tamper, cross-purpose replay, nonce freshness) live with the codec.
    #[test]
    fn a_login_state_names_its_purpose_and_carries_no_account() {
        let query = StartQuery {
            device_name: Some("laptop".to_string()),
            device_platform: Some("macos".to_string()),
            app_version: Some("1.2.3".to_string()),
        };
        let payload = state_payload(&query, "verifier".to_string(), "nonce".to_string());
        assert_eq!(payload.purpose, Purpose::Login);
        assert_eq!(payload.verifier, "verifier");
        assert_eq!(payload.nonce, "nonce");
        // The login callback resolves the account from the `id_token`; a `user_id` here would
        // be a second source of truth for who is signing in.
        assert!(payload.user_id.is_none());
        assert!(payload.exp > Utc::now().timestamp());

        // The defaults are the ones the deep-link contract expects for a bare start.
        let bare = StartQuery {
            device_name: None,
            device_platform: None,
            app_version: None,
        };
        let payload = state_payload(&bare, "v".to_string(), "n".to_string());
        assert_eq!(payload.device_name, "Google sign-in");
        assert_eq!(payload.device_platform, "web");
        assert!(payload.app_version.is_none());
    }

    #[test]
    fn the_issuer_list_accepts_both_google_spellings() {
        assert!(GOOGLE_ISSUERS.contains(&"https://accounts.google.com"));
        assert!(GOOGLE_ISSUERS.contains(&"accounts.google.com"));
    }
}
