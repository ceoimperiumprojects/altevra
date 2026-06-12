//! R2 embedding backfill — enqueue ALL existing DB content for embedding via
//! the `db://` synthetic-document contract (see [`crate::db_uri`]).
//!
//! Sources scanned (one watermark row per `(source_type, model)`):
//!   * `turns`          → `db://turn/<id>`
//!   * `learnings`      → `db://learning/<id>`
//!   * `personal_notes` → `db://note/<id>`
//!   * `wiki_pages`     → `db://wiki/<slug>` (body read from the page's file)
//!   * `research_items` → `db://research/<id>`
//!
//! Each object is reduced to its *embedded text* via
//! [`resolve_db_object_text`] (the SAME resolver the embedder worker uses when
//! it drains a `db://` row, so checksums always line up), then:
//!
//!   * `memory_documents.source_path = uri` with a matching checksum
//!     → **unchanged**, nothing to do;
//!   * a `pending_indexing` row for the uri already `pending`
//!     → **already_queued**, nothing to do (re-runs leave the queue as-is);
//!   * otherwise → **enqueued**: upsert into `pending_indexing` with
//!     `status='pending'` (URI uniqueness — `UNIQUE(path)` — guarantees no
//!     duplicate rows; checksum drift on an indexed object lands here, which
//!     is exactly the re-embed invalidation path).
//!
//! Resumability: `embed_backfill_watermark.last_id` (migration 041) is an
//! in-pass cursor — batches advance it, an interrupted run resumes from it,
//! and a COMPLETED pass resets it to `''` so the next run re-verifies the
//! whole corpus cheaply (checksum skip). `total_enqueued` accumulates across
//! passes. Dry-run touches neither table and reports per-source counts only.

use std::collections::BTreeMap;

use anyhow::Context;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::db_uri::{db_uri, embed_checksum, DbObjectType};

/// All sources the backfill scans, in a stable order.
pub const BACKFILL_SOURCES: [DbObjectType; 5] = [
    DbObjectType::Turn,
    DbObjectType::Learning,
    DbObjectType::Note,
    DbObjectType::Wiki,
    DbObjectType::Research,
];

/// The embedded-text view of a DB object (the synthetic document body).
#[derive(Debug, Clone)]
pub struct DbObjectText {
    pub title: Option<String>,
    pub text: String,
}

/// Per-source counters for one backfill run.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SourceReport {
    /// Rows scanned in this run.
    pub scanned: u64,
    /// Rows (newly) enqueued into `pending_indexing` — includes re-embeds
    /// triggered by checksum change on an already-indexed object.
    pub enqueued: u64,
    /// Rows whose `memory_documents` checksum already matches the current
    /// embedded text — nothing to do.
    pub unchanged: u64,
    /// Rows that already sit `pending` in the queue — re-runs don't touch them.
    pub already_queued: u64,
    /// Rows skipped (empty text, unreadable wiki file, dangling reference).
    pub skipped: u64,
}

/// Full report of one `altevra embed backfill` run.
#[derive(Debug, Clone, Serialize)]
pub struct BackfillReport {
    pub model: String,
    pub dry_run: bool,
    /// Keyed by source type ("turn", "learning", ...), stable order.
    pub sources: BTreeMap<String, SourceReport>,
}

impl BackfillReport {
    pub fn total_enqueued(&self) -> u64 {
        self.sources.values().map(|s| s.enqueued).sum()
    }
    pub fn total_scanned(&self) -> u64 {
        self.sources.values().map(|s| s.scanned).sum()
    }
}

/// Resolve the embedded text for a `db://<otype>/<id>` object.
///
/// Returns `Ok(None)` when the object does not exist, has nothing to embed
/// (empty text), or — for wiki pages — its backing file is unreadable. This
/// resolver is shared by the backfill (checksum computation) and the embedder
/// worker (`db://` rows in `pending_indexing`), so the sha256 the backfill
/// records is byte-identical to what the worker embeds.
pub async fn resolve_db_object_text(
    pool: &SqlitePool,
    otype: &str,
    id: &str,
) -> anyhow::Result<Option<DbObjectText>> {
    let resolved: Option<DbObjectText> = match otype {
        "turn" => {
            let row: Option<(String,)> = sqlx::query_as("SELECT content FROM turns WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
            row.map(|(content,)| DbObjectText {
                title: None,
                text: content,
            })
        }
        "learning" => {
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT title, body FROM learnings WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            row.map(|(title, body)| DbObjectText {
                text: format!("{title}\n\n{body}"),
                title: Some(title),
            })
        }
        "note" => {
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT kind, body FROM personal_notes WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            row.map(|(kind, body)| DbObjectText {
                title: Some(format!("personal note: {kind}")),
                text: body,
            })
        }
        "wiki" => {
            // db://wiki/<slug> — the indexed row points at the markdown file
            // on disk; the page BODY lives there, not in SQLite.
            let row: Option<(Option<String>, String, String)> = sqlx::query_as(
                "SELECT title, topic, path FROM wiki_pages WHERE slug = ? \
                 ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            match row {
                Some((title, topic, path)) => match std::fs::read_to_string(&path) {
                    Ok(body) => Some(DbObjectText {
                        title: Some(title.unwrap_or(topic)),
                        text: body,
                    }),
                    Err(_) => None, // unreadable page file → skip, re-checked next run
                },
                None => None,
            }
        }
        "research" => {
            let row: Option<(String, String, String)> =
                sqlx::query_as("SELECT title, summary, link FROM research_items WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            row.map(|(title, summary, link)| DbObjectText {
                text: format!("{title}\n\n{summary}\n\n{link}"),
                title: Some(title),
            })
        }
        other => anyhow::bail!("unknown db:// object type: {other}"),
    };
    // Nothing to embed → treat as absent so callers skip instead of indexing
    // an empty document.
    Ok(resolved.filter(|o| !o.text.trim().is_empty()))
}

/// Run the backfill over every source. See module docs for semantics.
pub async fn run_backfill(
    pool: &SqlitePool,
    model: &str,
    batch_size: usize,
    dry_run: bool,
) -> anyhow::Result<BackfillReport> {
    let batch_size = batch_size.max(1);
    let mut sources = BTreeMap::new();
    for otype in BACKFILL_SOURCES {
        let report = backfill_source(pool, otype, model, batch_size, dry_run)
            .await
            .with_context(|| format!("backfill source '{}' failed", otype.as_str()))?;
        sources.insert(otype.as_str().to_string(), report);
    }
    Ok(BackfillReport {
        model: model.to_string(),
        dry_run,
        sources,
    })
}

async fn backfill_source(
    pool: &SqlitePool,
    otype: DbObjectType,
    model: &str,
    batch_size: usize,
    dry_run: bool,
) -> anyhow::Result<SourceReport> {
    let mut report = SourceReport::default();

    // Resume cursor: a real run continues an interrupted pass; dry-run always
    // scans from the start so its counts describe the WHOLE corpus.
    let mut cursor: String = if dry_run {
        String::new()
    } else {
        read_watermark(pool, otype.as_str(), model)
            .await?
            .unwrap_or_default()
    };

    loop {
        let batch = list_source_batch(pool, otype, &cursor, batch_size).await?;
        let batch_len = batch.len();
        let mut batch_enqueued = 0u64;

        for (row_id, uri_key) in &batch {
            report.scanned += 1;
            // Always advance the cursor — a skipped/unchanged row must never
            // pin the batch loop in place.
            cursor = row_id.clone();
            let uri = db_uri(otype, uri_key);

            let Some(obj) = resolve_db_object_text(pool, otype.as_str(), uri_key).await? else {
                report.skipped += 1;
                continue;
            };
            let checksum = embed_checksum(&obj.text);

            // Already indexed at this exact content? (re-embed invalidation:
            // a mismatch here falls through and re-enqueues the uri.)
            let indexed: Option<(String,)> =
                sqlx::query_as("SELECT checksum FROM memory_documents WHERE source_path = ?")
                    .bind(&uri)
                    .fetch_optional(pool)
                    .await?;
            if matches!(indexed, Some((ref c,)) if *c == checksum) {
                report.unchanged += 1;
                continue;
            }

            // Already waiting in the queue? Leave it — idempotent re-runs.
            let queued_status: Option<(String,)> =
                sqlx::query_as("SELECT status FROM pending_indexing WHERE path = ?")
                    .bind(&uri)
                    .fetch_optional(pool)
                    .await?;
            if matches!(queued_status, Some((ref s,)) if s == "pending") {
                report.already_queued += 1;
                continue;
            }

            report.enqueued += 1;
            batch_enqueued += 1;
            if !dry_run {
                sqlx::query(
                    r#"INSERT INTO pending_indexing (id, path, status) VALUES (?, ?, 'pending')
                       ON CONFLICT(path) DO UPDATE SET
                         status = 'pending',
                         error = NULL"#,
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&uri)
                .execute(pool)
                .await?;
            }
        }

        if !dry_run && batch_len > 0 {
            advance_watermark(pool, otype.as_str(), model, &cursor, batch_enqueued).await?;
        }
        if batch_len < batch_size {
            break;
        }
    }

    // Pass completed: reset the cursor so the NEXT run re-verifies the whole
    // corpus (checksum-change → re-embed) instead of resuming past it.
    if !dry_run {
        advance_watermark(pool, otype.as_str(), model, "", 0).await?;
    }
    Ok(report)
}

/// One batch of `(cursor_id, uri_key)` pairs for a source, ordered by primary
/// key. `uri_key` is the id for everything except wiki pages, which key their
/// synthetic uri by slug (`db://wiki/<slug>`).
async fn list_source_batch(
    pool: &SqlitePool,
    otype: DbObjectType,
    after_id: &str,
    limit: usize,
) -> anyhow::Result<Vec<(String, String)>> {
    let sql = match otype {
        DbObjectType::Turn => "SELECT id, id AS k FROM turns WHERE id > ? ORDER BY id LIMIT ?",
        DbObjectType::Learning => {
            "SELECT id, id AS k FROM learnings WHERE id > ? ORDER BY id LIMIT ?"
        }
        DbObjectType::Note => {
            "SELECT id, id AS k FROM personal_notes WHERE id > ? ORDER BY id LIMIT ?"
        }
        DbObjectType::Wiki => {
            "SELECT id, slug AS k FROM wiki_pages WHERE id > ? ORDER BY id LIMIT ?"
        }
        DbObjectType::Research => {
            "SELECT id, id AS k FROM research_items WHERE id > ? ORDER BY id LIMIT ?"
        }
    };
    let rows: Vec<(String, String)> = sqlx::query_as(sql)
        .bind(after_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

async fn read_watermark(
    pool: &SqlitePool,
    source_type: &str,
    model: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT last_id FROM embed_backfill_watermark WHERE source_type = ? AND model = ?",
    )
    .bind(source_type)
    .bind(model)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

async fn advance_watermark(
    pool: &SqlitePool,
    source_type: &str,
    model: &str,
    last_id: &str,
    enqueued_delta: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO embed_backfill_watermark (id, source_type, model, last_id, total_enqueued)
           VALUES (?, ?, ?, ?, ?)
           ON CONFLICT(source_type, model) DO UPDATE SET
             last_id = excluded.last_id,
             total_enqueued = total_enqueued + ?,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(source_type)
    .bind(model)
    .bind(last_id)
    .bind(enqueued_delta as i64)
    .bind(enqueued_delta as i64)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::TempDir;
    use uuid::Uuid;

    /// Per-test TempDir DB carrying the minimal shapes of every table the
    /// backfill touches (sources + queue + docs + 041 watermark).
    async fn setup() -> (TempDir, SqlitePool) {
        let tmp = TempDir::new().unwrap();
        let url = format!("sqlite://{}?mode=rwc", tmp.path().join("test.db").display());
        let pool = SqlitePoolOptions::new().connect(&url).await.unwrap();
        for ddl in [
            "CREATE TABLE turns (id TEXT PRIMARY KEY, content TEXT NOT NULL)",
            "CREATE TABLE learnings (id TEXT PRIMARY KEY, title TEXT NOT NULL, body TEXT NOT NULL)",
            "CREATE TABLE personal_notes (id TEXT PRIMARY KEY, kind TEXT NOT NULL, body TEXT NOT NULL)",
            "CREATE TABLE wiki_pages (id TEXT PRIMARY KEY, topic TEXT NOT NULL, slug TEXT NOT NULL, \
             path TEXT NOT NULL, title TEXT, updated_at TEXT NOT NULL DEFAULT '')",
            "CREATE TABLE research_items (id TEXT PRIMARY KEY, title TEXT NOT NULL, \
             summary TEXT NOT NULL DEFAULT '', link TEXT NOT NULL DEFAULT '')",
            "CREATE TABLE pending_indexing (id TEXT PRIMARY KEY, path TEXT NOT NULL, \
             queued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), \
             status TEXT NOT NULL DEFAULT 'pending', last_attempt_at TEXT, error TEXT, \
             fail_count INTEGER NOT NULL DEFAULT 0, UNIQUE(path))",
            "CREATE TABLE memory_documents (id TEXT PRIMARY KEY, source_path TEXT NOT NULL, \
             title TEXT, body TEXT NOT NULL, checksum TEXT NOT NULL, UNIQUE(source_path))",
            "CREATE TABLE embed_backfill_watermark (id TEXT PRIMARY KEY, source_type TEXT NOT NULL, \
             model TEXT NOT NULL, last_id TEXT NOT NULL DEFAULT '', \
             total_enqueued INTEGER NOT NULL DEFAULT 0, \
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), \
             UNIQUE(source_type, model))",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        (tmp, pool)
    }

    async fn count(pool: &SqlitePool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    async fn insert_turn(pool: &SqlitePool, content: &str) -> String {
        let id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO turns (id, content) VALUES (?, ?)")
            .bind(&id)
            .bind(content)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn backfill_enqueues_every_source_with_db_uris() {
        let (tmp, pool) = setup().await;
        let turn_id = insert_turn(&pool, "the user asked about ReVesta GTM").await;
        sqlx::query("INSERT INTO learnings (id, title, body) VALUES ('L1', 'GTM', 'sell direct')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO personal_notes (id, kind, body) VALUES ('N1', 'idea', 'gym at 6')")
            .execute(&pool)
            .await
            .unwrap();
        let wiki_file = tmp.path().join("revesta.md");
        std::fs::write(&wiki_file, "# ReVesta\n\nliving page body.\n").unwrap();
        sqlx::query(
            "INSERT INTO wiki_pages (id, topic, slug, path, title) VALUES ('W1', 'ReVesta', 'revesta', ?, 'ReVesta')",
        )
        .bind(wiki_file.to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO research_items (id, title, summary, link) VALUES ('R1', 'Paper', 'about embeddings', 'https://x.test')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let report = run_backfill(&pool, "bge-m3", 100, false).await.unwrap();
        assert_eq!(report.total_enqueued(), 5);
        assert_eq!(report.sources["turn"].enqueued, 1);
        assert_eq!(report.sources["learning"].enqueued, 1);
        assert_eq!(report.sources["note"].enqueued, 1);
        assert_eq!(report.sources["wiki"].enqueued, 1);
        assert_eq!(report.sources["research"].enqueued, 1);

        // Every queue row carries a db:// synthetic uri, never a fake fs path.
        let paths: Vec<(String,)> = sqlx::query_as("SELECT path FROM pending_indexing ORDER BY path")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(paths.len(), 5);
        for (p,) in &paths {
            assert!(p.starts_with("db://"), "non-synthetic path enqueued: {p}");
        }
        assert!(paths.iter().any(|(p,)| p == &format!("db://turn/{turn_id}")));
        assert!(paths.iter().any(|(p,)| p == "db://wiki/revesta"), "wiki keyed by slug");
    }

    #[tokio::test]
    async fn rerun_is_idempotent_and_watermark_advances() {
        let (_tmp, pool) = setup().await;
        insert_turn(&pool, "first turn").await;
        insert_turn(&pool, "second turn").await;

        let first = run_backfill(&pool, "bge-m3", 1, false).await.unwrap();
        assert_eq!(first.sources["turn"].enqueued, 2);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM pending_indexing").await, 2);

        // Watermark advanced: row exists, total_enqueued counted, cursor reset
        // after the completed pass (it is a resume cursor, not a skip-fence).
        let (last_id, total): (String, i64) = sqlx::query_as(
            "SELECT last_id, total_enqueued FROM embed_backfill_watermark \
             WHERE source_type = 'turn' AND model = 'bge-m3'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(total, 2);
        assert_eq!(last_id, "", "completed pass resets the cursor");

        // Run twice → same queue, nothing re-enqueued.
        let second = run_backfill(&pool, "bge-m3", 1, false).await.unwrap();
        assert_eq!(second.sources["turn"].enqueued, 0);
        assert_eq!(second.sources["turn"].already_queued, 2);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM pending_indexing").await, 2);
        let (total2,): (i64,) = sqlx::query_as(
            "SELECT total_enqueued FROM embed_backfill_watermark \
             WHERE source_type = 'turn' AND model = 'bge-m3'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(total2, 2, "idempotent re-run adds nothing to the tally");
    }

    #[tokio::test]
    async fn interrupted_pass_resumes_from_watermark() {
        let (_tmp, pool) = setup().await;
        let a = insert_turn(&pool, "alpha").await;
        let b = insert_turn(&pool, "beta").await;
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };

        // Simulate an interrupted pass that already processed `lo`.
        advance_watermark(&pool, "turn", "bge-m3", &lo, 1).await.unwrap();

        let report = run_backfill(&pool, "bge-m3", 100, false).await.unwrap();
        assert_eq!(report.sources["turn"].scanned, 1, "resumes past the cursor");
        let (path,): (String,) = sqlx::query_as("SELECT path FROM pending_indexing")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(path, format!("db://turn/{hi}"));
    }

    #[tokio::test]
    async fn checksum_invalidation_enqueues_reembed() {
        let (_tmp, pool) = setup().await;
        let turn_id = insert_turn(&pool, "original content").await;
        let uri = format!("db://turn/{turn_id}");

        run_backfill(&pool, "bge-m3", 100, false).await.unwrap();
        // Simulate the worker having indexed + embedded it.
        sqlx::query("UPDATE pending_indexing SET status = 'done' WHERE path = ?")
            .bind(&uri)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memory_documents (id, source_path, body, checksum) VALUES ('D1', ?, ?, ?)")
            .bind(&uri)
            .bind("original content")
            .bind(embed_checksum("original content"))
            .execute(&pool)
            .await
            .unwrap();

        // No change → unchanged, queue untouched.
        let same = run_backfill(&pool, "bge-m3", 100, false).await.unwrap();
        assert_eq!(same.sources["turn"].unchanged, 1);
        assert_eq!(same.sources["turn"].enqueued, 0);
        let (status,): (String,) = sqlx::query_as("SELECT status FROM pending_indexing WHERE path = ?")
            .bind(&uri)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "done");

        // Content changes → checksum mismatch → re-enqueued for re-embed.
        sqlx::query("UPDATE turns SET content = 'edited content' WHERE id = ?")
            .bind(&turn_id)
            .execute(&pool)
            .await
            .unwrap();
        let invalidated = run_backfill(&pool, "bge-m3", 100, false).await.unwrap();
        assert_eq!(invalidated.sources["turn"].enqueued, 1, "checksum change must re-embed");
        let (status,): (String,) = sqlx::query_as("SELECT status FROM pending_indexing WHERE path = ?")
            .bind(&uri)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM pending_indexing").await, 1, "URI unique — no dup rows");
    }

    #[tokio::test]
    async fn dry_run_reports_counts_and_writes_nothing() {
        let (_tmp, pool) = setup().await;
        insert_turn(&pool, "turn one").await;
        insert_turn(&pool, "turn two").await;
        sqlx::query("INSERT INTO learnings (id, title, body) VALUES ('L1', 'T', 'B')")
            .execute(&pool)
            .await
            .unwrap();

        let report = run_backfill(&pool, "bge-m3", 100, true).await.unwrap();
        assert!(report.dry_run);
        assert_eq!(report.sources["turn"].enqueued, 2);
        assert_eq!(report.sources["learning"].enqueued, 1);
        assert_eq!(report.total_enqueued(), 3);

        assert_eq!(count(&pool, "SELECT COUNT(*) FROM pending_indexing").await, 0, "dry-run wrote the queue");
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM embed_backfill_watermark").await,
            0,
            "dry-run advanced the watermark"
        );
    }

    #[tokio::test]
    async fn empty_and_dangling_objects_are_skipped() {
        let (_tmp, pool) = setup().await;
        insert_turn(&pool, "   \n  ").await; // whitespace-only → skip
        sqlx::query(
            "INSERT INTO wiki_pages (id, topic, slug, path, title) \
             VALUES ('W1', 'Ghost', 'ghost', '/nonexistent/ghost.md', 'Ghost')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let report = run_backfill(&pool, "bge-m3", 100, false).await.unwrap();
        assert_eq!(report.sources["turn"].skipped, 1);
        assert_eq!(report.sources["wiki"].skipped, 1);
        assert_eq!(report.total_enqueued(), 0);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM pending_indexing").await, 0);
    }
}
