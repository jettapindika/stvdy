//! Route table assembly.
//!
//! Three groups, because they carry different guarantees:
//!   * public: `/health` and the OAuth start/callback pair.
//!   * rate limited: everything under `/auth` that takes credentials.
//!   * authenticated: the rest, each handler taking `AuthUser` or `VerifiedUser`
//!     so scoping is decided by the extractor rather than by remembering to add
//!     a `where user_id = $1`.

pub mod health;

use axum::Router;
use axum::http::HeaderValue;
use axum::routing::{get, post};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::TraceLayer;

use crate::assets;
use crate::auth;
use crate::calendar;
use crate::chat;
use crate::cheatsheets;
use crate::chunks;
use crate::config::AppConfig;
use crate::devices;
use crate::events;
use crate::flashcards;
use crate::goals;
use crate::planner;
use crate::quizzes;
use crate::reminders;
use crate::reports;
use crate::settings;
use crate::sources;
use crate::state::AppState;
use crate::study;
use crate::syllabus;
use crate::sync;
use crate::tasks;
use crate::workspaces;
use crate::{admin, usage};

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

/// Builds the router.
///
/// Returns a `Result` because the rate limiter's configuration is validated
/// here: a process that cannot limit its auth endpoints must fail to start
/// rather than serve them unprotected.
pub fn router(state: AppState) -> anyhow::Result<Router> {
    let cors = cors_layer(&state.config);
    let auth_limiter = crate::ratelimit::auth_layer()?;

    // Credential-taking routes. Separate from the rest so the limiter cannot be
    // lost by a later `.merge()` that forgets to re-apply it.
    let auth_routes = Router::new()
        .route("/auth/signup", post(auth::routes::signup))
        .route("/auth/login", post(auth::routes::login))
        .route("/auth/refresh", post(auth::routes::refresh))
        .route("/auth/logout", post(auth::routes::logout))
        .route("/auth/verify-email", post(auth::routes::verify_email))
        // Google's callback is a browser redirect, so it is a GET and it is not
        // rate limited by peer IP: the peer is Google's redirect, not the user.
        .route("/auth/google/exchange", post(auth::routes::google_exchange))
        .layer(auth_limiter.clone());

    let public_routes = Router::new()
        .route("/health", get(health::health))
        .route("/auth/google/start", get(auth::routes::google_start))
        .route("/auth/google/callback", get(auth::routes::google_callback))
        // The gateway's callback. No bearer token: the gateway has no session, and the
        // signature over the raw body is its authorisation (A292/A309). It is in the
        // public group rather than the authenticated one because a route that
        // authenticates by HMAC and one that authenticates by session are different
        // things, and a shared `.merge()` is how the wrong one gets a layer it must not
        // have.
        .merge(crate::billing::routes::public_routes())
        // The print assets. Public because the browser that fetches them is headless
        // Chromium rendering a document with no credentials, and because every file is a
        // vendored upstream release with no user data in it (A339). The renderer rewrites
        // the template's CDN references to this path before printing, so a PDF never
        // depends on a third-party origin being reachable.
        .merge(assets::routes());

    // The feed carries no bearer token, so it is not an authenticated route — but it *is*
    // an unauthenticated database lookup keyed by a secret, which makes it a brute-force
    // target, so it shares the credential bucket rather than the unlimited public group
    // (A243). The same limiter instance, not a second one: two buckets for one client would
    // double the attempts a script gets.
    //
    // The student-verification route is here for a different reason: it *sends mail* to an
    // address the caller supplies. The suffix list is the content rule and this is the
    // volume rule (A299) — without it the endpoint is a mail amplifier pointed at whatever
    // campus domain the list accepts.
    let limited_public_routes = Router::new()
        .merge(calendar::public_routes())
        .merge(crate::billing::routes::limited_routes())
        .layer(auth_limiter);

    let authenticated = Router::new()
        .route("/auth/me", get(auth::routes::me))
        .route("/auth/logout-all", post(auth::routes::logout_all))
        .route(
            "/auth/resend-verification",
            post(auth::routes::resend_verification),
        )
        .route(
            "/auth/google/set-password",
            post(auth::routes::google_set_password),
        )
        // Each feature module owns its own paths, so a new route in one file
        // cannot be lost by forgetting to register it here.
        .merge(devices::routes())
        .merge(workspaces::routes())
        .merge(chunks::routes())
        .merge(chat::routes())
        .merge(flashcards::routes())
        .merge(quizzes::routes())
        .merge(reports::routes())
        .merge(cheatsheets::routes())
        .merge(tasks::routes())
        .merge(events::routes())
        .merge(calendar::routes())
        .merge(study::routes())
        .merge(reminders::routes())
        .merge(settings::routes())
        .merge(crate::actions::routes())
        .merge(usage::routes())
        .merge(admin::routes())
        .merge(crate::billing::routes::routes())
        .merge(sources::routes(sources::max_upload_bytes(
            &state.config.plans,
        )))
        // Goals are not plan-gated (the free plan has goals and a manual planner); the two
        // generating routes under `planner` are, and they carry their own refusal.
        .merge(goals::routes())
        .merge(planner::routes::routes())
        // The same body limit the generic upload gets, for the same reason: the plan check is
        // what produces the precise refusal, and this only stops a body no plan could accept
        // from being read at all. A syllabus is a PDF or pasted text, so the ceiling is the
        // upload's.
        .merge(syllabus::routes(sources::max_upload_bytes(
            &state.config.plans,
        )))
        // The sync routes carry their own per-account cap (A185), so they are merged
        // with the layer already applied rather than into the shared chain.
        .merge(crate::ratelimit::sync_layer(sync::routes(), state.clone()));

    // A155's per-account spend cap, applied to the merged authenticated routes.
    // After the merge so it covers every spending route and no read: the guard
    // decides from `MatchedPath`, and `route_layer` means the fallback never sees it.
    let authenticated = crate::ratelimit::spend_layer(authenticated, state.clone());

    let router = Router::new()
        .merge(public_routes)
        .merge(limited_public_routes)
        .merge(auth_routes)
        .merge(authenticated)
        .fallback(health::not_found)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(SetSensitiveRequestHeadersLayer::new(std::iter::once(
            axum::http::header::AUTHORIZATION,
        )))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .layer(cors)
        .with_state(state);

    Ok(router)
}
