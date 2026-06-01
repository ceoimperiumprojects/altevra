//! Redactor — replaces detected secrets in arbitrary text with a placeholder.
//!
//! Iterates matches back-to-front so byte offsets stay valid as the string is
//! mutated in place.

use crate::detector::detect_secrets;

/// Default placeholder used by [`redact`].
pub const DEFAULT_REPLACEMENT: &str = "[REDACTED]";

/// Replace every detected secret in `text` with `[REDACTED]`.
pub fn redact(text: &str) -> String {
    redact_with(text, DEFAULT_REPLACEMENT)
}

/// Replace every detected secret in `text` with `replacement`.
///
/// For `DatabaseUrl` matches only the password portion is replaced, preserving
/// the host / path of the URL.
pub fn redact_with(text: &str, replacement: &str) -> String {
    let mut matches = detect_secrets(text);
    if matches.is_empty() {
        return text.to_string();
    }

    // Mutate from the end so earlier offsets remain valid.
    matches.sort_by_key(|m| m.start);
    let mut buf = String::from(text);
    for m in matches.into_iter().rev() {
        // Defensive bounds — `detect_secrets` already guarantees valid offsets,
        // but keep the redactor robust against future regex changes.
        if m.end <= buf.len() && m.start <= m.end {
            buf.replace_range(m.start..m.end, replacement);
        }
    }
    buf
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_replaces_all_secrets_with_default_marker() {
        let text = "key=sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 and AKIAIOSFODNN7EXAMPLE";
        let redacted = redact(text);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345"));
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn redact_preserves_non_secret_content() {
        let text = "the quick brown fox jumps over the lazy dog";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn redact_with_custom_replacement() {
        let text = "token: ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let redacted = redact_with(text, "***");
        assert!(redacted.contains("***"));
        assert!(!redacted.contains("ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
        assert!(redacted.starts_with("token: "));
    }

    #[test]
    fn redact_db_url_strips_credentials() {
        let text = "DATABASE_URL=postgres://app:supersecret@db.example.com:5432/prod";
        let redacted = redact(text);
        // Password is gone.
        assert!(!redacted.contains("supersecret"));
        // The whole user:pass credential segment is redacted (fail-closed); the
        // host/path structure is preserved so the URL is still recognisable.
        assert!(redacted.contains("postgres://"));
        assert!(redacted.contains("@db.example.com:5432/prod"));
        assert!(!redacted.contains("app:supersecret"));
    }

    #[test]
    fn redact_handles_multiple_secrets_back_to_front() {
        let text = "a=sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 b=sk-ZYXWVUTSRQPONMLKJIHGFEDCBA987654";
        let redacted = redact(text);
        let occurrences = redacted.matches("[REDACTED]").count();
        assert_eq!(occurrences, 2);
        assert!(redacted.starts_with("a="));
        assert!(redacted.contains(" b="));
    }

    #[test]
    fn redact_empty_string_is_empty() {
        assert_eq!(redact(""), "");
    }
}
