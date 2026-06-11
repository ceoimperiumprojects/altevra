//! BlockWritesRepository (R5 memory sync hub) — block-level guarded writer
//! manifest (migration 042).
//!
//! Unlike `ManagedWritesRepository` (migration 040) which tracks whole-file
//! writes for skill-sync, this table tracks individual `<!-- ALTEVRA_MANAGED_START
//! ... ALTEVRA_MANAGED_END -->` blocks inside human-owned files (e.g. CLAUDE.md,
//! Obsidian notes). The key is `(file_path, marker_id)`.
//!
//! Drift detection: `block_hash != current block hash` → DRIFT → refuse + review.
//! Content outside the markers is never touched by the writer.

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;

#[derive(Debug, Clone)]
pub struct BlockWriteRow {
    pub id: String,
    /// Absolute path of the target file.
    pub file_path: String,
    /// Optional label embedded in the START marker comment. Empty string = unlabeled.
    pub marker_id: String,
    /// sha256 hex of the block bytes WE wrote (drift baseline).
    pub block_hash: String,
    /// Pre-write backup path. `None` = the write CREATED the block (no prior content).
    pub backup_path: Option<String>,
    /// JSON object with ingest provenance: `{source_file, mtime, ingest_ts}`.
    pub provenance: Option<String>,
    pub ts: String,
}

pub struct BlockWritesRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> BlockWritesRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Record (or refresh) the manifest row for a `(file_path, marker_id)` after a
    /// successful block write. UNIQUE(file_path, marker_id): re-recording updates in
    /// place rather than duplicating.
    pub async fn record_write(
        &self,
        file_path: &str,
        marker_id: &str,
        block_hash: &str,
        backup_path: Option<&str>,
        provenance: Option<&str>,
    ) -> anyhow::Result<()> {
        if file_path.is_empty() || block_hash.is_empty() {
            anyhow::bail!("file_path and block_hash must be non-empty");
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO block_writes \
             (id, file_path, marker_id, block_hash, backup_path, provenance, ts) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(file_path, marker_id) DO UPDATE SET \
               block_hash = excluded.block_hash, \
               backup_path = excluded.backup_path, \
               provenance = excluded.provenance, \
               ts = excluded.ts",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(file_path)
        .bind(marker_id)
        .bind(block_hash)
        .bind(backup_path)
        .bind(provenance)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Look up the drift baseline for a `(file_path, marker_id)` pair.
    /// `None` = the block writer never wrote this pair (no baseline).
    pub async fn get(
        &self,
        file_path: &str,
        marker_id: &str,
    ) -> anyhow::Result<Option<BlockWriteRow>> {
        let row = sqlx::query(
            "SELECT id, file_path, marker_id, block_hash, backup_path, provenance, ts \
             FROM block_writes WHERE file_path = ? AND marker_id = ?",
        )
        .bind(file_path)
        .bind(marker_id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(to_row))
    }

    /// All rows for a given file path, ordered by ts DESC.
    pub async fn list_for_file(&self, file_path: &str) -> anyhow::Result<Vec<BlockWriteRow>> {
        let rows = sqlx::query(
            "SELECT id, file_path, marker_id, block_hash, backup_path, provenance, ts \
             FROM block_writes WHERE file_path = ? ORDER BY ts DESC",
        )
        .bind(file_path)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(to_row).collect())
    }

    /// All manifest rows, newest first.
    pub async fn list(&self, limit: i64) -> anyhow::Result<Vec<BlockWriteRow>> {
        let rows = sqlx::query(
            "SELECT id, file_path, marker_id, block_hash, backup_path, provenance, ts \
             FROM block_writes ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(to_row).collect())
    }
}

fn to_row(r: sqlx::sqlite::SqliteRow) -> BlockWriteRow {
    BlockWriteRow {
        id: r.get("id"),
        file_path: r.get("file_path"),
        marker_id: r.get("marker_id"),
        block_hash: r.get("block_hash"),
        backup_path: r.get("backup_path"),
        provenance: r.get("provenance"),
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
        let repo = BlockWritesRepository::new(&p);

        // No baseline yet.
        assert!(repo.get("/x/CLAUDE.md", "").await.unwrap().is_none());

        // First write: no backup.
        repo.record_write("/x/CLAUDE.md", "", &"a".repeat(64), None, None)
            .await
            .unwrap();
        let row = repo.get("/x/CLAUDE.md", "").await.unwrap().unwrap();
        assert_eq!(row.block_hash, "a".repeat(64));
        assert!(row.backup_path.is_none());

        // Refresh: same key, updates in place.
        repo.record_write(
            "/x/CLAUDE.md",
            "",
            &"b".repeat(64),
            Some("/bak/CLAUDE.md"),
            Some(r#"{"source_file":"~/.claude/CLAUDE.md"}"#),
        )
        .await
        .unwrap();
        let rows = repo.list_for_file("/x/CLAUDE.md").await.unwrap();
        assert_eq!(rows.len(), 1, "UNIQUE(file_path, marker_id) must merge");
        assert_eq!(rows[0].block_hash, "b".repeat(64));
        assert_eq!(rows[0].backup_path.as_deref(), Some("/bak/CLAUDE.md"));

        // Different marker_id on the same file = separate row.
        repo.record_write("/x/CLAUDE.md", "context", &"c".repeat(64), None, None)
            .await
            .unwrap();
        let all = repo.list_for_file("/x/CLAUDE.md").await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn empty_keys_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = pool(&dir).await;
        let repo = BlockWritesRepository::new(&p);
        assert!(repo.record_write("", "", "hash", None, None).await.is_err());
        assert!(repo.record_write("/x", "", "", None, None).await.is_err());
    }
}
