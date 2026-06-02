//! Dense vector store backed by `sqlite-vec` (the C extension statically linked via
//! `rusqlite` + `cc`) — keeps Altevra SINGLE-BINARY and LOCAL-FIRST (no separate vector
//! service). Brute-force KNN, which is more than enough at second-brain scale
//! (~75ms @ 100k × 768d). Vectors are passed as JSON text (no extra zerocopy dep).
//!
//! A dedicated rusqlite connection is used (not the sqlx pool): sqlx cannot register
//! the statically-linked extension via `sqlite3_auto_extension`. This store is the
//! opt-in hybrid layer's dense half; FTS5 (in altevra-db) is the lexical half, fused
//! by `hybrid_rrf`.

use rusqlite::{ffi::sqlite3_auto_extension, params, Connection};
use sqlite_vec::sqlite3_vec_init;
use std::sync::Mutex;

pub struct SqliteVecStore {
    conn: Mutex<Connection>,
    dim: usize,
}

impl SqliteVecStore {
    fn register_extension() {
        // Register the statically-linked sqlite-vec extension before opening a conn.
        unsafe {
            sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(),
            >(sqlite3_vec_init as *const ())));
        }
    }

    fn init(conn: &Connection, dim: usize) -> anyhow::Result<()> {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS object_vec USING vec0(\
             object_id TEXT PRIMARY KEY, embedding float[{dim}]);"
        ))?;
        Ok(())
    }

    pub fn open(path: &str, dim: usize) -> anyhow::Result<Self> {
        Self::register_extension();
        let conn = Connection::open(path)?;
        Self::init(&conn, dim)?;
        Ok(Self {
            conn: Mutex::new(conn),
            dim,
        })
    }

    pub fn open_in_memory(dim: usize) -> anyhow::Result<Self> {
        Self::register_extension();
        let conn = Connection::open_in_memory()?;
        Self::init(&conn, dim)?;
        Ok(Self {
            conn: Mutex::new(conn),
            dim,
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Insert or replace an object's dense vector.
    pub fn upsert(&self, object_id: &str, vector: &[f32]) -> anyhow::Result<()> {
        if vector.len() != self.dim {
            anyhow::bail!(
                "vector dim {} != store dim {} for object {object_id}",
                vector.len(),
                self.dim
            );
        }
        let json = serde_json::to_string(vector)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vec store mutex poisoned"))?;
        // vec0 virtual tables don't support UPSERT; delete-then-insert is the idiom.
        conn.execute("DELETE FROM object_vec WHERE object_id = ?1", params![object_id])?;
        conn.execute(
            "INSERT INTO object_vec(object_id, embedding) VALUES (?1, ?2)",
            params![object_id, json],
        )?;
        Ok(())
    }

    /// k-nearest neighbours of `query` by distance (ascending). Returns (object_id, distance).
    pub fn knn(&self, query: &[f32], k: usize) -> anyhow::Result<Vec<(String, f32)>> {
        let json = serde_json::to_string(query)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vec store mutex poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT object_id, distance FROM object_vec \
             WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![json, k as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)? as f32))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn count(&self) -> anyhow::Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vec store mutex poisoned"))?;
        let n: i64 = conn.query_row("SELECT count(*) FROM object_vec", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises the real statically-linked sqlite-vec extension. Gated #[ignore] so the
    // default `cargo test --features embedding` stays fast; run explicitly to verify.
    #[test]
    #[ignore = "loads the sqlite-vec C extension; run manually with --ignored"]
    fn upsert_then_knn_roundtrip() {
        let store = SqliteVecStore::open_in_memory(3).unwrap();
        store.upsert("a", &[1.0, 0.0, 0.0]).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0]).unwrap();
        store.upsert("c", &[0.9, 0.1, 0.0]).unwrap();
        assert_eq!(store.count().unwrap(), 3);
        let hits = store.knn(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits.len(), 2);
        // nearest to [1,0,0] is "a", then "c".
        assert_eq!(hits[0].0, "a");
        assert_eq!(hits[1].0, "c");
    }

    #[test]
    #[ignore = "loads the sqlite-vec C extension; run manually with --ignored"]
    fn dim_mismatch_errors() {
        let store = SqliteVecStore::open_in_memory(4).unwrap();
        assert!(store.upsert("x", &[1.0, 2.0]).is_err());
    }
}
