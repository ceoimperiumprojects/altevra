//! ManagedWritesRepository (PLAN-ALIVE §P3 install/sync) — the sync writer's
//! drift-detection manifest (migration 040).
//!
//! "Never overwrite human edits" is undetectable without a stored baseline:
//! this table records, per target file the skill-sync writer ever wrote, the
//! sha256 of the content WE wrote (`block_hash`) and where the pre-write
//! backup of the previous content landed (`backup_path`). The guarded applier
//! consults [`get_by_path`] before any write: current-file hash differing from
//! the manifest hash means a human (or another tool) edited the file since our
//! last write ⇒ DRIFT ⇒ refuse + route to review. `altevra skill-sync restore`
//! copies `backup_path` back over `target_path`.
//!
//! [`get_by_path`]: ManagedWritesRepository::get_by_path

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;

#[derive(Debug, Clone)]
pub struct ManagedWriteRow {
    pub id: String,
    /// Absolute path of the file the sync writer wrote.
    pub target_path: String,
    /// sha256 hex of the content we wrote (the drift baseline).
    pub block_hash: String,
    /// Where the PREVIOUS content was backed up before this write
    /// (`~/.altevra/backups/sync/<ts>/…`). `None` when the write created the file.
    pub backup_path: Option<String>,
    pub ts: String,
}

pub struct ManagedWritesRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ManagedWritesRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Record (or refresh) the manifest row for a target path after a
    /// successful write. UNIQUE(target_path): re-recording updates the hash,
    /// backup pointer and timestamp instead of duplicating.
    pub async fn record_write(
        &self,
        target_path: &str,
        block_hash: &str,
        backup_path: Option<&str>,
    ) -> anyhow::Result<()> {
        if target_path.is_empty() || block_hash.is_empty() {
            anyhow::bail!("target_path and block_hash must be non-empty");
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO managed_writes (id, target_path, block_hash, backup_path, ts) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(target_path) DO UPDATE SET \
               block_hash = excluded.block_hash, backup_path = excluded.backup_path, \
               ts = excluded.ts",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(target_path)
        .bind(block_hash)
        .bind(backup_path)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The drift baseline for a path — `None` means we never wrote it (no
    /// baseline ⇒ the writer falls back to the managed-marker check only).
    pub async fn get_by_path(&self, target_path: &str) -> anyhow::Result<Option<ManagedWriteRow>> {
        let row = sqlx::query(
            "SELECT id, target_path, block_hash, backup_path, ts \
             FROM managed_writes WHERE target_path = ?",
        )
        .bind(target_path)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(to_row))
    }

    /// All manifest rows, newest first (for `altevra skill-sync manifest`).
    pub async fn list(&self, limit: i64) -> anyhow::Result<Vec<ManagedWriteRow>> {
        let rows = sqlx::query(
            "SELECT id, target_path, block_hash, backup_path, ts \
             FROM managed_writes ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(to_row).collect())
    }
}

fn to_row(r: sqlx::sqlite::SqliteRow) -> ManagedWriteRow {
    ManagedWriteRow {
        id: r.get("id"),
        target_path: r.get("target_path"),
        block_hash: r.get("block_hash"),
        backup_path: r.get("backup_path"),
        ts: r.get("ts"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    async fn pool(dir: &tempfile::TempDir) -> SqlitePool {
        let db = dir.path().join("test.db");
        let p = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn record_get_roundtrip_and_upsert() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = pool(&dir).await;
        let repo = ManagedWritesRepository::new(&p);

        assert!(repo.get_by_path("/x/SKILL.md").await.unwrap().is_none());

        repo.record_write("/x/SKILL.md", &"a".repeat(64), None)
            .await
            .unwrap();
        let row = repo.get_by_path("/x/SKILL.md").await.unwrap().unwrap();
        assert_eq!(row.block_hash, "a".repeat(64));
        assert!(row.backup_path.is_none());

        // Re-record (refresh write): same path updates, never duplicates.
        repo.record_write("/x/SKILL.md", &"b".repeat(64), Some("/bak/SKILL.md"))
            .await
            .unwrap();
        let rows = repo.list(10).await.unwrap();
        assert_eq!(rows.len(), 1, "UNIQUE(target_path) must merge");
        assert_eq!(rows[0].block_hash, "b".repeat(64));
        assert_eq!(rows[0].backup_path.as_deref(), Some("/bak/SKILL.md"));
    }

    #[tokio::test]
    async fn empty_keys_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = pool(&dir).await;
        let repo = ManagedWritesRepository::new(&p);
        assert!(repo.record_write("", "h", None).await.is_err());
        assert!(repo.record_write("/x", "", None).await.is_err());
    }
}
