//! Hybrid search: BM25 lexical + cosine vector similarity, fused via Reciprocal
//! Rank Fusion (RRF). When no embeddings are available, falls back to BM25.

use crate::{
    chunker::Chunk,
    embedding::Embedding,
    gemini::cosine,
    search::{search_bm25, SearchHit},
};

const RRF_K: f32 = 60.0;

#[derive(Debug, Clone)]
pub struct EmbeddedChunk {
    pub chunk: Chunk,
    pub embedding: Option<Embedding>,
}

/// Run hybrid search. Each chunk may have an embedding; the query embedding is
/// optional. If query_embedding is None, this is a BM25-only search.
pub fn hybrid_search(
    query: &str,
    query_embedding: Option<&Embedding>,
    chunks: &[EmbeddedChunk],
    limit: usize,
) -> Vec<SearchHit> {
    // BM25 ranking
    let raw_chunks: Vec<Chunk> = chunks.iter().map(|c| c.chunk.clone()).collect();
    let bm25_hits = search_bm25(query, &raw_chunks, limit * 4);

    // Vector ranking
    let vector_hits: Vec<(uuid::Uuid, f32)> = match query_embedding {
        Some(qe) => {
            let mut scored: Vec<_> = chunks
                .iter()
                .filter_map(|c| {
                    c.embedding
                        .as_ref()
                        .map(|e| (c.chunk.id, cosine(&qe.vector, &e.vector)))
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(limit * 4);
            scored
        }
        None => vec![],
    };

    // RRF fusion: score(d) = sum over rankings of 1/(k + rank)
    let mut fused: std::collections::HashMap<uuid::Uuid, f32> = std::collections::HashMap::new();
    for (rank, hit) in bm25_hits.iter().enumerate() {
        let s = 1.0 / (RRF_K + (rank + 1) as f32);
        *fused.entry(hit.chunk_id).or_insert(0.0) += s;
    }
    for (rank, (id, _score)) in vector_hits.iter().enumerate() {
        let s = 1.0 / (RRF_K + (rank + 1) as f32);
        *fused.entry(*id).or_insert(0.0) += s;
    }

    let mut combined: Vec<(uuid::Uuid, f32)> = fused.into_iter().collect();
    combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    combined.truncate(limit);

    combined
        .into_iter()
        .filter_map(|(id, score)| {
            let bm25_hit = bm25_hits.iter().find(|h| h.chunk_id == id);
            let chunk = chunks.iter().find(|c| c.chunk.id == id)?;
            Some(SearchHit {
                chunk_id: id,
                source_path: chunk.chunk.meta.source_path.clone(),
                heading_path: chunk.chunk.meta.heading_path.clone(),
                score,
                snippet: bm25_hit
                    .map(|h| h.snippet.clone())
                    .unwrap_or_else(|| chunk.chunk.text.chars().take(200).collect()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkMeta;

    fn make_chunk(id: u128, text: &str) -> Chunk {
        Chunk {
            id: uuid::Uuid::from_u128(id),
            text: text.to_string(),
            meta: ChunkMeta {
                source_path: None,
                heading_path: vec![],
                start_byte: 0,
                end_byte: text.len(),
            },
            checksum: "x".into(),
        }
    }

    fn make_embedded(id: u128, text: &str, vec: Vec<f32>) -> EmbeddedChunk {
        EmbeddedChunk {
            chunk: make_chunk(id, text),
            embedding: Some(Embedding {
                vector: vec,
                model: "test".to_string(),
            }),
        }
    }

    #[test]
    fn bm25_only_when_no_query_embedding() {
        let chunks = vec![
            make_embedded(1, "altevra agent operating system", vec![1.0, 0.0]),
            make_embedded(2, "random unrelated text", vec![0.0, 1.0]),
        ];
        let hits = hybrid_search("altevra agent", None, &chunks, 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].chunk_id, uuid::Uuid::from_u128(1));
    }

    #[test]
    fn fusion_includes_vector_neighbours() {
        let chunks = vec![
            make_embedded(1, "lexical match", vec![1.0, 0.0, 0.0]),
            make_embedded(2, "no lexical overlap here", vec![0.0, 1.0, 0.0]),
            make_embedded(3, "nothing else", vec![0.0, 0.0, 1.0]),
        ];
        let query_emb = Embedding {
            vector: vec![0.0, 1.0, 0.0],
            model: "test".into(),
        };
        let hits = hybrid_search("lexical", Some(&query_emb), &chunks, 5);
        // Both BM25 hit (chunk 1) and vector hit (chunk 2) should appear.
        let ids: std::collections::HashSet<_> = hits.iter().map(|h| h.chunk_id).collect();
        assert!(ids.contains(&uuid::Uuid::from_u128(1)));
        assert!(ids.contains(&uuid::Uuid::from_u128(2)));
    }

    #[test]
    fn limit_respected() {
        let chunks: Vec<_> = (0..10)
            .map(|i| make_embedded(i as u128, &format!("word{i}"), vec![1.0; 3]))
            .collect();
        let hits = hybrid_search("word", None, &chunks, 3);
        assert!(hits.len() <= 3);
    }

    #[test]
    fn empty_chunks_returns_empty() {
        let hits = hybrid_search("x", None, &[], 5);
        assert!(hits.is_empty());
    }
}
