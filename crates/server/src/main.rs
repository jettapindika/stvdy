use std::sync::Arc;

use anyhow::Context;
use stvdy_server::{config::AppConfig, db, routes, state::AppState, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let bind_addr = config.bind_addr;
    let state = AppState {
        pool,
        config: Arc::new(config),
    };
    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(addr = %bind_addr, "stvdy-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
