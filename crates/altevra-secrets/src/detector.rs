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
    /// OpenAI API key — legacy `sk-...` AND modern `sk-proj-`/`sk-svcacct-`/`sk-admin-`.
    OpenAIKey,
    /// Anthropic API key (`sk-ant-...`).
    AnthropicKey,
    /// Stripe secret/restricted key (`sk_live_`, `sk_test_`, `rk_live_`, `rk_test_`).
    StripeKey,
    /// AWS access key id (`AKIA`, `ASIA`, `AROA`, `AIDA`, ...).
    AwsAccessKey,
    /// Google API key (`AIza...`).
    GoogleApiKey,
    /// npm access token (`npm_...`).
    NpmToken,
    /// GitHub personal access token (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`,
    /// or `github_pat_...`).
    GitHubToken,
    /// Slack token (`xoxb-`, `xoxp-`, `xoxa-`, `xoxr-`, `xoxs-`).
    SlackToken,
    /// Slack incoming-webhook URL (`hooks.slack.com/services/...`).
    SlackWebhook,
    /// Generic `api_key=`, `secret=`, `token=`, `password=`, `client_secret=` assignment.
    GenericApiKey,
    /// `Authorization: Bearer <token>` header value.
    BearerToken,
    /// JSON Web Token (`eyJ...eyJ...`).
    JwtToken,
    /// PEM private key header.
    PrivateKey,
    /// Database connection URL with embedded credentials (`user:pass@`).
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
    /// whole `user:pass` credential segment (so a password containing `@`
    /// cannot partially leak), not the entire URL.
    pub matched: String,
}

// ---- compiled pattern cache ------------------------------------------------

struct Patterns {
    openai: Regex,
    anthropic: Regex,
    stripe: Regex,
    aws: Regex,
    google: Regex,
    npm: Regex,
    github_prefix: Regex,
    github_pat: Regex,
    slack: Regex,
    slack_webhook: Regex,
    generic: Regex,
    bearer: Regex,
    jwt: Regex,
    private_key: Regex,
    db_url: Regex,
}

fn patterns() -> &'static Patterns {
    static CELL: OnceLock<Patterns> = OnceLock::new();
    CELL.get_or_init(|| Patterns {
        // Anthropic check happens first; OpenAI must not absorb `sk-ant-`.
        // OpenAI: legacy `sk-<alnum>` AND modern `sk-proj-`/`sk-svcacct-`/`sk-admin-`
        // (those contain hyphens), so the char class allows `_-` after `sk-`.
        openai: Regex::new(r"sk-[A-Za-z0-9_-]{20,}").unwrap(),
        anthropic: Regex::new(r"sk-ant-[A-Za-z0-9_\-]{20,}").unwrap(),
        // Stripe secret/restricted keys (underscore form — distinct from OpenAI's dash).
        stripe: Regex::new(r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{20,}").unwrap(),
        // AWS access-key ids: long-term (AKIA) + temporary/STS (ASIA) + role/user/etc.
        aws: Regex::new(r"(?:AKIA|ASIA|AROA|AIDA|AGPA|ANPA|ANVA|AIPA)[0-9A-Z]{16}").unwrap(),
        google: Regex::new(r"AIza[0-9A-Za-z_\-]{35}").unwrap(),
        npm: Regex::new(r"npm_[A-Za-z0-9]{36}").unwrap(),
        github_prefix: Regex::new(r"(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}").unwrap(),
        github_pat: Regex::new(r"github_pat_[A-Za-z0-9_]{82}").unwrap(),
        slack: Regex::new(r"xox[bpars]-[A-Za-z0-9-]{10,}").unwrap(),
        slack_webhook: Regex::new(r"hooks\.slack\.com/services/[A-Za-z0-9/_+-]{20,}").unwrap(),
        // Leading `[\w-]*?` lets trailing-token forms match (`aws_secret_access_key=`,
        // `gcp_api_key=`) — the bare keyword alternation alone missed them.
        generic: Regex::new(
            r#"(?i)([\w-]*?(?:api[-_]?key|secret|token|password|passwd|pwd|client[-_]?secret|access[-_]?key|apikey))\s*[=:]\s*['"]?([A-Za-z0-9_+/=\-]{20,})"#,
        )
        .unwrap(),
        bearer: Regex::new(r"(?i)bearer\s+([A-Za-z0-9_\-./=+]{20,})").unwrap(),
        jwt: Regex::new(r"eyJ[A-Za-z0-9_\-]+\.eyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+").unwrap(),
        // Match the WHOLE PEM block (header → base64 body → footer), not just the
        // header line — otherwise the key material leaks (R11 Codex #3 + re-verify).
        // `(?s)` so `.` spans newlines. Two alternatives: (1) header..END footer
        // when present; (2) header + the trailing base64 body when the footer is
        // ABSENT (truncated/streamed paste) — without (2) a header-only paste
        // redacted just the 27-byte header and leaked the whole body.
        private_key: Regex::new(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?:.*?-----END [A-Z0-9 ]*PRIVATE KEY-----|(?:[\r\n]+[A-Za-z0-9+/=]{16,})+)?",
        )
        .unwrap(),
        // Capture the FULL `user:pass` credential segment (greedy up to the LAST `@`
        // before the host). The password class must allow `/` and `@` (common in
        // un-percent-encoded base64 passwords) — restricting it to `[^/\s]` leaked
        // any DB password containing a slash (re-verify HIGH). The trailing host
        // token (no `/@:` ) anchors the final `@`.
        db_url: Regex::new(
            r"(?:postgres|postgresql|mysql|mongodb|mongodb\+srv|redis|rediss|amqp|amqps)://([^\s]+:[^\s]+)@[^\s/@:?#]+",
        )
        .unwrap(),
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
    for m in p.stripe.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::StripeKey,
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
    for m in p.google.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::GoogleApiKey,
            start: m.start(),
            end: m.end(),
            matched: m.as_str().to_string(),
        });
    }
    for m in p.npm.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::NpmToken,
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
    for m in p.slack_webhook.find_iter(text) {
        out.push(SecretMatch {
            kind: SecretKind::SlackWebhook,
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
    // `Authorization: Bearer <token>` — report the token value group only.
    for caps in p.bearer.captures_iter(text) {
        if let Some(val) = caps.get(1) {
            out.push(SecretMatch {
                kind: SecretKind::BearerToken,
                start: val.start(),
                end: val.end(),
                matched: val.as_str().to_string(),
            });
        }
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
    // Database URLs — redact the WHOLE `user:pass` credential segment (group 1),
    // not just the password, so a password containing `@` cannot partially leak.
    for caps in p.db_url.captures_iter(text) {
        if let Some(cred) = caps.get(1) {
            out.push(SecretMatch {
                kind: SecretKind::DatabaseUrl,
                start: cred.start(),
                end: cred.end(),
                matched: cred.as_str().to_string(),
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
    fn private_key_redacts_whole_block_not_just_header() {
        let pem = "before\n-----BEGIN PRIVATE KEY-----\nMIIBVwIBADANBgkqhkiG9w0BAQEFAASCAT8\nAAAAAAAAAAAAAAAAAAAAAAAA\n-----END PRIVATE KEY-----\nafter";
        let red = crate::redactor::redact(pem);
        // The base64 body and footer must be gone, not just the header.
        assert!(!red.contains("MIIBVwIBAD"), "key body leaked: {red}");
        assert!(!red.contains("END PRIVATE KEY"), "footer leaked: {red}");
        assert!(red.starts_with("before"));
        assert!(red.trim_end().ends_with("after"));
    }

    #[test]
    fn private_key_redacts_body_when_footer_missing() {
        // Header-only paste (no END footer) must still swallow the base64 body
        // (re-verify CRITICAL: the optional-footer group matched zero body chars).
        let pem = "key:\n-----BEGIN PRIVATE KEY-----\nMIIBVwIBADANBgkqLEAKEDBODYw0BAQEF\nAAAAAAAAAAAAAAAA\nthen prose";
        let red = crate::redactor::redact(pem);
        assert!(
            !red.contains("LEAKEDBODY"),
            "body leaked when footer absent: {red}"
        );
        assert!(red.contains("then prose"));
    }

    #[test]
    fn db_url_password_with_slash_fully_redacted() {
        // A password containing '/' (un-percent-encoded base64) must not leak
        // (re-verify HIGH: the password class forbade '/').
        let cases = [
            (
                "postgresql://admin:Xy9/aBc+dEf2gHi@host/db",
                "Xy9/aBc+dEf2gHi",
            ),
            ("postgres://user:pa/ss@host/db", "pa/ss"),
            ("redis://default:abc/def/ghi@h:6379", "abc/def/ghi"),
        ];
        for (url, secret) in cases {
            let hits = detect_secrets(url);
            assert!(
                hits.iter().any(|h| h.kind == SecretKind::DatabaseUrl),
                "db url not detected: {url}"
            );
            let red = crate::redactor::redact(url);
            assert!(!red.contains(secret), "slash password leaked: {red}");
        }
    }

    #[test]
    fn detects_trailing_token_access_key_assignment() {
        for s in [
            "aws_secret_access_key=ABCDEFGHIJKLMNOPQRSTUV",
            "GCP_API_KEY=ABCDEFGHIJKLMNOPQRSTUV",
        ] {
            let hits = detect_secrets(s);
            assert!(
                hits.iter().any(|h| h.kind == SecretKind::GenericApiKey),
                "missed trailing-token assignment: {s}"
            );
        }
    }

    #[test]
    fn detects_db_url_credentials() {
        let url = "postgres://user:hunter2pw@localhost:5432/db";
        let hits = detect_secrets(url);
        let db = hits
            .iter()
            .find(|h| h.kind == SecretKind::DatabaseUrl)
            .expect("expected db url match");
        // The WHOLE user:pass segment is captured (not just the password).
        assert_eq!(db.matched, "user:hunter2pw");

        // No password section — must not match.
        let neg = detect_secrets("postgres://localhost:5432/db");
        assert!(neg.iter().all(|h| h.kind != SecretKind::DatabaseUrl));
    }

    #[test]
    fn db_url_password_with_at_does_not_partially_leak() {
        // A password containing `@` must be fully captured up to the LAST `@`.
        let url = "postgres://user:p@sswithat@host/db";
        let hits = detect_secrets(url);
        let db = hits
            .iter()
            .find(|h| h.kind == SecretKind::DatabaseUrl)
            .expect("expected db url match");
        assert_eq!(db.matched, "user:p@sswithat");
        // The redactor must leave NO part of the password behind.
        let red = crate::redactor::redact(url);
        assert!(!red.contains("sswithat"), "password leaked: {red}");
        assert!(red.contains("@host/db"));
    }

    #[test]
    fn detects_postgresql_full_scheme_and_other_schemes() {
        for url in [
            "postgresql://u:longpasswordvalue123@h/db",
            "mongodb+srv://u:longpasswordvalue123@h/db",
            "redis://u:longpasswordvalue123@h:6379",
        ] {
            let hits = detect_secrets(url);
            assert!(
                hits.iter().any(|h| h.kind == SecretKind::DatabaseUrl),
                "missed db scheme: {url}"
            );
        }
    }

    #[test]
    fn detects_modern_openai_project_and_service_keys() {
        // Assembled at compile time (concat!) so the source carries no contiguous
        // secret literal — keeps GitHub push-protection happy for a detector that,
        // by nature, must test real key shapes.
        for key in [
            concat!("sk-", "proj-", "T3BlbkFJabcdefghijklmnop0123456789"),
            concat!("sk-", "svcacct-", "abcdefghijklmnop0123456789XYZ"),
            concat!("sk-", "admin-", "abcdefghijklmnop0123456789XYZ"),
        ] {
            let hits = detect_secrets(key);
            assert!(
                hits.iter().any(|h| h.kind == SecretKind::OpenAIKey),
                "missed modern OpenAI key: {key}"
            );
        }
    }

    #[test]
    fn detects_stripe_keys() {
        for key in [
            concat!("sk", "_live_", "abcdefghijklmnop01234567"),
            concat!("sk", "_test_", "abcdefghijklmnop01234567"),
            concat!("rk", "_live_", "abcdefghijklmnop01234567"),
        ] {
            let hits = detect_secrets(key);
            assert!(
                hits.iter().any(|h| h.kind == SecretKind::StripeKey),
                "missed Stripe key: {key}"
            );
        }
    }

    #[test]
    fn detects_google_and_aws_sts_and_npm() {
        let g = detect_secrets(concat!("AI", "za", "SyA1234567890abcdefghijklmnopqrstuv0"));
        assert!(g.iter().any(|h| h.kind == SecretKind::GoogleApiKey));
        // STS temporary credentials (ASIA prefix) — previously missed.
        let a = detect_secrets("ASIAIOSFODNN7EXAMPLE");
        assert!(a.iter().any(|h| h.kind == SecretKind::AwsAccessKey));
        let n = detect_secrets(&format!("npm_{}", "a".repeat(36)));
        assert!(n.iter().any(|h| h.kind == SecretKind::NpmToken));
    }

    #[test]
    fn detects_slack_webhook_and_bearer_and_password_assignment() {
        let w = detect_secrets(concat!(
            "https://hooks.slack.com/",
            "services/",
            "T00000000/B00000000/abcdefABCDEF1234"
        ));
        assert!(w.iter().any(|h| h.kind == SecretKind::SlackWebhook));
        let b = detect_secrets("Authorization: Bearer abcdefghijklmnop0123456789");
        assert!(b.iter().any(|h| h.kind == SecretKind::BearerToken));
        let p = detect_secrets("password=correcthorsebatterystaple1");
        assert!(p.iter().any(|h| h.kind == SecretKind::GenericApiKey));
    }

    #[test]
    fn empty_input_returns_no_matches() {
        assert!(detect_secrets("").is_empty());
        assert!(detect_secrets("just some harmless prose").is_empty());
    }
}
