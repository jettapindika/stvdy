//! Route table assembly.

pub mod health;

use axum::Router;
use axum::http::HeaderValue;
use axum::routing::get;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::TraceLayer;

use crate::config::AppConfig;
use crate::state::AppState;

/// Origins the native shell presents. These are NOT `http://localhost`: Tauri
/// serves the SPA from its own scheme, so omitting them is the classic
/// "works in dev, every desktop request fails in prod" bug.
const TAURI_ORIGINS: &[&str] = &[
    "tauri://localhost",       // macOS, iOS
    "https://tauri.localhost", // Windows
    "http://tauri.localhost",  // Windows, Android
];

fn cors_layer(config: &AppConfig) -> CorsLayer {
    if config.env.is_dev() {
        // Dev only: the Vite dev server and the Tauri webviews vary by machine
        // and port. Note `permissive()` sets no `allow_credentials`, so browsers
        // will not attach cookies even with a wildcard origin. And auth here is
        // a bearer header, not a cookie.
        return CorsLayer::permissive();
    }

    let mut origins: Vec<HeaderValue> = TAURI_ORIGINS
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    if let Some(web) = &config.web_origin {
        match HeaderValue::from_str(web) {
            Ok(value) => origins.push(value),
            Err(error) => {
                // Misconfiguration must be loud, but not fatal: the server still
                // serves the API to the native shell.
                tracing::error!(%error, origin = %web, "WEB_ORIGIN is not a valid header value; ignoring");
            }
        }
    }

    tracing::info!(count = origins.len(), "CORS restricted to known origins");
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(tower_http::cors::Any)
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
        ])
}

pub fn router(state: AppState) -> Router {
    let cors = cors_layer(&state.config);

    Router::new()
        .route("/health", get(health::health))
        .fallback(health::not_found)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(SetSensitiveRequestHeadersLayer::new(std::iter::once(
            axum::http::header::AUTHORIZATION,
        )))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .layer(cors)
        .with_state(state)
}
