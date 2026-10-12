use std::sync::Arc;

use anyhow::Context;
use stvdy_server::integrations::Providers;
use stvdy_server::{config::AppConfig, credits, db, jobs, routes, state::AppState, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` for local development so the documented quickstart
    // (`cp .env.example .env && cargo run -p stvdy-server`) actually works.
    // dotenvy never overrides variables that are already set, so a real
    // deployment's environment (and CI) always wins over a stray file.
    let _ = dotenvy::dotenv();

    // Two modes that exist for the deployment, not for a developer, and they are parsed
    // before the pool is opened so a typo in a flag is not discovered after a migration has
    // already run (A346).
    //
    // `--migrate-only` is the deploy procedure's first step, and it is a deployment contract
    // rather than a convenience: two replicas starting together both run `sqlx::migrate!`
    // against one database, and the advisory lock makes that survivable but leaves the second
    // replica serving traffic against a schema it has not verified. Running this to
    // completion *before* any replica starts is what removes the race, so an orchestrator must
    // sequence it rather than run it beside the app.
    //
    // `--fetch-embedder-model` is what the image's build stage calls, and it needs no
    // database: the download is the side effect.
    match std::env::args().nth(1).as_deref() {
        Some("--migrate-only") => return migrate_only().await,
        Some("--fetch-embedder-model") => return fetch_embedder_model().await,
        Some(other) => anyhow::bail!("unknown argument {other}"),
        None => {}
    }

    // Load and validate configuration before binding anything: a broken
    // plans.toml must be a startup failure, not a 500 on the first request.
    let config = AppConfig::from_env()?;

    telemetry::init(
        if config.env.is_dev() { "dev" } else { "prod" },
        "info,stvdy_server=debug,tower_http=debug",
    );

    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    db::seed_plans(&pool, &config.plans).await?;

    // Boot assertions that would otherwise fail silently at first use, ordered
    // cheapest first inside: the storage backend is reachable, the bucket
    // exists, pdfium binds, and the embedding dimension agrees with the database
    // column (A2). The model load is last because it is the expensive one.
    let providers = Providers::build(&config, &pool).await?;

    // A job left `running` by a killed process would otherwise sit there
    // forever, and the source it belongs to would stay `processing` with no way
    // out. Requeueing at boot is what makes a crash recoverable.
    let requeued = jobs::requeue_orphans(&pool).await?;
    if requeued > 0 {
        tracing::warn!(requeued, "requeued jobs left running by a previous process");
    }

    // A refund that could not be written when a generation failed for good leaves
    // the row terminal with its charge still pending (see `fail_and_refund`).
    // Nothing else resolves that state, so recover it here, before the worker can
    // start charging new generations.
    let refunded = credits::refund_pending(&pool, credits::RefundTarget::Deck).await?
        + credits::refund_pending(&pool, credits::RefundTarget::Quiz).await?
        + credits::refund_pending(&pool, credits::RefundTarget::Report).await?
        + credits::refund_pending(&pool, credits::RefundTarget::Cheatsheet).await?
        // Phase 11's two chargeable generations: the planner's proposal and the syllabus
        // extraction. Both charge before the model is called, so both can be left with a
        // pending refund by the same infrastructure failure.
        + credits::refund_pending(&pool, credits::RefundTarget::PlanProposal).await?
        + credits::refund_pending(&pool, credits::RefundTarget::SyllabusDraft).await?;
    if refunded > 0 {
        tracing::warn!(
            refunded,
            "recovered charges left pending by a failed refund"
        );
    }

    let bind_addr = config.bind_addr;
    let state = AppState {
        pool,
        config: Arc::new(config),
        storage: providers.storage,
        embedder: providers.embedder,
        mailer: providers.mailer,
        clock: providers.clock,
        llm: providers.llm,
        search: providers.search,
        renderer: providers.renderer,
        fetcher: providers.fetcher,
        ics_fetch: providers.ics_fetch,
        payment: providers.payment,
        push: providers.push,
        jobs: jobs::Queue::new(),
    };

    let worker = jobs::worker::spawn(state.clone());
    // The only time-based producer: it enqueues each account's local period-boundary jobs
    // and the worker runs them. Separate tasks because they fail differently — a scheduler
    // pass that fails is retried by the next tick, while a claimed job has its own retries.
    let scheduler = jobs::schedule::spawn(state.clone());
    let app = routes::router(state)?;

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(addr = %bind_addr, "stvdy-server listening");

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal());

    let result = server.await.context("server error");
    // Neither owns state that needs flushing, but stopping them explicitly keeps a
    // shutdown from looking like a hang while one finishes a job.
    worker.abort();
    scheduler.abort();
    result
}

/// The deployment's migration step: connect, migrate, seed, exit.
///
/// It **does not bind a port**, and that is the point rather than an implementation detail.
/// The race it removes is two replicas starting together: both would run `sqlx::migrate!`,
/// and while the advisory lock makes that safe, the replica that loses the race then serves
/// traffic against a schema it has not verified. An orchestrator that runs this to completion
/// before starting any replica removes the race; a flag that also served traffic would not.
///
/// `seed_plans` runs here too, because it reconciles the `plans` table with `config/plans.toml`
/// and the replicas would otherwise race on the same reconciliation.
async fn migrate_only() -> anyhow::Result<()> {
    let config = AppConfig::from_env()?;
    telemetry::init(
        if config.env.is_dev() { "dev" } else { "prod" },
        "info,stvdy_server=debug",
    );

    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    db::seed_plans(&pool, &config.plans).await?;

    tracing::info!("migrations applied; exiting without binding a port");
    Ok(())
}

/// Populates `FASTEMBED_CACHE_DIR` and exits.
///
/// `fastembed` downloads the model on first construction, and the image's build stage calls
/// this so the running container never does: a first ingest on a fresh host would otherwise
/// either stall on a 1.06 GB download or fail when the host has no egress (A342).
///
/// It goes through `FastembedEmbedder::new` — the server's own constructor — rather than
/// calling `fastembed` here, so the cache layout, the model name and the dimension assertion
/// cannot differ between the image and the running process. A hand-copied directory or a
/// second model map would be invalidated by a version bump with no error until the first
/// ingest.
///
/// **It needs no database.** That is what lets the image's build stage run it before Postgres
/// exists: the config is loaded (which is file-only), the embedder is constructed, and the
/// download is the side effect.
async fn fetch_embedder_model() -> anyhow::Result<()> {
    let config = AppConfig::from_env()?;
    let embedder = stvdy_server::integrations::embedder::FastembedEmbedder::new(&config.embedder)?;
    let dim = stvdy_core::traits::embedder::Embedder::dim(&embedder);
    drop(embedder);

    anyhow::ensure!(
        config.embedder.cache_dir.is_dir(),
        "the model did not populate {}; the image would re-download it at first ingest",
        config.embedder.cache_dir.display()
    );
    tracing::info!(
        model = %config.embedder.model,
        dim,
        dir = %config.embedder.cache_dir.display(),
        "embedding model cached"
    );
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
