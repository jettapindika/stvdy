use std::sync::Arc;

use anyhow::Context;
use stvdy_server::{config::AppConfig, db, routes, state::AppState, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = if std::env::var("APP_ENV").as_deref() == Ok("prod") {
        "prod"
    } else {
        "dev"
    };
    telemetry::init(env, "info,stvdy_server=debug,tower_http=debug");

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let bind_addr: std::net::SocketAddr =
        bind_addr.parse().context("BIND_ADDR must be host:port")?;
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/stvdy".to_string());

    let pool = db::connect(&database_url).await?;

    let config = AppConfig {
        env: if env == "prod" {
            stvdy_server::config::Env::Prod
        } else {
            stvdy_server::config::Env::Dev
        },
        bind_addr,
        database_url,
    };
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
