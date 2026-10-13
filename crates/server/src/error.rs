//! Typed errors mapped to HTTP status codes. Filled in by Task 9.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::json;

pub type AppResult<T> = Result<T, AppError>;

/// Every error the API can return. The status/code pairs are the contract
/// documented in the roadmap § HTTP error mapping; keep the two in sync.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("authentication required")]
    Unauthorized,

    #[error("access token expired")]
    TokenExpired,

    #[error("not permitted")]
    Forbidden,

    /// `forbidden`, and it says why.
    ///
    /// The unit variant's "not permitted" is the right answer when there is nothing more to
    /// say. A row the calendar owns is not that case: Phase 10 fills `events.source`,
    /// `external_id`, `etag` and `connection_id`, and a client told only "not permitted"
    /// reads it as a permissions bug rather than as "this event belongs to your Google
    /// calendar" (A198).
    #[error("{0}")]
    ForbiddenWith(String),

    #[error("not found")]
    NotFound,

    #[error("{0}")]
    Validation(String),

    #[error("{0}")]
    Conflict(String),

    #[error("sync cursor is older than retained history")]
    ResyncRequired,

    /// A signed URL past its expiry. Distinct from `Forbidden` because the remedy
    /// is "request a fresh link", not "you may not read this".
    #[error("this link has expired")]
    LinkExpired,

    /// The account was deleted at the student's own request (A355).
    ///
    /// A distinct variant rather than `Unauthorized` for the reason the spec's deletion rule
    /// is a *promise*: a generic 401 tells a student their password is wrong, and the remedy
    /// they reach for — reset it, try again — cannot work, because the account they are
    /// typing at is going away. The message names the purge date, which is the only fact the
    /// student needs and the one thing a 401 could not carry, and it says what that date does
    /// to the address, because "my address is refused" is what brings them to signup and the
    /// purge is what gives it back (`purge_accounts` anonymizes the row, which releases it).
    ///
    /// **This is one of the two messages the client renders rather than replaces** (the other
    /// is `Validation`), so it is written as a sentence a student reads: capitalised, and the
    /// date at minute precision rather than `chrono`'s sub-second `Display`. The purge date
    /// exists nowhere else — the envelope's fixed five keys have no field for it — so a
    /// composed client sentence could only say "after a while" about the one date the student
    /// is waiting for.
    #[error(
        "This account was deleted at your request; its data is removed on {}, \
         which releases this address for a new account",
        .purges_at.format("%Y-%m-%d %H:%M UTC")
    )]
    AccountDeleted {
        /// When `purge_deleted_accounts` will remove the data.
        purges_at: DateTime<Utc>,
    },

    #[error("rate limited")]
    RateLimited,

    #[error("{message}")]
    Entitlement {
        code: &'static str,
        message: String,
        min_plan: Option<&'static str>,
        /// Populated for `limit_reached`, so the UI can say "3 of 3 devices"
        /// instead of only "upgrade".
        limit: Option<i64>,
        used: Option<i64>,
    },

    /// A verified email is required before the workspace opens (A43, A359).
    /// Distinct from `Forbidden` because the remedy is "check your inbox", not
    /// "upgrade".
    #[error("email verification required")]
    EmailNotVerified,

    #[error("provider error: {0}")]
    Provider(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::Unauthorized | AppError::TokenExpired => StatusCode::UNAUTHORIZED,
            AppError::Forbidden | AppError::ForbiddenWith(_) => StatusCode::FORBIDDEN,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::ResyncRequired | AppError::LinkExpired => StatusCode::GONE,
            // 403, not 410: the account is not a resource the client can re-fetch, and a
            // client that branched on 410 for a sync cursor would misread this as one. What
            // distinguishes it is the code, which is what the client branches on anyway.
            AppError::AccountDeleted { .. } => StatusCode::FORBIDDEN,
            // A limit the user can raise by paying is 422; a feature the plan
            // does not include at all is 403. Both are documented in the roadmap
            // § HTTP error mapping.
            AppError::Entitlement { code, .. } if *code == "insufficient_credits" => {
                StatusCode::PAYMENT_REQUIRED
            }
            AppError::Entitlement { code, .. } if *code == "limit_reached" => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            AppError::Entitlement { .. } | AppError::EmailNotVerified => StatusCode::FORBIDDEN,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::Provider(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::Unauthorized => "unauthorized",
            AppError::TokenExpired => "token_expired",
            AppError::Forbidden | AppError::ForbiddenWith(_) => "forbidden",
            AppError::NotFound => "not_found",
            AppError::Validation(_) => "validation",
            AppError::Conflict(_) => "conflict",
            AppError::ResyncRequired => "resync_required",
            AppError::LinkExpired => "link_expired",
            AppError::AccountDeleted { .. } => "account_deleted",
            AppError::RateLimited => "rate_limited",
            AppError::Entitlement { code, .. } => code,
            AppError::EmailNotVerified => "email_not_verified",
            AppError::Provider(_) => "provider_error",
            AppError::Internal(_) => "internal",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();

        // Internal causes are logged, never returned: they can carry SQL text.
        let message = match &self {
            AppError::Internal(cause) => {
                tracing::error!(error = %cause, "internal error");
                "internal error".to_string()
            }
            other => other.to_string(),
        };

        // The shape is fixed: every field is always present, so the client's
        // error mapper never has to guess. A missing `limit` means "not
        // applicable", not "absent".
        let mut body = json!({
            "code": code,
            "message": message,
            "min_plan": serde_json::Value::Null,
            "limit": serde_json::Value::Null,
            "used": serde_json::Value::Null,
        });
        if let AppError::Entitlement {
            min_plan,
            limit,
            used,
            ..
        } = &self
        {
            body["min_plan"] = json!(min_plan);
            body["limit"] = json!(limit);
            body["used"] = json!(used);
        }

        (status, Json(body)).into_response()
    }
}
