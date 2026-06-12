pub mod backfill;
pub mod chunker;
pub mod db_uri;
pub mod embedding;
pub mod embedding_router;
pub mod gemini;
pub mod hybrid;
pub mod hybrid_db;
pub mod hybrid_rrf;
pub mod ingestion;
pub mod search;
pub mod vector_store;
pub mod worker;

// Local hybrid embedding lane (BGE-M3 dense + sqlite-vec). Heavy native deps
// (onnxruntime, sqlite-vec) live behind the `embedding` feature so the default
// build/tests are unchanged (R12: core retrieval stays tag+FTS5+graph).
#[cfg(feature = "embedding")]
pub mod bge;
#[cfg(feature = "embedding")]
pub mod vec_store_sqlite;

pub use backfill::{
    resolve_db_object_text, run_backfill, BackfillReport, DbObjectText, SourceReport,
    BACKFILL_SOURCES,
};
pub use chunker::{chunk_markdown, Chunk, ChunkMeta};
pub use db_uri::{db_uri, embed_checksum, parse_db_uri, DbObjectType, MAX_CHUNKS_PER_TURN};
pub use hybrid_db::{hybrid_db_search, DbHybridHit};
pub use embedding::{AsyncEmbeddingProvider, Embedding, EmbeddingProvider, NoOpEmbedder};
pub use embedding_router::{EmbeddingRole, EmbeddingRouter};
pub use gemini::{cosine, GeminiEmbedder, GEMINI_DIM, GEMINI_MODEL};
pub use hybrid::{hybrid_search, EmbeddedChunk};
pub use hybrid_rrf::{rrf_fuse, rrf_fuse_two, DEFAULT_RRF_K};
pub use ingestion::{
    fts_index_chunk, guard_document, ingest_file, ingest_text, ingest_url_content,
    IngestedDocument,
};
pub use search::{search_bm25, SearchHit, SearchIndex};
pub use vector_store::{
    register_model_dim, registered_dim, search_by_vector, vector_count, vector_exists,
    write_vector_guarded,
};
pub use worker::{EmbedderWorker, EmbedderWorkerConfig, QueueStats};

#[cfg(feature = "embedding")]
pub use bge::{Bge3Embedder, BGE_M3_DIM, BGE_M3_MODEL};
