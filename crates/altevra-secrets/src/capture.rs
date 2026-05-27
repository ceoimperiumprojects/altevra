//! Auto-capture: detect secrets in arbitrary text and save them into the
//! secret store under generated, deterministic keys.
//!
//! Workflow:
//!   1. `detect_secrets(text)` finds matches.
//!   2. For each match we derive a stable key name from `SecretKind` + a
//!      short fingerprint (first 8 chars of SHA-256 of the value). The
//!      fingerprint is also returned, so callers can correlate the same
//!      secret across turns/sessions without leaking it.
//!   3. `store.set(key, value)` persists it (keyring or encrypted file).
//!   4. The redactor still scrubs the live text — auto-capture and redaction
//!      are complementary, not alternatives.

use sha2::{Digest, Sha256};

use crate::detector::{detect_secrets, SecretKind};
use crate::store::SecretStore;

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub kind: SecretKind,
    pub key: String,
    pub fingerprint: String,
    pub was_new: bool,
}

/// Scan `text` for secrets and persist any found into `store`. Returns one
/// `CaptureResult` per detected secret (deduplicated by key name).
///
/// Database-URL passwords use the captured password substring as the value
/// and a fingerprint of that password.
pub fn auto_capture(text: &str, store: &SecretStore) -> anyhow::Result<Vec<CaptureResult>> {
    let matches = detect_secrets(text);
    let mut out = Vec::with_capacity(matches.len());
    let mut seen = std::collections::HashSet::new();

    for m in matches {
        let value = m.matched.as_str();
        let key = derive_key_name(m.kind, value);
        if !seen.insert(key.clone()) {
            continue;
        }
        let fingerprint = fingerprint8(value);
        let was_new = match store.get(&key) {
            Ok(Some(existing)) => existing != value,
            Ok(None) => true,
            Err(_) => true, // best-effort: still attempt to write
        };
        if was_new {
            // Best-effort store; on failure we silently continue so a
            // missing keyring service doesn't take down the hook chain.
            let _ = store.set(&key, value);
        }
        out.push(CaptureResult {
            kind: m.kind,
            key,
            fingerprint,
            was_new,
        });
    }
    Ok(out)
}

/// Stable key name like `anthropic_key_a1b2c3d4`. Same secret → same key.
pub fn derive_key_name(kind: SecretKind, value: &str) -> String {
    let prefix = match kind {
        SecretKind::OpenAIKey => "openai_key",
        SecretKind::AnthropicKey => "anthropic_key",
        SecretKind::AwsAccessKey => "aws_access_key",
        SecretKind::GitHubToken => "github_token",
        SecretKind::SlackToken => "slack_token",
        SecretKind::GenericApiKey => "api_key",
        SecretKind::JwtToken => "jwt_token",
        SecretKind::PrivateKey => "private_key",
        SecretKind::DatabaseUrl => "db_password",
    };
    format!("{prefix}_{}", fingerprint8(value))
}

/// First 8 hex chars of SHA-256(value). Stable across processes/machines.
pub fn fingerprint8(value: &str) -> String {
    let mut h = Sha256::new();
    h.update(value.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..4])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SecretStore;
    use tempfile::TempDir;

    fn temp_store(tmp: &TempDir) -> SecretStore {
        std::env::set_var("ALTEVRA_TEST_CAPTURE_KEY", "supersecret-test-passphrase");
        SecretStore::new_encrypted_file(
            "altevra-test",
            tmp.path().join("secrets.enc"),
            "ALTEVRA_TEST_CAPTURE_KEY",
        )
    }

    #[test]
    fn key_naming_is_stable() {
        let a = derive_key_name(SecretKind::AnthropicKey, "sk-ant-abc123");
        let b = derive_key_name(SecretKind::AnthropicKey, "sk-ant-abc123");
        assert_eq!(a, b);
        assert!(a.starts_with("anthropic_key_"));
    }

    #[test]
    fn captures_anthropic_key() {
        let tmp = TempDir::new().unwrap();
        let store = temp_store(&tmp);
        let text = "use this: sk-ant-abc1234567890DEFghijklmnop and call it.";
        let results = auto_capture(text, &store).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SecretKind::AnthropicKey);
        assert!(results[0].was_new);
        let stored = store.get(&results[0].key).unwrap().unwrap();
        assert!(stored.starts_with("sk-ant-"));
    }

    #[test]
    fn idempotent_capture_marks_existing_as_not_new() {
        let tmp = TempDir::new().unwrap();
        let store = temp_store(&tmp);
        let text = "github: ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let first = auto_capture(text, &store).unwrap();
        assert!(first[0].was_new);
        let second = auto_capture(text, &store).unwrap();
        assert!(!second[0].was_new);
    }

    #[test]
    fn fingerprint_is_short_and_hex() {
        let f = fingerprint8("anything");
        assert_eq!(f.len(), 8);
        assert!(f.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn duplicate_secrets_in_one_text_dedupe() {
        let tmp = TempDir::new().unwrap();
        let store = temp_store(&tmp);
        let text = "sk-ant-xx11111111111111111111yyyy and again sk-ant-xx11111111111111111111yyyy";
        let results = auto_capture(text, &store).unwrap();
        assert_eq!(results.len(), 1);
    }
}
