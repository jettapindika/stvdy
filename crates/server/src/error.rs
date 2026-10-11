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

    #[error("rate limited")]
    RateLimited,

    #[error("{message}")]
    Entitlement {
        code: &'static str,
        message: String,
        min_plan: Option<&'static str>,
    },

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
            AppError::ResyncRequired => StatusCode::GONE,
            AppError::Entitlement { code, .. } if *code == "insufficient_credits" => {
                StatusCode::PAYMENT_REQUIRED
            }
            AppError::Entitlement { .. } => StatusCode::FORBIDDEN,
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
            AppError::RateLimited => "rate_limited",
            AppError::Entitlement { code, .. } => code,
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

        let mut body = json!({ "code": code, "message": message });
        if let AppError::Entitlement { min_plan, .. } = &self {
            body["min_plan"] = json!(min_plan);
        }

        (status, Json(body)).into_response()
    }
}
