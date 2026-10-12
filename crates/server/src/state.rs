//! Shared application state.

use std::sync::Arc;

use sqlx::PgPool;
use stvdy_core::traits::clock::Clock;
use stvdy_core::traits::embedder::Embedder;
use stvdy_core::traits::mailer::Mailer;
use stvdy_core::traits::storage::Storage;

use stvdy_core::traits::llm::LlmProvider;

use crate::config::AppConfig;
use crate::integrations::fetch::ChainFetcher;
use crate::integrations::search::composite::CompositeSearch;
use crate::jobs::Queue;
use crate::render::chrome::Renderer;

/// Provider handles and the job queue. Built once at startup by
/// `integrations::Providers::build` plus `jobs::spawn_worker`, so a handler
/// never has to reach for a global.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<AppConfig>,
    pub storage: Arc<dyn Storage>,
    pub embedder: Arc<dyn Embedder>,
    pub mailer: Arc<dyn Mailer>,
    pub clock: Arc<dyn Clock>,
    /// `None` when no model provider is configured: chat answers
    /// `provider_error` rather than failing at the first request with a
    /// transport error that looks like an outage.
    pub llm: Option<Arc<dyn LlmProvider>>,
    /// `None` only when the pinned tier is unconfigured. The composite itself
    /// needs no keys, so this is present on a default deployment.
    pub search: Option<Arc<CompositeSearch>>,
    /// The pooled Chromium, or `None` when no browser could be found.
    ///
    /// `None` is a supported state and not a degraded one: the only thing that needs a
    /// browser is the *print* step, and a cheatsheet without a PDF is still a ready
    /// document with its blocks stored (the re-render endpoint exists precisely so the
    /// print can be repeated later). A report's PDF export is the one route that answers
    /// `502 provider_error` without it, because there is nothing else it could answer.
    pub renderer: Option<Arc<Renderer>>,
    /// The page fetcher for a user-supplied URL.
    ///
    /// **Every** fetch of a URL a user gave us goes through this, never a bare
    /// `reqwest` call: the chain is what carries the SSRF guard (A18/A34), and a
    /// second fetch path would be a second place to forget it. Present on every
    /// deployment, because the chain's last step needs no key and no binary.
    pub fetcher: Arc<ChainFetcher>,
    /// The raw-text fetch for a user-supplied ICS URL (A233).
    ///
    /// Separate from [`fetcher`](Self::fetcher) on purpose, and not a second guard: the guard
    /// is shared (`fetch_guarded_text` calls the same `parse_target`/`resolve_public`/pinned
    /// client), while the *body handling* differs — `ChainFetcher` runs readability, which
    /// turns an ICS document into prose and imports nothing, silently. It is a trait object so
    /// the route tests can inject a fake; the real guard is exactly what refuses a local stub,
    /// so a test that pointed the real fetcher at one and passed would be proving the guard
    /// absent.
    pub ics_fetch: Arc<dyn crate::calendar::ics_in::IcsFetch>,
    /// The payment gateway, or `None` when the deployment has none (Phase 13).
    ///
    /// `None` is a supported state and not a degraded one: with no gateway there is
    /// nothing to sell, so [`AppState::payment`] answers 502 `provider_error` and every
    /// other route is unaffected. It is deliberately **not** a mock in production — a
    /// double's QR string is not money, and a signature we chose ourselves proves
    /// nothing about who sent a callback.
    pub payment: Option<Arc<dyn crate::integrations::payment::PaymentGateway>>,
    pub jobs: Queue,
}

impl AppState {
    pub fn plans(&self) -> &stvdy_core::config::PlansFile {
        &self.config.plans
    }

    /// The payment gateway, or the honest 502.
    ///
    /// One accessor for both callers (checkout and the webhook), so a deployment with no
    /// gateway cannot answer one way at checkout and another way at the callback. The
    /// error is `provider_error`, not `not_found`: nothing is missing from the request —
    /// the *deployment* cannot take money, and an operator reading the log must be able
    /// to tell the two apart.
    pub fn payment(
        &self,
    ) -> crate::error::AppResult<&Arc<dyn crate::integrations::payment::PaymentGateway>> {
        self.payment.as_ref().ok_or_else(|| {
            crate::error::AppError::Provider(
                "no payment gateway is configured on this deployment".to_string(),
            )
        })
    }

    /// The gateway's configuration, which is present exactly when [`AppState::payment`]
    /// is. It carries the invoice lifetime, the late-payment switch and the reconcile
    /// window — the three values that are the gateway's without being a field name.
    pub fn qris(&self) -> crate::error::AppResult<&crate::config::QrisConfig> {
        self.config.qris.as_ref().ok_or_else(|| {
            crate::error::AppError::Provider(
                "no payment gateway is configured on this deployment".to_string(),
            )
        })
    }
}
