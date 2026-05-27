use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub type DbPool = PgPool;

/// Try to create a Postgres pool. Returns `None` if connection fails — graceful degradation
/// for local-first usage where Postgres is optional.
pub async fn try_create_pool(database_url: &str, max_connections: u32) -> Option<DbPool> {
    match PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(database_url)
        .await
    {
        Ok(pool) => Some(pool),
        Err(e) => {
            tracing::debug!("Postgres pool unavailable: {e}");
            None
        }
    }
}

/// Strict pool creation that fails fast.
pub async fn create_pool(database_url: &str, max_connections: u32) -> anyhow::Result<DbPool> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await?;
    Ok(pool)
}

/// Run embedded migrations.
pub async fn run_migrations(pool: &DbPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
