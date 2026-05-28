//! Lightweight in-memory BM25 search over markdown chunks.
//!
//! No external indexer (no tantivy). Tokens are lowercase whitespace-split
//! with punctuation stripped, and a tiny English stopword list is filtered.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::chunker::Chunk;
use crate::ingestion::IngestedDocument;

const BM25_K1: f32 = 1.5;
const BM25_B: f32 = 0.75;
const SNIPPET_WINDOW: usize = 200;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "in", "on", "at", "of", "to", "for", "and", "or",
    "but", "with",
];

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

/// Tokenize a string: lowercase, replace non-alphanumeric with whitespace,
/// drop short tokens and stopwords.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                current.push(lc);
            }
        } else if !current.is_empty() {
            push_token(&mut out, std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        push_token(&mut out, current);
    }
    out
}

fn push_token(out: &mut Vec<String>, t: String) {
    if t.len() < 2 {
        return;
    }
    if is_stopword(&t) {
        return;
    }
    out.push(t);
}

/// A search result pointing back at a specific chunk.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub chunk_id: uuid::Uuid,
    pub source_path: Option<PathBuf>,
    pub heading_path: Vec<String>,
    pub score: f32,
    pub snippet: String,
}

/// In-memory chunk store with BM25 statistics maintained incrementally.
#[derive(Debug, Default)]
pub struct SearchIndex {
    chunks: Vec<Chunk>,
    /// Per-document token counts (parallel to `chunks`).
    doc_tokens: Vec<HashMap<String, u32>>,
    /// Per-document length (total tokens including duplicates).
    doc_lengths: Vec<u32>,
    /// Document frequency per term across the corpus.
    doc_freq: HashMap<String, u32>,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Insert a single chunk and update BM25 statistics.
    pub fn add(&mut self, chunk: Chunk) {
        let tokens = tokenize(&chunk.text);
        let length = tokens.len() as u32;
        let mut tf: HashMap<String, u32> = HashMap::new();
        for tok in tokens {
            *tf.entry(tok).or_insert(0) += 1;
        }
        for term in tf.keys() {
            *self.doc_freq.entry(term.clone()).or_insert(0) += 1;
        }
        self.doc_tokens.push(tf);
        self.doc_lengths.push(length);
        self.chunks.push(chunk);
    }

    /// Insert every chunk from an ingested document.
    pub fn add_document(&mut self, doc: IngestedDocument) {
        for chunk in doc.chunks {
            self.add(chunk);
        }
    }

    fn avg_doc_length(&self) -> f32 {
        if self.doc_lengths.is_empty() {
            return 0.0;
        }
        let total: u64 = self.doc_lengths.iter().map(|x| *x as u64).sum();
        total as f32 / self.doc_lengths.len() as f32
    }

    /// Rank chunks against `query` using BM25 and return up to `limit` hits.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        if self.chunks.is_empty() || limit == 0 {
            return Vec::new();
        }
        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return Vec::new();
        }
        let n = self.chunks.len() as f32;
        let avgdl = self.avg_doc_length().max(1.0);

        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(self.chunks.len());
        for (idx, tf) in self.doc_tokens.iter().enumerate() {
            let dl = self.doc_lengths[idx] as f32;
            if dl == 0.0 {
                continue;
            }
            let mut score = 0.0f32;
            for term in &query_terms {
                let Some(&f) = tf.get(term) else { continue };
                let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
                if df == 0.0 {
                    continue;
                }
                // BM25 IDF with the +1 smoothing variant (always positive).
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                let f = f as f32;
                let numerator = f * (BM25_K1 + 1.0);
                let denominator = f + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avgdl);
                score += idf * (numerator / denominator);
            }
            if score > 0.0 {
                scored.push((idx, score));
            }
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        scored
            .into_iter()
            .map(|(idx, score)| {
                let chunk = &self.chunks[idx];
                let snippet = make_snippet(&chunk.text, &query_terms);
                SearchHit {
                    chunk_id: chunk.id,
                    source_path: chunk.meta.source_path.clone(),
                    heading_path: chunk.meta.heading_path.clone(),
                    score,
                    snippet,
                }
            })
            .collect()
    }
}

/// Build a small ad-hoc index, run a query, and return hits. Convenience
/// wrapper around [`SearchIndex`] for one-shot searches.
pub fn search_bm25(query: &str, chunks: &[Chunk], limit: usize) -> Vec<SearchHit> {
    let mut idx = SearchIndex::new();
    for chunk in chunks {
        idx.add(chunk.clone());
    }
    idx.search(query, limit)
}

/// Produce a ~200 character snippet around the first matched query term.
fn make_snippet(text: &str, query_terms: &[String]) -> String {
    let lower = text.to_lowercase();
    let mut best_pos: Option<usize> = None;
    for term in query_terms {
        if let Some(pos) = lower.find(term.as_str()) {
            best_pos = Some(match best_pos {
                Some(existing) if existing < pos => existing,
                _ => pos,
            });
        }
    }
    let center = best_pos.unwrap_or(0);
    let half = SNIPPET_WINDOW / 2;
    let start = center.saturating_sub(half);
    let end = (center + half).min(text.len());
    // Snap to char boundaries.
    let start = snap_left(text, start);
    let end = snap_right(text, end);
    let mut snippet = String::new();
    if start > 0 {
        snippet.push_str("...");
    }
    snippet.push_str(&text[start..end]);
    if end < text.len() {
        snippet.push_str("...");
    }
    // Collapse whitespace runs for compactness.
    snippet.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn snap_left(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn snap_right(s: &str, mut idx: usize) -> usize {
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkMeta;
    use crate::ingestion::ingest_text;

    fn mk_chunk(text: &str) -> Chunk {
        let meta = ChunkMeta {
            source_path: None,
            heading_path: Vec::new(),
            start_byte: 0,
            end_byte: text.len(),
        };
        // Build via public API path (re-use Chunk constructor through chunker?
        // It is private — so we go through chunk_markdown for fidelity).
        let chunks = crate::chunker::chunk_markdown(text, None, 100_000);
        if let Some(c) = chunks.into_iter().next() {
            Chunk {
                id: c.id,
                text: c.text,
                meta,
                checksum: c.checksum,
            }
        } else {
            // Fallback for unusual input.
            panic!("test helper expected at least one chunk for input: {text}");
        }
    }

    #[test]
    fn empty_index_returns_no_hits() {
        let idx = SearchIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        let hits = idx.search("anything", 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn no_match_returns_empty_hits() {
        let mut idx = SearchIndex::new();
        idx.add(mk_chunk("Rust is a memory safe systems language."));
        let hits = idx.search("javascript", 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn single_term_ranks_relevant_chunk_higher() {
        let mut idx = SearchIndex::new();
        idx.add(mk_chunk("Cooking pasta takes water salt and time."));
        idx.add(mk_chunk(
            "Rust ownership ownership ownership and lifetimes are core concepts.",
        ));
        let hits = idx.search("ownership", 10);
        assert!(!hits.is_empty());
        assert!(hits[0].snippet.to_lowercase().contains("ownership"));
    }

    #[test]
    fn multi_term_query_scores_combined_matches_higher() {
        let mut idx = SearchIndex::new();
        idx.add(mk_chunk(
            "Rust is a safe systems programming language with great tooling.",
        ));
        idx.add(mk_chunk(
            "Python is a flexible scripting language used widely.",
        ));
        idx.add(mk_chunk("This chunk talks about nothing in particular."));
        let hits = idx.search("rust programming language", 10);
        assert!(!hits.is_empty());
        // The chunk that contains rust + programming + language should win.
        assert!(hits[0].snippet.to_lowercase().contains("rust"));
    }

    #[test]
    fn empty_query_returns_empty_hits() {
        let mut idx = SearchIndex::new();
        idx.add(mk_chunk("Some content here."));
        assert!(idx.search("", 5).is_empty());
        assert!(idx.search("   ", 5).is_empty());
    }

    #[test]
    fn stopwords_are_ignored() {
        let mut idx = SearchIndex::new();
        idx.add(mk_chunk("Database indexing in PostgreSQL is efficient."));
        // "the" / "is" alone yield nothing because they are stopwords.
        assert!(idx.search("the is at", 5).is_empty());
    }

    #[test]
    fn limit_caps_results() {
        let mut idx = SearchIndex::new();
        for i in 0..5 {
            idx.add(mk_chunk(&format!(
                "{i} chunk number talks about embeddings indexing search"
            )));
        }
        let hits = idx.search("embeddings", 2);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn add_document_inserts_all_chunks() {
        let doc = ingest_text(
            "# Doc\n\nFirst paragraph about embeddings.\n\n## Sub\n\nSecond paragraph.\n",
            None,
            crate::chunker::DEFAULT_CHUNK_SIZE,
        );
        let chunk_count = doc.chunks.len();
        let mut idx = SearchIndex::new();
        idx.add_document(doc);
        assert_eq!(idx.len(), chunk_count);
        let hits = idx.search("embeddings", 5);
        assert!(!hits.is_empty());
    }

    #[test]
    fn search_bm25_helper_works_standalone() {
        let chunks = vec![
            mk_chunk("BM25 ranking is widely used."),
            mk_chunk("Vector search uses embeddings."),
        ];
        let hits = search_bm25("bm25 ranking", &chunks, 3);
        assert!(!hits.is_empty());
        assert!(hits[0].snippet.to_lowercase().contains("bm25"));
    }

    #[test]
    fn hits_carry_source_metadata() {
        use std::path::PathBuf;
        let path = PathBuf::from("/tmp/note.md");
        let mut idx = SearchIndex::new();
        let mut chunk = mk_chunk("Custom metadata lives on chunk meta fields.");
        chunk.meta.source_path = Some(path.clone());
        chunk.meta.heading_path = vec!["# Top".to_string()];
        idx.add(chunk);
        let hits = idx.search("metadata", 5);
        assert_eq!(hits[0].source_path.as_deref(), Some(path.as_path()));
        assert_eq!(hits[0].heading_path, vec!["# Top".to_string()]);
    }
}
