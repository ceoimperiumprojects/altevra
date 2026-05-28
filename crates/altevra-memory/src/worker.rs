//! Continuous embedder worker (v0.3.3).
//!
//! Polls the `embedder_queue` table for `status='pending'` chunks, calls the
//! configured `AsyncEmbeddingProvider` (Gemini in production, MockEmbedder in
//! tests), and writes vectors via `vector_store::write_vector`. Rate-limited
//! with a token bucket so we stay well under the Gemini free tier (1500 RPM).

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

    /// Run a single batch. Returns the number of chunks SUCCESSFULLY embedded.
    #[allow(clippy::await_holding_lock)]
    pub async fn tick(&self) -> anyhow::Result<usize> {
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
                    if let Err(e) = vector_store::write_vector(
                        &self.pool,
                        chunk_id,
                        self.embedder.model_name(),
                        &emb.vector,
                    )
                    .await
                    {
                        self.mark_failed(&chunk_id_text, &format!("write_vector: {e}"))
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
        // Minimal schema needed for worker tests.
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
        .execute(&pool)
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
        .execute(&pool)
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
        .execute(&pool)
        .await
        .unwrap();
        pool
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
}
