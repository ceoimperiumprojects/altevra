//! Continuous embedder worker (v0.3.3).
//!
//! Polls the `embedder_queue` table for `status='pending'` chunks, calls the
//! configured `AsyncEmbeddingProvider` (Gemini in production, MockEmbedder in
//! tests), and writes vectors via `vector_store::write_vector_guarded` (R2
//! dim-gate: the model+dim registered in `embed_meta` is enforced on every
//! write). Rate-limited with a token bucket so we stay well under the Gemini
//! free tier (1500 RPM).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::{Row, SqlitePool};
use tokio::sync::watch;
use uuid::Uuid;

use crate::embedding::AsyncEmbeddingProvider;
use crate::vector_store;

#[derive(Debug, Clone)]
pub struct EmbedderWorkerConfig {
    pub batch_size: usize,
    pub poll_idle_ms: u64,   // sleep when queue empty
    pub poll_active_ms: u64, // sleep between consecutive non-empty batches
    pub rate_limit_rpm: u32,
    pub max_fail_count: u32,
}

impl Default for EmbedderWorkerConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            poll_idle_ms: 30_000,
            poll_active_ms: 5_000,
            rate_limit_rpm: 1000,
            max_fail_count: 5,
        }
    }
}

/// Token-bucket rate limiter. Simple, monotonic, no external deps.
struct RateLimiter {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: Instant,
}

impl RateLimiter {
    fn new(rpm: u32) -> Self {
        let per_sec = rpm as f64 / 60.0;
        Self {
            capacity: per_sec.max(1.0),
            tokens: per_sec.max(1.0),
            refill_per_sec: per_sec,
            last_refill: Instant::now(),
        }
    }

    // refill/acquire are kept as legacy helpers for the v0.3.1-era worker.
    // Live workloads use the async-safe inline snapshot path in EmbedderWorker
    // (which mirrors `altevra_llm::RateLimiter::acquire`).
    #[allow(dead_code)]
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last_refill = now;
    }

    #[allow(dead_code)]
    async fn acquire(&mut self) {
        loop {
            self.refill();
            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                return;
            }
            let needed = 1.0 - self.tokens;
            let sleep_secs = (needed / self.refill_per_sec).max(0.001);
            tokio::time::sleep(Duration::from_secs_f64(sleep_secs)).await;
        }
    }
}

pub struct EmbedderWorker<E: AsyncEmbeddingProvider> {
    embedder: E,
    pool: SqlitePool,
    config: EmbedderWorkerConfig,
    limiter: Arc<Mutex<RateLimiter>>,
}

#[derive(Debug, Clone)]
pub struct QueueStats {
    pub pending: i64,
    pub in_progress: i64,
    pub done: i64,
    pub failed: i64,
}

impl<E: AsyncEmbeddingProvider + 'static> EmbedderWorker<E> {
    pub fn new(embedder: E, pool: SqlitePool, config: EmbedderWorkerConfig) -> Self {
        let limiter = Arc::new(Mutex::new(RateLimiter::new(config.rate_limit_rpm)));
        Self {
            embedder,
            pool,
            config,
            limiter,
        }
    }

    /// Seed the queue: insert (status='pending') for every chunk that lacks a vector.
    pub async fn seed_queue(&self) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            r#"SELECT id FROM memory_chunks
               WHERE id NOT IN (SELECT chunk_id FROM memory_chunk_vectors_v2)
                 AND id NOT IN (SELECT chunk_id FROM embedder_queue WHERE status IN ('pending','in_progress','done'))"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let n = rows.len();
        for r in rows {
            let id: String = r.get("id");
            let _ = sqlx::query(
                r#"INSERT INTO embedder_queue (chunk_id, status) VALUES (?, 'pending')
                   ON CONFLICT(chunk_id) DO NOTHING"#,
            )
            .bind(id)
            .execute(&self.pool)
            .await;
        }
        Ok(n)
    }

    /// Drain the `pending_indexing` file queue (fed by the vault watcher daemon
    /// and the brain's `vault_indexer` job). For each pending path:
    ///
    ///   1. `ingest_file` → chunk the markdown,
    ///   2. `guard_document` → redact secrets/PII BEFORE anything persists,
    ///   3. upsert `memory_documents` + replace its `memory_chunks`,
    ///   4. enqueue every chunk into `embedder_queue` (the lane `tick` embeds).
    ///
    /// Idempotency: a path whose raw-byte checksum matches the stored document
    /// is marked `done` without re-chunking. A changed file replaces its old
    /// chunks (their `embedder_queue`/vector rows are removed explicitly — no
    /// reliance on FK cascades). Unreadable paths are marked `failed` with the
    /// error preserved in the row.
    ///
    /// Returns the number of queue rows processed (done + failed).
    pub async fn drain_pending_files(&self) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            r#"SELECT id, path FROM pending_indexing
               WHERE status = 'pending'
               ORDER BY queued_at
               LIMIT ?"#,
        )
        .bind(self.config.batch_size as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut processed = 0usize;
        for row in rows {
            let row_id: String = row.get("id");
            let path: String = row.get("path");
            match self.index_file(&path).await {
                Ok(()) => {
                    let _ = sqlx::query(
                        r#"UPDATE pending_indexing
                           SET status = 'done',
                               last_attempt_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                               error = NULL
                           WHERE id = ?"#,
                    )
                    .bind(&row_id)
                    .execute(&self.pool)
                    .await;
                }
                Err(e) => {
                    let _ = sqlx::query(
                        r#"UPDATE pending_indexing
                           SET status = 'failed',
                               last_attempt_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                               error = ?,
                               fail_count = fail_count + 1
                           WHERE id = ?"#,
                    )
                    .bind(e.to_string())
                    .bind(&row_id)
                    .execute(&self.pool)
                    .await;
                }
            }
            processed += 1;
        }
        Ok(processed)
    }

    /// Ingest ONE queued path into memory_documents/memory_chunks and enqueue
    /// its chunks for embedding. No-op when the content is byte-identical to
    /// the stored document (checksum match). Synthetic `db://` URIs (R2
    /// backfill contract) resolve their text from the DB instead of the fs.
    async fn index_file(&self, path: &str) -> anyhow::Result<()> {
        use crate::chunker::DEFAULT_CHUNK_SIZE;
        use crate::ingestion::{guard_document, ingest_file};
        use altevra_core::security::Sensitivity;

        if let Some((otype, obj_id)) = crate::db_uri::parse_db_uri(path) {
            return self.index_db_object(path, otype, obj_id).await;
        }

        let mut doc = ingest_file(std::path::Path::new(path), DEFAULT_CHUNK_SIZE)?;

        let existing: Option<(String, String)> =
            sqlx::query_as("SELECT id, checksum FROM memory_documents WHERE source_path = ?")
                .bind(path)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((_, ref checksum)) = existing {
            if *checksum == doc.checksum {
                return Ok(()); // unchanged — already indexed
            }
        }

        // Redact secrets/PII in every chunk BEFORE persisting (same contract as
        // the capture lane: unguarded text never reaches the DB or the embedder).
        guard_document(&mut doc, Sensitivity::Internal);

        let title = doc
            .frontmatter
            .as_ref()
            .and_then(|f| f.get("title"))
            .and_then(|v| v.as_str())
            .map(String::from);
        self.persist_document(path, title, &doc, existing.map(|(id, _)| id))
            .await
    }

    /// Index ONE synthetic `db://<type>/<id>` object (R2 backfill contract):
    /// resolve the embedded text from the DB, checksum = sha256 of that text,
    /// re-index only on checksum change, turn→chunks capped per turn.
    async fn index_db_object(&self, uri: &str, otype: &str, obj_id: &str) -> anyhow::Result<()> {
        use crate::backfill::resolve_db_object_text;
        use crate::chunker::DEFAULT_CHUNK_SIZE;
        use crate::db_uri::MAX_CHUNKS_PER_TURN;
        use crate::ingestion::{guard_document, ingest_text};
        use altevra_core::security::Sensitivity;

        let Some(obj) = resolve_db_object_text(&self.pool, otype, obj_id).await? else {
            anyhow::bail!("db object not found or unreadable: {uri}");
        };

        // ingest_text's checksum is sha256 over the full text bytes — exactly
        // the db:// contract checksum (db_uri::embed_checksum).
        let mut doc = ingest_text(&obj.text, None, DEFAULT_CHUNK_SIZE);

        let existing: Option<(String, String)> =
            sqlx::query_as("SELECT id, checksum FROM memory_documents WHERE source_path = ?")
                .bind(uri)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((_, ref checksum)) = existing {
            if *checksum == doc.checksum {
                return Ok(()); // unchanged — already indexed at this text
            }
        }

        // Cap turn chunks: one verbose assistant reply must not flood the queue.
        if otype == "turn" && doc.chunks.len() > MAX_CHUNKS_PER_TURN {
            doc.chunks.truncate(MAX_CHUNKS_PER_TURN);
        }

        // Same guard contract as the file lane — even though turns are
        // redacted at capture time, re-guarding here is cheap and fail-closed.
        guard_document(&mut doc, Sensitivity::Internal);

        self.persist_document(uri, obj.title.clone(), &doc, existing.map(|(id, _)| id))
            .await
    }

    /// Upsert a (guarded) document + replace its chunks + enqueue them for
    /// embedding. Shared by the file lane and the `db://` object lane.
    async fn persist_document(
        &self,
        source_path: &str,
        title: Option<String>,
        doc: &crate::ingestion::IngestedDocument,
        existing_id: Option<String>,
    ) -> anyhow::Result<()> {
        let body: String = doc
            .chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let doc_id = match existing_id {
            Some(id) => {
                sqlx::query(
                    r#"UPDATE memory_documents
                       SET title = ?, body = ?, checksum = ?,
                           indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                       WHERE id = ?"#,
                )
                .bind(&title)
                .bind(&body)
                .bind(&doc.checksum)
                .bind(&id)
                .execute(&self.pool)
                .await?;
                // Stale chunks die with their queue/vector rows. Explicit
                // deletes — SQLite FK cascades only fire when the pragma is on.
                sqlx::query(
                    "DELETE FROM embedder_queue WHERE chunk_id IN \
                     (SELECT id FROM memory_chunks WHERE document_id = ?)",
                )
                .bind(&id)
                .execute(&self.pool)
                .await?;
                sqlx::query(
                    "DELETE FROM memory_chunk_vectors_v2 WHERE chunk_id IN \
                     (SELECT id FROM memory_chunks WHERE document_id = ?)",
                )
                .bind(&id)
                .execute(&self.pool)
                .await?;
                sqlx::query("DELETE FROM memory_chunks WHERE document_id = ?")
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                id
            }
            None => {
                let id = doc.document_id.to_string();
                sqlx::query(
                    "INSERT INTO memory_documents (id, source_path, title, body, checksum) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(source_path)
                .bind(&title)
                .bind(&body)
                .bind(&doc.checksum)
                .execute(&self.pool)
                .await?;
                id
            }
        };

        for c in &doc.chunks {
            let heading =
                serde_json::to_string(&c.meta.heading_path).unwrap_or_else(|_| "[]".into());
            sqlx::query(
                r#"INSERT INTO memory_chunks
                   (id, document_id, heading_path, text, checksum, start_byte, end_byte)
                   VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(c.id.to_string())
            .bind(&doc_id)
            .bind(heading)
            .bind(&c.text)
            .bind(&c.checksum)
            .bind(c.meta.start_byte as i64)
            .bind(c.meta.end_byte as i64)
            .execute(&self.pool)
            .await?;
            sqlx::query(
                r#"INSERT INTO embedder_queue (chunk_id, status) VALUES (?, 'pending')
                   ON CONFLICT(chunk_id) DO NOTHING"#,
            )
            .bind(c.id.to_string())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Run a single batch. Returns the number of chunks SUCCESSFULLY embedded.
    #[allow(clippy::await_holding_lock)]
    pub async fn tick(&self) -> anyhow::Result<usize> {
        // Stage 0: drain the pending_indexing file queue into memory_chunks +
        // embedder_queue so queued vault files actually reach the embed lane.
        // Best-effort: a schema without 006/009 tables (minimal test pools)
        // must not break embedding; per-file errors are persisted on the row.
        let _ = self.drain_pending_files().await;

        // Claim a batch: read pending chunks, mark them in_progress in one query.
        let rows = sqlx::query(
            r#"SELECT eq.chunk_id, mc.text
               FROM embedder_queue eq
               JOIN memory_chunks mc ON mc.id = eq.chunk_id
               WHERE eq.status = 'pending'
               ORDER BY eq.enqueued_at
               LIMIT ?"#,
        )
        .bind(self.config.batch_size as i64)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(0);
        }

        // R2 write-gate: record the active model+dim once (idempotent) so
        // write_vector_guarded can refuse foreign-dim vectors below.
        vector_store::register_model_dim(
            &self.pool,
            self.embedder.model_name(),
            self.embedder.dim(),
        )
        .await?;

        let mut success = 0usize;
        for row in rows {
            let chunk_id_text: String = row.get("chunk_id");
            let text: String = row.get("text");
            let chunk_id = match Uuid::parse_str(&chunk_id_text) {
                Ok(id) => id,
                Err(_) => continue,
            };

            // Mark in_progress.
            let _ = sqlx::query(
                r#"UPDATE embedder_queue
                   SET status = 'in_progress',
                       started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                   WHERE chunk_id = ?"#,
            )
            .bind(&chunk_id_text)
            .execute(&self.pool)
            .await;

            // Rate-limit before each embed call.
            // Manual snapshot/drop pattern: pull state from the guard, drop it
            // before any .await, do the math, then re-lock to commit.
            // clippy::await_holding_lock is a false positive here because the
            // guard is explicitly dropped via `drop(g)` before any await.
            #[allow(clippy::await_holding_lock)]
            {
                let g = self.limiter.lock().unwrap();
                let snapshot = (g.refill_per_sec, g.tokens, g.capacity, g.last_refill);
                drop(g);
                // Recreate limiter state in an async-safe path. We re-lock briefly
                // around the math but await OUTSIDE the lock to keep Send semantics.
                let (per_sec, mut tokens, cap, last) = snapshot;
                let now = Instant::now();
                let elapsed = now.duration_since(last).as_secs_f64();
                tokens = (tokens + elapsed * per_sec).min(cap);
                if tokens < 1.0 {
                    let needed = 1.0 - tokens;
                    tokio::time::sleep(Duration::from_secs_f64((needed / per_sec).max(0.001)))
                        .await;
                    tokens = 1.0;
                }
                tokens -= 1.0;
                let mut g2 = self.limiter.lock().unwrap();
                g2.tokens = tokens;
                g2.last_refill = Instant::now();
            }

            match self.embedder.embed(&text).await {
                Ok(emb) => {
                    if let Err(e) = vector_store::write_vector_guarded(
                        &self.pool,
                        chunk_id,
                        self.embedder.model_name(),
                        &emb.vector,
                    )
                    .await
                    {
                        self.mark_failed(&chunk_id_text, &format!("write_vector_guarded: {e}"))
                            .await;
                        continue;
                    }
                    let _ = sqlx::query(
                        r#"UPDATE embedder_queue
                           SET status = 'done',
                               finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                               last_error = NULL
                           WHERE chunk_id = ?"#,
                    )
                    .bind(&chunk_id_text)
                    .execute(&self.pool)
                    .await;
                    success += 1;
                }
                Err(e) => {
                    self.mark_failed(&chunk_id_text, &e.to_string()).await;
                }
            }
        }
        Ok(success)
    }

    async fn mark_failed(&self, chunk_id_text: &str, error: &str) {
        let _ = sqlx::query(
            r#"UPDATE embedder_queue
               SET fail_count = fail_count + 1,
                   last_error = ?,
                   status = CASE
                     WHEN fail_count + 1 >= ? THEN 'failed'
                     ELSE 'pending'
                   END,
                   finished_at = CASE
                     WHEN fail_count + 1 >= ? THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     ELSE finished_at
                   END
               WHERE chunk_id = ?"#,
        )
        .bind(error)
        .bind(self.config.max_fail_count as i64)
        .bind(self.config.max_fail_count as i64)
        .bind(chunk_id_text)
        .execute(&self.pool)
        .await;
    }

    pub async fn stats(&self) -> anyhow::Result<QueueStats> {
        let mut stats = QueueStats {
            pending: 0,
            in_progress: 0,
            done: 0,
            failed: 0,
        };
        let rows = sqlx::query("SELECT status, COUNT(*) AS n FROM embedder_queue GROUP BY status")
            .fetch_all(&self.pool)
            .await?;
        for r in rows {
            let s: String = r.get("status");
            let n: i64 = r.get("n");
            match s.as_str() {
                "pending" => stats.pending = n,
                "in_progress" => stats.in_progress = n,
                "done" => stats.done = n,
                "failed" => stats.failed = n,
                _ => {}
            }
        }
        Ok(stats)
    }

    /// Loop forever until shutdown_rx flips to true.
    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) -> anyhow::Result<()> {
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            let processed = self.tick().await.unwrap_or(0);
            let sleep_ms = if processed == 0 {
                self.config.poll_idle_ms
            } else {
                self.config.poll_active_ms
            };
            tokio::select! {
                _ = shutdown_rx.changed() => {}
                _ = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::Embedding;
    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Deterministic mock — vector = [text.len() as f32].
    struct MockEmbedder {
        fail_first_n: Arc<Mutex<u32>>,
    }

    impl MockEmbedder {
        fn new() -> Self {
            Self {
                fail_first_n: Arc::new(Mutex::new(0)),
            }
        }
        fn with_failures(n: u32) -> Self {
            Self {
                fail_first_n: Arc::new(Mutex::new(n)),
            }
        }
    }

    #[async_trait]
    impl AsyncEmbeddingProvider for MockEmbedder {
        async fn embed(&self, text: &str) -> anyhow::Result<Embedding> {
            let mut g = self.fail_first_n.lock().unwrap();
            if *g > 0 {
                *g -= 1;
                anyhow::bail!("simulated 429 rate_limit");
            }
            Ok(Embedding {
                vector: vec![text.len() as f32, 1.0, 2.0],
                model: "mock".to_string(),
            })
        }
        fn dim(&self) -> usize {
            3
        }
        fn model_name(&self) -> &str {
            "mock"
        }
    }

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        create_schema(&pool).await;
        pool
    }

    /// File-backed per-test DB (R2 db:// object-lane tests).
    async fn setup_file_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::TempDir::new().unwrap();
        let url = format!("sqlite://{}?mode=rwc", tmp.path().join("test.db").display());
        let pool = SqlitePoolOptions::new().connect(&url).await.unwrap();
        create_schema(&pool).await;
        (tmp, pool)
    }

    /// Minimal schema needed for worker tests.
    async fn create_schema(pool: &SqlitePool) {
        sqlx::query(
            r#"CREATE TABLE memory_chunks (
                id TEXT PRIMARY KEY,
                text TEXT NOT NULL,
                checksum TEXT NOT NULL DEFAULT '',
                start_byte INTEGER NOT NULL DEFAULT 0,
                end_byte INTEGER NOT NULL DEFAULT 0,
                heading_path TEXT,
                document_id TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            )"#,
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE embedder_queue (
                chunk_id TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'pending',
                enqueued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                started_at TEXT, finished_at TEXT,
                fail_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )"#,
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE memory_chunk_vectors_v2 (
                chunk_id TEXT PRIMARY KEY,
                model TEXT NOT NULL,
                dim INTEGER NOT NULL,
                embedding TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            )"#,
        )
        .execute(pool)
        .await
        .unwrap();
        // File-queue tables (009 + 006 shapes) for the drain_pending_files path.
        sqlx::query(
            r#"CREATE TABLE pending_indexing (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                queued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                status TEXT NOT NULL DEFAULT 'pending',
                last_attempt_at TEXT,
                error TEXT,
                fail_count INTEGER NOT NULL DEFAULT 0,
                UNIQUE (path)
            )"#,
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE memory_documents (
                id TEXT PRIMARY KEY,
                project_id TEXT,
                source_path TEXT NOT NULL,
                title TEXT,
                body TEXT NOT NULL,
                checksum TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                indexed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                UNIQUE (source_path)
            )"#,
        )
        .execute(pool)
        .await
        .unwrap();
        // 041: model+dim registry for the R2 write-gate.
        sqlx::query(
            r#"CREATE TABLE embed_meta (
                model TEXT PRIMARY KEY,
                dim INTEGER NOT NULL,
                set_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            )"#,
        )
        .execute(pool)
        .await
        .unwrap();
        // Turns table so db://turn/<id> objects can be resolved.
        sqlx::query("CREATE TABLE turns (id TEXT PRIMARY KEY, content TEXT NOT NULL)")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn enqueue_path(pool: &SqlitePool, path: &std::path::Path) {
        sqlx::query("INSERT INTO pending_indexing (id, path, status) VALUES (?, ?, 'pending')")
            .bind(Uuid::new_v4().to_string())
            .bind(path.to_string_lossy().to_string())
            .execute(pool)
            .await
            .unwrap();
    }

    async fn count(pool: &SqlitePool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    async fn insert_chunks(pool: &SqlitePool, n: usize) -> Vec<Uuid> {
        let mut ids = vec![];
        for i in 0..n {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO memory_chunks (id, text) VALUES (?, ?)")
                .bind(id.to_string())
                .bind(format!("chunk-{i}"))
                .execute(pool)
                .await
                .unwrap();
            ids.push(id);
        }
        ids
    }

    #[tokio::test]
    async fn seed_then_tick_drains_queue() {
        let pool = setup_pool().await;
        insert_chunks(&pool, 5).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig {
                batch_size: 100,
                rate_limit_rpm: 100_000,
                ..EmbedderWorkerConfig::default()
            },
        );
        assert_eq!(worker.seed_queue().await.unwrap(), 5);
        let n = worker.tick().await.unwrap();
        assert_eq!(n, 5);
        assert_eq!(vector_store::vector_count(&pool).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn batch_size_limits_tick() {
        let pool = setup_pool().await;
        insert_chunks(&pool, 25).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig {
                batch_size: 10,
                rate_limit_rpm: 100_000,
                ..EmbedderWorkerConfig::default()
            },
        );
        worker.seed_queue().await.unwrap();
        let n = worker.tick().await.unwrap();
        assert_eq!(n, 10);
    }

    #[tokio::test]
    async fn failed_chunks_eventually_marked_failed() {
        let pool = setup_pool().await;
        insert_chunks(&pool, 1).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::with_failures(10),
            pool.clone(),
            EmbedderWorkerConfig {
                batch_size: 5,
                rate_limit_rpm: 100_000,
                max_fail_count: 3,
                ..EmbedderWorkerConfig::default()
            },
        );
        worker.seed_queue().await.unwrap();
        for _ in 0..5 {
            let _ = worker.tick().await;
        }
        let stats = worker.stats().await.unwrap();
        assert_eq!(stats.failed, 1);
    }

    #[tokio::test]
    async fn seed_is_idempotent() {
        let pool = setup_pool().await;
        insert_chunks(&pool, 3).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig::default(),
        );
        assert_eq!(worker.seed_queue().await.unwrap(), 3);
        assert_eq!(worker.seed_queue().await.unwrap(), 0);
    }

    // ---- pending_indexing drain (P0 §5: the file queue gets a consumer) ----

    #[tokio::test]
    async fn tick_drains_pending_indexing_through_to_vectors() {
        // End-to-end over the previously-dead path: a queued FILE (not a chunk)
        // is ingested → chunked → enqueued → embedded, all inside one tick.
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("note.md");
        std::fs::write(
            &file,
            "---\ntitle: Drain Test\n---\n\n# Heading\n\nA body paragraph about altevra.\n",
        )
        .unwrap();

        let pool = setup_pool().await;
        enqueue_path(&pool, &file).await;

        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig {
                rate_limit_rpm: 100_000,
                ..EmbedderWorkerConfig::default()
            },
        );
        let embedded = worker.tick().await.unwrap();
        assert!(embedded >= 1, "drained chunks must be embedded in-tick");

        // Document + chunks persisted.
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_documents").await, 1);
        let chunks = count(&pool, "SELECT COUNT(*) FROM memory_chunks").await;
        assert!(chunks >= 1, "ingested file must produce chunks");
        // Every chunk got a vector (NoOp-free path: MockEmbedder).
        assert_eq!(vector_store::vector_count(&pool).await.unwrap() as i64, chunks);
        // Queue row is consumed.
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM pending_indexing WHERE status = 'done'"
            )
            .await,
            1
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM pending_indexing WHERE status = 'pending'"
            )
            .await,
            0
        );
        // Title came from frontmatter.
        let title: Option<String> =
            sqlx::query_scalar("SELECT title FROM memory_documents LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title.as_deref(), Some("Drain Test"));
    }

    #[tokio::test]
    async fn drain_marks_missing_file_failed_with_error() {
        let pool = setup_pool().await;
        enqueue_path(&pool, std::path::Path::new("/nonexistent/missing.md")).await;

        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig::default(),
        );
        let processed = worker.drain_pending_files().await.unwrap();
        assert_eq!(processed, 1);

        let (status, fail_count, error): (String, i64, Option<String>) = sqlx::query_as(
            "SELECT status, fail_count, error FROM pending_indexing LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(fail_count, 1);
        assert!(error.is_some(), "the ingest error must be preserved");
        // Nothing half-written.
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_documents").await, 0);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_chunks").await, 0);
    }

    #[tokio::test]
    async fn drain_is_idempotent_and_reindexes_changed_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("doc.md");
        std::fs::write(&file, "# V1\n\noriginal content.\n").unwrap();

        let pool = setup_pool().await;
        enqueue_path(&pool, &file).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig {
                rate_limit_rpm: 100_000,
                ..EmbedderWorkerConfig::default()
            },
        );
        worker.tick().await.unwrap();
        let chunks_v1 = count(&pool, "SELECT COUNT(*) FROM memory_chunks").await;

        // Re-queue the SAME unchanged file → checksum match → done, no dupes.
        sqlx::query("UPDATE pending_indexing SET status = 'pending'")
            .execute(&pool)
            .await
            .unwrap();
        worker.tick().await.unwrap();
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM memory_chunks").await,
            chunks_v1,
            "unchanged file must not duplicate chunks"
        );
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_documents").await, 1);

        // Change the file, re-queue → old chunks replaced, doc stays single.
        std::fs::write(&file, "# V2\n\ncompletely different body now.\n").unwrap();
        sqlx::query("UPDATE pending_indexing SET status = 'pending'")
            .execute(&pool)
            .await
            .unwrap();
        worker.tick().await.unwrap();
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM memory_documents").await,
            1,
            "changed file updates its document in place"
        );
        let body: String = sqlx::query_scalar("SELECT body FROM memory_documents LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(body.contains("different body"), "body re-indexed: {body}");
        // No orphaned queue rows for deleted chunks.
        let orphans = count(
            &pool,
            "SELECT COUNT(*) FROM embedder_queue WHERE chunk_id NOT IN (SELECT id FROM memory_chunks)",
        )
        .await;
        assert_eq!(orphans, 0, "stale chunks must leave no queue orphans");
    }

    #[tokio::test]
    async fn drain_redacts_secrets_before_persisting() {
        // The guard contract holds on the file lane too: a key in a vault file
        // never reaches memory_chunks/documents in plaintext.
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("leaky.md");
        std::fs::write(
            &file,
            "# Note\n\nkey is sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA ok.\n",
        )
        .unwrap();

        let pool = setup_pool().await;
        enqueue_path(&pool, &file).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig::default(),
        );
        worker.drain_pending_files().await.unwrap();

        let body: String = sqlx::query_scalar("SELECT body FROM memory_documents LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!body.contains("sk-ant-api03-"), "secret leaked into body");
        let leaked = count(
            &pool,
            "SELECT COUNT(*) FROM memory_chunks WHERE text LIKE '%sk-ant-api03-%'",
        )
        .await;
        assert_eq!(leaked, 0, "secret leaked into chunks");
    }

    #[tokio::test]
    async fn stats_reflects_state_transitions() {
        let pool = setup_pool().await;
        insert_chunks(&pool, 2).await;
        let worker = EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig {
                rate_limit_rpm: 100_000,
                ..EmbedderWorkerConfig::default()
            },
        );
        worker.seed_queue().await.unwrap();
        let s = worker.stats().await.unwrap();
        assert_eq!(s.pending, 2);
        worker.tick().await.unwrap();
        let s = worker.stats().await.unwrap();
        assert_eq!(s.done, 2);
        assert_eq!(s.pending, 0);
    }

    // ---- db:// synthetic objects (R2 backfill contract) ----

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

    fn test_worker(pool: &SqlitePool) -> EmbedderWorker<MockEmbedder> {
        EmbedderWorker::new(
            MockEmbedder::new(),
            pool.clone(),
            EmbedderWorkerConfig {
                rate_limit_rpm: 100_000,
                ..EmbedderWorkerConfig::default()
            },
        )
    }

    #[tokio::test]
    async fn tick_indexes_db_turn_object_through_to_vectors() {
        let (_tmp, pool) = setup_file_pool().await;
        let turn_id = insert_turn(&pool, "the user asked about embedding backfills").await;
        let uri = format!("db://turn/{turn_id}");
        enqueue_path(&pool, std::path::Path::new(&uri)).await;

        let worker = test_worker(&pool);
        let embedded = worker.tick().await.unwrap();
        assert!(embedded >= 1, "db:// chunks must embed in-tick");

        // Document persisted under the synthetic uri with the contract checksum
        // (sha256 of the embedded text).
        let (source_path, checksum): (String, String) = sqlx::query_as(
            "SELECT source_path, checksum FROM memory_documents LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(source_path, uri);
        assert_eq!(
            checksum,
            crate::db_uri::embed_checksum("the user asked about embedding backfills")
        );
        let chunks = count(&pool, "SELECT COUNT(*) FROM memory_chunks").await;
        assert!(chunks >= 1);
        assert_eq!(vector_store::vector_count(&pool).await.unwrap(), chunks);
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM pending_indexing WHERE status = 'done'").await,
            1
        );
    }

    #[tokio::test]
    async fn db_turn_chunks_are_capped_per_turn() {
        let (_tmp, pool) = setup_file_pool().await;
        // A monster turn: 40 sections of ~1.5k chars → way past the cap.
        let huge: String = (0..40)
            .map(|i| format!("# Section {i}\n\n{}\n\n", "altevra ".repeat(180)))
            .collect();
        let turn_id = insert_turn(&pool, &huge).await;
        enqueue_path(&pool, std::path::Path::new(&format!("db://turn/{turn_id}"))).await;

        let worker = test_worker(&pool);
        worker.tick().await.unwrap();

        let chunks = count(&pool, "SELECT COUNT(*) FROM memory_chunks").await;
        assert!(
            (1..=crate::db_uri::MAX_CHUNKS_PER_TURN as i64).contains(&chunks),
            "turn chunks must be capped at {} (got {chunks})",
            crate::db_uri::MAX_CHUNKS_PER_TURN
        );
    }

    #[tokio::test]
    async fn db_object_checksum_change_reindexes_in_place() {
        let (_tmp, pool) = setup_file_pool().await;
        let turn_id = insert_turn(&pool, "version one of the thought").await;
        let uri = format!("db://turn/{turn_id}");
        enqueue_path(&pool, std::path::Path::new(&uri)).await;

        let worker = test_worker(&pool);
        worker.tick().await.unwrap();

        // Unchanged re-queue → checksum match → no duplicate chunks.
        let chunks_v1 = count(&pool, "SELECT COUNT(*) FROM memory_chunks").await;
        sqlx::query("UPDATE pending_indexing SET status = 'pending'")
            .execute(&pool)
            .await
            .unwrap();
        worker.tick().await.unwrap();
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_chunks").await, chunks_v1);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_documents").await, 1);

        // Content edit → checksum change → re-indexed in place (re-embed).
        sqlx::query("UPDATE turns SET content = 'version two, fully rewritten' WHERE id = ?")
            .bind(&turn_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE pending_indexing SET status = 'pending'")
            .execute(&pool)
            .await
            .unwrap();
        worker.tick().await.unwrap();
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_documents").await, 1);
        let (checksum, body): (String, String) =
            sqlx::query_as("SELECT checksum, body FROM memory_documents LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            checksum,
            crate::db_uri::embed_checksum("version two, fully rewritten")
        );
        assert!(body.contains("version two"), "body re-indexed: {body}");
        // No orphaned queue/vector rows for replaced chunks.
        let orphans = count(
            &pool,
            "SELECT COUNT(*) FROM embedder_queue WHERE chunk_id NOT IN (SELECT id FROM memory_chunks)",
        )
        .await;
        assert_eq!(orphans, 0);
    }

    #[tokio::test]
    async fn missing_db_object_marks_queue_row_failed() {
        let (_tmp, pool) = setup_file_pool().await;
        enqueue_path(
            &pool,
            std::path::Path::new("db://turn/00000000-0000-0000-0000-000000000000"),
        )
        .await;

        let worker = test_worker(&pool);
        worker.drain_pending_files().await.unwrap();

        let (status, error): (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM pending_indexing LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("not found"));
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM memory_documents").await, 0);
    }
}
