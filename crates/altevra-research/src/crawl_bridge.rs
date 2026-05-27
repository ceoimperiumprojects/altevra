//! Bridge to imperium-crawl CLI for hard-mode scraping (login walls, JS heavy,
//! paywall, anti-bot). Light Altevra scraper handles the 80% common path; this
//! module is the escape hatch for the remaining 20%.
//!
//! Wire: shell-out to `npx -y imperium-crawl <command>` (TS v2.6.1 stable).
//! Override via `IMPERIUM_CRAWL_PATH` env to point at a local checkout.
//!
//! Fails closed with an actionable error message when imperium-crawl is not
//! installed — does not auto-install or modify $PATH.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct CrawlOpts {
    pub timeout_secs: Option<u64>,
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlResult {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    pub html: Option<String>,
    pub status_code: Option<u16>,
}

/// Probe whether imperium-crawl is available. Returns the binary spec we'd use.
pub fn imperium_crawl_spec() -> ImperiumCrawlSpec {
    if let Ok(p) = std::env::var("IMPERIUM_CRAWL_PATH") {
        if !p.is_empty() {
            return ImperiumCrawlSpec::LocalNode(PathBuf::from(p));
        }
    }
    ImperiumCrawlSpec::Npx
}

#[derive(Debug, Clone)]
pub enum ImperiumCrawlSpec {
    /// `node <path>` — IMPERIUM_CRAWL_PATH override pointing at dist/index.js
    LocalNode(PathBuf),
    /// `npx -y imperium-crawl` — default; downloads on first run
    Npx,
}

impl ImperiumCrawlSpec {
    pub fn command(&self) -> Command {
        match self {
            Self::LocalNode(p) => {
                let mut c = Command::new("node");
                c.arg(p);
                c
            }
            Self::Npx => {
                let mut c = Command::new("npx");
                c.arg("-y").arg("imperium-crawl");
                c
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::LocalNode(p) => format!("node {}", p.display()),
            Self::Npx => "npx -y imperium-crawl".to_string(),
        }
    }
}

/// Shell out to `imperium-crawl scrape --url <url> --json` and parse the result.
pub async fn crawl_via_imperium(url: &str, opts: &CrawlOpts) -> anyhow::Result<CrawlResult> {
    let spec = imperium_crawl_spec();
    let mut cmd = spec.command();
    cmd.arg("scrape")
        .arg("--url")
        .arg(url)
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for extra in &opts.extra_args {
        cmd.arg(extra);
    }

    let timeout = std::time::Duration::from_secs(opts.timeout_secs.unwrap_or(120));
    let mut child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!(
            "could not invoke imperium-crawl ({}): {e}. Install via `npm install -g imperium-crawl` or set IMPERIUM_CRAWL_PATH.",
            spec.describe()
        )
    })?;

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();

    let waited = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            stdout.read_to_end(&mut out_buf),
            stderr.read_to_end(&mut err_buf),
            child.wait()
        )
    })
    .await
    .map_err(|_| anyhow::anyhow!("imperium-crawl timed out after {:?}", timeout))?;

    let (_, _, status) = waited?;
    if !status.success() {
        let stderr_text = String::from_utf8_lossy(&err_buf);
        anyhow::bail!(
            "imperium-crawl exited with status {status}: {}",
            stderr_text.trim()
        );
    }

    parse_scrape_output(&out_buf, url)
}

/// Shell out to `imperium-crawl interact --url <url> --actions @<recipe>.json --json`.
pub async fn crawl_with_login(
    url: &str,
    recipe: &std::path::Path,
    opts: &CrawlOpts,
) -> anyhow::Result<CrawlResult> {
    if !recipe.exists() {
        anyhow::bail!("login recipe not found: {}", recipe.display());
    }
    let spec = imperium_crawl_spec();
    let mut cmd = spec.command();
    cmd.arg("interact")
        .arg("--url")
        .arg(url)
        .arg("--actions")
        .arg(format!("@{}", recipe.display()))
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for extra in &opts.extra_args {
        cmd.arg(extra);
    }

    let timeout = std::time::Duration::from_secs(opts.timeout_secs.unwrap_or(180));
    let mut child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!("could not invoke imperium-crawl ({}): {e}", spec.describe())
    })?;

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let waited = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            stdout.read_to_end(&mut out_buf),
            stderr.read_to_end(&mut err_buf),
            child.wait()
        )
    })
    .await
    .map_err(|_| anyhow::anyhow!("imperium-crawl interact timed out"))?;

    let (_, _, status) = waited?;
    if !status.success() {
        anyhow::bail!(
            "imperium-crawl interact failed: {}",
            String::from_utf8_lossy(&err_buf).trim()
        );
    }
    parse_scrape_output(&out_buf, url)
}

fn parse_scrape_output(stdout: &[u8], url: &str) -> anyhow::Result<CrawlResult> {
    // imperium-crawl --json shapes vary by command; we accept any JSON that
    // has a `text` or `markdown` field.
    let value: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|e| anyhow::anyhow!("imperium-crawl returned non-JSON: {e}"))?;

    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(String::from);
    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("markdown").and_then(|v| v.as_str()))
        .or_else(|| value.get("content").and_then(|v| v.as_str()))
        .map(String::from)
        .unwrap_or_default();
    let html = value.get("html").and_then(|v| v.as_str()).map(String::from);
    let status_code = value
        .get("status")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .or_else(|| {
            value
                .get("status_code")
                .and_then(|v| v.as_u64())
                .map(|n| n as u16)
        });

    Ok(CrawlResult {
        url: url.to_string(),
        title,
        text,
        html,
        status_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults_to_npx() {
        std::env::remove_var("IMPERIUM_CRAWL_PATH");
        let s = imperium_crawl_spec();
        assert!(matches!(s, ImperiumCrawlSpec::Npx));
        assert_eq!(s.describe(), "npx -y imperium-crawl");
    }

    #[test]
    fn spec_picks_up_env_override() {
        std::env::set_var("IMPERIUM_CRAWL_PATH", "/tmp/dist/index.js");
        let s = imperium_crawl_spec();
        assert!(matches!(s, ImperiumCrawlSpec::LocalNode(_)));
        assert!(s.describe().contains("/tmp/dist/index.js"));
        std::env::remove_var("IMPERIUM_CRAWL_PATH");
    }

    #[test]
    fn parse_scrape_output_minimal() {
        let raw = br#"{"title": "Hello", "text": "Body", "status": 200}"#;
        let r = parse_scrape_output(raw, "https://example.com").unwrap();
        assert_eq!(r.title.as_deref(), Some("Hello"));
        assert_eq!(r.text, "Body");
        assert_eq!(r.status_code, Some(200));
    }

    #[test]
    fn parse_scrape_output_accepts_markdown_field() {
        let raw = br##"{"markdown": "# Heading"}"##;
        let r = parse_scrape_output(raw, "https://example.com").unwrap();
        assert_eq!(r.text, "# Heading");
    }

    #[test]
    fn parse_scrape_output_errors_on_non_json() {
        let r = parse_scrape_output(b"not json", "https://x");
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn crawl_with_login_errors_on_missing_recipe() {
        let r = crawl_with_login(
            "https://example.com",
            std::path::Path::new("/tmp/__nope__.json"),
            &CrawlOpts::default(),
        )
        .await;
        assert!(r.is_err());
    }
}
