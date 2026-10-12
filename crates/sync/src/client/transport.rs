//! How the engine talks to the server, and the four ways that can fail.
//!
//! A `Transport` is a function of the wire and nothing else: the shell's implementation is
//! `reqwest` over HTTP, and the convergence suite's is the real router called in process
//! (A182). Both classify a response with [`TransportError::from_status`], so the engine
//! cannot behave differently under test than it does in the app — which is the whole point
//! of testing it against the routes rather than against a mock of them.

use crate::proto::{PullQuery, PullResponse, PushRequest, PushResponse};

/// A transport failure, in the only four shapes the engine reacts to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Nothing was reached. The outbox is kept and the pass is retried with backoff.
    #[error("the server could not be reached: {0}")]
    Network(String),
    /// This device's session is gone: the engine wipes and the app signs out (A186).
    #[error("this device's session has been revoked")]
    Revoked,
    /// The cursor is older than retained history: wipe and resync from zero (A166).
    #[error("the cursor is older than retained history")]
    Stale,
    /// The route answered, and refused.
    #[error("the server refused with {status}: {code}")]
    Server { status: u16, code: String },
}

impl TransportError {
    /// The error a sync route's response is.
    ///
    /// `401` and `410` are the two the engine acts on rather than reports: a revoked
    /// device and a stale cursor each have their own remedy, and neither is an error the
    /// user is shown (A166/A186). Everything else is a refusal the engine classifies by
    /// status when it decides whether retrying can help.
    pub fn from_status(status: u16, code: &str) -> Self {
        match status {
            401 => TransportError::Revoked,
            410 => TransportError::Stale,
            status => TransportError::Server {
                status,
                code: code.to_string(),
            },
        }
    }
}

/// The server's sync routes, as the engine needs them.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// One page of the caller's change log.
    async fn pull(&self, query: PullQuery) -> Result<PullResponse, TransportError>;

    /// One batch of the outbox.
    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError>;
}
