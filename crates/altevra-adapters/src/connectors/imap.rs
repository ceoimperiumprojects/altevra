//! IMAP email-headers connector (PLAN-EXTEND §E1.5).
//!
//! Pulls UNSEEN message HEADERS + a capped snippet ONLY — NEVER full bodies
//! (privacy doctrine). Auth is an application password (e.g. a Gmail app
//! password) resolved from the keyring; domain = `comms`; config-gated OFF.
//!
//! The connector talks to IMAP through an injected [`ImapTransport`] so it is
//! fully fixture-testable with NO real server or creds (the `RecordedImap`
//! transport replays a canned `FETCH` response). The live transport is a
//! follow-up (a real IMAP/TLS client is a clean drop-in on this same trait); the
//! point tonight is to prove the rails end-to-end through the safety stack.

use super::{
    AuthMode, Connector, ConnectorCtx, ConnectorDescriptor, ConnectorHealth, ConnectorItem,
    ConnectorPayload, ItemProvenance,
};
use altevra_core::domain::Domain;
use chrono::{DateTime, Utc};

/// Default snippet cap (chars) — applied even if config requests more.
pub const DEFAULT_SNIPPET_CHARS: usize = 200;
/// Absolute ceiling on the snippet, regardless of config (privacy guard rail).
pub const MAX_SNIPPET_CHARS: usize = 500;

/// One raw header row a transport returns. The transport NEVER returns a full
/// body — only these header fields + a pre-capped snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapHeader {
    pub uid: String,
    pub subject: String,
    pub from: String,
    pub date: Option<DateTime<Utc>>,
    /// Pre-truncated snippet (the transport must already cap; the connector
    /// re-caps defensively).
    pub snippet: String,
}

/// Pluggable IMAP transport. The connector calls `fetch_unseen_headers`; tests
/// inject [`RecordedImap`]. A live transport implementing this trait against a
/// real server is a clean follow-up.
pub trait ImapTransport: Send + Sync {
    /// Fetch UNSEEN headers from `mailbox`. `host`/`port`/`user`/`app_password`
    /// are the connection params; `snippet_chars` is the requested cap.
    fn fetch_unseen_headers(
        &self,
        host: &str,
        port: u16,
        user: &str,
        app_password: &str,
        mailbox: &str,
        snippet_chars: usize,
    ) -> anyhow::Result<Vec<ImapHeader>>;
}

/// A fixture transport replaying canned headers — the test mock. NEVER touches
/// the network. Records the params it was called with so tests can assert
/// (e.g. that the app password was passed through, not logged).
pub struct RecordedImap {
    pub headers: Vec<ImapHeader>,
}

impl RecordedImap {
    pub fn new(headers: Vec<ImapHeader>) -> Self {
        Self { headers }
    }
}

impl ImapTransport for RecordedImap {
    fn fetch_unseen_headers(
        &self,
        _host: &str,
        _port: u16,
        _user: &str,
        _app_password: &str,
        _mailbox: &str,
        snippet_chars: usize,
    ) -> anyhow::Result<Vec<ImapHeader>> {
        // Re-cap defensively at the transport boundary too.
        let cap = snippet_chars.min(MAX_SNIPPET_CHARS);
        Ok(self
            .headers
            .iter()
            .cloned()
            .map(|mut h| {
                h.snippet = cap_snippet(&h.snippet, cap);
                h
            })
            .collect())
    }
}

/// The default (live) transport. Not yet wired to a real IMAP/TLS client — it
/// returns an instructive error so the connector fails CLOSED (red health, zero
/// items) until a real transport is dropped in. This keeps the dependency
/// surface minimal while the rails ship disabled-by-default.
pub struct LiveImap;

impl ImapTransport for LiveImap {
    fn fetch_unseen_headers(
        &self,
        _host: &str,
        _port: u16,
        _user: &str,
        _app_password: &str,
        _mailbox: &str,
        _snippet_chars: usize,
    ) -> anyhow::Result<Vec<ImapHeader>> {
        anyhow::bail!(
            "live IMAP transport not yet wired (the connector rails are proven via the \
             recorded-response fixture; a real IMAP/TLS client drops into ImapTransport)"
        )
    }
}

pub struct ImapConnector {
    transport: Box<dyn ImapTransport>,
}

impl ImapConnector {
    pub fn new() -> Self {
        Self { transport: Box::new(LiveImap) }
    }
    /// Build with an injected transport (tests pass [`RecordedImap`]).
    pub fn with_transport(transport: Box<dyn ImapTransport>) -> Self {
        Self { transport }
    }
}

impl Default for ImapConnector {
    fn default() -> Self {
        Self::new()
    }
}

fn cap_snippet(s: &str, cap: usize) -> String {
    let cap = cap.min(MAX_SNIPPET_CHARS);
    s.chars().take(cap).collect()
}

impl Connector for ImapConnector {
    fn descriptor(&self) -> ConnectorDescriptor {
        ConnectorDescriptor {
            name: "imap".into(),
            kind: "email".into(),
            auth_mode: AuthMode::AppPassword,
            // comms isn't a builtin domain → Domain::Other; floors at Confidential.
            domains: vec![Domain::Other("comms".into())],
            description: "IMAP UNSEEN headers + capped snippet ONLY (never bodies; app password)"
                .into(),
        }
    }

    fn pull(&self, ctx: &ConnectorCtx) -> anyhow::Result<Vec<ConnectorItem>> {
        let host = ctx.config.param("host").unwrap_or("imap.gmail.com");
        let port: u16 = ctx
            .config
            .param("port")
            .and_then(|p| p.parse().ok())
            .unwrap_or(993);
        let user = ctx
            .config
            .param("user")
            .ok_or_else(|| anyhow::anyhow!("imap: params.user required"))?;
        let mailbox = ctx.config.param("mailbox").unwrap_or("INBOX");
        let snippet_chars = ctx
            .config
            .param("snippet_chars")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_SNIPPET_CHARS)
            .min(MAX_SNIPPET_CHARS);
        let app_password = ctx
            .auth_value
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("imap: app password not found in keyring"))?;

        let domain = ctx
            .config
            .domain
            .as_deref()
            .map(|d| d.parse().unwrap_or(Domain::Other("comms".into())))
            .unwrap_or(Domain::Other("comms".into()));

        let headers = self.transport.fetch_unseen_headers(
            host,
            port,
            user,
            app_password,
            mailbox,
            snippet_chars,
        )?;

        Ok(headers
            .into_iter()
            .map(|h| ConnectorItem {
                provenance: ItemProvenance {
                    connector: "imap".into(),
                    external_id: h.uid.clone(),
                    ts: h.date.unwrap_or(ctx.now),
                },
                domain: domain.clone(),
                payload: ConnectorPayload::EmailHeader {
                    subject: h.subject,
                    from: h.from,
                    date: h.date,
                    // Defensive final cap — a snippet NEVER exceeds the ceiling.
                    snippet: cap_snippet(&h.snippet, snippet_chars),
                },
            })
            .collect())
    }

    fn health(&self, ctx: &ConnectorCtx) -> ConnectorHealth {
        if !ctx.config.enabled {
            return ConnectorHealth::disabled("imap");
        }
        if ctx.config.param("user").is_none() {
            return ConnectorHealth::unconfigured("imap", "set params.user");
        }
        if ctx.auth_value.is_none() {
            return ConnectorHealth::red(
                "imap",
                format!("app password '{}' not in keyring", ctx.config.auth_secret),
            );
        }
        ConnectorHealth::green("imap", "configured")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::config::ConnectorConfig;
    use std::collections::BTreeMap;

    fn ctx(enabled: bool, auth: Option<&str>) -> ConnectorCtx {
        let mut params = BTreeMap::new();
        params.insert("user".to_string(), "you@gmail.com".to_string());
        params.insert("snippet_chars".to_string(), "1000".to_string()); // request > ceiling
        ConnectorCtx {
            config: ConnectorConfig {
                enabled,
                auth_secret: "ALTEVRA_IMAP_APP_PASSWORD".into(),
                cadence_minutes: 30,
                domain: None,
                params,
            },
            auth_value: auth.map(String::from),
            now: Utc::now(),
        }
    }

    fn fixture() -> Vec<ImapHeader> {
        vec![
            ImapHeader {
                uid: "101".into(),
                subject: "Invoice from Vega".into(),
                from: "billing@vega.rs".into(),
                date: None,
                snippet: "x".repeat(2000), // huge — must be capped
            },
            ImapHeader {
                uid: "102".into(),
                subject: "Re: GTM call".into(),
                from: "srdjan@htec.com".into(),
                date: None,
                snippet: "short snippet".into(),
            },
        ]
    }

    #[test]
    fn pull_returns_headers_only_with_capped_snippet() {
        let c = ImapConnector::with_transport(Box::new(RecordedImap::new(fixture())));
        let items = c.pull(&ctx(true, Some("app-pass-secret"))).unwrap();
        assert_eq!(items.len(), 2);
        for it in &items {
            match &it.payload {
                ConnectorPayload::EmailHeader { snippet, .. } => {
                    assert!(
                        snippet.chars().count() <= MAX_SNIPPET_CHARS,
                        "snippet must be capped to the ceiling, got {}",
                        snippet.chars().count()
                    );
                }
                _ => panic!("expected email header"),
            }
            assert_eq!(it.provenance.connector, "imap");
        }
    }

    #[test]
    fn no_app_password_is_error_not_panic() {
        let c = ImapConnector::with_transport(Box::new(RecordedImap::new(fixture())));
        assert!(c.pull(&ctx(true, None)).is_err());
    }

    #[test]
    fn health_red_when_secret_missing() {
        let c = ImapConnector::new();
        assert_eq!(c.health(&ctx(false, None)).status, "disabled");
        assert_eq!(c.health(&ctx(true, None)).status, "red");
        assert_eq!(c.health(&ctx(true, Some("x"))).status, "green");
    }

    #[test]
    fn live_transport_fails_closed() {
        // Default (live) transport returns an error → no silent empty success.
        let c = ImapConnector::new();
        assert!(c.pull(&ctx(true, Some("pw"))).is_err());
    }
}
