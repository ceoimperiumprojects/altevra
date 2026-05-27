//! Persistent vector storage for memory_chunks. Uses the SQLite TEXT fallback
//! (JSON-encoded Vec<f32>) defined in migration 010. When the optional
//! `sqlite-vec` extension is wired up later, we will read from `vec0` virtual
//! tables instead — but the API here stays stable.

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::gemini::cosine;

/// Write (or replace) the vector for a chunk.
pub async fn write_vector(
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
/// For now this loads ALL vectors into memory. Fine up to ~100k chunks; beyond
/// that we'll need sqlite-vec or a HNSW index.
pub async fn search_by_vector(
    pool: &SqlitePool,
    query_vector: &[f32],
    limit: i64,
) -> anyhow::Result<Vec<(Uuid, f32)>> {
    let rows = sqlx::query("SELECT chunk_id, embedding FROM memory_chunk_vectors_v2")
        .fetch_all(pool)
        .await?;

    let mut scored: Vec<(Uuid, f32)> = rows
        .into_iter()
        .filter_map(|r| {
            let id_text: String = r.get("chunk_id");
            let vec_text: String = r.get("embedding");
            let id = Uuid::parse_str(&id_text).ok()?;
            let vec: Vec<f32> = serde_json::from_str(&vec_text).ok()?;
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

    async fn setup() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
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

    #[tokio::test]
    async fn write_then_count_then_search() {
        let pool = setup().await;
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        write_vector(&pool, id_a, "test", &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        write_vector(&pool, id_b, "test", &[0.0, 1.0, 0.0])
            .await
            .unwrap();

        assert_eq!(vector_count(&pool).await.unwrap(), 2);
        assert!(vector_exists(&pool, id_a).await.unwrap());

        let hits = search_by_vector(&pool, &[1.0, 0.1, 0.0], 5).await.unwrap();
        assert_eq!(hits[0].0, id_a);
        assert!(hits[0].1 > hits[1].1);
    }

    #[tokio::test]
    async fn upsert_replaces_existing_vector() {
        let pool = setup().await;
        let id = Uuid::new_v4();
        write_vector(&pool, id, "v1", &[1.0]).await.unwrap();
        write_vector(&pool, id, "v2", &[2.0]).await.unwrap();
        assert_eq!(vector_count(&pool).await.unwrap(), 1);
    }
}
