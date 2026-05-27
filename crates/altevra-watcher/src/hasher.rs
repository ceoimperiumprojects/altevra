//! File content hashing for diff tracking. We keep only the first 8 hex
//! characters of SHA-256 — enough to detect changes and cheap to compute.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Compute the 8-char hex prefix of SHA-256 over the file contents. Returns
/// `None` if the file does not exist (e.g. for delete events).
pub fn short_hash(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let digest = h.finalize();
    Some(hex::encode(&digest[..4]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn returns_stable_8_hex_for_existing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, b"hello").unwrap();
        let h1 = short_hash(&p).unwrap();
        let h2 = short_hash(&p).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 8);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn returns_none_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("nope.txt");
        assert!(short_hash(&p).is_none());
    }

    #[test]
    fn different_content_yields_different_hash() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        std::fs::write(&a, b"foo").unwrap();
        std::fs::write(&b, b"bar").unwrap();
        assert_ne!(short_hash(&a), short_hash(&b));
    }
}
