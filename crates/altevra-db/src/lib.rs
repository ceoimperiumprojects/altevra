//! Altevra local-first database layer (SQLite, v0.2).
//!
//! Public API surface kept stable for downstream crates:
//!   * `DbPool` — `sqlx::SqlitePool` alias.
//!   * `create_pool`, `try_create_pool`, `run_migrations` — pool/init helpers.
//!   * All seven repositories (`EventsRepository`, ..., `TasksRepository`).
//!
//! Internal helpers (`uuid_from_text`, `ts_from_text`) are exposed so that
//! sibling crates can share the same UUID/timestamp text encoding when
//! constructing/parsing rows manually.

pub mod pool;
pub mod repositories;
pub mod util;

pub use pool::{create_pool, run_migrations, try_create_pool, DbPool};
pub use repositories::*;
pub use util::{opt_uuid_from_text, ts_from_text, uuid_from_text};
