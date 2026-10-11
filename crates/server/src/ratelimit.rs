//! Per-IP rate limiting for the auth endpoints.
//!
//! A12 fixes the shape: in-process token buckets per route class, explicitly per
//! instance rather than global, because the MVP runs one instance. A43's
//! *database-backed* per-IP and per-email-domain signup limiter is a Phase 7
//! gate; it is deliberately not half-built here, because a control that looks
//! like an abuse defence but resets on restart is worse than a documented gap.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::response::IntoResponse;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};

use crate::error::AppError;

/// Requests per minute per IP on the auth routes.
///
/// Generous relative to the refill, because a legitimate client retries a failed
/// login a few times: the point is to stop a script, not to annoy a user. This
/// is the *second* line of defence; the account-level cap in A43 is the first.
const AUTH_PER_MINUTE: u32 = 30;

/// The client's IP.
///
/// `X-Forwarded-For` is deliberately not trusted: with no proxy in front it is
/// attacker-controlled, and honouring it would let one client bypass the limit
/// by inventing a header. A deployment behind a load balancer needs its own
/// extractor, which is a decision for the phase that deploys one.
#[derive(Clone, Copy)]
pub struct PeerIp;

impl KeyExtractor for PeerIp {
    type Key = IpAddr;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip())
            // Tests drive the router with `oneshot` and have no socket, so they
            // all share one bucket. That is what makes a rate-limit test
            // possible without a listener.
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        Ok(peer)
    }
}

/// The layer for the auth routes, answering the documented error shape rather
/// than governor's plain-text default: the client's error mapper only knows the
/// `{code, message, ...}` contract.
///
/// A `Result` rather than a panic: the builder returns `None` only for a zero
/// period or burst, and a process that cannot rate-limit its auth endpoints
/// should fail to start rather than run unprotected.
pub fn auth_layer()
-> anyhow::Result<GovernorLayer<PeerIp, governor::middleware::NoOpMiddleware, axum::body::Body>> {
    let config = GovernorConfigBuilder::default()
        .key_extractor(PeerIp)
        .period(Duration::from_secs(60))
        .burst_size(AUTH_PER_MINUTE)
        .finish()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the auth rate limiter's period and burst size must be non-zero \
                 (period 60s, burst {AUTH_PER_MINUTE})"
            )
        })?;

    Ok(
        GovernorLayer::new(Arc::new(config)).error_handler(|error| match error {
            GovernorError::TooManyRequests { wait_time, .. } => {
                tracing::warn!(wait_time, "auth rate limit tripped");
                AppError::RateLimited.into_response()
            }
            other => AppError::Internal(anyhow::anyhow!("rate limiter: {other}")).into_response(),
        }),
    )
}
