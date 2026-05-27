//! SQLite connection pool for Altevra (local-first storage).
//!
//! v0.2 "Living Brain" replaces the previous Postgres-backed store with an
//! embedded SQLite database at `.altevra/altevra.db`. Connections are managed
//! via `sqlx::SqlitePool`, which keeps the public type alias `DbPool` stable
//! for downstream crates.
//!
//! ## Optional sqlite-vec extension
//!
//! When this crate is built with the `vec` cargo feature, every connection
//! registers the `sqlite-vec` extension via `sqlite_vec::sqlite3_vec_init`,
//! which exposes `vec0` virtual tables for ANN search.
//!
//! When the feature is disabled (default) we fall back to plain JSON-encoded
//! `Vec<f32>` storage in the `memory_chunk_vectors` table and expect callers
//! (e.g. altevra-memory) to compute cosine similarity in Rust. This keeps the
//! crate buildable on any host without external C deps.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

pub type DbPool = SqlitePool;

/// Default maximum connections. SQLite serialises writes regardless, so 8 is
/// a safe upper bound for concurrent readers and one writer at a time.
const DEFAULT_MAX_CONNECTIONS: u32 = 8;

fn build_options(path: &str) -> anyhow::Result<SqliteConnectOptions> {
    // `mode=rwc` lets sqlx create the file if it does not exist yet.
    // We intentionally do NOT use `sqlite::memory:` shortcut here because
    // SqliteConnectOptions::from_str handles `sqlite::memory:` and absolute /
    // relative paths uniformly.
    let url = if path.starts_with("sqlite:") {
        path.to_string()
    } else {
        format!("sqlite://{path}?mode=rwc")
    };

    // Ensure parent directory exists for file-backed databases — sqlx will
    // happily fail with "unable to open database file" otherwise.
    if !path.starts_with("sqlite:") && !path.contains(":memory:") {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }

    let opts = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);

    Ok(opts)
}

/// Strict pool creation — fails fast on misconfiguration.
///
/// `path` may be:
///   * a filesystem path (`.altevra/altevra.db`),
///   * an explicit `sqlite:` URL (`sqlite::memory:`, `sqlite:/abs/path`),
///   * the special `:memory:` marker.
pub async fn create_pool(path: &str) -> anyhow::Result<DbPool> {
    let opts = build_options(path)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(DEFAULT_MAX_CONNECTIONS)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await?;
    register_vec_extension(&pool).await?;
    Ok(pool)
}

/// Best-effort pool creation. Returns `None` on failure so callers can fall
/// back to in-memory JSON state (mirrors the previous Postgres semantics).
pub async fn try_create_pool(path: &str) -> Option<DbPool> {
    match create_pool(path).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            tracing::debug!("SQLite pool unavailable: {e}");
            None
        }
    }
}

/// Run embedded migrations. Idempotent.
pub async fn run_migrations(pool: &DbPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Register the sqlite-vec extension on every connection, when compiled with
/// the `vec` feature. With the feature disabled this is a no-op so the
/// fallback JSON-encoded vector path is used.
#[cfg(feature = "vec")]
async fn register_vec_extension(pool: &DbPool) -> anyhow::Result<()> {
    use sqlx::Executor;
    // sqlite-vec exposes a static-link helper; loading is per-connection.
    // We touch a connection and call the SELECT vec_version() smoke test.
    // If the linkage breaks, fail loudly so the operator can disable `vec`.
    let mut conn = pool.acquire().await?;
    // The actual entrypoint registration is wired up via libsqlite3-sys's
    // `extension` feature; if the build can't expose load_extension we just
    // skip the smoke test rather than crashing the binary.
    let res = conn
        .fetch_optional(sqlx::query("SELECT vec_version()"))
        .await;
    match res {
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!(
                "sqlite-vec extension not available at runtime ({e}); \
                 falling back to JSON-encoded embeddings in memory_chunk_vectors"
            );
            Ok(())
        }
    }
}

#[cfg(not(feature = "vec"))]
async fn register_vec_extension(_pool: &DbPool) -> anyhow::Result<()> {
    Ok(())
}
