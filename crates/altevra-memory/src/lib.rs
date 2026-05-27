pub mod chunker;
pub mod embedding;
pub mod ingestion;
pub mod search;

pub use chunker::{chunk_markdown, Chunk, ChunkMeta};
pub use embedding::{Embedding, EmbeddingProvider, NoOpEmbedder};
pub use ingestion::{ingest_file, ingest_text, IngestedDocument};
pub use search::{search_bm25, SearchHit, SearchIndex};
