use serde::{Deserialize, Serialize};

/// Wire error codes. Mirrors `AppError::code()` on the server and the client's
/// error handling, so both ends can switch exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    TokenExpired,
    Forbidden,
    NotFound,
    Validation,
    Conflict,
    ResyncRequired,
    LimitReached,
    InsufficientCredits,
    PlanRequired,
    FeatureLocked,
    /// The entity is not client-writable (see `Entity::is_client_writable`):
    /// messages, sources, flashcards, quizzes, decks, reports, goals,
    /// workspaces and pending_actions are written through their own endpoints,
    /// never through `/sync/push`.
    ReadOnlyEntity,
    RateLimited,
    ProviderError,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::TokenExpired => "token_expired",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Validation => "validation",
            ErrorCode::Conflict => "conflict",
            ErrorCode::ResyncRequired => "resync_required",
            ErrorCode::LimitReached => "limit_reached",
            ErrorCode::InsufficientCredits => "insufficient_credits",
            ErrorCode::PlanRequired => "plan_required",
            ErrorCode::FeatureLocked => "feature_locked",
            ErrorCode::ReadOnlyEntity => "read_only_entity",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::ProviderError => "provider_error",
            ErrorCode::Internal => "internal",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
