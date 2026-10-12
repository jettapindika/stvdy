//! Calendar: the ICS feed out, the ICS import, the Google consent flow and the
//! two-way sync.
//!
//! [`routes`] returns the authenticated half. [`public_routes`] returns the one route that
//! carries no bearer token, and it is separate rather than merged into the authenticated
//! group so `routes::mod` places it deliberately — beside the credential routes, in the same
//! rate-limit bucket, because it is an unauthenticated database lookup keyed by a secret
//! (A243).

pub mod ics_out;

use axum::Router;

use crate::state::AppState;

/// The authenticated calendar routes: the feed token's read and rotate.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/calendar/ics-token", axum::routing::get(ics_out::token))
        .route(
            "/calendar/ics-token/regenerate",
            axum::routing::post(ics_out::regenerate_token),
        )
}

/// The routes with no authenticated user: `GET /cal/{token}`.
///
/// Separate from [`routes`] so the caller can put it in `public_routes` **with** the auth
/// rate limiter applied, rather than merging everything and losing the limiter.
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/cal/{token}", axum::routing::get(ics_out::feed))
}
