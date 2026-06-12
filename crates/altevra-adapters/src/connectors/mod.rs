//! Connector SDK (PLAN-EXTEND §E1) — the modularity core.
//!
//! A `Connector` is a tiny adapter that pulls items from an external tool
//! (Google Calendar via ICS, Gmail via IMAP, Linear via GraphQL, an Obsidian
//! vault, …) and hands them to Altevra as typed [`ConnectorItem`]s. The whole
//! point: any new external tool connects in MINUTES via config, and EVERYTHING
//! it pulls flows through the SAME safety stack the rest of Altevra uses —
//! `guard_text` (secret/PII redaction + classify) → a domain-policy sensitivity
//! floor → persistence into `events` + `object_index` with provenance. Nothing
//! a connector pulls bypasses the gates.
//!
//! Design rules (load-bearing):
//!  - **Disabled by default.** Every connector in `~/.altevra/connectors.toml`
//!    ships `enabled = false`. A connector that is off pulls nothing.
//!  - **Secrets never live in the toml.** Auth values are referenced by a
//!    keyring KEY NAME (`auth_secret`); the value is fetched from the existing
//!    `altevra-secrets` keyring at pull time. The toml only ever holds the
//!    key name, never the secret itself.
//!  - **Guard everything.** Item title + body run through `guard_text` before
//!    they touch the database; embedded tokens are redacted in place and a
//!    fingerprint-only sighting is returned to the caller for audit.
//!  - **Domain floor.** Each item carries a declared [`Domain`]; the persisted
//!    sensitivity is RAISED (never lowered) to that domain's policy floor.

use altevra_core::domain::Domain;
use altevra_core::security::Sensitivity;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod config;
pub mod ics;
pub mod imap;
pub mod ingest;
pub mod linear;
pub mod obsidian;

pub use config::{ConnectorConfig, ConnectorsConfig, DEFAULT_CONNECTORS_TEMPLATE};
pub use ics::IcsConnector;
pub use imap::{ImapConnector, ImapTransport, RecordedImap};
pub use ingest::{ingest_items, IngestOutcome, IngestedItem};
pub use linear::{LinearConnector, LinearTransport, RecordedLinear};
pub use obsidian::ObsidianConnector;

/// How a connector authenticates. `None` connectors need no secret; the other
/// modes name a secret KIND so the config/wizard can prompt correctly. The
/// actual value always lives in the keyring (referenced by key name), never in
/// the toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// No credential required (local file ICS, descriptor-only Obsidian).
    None,
    /// A bearer/api key (Linear).
    ApiKey,
    /// An application-specific password (Gmail/IMAP app password).
    AppPassword,
    /// A private/secret ICS URL (Google Calendar private-ICS; no OAuth).
    IcsUrl,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::None => "none",
            AuthMode::ApiKey => "api_key",
            AuthMode::AppPassword => "app_password",
            AuthMode::IcsUrl => "ics_url",
        }
    }
    /// Whether this mode resolves a secret from the keyring at pull time.
    pub fn needs_secret(&self) -> bool {
        !matches!(self, AuthMode::None)
    }
}

/// Static identity of a connector — what it is and what it touches. Returned by
/// [`Connector::descriptor`]; used by `connector list`, the registry, and the
/// config template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorDescriptor {
    /// Stable lowercase name (config key + tool_records name), e.g. `ics`.
    pub name: String,
    /// Coarse kind label for the daily-value surface, e.g. `calendar`, `email`,
    /// `issues`, `notes`.
    pub kind: String,
    pub auth_mode: AuthMode,
    /// Domains this connector's items land in (R3 — declared up front).
    pub domains: Vec<Domain>,
    /// One-line human description.
    pub description: String,
}

/// Health of a connector — green/red plus a one-line reason. A failing pull
/// flips this red; it never aborts other connectors or other brain jobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorHealth {
    pub name: String,
    /// `green` | `red` | `disabled` | `unconfigured`.
    pub status: String,
    pub detail: String,
}

impl ConnectorHealth {
    pub fn green(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: "green".into(), detail: detail.into() }
    }
    pub fn red(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: "red".into(), detail: detail.into() }
    }
    pub fn disabled(name: &str) -> Self {
        Self { name: name.into(), status: "disabled".into(), detail: "enabled = false".into() }
    }
    pub fn unconfigured(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: "unconfigured".into(), detail: detail.into() }
    }
    pub fn is_green(&self) -> bool {
        self.status == "green"
    }
}

/// Provenance carried by every pulled item (CLAUDE.md §4.3): which connector,
/// the external id, and when the source produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemProvenance {
    pub connector: String,
    pub external_id: String,
    pub ts: DateTime<Utc>,
}

/// The typed payloads a connector can produce. Each variant maps to a coarse
/// object_type at persistence time; all carry a declared domain + provenance via
/// the wrapping [`ConnectorItem`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorPayload {
    CalendarEvent {
        title: String,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
        location: Option<String>,
        notes: Option<String>,
    },
    EmailHeader {
        subject: String,
        from: String,
        date: Option<DateTime<Utc>>,
        /// Capped snippet ONLY — never a full body (privacy doctrine).
        snippet: String,
    },
    Issue {
        title: String,
        state: String,
        url: Option<String>,
        body: Option<String>,
    },
    Note {
        title: String,
        body: String,
    },
}

impl ConnectorPayload {
    /// Coarse object_type used at persistence + in the object_index.
    pub fn object_type(&self) -> &'static str {
        match self {
            ConnectorPayload::CalendarEvent { .. } => "calendar_event",
            ConnectorPayload::EmailHeader { .. } => "email_header",
            ConnectorPayload::Issue { .. } => "issue",
            ConnectorPayload::Note { .. } => "note",
        }
    }

    /// Human title (used in the brief + as the indexed title).
    pub fn title(&self) -> String {
        match self {
            ConnectorPayload::CalendarEvent { title, .. } => title.clone(),
            ConnectorPayload::EmailHeader { subject, .. } => subject.clone(),
            ConnectorPayload::Issue { title, .. } => title.clone(),
            ConnectorPayload::Note { title, .. } => title.clone(),
        }
    }

    /// Full guardable text body (title + payload-specific prose). This is the
    /// text that flows through `guard_text` and becomes the FTS body.
    pub fn guardable_text(&self) -> String {
        match self {
            ConnectorPayload::CalendarEvent { title, location, notes, .. } => {
                let mut s = title.clone();
                if let Some(l) = location {
                    s.push_str(&format!("\nLocation: {l}"));
                }
                if let Some(n) = notes {
                    s.push_str(&format!("\n{n}"));
                }
                s
            }
            ConnectorPayload::EmailHeader { subject, from, snippet, .. } => {
                format!("{subject}\nFrom: {from}\n{snippet}")
            }
            ConnectorPayload::Issue { title, state, url, body } => {
                let mut s = format!("{title}\nState: {state}");
                if let Some(u) = url {
                    s.push_str(&format!("\n{u}"));
                }
                if let Some(b) = body {
                    s.push_str(&format!("\n{b}"));
                }
                s
            }
            ConnectorPayload::Note { title, body } => format!("{title}\n{body}"),
        }
    }
}

/// One pulled item: a typed payload + declared domain + provenance. The domain
/// is what drives the persistence-time sensitivity floor (R3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorItem {
    pub payload: ConnectorPayload,
    pub domain: Domain,
    pub provenance: ItemProvenance,
}

impl ConnectorItem {
    /// A stable, collision-resistant object id: `connector:external_id`. Used as
    /// the `object_index` id so re-pulls are idempotent (INSERT OR REPLACE).
    pub fn object_id(&self) -> String {
        format!("{}:{}", self.provenance.connector, self.provenance.external_id)
    }
}

/// Runtime context passed to [`Connector::pull`]. Carries the resolved config
/// for this connector plus the `now` clock. The secret value (if any) has
/// already been resolved from the keyring by the caller and lives in
/// `auth_value` — the connector never reads the keyring directly.
#[derive(Debug, Clone)]
pub struct ConnectorCtx {
    pub config: config::ConnectorConfig,
    /// Resolved secret value from the keyring (None for `auth_mode = none` or
    /// when the secret is absent). NEVER logged.
    pub auth_value: Option<String>,
    pub now: DateTime<Utc>,
}

/// The connector trait. A connector is stateless; all config arrives via
/// [`ConnectorCtx`]. Implementations MUST NOT read secrets from disk/keyring
/// themselves (the caller resolves `auth_value`) and MUST NOT perform external
/// side effects beyond a read-only pull.
pub trait Connector {
    /// Static identity (name/kind/auth/domains/description).
    fn descriptor(&self) -> ConnectorDescriptor;

    /// Pull items from the source. Read-only. Returns the raw (un-guarded) items;
    /// the ingest path guards + classifies + persists them. A pull failure is an
    /// `Err` — the caller maps it to a red health, never a panic.
    fn pull(&self, ctx: &ConnectorCtx) -> anyhow::Result<Vec<ConnectorItem>>;

    /// Cheap reachability/credential check WITHOUT a full pull. Returns health.
    fn health(&self, ctx: &ConnectorCtx) -> ConnectorHealth;
}

/// The set of reference connectors Altevra ships. Returned in a stable order so
/// `connector list` / the registry seeding is deterministic.
pub fn builtin_connectors() -> Vec<Box<dyn Connector>> {
    vec![
        Box::new(IcsConnector::new()),
        Box::new(ImapConnector::new()),
        Box::new(LinearConnector::new()),
        Box::new(ObsidianConnector::new()),
    ]
}

/// Resolve a builtin connector by name (for `connector sync --name`).
pub fn connector_by_name(name: &str) -> Option<Box<dyn Connector>> {
    builtin_connectors().into_iter().find(|c| c.descriptor().name == name)
}

/// Map a [`Domain`] to its policy sensitivity FLOOR. Mirrors the seeded
/// `domain_policies.default_sensitivity` matrix (migration 024) so connector
/// ingest applies the same floor as the rest of the system WITHOUT a DB round
/// trip per item. Fail-closed: an unknown domain floors at `Confidential`
/// (never `Internal`/`Public`) so a mis-declared domain can't leak.
pub fn domain_sensitivity_floor(domain: &Domain) -> Sensitivity {
    match domain {
        Domain::Public => Sensitivity::Public,
        Domain::Business | Domain::Project => Sensitivity::Internal,
        Domain::Client | Domain::Personal | Domain::Legal | Domain::Financial => {
            Sensitivity::Confidential
        }
        Domain::Relationship | Domain::Health => Sensitivity::Restricted,
        // Unknown / Other (e.g. "comms") → fail-closed Confidential.
        Domain::Other(_) => Sensitivity::Confidential,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial mock connector for the trait round-trip test (no IO).
    struct MockConnector;
    impl Connector for MockConnector {
        fn descriptor(&self) -> ConnectorDescriptor {
            ConnectorDescriptor {
                name: "mock".into(),
                kind: "notes".into(),
                auth_mode: AuthMode::None,
                domains: vec![Domain::Business],
                description: "mock".into(),
            }
        }
        fn pull(&self, ctx: &ConnectorCtx) -> anyhow::Result<Vec<ConnectorItem>> {
            Ok(vec![ConnectorItem {
                payload: ConnectorPayload::Note {
                    title: "hello".into(),
                    body: "world".into(),
                },
                domain: Domain::Business,
                provenance: ItemProvenance {
                    connector: "mock".into(),
                    external_id: "1".into(),
                    ts: ctx.now,
                },
            }])
        }
        fn health(&self, _ctx: &ConnectorCtx) -> ConnectorHealth {
            ConnectorHealth::green("mock", "ok")
        }
    }

    fn ctx() -> ConnectorCtx {
        ConnectorCtx {
            config: ConnectorConfig::default_for("mock", AuthMode::None),
            auth_value: None,
            now: Utc::now(),
        }
    }

    #[test]
    fn trait_round_trip_descriptor_pull_health() {
        let c = MockConnector;
        let d = c.descriptor();
        assert_eq!(d.name, "mock");
        assert_eq!(d.auth_mode, AuthMode::None);
        let items = c.pull(&ctx()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].payload.object_type(), "note");
        assert_eq!(items[0].object_id(), "mock:1");
        assert!(c.health(&ctx()).is_green());
    }

    #[test]
    fn domain_floor_never_under_protects() {
        assert_eq!(domain_sensitivity_floor(&Domain::Public), Sensitivity::Public);
        assert_eq!(domain_sensitivity_floor(&Domain::Business), Sensitivity::Internal);
        assert_eq!(domain_sensitivity_floor(&Domain::Health), Sensitivity::Restricted);
        // unknown ("comms") → fail-closed Confidential, never Internal.
        assert_eq!(
            domain_sensitivity_floor(&Domain::Other("comms".into())),
            Sensitivity::Confidential
        );
    }

    #[test]
    fn auth_mode_secret_semantics() {
        assert!(!AuthMode::None.needs_secret());
        assert!(AuthMode::ApiKey.needs_secret());
        assert!(AuthMode::AppPassword.needs_secret());
        assert!(AuthMode::IcsUrl.needs_secret());
    }

    #[test]
    fn builtins_are_stable_and_named() {
        let names: Vec<String> = builtin_connectors()
            .iter()
            .map(|c| c.descriptor().name)
            .collect();
        assert_eq!(names, vec!["ics", "imap", "linear", "obsidian"]);
        assert!(connector_by_name("linear").is_some());
        assert!(connector_by_name("nope").is_none());
    }
}
