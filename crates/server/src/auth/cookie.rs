//! Session cookie helpers for web-based auth persistence.
//!
//! The cookie carries `{refresh_token}:{device_id}` in an HttpOnly cookie at
//! `Path=/`. The path is root because the server is reached at `/auth/…` in dev
//! (direct `localhost:8080`) and `/api/auth/…` in prod (behind the nginx proxy
//! that strips `/api`). A narrower path would miss one of the two. The cookie
//! is HttpOnly, so no script can read it regardless of path. Native clients
//! ignore it and keep sending tokens in the JSON body.

use axum::http::HeaderValue;
use chrono::{DateTime, Utc};

const COOKIE_NAME: &str = "stvdy_session";

/// Builds a `Set-Cookie` header value that carries the refresh token and device id.
pub fn session_cookie(
    refresh_token: &str,
    device_id: &str,
    expires_at: DateTime<Utc>,
    is_dev: bool,
) -> HeaderValue {
    let max_age = (expires_at - Utc::now()).num_seconds().max(0);
    let secure = if is_dev { "" } else { "; Secure" };
    let value = format!(
        "{COOKIE_NAME}={refresh_token}:{device_id}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}"
    );
    // The value is ASCII-safe: base64url tokens, a UUID, and fixed cookie
    // attributes produce no non-visible-ASCII byte. A non-ASCII refresh token
    // from a broken issuer is a server bug, not a user input, so the panic in
    // debug and the allow in release are both acceptable.
    #[allow(clippy::expect_used)]
    HeaderValue::from_str(&value).expect("cookie value is valid ASCII")
}

/// Builds a `Set-Cookie` that clears the session cookie.
pub fn clear_cookie(is_dev: bool) -> HeaderValue {
    let secure = if is_dev { "" } else { "; Secure" };
    let value = format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure}");
    #[allow(clippy::expect_used)]
    HeaderValue::from_str(&value).expect("cookie value is valid ASCII")
}

/// Parses the refresh token and device id from the `Cookie` header.
/// Returns `None` if the cookie is absent or malformed.
pub fn parse_session_cookie(cookie_header: &str) -> Option<(String, String)> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("stvdy_session=") {
            let (refresh, device) = value.split_once(':')?;
            if refresh.is_empty() || device.is_empty() {
                return None;
            }
            return Some((refresh.to_string(), device.to_string()));
        }
    }
    None
}
