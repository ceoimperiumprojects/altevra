//! SessionStart context injection (PLAN-ALIVE §P2) — the "alive moment".
//!
//! This module owns the two PURE halves of the injection pipeline (no DB):
//!
//!  1. **The (tool × transport) matrix** — [`session_start_transport`] is the
//!     ONE place that decides which channel a tool's session-start context
//!     travels on. Claude Code consumes the hook `additionalContext` field;
//!     Hermes pulls the same content via the MCP `get_agent_bootstrap_packet`;
//!     Cursor pulls via `altevra context --session-block`; **Codex gets
//!     NOTHING** — its hook stdout is user-visible and clobbers the TUI.
//!
//!  2. **Rendering + token budget** — [`render_session_context_block`] turns
//!     pre-gated [`SessionContextData`] into the injected markdown block and
//!     hard-truncates it to [`SESSION_BLOCK_TOKEN_BUDGET`] (1–2K tokens,
//!     pinned — NOT the context_packet default of 8000).
//!
//! The DB half (gather + ExposureGate + `exposure_decisions` audit) lives in
//! `altevra_bootstrap::session_context` — this crate stays db-free.

use serde::{Deserialize, Serialize};

/// Pinned token budget for the SessionStart block (§P2.5). chars/4 heuristic.
pub const SESSION_BLOCK_TOKEN_BUDGET: usize = 2000;

/// chars/4 token heuristic — the same estimate `retrieval.rs` / `prompts.rs` use.
pub fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

/// Which channel carries session-start context for a tool (§P2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartTransport {
    /// Claude Code: hook stdout is injected as context — emit EXACTLY ONE
    /// `hookSpecificOutput.additionalContext` JSON document.
    HookAdditionalContext,
    /// Hermes: no hook channel — the same content rides the MCP
    /// `get_agent_bootstrap_packet` (`available_tools` + `session_context`).
    BootstrapPacket,
    /// Cursor + unknown tools: pull via `altevra context --session-block`.
    /// The hook (if any) keeps the legacy `{"session_id": …}` stdout.
    PullCli,
    /// Codex: NOTHING on stdout — its hook output is user-visible and
    /// clobbers the TUI. The session id goes to stderr only.
    Nothing,
}

/// THE (tool × transport) decision point — keyed by the hook's `--tool`.
/// Every emitter (hook, MCP, CLI) consults this one function (§P2.1).
pub fn session_start_transport(tool: &str) -> SessionStartTransport {
    match tool.trim().to_lowercase().as_str() {
        "claude" | "claude_code" | "claudecode" | "claude-code" => {
            SessionStartTransport::HookAdditionalContext
        }
        "codex" | "openai-codex" => SessionStartTransport::Nothing,
        "hermes" => SessionStartTransport::BootstrapPacket,
        // cursor + anything unknown: pull fallback, legacy hook stdout.
        _ => SessionStartTransport::PullCli,
    }
}

/// A one-line tool summary for injection / the bootstrap packet (§P2 #7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSummary {
    pub name: String,
    pub kind: String,
    /// Canonical one-line invocation (e.g. `imperium-crawl <cmd>`).
    pub invocation: String,
}

/// Pre-gated, render-ready session context. Every string in here has ALREADY
/// passed `ExposureGate::decide` + guard redaction in the gather layer — the
/// renderer never sees an unfiltered item.
#[derive(Debug, Clone, Default)]
pub struct SessionContextData {
    /// Active goal titles (guarded + gated).
    pub goals: Vec<String>,
    /// Last N decision titles (gated; Restricted/high-water/Unscanned excluded).
    pub decisions: Vec<String>,
    /// Open proposal one-liners (review queue; guarded + gated).
    pub proposals: Vec<String>,
    /// Tool Register counts by kind, e.g. `[("binary", 580), ("skill", 20)]`.
    pub tool_counts: Vec<(String, usize)>,
    /// Curated/seeded tools (capped — NOT all 612).
    pub curated_tools: Vec<ToolSummary>,
    /// Total registered tools (for the "X more" long-tail line).
    pub tools_total: usize,
}

impl SessionContextData {
    pub fn is_empty(&self) -> bool {
        self.goals.is_empty()
            && self.decisions.is_empty()
            && self.proposals.is_empty()
            && self.tools_total == 0
    }
}

/// Render the curated tool list lines (shared by the session block and the
/// prompt builder's Tool Register layer).
pub fn render_tool_lines(tools: &[ToolSummary], more: usize) -> String {
    let mut s = String::new();
    for t in tools {
        s.push_str(&format!("- {} ({}): {}\n", t.name, t.kind, t.invocation));
    }
    if more > 0 {
        s.push_str(&format!(
            "… {more} more — query `get_capabilities` (MCP) or `altevra tool list`.\n"
        ));
    }
    s
}

/// Render the compact `=== ALTEVRA TOOL REGISTER ===` summary (§P2 #3):
/// counts by kind + curated tools by name with a one-line invocation; the
/// long tail is one "X more" pointer line.
pub fn render_tool_register_block(data: &SessionContextData) -> String {
    if data.tools_total == 0 {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("=== ALTEVRA TOOL REGISTER ===\n");
    let counts = data
        .tool_counts
        .iter()
        .map(|(k, n)| format!("{k}: {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    s.push_str(&format!(
        "{} tool(s) registered ({counts}).\n",
        data.tools_total
    ));
    let more = data.tools_total.saturating_sub(data.curated_tools.len());
    s.push_str(&render_tool_lines(&data.curated_tools, more));
    s
}

/// Render the full SessionStart block. Empty data → empty string (errors in
/// the gather layer degrade to empty sections, so an all-failed assembly is
/// the empty block — fail-open for availability, §P2.4). The result is
/// hard-truncated to [`SESSION_BLOCK_TOKEN_BUDGET`].
pub fn render_session_context_block(data: &SessionContextData) -> String {
    if data.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("=== ALTEVRA SESSION CONTEXT ===\n");
    if !data.goals.is_empty() {
        s.push_str("\n## Active goals\n");
        for g in &data.goals {
            s.push_str(&format!("- {g}\n"));
        }
    }
    if !data.decisions.is_empty() {
        s.push_str("\n## Recent decisions\n");
        for d in &data.decisions {
            s.push_str(&format!("- {d}\n"));
        }
    }
    if !data.proposals.is_empty() {
        s.push_str("\n## Open proposals (review queue)\n");
        for p in &data.proposals {
            s.push_str(&format!("- {p}\n"));
        }
    }
    let tools = render_tool_register_block(data);
    if !tools.is_empty() {
        s.push('\n');
        s.push_str(&tools);
    }
    truncate_to_token_budget(s, SESSION_BLOCK_TOKEN_BUDGET)
}

/// Truncate a rendered block to a token budget (chars/4) on LINE boundaries,
/// appending a truncation marker. The marker is budgeted too — the final
/// string is always ≤ `budget` tokens.
pub fn truncate_to_token_budget(block: String, budget: usize) -> String {
    if estimate_tokens(&block) <= budget {
        return block;
    }
    const MARKER: &str = "… (truncated to session token budget)\n";
    let max_chars = (budget * 4).saturating_sub(MARKER.len());
    let mut out = String::with_capacity(max_chars + MARKER.len());
    for line in block.lines() {
        // +1 for the newline this line will carry.
        if out.len() + line.len() + 1 > max_chars {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(MARKER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_matrix_is_tool_times_transport() {
        // §P2.1 — the locked matrix.
        assert_eq!(
            session_start_transport("claude-code"),
            SessionStartTransport::HookAdditionalContext
        );
        assert_eq!(
            session_start_transport("Claude"),
            SessionStartTransport::HookAdditionalContext
        );
        // Codex: NOTHING — user-visible stdout clobbers the TUI.
        assert_eq!(session_start_transport("codex"), SessionStartTransport::Nothing);
        assert_eq!(
            session_start_transport("openai-codex"),
            SessionStartTransport::Nothing
        );
        assert_eq!(
            session_start_transport("hermes"),
            SessionStartTransport::BootstrapPacket
        );
        assert_eq!(session_start_transport("cursor"), SessionStartTransport::PullCli);
        // unknown tools default to the pull fallback, never to injection.
        assert_eq!(
            session_start_transport("future-tool"),
            SessionStartTransport::PullCli
        );
    }

    fn sample_data() -> SessionContextData {
        SessionContextData {
            goals: vec!["2 paying Simple Surplus clients".into()],
            decisions: vec!["ONE canonical DB".into(), "working_dir on session+turn".into()],
            proposals: vec!["skill: tighten import dedup (tier1)".into()],
            tool_counts: vec![("binary".into(), 600), ("skill".into(), 10), ("cli".into(), 2)],
            curated_tools: vec![ToolSummary {
                name: "imperium-crawl".into(),
                kind: "cli".into(),
                invocation: "imperium-crawl <cmd>".into(),
            }],
            tools_total: 612,
        }
    }

    #[test]
    fn block_contains_goals_decisions_and_tool_register() {
        let block = render_session_context_block(&sample_data());
        assert!(block.contains("=== ALTEVRA SESSION CONTEXT ==="));
        assert!(block.contains("2 paying Simple Surplus clients"));
        assert!(block.contains("ONE canonical DB"));
        assert!(block.contains("tighten import dedup"));
        assert!(block.contains("=== ALTEVRA TOOL REGISTER ==="));
        assert!(block.contains("612 tool(s) registered"));
        assert!(block.contains("imperium-crawl (cli): imperium-crawl <cmd>"));
        // long tail is ONE pointer line, never 612 entries.
        assert!(block.contains("… 611 more — query `get_capabilities`"));
    }

    #[test]
    fn empty_data_renders_empty_block() {
        assert_eq!(render_session_context_block(&SessionContextData::default()), "");
    }

    #[test]
    fn block_is_truncated_to_token_budget() {
        // §P2.5 gate: budget ≤ 2K tokens asserted, even with absurd input.
        let mut data = sample_data();
        for i in 0..2000 {
            data.goals
                .push(format!("goal number {i} with a reasonably long descriptive title"));
        }
        let block = render_session_context_block(&data);
        assert!(
            estimate_tokens(&block) <= SESSION_BLOCK_TOKEN_BUDGET,
            "block must fit the pinned 2K budget, got {} tokens",
            estimate_tokens(&block)
        );
        assert!(block.contains("truncated to session token budget"));
        // truncation is line-bounded: every kept line is intact.
        assert!(block.starts_with("=== ALTEVRA SESSION CONTEXT ==="));
    }

    #[test]
    fn truncate_no_op_under_budget() {
        let s = "short\n".to_string();
        assert_eq!(truncate_to_token_budget(s.clone(), 100), s);
    }
}
