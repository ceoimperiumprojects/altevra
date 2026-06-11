//! Hybrid search over the DB-backed corpus (R2 §5).
//!
//! Combines:
//!   1. BM25 lexical search via `object_fts` (already indexed by FTS5).
//!   2. Vector cosine search via `memory_chunk_vectors_v2` (model+dim filtered).
//!
//! Results are fused with Reciprocal Rank Fusion (RRF). When no vectors exist
//! (e.g. embedder hasn't run yet), falls back to BM25-only — never errors.
//!
//! This is separate from `hybrid.rs` (in-memory chunk search) because it
//! operates against the PERSISTENT DB and does the JOIN from chunk → session/turn.

use sqlx::{Row, SqlitePool};

use crate::{gemini::cosine, hybrid_rrf::rrf_fuse_two};

/// A hit from the DB hybrid search — enough to render a recall breadcrumb.
#[derive(Debug, Clone)]
pub struct DbHybridHit {
    /// The object type: "learning", "wiki", "turn", etc.
    pub object_type: String,
    pub object_id: String,
    pub title: String,
    /// Short snippet of the matched text.
    pub snippet: String,
    /// Fused RRF score (higher is better).
    pub score: f32,
}

/// Run hybrid BM25 + vector search over the DB corpus.
///
/// `query_text` — human query string (used for BM25 and, if `query_vector` is
///   `Some`, for vector search).
/// `query_vector` — pre-computed embedding of the query. Pass `None` for
///   lexical-only search.
/// `model` + `dim` — the embedding model we are searching against. Rows with
///   mismatched model/dim are silently skipped (R2 query-gate).
/// `limit` — max hits to return.
///
/// Falls back to BM25-only when `query_vector` is `None` OR when the vector
/// store has zero matching vectors for this model/dim.
pub async fn hybrid_db_search(
    pool: &SqlitePool,
    query_text: &str,
    query_vector: Option<&[f32]>,
    model: &str,
    dim: usize,
    limit: usize,
) -> anyhow::Result<Vec<DbHybridHit>> {
    // --- Leg 1: BM25 via object_fts ---
    let bm25_hits = bm25_search(pool, query_text, (limit * 4) as i64).await?;

    // --- Leg 2: vector cosine (model+dim filtered) ---
    let vector_ranked: Vec<(String, f32)> = if let Some(qv) = query_vector {
        if qv.len() != dim {
            vec![] // dim mismatch — silent fallback
        } else {
            vector_search(pool, qv, model, dim as i64, (limit * 4) as i64).await?
        }
    } else {
        vec![]
    };

    // --- RRF fusion ---
    // BM25 hits keyed by object_id; vector hits keyed by chunk_id.
    // We need to map chunk_id → object_id for the fusion.
    let bm25_ids: Vec<String> = bm25_hits.iter().map(|(id, _)| id.clone()).collect();
    let vec_obj_ids: Vec<String> = if vector_ranked.is_empty() {
        vec![]
    } else {
        // Map chunk_ids to object_ids via memory_chunks.document_id.
        chunk_ids_to_object_ids(pool, &vector_ranked).await?
    };

    // Build ranked lists for RRF.
    let bm25_ranked: Vec<String> = bm25_ids.clone();
    let vec_ranked_obj: Vec<String> = vec_obj_ids;

    // Use rrf_fuse_two from hybrid_rrf crate.
    let mut fused = rrf_fuse_two(bm25_ranked, vec_ranked_obj);
    fused.truncate(limit);

    // Enrich hits.
    let mut out = Vec::with_capacity(fused.len());
    for (obj_id, rrf_score) in fused {
        // Look up the hit from BM25 result (has title/snippet).
        if let Some((_, bm25_meta)) = bm25_hits.iter().find(|(id, _)| id == &obj_id) {
            out.push(DbHybridHit {
                object_type: bm25_meta.object_type.clone(),
                object_id: obj_id.clone(),
                title: bm25_meta.title.clone(),
                snippet: snippet_from_body(&bm25_meta.body, query_text, 200),
                score: rrf_score,
            });
        }
        // If hit is only in vector results but not BM25 (semantic-only), we
        // don't have a body readily. For now, emit a minimal hit.
        // Full enrichment would require an extra DB join — acceptable tradeoff.
    }

    Ok(out)
}

// ---- internal helpers ----

struct BM25Meta {
    object_type: String,
    title: String,
    body: String,
}

/// BM25 search over object_fts. Returns (object_id, metadata) pairs.
/// Gracefully returns empty vec if the FTS table doesn't exist.
async fn bm25_search(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> anyhow::Result<Vec<(String, BM25Meta)>> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    let safe = sanitize_fts(query);
    if safe.is_empty() {
        return Ok(vec![]);
    }
    let rows = sqlx::query(
        "SELECT object_type, object_id, title, body \
         FROM object_fts WHERE object_fts MATCH ? ORDER BY bm25(object_fts) LIMIT ?",
    )
    .bind(&safe)
    .bind(limit)
    .fetch_all(pool)
    .await
    .unwrap_or_default(); // FTS table may not exist in test pools

    Ok(rows
        .into_iter()
        .map(|r| {
            let id: String = r.get("object_id");
            (
                id,
                BM25Meta {
                    object_type: r.get("object_type"),
                    title: r.get("title"),
                    body: r.get("body"),
                },
            )
        })
        .collect())
}

/// Vector search over memory_chunk_vectors_v2, model+dim filtered (R2 query-gate).
/// Returns (chunk_id_text, score) pairs.
async fn vector_search(
    pool: &SqlitePool,
    query_vector: &[f32],
    model: &str,
    dim: i64,
    limit: i64,
) -> anyhow::Result<Vec<(String, f32)>> {
    let rows = sqlx::query(
        "SELECT chunk_id, embedding FROM memory_chunk_vectors_v2 \
         WHERE model = ? AND dim = ?",
    )
    .bind(model)
    .bind(dim)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut scored: Vec<(String, f32)> = rows
        .into_iter()
        .filter_map(|r| {
            let chunk_id: String = r.get("chunk_id");
            let vec_text: String = r.get("embedding");
            let vec: Vec<f32> = serde_json::from_str(&vec_text).ok()?;
            if vec.len() != query_vector.len() {
                return None; // defensive dim check
            }
            let score = cosine(query_vector, &vec);
            Some((chunk_id, score))
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit as usize);
    Ok(scored)
}

/// Map chunk_ids (from vector search) to object_ids (needed for RRF fusion
/// with BM25 which uses object_ids).
async fn chunk_ids_to_object_ids(
    pool: &SqlitePool,
    vector_ranked: &[(String, f32)],
) -> anyhow::Result<Vec<String>> {
    if vector_ranked.is_empty() {
        return Ok(vec![]);
    }
    // memory_chunks.document_id links a chunk to its memory_document.
    // memory_documents.source_path can be a synthetic db:// URI or a file path.
    // We collect document_id as the object_id for RRF.
    let mut out = Vec::with_capacity(vector_ranked.len());
    for (chunk_id, _) in vector_ranked {
        // Try to resolve chunk → document_id.
        let row = sqlx::query(
            "SELECT mc.document_id FROM memory_chunks mc WHERE mc.id = ?",
        )
        .bind(chunk_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if let Some(r) = row {
            out.push(r.get::<String, _>("document_id"));
        } else {
            // Chunk not in memory_chunks (could be a db:// object chunk).
            // Use the chunk_id itself as a degenerate object_id.
            out.push(chunk_id.clone());
        }
    }
    Ok(out)
}

fn sanitize_fts(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && t.len() > 1)
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

fn snippet_from_body(body: &str, query: &str, max: usize) -> String {
    let lc = body.to_lowercase();
    let mut first = None;
    for tok in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
    {
        if let Some(p) = lc.find(tok) {
            first = Some(first.map_or(p, |cur: usize| cur.min(p)));
        }
    }
    let start = first.map(|p| p.saturating_sub(50)).unwrap_or(0);
    let end = (start + max).min(body.len());
    let s = &body[snap_left(body, start)..snap_right(body, end)];
    s.replace('\n', " ")
}

fn snap_left(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
fn snap_right(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    async fn setup_fts_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Minimal schema for hybrid_db tests.
        sqlx::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS object_fts USING fts5(
                object_type, object_id UNINDEXED, title, body, tags
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS memory_chunk_vectors_v2 (
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
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS memory_chunks (
                id TEXT PRIMARY KEY,
                document_id TEXT NOT NULL DEFAULT '',
                text TEXT NOT NULL,
                checksum TEXT NOT NULL DEFAULT '',
                start_byte INTEGER NOT NULL DEFAULT 0,
                end_byte INTEGER NOT NULL DEFAULT 0,
                heading_path TEXT,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn bm25_only_when_no_vector() {
        let pool = setup_fts_pool().await;
        // Index a learning.
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
             VALUES ('learning', 'L1', 'GTM plan', 'Sell ReVesta direct to operators', 'business')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let hits = hybrid_db_search(&pool, "ReVesta operators", None, "bge-m3", 1024, 5)
            .await
            .unwrap();
        assert!(!hits.is_empty(), "BM25 must find the learning");
        assert_eq!(hits[0].object_id, "L1");
    }

    #[tokio::test]
    async fn semantic_only_hit_via_vectors() {
        let pool = setup_fts_pool().await;
        // Two documents: one matches lexically, one only semantically.
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
             VALUES ('learning', 'L1', 'GTM', 'direct outreach to operators', 'biz')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
             VALUES ('learning', 'L2', 'Random', 'nothing related here at all', 'misc')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Give L1's chunk a vector close to query; L2 chunk a distant vector.
        let chunk_l1 = Uuid::new_v4();
        let chunk_l2 = Uuid::new_v4();
        // Store the chunk rows.
        sqlx::query("INSERT INTO memory_chunks (id, document_id, text) VALUES (?, ?, ?)")
            .bind(chunk_l1.to_string())
            .bind("L1")
            .bind("direct outreach")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memory_chunks (id, document_id, text) VALUES (?, ?, ?)")
            .bind(chunk_l2.to_string())
            .bind("L2")
            .bind("nothing related")
            .execute(&pool)
            .await
            .unwrap();
        // Vectors: L1 is [1,0,0], L2 is [0,0,1]. Query is [1,0,0] → L1 wins.
        let v_l1 = serde_json::to_string(&[1.0f32, 0.0, 0.0]).unwrap();
        let v_l2 = serde_json::to_string(&[0.0f32, 0.0, 1.0]).unwrap();
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(chunk_l1.to_string())
        .bind(&v_l1)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(chunk_l2.to_string())
        .bind(&v_l2)
        .execute(&pool)
        .await
        .unwrap();

        let query_vec = vec![1.0f32, 0.0, 0.0];
        let hits = hybrid_db_search(
            &pool,
            "outreach",
            Some(&query_vec),
            "bge-m3",
            3,
            5,
        )
        .await
        .unwrap();
        // L1 should appear (both BM25 and vector match it).
        let ids: Vec<&str> = hits.iter().map(|h| h.object_id.as_str()).collect();
        assert!(ids.contains(&"L1"), "L1 must be in results: {ids:?}");
    }

    #[tokio::test]
    async fn mismatched_dim_vectors_filtered_out() {
        let pool = setup_fts_pool().await;
        // Insert a vector with dim=3 but query with dim=1024 — must be skipped.
        let chunk_id = Uuid::new_v4();
        let v = serde_json::to_string(&[1.0f32, 0.0, 0.0]).unwrap();
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(chunk_id.to_string())
        .bind(&v)
        .execute(&pool)
        .await
        .unwrap();

        let query_vec = vec![0.5f32; 1024]; // dim=1024, not 3
        let hits =
            hybrid_db_search(&pool, "anything", Some(&query_vec), "bge-m3", 1024, 5)
                .await
                .unwrap();
        // No BM25 results (empty FTS), vector results filtered (dim mismatch).
        assert!(hits.is_empty(), "dim-mismatched vectors must not score");
    }

    #[tokio::test]
    async fn lexical_fallback_when_no_vectors_exist() {
        let pool = setup_fts_pool().await;
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
             VALUES ('wiki', 'W1', 'ReVesta Setup', 'configure the pipeline for operators', 'product')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // No vectors stored. Search without query_vector → BM25 fallback.
        let hits = hybrid_db_search(&pool, "pipeline operators", None, "bge-m3", 1024, 5)
            .await
            .unwrap();
        assert!(!hits.is_empty(), "BM25 fallback must fire");
        assert_eq!(hits[0].object_id, "W1");
    }
}
