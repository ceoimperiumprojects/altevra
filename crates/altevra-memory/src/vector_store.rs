//! Persistent vector storage for memory_chunks. Uses the SQLite TEXT fallback
//! (JSON-encoded Vec<f32>) defined in migration 010. When the optional
//! `sqlite-vec` extension is wired up later, we will read from `vec0` virtual
//! tables instead — but the API here stays stable.
//!
//! R2 dim-gates: `memory_chunk_vectors_v2` carries `model` + `dim`, and the
//! `embed_meta` table (migration 041) records the registered dim per model.
//! - **Write-gate** — [`write_vector_guarded`] refuses any vector whose dim
//!   differs from the registered model dim (foreign-dim vectors never land).
//! - **Query-gate** — [`search_by_vector`] filters by `model + dim` and never
//!   cosine-scores across mixed dims (the pre-R2 version scored blindly).

use anyhow::bail;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::gemini::cosine;

/// Register the active embedding model + dim in `embed_meta` (records once).
///
/// Idempotent for the same `(model, dim)` pair. Refuses to re-register an
/// existing model under a DIFFERENT dim — that would silently mix vector
/// spaces; switching dims requires an explicit re-embed migration instead.
pub async fn register_model_dim(pool: &SqlitePool, model: &str, dim: usize) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO embed_meta (model, dim) VALUES (?, ?) ON CONFLICT(model) DO NOTHING")
        .bind(model)
        .bind(dim as i64)
        .execute(pool)
        .await?;
    let stored: i64 = sqlx::query_scalar("SELECT dim FROM embed_meta WHERE model = ?")
        .bind(model)
        .fetch_one(pool)
        .await?;
    if stored != dim as i64 {
        bail!(
            "embed_meta: model '{model}' is already registered with dim {stored}; \
             refusing to re-register with dim {dim} (one model, one dim)"
        );
    }
    Ok(())
}

/// Look up the registered dim for `model` in `embed_meta`. `None` when the
/// model was never registered.
pub async fn registered_dim(pool: &SqlitePool, model: &str) -> anyhow::Result<Option<i64>> {
    let dim: Option<i64> = sqlx::query_scalar("SELECT dim FROM embed_meta WHERE model = ?")
        .bind(model)
        .fetch_optional(pool)
        .await?;
    Ok(dim)
}

/// Write-gate (R2): write a vector ONLY when its dim matches the dim that
/// [`register_model_dim`] recorded for `model`. Unregistered models and
/// foreign dims are refused — nothing lands in `memory_chunk_vectors_v2`.
pub async fn write_vector_guarded(
    pool: &SqlitePool,
    chunk_id: Uuid,
    model: &str,
    vector: &[f32],
) -> anyhow::Result<()> {
    let Some(dim) = registered_dim(pool, model).await? else {
        bail!(
            "write-gate: model '{model}' is not registered in embed_meta — \
             call register_model_dim(model, dim) before writing vectors"
        );
    };
    if vector.len() as i64 != dim {
        bail!(
            "write-gate: refusing dim-{} vector for model '{model}' (registered dim {dim})",
            vector.len()
        );
    }
    write_vector_unchecked(pool, chunk_id, model, vector).await
}

/// Raw upsert, no gate. Private on purpose: every external caller goes
/// through [`write_vector_guarded`] so foreign dims can never land.
async fn write_vector_unchecked(
    pool: &SqlitePool,
    chunk_id: Uuid,
    model: &str,
    vector: &[f32],
) -> anyhow::Result<()> {
    let json = serde_json::to_string(vector)?;
    sqlx::query(
        r#"INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding)
           VALUES (?, ?, ?, ?)
           ON CONFLICT (chunk_id) DO UPDATE SET
             model = excluded.model,
             dim = excluded.dim,
             embedding = excluded.embedding,
             created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')"#,
    )
    .bind(chunk_id.to_string())
    .bind(model)
    .bind(vector.len() as i64)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Return chunk_ids ordered by cosine similarity to `query_vector`, top `limit`.
///
/// Query-gate (R2): only rows whose `model` matches AND whose `dim` equals
/// `query_vector.len()` are scored. Mixed-dim or foreign-model rows are
/// excluded in SQL, plus a defensive per-row length check after JSON decode —
/// cosine across mismatched dims never happens.
///
/// For now this loads the matching vectors into memory. Fine up to ~100k
/// chunks; beyond that we'll need sqlite-vec or a HNSW index.
pub async fn search_by_vector(
    pool: &SqlitePool,
    query_vector: &[f32],
    model: &str,
    limit: i64,
) -> anyhow::Result<Vec<(Uuid, f32)>> {
    let rows = sqlx::query(
        "SELECT chunk_id, embedding FROM memory_chunk_vectors_v2 WHERE model = ? AND dim = ?",
    )
    .bind(model)
    .bind(query_vector.len() as i64)
    .fetch_all(pool)
    .await?;

    let mut scored: Vec<(Uuid, f32)> = rows
        .into_iter()
        .filter_map(|r| {
            let id_text: String = r.get("chunk_id");
            let vec_text: String = r.get("embedding");
            let id = Uuid::parse_str(&id_text).ok()?;
            let vec: Vec<f32> = serde_json::from_str(&vec_text).ok()?;
            if vec.len() != query_vector.len() {
                return None; // defensive: stored dim column lied
            }
            let score = cosine(query_vector, &vec);
            Some((id, score))
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit as usize);
    Ok(scored)
}

/// Check whether a chunk already has a vector.
pub async fn vector_exists(pool: &SqlitePool, chunk_id: Uuid) -> anyhow::Result<bool> {
    let row = sqlx::query("SELECT 1 AS x FROM memory_chunk_vectors_v2 WHERE chunk_id = ?")
        .bind(chunk_id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Count of chunks with vectors stored.
pub async fn vector_count(pool: &SqlitePool) -> anyhow::Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM memory_chunk_vectors_v2")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::TempDir;

    /// Per-test TempDir-backed DB with the v2 vector table + embed_meta (041).
    async fn setup() -> (TempDir, SqlitePool) {
        let tmp = TempDir::new().unwrap();
        let url = format!("sqlite://{}?mode=rwc", tmp.path().join("test.db").display());
        let pool = SqlitePoolOptions::new().connect(&url).await.unwrap();
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
        sqlx::query(
            r#"CREATE TABLE embed_meta (
                model TEXT PRIMARY KEY,
                dim INTEGER NOT NULL,
                set_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        (tmp, pool)
    }

    /// Insert a row bypassing the write-gate (simulating pre-R2 / foreign data).
    async fn insert_raw(pool: &SqlitePool, chunk_id: Uuid, model: &str, vector: &[f32]) {
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, ?, ?, ?)",
        )
        .bind(chunk_id.to_string())
        .bind(model)
        .bind(vector.len() as i64)
        .bind(serde_json::to_string(vector).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn write_read_back_at_registered_dim() {
        let (_tmp, pool) = setup().await;
        register_model_dim(&pool, "bge-m3", 3).await.unwrap();

        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        write_vector_guarded(&pool, id_a, "bge-m3", &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        write_vector_guarded(&pool, id_b, "bge-m3", &[0.0, 1.0, 0.0])
            .await
            .unwrap();

        assert_eq!(vector_count(&pool).await.unwrap(), 2);
        assert!(vector_exists(&pool, id_a).await.unwrap());

        let hits = search_by_vector(&pool, &[1.0, 0.1, 0.0], "bge-m3", 5)
            .await
            .unwrap();
        assert_eq!(hits[0].0, id_a);
        assert!(hits[0].1 > hits[1].1);
    }

    #[tokio::test]
    async fn write_gate_refuses_foreign_dim() {
        let (_tmp, pool) = setup().await;
        register_model_dim(&pool, "bge-m3", 3).await.unwrap();

        let err = write_vector_guarded(&pool, Uuid::new_v4(), "bge-m3", &[1.0, 2.0])
            .await
            .expect_err("dim-2 vector must be refused at the registered dim 3");
        assert!(err.to_string().contains("write-gate"), "got: {err}");
        assert_eq!(vector_count(&pool).await.unwrap(), 0, "nothing may land");
    }

    #[tokio::test]
    async fn write_gate_refuses_unregistered_model() {
        let (_tmp, pool) = setup().await;
        let err = write_vector_guarded(&pool, Uuid::new_v4(), "ghost-model", &[1.0])
            .await
            .expect_err("unregistered model must be refused");
        assert!(err.to_string().contains("not registered"), "got: {err}");
        assert_eq!(vector_count(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn register_model_dim_records_once_and_refuses_dim_change() {
        let (_tmp, pool) = setup().await;
        register_model_dim(&pool, "bge-m3", 1024).await.unwrap();
        // Same pair again — idempotent.
        register_model_dim(&pool, "bge-m3", 1024).await.unwrap();
        // Different dim for the same model — refused.
        let err = register_model_dim(&pool, "bge-m3", 768)
            .await
            .expect_err("dim change must be refused");
        assert!(err.to_string().contains("already registered"), "got: {err}");
        assert_eq!(registered_dim(&pool, "bge-m3").await.unwrap(), Some(1024));
    }

    #[tokio::test]
    async fn query_gate_filters_model_and_dim_never_scores_mixed() {
        let (_tmp, pool) = setup().await;
        // Three rows: the matching space, a foreign model, a foreign dim.
        let id_match = Uuid::new_v4();
        let id_other_model = Uuid::new_v4();
        let id_other_dim = Uuid::new_v4();
        insert_raw(&pool, id_match, "bge-m3", &[1.0, 0.0, 0.0]).await;
        insert_raw(&pool, id_other_model, "gemini", &[1.0, 0.0, 0.0]).await;
        // Same model but a different dim (legacy/corrupt row): must never be
        // cosine-scored against a dim-3 query.
        insert_raw(&pool, id_other_dim, "bge-m3", &[1.0, 0.0, 0.0, 0.0, 0.0]).await;

        let hits = search_by_vector(&pool, &[1.0, 0.0, 0.0], "bge-m3", 10)
            .await
            .unwrap();
        let ids: Vec<Uuid> = hits.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![id_match], "only model+dim matches may score: {ids:?}");
    }

    #[tokio::test]
    async fn query_gate_defensive_check_skips_lying_dim_column() {
        let (_tmp, pool) = setup().await;
        // dim column says 3, but the stored payload is dim 2 — must be skipped.
        sqlx::query(
            "INSERT INTO memory_chunk_vectors_v2 (chunk_id, model, dim, embedding) VALUES (?, 'bge-m3', 3, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(serde_json::to_string(&[1.0f32, 0.0]).unwrap())
        .execute(&pool)
        .await
        .unwrap();

        let hits = search_by_vector(&pool, &[1.0, 0.0, 0.0], "bge-m3", 10)
            .await
            .unwrap();
        assert!(hits.is_empty(), "payload-dim mismatch must not score");
    }

    #[tokio::test]
    async fn upsert_replaces_existing_vector() {
        let (_tmp, pool) = setup().await;
        register_model_dim(&pool, "v1", 1).await.unwrap();
        let id = Uuid::new_v4();
        write_vector_guarded(&pool, id, "v1", &[1.0]).await.unwrap();
        write_vector_guarded(&pool, id, "v1", &[2.0]).await.unwrap();
        assert_eq!(vector_count(&pool).await.unwrap(), 1);
    }
}
