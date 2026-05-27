//! Markdown document parser used by the vault crate.

use std::fs;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::frontmatter::{parse_frontmatter, Frontmatter};

/// Errors that can occur while parsing a document on disk.
#[derive(Debug, Error)]
pub enum ParserError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("frontmatter error in {path}: {source}")]
    Frontmatter {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
}

/// A fully parsed markdown document on disk.
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub path: PathBuf,
    pub frontmatter: Option<Frontmatter>,
    pub body: String,
    /// SHA-256 of the full original file contents (lowercase hex).
    pub checksum: String,
    /// First H1 heading found in the body, if any.
    pub title: Option<String>,
    /// Tags extracted from the frontmatter `tags` field (array or csv string).
    pub tags: Vec<String>,
}

/// Parse a markdown document from disk.
pub fn parse_document(path: &Path) -> anyhow::Result<ParsedDocument> {
    let content = fs::read_to_string(path).map_err(|e| ParserError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    parse_document_from_str(path, &content)
}

/// Parse a markdown document from an in-memory string.  Exposed mostly for
/// tests but useful when content is already loaded.
pub fn parse_document_from_str(path: &Path, content: &str) -> anyhow::Result<ParsedDocument> {
    let checksum = sha256_hex(content.as_bytes());

    let (frontmatter, body) = parse_frontmatter(content).map_err(|e| ParserError::Frontmatter {
        path: path.to_path_buf(),
        source: e,
    })?;

    let body = body.to_string();
    let title = extract_first_h1(&body);
    let tags = frontmatter.as_ref().map(|f| f.tags()).unwrap_or_default();

    Ok(ParsedDocument {
        path: path.to_path_buf(),
        frontmatter,
        body,
        checksum,
        title,
        tags,
    })
}

/// Extract the text of the first H1 heading from a markdown body.
fn extract_first_h1(body: &str) -> Option<String> {
    let parser = Parser::new(body);
    let mut in_h1 = false;
    let mut buf = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) => {
                in_h1 = true;
                buf.clear();
            }
            Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
                if in_h1 {
                    let trimmed = buf.trim().to_string();
                    return if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    };
                }
            }
            Event::Text(t) if in_h1 => {
                buf.push_str(&t);
            }
            Event::Code(t) if in_h1 => {
                buf.push_str(&t);
            }
            _ => {}
        }
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_tmp(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_document_with_frontmatter_and_title() {
        let dir = TempDir::new().unwrap();
        let p = write_tmp(
            &dir,
            "note.md",
            "---\ntitle: meta\ntags: [a, b]\n---\n# Real Title\n\ncontent\n",
        );
        let parsed = parse_document(&p).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Real Title"));
        assert_eq!(parsed.tags, vec!["a", "b"]);
        assert!(parsed.frontmatter.is_some());
        assert!(parsed.body.starts_with("# Real Title"));
        assert_eq!(parsed.checksum.len(), 64);
    }

    #[test]
    fn parses_document_without_frontmatter() {
        let dir = TempDir::new().unwrap();
        let p = write_tmp(&dir, "note.md", "# Just a heading\nbody\n");
        let parsed = parse_document(&p).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Just a heading"));
        assert!(parsed.frontmatter.is_none());
        assert!(parsed.tags.is_empty());
    }

    #[test]
    fn empty_file_parses_to_empty_doc() {
        let dir = TempDir::new().unwrap();
        let p = write_tmp(&dir, "empty.md", "");
        let parsed = parse_document(&p).unwrap();
        assert!(parsed.frontmatter.is_none());
        assert!(parsed.title.is_none());
        assert_eq!(parsed.body, "");
        assert!(parsed.tags.is_empty());
    }

    #[test]
    fn checksum_is_deterministic() {
        let dir = TempDir::new().unwrap();
        let p1 = write_tmp(&dir, "a.md", "hello");
        let p2 = write_tmp(&dir, "b.md", "hello");
        let a = parse_document(&p1).unwrap();
        let b = parse_document(&p2).unwrap();
        assert_eq!(a.checksum, b.checksum);
    }

    #[test]
    fn missing_file_errors() {
        let p = PathBuf::from("/definitely/does/not/exist.md");
        let err = parse_document(&p).unwrap_err();
        assert!(err.to_string().contains("failed to read"));
    }

    #[test]
    fn malformed_frontmatter_surfaces_error() {
        let dir = TempDir::new().unwrap();
        let p = write_tmp(&dir, "bad.md", "---\ntitle: ok\n");
        let err = parse_document(&p).unwrap_err();
        assert!(
            err.to_string().contains("frontmatter error") || err.to_string().contains("closing")
        );
    }

    #[test]
    fn extracts_first_of_multiple_h1s() {
        let parsed =
            parse_document_from_str(Path::new("mem.md"), "# First\nbody\n# Second\nmore\n")
                .unwrap();
        assert_eq!(parsed.title.as_deref(), Some("First"));
    }

    #[test]
    fn ignores_lower_level_headings_for_title() {
        let parsed =
            parse_document_from_str(Path::new("mem.md"), "## Subheading\nno h1 here\n").unwrap();
        assert!(parsed.title.is_none());
    }
}
