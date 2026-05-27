use altevra_mcp::server::{McpRequest, McpResponse, McpServer};
use clap::Args;
use serde_json::Value;
use std::io::{BufRead, Write};

#[derive(Args)]
pub struct ServeArgs {
    /// Project identifier passed to bootstrap tools
    #[arg(long)]
    pub project: Option<String>,

    /// Path to Altevra vault (directory containing 06-skills/, 07-capabilities/)
    #[arg(long, default_value = ".")]
    pub vault: std::path::PathBuf,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let vault = args
        .vault
        .canonicalize()
        .unwrap_or_else(|_| args.vault.clone());
    let server = McpServer::new(version).with_vault(&vault);

    // MCP stdio: one JSON-RPC object per line in, one per line out.
    // Notifications (no "id" key) are handled silently — no response sent.
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    eprintln!(
        "altevra MCP server v{version} ready (stdio). Project: {} | Vault: {}",
        args.project.as_deref().unwrap_or("(none)"),
        vault.display()
    );

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        // Parse raw first so we can inspect the "id" key before deserializing.
        let raw: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = McpResponse::error(Value::Null, -32700, format!("Parse error: {e}"));
                writeln!(out, "{}", serde_json::to_string(&err)?)?;
                out.flush()?;
                continue;
            }
        };

        // Notifications have no "id" key — handle but don't respond.
        let has_id = raw
            .as_object()
            .map(|o| o.contains_key("id"))
            .unwrap_or(false);

        let req = McpRequest {
            jsonrpc: raw["jsonrpc"].as_str().unwrap_or("2.0").to_string(),
            id: if has_id {
                raw["id"].clone()
            } else {
                Value::Null
            },
            method: raw["method"].as_str().unwrap_or("").to_string(),
            params: raw.get("params").cloned(),
        };

        // "initialized" notification: acknowledged, no response.
        if req.method == "initialized" && !has_id {
            continue;
        }

        let response = server.handle(req);

        if has_id {
            writeln!(out, "{}", serde_json::to_string(&response)?)?;
            out.flush()?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_mcp::server::McpServer;

    fn server() -> McpServer {
        McpServer::new("0.1.0")
    }

    #[test]
    fn test_initialize_response() {
        let req = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "claude-code", "version": "1.0.0"},
                "capabilities": {}
            })),
        };
        let resp = server().handle(req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "altevra");
    }

    #[test]
    fn test_tools_list_after_init() {
        let s = server();
        // initialize first
        let init = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "initialize".to_string(),
            params: None,
        };
        assert!(s.handle(init).error.is_none());

        // then list tools
        let list = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(2),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = s.handle(list);
        assert!(resp.error.is_none());
        let tools = &resp.result.unwrap()["tools"];
        assert!(tools.as_array().unwrap().len() >= 3);
    }

    #[test]
    fn test_notification_has_no_id() {
        // "initialized" notification should not crash anything
        let req = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: Value::Null,
            method: "initialized".to_string(),
            params: None,
        };
        // McpServer.handle() will return "method not found" for initialized
        // but the stdio loop checks has_id before sending — test only that handle doesn't panic
        let _ = server().handle(req);
    }
}
