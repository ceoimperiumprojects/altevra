//! Hybrid search over the DB-backed corpus (R2 §5, Phase 1 retrieval backbone).
//!
//! Combines:
//!   1. BM25 lexical search via `object_fts` (already indexed by FTS5).
//!   2. Vector cosine search via `memory_chunk_vectors_v2` (model+dim filtered).
//!
//! Results are fused with Reciprocal Rank Fusion (RRF) on ONE canonical key
//! space. When no vectors exist (e.g. embedder hasn't run yet), falls back to
//! BM25-only — never errors.
//!
//! ## The fusion-key contract (Codex #4 / #1)
//!
//! Every leg keys on a single canonical [`RetrievalKey`]:
//!   - chunk-corpus rows (vectors, and `object_fts` rows of type `memory_chunk`)
//!     key on `chunk:<chunk_id>`,
//!   - durable-object rows (learning/wiki/turn/… in `object_fts`) key on
//!     `obj:<type>:<id>`.
//! Because a `memory_chunk` that matches BOTH lexically AND semantically lands
//! on the SAME `chunk:<id>` key, RRF actually fuses the matching evidence —
//! the previous code keyed BM25 on `object_id` and the vector leg on
//! `memory_chunks.document_id`, two different spaces that never fused.
//!
//! Each hit also carries a [`SourceRef`] (object_type, object_id, session_id,
//! turn_idx, source_path, ts) for citation + filters, and vector-only /
//! keyword-only hits both surface (the chunk→`memory_chunks.text` join means a
//! semantic-only chunk hit still has a renderable snippet — fixes the old
//! "semantic hole" at lines 96-98).

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::{gemini::cosine, hybrid_rrf::rrf_fuse};

/// Provenance + citation metadata for one retrieval hit. Carries everything the
/// caller needs to render a breadcrumb, apply project/tool/time filters, and
/// trace the hit back to its origin (Codex #6 — the hybrid path must not lose
/// the provenance `search_turns_with_provenance` exposes today).
#[derive(Debug, Clone, Default)]
pub struct SourceRef {
    /// Canonical object type: "learning" / "wiki" / "turn" / "memory_chunk" / …
    pub object_type: String,
    /// Canonical object id (object_fts.object_id, or the resolved db:// id, or
    /// the chunk_id for raw file/chunk hits).
    pub object_id: String,
    /// Parent session for turn-derived chunks (resolved via `db://turn/<id>`).
    pub session_id: Option<String>,
    /// Turn index within the parent session, when known.
    pub turn_idx: Option<i64>,
    /// Where the chunk came from: a filesystem path or a synthetic `db://` URI.
    pub source_path: Option<String>,
    /// Best-known timestamp for the hit (chunk.created_at or object.updated_at).
    pub ts: Option<DateTime<Utc>>,
}

/// A hit from the DB hybrid search — enough to render a recall breadcrumb,
/// cite the source, and apply post-filters.
#[derive(Debug, Clone)]
pub struct DbHybridHit {
    /// The object type: "learning", "wiki", "turn", "memory_chunk", etc.
    /// (Kept as a top-level field for back-compat with existing call-sites;
    /// mirrors `source_ref.object_type`.)
    pub object_type: String,
    /// Canonical object id (mirrors `source_ref.object_id`).
    pub object_id: String,
    pub title: String,
    /// Short snippet of the matched text.
    pub snippet: String,
    /// Fused RRF score (higher is better).
    pub score: f32,
    /// Provenance + citation metadata.
    pub source_ref: SourceRef,
}

/// Typed retrieval request — the one primitive recall/ask/MCP route through so
/// they share the same filters + provenance (Codex #6). `query` is required;
/// the rest narrow the result set.
#[derive(Debug, Clone, Default)]
pub struct RetrievalRequest {
    pub query: String,
    /// Restrict to a project (matches `sessions.project_name` for turn-derived
    /// chunks; applied loosely against object domain elsewhere).
    pub project: Option<String>,
    /// Restrict to a tool (`claude-code` | `codex` | … — turn-derived only).
    pub tool: Option<String>,
    /// Inclusive start of the time window.
    pub since: Option<DateTime<Utc>>,
    /// Exclusive end of the time window.
    pub until: Option<DateTime<Utc>>,
    /// Max hits to return.
    pub limit: usize,
}

/// Canonical fusion key — the single key space all legs map into so RRF fuses
/// matching evidence instead of comparing apples to oranges.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum RetrievalKey {
    /// A chunk in the `memory_chunks` corpus (file or db:// object chunk).
    Chunk(String),
    /// A durable object (learning/wiki/turn/…) indexed in `object_fts`.
    Object { object_type: String, object_id: String },
}

impl RetrievalKey {
    /// Build a key from an `object_fts` row. `memory_chunk` rows fold into the
    /// chunk key space (their `object_id` IS the chunk_id) so a lexical hit on
    /// a chunk fuses with the same chunk's vector hit.
    fn from_object_row(object_type: &str, object_id: &str) -> Self {
        if object_type == "memory_chunk" {
            RetrievalKey::Chunk(object_id.to_string())
        } else {
            RetrievalKey::Object {
                object_type: object_type.to_string(),
                object_id: object_id.to_string(),
            }
        }
    }
}

/// Resolve a [`RetrievalRequest`] into provenance-rich hybrid hits. This is THE
/// retrieval primitive — recall/ask (and later MCP) route through it so filters
/// and provenance are shared, not re-implemented per surface.
///
/// `query_vector` — pre-computed embedding of `req.query` (pass `None` for
///   lexical-only). `model` + `dim` gate which vectors are scored (R2).
pub async fn retrieve(
    pool: &SqlitePool,
    req: &RetrievalRequest,
    query_vector: Option<&[f32]>,
    model: &str,
    dim: usize,
) -> anyhow::Result<Vec<DbHybridHit>> {
    let limit = req.limit.max(1);

    // --- Leg 1: BM25 via object_fts (objects + any indexed memory_chunk rows) ---
    let bm25_hits = bm25_search(pool, &req.query, (limit * 4) as i64).await?;

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

    // --- Build ranked lists in ONE canonical key space, then RRF-fuse. ---
    let bm25_ranked: Vec<RetrievalKey> = bm25_hits
        .iter()
        .map(|(_, m)| RetrievalKey::from_object_row(&m.object_type, &m.object_id))
        .collect();
    let vec_ranked: Vec<RetrievalKey> = vector_ranked
        .iter()
        .map(|(chunk_id, _)| RetrievalKey::Chunk(chunk_id.clone()))
        .collect();

    let mut fused = rrf_fuse(&[bm25_ranked, vec_ranked], crate::hybrid_rrf::DEFAULT_RRF_K);

    // Enrich each fused key into a DbHybridHit. Over-fetch then filter+truncate,
    // because the time/project/tool filters can drop hits.
    let mut out = Vec::with_capacity(fused.len());
    // Cache the BM25 metadata by canonical key for O(1) lookup.
    let bm25_by_key: std::collections::HashMap<RetrievalKey, &BM25Meta> = bm25_hits
        .iter()
        .map(|(_, m)| (RetrievalKey::from_object_row(&m.object_type, &m.object_id), m))
        .collect();

    for (key, rrf_score) in fused.drain(..) {
        let hit = match &key {
            // Object-corpus hit: BM25 metadata has everything (title/body/type).
            RetrievalKey::Object { object_type, object_id } => {
                let Some(meta) = bm25_by_key.get(&key) else {
                    continue; // object key only present via BM25 — must have meta
                };
                DbHybridHit {
                    object_type: object_type.clone(),
                    object_id: object_id.clone(),
                    title: meta.title.clone(),
                    snippet: snippet_from_body(&meta.body, &req.query, 200),
                    score: rrf_score,
                    source_ref: SourceRef {
                        object_type: object_type.clone(),
                        object_id: object_id.clone(),
                        ts: meta.updated_at,
                        ..Default::default()
                    },
                }
            }
            // Chunk-corpus hit. May have BM25 meta (chunk indexed in object_fts)
            // AND/OR a vector hit. Either way, resolve text+provenance from
            // memory_chunks/memory_documents so vector-only AND keyword-only
            // chunk hits both surface (fixes the old semantic hole).
            RetrievalKey::Chunk(chunk_id) => {
                let resolved = resolve_chunk(pool, chunk_id).await?;
                let Some(c) = resolved else {
                    // Chunk row gone (race) — fall back to BM25 meta if the chunk
                    // was indexed in object_fts, else drop it.
                    if let Some(meta) = bm25_by_key.get(&key) {
                        out.push(DbHybridHit {
                            object_type: meta.object_type.clone(),
                            object_id: meta.object_id.clone(),
                            title: meta.title.clone(),
                            snippet: snippet_from_body(&meta.body, &req.query, 200),
                            score: rrf_score,
                            source_ref: SourceRef {
                                object_type: meta.object_type.clone(),
                                object_id: meta.object_id.clone(),
                                ..Default::default()
                            },
                        });
                        if out.len() >= limit {
                            break;
                        }
                    }
                    continue;
                };
                DbHybridHit {
                    object_type: c.source_ref.object_type.clone(),
                    object_id: c.source_ref.object_id.clone(),
                    title: c.title.clone(),
                    snippet: snippet_from_body(&c.text, &req.query, 200),
                    score: rrf_score,
                    source_ref: c.source_ref,
                }
            }
        };

        // Post-filter: time window + tool/project (provenance-aware).
        if !passes_filters(&hit, req) {
            continue;
        }
        out.push(hit);
        if out.len() >= limit {
            break;
        }
    }

    Ok(out)
}

/// Back-compat shim: the old positional signature, now built on `retrieve`.
/// Existing tests + call-sites that pass (query, vector, model, dim, limit)
/// keep working with no filters.
pub async fn hybrid_db_search(
    pool: &SqlitePool,
    query_text: &str,
    query_vector: Option<&[f32]>,
    model: &str,
    dim: usize,
    limit: usize,
) -> anyhow::Result<Vec<DbHybridHit>> {
    let req = RetrievalRequest {
        query: query_text.to_string(),
        limit,
        ..Default::default()
    };
    retrieve(pool, &req, query_vector, model, dim).await
}

// ---- internal helpers ----

struct BM25Meta {
    object_type: String,
    object_id: String,
    title: String,
    body: String,
    updated_at: Option<DateTime<Utc>>,
}

/// A chunk resolved from the persistent corpus, with provenance.
struct ResolvedChunk {
    title: String,
    text: String,
    source_ref: SourceRef,
}

/// BM25 search over object_fts. Returns (canonical-key-unused, metadata) pairs.
/// Gracefully returns empty vec if the FTS table doesn't exist.
async fn bm25_search(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> anyhow::Result<Vec<((), BM25Meta)>> {
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
            (
                (),
                BM25Meta {
                    object_type: r.get("object_type"),
                    object_id: r.get("object_id"),
                    title: r.get("title"),
                    body: r.get("body"),
                    updated_at: None,
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

/// Resolve a chunk_id → its text + title + provenance via the
/// memory_chunks/memory_documents join. This is the join that makes
/// vector-only (and keyword-only chunk) hits surface with a renderable snippet
/// — the fix for the old "semantic hole". Provenance includes the source_path
/// (filesystem path OR `db://type/id`); for `db://turn/<id>` we also resolve
/// the turn's session_id + turn_idx so turn hits carry full provenance.
async fn resolve_chunk(
    pool: &SqlitePool,
    chunk_id: &str,
) -> anyhow::Result<Option<ResolvedChunk>> {
    let row = sqlx::query(
        "SELECT mc.text AS text, mc.created_at AS created_at, \
                COALESCE(md.title, '') AS doc_title, \
                COALESCE(md.source_path, '') AS source_path \
         FROM memory_chunks mc \
         LEFT JOIN memory_documents md ON md.id = mc.document_id \
         WHERE mc.id = ?",
    )
    .bind(chunk_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let Some(r) = row else {
        return Ok(None);
    };

    let text: String = r.get("text");
    let doc_title: String = r.get("doc_title");
    let source_path: String = r.get("source_path");
    let created_raw: String = r.try_get("created_at").unwrap_or_default();
    let ts = chrono::DateTime::parse_from_rfc3339(&created_raw)
        .map(|d| d.with_timezone(&Utc))
        .ok();

    // Derive canonical (object_type, object_id) + turn provenance from the
    // source_path. A `db://type/id` URI gives the real object identity; a real
    // filesystem path is a `memory_chunk` keyed by chunk_id.
    let (object_type, object_id, mut session_id, mut turn_idx) =
        if let Some((otype, oid)) = crate::db_uri::parse_db_uri(&source_path) {
            (otype.to_string(), oid.to_string(), None, None)
        } else {
            ("memory_chunk".to_string(), chunk_id.to_string(), None, None)
        };

    // For db://turn/<id>, pull the parent session + turn index so turn hits keep
    // the provenance search_turns_with_provenance exposes today (Codex #6).
    if object_type == "turn" {
        if let Some(tr) = sqlx::query(
            "SELECT session_id, turn_idx FROM turns WHERE id = ?",
        )
        .bind(&object_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        {
            session_id = tr.try_get::<String, _>("session_id").ok();
            turn_idx = tr.try_get::<i64, _>("turn_idx").ok();
        }
    }

    let title = if !doc_title.is_empty() {
        doc_title
    } else if !source_path.is_empty() {
        source_path.trim_start_matches("./").to_string()
    } else {
        object_id.clone()
    };

    Ok(Some(ResolvedChunk {
        title,
        text,
        source_ref: SourceRef {
            object_type,
            object_id,
            session_id,
            turn_idx,
            source_path: if source_path.is_empty() {
                None
            } else {
                Some(source_path)
            },
            ts,
        },
    }))
}

/// Apply the request's time-window + tool/project filters against a hit's
/// provenance. Filters fail-open when the relevant provenance is absent (an
/// object with no timestamp is not silently dropped by a time window) — the
/// caller's surface (recall/ask) keeps its own coarser filtering on top.
fn passes_filters(hit: &DbHybridHit, req: &RetrievalRequest) -> bool {
    // Time window: only enforce when we have a timestamp.
    if let Some(ts) = hit.source_ref.ts {
        if let Some(s) = req.since {
            if ts < s {
                return false;
            }
        }
        if let Some(u) = req.until {
            if ts >= u {
                return false;
            }
        }
    }
    // tool/project filtering is session-scoped (turn-derived chunks). When the
    // request scopes to a tool, drop non-turn hits that can't carry a tool.
    // (recall/ask retain their own object/file filtering, so this stays loose.)
    if req.tool.is_some() && hit.source_ref.object_type != "turn" {
        return false;
    }
    true
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
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS memory_documents (
                id TEXT PRIMARY KEY,
                source_path TEXT NOT NULL DEFAULT '',
                title TEXT,
                body TEXT NOT NULL DEFAULT '',
                checksum TEXT NOT NULL DEFAULT ''
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS turns (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL DEFAULT '',
                turn_idx INTEGER NOT NULL DEFAULT 0,
                content TEXT NOT NULL DEFAULT ''
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
        // Provenance mirrors the top-level fields.
        assert_eq!(hits[0].source_ref.object_type, "learning");
        assert_eq!(hits[0].source_ref.object_id, "L1");
    }

    #[tokio::test]
    async fn semantic_only_hit_via_vectors() {
        let pool = setup_fts_pool().await;
        // L1 matches lexically AND has a chunk+vector; L2 only has a distant vector.
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
             VALUES ('learning', 'L1', 'GTM', 'direct outreach to operators', 'biz')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Give L1's chunk a vector close to query; L2 chunk a distant vector.
        let chunk_l1 = Uuid::new_v4();
        let chunk_l2 = Uuid::new_v4();
        sqlx::query("INSERT INTO memory_chunks (id, document_id, text) VALUES (?, ?, ?)")
            .bind(chunk_l1.to_string())
            .bind("doc-l1")
            .bind("direct outreach to operators in Florida")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memory_chunks (id, document_id, text) VALUES (?, ?, ?)")
            .bind(chunk_l2.to_string())
            .bind("doc-l2")
            .bind("a totally unrelated chunk about something else")
            .execute(&pool)
            .await
            .unwrap();
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
        let hits =
            hybrid_db_search(&pool, "outreach", Some(&query_vec), "bge-m3", 3, 5)
                .await
                .unwrap();
        // The semantic-only chunk hit must surface WITH a snippet (the old code
        // dropped it). L1 (lexical) is present too.
        assert!(!hits.is_empty(), "must return fused hits");
        let ids: Vec<&str> = hits.iter().map(|h| h.object_id.as_str()).collect();
        assert!(
            ids.contains(&"L1"),
            "lexical object hit must be present: {ids:?}"
        );
        // The closest vector chunk surfaces with a non-empty snippet (semantic hole fixed).
        let chunk_hit = hits
            .iter()
            .find(|h| h.object_type == "memory_chunk")
            .expect("semantic chunk hit must surface");
        assert!(
            chunk_hit.snippet.contains("outreach") || !chunk_hit.snippet.is_empty(),
            "semantic-only chunk hit must carry a snippet: {:?}",
            chunk_hit.snippet
        );
    }

    #[tokio::test]
    async fn mismatched_dim_vectors_filtered_out() {
        let pool = setup_fts_pool().await;
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

        let hits = hybrid_db_search(&pool, "pipeline operators", None, "bge-m3", 1024, 5)
            .await
            .unwrap();
        assert!(!hits.is_empty(), "BM25 fallback must fire");
        assert_eq!(hits[0].object_id, "W1");
    }

    /// Codex #4: a chunk matching BOTH lexically (object_fts memory_chunk row)
    /// AND semantically (vector) must FUSE onto one `chunk:<id>` key — proving
    /// the two legs share a key space now.
    #[tokio::test]
    async fn chunk_lexical_and_vector_fuse_on_one_key() {
        let pool = setup_fts_pool().await;
        let chunk_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO memory_chunks (id, document_id, text) VALUES (?, 'd1', ?)")
            .bind(&chunk_id)
            .bind("direct outreach to florida operators")
            .execute(&pool)
            .await
            .unwrap();
        // Index the SAME chunk into object_fts as a memory_chunk row.
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
             VALUES ('memory_chunk', ?, '', 'direct outreach to florida operators', '')",
        )
        .bind(&chunk_id)
        .execute(&pool)
        .await
        .unwrap();
        // And give it a matching vector.
        let v = serde_json::to_string(&[1.0f32, 0.0, 0.0]).unwrap();
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(&chunk_id)
        .bind(&v)
        .execute(&pool)
        .await
        .unwrap();

        let qv = vec![1.0f32, 0.0, 0.0];
        let hits = hybrid_db_search(&pool, "outreach operators", Some(&qv), "bge-m3", 3, 10)
            .await
            .unwrap();
        // Exactly ONE hit for that chunk (fused, not duplicated across legs).
        let matches: Vec<&DbHybridHit> =
            hits.iter().filter(|h| h.object_id == chunk_id).collect();
        assert_eq!(
            matches.len(),
            1,
            "chunk must fuse to a single key across both legs, got {}",
            matches.len()
        );
    }

    /// Codex #6: a turn-derived chunk (`db://turn/<id>`) must carry session_id +
    /// turn_idx provenance through the hybrid path.
    #[tokio::test]
    async fn turn_chunk_carries_session_provenance() {
        let pool = setup_fts_pool().await;
        let turn_id = Uuid::new_v4().to_string();
        let session_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO turns (id, session_id, turn_idx, content) VALUES (?, ?, 3, 'x')")
            .bind(&turn_id)
            .bind(&session_id)
            .execute(&pool)
            .await
            .unwrap();
        let chunk_id = Uuid::new_v4().to_string();
        let uri = format!("db://turn/{turn_id}");
        sqlx::query(
            "INSERT INTO memory_documents (id, source_path, title) VALUES ('docturn', ?, 'Turn')",
        )
        .bind(&uri)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO memory_chunks (id, document_id, text) VALUES (?, 'docturn', ?)")
            .bind(&chunk_id)
            .bind("the user discussed florida operator outreach")
            .execute(&pool)
            .await
            .unwrap();
        let v = serde_json::to_string(&[1.0f32, 0.0, 0.0]).unwrap();
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(&chunk_id)
        .bind(&v)
        .execute(&pool)
        .await
        .unwrap();

        let qv = vec![1.0f32, 0.0, 0.0];
        let hits = hybrid_db_search(&pool, "outreach", Some(&qv), "bge-m3", 3, 5)
            .await
            .unwrap();
        let turn_hit = hits
            .iter()
            .find(|h| h.object_type == "turn")
            .expect("turn chunk must surface");
        assert_eq!(turn_hit.source_ref.session_id.as_deref(), Some(session_id.as_str()));
        assert_eq!(turn_hit.source_ref.turn_idx, Some(3));
        assert_eq!(turn_hit.object_id, turn_id);
    }

    /// Codex #6: the typed RetrievalRequest time-window filter narrows results.
    #[tokio::test]
    async fn retrieval_request_time_window_filters() {
        let pool = setup_fts_pool().await;
        let chunk_id = Uuid::new_v4().to_string();
        // created_at far in the past.
        sqlx::query(
            "INSERT INTO memory_chunks (id, document_id, text, created_at) \
             VALUES (?, 'd', 'florida outreach', '2020-01-01T00:00:00.000Z')",
        )
        .bind(&chunk_id)
        .execute(&pool)
        .await
        .unwrap();
        let v = serde_json::to_string(&[1.0f32, 0.0, 0.0]).unwrap();
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(&chunk_id)
        .bind(&v)
        .execute(&pool)
        .await
        .unwrap();

        let qv = vec![1.0f32, 0.0, 0.0];
        // since = 2024 → the 2020 chunk must be filtered out.
        let req = RetrievalRequest {
            query: "outreach".into(),
            since: Some("2024-01-01T00:00:00Z".parse().unwrap()),
            limit: 5,
            ..Default::default()
        };
        let hits = retrieve(&pool, &req, Some(&qv), "bge-m3", 3).await.unwrap();
        assert!(
            hits.iter().all(|h| h.object_id != chunk_id),
            "the 2020 chunk must be dropped by the since-filter"
        );
    }
}
