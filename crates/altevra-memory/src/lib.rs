pub mod chunker;
pub mod embedding;
pub mod gemini;
pub mod hybrid;
pub mod ingestion;
pub mod search;
pub mod vector_store;
pub mod worker;

pub use chunker::{chunk_markdown, Chunk, ChunkMeta};
pub use embedding::{AsyncEmbeddingProvider, Embedding, EmbeddingProvider, NoOpEmbedder};
pub use gemini::{cosine, GeminiEmbedder, GEMINI_DIM, GEMINI_MODEL};
pub use hybrid::{hybrid_search, EmbeddedChunk};
pub use ingestion::{ingest_file, ingest_text, ingest_url_content, IngestedDocument};
pub use search::{search_bm25, SearchHit, SearchIndex};
pub use vector_store::{search_by_vector, vector_count, vector_exists, write_vector};
pub use worker::{EmbedderWorker, EmbedderWorkerConfig, QueueStats};
