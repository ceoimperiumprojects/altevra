//! Markdown-aware chunker.
//!
//! Walks a markdown document and groups paragraphs/lists under their
//! heading hierarchy. Emits a new chunk when the accumulated text size
//! exceeds `target_size` (approximated as character count — ~4 chars/token).

use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};

/// Default target chunk size in characters (~500 tokens at 4 chars/token).
pub const DEFAULT_CHUNK_SIZE: usize = 2000;

/// Metadata attached to a chunk describing its origin in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkMeta {
    pub source_path: Option<PathBuf>,
    /// Heading hierarchy leading to the chunk, e.g.
    /// `["# Section", "## Subsection"]`.
    pub heading_path: Vec<String>,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// A single retrievable unit of text extracted from a markdown document.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: uuid::Uuid,
    pub text: String,
    pub meta: ChunkMeta,
    /// Hex-encoded SHA-256 of `text`. Stable for identical content.
    pub checksum: String,
}

impl Chunk {
    fn new(text: String, meta: ChunkMeta) -> Self {
        let checksum = sha256_hex(text.as_bytes());
        Self {
            id: uuid::Uuid::new_v4(),
            text,
            meta,
            checksum,
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn heading_prefix(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "#",
        HeadingLevel::H2 => "##",
        HeadingLevel::H3 => "###",
        HeadingLevel::H4 => "####",
        HeadingLevel::H5 => "#####",
        HeadingLevel::H6 => "######",
    }
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Internal accumulator while walking events.
struct ChunkerState {
    source: Option<PathBuf>,
    target_size: usize,
    heading_path: Vec<String>,
    current_text: String,
    chunk_start_byte: usize,
    chunks: Vec<Chunk>,
}

impl ChunkerState {
    fn new(source: Option<PathBuf>, target_size: usize) -> Self {
        Self {
            source,
            target_size,
            heading_path: Vec::new(),
            current_text: String::new(),
            chunk_start_byte: 0,
            chunks: Vec::new(),
        }
    }

    fn append(&mut self, s: &str) {
        self.current_text.push_str(s);
    }

    fn append_line_break(&mut self) {
        if !self.current_text.ends_with('\n') {
            self.current_text.push('\n');
        }
    }

    fn append_paragraph_break(&mut self) {
        // Two newlines separate blocks visually.
        if self.current_text.is_empty() {
            return;
        }
        if !self.current_text.ends_with("\n\n") {
            if self.current_text.ends_with('\n') {
                self.current_text.push('\n');
            } else {
                self.current_text.push_str("\n\n");
            }
        }
    }

    fn flush(&mut self, end_byte: usize) {
        let text = self.current_text.trim().to_string();
        if text.is_empty() {
            self.current_text.clear();
            self.chunk_start_byte = end_byte;
            return;
        }
        let meta = ChunkMeta {
            source_path: self.source.clone(),
            heading_path: self.heading_path.clone(),
            start_byte: self.chunk_start_byte,
            end_byte,
        };
        self.chunks.push(Chunk::new(text, meta));
        self.current_text.clear();
        self.chunk_start_byte = end_byte;
    }

    fn maybe_flush(&mut self, end_byte: usize) {
        if self.current_text.trim().len() >= self.target_size {
            self.flush(end_byte);
        }
    }

    fn push_heading(&mut self, level: HeadingLevel, text: String) {
        let depth = heading_depth(level);
        // Truncate heading_path to the appropriate depth - 1, then push.
        while self.heading_path.len() >= depth {
            self.heading_path.pop();
        }
        // Pad with empty placeholders if the document skipped levels.
        while self.heading_path.len() < depth - 1 {
            self.heading_path.push(String::new());
        }
        let entry = format!("{} {}", heading_prefix(level), text.trim());
        self.heading_path.push(entry);
    }
}

/// Split a markdown document into chunks. Returns at least one chunk for any
/// non-empty input; returns an empty vector when the document contains no
/// renderable text.
pub fn chunk_markdown(content: &str, source: Option<&Path>, target_size: usize) -> Vec<Chunk> {
    let target_size = if target_size == 0 {
        DEFAULT_CHUNK_SIZE
    } else {
        target_size
    };
    let mut state = ChunkerState::new(source.map(PathBuf::from), target_size);

    let parser = Parser::new(content).into_offset_iter();
    let mut in_heading: Option<HeadingLevel> = None;
    let mut heading_buffer = String::new();

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                // Heading starts a new logical section. If we already have
                // content, finalize it so the new chunk inherits the new
                // heading_path.
                if !state.current_text.trim().is_empty() {
                    state.flush(range.start);
                }
                in_heading = Some(level);
                heading_buffer.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(level) = in_heading.take() {
                    state.push_heading(level, heading_buffer.clone());
                    heading_buffer.clear();
                    // Also include the heading text in the chunk body so
                    // search can match on it.
                    let prefix = heading_prefix(level);
                    state.append(&format!(
                        "{} {}\n\n",
                        prefix,
                        state
                            .heading_path
                            .last()
                            .map(|s| s.trim_start_matches('#').trim())
                            .unwrap_or("")
                    ));
                    state.chunk_start_byte = range.start;
                }
            }
            Event::Text(t) => {
                if in_heading.is_some() {
                    heading_buffer.push_str(&t);
                } else {
                    state.append(&t);
                }
            }
            Event::Code(t) => {
                if in_heading.is_some() {
                    heading_buffer.push_str(&t);
                } else {
                    state.append("`");
                    state.append(&t);
                    state.append("`");
                }
            }
            Event::SoftBreak => state.append(" "),
            Event::HardBreak => state.append_line_break(),
            Event::End(TagEnd::Paragraph) => {
                state.append_paragraph_break();
                state.maybe_flush(range.end);
            }
            Event::End(TagEnd::Item) => {
                state.append_line_break();
            }
            Event::End(TagEnd::List(_)) => {
                state.append_paragraph_break();
                state.maybe_flush(range.end);
            }
            Event::Start(Tag::Item) => {
                state.append("- ");
            }
            Event::Start(Tag::CodeBlock(_)) => {
                state.append("\n");
            }
            Event::End(TagEnd::CodeBlock) => {
                state.append("\n");
                state.maybe_flush(range.end);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                state.append("> ");
            }
            Event::End(TagEnd::BlockQuote) => {
                state.append_paragraph_break();
            }
            _ => {}
        }
    }

    state.flush(content.len());
    state.chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_produces_no_chunks() {
        let chunks = chunk_markdown("", None, DEFAULT_CHUNK_SIZE);
        assert!(chunks.is_empty());
    }

    #[test]
    fn whitespace_only_document_produces_no_chunks() {
        let chunks = chunk_markdown("   \n\n   \n", None, DEFAULT_CHUNK_SIZE);
        assert!(chunks.is_empty());
    }

    #[test]
    fn single_heading_with_paragraph() {
        let md = "# Title\n\nHello world, this is a paragraph.\n";
        let chunks = chunk_markdown(md, None, DEFAULT_CHUNK_SIZE);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert_eq!(chunk.meta.heading_path, vec!["# Title".to_string()]);
        assert!(chunk.text.contains("Hello world"));
        assert!(!chunk.checksum.is_empty());
    }

    #[test]
    fn deep_heading_nesting_tracked() {
        let md = "# A\n\nfirst\n\n## B\n\nsecond\n\n### C\n\nthird text here.\n";
        let chunks = chunk_markdown(md, None, DEFAULT_CHUNK_SIZE);
        assert!(chunks.len() >= 3);
        // The last chunk should sit under A > B > C.
        let last = chunks.last().unwrap();
        assert_eq!(last.meta.heading_path.len(), 3);
        assert!(last.meta.heading_path[0].starts_with("# A"));
        assert!(last.meta.heading_path[1].starts_with("## B"));
        assert!(last.meta.heading_path[2].starts_with("### C"));
    }

    #[test]
    fn oversized_content_splits_into_multiple_chunks() {
        // Build a long paragraph well beyond target_size = 200.
        let paragraph = "lorem ipsum dolor sit amet, ".repeat(60);
        let md = format!("# Big\n\n{paragraph}\n\nMore text.\n\n{paragraph}\n");
        let chunks = chunk_markdown(&md, None, 200);
        assert!(
            chunks.len() >= 2,
            "expected multiple chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(!chunk.text.is_empty());
            assert!(chunk.meta.heading_path[0].starts_with("# Big"));
        }
    }

    #[test]
    fn checksum_is_stable_for_same_text() {
        let md = "# Same\n\nIdentical body text.\n";
        let a = chunk_markdown(md, None, DEFAULT_CHUNK_SIZE);
        let b = chunk_markdown(md, None, DEFAULT_CHUNK_SIZE);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.checksum, y.checksum);
            assert_ne!(x.id, y.id); // UUIDs are random per call
        }
    }

    #[test]
    fn source_path_is_propagated() {
        let path = PathBuf::from("/tmp/foo.md");
        let chunks = chunk_markdown("# X\n\nbody.\n", Some(&path), DEFAULT_CHUNK_SIZE);
        assert_eq!(chunks[0].meta.source_path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn list_items_preserved() {
        let md = "# List\n\n- one\n- two\n- three\n";
        let chunks = chunk_markdown(md, None, DEFAULT_CHUNK_SIZE);
        assert_eq!(chunks.len(), 1);
        let txt = &chunks[0].text;
        assert!(txt.contains("one"));
        assert!(txt.contains("two"));
        assert!(txt.contains("three"));
    }
}
