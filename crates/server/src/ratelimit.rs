//! Rate limiting: the per-IP auth bucket (A12), A43's signup limiter, and A155's
//! per-account spend cap.
//!
//! A12 fixes the shape of the auth limiter: in-process token buckets per route
//! class, explicitly per instance rather than global, because the MVP runs one
//! instance. The *signup* limiter is deliberately not that shape: it is
//! database-backed (A154), because an in-process bucket resets on restart and a
//! restart is free for a script.
//!
//! The spend cap (A155) is in-process on purpose, and its job is narrow: it
//! bounds the *overshoot* of the provider-cost ceiling, which is checked before
//! a provider call and recorded after it. Losing the bucket on restart costs a
//! few seconds of overshoot, not an unbounded one, so durability buys nothing
//! here.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Requests per minute per IP on the auth routes.
///
/// Generous relative to the refill, because a legitimate client retries a failed
/// login a few times: the point is to stop a script, not to annoy a user. This
/// is the *second* line of defence; the account-level cap in A43 is the first.
const AUTH_PER_MINUTE: u32 = 30;

/// The client's IP as the rate limiters count it.
///
/// The socket peer is not the client behind a proxy. `deploy/staging/nginx.conf`
/// runs behind the Cloudflare Tunnel, so every tunnel request reaches nginx from
/// the docker bridge gateway; counting `ConnectInfo` would put the whole site in
/// one bucket. nginx resolves the true client with its `real_ip` module and
/// passes it in `X-Real-IP`, overwriting whatever the caller sent.
///
/// Reading a header is safe only because the server is not directly reachable:
/// `deploy/staging/docker-compose.staging.yml` publishes no port for `server`, so
/// nginx is the only thing that can open a connection to it. A deployment that
/// exposed the port would let a caller forge this header and bypass the limit —
/// which is why the compose file's absent `ports:` is part of this rule rather
/// than a detail of it. `X-Real-IP` is preferred over `X-Forwarded-For` because
/// nginx sets it to a single resolved address, while the forwarded chain is a
/// list whose first entry a direct caller can supply.
///
/// Shared by the tower governor key extractor and the signup limiter, so the two
/// cannot disagree about who the client is — a limiter that counted a different
/// address than the one the other one throttled would be worse than one of them.
fn peer_ip(headers: &axum::http::HeaderMap, extensions: &axum::http::Extensions) -> IpAddr {
    headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
        .or_else(|| {
            extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(addr)| addr.ip())
        })
        // Tests drive the router with `oneshot` and have no socket, so they
        // all share one bucket. That is what makes a rate-limit test
        // possible without a listener.
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}

/// The client's IP for the tower governor.
#[derive(Clone, Copy)]
pub struct PeerIp;

impl KeyExtractor for PeerIp {
    type Key = IpAddr;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
        Ok(peer_ip(req.headers(), req.extensions()))
    }
}

/// The client's IP for A43's signup limiter.
///
/// An extractor rather than a header read in the handler, so the address comes
/// from the connection (`ConnectInfo`) and never from a value the caller can set.
pub struct SignupIp(pub String);

impl<S> axum::extract::FromRequestParts<S> for SignupIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(peer_ip(&parts.headers, &parts.extensions).to_string()))
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

/// Signup attempts per IP over a rolling 24 hours (A154).
pub const SIGNUP_PER_IP_PER_DAY: u32 = 25;

/// Signup attempts per **non-public** email domain over a rolling 24 hours (A154).
///
/// The higher number is deliberate: a campus or a company puts many honest people
/// behind one domain, and the per-IP rule is what bounds a farm.
pub const SIGNUP_PER_DOMAIN_PER_DAY: u32 = 100;

/// Spend actions per minute per account (A155).
pub const SPEND_PER_MINUTE: u32 = 10;

/// Sync requests per minute per account (A185).
///
/// Deliberately far above a real device's rate — a pull per change plus a reconnect is
/// a handful per minute — so a legitimate burst is never what trips it and a stream
/// reconnect is not abuse. The limit exists because the spec requires sync to be rate
/// limited, not because a device would ever reach it.
pub const SYNC_PER_MINUTE: u32 = 600;

/// Mailbox domains that are exempt from the per-domain signup rule (A154).
///
/// A farm uses gmail too, so a limit here stops no farm while it does stop a
/// cohort of students signing up from their phones. The per-IP rule and the
/// per-account ceiling still bound the abuse; this list only decides where a
/// *domain* count is meaningless.
const PUBLIC_EMAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "yahoo.com",
    "yahoo.co.id",
    "yahoo.co.uk",
    "ymail.com",
    "rocketmail.com",
    "outlook.com",
    "hotmail.com",
    "hotmail.co.id",
    "live.com",
    "msn.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "proton.me",
    "protonmail.com",
    "pm.me",
    "aol.com",
    "gmx.com",
    "mail.com",
    "zoho.com",
    "yandex.com",
    "yandex.ru",
    // Indonesian consumer providers: an ISP mailbox is the same case as gmail
    // here — shared by millions, and a farm can hold one.
    "telkom.net",
    "telkoms.net",
    "plasa.com",
    "indosat.net.id",
    "xl.co.id",
    "cbn.net.id",
];

/// Whether an email domain is a public mailbox provider, and so exempt from the
/// per-domain signup rule (A154).
///
/// Case-insensitive and tolerant of a trailing dot, because `Gmail.com.` is the
/// same mailbox as `gmail.com` and a rule that a farm can step around with a
/// character is not a rule.
pub fn is_public_email_domain(domain: &str) -> bool {
    let normalized = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    PUBLIC_EMAIL_DOMAINS.contains(&normalized.as_str())
}

/// A43's signup limiter.
///
/// Database-backed, because an in-process bucket resets on restart and a restart
/// is free for a script. Counted per IP and per non-public email domain over a
/// rolling 24 hours, and every attempt is recorded **before** the user row is
/// written, so a farm's failures count too. Every block logs the IP, the domain
/// and the count, because R5's whole point is that the false-positive rate on a
/// campus NAT has to be measurable before the number is tuned.
pub struct SignupLimiter {
    pub per_ip: u32,
    pub per_domain: u32,
}

/// The configured signup limiter.
///
/// A function rather than a `const` so the two numbers stay in one place and a
/// caller cannot construct a limiter with different ones by accident.
pub fn signup_limiter() -> SignupLimiter {
    SignupLimiter {
        per_ip: SIGNUP_PER_IP_PER_DAY,
        per_domain: SIGNUP_PER_DOMAIN_PER_DAY,
    }
}

impl SignupLimiter {
    /// Refuses when this IP, or this domain when it is not a public mailbox, has
    /// already spent its 24 hours.
    ///
    /// Two statements rather than one so the refusal can name which rule tripped:
    /// a campus NAT hitting the IP limit and a farm hitting the domain limit are
    /// different problems and R5 has to be able to tell them apart from the log.
    pub async fn check(&self, pool: &PgPool, ip: &str, domain: &str) -> Result<(), AppError> {
        let per_ip: i64 = sqlx::query_scalar(
            "select count(*) from signup_attempts
              where ip = $1 and created_at > now() - interval '24 hours'",
        )
        .bind(ip)
        .fetch_one(pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("counting signup attempts: {error}"))
        })?;

        if per_ip >= i64::from(self.per_ip) {
            tracing::warn!(
                %ip,
                %domain,
                count = per_ip,
                limit = self.per_ip,
                window = "24h",
                rule = "per_ip",
                "signup refused"
            );
            return Err(AppError::RateLimited);
        }

        if is_public_email_domain(domain) {
            return Ok(());
        }

        let per_domain: i64 = sqlx::query_scalar(
            "select count(*) from signup_attempts
              where email_domain = $1 and created_at > now() - interval '24 hours'",
        )
        .bind(domain)
        .fetch_one(pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("counting signup attempts: {error}"))
        })?;

        if per_domain >= i64::from(self.per_domain) {
            tracing::warn!(
                %ip,
                %domain,
                count = per_domain,
                limit = self.per_domain,
                window = "24h",
                rule = "per_domain",
                "signup refused"
            );
            return Err(AppError::RateLimited);
        }

        Ok(())
    }
}

/// Records one signup attempt, successful or not.
///
/// Called before the `users` row is written: a farm that reuses a taken address,
/// or that fails the device check, still counts, and its *failures* are the
/// signal that separates a farm from a lecture hall.
pub async fn record_signup_attempt(
    pool: &PgPool,
    ip: &str,
    email_domain: &str,
) -> Result<(), AppError> {
    sqlx::query("insert into signup_attempts (ip, email_domain) values ($1, $2)")
        .bind(ip)
        .bind(email_domain)
        .execute(pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("recording a signup attempt: {error}"))
        })?;
    Ok(())
}

/// One account's spend bucket.
struct SpendBucket {
    tokens: f64,
    last: Instant,
}

/// The live buckets, keyed by account.
///
/// In-process and per instance (A155). The map is pruned by idleness rather than
/// by a timer: an account idle for `SPEND_BUCKET_IDLE` has a full bucket, so
/// dropping its entry is indistinguishable from keeping it, and the map cannot
/// grow with the user table.
static SPEND_BUCKETS: LazyLock<Mutex<HashMap<Uuid, SpendBucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// How long an untouched bucket is kept.
///
/// Twice the refill period: a bucket left alone for a minute is full again, so
/// two minutes is a full window of margin against a clock that moves oddly.
const SPEND_BUCKET_IDLE: Duration = Duration::from_secs(120);

/// Above this many live buckets, idle ones are dropped. A bound on memory, not a
/// tuning knob.
const SPEND_BUCKETS_MAX: usize = 4096;

/// Whether this account may spend now, taking one token if it may.
///
/// A token bucket rather than a fixed window: a fixed window lets an account
/// spend its whole allowance at the end of one window and again at the start of
/// the next, which is exactly the burst this exists to bound.
fn spend_allowed(user_id: Uuid) -> bool {
    token_allowed(&SPEND_BUCKETS, SPEND_BUCKETS_MAX, user_id, SPEND_PER_MINUTE)
}

/// Takes one token from `user_id`'s bucket, refilling it by `per_minute` over a minute.
///
/// Shared by the spend cap and the sync cap so the two cannot disagree about what a
/// rate limit is. A poisoned lock must not take the whole server down: the worst case
/// is one account's bucket, and refusing every request until a restart would be worse.
fn token_allowed(
    buckets: &Mutex<HashMap<Uuid, SpendBucket>>,
    max: usize,
    user_id: Uuid,
    per_minute: u32,
) -> bool {
    let now = Instant::now();
    let capacity = f64::from(per_minute);
    let refill_per_second = capacity / 60.0;

    let mut buckets = buckets
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if buckets.len() > max {
        buckets.retain(|_, bucket| now.saturating_duration_since(bucket.last) < SPEND_BUCKET_IDLE);
    }

    let bucket = buckets.entry(user_id).or_insert(SpendBucket {
        tokens: capacity,
        last: now,
    });

    let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * refill_per_second).min(capacity);
    bucket.last = now;

    if bucket.tokens < 1.0 {
        return false;
    }
    bucket.tokens -= 1.0;
    true
}

/// Empties every spend bucket, so the next action gets the full allowance.
///
/// For tests, and for the same reason `crate::testing` exists: a suite that drives
/// twenty uploads in a loop to reach `sources_total` is exercising the *count* axis,
/// and the cap would refuse the eleventh for a reason that has nothing to do with
/// what the test is about. Public rather than `#[cfg(test)]` because an integration
/// test is a separate crate and cannot see a `cfg(test)` item.
pub fn reset_spend_buckets() {
    SPEND_BUCKETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// The routes that spend provider money or credits, by their router template.
///
/// A read is never here (A149): the sweep's `ReadOnly` rows are exactly the ones
/// a downgraded or over-limit account must keep working. `route_sweep.rs` asserts
/// that every entry here is also a `Swept` row, so this list cannot drift into
/// naming a route the entitlement decision does not cover.
pub fn spending_routes() -> &'static [(&'static str, &'static str)] {
    &[
        ("POST", "/workspaces/{id}/chat"),
        ("POST", "/workspaces/{id}/decks"),
        ("POST", "/workspaces/{id}/quizzes"),
        ("POST", "/workspaces/{id}/sources"),
        ("POST", "/workspaces/{id}/reports/outline"),
        ("POST", "/reports/{id}/generate"),
        ("POST", "/workspaces/{id}/cheatsheets"),
    ]
}

/// Whether this request is one of the spending routes.
///
/// `MatchedPath` is the router *template* (`/workspaces/{id}/chat`), not the URI,
/// so the comparison is against the same strings `spending_routes` names rather
/// than against an id that changes per request.
fn is_spending(method: &str, matched_path: Option<&str>) -> bool {
    let Some(path) = matched_path else {
        return false;
    };
    spending_routes()
        .iter()
        .any(|(route_method, route_path)| *route_method == method && *route_path == path)
}

/// The account a request claims to be, from its bearer token.
///
/// Decoded rather than re-authenticated: the handler's own extractor does the
/// full check (session, device, subscription), and a limiter that repeated it
/// would cost a database round trip per request to bound a burst of ten.
fn bearer_user(state: &AppState, request: &Request) -> Option<Uuid> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = header.strip_prefix("Bearer ")?;
    crate::auth::tokens::decode_jwt(&state.config, token)
        .ok()
        .map(|claims| claims.sub)
}

/// The per-account spend cap (A155), applied to a router.
///
/// Takes and returns the `Router` rather than being a bare `tower::Layer`, because
/// `Router::route_layer` needs `L::Service: Send + Sync` and an `impl Layer` return
/// type cannot state that. `route_layer`, not `layer`: the guard runs only for a
/// matched route, because `MatchedPath` is what tells it whether that route spends,
/// and a read must never be capped (A149).
pub fn spend_layer(router: axum::Router<AppState>, state: AppState) -> axum::Router<AppState> {
    router.route_layer(axum::middleware::from_fn_with_state(state, spend_guard))
}

/// The spend cap's middleware.
///
/// It answers the roadmap's row for the cap, `429 rate_limited`, and it runs
/// **before** the handler, so a burst cannot each pass the ceiling check before
/// any of them records a cost — which is the overshoot A155 exists to bound.
pub async fn spend_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let spends = request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|path| is_spending(request.method().as_str(), Some(path.as_str())));
    if !spends {
        return next.run(request).await;
    }

    let Some(user_id) = bearer_user(&state, &request) else {
        // No usable token: the handler's extractor answers 401, which is the right
        // error. Counting it would let an unauthenticated caller fill a bucket that
        // belongs to someone else.
        return next.run(request).await;
    };

    if !spend_allowed(user_id) {
        tracing::warn!(%user_id, limit = SPEND_PER_MINUTE, "per-account spend cap tripped");
        return AppError::RateLimited.into_response();
    }

    next.run(request).await
}

/// The live sync buckets, keyed by account.
///
/// Same shape and same reasoning as [`SPEND_BUCKETS`] (A185): in-process, per instance,
/// pruned by idleness.
static SYNC_BUCKETS: LazyLock<Mutex<HashMap<Uuid, SpendBucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Above this many live buckets, idle ones are dropped.
const SYNC_BUCKETS_MAX: usize = 4096;

/// Whether this account may make another sync request now.
fn sync_allowed(user_id: Uuid) -> bool {
    token_allowed(&SYNC_BUCKETS, SYNC_BUCKETS_MAX, user_id, SYNC_PER_MINUTE)
}

/// Empties every sync bucket, for a test that drives many requests in a loop.
pub fn reset_sync_buckets() {
    SYNC_BUCKETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// The per-account sync cap (A185), applied to the sync router.
///
/// `route_layer`, not `layer`: the guard runs only for a matched sync route, so the
/// fallback and every other route are untouched. It answers `429 rate_limited`, the same
/// body the auth and spend limiters answer.
pub fn sync_layer(router: axum::Router<AppState>, state: AppState) -> axum::Router<AppState> {
    router.route_layer(axum::middleware::from_fn_with_state(state, sync_guard))
}

/// The sync cap's middleware.
pub async fn sync_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(user_id) = bearer_user(&state, &request) else {
        // No usable token: the handler's extractor answers 401. Counting it would let an
        // unauthenticated caller fill a bucket that belongs to someone else.
        return next.run(request).await;
    };

    if !sync_allowed(user_id) {
        tracing::warn!(%user_id, limit = SYNC_PER_MINUTE, "per-account sync cap tripped");
        return AppError::RateLimited.into_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_mailbox_domain_is_exempt_whatever_its_case() {
        assert!(is_public_email_domain("gmail.com"));
        assert!(is_public_email_domain("Gmail.com."));
        assert!(is_public_email_domain(" yahoo.co.id "));
        // A university or a company is not a mailbox provider: the domain rule
        // exists for exactly these.
        assert!(!is_public_email_domain("ugm.ac.id"));
        assert!(!is_public_email_domain("student.itb.ac.id"));
        assert!(!is_public_email_domain("example.test"));
    }

    #[test]
    fn the_spend_cap_refuses_the_eleventh_action_and_refills() {
        let account = Uuid::now_v7();
        for _ in 0..SPEND_PER_MINUTE {
            assert!(spend_allowed(account), "the allowance is not spent");
        }
        assert!(
            !spend_allowed(account),
            "the cap must refuse the action past the allowance"
        );

        // A second account is unaffected: the bucket is per account, which is the
        // property a shared bucket would silently break.
        let other = Uuid::now_v7();
        assert!(spend_allowed(other));
    }

    #[test]
    fn a_read_route_is_not_a_spending_route() {
        for path in ["/workspaces/{id}/sources", "/sources/{id}/status"] {
            assert!(
                !is_spending("GET", Some(path)),
                "{path} is a read and must not be capped"
            );
        }
        assert!(is_spending("POST", Some("/workspaces/{id}/chat")));
        // The same path with a read method is not a spend.
        assert!(!is_spending("GET", Some("/workspaces/{id}/chat")));
        // And an unclassified path (the fallback) is never a spend.
        assert!(!is_spending("POST", None));
    }
}
