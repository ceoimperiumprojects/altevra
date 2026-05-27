//! MCP tools for v0.3.7.5 Research v2 — discover_feed, github_trending,
//! web_search, project_research.

use crate::server::McpResponse;
use serde_json::Value;

pub fn handle_discover_feed(id: Value, args: &Value) -> McpResponse {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let auto_promote = args
        .get("auto_promote")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if url.is_empty() {
        return McpResponse::error(id, -32602, "url required");
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let page = altevra_research::scrape_url(url).await?;
        let feed_links = altevra_research::discover::extract_feed_links(url, &page.html);
        let outbound = altevra_research::discover::extract_outbound_links(url, &page.html);
        let promising = altevra_research::discover::filter_promising_blog_links(&outbound);

        let mut promoted = 0usize;
        if auto_promote {
            let cfg_path = altevra_research::feeds::FeedConfig::default_path();
            let mut cfg = if cfg_path.exists() {
                altevra_research::feeds::FeedConfig::load(&cfg_path)?
            } else {
                altevra_research::default_feeds()
            };
            for f in &feed_links {
                let id = sanitize_feed_id(f);
                if cfg.find(&id).is_some() {
                    continue;
                }
                let _ = cfg.add(altevra_research::feeds::FeedSource {
                    id,
                    name: f.clone(),
                    url: f.clone(),
                    kind: altevra_research::feeds::FeedKind::Rss,
                    category: "auto-discovered".into(),
                    trust_weight: 0.5,
                    enabled: true,
                    fetch_interval_minutes: 180,
                });
                promoted += 1;
            }
            cfg.save(&cfg_path)?;
        }

        Ok(serde_json::json!({
            "url": url,
            "status": page.status,
            "feed_links": feed_links,
            "promising_outbound": promising,
            "auto_promote": auto_promote,
            "promoted_count": promoted,
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

pub fn handle_github_trending(id: Value, args: &Value) -> McpResponse {
    let language = args
        .get("language")
        .and_then(|v| v.as_str())
        .map(String::from);
    let since = args
        .get("since")
        .and_then(|v| v.as_str())
        .unwrap_or("daily")
        .to_string();
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(25) as usize;

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        use altevra_research::sources::github_trending::{GitHubTrendingSource, TrendingPeriod};
        use altevra_research::sources::{FetchCtx, SourceProvider};
        let period = match since.as_str() {
            "weekly" => TrendingPeriod::Weekly,
            "monthly" => TrendingPeriod::Monthly,
            _ => TrendingPeriod::Daily,
        };
        let source = GitHubTrendingSource::new(language.clone(), period);
        let items = source.fetch(&FetchCtx::default()).await?;
        let slice: Vec<_> = items.into_iter().take(limit).collect();
        Ok(serde_json::json!({
            "language": language,
            "since": since,
            "count": slice.len(),
            "items": slice,
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

pub fn handle_web_search(id: Value, args: &Value) -> McpResponse {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let provider = args
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("ddg");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    if query.is_empty() {
        return McpResponse::error(id, -32602, "query required");
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        use altevra_research::sources::web_search::{WebSearchProviderKind, WebSearchSource};
        use altevra_research::sources::{FetchCtx, SourceProvider};
        let kind = match provider {
            "brave" => WebSearchProviderKind::Brave,
            "exa" => WebSearchProviderKind::Exa,
            _ => WebSearchProviderKind::DuckDuckGo,
        };
        let mut source = WebSearchSource::new(query.to_string()).with_chain(vec![kind]);
        if let Ok(k) = std::env::var("BRAVE_API_KEY") {
            source = source.with_brave(k);
        }
        if let Ok(k) = std::env::var("EXA_API_KEY") {
            source = source.with_exa(k);
        }
        let items = source
            .fetch(&FetchCtx {
                limit,
                ..Default::default()
            })
            .await?;
        Ok(serde_json::json!({
            "query": query,
            "provider": provider,
            "count": items.len(),
            "items": items,
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

pub fn handle_project_research(id: Value, args: &Value) -> McpResponse {
    let project_id = args
        .get("project_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if project_id.is_empty() {
        return McpResponse::error(id, -32602, "project_id required");
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        use altevra_research::projects::ProjectAgent;
        let id_path = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
            .join(".imperium")
            .join("identity")
            .join("projects.yaml");
        if !id_path.exists() {
            return Ok(serde_json::json!({
                "project_id": project_id,
                "error": "no imperium identity file",
                "items": [],
            }));
        }
        let agents = ProjectAgent::load_all(&id_path)?;
        let agent = agents
            .into_iter()
            .find(|a| a.project_id == project_id)
            .ok_or_else(|| anyhow::anyhow!("no agent for id"))?;
        Ok(serde_json::json!({
            "project_id": agent.project_id,
            "priority": agent.priority,
            "keywords": agent.keywords,
            "queries": agent.queries,
            "daily_budget_queries": agent.daily_budget_queries,
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

fn sanitize_feed_id(url: &str) -> String {
    let host = url
        .splitn(4, '/')
        .nth(2)
        .unwrap_or("feed")
        .replace('.', "-");
    let now = chrono::Utc::now().timestamp() % 100_000;
    format!("{host}-{now}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_feed_missing_url_errors() {
        let resp = handle_discover_feed(serde_json::json!(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn web_search_missing_query_errors() {
        let resp = handle_web_search(serde_json::json!(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn project_research_missing_id_errors() {
        let resp = handle_project_research(serde_json::json!(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn sanitize_feed_id_extracts_host() {
        let id = sanitize_feed_id("https://example.com/rss");
        assert!(id.starts_with("example-com-"));
    }

    #[tokio::test]
    async fn project_research_unknown_id_errors_or_empty() {
        let args = serde_json::json!({"project_id": "no-such-project-xyz"});
        let resp = handle_project_research(serde_json::json!(1), &args);
        // Either an error or a graceful empty result is acceptable.
        if let Some(result) = &resp.result {
            // If no imperium file present, returns "no imperium identity file".
            let _ = result;
        } else {
            assert!(resp.error.is_some());
        }
    }
}
