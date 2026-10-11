//! Database pool, migrations and plan seeding.

use anyhow::Context;
use sqlx::postgres::{PgPool, PgPoolOptions};
use stvdy_core::config::{PlanId, PlansFile};

use crate::config::AppConfig;

pub async fn connect(cfg: &AppConfig) -> anyhow::Result<PgPool> {
    connect_url(&cfg.database_url).await
}

pub async fn connect_url(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
        .context("connecting to Postgres")?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .context("running migrations")?;
    Ok(())
}

/// Mirrors `config/plans.toml` into the `plans` table so billing rows keep a
/// valid FK and the entitlement snapshot stays auditable.
///
/// The TOML remains the authority: entitlement checks read the typed config,
/// never this table. The migration seeds the same three rows so the schema is
/// self-sufficient; this upsert keeps them in step with the file at boot.
pub async fn seed_plans(pool: &PgPool, plans: &PlansFile) -> anyhow::Result<()> {
    for plan_id in PlanId::ALL {
        let config = plans.get(plan_id);
        let entitlements = serde_json::to_value(config).context("serializing plan config")?;

        sqlx::query(
            "insert into plans (id, name, price_idr_monthly, price_idr_yearly, entitlements)
             values ($1, $2, $3, $4, $5)
             on conflict (id) do update
               set name = excluded.name,
                   price_idr_monthly = excluded.price_idr_monthly,
                   price_idr_yearly = excluded.price_idr_yearly,
                   entitlements = excluded.entitlements",
        )
        .bind(plan_id.as_str())
        .bind(&config.name)
        .bind(i32::try_from(config.price_idr_monthly).context("monthly price out of range")?)
        .bind(i32::try_from(config.price_idr_yearly).context("yearly price out of range")?)
        .bind(entitlements)
        .execute(pool)
        .await
        .with_context(|| format!("seeding plan {plan_id}"))?;
    }
    Ok(())
}
