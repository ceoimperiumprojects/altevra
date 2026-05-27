//! Secret detection — scans arbitrary text for API keys, tokens, and other
//! sensitive values. Used by hooks before content is forwarded to AI tools.
//!
//! Patterns are compiled once via `OnceLock` and cached for the lifetime of
//! the process.

use regex::Regex;
use std::sync::OnceLock;

/// The kind of secret a detector pattern matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// OpenAI API key (`sk-...`).
    OpenAIKey,
    /// Anthropic API key (`sk-ant-...`).
    AnthropicKey,
    /// AWS access key id (`AKIA...`).
    AwsAccessKey,
    /// GitHub personal access token (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`,
    /// or `github_pat_...`).
    GitHubToken,
    /// Slack token (`xoxb-`, `xoxp-`, `xoxa-`, `xoxr-`, `xoxs-`).
    SlackToken,
    /// Generic `api_key=`, `secret=`, or `token=` assignment.
    GenericApiKey,
    /// JSON Web Token (`eyJ...eyJ...`).
    JwtToken,
    /// PEM private key header.
    PrivateKey,
    /// Database connection URL with embedded password.
    DatabaseUrl,
}

/// A single secret occurrence inside the analysed text.
#[derive(Debug, Clone)]
pub struct SecretMatch {
    pub kind: SecretKind,
    /// Byte offset (inclusive) where the matched fragment starts.
    pub start: usize,
    /// Byte offset (exclusive) where the matched fragment ends.
    pub end: usize,
    /// The substring that should be redacted. For `DatabaseUrl` this is the
    /// password only, not the entire URL.
    pub matched: String,
}

// ---- compiled pattern cache ------------------------------------------------

struct Patterns {
    openai: Regex,
    anthropic: Regex,
    aws: Regex,
    github_prefix: Regex,
    github_pat: Regex,
    slack: Regex,
    generic: Regex,
    jwt: Regex,
    private_key: Regex,
    db_url: Regex,
}

fn patterns() -> &'static Patterns {
    static CELL: OnceLock<Patterns> = OnceLock::new();
    CELL.get_or_init(|| Patterns {
        // Anthropic check happens first; OpenAI must not absorb `sk-ant-`.
        openai: Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(),
        anthropic: Regex::new(r"sk-ant-[A-Za-z0-9_\-]{20,}").unwrap(),
        aws: Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
        github_prefix: Regex::new(r"(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}").unwrap(),
        github_pat: Regex::new(r"github_pat_[A-Za-z0-9_]{82}").unwrap(),
        slack: Regex::new(r"xox[bpars]-[A-Za-z0-9-]{10,}").unwrap(),
        generic: Regex::new(
            r#"(?i)(api[-_]?key|secret|token)\s*[=:]\s*['"]?([A-Za-z0-9_+/=\-]{20,})"#,
        )
        .unwrap(),
        jwt: Regex::new(r"eyJ[A-Za-z0-9_\-]+\.eyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+").unwrap(),
        private_key: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
        db_url: Regex::new(r"(postgres|mysql|mongodb)://[^:\s]+:([^@\s]+)@").unwrap(),
    })
}

/// Scan `text` and return every detected secret.
///
/// Matches are returned in ascending order by `start` offset. Overlapping
/// matches are de-duplicated: the longer / more specific match wins, so that
/// e.g. `sk-ant-...` is reported as `AnthropicKey`, not `OpenAIKey`.
pub fn detect_secrets(text: &str) -> Vec<SecretMatch> {
    let p = patterns();
    let mut out: Vec<SecretMatch> = Vec::new();

    // Anthropic (must run before OpenAI so we can exclude overlaps).
    for m in p.anthropic.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::AnthropicKey,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    // OpenAI — skip when the prefix is `sk-ant-` (already captured above).
    for m in p.openai.find_iter(text) {
        if m.as_str().starts_with("sk-ant-") {
            continue;
        }
        out.push(SecretMatch {
            kind: SecretKind::OpenAIKey,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.aws.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::AwsAccessKey,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.github_prefix.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::GitHubToken,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.github_pat.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::GitHubToken,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.slack.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::SlackToken,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.jwt.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::JwtToken,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.private_key.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::PrivateKey,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    // Generic assignment — only the captured value group is reported.
    for caps in p.generic.captures_iter(text) {
        if let Some(val) = caps.get(2) {
            out.push(SecretMatch {
                kind: SecretKind::GenericApiKey,
                start: val.start(),
                end: val.end(),
                matched: val.as_str().to_string(),
            });
        }
    }
    // Database URLs — capture password only.
    for caps in p.db_url.captures_iter(text) {
        if let Some(pw) = caps.get(2) {
            out.push(SecretMatch {
                kind: SecretKind::DatabaseUrl,
                start: pw.start(),
                end: pw.end(),
                matched: pw.as_str().to_string(),
            });
        }
    }

    // Sort by start offset.
    out.sort_by_key(|m| m.start);

    // Drop overlapping matches: keep the first (which, after sort, is the
    // earliest starting one); skip any subsequent match that starts inside it.
    let mut dedup: Vec<SecretMatch> = Vec::with_capacity(out.len());
    for m in out {
        if let Some(prev) = dedup.last() {
            if m.start < prev.end {
                continue;
            }
        }
        dedup.push(m);
    }
    dedup
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openai_key_positive() {
        let text = "my key is sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        let hits = detect_secrets(text);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SecretKind::OpenAIKey);
    }

    #[test]
    fn openai_does_not_swallow_anthropic() {
        let text = "creds sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAA";
        let hits = detect_secrets(text);
        assert_eq!(hits.len(), 1, "expected single match, got: {hits:?}");
        assert_eq!(hits[0].kind, SecretKind::AnthropicKey);
    }

    #[test]
    fn openai_negative_short_prefix() {
        // `sk-short` is too short to match the {20,} length requirement.
        let hits = detect_secrets("sk-short");
        assert!(
            hits.iter().all(|h| h.kind != SecretKind::OpenAIKey),
            "false positive on `sk-short`: {hits:?}"
        );
    }

    #[test]
    fn detects_aws_access_key_positive_and_negative() {
        let pos = detect_secrets("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(pos.len(), 1);
        assert_eq!(pos[0].kind, SecretKind::AwsAccessKey);

        // Lowercase / wrong prefix must not match.
        let neg = detect_secrets("akiaiosfodnn7example AKIA-tooshort");
        assert!(neg.iter().all(|h| h.kind != SecretKind::AwsAccessKey));
    }

    #[test]
    fn detects_github_tokens() {
        let pat = format!("ghp_{}", "A".repeat(36));
        let hits = detect_secrets(&pat);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SecretKind::GitHubToken);

        let pat2 = format!("github_pat_{}", "a".repeat(82));
        let hits2 = detect_secrets(&pat2);
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].kind, SecretKind::GitHubToken);

        // Wrong prefix length / unsupported prefix.
        let neg = detect_secrets("ghx_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(neg.iter().all(|h| h.kind != SecretKind::GitHubToken));
    }

    #[test]
    fn detects_slack_tokens() {
        let hits = detect_secrets("xoxb-1234567890-ABCDEFGHIJKL");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SecretKind::SlackToken);

        // Unsupported prefix.
        let neg = detect_secrets("xoxz-1234567890-ABCDEFGHIJKL");
        assert!(neg.iter().all(|h| h.kind != SecretKind::SlackToken));
    }

    #[test]
    fn detects_generic_api_key() {
        let hits = detect_secrets("api_key=ABCDEFGHIJKLMNOPQRSTUV");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SecretKind::GenericApiKey);

        // Too short — should not trigger.
        let neg = detect_secrets("api_key=short");
        assert!(neg.iter().all(|h| h.kind != SecretKind::GenericApiKey));
    }

    #[test]
    fn detects_jwt_positive_and_negative() {
        let jwt =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let hits = detect_secrets(jwt);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SecretKind::JwtToken);

        // Missing third segment.
        let neg = detect_secrets("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9");
        assert!(neg.iter().all(|h| h.kind != SecretKind::JwtToken));
    }

    #[test]
    fn detects_private_key_header() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJ...";
        let hits = detect_secrets(pem);
        assert!(hits.iter().any(|h| h.kind == SecretKind::PrivateKey));

        let neg = detect_secrets("-----BEGIN CERTIFICATE-----");
        assert!(neg.iter().all(|h| h.kind != SecretKind::PrivateKey));
    }

    #[test]
    fn detects_db_url_password_only() {
        let url = "postgres://user:hunter2pw@localhost:5432/db";
        let hits = detect_secrets(url);
        let db = hits
            .iter()
            .find(|h| h.kind == SecretKind::DatabaseUrl)
            .expect("expected db url match");
        assert_eq!(db.matched, "hunter2pw");

        // No password section — must not match.
        let neg = detect_secrets("postgres://localhost:5432/db");
        assert!(neg.iter().all(|h| h.kind != SecretKind::DatabaseUrl));
    }

    #[test]
    fn empty_input_returns_no_matches() {
        assert!(detect_secrets("").is_empty());
        assert!(detect_secrets("just some harmless prose").is_empty());
    }
}
