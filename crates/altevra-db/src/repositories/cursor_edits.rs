//! Cursor CLI ai-tracking import surface (P0.9 E4).
//!
//! Persists structured rows lifted from `~/.cursor/ai-tracking/ai-code-tracking.db`
//! (Cursor CLI's local SQLite). The importer in `altevra-adapters::cursor_cli`
//! opens the upstream db READ-ONLY, runs each indexable text field through
//! `altevra-secrets::guard_text`, and either:
//!
//!   * REJECTS the row when a credential-class secret is sighted (PEM / API
//!     key / db-url) — `insert()` is never called for that row;
//!   * REDACTS the row and writes it here with `redaction_status` carrying the
//!     scanned verdict (`clean` / `redacted`).
//!
//! The write path mirrors `LearningsRepository::insert` — it indexes into
//! `object_index` + `object_fts` ONLY when the verdict is scanned (R11
//! fail-closed). The exposure gate downstream still arbitrates exposure; the
//! lifecycle archiver (E1) can archive these rows like any other object.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::repositories::objects::{ObjectIndexRepository, ObjectIndexRow};
use crate::util::ts_to_text;

/// One Cursor edit / tracked file row, post-guard, ready to land in
/// `cursor_edits` and the cross-type index.
#[derive(Debug, Clone)]
pub struct CursorEditRow {
    pub id: String,
    pub content_hash: String,
    pub source: Option<String>,
    pub file_path: Option<String>,
    pub file_extension: Option<String>,
    pub conversation_id: Option<String>,
    pub request_id: Option<String>,
    pub model: Option<String>,
    /// Already passed through `guard_text` upstream — the caller is responsible
    /// for not handing un-guarded text in (mirrors `LearningRow`).
    pub snippet: String,
    pub length: i64,
    /// Cursor's `timestamp` column (ms since epoch). Optional — older rows
    /// sometimes lack it; we never synthesise.
    pub cursor_ts: Option<i64>,
    /// Cursor's `createdAt` column (ms since epoch) — always present in real
    /// rows, so we keep it non-null.
    pub cursor_created: i64,
    pub title: String,
    pub status: String,
    pub domain: String,
    pub scope: Option<String>,
    pub sensitivity: String,
    /// JSON object — origin, imported_from, source_db, optional cursor_ts.
    pub provenance: String,
    pub redaction_status: String,
    /// JSON array.
    pub categories: String,
    /// JSON array.
    pub tags: String,
}

impl CursorEditRow {
    /// Only scanned verdicts (`clean` / `redacted`) are safe to index — mirrors
    /// the contract enforced on decisions/learnings (T1.13, R11 fail-closed).
    fn is_indexable(&self) -> bool {
        matches!(self.redaction_status.as_str(), "clean" | "redacted")
    }
}

pub struct CursorEditsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> CursorEditsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Idempotent insert (INSERT OR REPLACE so a re-import of the same Cursor
    /// db doesn't UNIQUE-violate). Indexes the row into `object_index` +
    /// `object_fts` via the single maintenance point — only when the redaction
    /// verdict is scanned. Returns true when the row entered the index.
    pub async fn insert(&self, row: &CursorEditRow) -> anyhow::Result<bool> {
        let now: DateTime<Utc> = Utc::now();
        let now_text = ts_to_text(&now);
        sqlx::query(
            "INSERT OR REPLACE INTO cursor_edits \
             (id, content_hash, source, file_path, file_extension, conversation_id, \
              request_id, model, snippet, length, cursor_ts, cursor_created, \
              title, status, domain, scope, sensitivity, provenance, redaction_status, \
              categories, tags, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.content_hash)
        .bind(row.source.as_deref())
        .bind(row.file_path.as_deref())
        .bind(row.file_extension.as_deref())
        .bind(row.conversation_id.as_deref())
        .bind(row.request_id.as_deref())
        .bind(row.model.as_deref())
        .bind(&row.snippet)
        .bind(row.length)
        .bind(row.cursor_ts)
        .bind(row.cursor_created)
        .bind(&row.title)
        .bind(&row.status)
        .bind(&row.domain)
        .bind(row.scope.as_deref())
        .bind(&row.sensitivity)
        .bind(&row.provenance)
        .bind(&row.redaction_status)
        .bind(&row.categories)
        .bind(&row.tags)
        .bind(&now_text)
        .bind(&now_text)
        .execute(self.pool)
        .await?;

        if !row.is_indexable() {
            // R11 fail-closed — un-scanned verdicts are persisted but never
            // become recall / packet candidates.
            return Ok(false);
        }

        ObjectIndexRepository::new(self.pool)
            .index_object(
                &ObjectIndexRow {
                    object_type: "cursor_edit".into(),
                    id: row.id.clone(),
                    status: row.status.clone(),
                    sensitivity: row.sensitivity.clone(),
                    domain: row.domain.clone(),
                    scope: row.scope.clone(),
                    title: Some(row.title.clone()),
                    categories: row.categories.clone(),
                    tags: row.tags.clone(),
                    redaction_status: row.redaction_status.clone(),
                    updated_at: now,
                },
                // Index over (title + snippet) so a recall over a file path or
                // a hash fragment can find the row through FTS too.
                &format!(
                    "{}\n{}\n{}",
                    row.title,
                    row.file_path.as_deref().unwrap_or(""),
                    row.snippet
                ),
            )
            .await?;
        Ok(true)
    }

    pub async fn count(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM cursor_edits")
            .fetch_one(self.pool)
            .await?;
        Ok(row.get::<i64, _>("n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_pool, run_migrations};
    use tempfile::TempDir;

    async fn pool() -> (TempDir, SqlitePool) {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        let p = create_pool(db.to_str().unwrap()).await.unwrap();
        run_migrations(&p).await.unwrap();
        (tmp, p)
    }

    fn sample_row(id: &str, hash: &str, body: &str) -> CursorEditRow {
        CursorEditRow {
            id: id.to_string(),
            content_hash: hash.to_string(),
            source: Some("cli".into()),
            file_path: Some("/tmp/example.rs".into()),
            file_extension: Some("rs".into()),
            conversation_id: Some("conv-1".into()),
            request_id: None,
            model: Some("claude-opus-4-7".into()),
            snippet: body.to_string(),
            length: body.len() as i64,
            cursor_ts: Some(1_778_247_477_915),
            cursor_created: 1_778_247_477_916,
            title: format!("cursor edit {hash}"),
            status: "active".into(),
            domain: "business".into(),
            scope: None,
            sensitivity: "internal".into(),
            provenance: r#"{"origin":"cursor_cli","source_db":"~/.cursor/ai-tracking/ai-code-tracking.db"}"#.into(),
            redaction_status: "clean".into(),
            categories: r#"["business","kind:cursor_edit"]"#.into(),
            tags: r#"["business","kind:cursor_edit"]"#.into(),
        }
    }

    #[tokio::test]
    async fn insert_indexes_clean_row() {
        let (_tmp, p) = pool().await;
        let repo = CursorEditsRepository::new(&p);
        let indexed = repo.insert(&sample_row("e1", "h1", "println!(\"hi\")")).await.unwrap();
        assert!(indexed);
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn insert_is_idempotent() {
        let (_tmp, p) = pool().await;
        let repo = CursorEditsRepository::new(&p);
        let row = sample_row("e1", "h1", "x");
        repo.insert(&row).await.unwrap();
        repo.insert(&row).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn unscanned_row_is_persisted_but_not_indexed() {
        let (_tmp, p) = pool().await;
        let repo = CursorEditsRepository::new(&p);
        let mut row = sample_row("e2", "h2", "x");
        row.redaction_status = "unscanned".into();
        let indexed = repo.insert(&row).await.unwrap();
        assert!(!indexed, "fail-closed: unscanned never indexes");
        assert_eq!(repo.count().await.unwrap(), 1);
    }
}
