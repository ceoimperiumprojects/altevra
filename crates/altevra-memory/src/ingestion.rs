//! File → document ingestion pipeline.
//!
//! Ties the markdown chunker to a frontmatter parser and produces an
//! [`IngestedDocument`] ready for indexing.

use std::path::{Path, PathBuf};

use altevra_core::security::Sensitivity;
use altevra_secrets::guard_text;
use anyhow::Context;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::chunker::{chunk_markdown, Chunk, DEFAULT_CHUNK_SIZE};

/// A markdown file after frontmatter extraction and chunking.
#[derive(Debug, Clone)]
pub struct IngestedDocument {
    pub document_id: uuid::Uuid,
    pub source_path: PathBuf,
    pub chunks: Vec<Chunk>,
    /// Frontmatter decoded into JSON. `None` when the document had no
    /// frontmatter block.
    pub frontmatter: Option<serde_json::Value>,
    /// Hex-encoded SHA-256 of the full original file bytes.
    pub checksum: String,
}

/// Read a markdown file from disk, parse it and split it into chunks.
pub fn ingest_file(path: &Path, chunk_size: usize) -> anyhow::Result<IngestedDocument> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read source file: {}", path.display()))?;
    let checksum = sha256_hex(&bytes);
    let text = String::from_utf8(bytes)
        .with_context(|| format!("source file is not valid UTF-8: {}", path.display()))?;

    let (frontmatter, body) = split_frontmatter(&text);
    let target = effective_chunk_size(chunk_size);
    let chunks = chunk_markdown(body, Some(path), target);

    Ok(IngestedDocument {
        document_id: uuid::Uuid::new_v4(),
        source_path: path.to_path_buf(),
        chunks,
        frontmatter,
        checksum,
    })
}

/// In-memory ingestion variant. Useful for tests and pipes.
pub fn ingest_text(text: &str, source: Option<PathBuf>, chunk_size: usize) -> IngestedDocument {
    let checksum = sha256_hex(text.as_bytes());
    let (frontmatter, body) = split_frontmatter(text);
    let target = effective_chunk_size(chunk_size);
    let chunks = chunk_markdown(body, source.as_deref(), target);

    IngestedDocument {
        document_id: uuid::Uuid::new_v4(),
        source_path: source.unwrap_or_else(|| PathBuf::from("<memory>")),
        chunks,
        frontmatter,
        checksum,
    }
}

/// Ingest a remote URL's textual content with enriched frontmatter (title, source tag, URL).
/// The frontmatter is built and prepended to the body so downstream callers can preserve
/// provenance when indexing the result.
pub fn ingest_url_content(
    url: &str,
    title: &str,
    content: &str,
    source_tag: &str,
    chunk_size: usize,
) -> IngestedDocument {
    let body = format!(
        "---\nkind: external-content\nsource: {src}\nurl: {url}\ntitle: {title}\nfetched_at: {ts}\n---\n\n{content}\n",
        src = source_tag,
        url = url,
        title = sanitize_yaml_value(title),
        ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        content = content,
    );
    // Build a deterministic synthetic source path so two ingests of the same URL produce
    // identical paths (helps with dedup at the repository layer).
    let safe = sanitize_path_segment(url);
    let source = PathBuf::from(format!("<{source_tag}>/{safe}.md"));
    ingest_text(&body, Some(source), chunk_size)
}

/// Route every chunk's text through `altevra-secrets::guard_text` BEFORE it is
/// persisted or indexed (T1.13 / R11). Secrets + PII are redacted in place; the
/// chunk's `text` is replaced with the safe value and its `checksum` recomputed
/// over the redacted bytes. Returns the most-restrictive sensitivity seen across
/// the document (default-UP, fail-closed) so the caller can label the persisted
/// document at the right ceiling.
///
/// This is the single pre-persist scrub for the memory lane — identical contract
/// to the capture path's `guard_text`, applied per chunk so large documents are
/// still cheap. NO embeddings happen here (R12: the vector lane is opt-in and
/// downstream); guarding is a precondition for BOTH the FTS index and any later
/// embed.
pub fn guard_document(doc: &mut IngestedDocument, declared: Sensitivity) -> Sensitivity {
    let mut ceiling = declared.clone();
    for chunk in &mut doc.chunks {
        let guarded = guard_text(&chunk.text, declared.clone());
        if guarded.value != chunk.text {
            chunk.text = guarded.value;
            chunk.checksum = sha256_hex(chunk.text.as_bytes());
        }
        ceiling = ceiling.combine(&guarded.sensitivity);
    }
    ceiling
}

/// Index a single already-guarded chunk into the FTS5 substrate (`object_fts`,
/// R12 — bm25, NO vectors). Mirrors `altevra_db::FtsRepository::index`: delete
/// any prior row for this object then insert, so a re-index can't duplicate.
/// `body` MUST be the redacted chunk text (run `guard_document` first) — un-
/// guarded text must never reach the index.
pub async fn fts_index_chunk(
    pool: &SqlitePool,
    chunk_id: &str,
    title: &str,
    body: &str,
    tags: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM object_fts WHERE object_type = 'memory_chunk' AND object_id = ?")
        .bind(chunk_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO object_fts (object_type, object_id, title, body, tags) \
         VALUES ('memory_chunk', ?, ?, ?, ?)",
    )
    .bind(chunk_id)
    .bind(title)
    .bind(body)
    .bind(tags)
    .execute(pool)
    .await?;
    Ok(())
}

fn sanitize_yaml_value(s: &str) -> String {
    s.replace('"', "'")
        .replace('\n', " ")
        .chars()
        .take(180)
        .collect()
}

fn sanitize_path_segment(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn effective_chunk_size(requested: usize) -> usize {
    if requested == 0 {
        DEFAULT_CHUNK_SIZE
    } else {
        requested
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Minimal YAML-frontmatter splitter.
///
/// TODO: replace with `altevra_vault::parse_document` once that crate's
/// `parser` module lands. Today the vault crate only exposes module
/// declarations, so we do the parse inline to avoid blocking.
fn split_frontmatter(text: &str) -> (Option<serde_json::Value>, &str) {
    let trimmed_start = text.trim_start_matches('\u{feff}');
    if !trimmed_start.starts_with("---") {
        return (None, text);
    }
    // Find the first line after the opening `---`.
    let after_open = match trimmed_start.find('\n') {
        Some(idx) => &trimmed_start[idx + 1..],
        None => return (None, text),
    };
    // Find the closing `---` on its own line.
    let mut search_from = 0usize;
    while let Some(rel) = after_open[search_from..].find("---") {
        let abs = search_from + rel;
        let at_line_start = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
        let end = abs + 3;
        let at_line_end = end == after_open.len()
            || after_open.as_bytes()[end] == b'\n'
            || after_open.as_bytes()[end] == b'\r';
        if at_line_start && at_line_end {
            let raw = &after_open[..abs];
            // Skip the trailing newline after the closing `---` if any.
            let mut body_start = end;
            while body_start < after_open.len()
                && (after_open.as_bytes()[body_start] == b'\n'
                    || after_open.as_bytes()[body_start] == b'\r')
            {
                body_start += 1;
            }
            let body = &after_open[body_start..];
            let parsed = parse_yaml_to_json(raw);
            return (parsed, body);
        }
        search_from = abs + 3;
    }
    (None, text)
}

/// Very small YAML subset → JSON mapper. Supports flat `key: value` pairs and
/// quoted strings. Anything fancier is preserved as a plain string.
fn parse_yaml_to_json(raw: &str) -> Option<serde_json::Value> {
    use serde_json::{Map, Value};
    let mut map = Map::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim().to_string();
        let value = v.trim().trim_matches('"').trim_matches('\'').to_string();
        let json_value = if value.is_empty() {
            Value::Null
        } else if let Ok(b) = value.parse::<bool>() {
            Value::Bool(b)
        } else if let Ok(i) = value.parse::<i64>() {
            Value::Number(i.into())
        } else {
            Value::String(value)
        };
        map.insert(key, json_value);
    }
    if map.is_empty() {
        None
    } else {
        Some(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::TempDir;

    #[test]
    fn guard_document_redacts_secret_and_pii_before_persist() {
        // A chunk carrying a live key + an email must be scrubbed in place; the
        // checksum re-derives over the redacted bytes (no plaintext survives).
        let mut doc = ingest_text(
            "Reach me at jane.doe@example.com with key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.",
            None,
            DEFAULT_CHUNK_SIZE,
        );
        let before = doc.chunks[0].checksum.clone();
        let ceiling = guard_document(&mut doc, Sensitivity::Internal);

        let text = &doc.chunks[0].text;
        assert!(
            !text.contains("jane.doe@example.com"),
            "email must be redacted"
        );
        assert!(!text.contains("sk-ant-api03-"), "secret must be redacted");
        assert!(text.contains("[REDACTED"), "placeholder present");
        assert_ne!(doc.chunks[0].checksum, before, "checksum re-derived");
        // a credential present raises the ceiling above Internal (default-up).
        assert!(matches!(
            ceiling,
            Sensitivity::Confidential | Sensitivity::Restricted | Sensitivity::Secret
        ));
    }

    #[test]
    fn guard_document_clean_text_unchanged() {
        let mut doc = ingest_text("# Notes\n\nPlain prose, nothing sensitive.\n", None, 0);
        let before = doc.chunks[0].text.clone();
        let ceiling = guard_document(&mut doc, Sensitivity::Internal);
        assert_eq!(doc.chunks[0].text, before, "clean text is left as-is");
        assert!(matches!(ceiling, Sensitivity::Internal));
    }

    #[tokio::test]
    async fn fts_index_chunk_makes_redacted_chunk_searchable() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Minimal object_fts (FTS5) — the same shape migration 030 creates.
        sqlx::query(
            "CREATE VIRTUAL TABLE object_fts USING fts5(object_type, object_id, title, body, tags)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut doc = ingest_text(
            "The surplus pipeline targets Florida operators.",
            None,
            DEFAULT_CHUNK_SIZE,
        );
        guard_document(&mut doc, Sensitivity::Internal);
        let chunk = &doc.chunks[0];
        fts_index_chunk(&pool, &chunk.id.to_string(), "pipeline", &chunk.text, "")
            .await
            .unwrap();

        // searchable by a body term, scoped to the memory_chunk object type.
        let row: (String,) = sqlx::query_as(
            "SELECT object_id FROM object_fts \
             WHERE object_type = 'memory_chunk' AND object_fts MATCH 'Florida surplus'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, chunk.id.to_string());

        // re-index must replace, not duplicate.
        fts_index_chunk(&pool, &chunk.id.to_string(), "pipeline", &chunk.text, "")
            .await
            .unwrap();
        let (cnt,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM object_fts WHERE object_type = 'memory_chunk'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cnt, 1, "re-index replaces the prior row");
    }

    #[test]
    fn ingest_text_without_frontmatter() {
        let doc = ingest_text("# Hello\n\nWorld.\n", None, DEFAULT_CHUNK_SIZE);
        assert!(doc.frontmatter.is_none());
        assert_eq!(doc.chunks.len(), 1);
        assert!(doc.checksum.len() == 64);
    }

    #[test]
    fn ingest_text_with_frontmatter() {
        let body = "---\ntitle: My Note\ntags: 3\nactive: true\n---\n\n# Body\n\nContent here.\n";
        let doc = ingest_text(body, None, DEFAULT_CHUNK_SIZE);
        let fm = doc.frontmatter.expect("frontmatter present");
        assert_eq!(fm.get("title").and_then(|v| v.as_str()), Some("My Note"));
        assert_eq!(fm.get("tags").and_then(|v| v.as_i64()), Some(3));
        assert_eq!(fm.get("active").and_then(|v| v.as_bool()), Some(true));
        assert!(!doc.chunks.is_empty());
        assert!(doc.chunks[0].text.contains("Content here"));
    }

    #[test]
    fn ingest_file_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(
            &path,
            "---\ntitle: Test\n---\n\n# H\n\nfirst paragraph.\n\n## H2\n\nsecond.\n",
        )
        .unwrap();

        let doc = ingest_file(&path, DEFAULT_CHUNK_SIZE).unwrap();
        assert_eq!(doc.source_path, path);
        assert!(doc.frontmatter.is_some());
        assert!(!doc.chunks.is_empty());
        // Checksum must match a fresh SHA-256 of the raw bytes.
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(doc.checksum, sha256_hex(&raw));
    }

    #[test]
    fn ingest_file_missing_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.md");
        let res = ingest_file(&path, DEFAULT_CHUNK_SIZE);
        assert!(res.is_err());
    }

    #[test]
    fn frontmatter_splitter_handles_no_closing_marker() {
        let (fm, body) = split_frontmatter("---\nkey: value\nnot closed\n");
        assert!(fm.is_none());
        assert!(body.starts_with("---"));
    }

    #[test]
    fn ingest_url_content_builds_frontmatter() {
        let doc = ingest_url_content(
            "https://example.com/post",
            "Cool Article",
            "First paragraph.\n\nSecond.",
            "research:hn-frontpage",
            DEFAULT_CHUNK_SIZE,
        );
        let fm = doc.frontmatter.expect("frontmatter present");
        assert_eq!(
            fm.get("kind").and_then(|v| v.as_str()),
            Some("external-content")
        );
        assert_eq!(
            fm.get("source").and_then(|v| v.as_str()),
            Some("research:hn-frontpage")
        );
        assert_eq!(
            fm.get("url").and_then(|v| v.as_str()),
            Some("https://example.com/post")
        );
        assert!(!doc.chunks.is_empty());
        // Deterministic source path for dedup.
        assert!(doc
            .source_path
            .to_string_lossy()
            .contains("research:hn-frontpage"));
    }
}
