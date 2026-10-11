//! Request extractors: `AuthUser` and `VerifiedUser`.
//!
//! Every authenticated handler takes one of these rather than reading the header
//! itself, so "is this request scoped to a real, non-revoked device" is decided
//! once. `VerifiedUser` additionally enforces A43's "verified email before any
//! AI action" rule; it is the extractor the spending routes use.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use crate::auth::tokens;
use crate::error::AppError;
use crate::state::AppState;

/// The authenticated caller: a user, a device, and the session family the access
/// token belongs to.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub session_id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub plan_id: String,
    pub timezone: String,
    pub is_admin: bool,
    pub student_verified: bool,
}

impl AuthUser {
    /// The plan as a typed value. A plan id in the database that is not in
    /// `plans.toml` cannot happen through the API, but it must not panic here:
    /// the caller gets the free plan's limits, which is the safe direction.
    pub fn plan(&self) -> stvdy_core::config::PlanId {
        stvdy_core::config::PlanId::parse(&self.plan_id)
    }
}

/// Extracts the bearer token, verifies it, and confirms the device and the
/// session family are still live.
///
/// The database check on every request is deliberate: a token alone would keep
/// working after a device was revoked or the user signed out everywhere, which
/// is exactly the guarantee the Devices screen promises.
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(AppError::Unauthorized)?;

        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or(AppError::Unauthorized)?
            .trim();

        let claims = tokens::decode_jwt(&state.config, token)?;

        let row = sqlx::query_as::<_, SessionUser>(
            // `s.revoked_at is null` is the load-bearing clause: without it a
            // signed-out or rotated-away session family still authenticates,
            // which is exactly the guarantee `logout-all` and device revocation
            // promise. Caught by the auth suite, not by inspection.
            "select u.id as user_id, u.email, u.email_verified, u.plan_id, u.timezone,
                    u.is_admin, u.student_verified
               from users u
               join devices d on d.id = $2 and d.user_id = u.id and d.revoked_at is null
               join auth_sessions s
                 on s.family_id = $3 and s.user_id = u.id and s.revoked_at is null
              where u.id = $1 and u.deleted_at is null
              group by u.id, u.email, u.email_verified, u.plan_id, u.timezone,
                       u.is_admin, u.student_verified",
        )
        .bind(claims.sub)
        .bind(claims.did)
        .bind(claims.sid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the caller: {error}")))?;

        let Some(row) = row else {
            return Err(AppError::Unauthorized);
        };

        Ok(AuthUser {
            user_id: row.user_id,
            device_id: claims.did,
            session_id: claims.sid,
            email: row.email,
            email_verified: row.email_verified,
            plan_id: row.plan_id,
            timezone: row.timezone,
            is_admin: row.is_admin,
            student_verified: row.student_verified,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SessionUser {
    user_id: Uuid,
    email: String,
    email_verified: bool,
    plan_id: String,
    timezone: String,
    is_admin: bool,
    student_verified: bool,
}

/// An authenticated user whose email is verified. A43: signup may create the
/// account, but chat, generation, search and ingestion stay locked until the
/// address is confirmed, so that a scripted account cannot spend.
#[derive(Debug, Clone)]
pub struct VerifiedUser(pub AuthUser);

impl std::ops::Deref for VerifiedUser {
    type Target = AuthUser;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromRequestParts<AppState> for VerifiedUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if !user.email_verified {
            return Err(AppError::EmailNotVerified);
        }
        Ok(VerifiedUser(user))
    }
}

/// An authenticated administrator (A7: `users.is_admin`, no roles table).
#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

impl std::ops::Deref for AdminUser {
    type Target = AuthUser;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            // 404, not 403: whether an admin endpoint exists is not something a
            // non-admin needs to learn.
            return Err(AppError::NotFound);
        }
        Ok(AdminUser(user))
    }
}
