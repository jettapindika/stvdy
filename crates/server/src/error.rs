//! Typed errors mapped to HTTP status codes. Filled in by Task 9.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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

    /// A verified email is required before any action that costs money (A43).
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
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::ResyncRequired | AppError::LinkExpired => StatusCode::GONE,
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
            AppError::Forbidden => "forbidden",
            AppError::NotFound => "not_found",
            AppError::Validation(_) => "validation",
            AppError::Conflict(_) => "conflict",
            AppError::ResyncRequired => "resync_required",
            AppError::LinkExpired => "link_expired",
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
