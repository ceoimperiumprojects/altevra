//! Synthetic document URI contract (R2).
//!
//! Every DB object that gets embedded receives a STABLE synthetic URI as its
//! `source_path` / `pending_indexing.path`. This eliminates fake filesystem
//! paths and ensures uniqueness across types.
//!
//! URI scheme:
//!   `db://turn/<turn_uuid>`
//!   `db://learning/<learning_uuid>`
//!   `db://note/<note_uuid>`
//!   `db://wiki/<wiki_slug_or_uuid>`
//!   `db://research/<item_uuid>`
//!
//! The checksum stored alongside is SHA-256 of the *embedded text* (not the
//! raw DB row). Re-embedding is triggered on checksum change.

use sha2::{Digest, Sha256};

/// Well-known object types for synthetic URIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbObjectType {
    Turn,
    Learning,
    Note,
    Wiki,
    Research,
}

impl DbObjectType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Learning => "learning",
            Self::Note => "note",
            Self::Wiki => "wiki",
            Self::Research => "research",
        }
    }
}

/// Build a stable synthetic URI for a DB object.
///
/// ```
/// use altevra_memory::db_uri::{DbObjectType, db_uri};
/// let uri = db_uri(DbObjectType::Turn, "550e8400-e29b-41d4-a716-446655440000");
/// assert_eq!(uri, "db://turn/550e8400-e29b-41d4-a716-446655440000");
/// ```
pub fn db_uri(object_type: DbObjectType, id: &str) -> String {
    format!("db://{}/{}", object_type.as_str(), id)
}

/// Parse a synthetic `db://type/id` URI. Returns `(type_str, id)` or `None`
/// for non-synthetic paths.
pub fn parse_db_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("db://")?;
    let slash = rest.find('/')?;
    let type_str = &rest[..slash];
    let id = &rest[slash + 1..];
    if id.is_empty() {
        return None;
    }
    Some((type_str, id))
}

/// Compute checksum = SHA-256 hex of the text to be embedded.
/// Re-embed when this changes.
pub fn embed_checksum(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Maximum number of chunks produced from a single turn. Turns can be huge;
/// cap to avoid overwhelming the queue with a single verbose assistant reply.
pub const MAX_CHUNKS_PER_TURN: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_uri_format() {
        assert_eq!(
            db_uri(DbObjectType::Turn, "abc-123"),
            "db://turn/abc-123"
        );
        assert_eq!(
            db_uri(DbObjectType::Learning, "zzz"),
            "db://learning/zzz"
        );
        assert_eq!(
            db_uri(DbObjectType::Wiki, "revesta-gtm"),
            "db://wiki/revesta-gtm"
        );
    }

    #[test]
    fn parse_db_uri_roundtrip() {
        for (otype, id) in [
            (DbObjectType::Turn, "abc"),
            (DbObjectType::Learning, "def-123"),
            (DbObjectType::Note, "ghi"),
            (DbObjectType::Wiki, "revesta"),
            (DbObjectType::Research, "xyz"),
        ] {
            let uri = db_uri(otype, id);
            let (parsed_type, parsed_id) = parse_db_uri(&uri).expect("must parse");
            assert_eq!(parsed_type, otype.as_str());
            assert_eq!(parsed_id, id);
        }
    }

    #[test]
    fn parse_non_synthetic_returns_none() {
        assert!(parse_db_uri("/home/pavle/Obsidian/note.md").is_none());
        assert!(parse_db_uri("db://").is_none());
        assert!(parse_db_uri("db://turn/").is_none());
        assert!(parse_db_uri("https://example.com").is_none());
    }

    #[test]
    fn checksum_is_stable() {
        let a = embed_checksum("hello world");
        let b = embed_checksum("hello world");
        assert_eq!(a, b);
        assert_ne!(a, embed_checksum("goodbye"));
    }

    #[test]
    fn checksum_is_sha256_hex() {
        // SHA-256 of "hello" is known.
        let h = embed_checksum("hello");
        assert_eq!(h.len(), 64, "SHA-256 hex = 64 chars");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
