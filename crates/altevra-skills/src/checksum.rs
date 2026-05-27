use sha2::{Digest, Sha256};

/// Compute SHA-256 checksum of content, returned as lowercase hex string.
pub fn compute(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Verify that content matches expected checksum.
pub fn verify(content: &str, expected: &str) -> bool {
    compute(content) == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_stable() {
        let c1 = compute("hello world");
        let c2 = compute("hello world");
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_checksum_different() {
        let c1 = compute("hello world");
        let c2 = compute("hello worlds");
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_verify() {
        let content = "skill content here";
        let cs = compute(content);
        assert!(verify(content, &cs));
        assert!(!verify("modified content", &cs));
    }
}
