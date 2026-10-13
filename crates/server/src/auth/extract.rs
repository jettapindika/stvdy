//! Request extractors: `AuthUser` and `VerifiedUser`.
//!
//! Every authenticated handler takes one of these rather than reading the header
//! itself, so "is this request scoped to a real, non-revoked device" is decided
//! once. `VerifiedUser` additionally enforces A43's "verified email before any
//! AI action" rule; it is the extractor the spending routes use.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use stvdy_core::entitlements::SubStatus;
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
    /// From `devices.platform`. The *only* platform an entitlement decision trusts
    /// (A144): a request body may claim anything, while a device row was written by a
    /// login against the platform allowlist.
    pub platform: String,
    /// The user's live `subscriptions.status`, or `SubStatus::None` when there is no
    /// row. It decides whether `plan_id` means anything (A146).
    pub subscription_status: SubStatus,
}

impl AuthUser {
    /// The plan the entitlement decision uses: `users.plan_id` honoured only while the
    /// subscription is active or in grace (A146).
    ///
    /// An expired subscription therefore drops to the free limits on its own, without an
    /// admin editing the row, and a plan id in the database that is not in `plans.toml`
    /// gets the free plan's limits — the safe direction.
    pub fn effective_plan(&self) -> stvdy_core::config::PlanId {
        crate::entitlements::effective_plan(&self.plan_id, self.subscription_status)
    }

    /// Whether an upgrade may be offered to this client at all (A143).
    ///
    /// The platform comes from the device row, never the body. An unrecognised value
    /// offers nothing: a platform the policy cannot classify must fail closed, and the
    /// config's own rule for a platform it does not know is deny-by-default.
    pub fn upgrade_offerable(&self, policy: &stvdy_core::config::BillingPolicy) -> bool {
        stvdy_core::config::Platform::ALL
            .into_iter()
            .find(|platform| platform.as_str() == self.platform)
            .is_some_and(|platform| policy.purchase_allowed(platform))
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
            //
            // The platform comes from the device row this request is scoped to (A144),
            // and the subscription status from the user's live row: an entitlement
            // decision must not read either from the request body.
            "select u.id as user_id, u.email, u.email_verified, u.plan_id, u.timezone,
                    u.is_admin, u.student_verified, d.platform,
                    (select sub.status
                       from subscriptions sub
                      where sub.user_id = u.id
                      order by (sub.status in ('active', 'grace')) desc,
                               sub.current_period_end desc
                      limit 1) as subscription_status
               from users u
               join devices d on d.id = $2 and d.user_id = u.id and d.revoked_at is null
               join auth_sessions s
                 on s.family_id = $3 and s.user_id = u.id and s.revoked_at is null
              where u.id = $1 and u.deleted_at is null
              group by u.id, u.email, u.email_verified, u.plan_id, u.timezone,
                       u.is_admin, u.student_verified, d.platform",
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
            platform: row.platform,
            subscription_status: crate::entitlements::subscription_status(
                row.subscription_status.as_deref(),
                row.user_id,
            ),
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
    platform: String,
    subscription_status: Option<String>,
}

/// An authenticated user whose email is verified: the extractor the workspace takes.
///
/// A43 (A359): signup creates the account, and **nothing else in the app works until the
/// address is confirmed**. The rule started narrower — verified email before any action that
/// *costs money*, which is why this extractor was on chat, generation, search, ingestion and
/// checkout and nowhere else — but a gate that stops at spending is a gate a scripted account
/// walks around: it can still hold a workspace, fill it from a device's local cache, and read
/// everything back, at no cost to itself and some cost to the operator.
///
/// So the axis is now the *surface*, not the price. Everything that reads or writes the
/// student's study data takes this extractor. What stays on [`AuthUser`] is the account's own
/// administrative surface — the session, the devices, the settings row, the export and the
/// deletion — because a student whose address cannot receive mail must still be able to
/// revoke a device, take their data out and leave. `route_sweep.rs` classifies every
/// registered route on this axis, so a new route has to choose a side rather than inherit
/// one.
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
