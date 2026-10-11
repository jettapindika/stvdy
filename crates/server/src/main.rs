use std::sync::Arc;

use anyhow::Context;
use stvdy_server::integrations::Providers;
use stvdy_server::{config::AppConfig, db, jobs, routes, state::AppState, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` for local development so the documented quickstart
    // (`cp .env.example .env && cargo run -p stvdy-server`) actually works.
    // dotenvy never overrides variables that are already set, so a real
    // deployment's environment (and CI) always wins over a stray file.
    let _ = dotenvy::dotenv();

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

    // Boot assertions that would otherwise fail silently at first use: the
    // storage backend is reachable, the bucket exists, and the embedding
    // dimension agrees with the database column (A2).
    let providers = Providers::build(&config, &pool).await?;
    if let Some(storage) = providers.storage_as_s3() {
        storage.ensure_bucket().await?;
    }

    // A job left `running` by a killed process would otherwise sit there
    // forever, and the source it belongs to would stay `processing` with no way
    // out. Requeueing at boot is what makes a crash recoverable.
    let requeued = jobs::requeue_orphans(&pool).await?;
    if requeued > 0 {
        tracing::warn!(requeued, "requeued jobs left running by a previous process");
    }

    let bind_addr = config.bind_addr;
    let state = AppState {
        pool,
        config: Arc::new(config),
        storage: providers.storage,
        embedder: providers.embedder,
        mailer: providers.mailer,
        clock: providers.clock,
        jobs: jobs::Queue::new(),
    };

    let worker = jobs::worker::spawn(state.clone());
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
    // The worker owns no state that needs flushing, but stopping it explicitly
    // keeps a shutdown from looking like a hang while it finishes a job.
    worker.abort();
    result
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
