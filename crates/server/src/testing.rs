//! Test scaffolding, compiled into the library so integration tests can build a
//! real `AppState` without a provider account, a bucket or a network.
//!
//! `expect` is allowed here on purpose: this module exists only for tests, and a
//! config that fails to load must be a loud panic at the first line of a test
//! rather than a `Result` every test has to thread through.
#![allow(clippy::expect_used)]
//!
//! The doubles live in `stvdy_core::traits::testing` and are reused here rather
//! than reimplemented, so a test double cannot drift from the trait it stands in
//! for. Only the pieces the doubles cannot cover (a real `AppConfig`, a real
//! Postgres pool) are built here.

use std::sync::Arc;

use sqlx::PgPool;
use stvdy_core::config::{PlanId, PlansFile, load_plans};
use stvdy_core::traits::clock::Clock;
use stvdy_core::traits::embedder::Embedder;
use stvdy_core::traits::mailer::Mailer;
use stvdy_core::traits::payment::{
    GatewayStatus, Invoice, InvoiceRequest, PaymentError, PaymentProvider, WebhookEvent,
};
use stvdy_core::traits::storage::Storage;
use stvdy_core::traits::testing::{
    FixedClock, HashEmbedder, InMemoryStorage, MockPayment, RecordingMailer,
};

use crate::config::AppConfig;
use crate::integrations::payment::{GatewayTransaction, PaymentGateway};
use crate::jobs::Queue;
use crate::state::AppState;

/// The repository root, from the crate manifest directory. Every relative config
/// path in `AppConfig` resolves against it.
pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A dev `AppConfig` with the values a test needs and nothing environment
/// dependent. `from_env_at` takes overrides precisely so tests never mutate the
/// process environment (Phase 0 execution note 11: it is global and racy).
pub fn config() -> AppConfig {
    AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            // The dev sink writes under var/mail; a test that inspects mail uses
            // the RecordingMailer instead, so this path is never used.
            ("MAIL_DIR", "var/mail"),
            ("STORAGE_BACKEND", "local"),
            ("STORAGE_LOCAL_DIR", "var/objects"),
            ("JWT_SECRET", "test-only-secret-that-is-long-enough"),
            ("ENCRYPTION_KEY_HEX", &"1".repeat(64)),
            ("ACCESS_TOKEN_TTL_MIN", "15"),
            ("REFRESH_TOKEN_TTL_DAYS", "30"),
            ("VERIFY_TOKEN_TTL_HOURS", "24"),
            ("PDFIUM_LIB_PATH", "var/pdfium/libpdfium.dylib"),
            // Pinned empty so a developer whose shell exports SOFFICE_BIN cannot make
            // the "no converter" case vacuous. The preview cases install a path
            // explicitly.
            ("SOFFICE_BIN", ""),
            ("GOOGLE_LOGIN_CLIENT_ID", ""),
            ("GOOGLE_LOGIN_CLIENT_SECRET", ""),
            ("GOOGLE_LOGIN_REDIRECT_URI", ""),
            // Search: pinned here rather than inherited, because `from_env_at`
            // falls back to the process environment for any key not in this list.
            // A developer whose `.env` pins `SEARCH_PROVIDER=tavily` with no key
            // would otherwise make every suite in this crate fail at boot with a
            // configuration error that has nothing to do with the test.
            ("SEARCH_PROVIDER", "composite"),
            ("SEARXNG_URL", ""),
            ("EXA_API_KEY", ""),
            ("SEARCH_API_KEY", ""),
            ("SEARCH_CACHE_TTL_HOURS", "24"),
            // Pinned like the search keys: a developer whose `.env` sets a retention
            // window would otherwise change what the sync suites compact.
            ("CHANGE_LOG_RETENTION_DAYS", "90"),
            ("CHANGE_LOG_TOMBSTONE_RETENTION_DAYS", "180"),
            // Pinned empty for the same reason, and with one more: a developer with a
            // real gateway in `.env` would otherwise make every suite in this crate
            // build a `QrisConfig` the test state does not use. A test that wants a
            // gateway builds its own config through `from_env_at`.
            ("QRIS_BASE_URL", ""),
            ("QRIS_API_KEY", ""),
            ("QRIS_CALLBACK_TOKEN", ""),
            ("QRIS_SIGNATURE_SCHEME", "hmac_sha256_hex"),
            ("QRIS_SIGNATURE_HEADER", "X-Callback-Signature"),
            ("QRIS_WEBHOOK_IPS", ""),
            ("QRIS_FEE_PERCENT", "0"),
            ("QRIS_FEE_FIXED_IDR", "0"),
            ("QRIS_INVOICE_TTL_MINUTES", "15"),
            ("QRIS_HONOR_LATE_PAYMENT", "true"),
            ("QRIS_RECONCILE_HOURS", "48"),
        ],
    )
    .expect("the test config must load")
}

pub fn plans() -> PlansFile {
    load_plans(&root().join("config/plans.toml")).expect("plans.toml must load")
}

/// A gateway that signs bodies like a real one **and** serves a scripted transaction
/// list, so the reconciliation sweep is testable.
///
/// It wraps [`MockPayment`] rather than reimplementing it: the signature is a real
/// HMAC over the bytes the test posts, which is the property every webhook test depends
/// on. The one thing `MockPayment` cannot do is answer `recent_transactions` — it is a
/// contract trait double in `crates/core`, and the listing is this server's own
/// extension — so the script for that lives here.
pub struct TestGateway {
    mock: MockPayment,
    transactions: std::sync::Mutex<std::collections::VecDeque<Vec<GatewayTransaction>>>,
}

impl TestGateway {
    /// A gateway signing with the mock's own secret.
    pub fn new() -> Self {
        Self {
            mock: MockPayment::new(),
            transactions: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    /// The headers a real gateway would send for this body.
    pub fn sign(&self, body: &[u8]) -> axum::http::HeaderMap {
        self.mock.sign(body)
    }

    /// The next status `get_status` will report.
    pub fn push_status(&self, status: GatewayStatus) {
        self.mock.push_status(status);
    }

    /// Every invoice request this double was handed, in order.
    pub fn invoices(&self) -> Vec<InvoiceRequest> {
        self.mock.invoices()
    }

    /// Queues one answer for `recent_transactions`. The last answer repeats, so a test
    /// that sweeps twice does not have to script the list twice.
    pub fn push_transactions(&self, list: Vec<GatewayTransaction>) {
        if let Ok(mut script) = self.transactions.lock() {
            script.push_back(list);
        }
    }
}

impl Default for TestGateway {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PaymentProvider for TestGateway {
    fn name(&self) -> &'static str {
        self.mock.name()
    }

    async fn create_invoice(&self, request: InvoiceRequest) -> Result<Invoice, PaymentError> {
        self.mock.create_invoice(request).await
    }

    async fn get_status(&self, order_id: &str) -> Result<GatewayStatus, PaymentError> {
        self.mock.get_status(order_id).await
    }

    fn parse_and_verify_webhook(
        &self,
        headers: &axum::http::HeaderMap,
        raw_body: &[u8],
    ) -> Result<WebhookEvent, PaymentError> {
        self.mock.parse_and_verify_webhook(headers, raw_body)
    }

    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError> {
        self.mock.refund_note(order_id, amount_idr).await
    }
}

#[async_trait::async_trait]
impl PaymentGateway for TestGateway {
    async fn recent_transactions(
        &self,
        _since_hours: i64,
    ) -> Result<Vec<GatewayTransaction>, PaymentError> {
        let mut script = self
            .transactions
            .lock()
            .map_err(|_| PaymentError::Provider("mock gateway lock poisoned".into()))?;
        // An empty script is an empty list, not an error: "the gateway knows nothing"
        // is a legitimate answer the sweep must handle, and a test that forgot to
        // script one should see the honest empty result rather than a panic.
        if script.len() > 1 {
            Ok(script.pop_front().unwrap_or_default())
        } else {
            Ok(script.front().cloned().unwrap_or_default())
        }
    }
}

/// An `AppState` backed by deterministic doubles.
///
/// Storage is `InMemoryStorage` and mail is `RecordingMailer`, so an ingestion
/// test reads back exactly the bytes it uploaded and a signup test reads the
/// verification link out of the captured message rather than out of a mailbox.
pub fn state(pool: PgPool) -> AppState {
    AppState {
        pool,
        config: Arc::new(config()),
        storage: Arc::new(InMemoryStorage::new()),
        embedder: Arc::new(HashEmbedder),
        mailer: Arc::new(RecordingMailer::default()),
        clock: Arc::new(FixedClock::at(chrono::Utc::now())),
        llm: None,
        search: None,
        renderer: None,
        fetcher: offline_fetcher(),
        ics_fetch: offline_ics_fetch(),
        // A real HMAC over the bytes a test posts, so a webhook test that passes
        // actually verified something. `TestState` keeps a handle to the same object.
        payment: Some(Arc::new(TestGateway::new())),
        push: Arc::new(crate::integrations::push::PushRouter::default()),
        jobs: Queue::new(),
    }
}

/// A fetch chain with nothing in it.
///
/// Deliberately empty rather than the real chain: a test that fetches without
/// installing a stub must fail with "no page fetcher is configured" rather than
/// reach the network, because a suite that quietly depends on a live page is a
/// suite that fails on a train.
pub fn offline_fetcher() -> Arc<crate::integrations::fetch::ChainFetcher> {
    Arc::new(crate::integrations::fetch::ChainFetcher::new(Vec::new()))
}

/// An ICS fetcher that refuses everything.
///
/// The same reasoning as [`offline_fetcher`]: a test that imports a URL without installing a
/// script must be refused rather than reach the network. `GuardedFetch` would resolve
/// `faculty.example` over real DNS before failing, which is exactly the quiet dependency a
/// suite must not have.
pub fn offline_ics_fetch() -> Arc<dyn crate::calendar::ics_in::IcsFetch> {
    Arc::new(ScriptedIcsFetch::new(vec![Scripted::Refused(
        "no ICS fetcher is installed in this test".to_string(),
    )]))
}

/// What a scripted ICS fetch should answer, in the order it is asked.
#[derive(Debug, Clone)]
pub enum Scripted {
    /// A `200` with a body and the validators to store.
    Body {
        body: String,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// A `304`: the caller's validators still match.
    NotModified,
    /// A refusal, as the guard would produce one.
    Refused(String),
}

/// One call the scripted fetcher received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcsCall {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A scripted ICS fetcher, so the route tests drive the import without a network.
///
/// A queue rather than a closure: the interesting assertion is *which request was sent
/// second* — A239's whole point is that the second poll carries `If-None-Match` — and a
/// closure that only returned a body would make the request invisible. The last scripted
/// answer repeats once the queue is exhausted, so a test that polls three times does not have
/// to script three bodies to assert on the second.
pub struct ScriptedIcsFetch {
    script: std::sync::Mutex<std::collections::VecDeque<Scripted>>,
    calls: std::sync::Mutex<Vec<IcsCall>>,
}

impl ScriptedIcsFetch {
    pub fn new(script: Vec<Scripted>) -> Self {
        Self {
            script: std::sync::Mutex::new(script.into()),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// A fetcher that answers `body` with `etag` forever, and never a `304`.
    pub fn always(body: &str, etag: Option<&str>) -> Self {
        Self::new(vec![Scripted::Body {
            body: body.to_string(),
            etag: etag.map(str::to_string),
            last_modified: None,
        }])
    }

    /// Every call received, in order.
    pub fn calls(&self) -> Vec<IcsCall> {
        self.calls.lock().expect("the call log").clone()
    }
}

#[async_trait::async_trait]
impl crate::calendar::ics_in::IcsFetch for ScriptedIcsFetch {
    async fn get(
        &self,
        url: &str,
        validators: Option<&crate::integrations::fetch::Validators>,
    ) -> Result<crate::integrations::fetch::GuardedText, stvdy_core::traits::search::FetchError>
    {
        use stvdy_core::traits::search::FetchError;

        self.calls.lock().expect("the call log").push(IcsCall {
            url: url.to_string(),
            etag: validators.and_then(|v| v.etag.clone()),
            last_modified: validators.and_then(|v| v.last_modified.clone()),
        });

        let mut script = self.script.lock().expect("the script");
        // The last answer repeats: a test that polls past its script is asking the same
        // question again, and an empty answer would make it look like an empty feed.
        let answer = if script.len() > 1 {
            script.pop_front()
        } else {
            script.front().cloned()
        }
        .expect("the scripted fetcher was given at least one answer");

        Ok(match answer {
            Scripted::Body {
                body,
                etag,
                last_modified,
            } => crate::integrations::fetch::GuardedText {
                final_url: url.to_string(),
                body,
                etag,
                last_modified,
                not_modified: false,
            },
            Scripted::NotModified => crate::integrations::fetch::GuardedText {
                final_url: url.to_string(),
                body: String::new(),
                etag: validators.and_then(|v| v.etag.clone()),
                last_modified: validators.and_then(|v| v.last_modified.clone()),
                not_modified: true,
            },
            Scripted::Refused(reason) => return Err(FetchError::Refused(reason)),
        })
    }
}

/// The same state with each provider handed out for assertions.
pub struct TestState {
    pub state: AppState,
    pub storage: Arc<InMemoryStorage>,
    pub mailer: Arc<RecordingMailer>,
    pub clock: Arc<FixedClock>,
    /// The gateway double, so a webhook test can sign the body it posts and a poll test
    /// can script the status the gateway will report.
    pub gateway: Arc<TestGateway>,
}

impl TestState {
    pub fn new(pool: PgPool) -> Self {
        // Best-effort: a `#[sqlx::test]` suite runs its cases in parallel threads,
        // so whichever case gets here first installs the subscriber and the rest
        // are no-ops. Without this an `AppError::Internal` reaches the test as a
        // bare 500 with the cause logged nowhere.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("stvdy_server=warn")),
            )
            .with_test_writer()
            .try_init();

        let storage = Arc::new(InMemoryStorage::new());
        let mailer = Arc::new(RecordingMailer::default());
        let clock = Arc::new(FixedClock::at(chrono::Utc::now()));
        let gateway = Arc::new(TestGateway::new());
        Self {
            state: AppState {
                pool,
                config: Arc::new(config()),
                storage: storage.clone(),
                embedder: Arc::new(HashEmbedder),
                mailer: mailer.clone(),
                clock: clock.clone(),
                llm: None,
                search: None,
                // Never launched in a test: the print step is the only thing that
                // needs a browser, and a suite that started Chromium per case would be
                // slow and would fail on a machine without one. The PDF routes answer
                // `provider_error` here, which is what a deployment without a browser
                // does too.
                renderer: None,
                fetcher: offline_fetcher(),
                ics_fetch: offline_ics_fetch(),
                payment: Some(gateway.clone()),
                // Empty by default, so a suite that does not ask for a transport gets the
                // deployment a developer actually has: in-app and email reminders deliver,
                // `channels_available` omits `push`, and no test accidentally depends on a
                // vendor it did not script.
                push: Arc::new(crate::integrations::push::PushRouter::default()),
                jobs: Queue::new(),
            },
            storage,
            mailer,
            clock,
            gateway,
        }
    }

    /// The seed plans, so `users.plan_id`'s FK is satisfiable inside a
    /// `#[sqlx::test]` scratch database.
    pub async fn seed_plans(&self) -> anyhow::Result<()> {
        crate::db::seed_plans(&self.state.pool, &self.state.config.plans).await
    }

    /// The same state with a scripted model provider, so the chat suite runs
    /// without a network. `ScriptedLlm` records the requests it received, which
    /// is what lets a test assert on the prompt the model actually saw.
    pub fn with_llm(mut self, llm: Arc<dyn stvdy_core::traits::llm::LlmProvider>) -> Self {
        self.state.llm = Some(llm);
        self
    }

    /// The same state with the clock pinned to an instant.
    ///
    /// The local-boundary tests need this: "the counter rolls at the user's own midnight"
    /// is a statement about an instant, and `FixedClock::at(now)` cannot make one. Both
    /// handles move together, so a test cannot read a different time than the state does.
    pub fn with_clock(mut self, clock: Arc<FixedClock>) -> Self {
        self.state.clock = clock.clone();
        self.clock = clock;
        self
    }

    /// The same state with a fetch chain of the test's own.
    ///
    /// The ingestion suite installs a stub here so the URL path is exercised end to
    /// end (guard, extraction, chunking) without a network, and the SSRF case
    /// installs a chain containing only the real `ReqwestFetcher`, which refuses a
    /// loopback target before it connects.
    pub fn with_fetcher(mut self, fetcher: Arc<crate::integrations::fetch::ChainFetcher>) -> Self {
        self.state.fetcher = fetcher;
        self
    }

    /// The same state with an ICS fetcher of the test's own.
    ///
    /// A route test **must** use this rather than pointing the real one at a local server: the
    /// real path's whole job is to refuse `127.0.0.1`, so a test that reached its stub through
    /// it would be proving the guard absent (A233). The refusal itself is asserted against
    /// `fetch_guarded_text` directly in `tests/ssrf.rs`.
    pub fn with_ics_fetch(mut self, fetch: Arc<dyn crate::calendar::ics_in::IcsFetch>) -> Self {
        self.state.ics_fetch = fetch;
        self
    }

    /// The same state with a different embedder.
    ///
    /// Needed by the Indonesian gate: the harness's `HashEmbedder` treats a query
    /// and a passage identically, so a test that ran retrieval through it could
    /// not catch a regression from `embed_query` back to `embed`, which is the
    /// silent failure that gate exists for. Swapping in the real model makes the
    /// assertion run on the path that ships.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.state.embedder = embedder;
        self
    }

    /// The same state with a search composite, so the web suite runs without a
    /// network.
    pub fn with_search(
        mut self,
        search: Arc<crate::integrations::search::composite::CompositeSearch>,
    ) -> Self {
        self.state.search = Some(search);
        self
    }

    /// The same state with a push router of the test's own.
    ///
    /// The reminder-job suite installs `ScriptedPush` here, which is the only way the
    /// delivery fan-out is assertable: the real transports need a vendor, and a test that
    /// pointed one at a local server would be testing `reqwest` rather than the handler's
    /// decisions about which device, which channel and which failure deletes a token.
    ///
    /// Both halves move together — the router and the config's `push` — because "is push
    /// available" is one question, and a state whose router had a transport while
    /// `channels_available` said otherwise would be a state that cannot exist in production.
    pub fn with_push(mut self, push: Arc<crate::integrations::push::PushRouter>) -> Self {
        self.state.push = push;
        self
    }

    /// The same state with no gateway at all.
    ///
    /// Both halves move together — the handle and the config — because "is there a
    /// gateway" is one question: a state whose config carries a gateway but whose
    /// handle is `None` would be a state that cannot exist in production.
    pub fn without_gateway(mut self) -> Self {
        self.state.payment = None;
        let mut config = (*self.state.config).clone();
        config.qris = None;
        self.state.config = Arc::new(config);
        self
    }

    /// The most recent verification link the recording mailer captured.
    pub fn last_verification_link(&self) -> Option<String> {
        self.mailer.sent().iter().rev().find_map(|message| {
            message
                .text
                .lines()
                .find(|line| line.contains("token="))
                .map(|line| line.trim().to_string())
        })
    }
}

/// Handy for a test that needs a specific plan.
pub fn plan_of(id: PlanId) -> stvdy_core::config::PlanConfig {
    plans().get(id).clone()
}

/// The trait objects, so a test can assert on the trait rather than on the
/// concrete double.
pub fn as_storage(state: &AppState) -> &dyn Storage {
    state.storage.as_ref()
}

pub fn as_embedder(state: &AppState) -> &dyn Embedder {
    state.embedder.as_ref()
}

pub fn as_mailer(state: &AppState) -> &dyn Mailer {
    state.mailer.as_ref()
}

pub fn as_clock(state: &AppState) -> &dyn Clock {
    state.clock.as_ref()
}
