//! Database pool, migrations and plan seeding. Filled in by Task 8.

use sqlx::PgPool;

/// Placeholder until Task 8 implements pool creation with tuned limits.
pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await?;
    Ok(pool)
}
