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
use stvdy_core::traits::storage::Storage;
use stvdy_core::traits::testing::{FixedClock, HashEmbedder, InMemoryStorage, RecordingMailer};

use crate::config::AppConfig;
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
        ],
    )
    .expect("the test config must load")
}

pub fn plans() -> PlansFile {
    load_plans(&root().join("config/plans.toml")).expect("plans.toml must load")
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
        jobs: Queue::new(),
    }
}

/// The same state with each provider handed out for assertions.
pub struct TestState {
    pub state: AppState,
    pub storage: Arc<InMemoryStorage>,
    pub mailer: Arc<RecordingMailer>,
    pub clock: Arc<FixedClock>,
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
                jobs: Queue::new(),
            },
            storage,
            mailer,
            clock,
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
