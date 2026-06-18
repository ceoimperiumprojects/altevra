//! Claude via the `claude -p` headless CLI (Claude Code SDK), using Pavle's
//! Claude subscription — NO API key required.
//!
//! Isolation is critical: the brain calls this on every insight/extraction job,
//! and a naive `claude -p` would (a) load the global ~/.claude/CLAUDE.md
//! personality and (b) fire the Altevra hooks in ~/.claude/settings.json —
//! which would recurse straight back into Altevra capture. We prevent both:
//!   --settings '{}'        → no hooks, no MCP servers (no recursion)
//!   --system-prompt <sys>  → replaces the global system prompt entirely
//!   --exclude-dynamic-system-prompt-sections
//!   current_dir(/tmp)      → no project CLAUDE.md is picked up
//!
//! Role routing (set by the ModelRouter / config): cheap_worker → Haiku,
//! strong_reasoner → Sonnet.

use crate::chat::{ChatMessage, ChatOpts, ChatRole};
use crate::provider::ChatProvider;
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// A chat provider that shells out to `claude -p`.
pub struct ClaudeCliProvider {
    model: String,
    id: String,
}

impl ClaudeCliProvider {
    pub fn new(model: impl Into<String>) -> Self {
        let model = model.into();
        Self {
            id: format!("claude-cli:{model}"),
            model,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl ChatProvider for ClaudeCliProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_local(&self) -> bool {
        // Runs through Anthropic's cloud (via the CLI), so NOT local. The router
        // therefore never routes LocalPrivate/personal work here (SI-7).
        false
    }

    async fn complete(&self, messages: &[ChatMessage], _opts: &ChatOpts) -> anyhow::Result<String> {
        // Split the system prompt from the conversation. claude -p takes one
        // system prompt via --system-prompt and the rest on stdin.
        let mut system = String::new();
        let mut convo = String::new();
        for m in messages {
            match m.role {
                ChatRole::System => {
                    if !system.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&m.content);
                }
                ChatRole::User => {
                    convo.push_str(&m.content);
                    convo.push('\n');
                }
                ChatRole::Assistant => {
                    convo.push_str("\n[previous assistant reply]:\n");
                    convo.push_str(&m.content);
                    convo.push('\n');
                }
                ChatRole::Tool => {
                    convo.push_str("\n[tool result]:\n");
                    convo.push_str(&m.content);
                    convo.push('\n');
                }
            }
        }
        if system.trim().is_empty() {
            system.push_str("You are a precise assistant. Answer directly, with no preamble.");
        }

        let mut child = Command::new("claude")
            .arg("-p")
            .arg("--model")
            .arg(&self.model)
            .arg("--output-format")
            .arg("text")
            .arg("--system-prompt")
            .arg(system.trim())
            .arg("--settings")
            .arg("{}") // kill hooks + MCP so this can't recurse into Altevra
            .arg("--exclude-dynamic-system-prompt-sections")
            .current_dir("/tmp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(convo.trim().as_bytes()).await?;
            stdin.shutdown().await?;
        }

        let out = child.wait_with_output().await?;
        if !out.status.success() {
            anyhow::bail!(
                "claude -p ({}) failed: {}",
                self.model,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}
