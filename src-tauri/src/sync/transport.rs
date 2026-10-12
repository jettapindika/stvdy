//! `HttpTransport`: the engine's [`Transport`] over `reqwest`, against the sync routes.
//!
//! Two things here are the shell's and not the engine's:
//!
//!   - **The pull's URI is built by the shared helper**, `stvdy_sync::client::testing::pull_uri`,
//!     so this transport cannot ask a subtly different question than the convergence suite
//!     asks — a transport that forgot `offset` would re-request the first page of a pass for
//!     ever, and that would be a bug the suite could not see.
//!   - **An expired access token is told apart from a revoked device.** Both are 401s, and
//!     the engine treats a 401 as a revoked device and wipes (A186). A refresh is the
//!     remedy for `token_expired`, not a wipe, so this transport emits the auth-expired
//!     notice and hands the engine the refusal as a `Server` error instead — which the
//!     engine shows as `needs_attention` with the server's own code.

use std::time::{Duration, Instant};

use reqwest::RequestBuilder;
use serde::de::DeserializeOwned;
use serde_json::Value;
use stvdy_sync::client::testing::pull_uri;
use stvdy_sync::client::{Transport, TransportError};
use stvdy_sync::proto::{ErrorCode, PullQuery, PullResponse, PushRequest, PushResponse};

use super::{Notifier, SyncNotice};

/// Long enough for a page of a large account over a slow link, short enough that a hung
/// request becomes an offline state the user can see rather than a pass that never ends.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The sync routes, over HTTP.
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
    token: String,
    notifier: Notifier,
}

impl HttpTransport {
    /// `base_url` is the API origin; a trailing slash is tolerated because the paths below
    /// start with one.
    pub fn new(
        base_url: &str,
        access_token: &str,
        notifier: Notifier,
    ) -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| TransportError::Network(error.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: access_token.to_string(),
            notifier,
        })
    }

    /// Sends a request and reads the answer, classifying both halves of the failure the
    /// engine knows about: nothing was reached, or the route refused.
    async fn answer<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        route: &'static str,
    ) -> Result<T, TransportError> {
        let started = Instant::now();
        let response = request
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| TransportError::Network(error.to_string()))?;
        let status = response.status().as_u16();
        let body: Value = response
            .json()
            .await
            .map_err(|error| TransportError::Network(error.to_string()))?;
        // The roadmap requires the latency of sync pull and push to be logged; the route
        // name and the status are what make a slow pass diagnosable.
        tracing::debug!(
            route,
            status,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "sync request"
        );

        if (200..300).contains(&status) {
            return serde_json::from_value(body).map_err(|error| TransportError::Server {
                status,
                code: error.to_string(),
            });
        }
        Err(self.refuse(status, &body))
    }

    /// The engine's reading of a refusal, plus the one case its own classifier cannot see.
    fn refuse(&self, status: u16, body: &Value) -> TransportError {
        let code = body
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == 401 && code == ErrorCode::TokenExpired.as_str() {
            // The shell has stopped the pass and is waiting: the SPA refreshes, reconfigures
            // and runs one pass. Nothing is wiped, because nothing is wrong with the cache.
            (self.notifier)(SyncNotice::AuthExpired);
            return TransportError::Server { status, code };
        }
        TransportError::from_status(status, &code)
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    async fn pull(&self, query: PullQuery) -> Result<PullResponse, TransportError> {
        let url = format!("{}{}", self.base_url, pull_uri(query));
        self.answer(self.client.get(url), "pull").await
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        let url = format!("{}/sync/push", self.base_url);
        self.answer(self.client.post(url).json(&request), "push")
            .await
    }
}
