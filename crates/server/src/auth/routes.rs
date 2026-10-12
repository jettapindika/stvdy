//! Auth routes: signup, login, refresh, logout, verification.
//!
//! Every one of these is on a per-IP rate-limited router (A12: per-instance, and
//! documented as such). `signup` additionally consults A43's database-backed
//! limiter, because an in-process bucket resets on restart and a restart is free
//! for a script.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use stvdy_core::entitlements::Feature;
use uuid::Uuid;

use crate::auth::{email_verify, extract::AuthUser, password, tokens};
use crate::credits;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The platforms a device can claim. Matches `billing_policy.toml`'s keys and
/// the `devices.platform` column, so a typo cannot create an unclassifiable
/// device that no policy applies to.
const PLATFORMS: &[&str] = &["web", "macos", "windows", "ios", "android"];

#[derive(Debug, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: String,
    pub app_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    pub device: DeviceInfo,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub device: DeviceInfo,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyEmailRequest {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct UserView {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub plan_id: String,
    pub timezone: String,
    pub student_verified: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub user: UserView,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub device_id: Uuid,
}

/// Normalizes an address before it is stored or compared: trimmed and
/// lowercased. Without this, `A@x.com` and `a@x.com` are two accounts and the
/// unique constraint does not catch the duplicate.
pub(crate) fn normalize_email(raw: &str) -> AppResult<String> {
    let email = raw.trim().to_lowercase();
    if email.len() > 254 || !email.contains('@') {
        return Err(AppError::Validation(
            "enter a valid email address".to_string(),
        ));
    }
    let (local, domain) = email
        .split_once('@')
        .ok_or_else(|| AppError::Validation("enter a valid email address".to_string()))?;
    if local.is_empty() || !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.')
    {
        return Err(AppError::Validation(
            "enter a valid email address".to_string(),
        ));
    }
    if email.chars().any(|c| c.is_whitespace()) {
        return Err(AppError::Validation(
            "enter a valid email address".to_string(),
        ));
    }
    Ok(email)
}

pub(crate) fn validate_device(device: &DeviceInfo) -> AppResult<()> {
    if !PLATFORMS.contains(&device.platform.as_str()) {
        return Err(AppError::Validation(format!(
            "platform must be one of {}",
            PLATFORMS.join(", ")
        )));
    }
    if device.name.trim().is_empty() || device.name.len() > 120 {
        return Err(AppError::Validation("device name is required".to_string()));
    }
    Ok(())
}

/// Resolves the device a sign-in came from: an existing live row for the same
/// `(platform, name)`, or a new one.
///
/// Reuse rather than always inserting, because the ordinary sequence sign up,
/// sign out, sign in, sign out, sign in would otherwise create five rows on the
/// user's single machine and hit `devices_max = 2` on free with no explanation.
/// The limit is enforced only when a row is actually created, so a returning
/// device never trips it. Revoked rows are not reused: the Devices screen is
/// where a revoked device is meant to be replaced.
pub(crate) async fn resolve_device(
    state: &AppState,
    user_id: Uuid,
    device: &DeviceInfo,
) -> AppResult<Uuid> {
    let name = device.name.trim();

    let existing: Option<Uuid> = sqlx::query_scalar(
        "update devices
            set last_seen_at = now(), app_version = coalesce($4, app_version)
          where id = (
                select id from devices
                 where user_id = $1 and platform = $2 and name = $3 and revoked_at is null
                 order by last_seen_at desc nulls last
                 limit 1)
      returning id",
    )
    .bind(user_id)
    .bind(&device.platform)
    .bind(name)
    .bind(&device.app_version)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("updating a device: {error}")))?;

    if let Some(id) = existing {
        return Ok(id);
    }

    enforce_device_limit(state, user_id).await?;
    insert_device(state, user_id, device).await
}

pub(crate) async fn insert_device(
    state: &AppState,
    user_id: Uuid,
    device: &DeviceInfo,
) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "insert into devices (user_id, name, platform, app_version, last_seen_at)
         values ($1, $2, $3, $4, now())
         returning id",
    )
    .bind(user_id)
    .bind(device.name.trim())
    .bind(&device.platform)
    .bind(&device.app_version)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a device: {error}")))?;
    Ok(id)
}

/// Enforces `devices_max` for the user's **effective** plan. A revoked device does not
/// count: that is what makes "sign out that old laptop" the remedy the error suggests.
///
/// The check goes through the entitlements engine's device arm rather than reading the
/// plan's `devices_max` here, so this route and the engine cannot disagree about the
/// limit. The plan is resolved from the user's own subscription status rather than from a
/// `plan_id` string, so an expired subscription cannot buy devices.
pub(crate) async fn enforce_device_limit(state: &AppState, user_id: Uuid) -> AppResult<()> {
    crate::entitlements::require_for(state, user_id, Feature::DeviceLogin).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn user_view(
    id: Uuid,
    email: String,
    email_verified: bool,
    email_verified_at: Option<DateTime<Utc>>,
    plan_id: String,
    timezone: String,
    student_verified: bool,
    created_at: Option<DateTime<Utc>>,
) -> UserView {
    UserView {
        id,
        email,
        email_verified,
        email_verified_at,
        plan_id,
        timezone,
        student_verified,
        created_at,
    }
}

/// `POST /auth/signup`
pub async fn signup(
    State(state): State<AppState>,
    ip: crate::ratelimit::SignupIp,
    Json(body): Json<SignupRequest>,
) -> AppResult<impl IntoResponse> {
    let email = normalize_email(&body.email)?;
    validate_device(&body.device)?;
    let password_hash = password::hash(&body.password)?;

    // A43's limiter, and it runs before the row is written. The attempt is
    // recorded even when the rest of this handler refuses (a taken address, a
    // device over the limit), because a farm's *failures* are the signal that
    // separates it from a lecture hall behind one NAT (A154/R5).
    let domain = email
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_string())
        .unwrap_or_default();
    crate::ratelimit::signup_limiter()
        .check(&state.pool, &ip.0, &domain)
        .await?;
    crate::ratelimit::record_signup_attempt(&state.pool, &ip.0, &domain).await?;

    // The unique index on `users.email` is the real guard; this pre-check turns
    // the common case into a clear message instead of a constraint error.
    //
    // A row that exists but is marked deleted is *not* the same case, and saying "an account
    // with this email already exists" would send the student looking for a password reset on
    // an account they deliberately deleted (A355). The address is held by the tombstone until
    // the purge anonymizes it, so this is the refusal that describes what actually happened.
    let existing: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("select deleted_at from users where email = $1")
            .bind(&email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("checking for an account: {error}"))
            })?;
    match existing {
        Some(Some(deleted_at)) => return Err(deletion_refusal(&state, deleted_at)),
        Some(None) => {
            return Err(AppError::Conflict(
                "an account with this email already exists".to_string(),
            ));
        }
        None => {}
    }

    let user_id = Uuid::now_v7();
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a transaction: {error}"))
        })?;

    sqlx::query(
        "insert into users (id, email, password_hash, timezone, is_admin)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind("Asia/Jakarta")
    // A7: the admin flag is seeded from ADMIN_EMAIL at startup, so a signup can
    // claim it only by matching that address exactly.
    .bind(state.config.admin_email.as_deref() == Some(email.as_str()))
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            AppError::Conflict("an account with this email already exists".to_string())
        } else {
            AppError::Internal(anyhow::anyhow!("creating an account: {error}"))
        }
    })?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a signup: {error}")))?;

    // The first period starts now, so the monthly allowance is granted now. The
    // recurring grant is a Phase 7 job; a job that runs at period start cannot
    // help the account whose period starts at signup, and without this a new
    // account has a zero balance and cannot send a single message.
    credits::grant_monthly(
        &state.pool,
        user_id,
        credits::monthly_allowance(&state, stvdy_core::config::PlanId::Free),
    )
    .await?;

    let device_id = resolve_device(&state, user_id, &body.device).await?;
    let pair = tokens::issue_pair(&state.pool, &state.config, user_id, device_id).await?;

    email_verify::send_verification(&state, user_id, &email).await?;

    tracing::info!(%user_id, platform = %body.device.platform, "account created");

    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            user: user_view(
                user_id,
                email,
                false,
                None,
                "free".to_string(),
                "Asia/Jakarta".to_string(),
                false,
                Some(Utc::now()),
            ),
            access_token: pair.access_token,
            refresh_token: pair.refresh_token,
            expires_at: pair.access_expires_at,
            device_id,
        }),
    ))
}

/// `POST /auth/login`
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let email = normalize_email(&body.email)?;
    validate_device(&body.device)?;

    // `deleted_at` is selected rather than filtered, and the refusal comes **after** the
    // password check (A355). Both halves matter:
    //
    //  * filtering it out would answer 401 for an account that exists and was deleted at the
    //    student's own request, which sends them to the password-reset path for an account
    //    that cannot be signed into — the remedy is wrong, not just the message;
    //  * refusing before the check would make the endpoint an oracle for "this address had an
    //    account that was deleted", which is a fact about a person that the caller has not
    //    proved they are.
    let row = sqlx::query_as::<_, LoginRow>(
        "select id, email, password_hash, email_verified, email_verified_at, plan_id,
                timezone, student_verified, created_at, deleted_at
           from users
          where email = $1",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("looking up an account: {error}")))?;

    // Unknown account and wrong password take the same path and the same time.
    let Some(row) = row else {
        password::verify_dummy(&body.password);
        return Err(AppError::Unauthorized);
    };

    let stored = row.password_hash.clone().unwrap_or_default();
    if stored.is_empty() {
        // A Google-only account (A48). It must not be able to set a password
        // through this route, and the message says how to get in.
        password::verify_dummy(&body.password);
        return Err(AppError::Unauthorized);
    }
    if !password::verify(&body.password, &stored) {
        return Err(AppError::Unauthorized);
    }

    // The credentials were right, so this is the student. Now tell them the truth about the
    // account they just proved they own: they deleted it, and when its data goes.
    if let Some(deleted_at) = row.deleted_at {
        return Err(deletion_refusal(&state, deleted_at));
    }

    let device_id = resolve_device(&state, row.id, &body.device).await?;
    let pair = tokens::issue_pair(&state.pool, &state.config, row.id, device_id).await?;

    tracing::info!(user_id = %row.id, platform = %body.device.platform, "login");

    Ok(Json(AuthResponse {
        user: user_view(
            row.id,
            row.email,
            row.email_verified,
            row.email_verified_at,
            row.plan_id,
            row.timezone,
            row.student_verified,
            row.created_at,
        ),
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        expires_at: pair.access_expires_at,
        device_id,
    }))
}

#[derive(sqlx::FromRow)]
pub(crate) struct LoginRow {
    pub(crate) id: Uuid,
    pub(crate) email: String,
    pub(crate) password_hash: Option<String>,
    pub(crate) email_verified: bool,
    pub(crate) email_verified_at: Option<DateTime<Utc>>,
    pub(crate) plan_id: String,
    pub(crate) timezone: String,
    pub(crate) student_verified: bool,
    pub(crate) created_at: Option<DateTime<Utc>>,
    /// `Some` for an account the student asked to delete (A355). Selected rather than filtered
    /// so the refusal can say so instead of pretending the address is unknown.
    pub(crate) deleted_at: Option<DateTime<Utc>>,
}

/// The refusal a marked account meets, with the date its data is removed (A355).
///
/// One function for the three entry points — `login`, `signup` and Google's `resolve_user` —
/// so the sentence a student reads cannot differ by which door they came through. It is
/// `account_deleted` rather than `unauthorized` because the remedy is different: nothing on
/// the sign-in screen can fix it, and the honest answer names the account's own state.
pub(crate) fn deletion_refusal(state: &AppState, deleted_at: DateTime<Utc>) -> AppError {
    AppError::AccountDeleted {
        purges_at: deleted_at + chrono::Duration::days(state.config.account_deletion_grace_days),
    }
}

#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

/// `POST /auth/refresh`
pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> AppResult<Json<RefreshResponse>> {
    let pair = tokens::rotate(
        &state.pool,
        &state.config,
        &SecretString::from(body.refresh_token),
    )
    .await?;

    Ok(Json(RefreshResponse {
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        expires_at: pair.access_expires_at,
    }))
}

/// `POST /auth/logout`
pub async fn logout(
    State(state): State<AppState>,
    Json(body): Json<LogoutRequest>,
) -> AppResult<StatusCode> {
    tokens::revoke_by_refresh_token(&state.pool, &SecretString::from(body.refresh_token)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /auth/logout-all`
pub async fn logout_all(State(state): State<AppState>, user: AuthUser) -> AppResult<StatusCode> {
    tokens::revoke_all_for_user(&state.pool, user.user_id).await?;
    tracing::info!(user_id = %user.user_id, "all sessions revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /auth/verify-email`
pub async fn verify_email(
    State(state): State<AppState>,
    Json(body): Json<VerifyEmailRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let user_id = email_verify::consume(&state.pool, body.token.trim()).await?;
    tracing::info!(%user_id, "email verified");
    Ok(Json(serde_json::json!({ "verified": true })))
}

/// `POST /auth/resend-verification`
pub async fn resend_verification(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<StatusCode> {
    if user.email_verified {
        return Ok(StatusCode::NO_CONTENT);
    }
    email_verify::send_verification(&state, user.user_id, &user.email).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /auth/me`
pub async fn me(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<UserView>> {
    // `deleted_at` is selected because [`LoginRow`] carries it. This route cannot see a
    // marked account — `AuthUser` requires `deleted_at is null` — so the value is always
    // `null` here; it is in the select because a struct's columns and a statement's columns
    // have to agree, and one struct for "the account as auth needs it" is better than two
    // that drift.
    let row = sqlx::query_as::<_, LoginRow>(
        "select id, email, password_hash, email_verified, email_verified_at, plan_id,
                timezone, student_verified, created_at, deleted_at
           from users where id = $1",
    )
    .bind(user.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the account: {error}")))?;

    Ok(Json(user_view(
        row.id,
        row.email,
        row.email_verified,
        row.email_verified_at,
        row.plan_id,
        row.timezone,
        row.student_verified,
        row.created_at,
    )))
}

/// `GET /auth/google/start`. A 302 to Google, or 404 when Google login is not
/// configured for this deployment.
pub async fn google_start(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<crate::auth::google::StartQuery>,
) -> AppResult<axum::response::Response> {
    let location = crate::auth::google::start(&state, &query).await?;
    Ok((
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response())
}

/// `GET /auth/google/callback`. Redirects to the app with a single-use ticket,
/// never with tokens: a deep link lands in browser history and the OS handler.
pub async fn google_callback(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<crate::auth::google::CallbackQuery>,
) -> AppResult<axum::response::Response> {
    let location = crate::auth::google::callback(&state, &query).await?;
    Ok((
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response())
}

/// `POST /auth/google/exchange`
pub async fn google_exchange(
    State(state): State<AppState>,
    Json(body): Json<crate::auth::google::ExchangeRequest>,
) -> AppResult<Json<AuthResponse>> {
    validate_device(&body.device)?;
    Ok(Json(crate::auth::google::exchange(&state, &body).await?))
}

/// `POST /auth/google/set-password`. A Google-only account can set one only
/// after its email is verified (A48), or the reset path becomes a takeover
/// vector.
pub async fn google_set_password(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<SetPasswordRequest>,
) -> AppResult<StatusCode> {
    crate::auth::google::set_password(&state, user, &body.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct SetPasswordRequest {
    pub password: String,
}
