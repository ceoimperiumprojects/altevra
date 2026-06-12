//! Linear connector (PLAN-EXTEND §E1.5).
//!
//! Pulls the API-key viewer's open issues via Linear's GraphQL API → surfaced
//! as [`ConnectorPayload::Issue`] (domain = `project`). Like the IMAP connector,
//! it talks GraphQL through an injected [`LinearTransport`] so it is fully
//! fixture-testable against a RECORDED response with no real key. The live
//! transport posts to the GraphQL endpoint with the api key in the
//! `Authorization` header (never logged).

use super::{
    AuthMode, Connector, ConnectorCtx, ConnectorDescriptor, ConnectorHealth, ConnectorItem,
    ConnectorPayload, ItemProvenance,
};
use altevra_core::domain::Domain;

/// The GraphQL query for the viewer's open issues. Kept minimal (id/title/state/
/// url) — Altevra surfaces titles, not full issue bodies.
pub const VIEWER_ISSUES_QUERY: &str = r#"{ viewer { assignedIssues(filter: { state: { type: { neq: "completed" } } }, first: 50) { nodes { id title url state { name type } } } } }"#;

/// One issue a transport returns (already extracted from the GraphQL JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearIssue {
    pub id: String,
    pub title: String,
    pub state: String,
    pub url: Option<String>,
}

/// Pluggable GraphQL transport. Tests inject [`RecordedLinear`].
pub trait LinearTransport: Send + Sync {
    /// POST `query` to `endpoint` with the api key; return the raw JSON response.
    fn post_graphql(&self, endpoint: &str, api_key: &str, query: &str)
        -> anyhow::Result<serde_json::Value>;
}

/// Fixture transport returning a recorded GraphQL response (the test mock).
pub struct RecordedLinear {
    pub response: serde_json::Value,
}

impl RecordedLinear {
    pub fn new(response: serde_json::Value) -> Self {
        Self { response }
    }
    /// Convenience: build a recorded response from a list of (id,title,state).
    pub fn from_issues(issues: &[(&str, &str, &str)]) -> Self {
        let nodes: Vec<serde_json::Value> = issues
            .iter()
            .map(|(id, title, state)| {
                serde_json::json!({
                    "id": id,
                    "title": title,
                    "url": format!("https://linear.app/issue/{id}"),
                    "state": { "name": state, "type": "started" }
                })
            })
            .collect();
        Self {
            response: serde_json::json!({
                "data": { "viewer": { "assignedIssues": { "nodes": nodes } } }
            }),
        }
    }
}

impl LinearTransport for RecordedLinear {
    fn post_graphql(
        &self,
        _endpoint: &str,
        _api_key: &str,
        _query: &str,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(self.response.clone())
    }
}

/// Live transport — POSTs to the Linear GraphQL endpoint (blocking; pull() is
/// sync). The api key goes in the `Authorization` header and is never logged.
pub struct LiveLinear;

impl LinearTransport for LiveLinear {
    fn post_graphql(
        &self,
        endpoint: &str,
        api_key: &str,
        query: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("Altevra/connector linear")
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let resp = client
            .post(endpoint)
            .header("Authorization", api_key)
            .json(&serde_json::json!({ "query": query }))
            .send()?;
        if !resp.status().is_success() {
            anyhow::bail!("linear graphql HTTP {}", resp.status());
        }
        Ok(resp.json()?)
    }
}

pub struct LinearConnector {
    transport: Box<dyn LinearTransport>,
}

impl LinearConnector {
    pub fn new() -> Self {
        Self { transport: Box::new(LiveLinear) }
    }
    pub fn with_transport(transport: Box<dyn LinearTransport>) -> Self {
        Self { transport }
    }
}

impl Default for LinearConnector {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the viewer-issues GraphQL response into [`LinearIssue`]s. Tolerant of
/// missing fields; returns an empty vec on a shape mismatch.
pub fn parse_viewer_issues(resp: &serde_json::Value) -> Vec<LinearIssue> {
    let nodes = resp
        .get("data")
        .and_then(|d| d.get("viewer"))
        .and_then(|v| v.get("assignedIssues"))
        .and_then(|a| a.get("nodes"))
        .and_then(|n| n.as_array());
    let Some(nodes) = nodes else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|n| {
            let id = n.get("id").and_then(|v| v.as_str())?.to_string();
            let title = n
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("(untitled)")
                .to_string();
            let state = n
                .get("state")
                .and_then(|s| s.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let url = n.get("url").and_then(|v| v.as_str()).map(String::from);
            Some(LinearIssue { id, title, state, url })
        })
        .collect()
}

impl Connector for LinearConnector {
    fn descriptor(&self) -> ConnectorDescriptor {
        ConnectorDescriptor {
            name: "linear".into(),
            kind: "issues".into(),
            auth_mode: AuthMode::ApiKey,
            domains: vec![Domain::Project],
            description: "Linear viewer open issues (GraphQL; api key)".into(),
        }
    }

    fn pull(&self, ctx: &ConnectorCtx) -> anyhow::Result<Vec<ConnectorItem>> {
        let endpoint = ctx
            .config
            .param("endpoint")
            .unwrap_or("https://api.linear.app/graphql");
        let api_key = ctx
            .auth_value
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("linear: api key not found in keyring"))?;
        let domain = ctx
            .config
            .domain
            .as_deref()
            .map(|d| d.parse().unwrap_or(Domain::Project))
            .unwrap_or(Domain::Project);

        let resp = self
            .transport
            .post_graphql(endpoint, api_key, VIEWER_ISSUES_QUERY)?;
        let issues = parse_viewer_issues(&resp);

        Ok(issues
            .into_iter()
            .map(|i| ConnectorItem {
                provenance: ItemProvenance {
                    connector: "linear".into(),
                    external_id: i.id.clone(),
                    ts: ctx.now,
                },
                domain: domain.clone(),
                payload: ConnectorPayload::Issue {
                    title: i.title,
                    state: i.state,
                    url: i.url,
                    body: None,
                },
            })
            .collect())
    }

    fn health(&self, ctx: &ConnectorCtx) -> ConnectorHealth {
        if !ctx.config.enabled {
            return ConnectorHealth::disabled("linear");
        }
        if ctx.auth_value.is_none() {
            return ConnectorHealth::red(
                "linear",
                format!("api key '{}' not in keyring", ctx.config.auth_secret),
            );
        }
        ConnectorHealth::green("linear", "configured")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::config::ConnectorConfig;
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn ctx(enabled: bool, auth: Option<&str>) -> ConnectorCtx {
        ConnectorCtx {
            config: ConnectorConfig {
                enabled,
                auth_secret: "ALTEVRA_LINEAR_API_KEY".into(),
                cadence_minutes: 120,
                domain: None,
                params: BTreeMap::new(),
            },
            auth_value: auth.map(String::from),
            now: Utc::now(),
        }
    }

    #[test]
    fn pull_parses_recorded_graphql_fixture() {
        let recorded = RecordedLinear::from_issues(&[
            ("ISS-1", "Fix ICS timezone bug", "In Progress"),
            ("ISS-2", "Ship connector SDK", "Todo"),
        ]);
        let c = LinearConnector::with_transport(Box::new(recorded));
        let items = c.pull(&ctx(true, Some("lin_api_xxx"))).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0].payload {
            ConnectorPayload::Issue { title, state, url, .. } => {
                assert_eq!(title, "Fix ICS timezone bug");
                assert_eq!(state, "In Progress");
                assert!(url.is_some());
            }
            _ => panic!("expected issue"),
        }
        assert_eq!(items[0].provenance.external_id, "ISS-1");
    }

    #[test]
    fn malformed_response_returns_empty() {
        let bad = RecordedLinear::new(serde_json::json!({"errors": [{"message": "nope"}]}));
        let c = LinearConnector::with_transport(Box::new(bad));
        let items = c.pull(&ctx(true, Some("k"))).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn no_key_errors_and_health_red() {
        let c = LinearConnector::with_transport(Box::new(RecordedLinear::from_issues(&[])));
        assert!(c.pull(&ctx(true, None)).is_err());
        assert_eq!(c.health(&ctx(true, None)).status, "red");
        assert_eq!(c.health(&ctx(true, Some("k"))).status, "green");
        assert_eq!(c.health(&ctx(false, None)).status, "disabled");
    }
}
