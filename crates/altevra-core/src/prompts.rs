//! Layered system prompt generator (Architecture v5 §14, §15).
//!
//! Builds a structured agent system prompt from many sources:
//! safety, Altevra rules, tool behavior, project, current task,
//! recent updates, skills, and an output protocol.
//!
//! Layer priority (highest first): safety > Altevra rules > tool behavior >
//! project instructions > current task/goal > last updates > skills > output protocol.

use serde::{Deserialize, Serialize};

use crate::updates::{Importance, UpdateFeedItem};

/// Note: we re-declare a minimal mirror of skills::ParsedSkill here so that
/// `altevra-core` does not need to depend on `altevra-skills` (would create a
/// cycle in the workspace dependency graph). The CLI / MCP layers translate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSkill {
    pub slug: String,
    pub version: String,
    pub title: String,
    pub description: Option<String>,
}

impl PromptSkill {
    pub fn new(
        slug: impl Into<String>,
        version: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            slug: slug.into(),
            version: version.into(),
            title: title.into(),
            description: None,
        }
    }
}

/// Input to the layered prompt builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInput {
    pub tool_name: String,
    pub project: Option<String>,
    pub current_task: Option<String>,
    pub current_goal: Option<String>,
    pub recent_updates: Vec<UpdateFeedItem>,
    pub skills: Vec<PromptSkill>,
    pub project_readme: Option<String>,
    pub altevra_version: String,
}

impl PromptInput {
    /// Construct a minimal input — useful for callers that only know the tool.
    pub fn new(tool_name: impl Into<String>, altevra_version: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            project: None,
            current_task: None,
            current_goal: None,
            recent_updates: vec![],
            skills: vec![],
            project_readme: None,
            altevra_version: altevra_version.into(),
        }
    }
}

/// Result of the layered prompt builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptOutput {
    pub system_prompt: String,
    pub layer_count: usize,
    pub token_estimate: usize,
    pub layers_included: Vec<String>,
}

/// Default cap on how many recent updates render into the prompt body.
pub const DEFAULT_UPDATES_LIMIT: usize = 5;

/// Maximum size in chars of an inlined project README excerpt.
const PROJECT_README_MAX_CHARS: usize = 1500;

/// Build the layered system prompt with no tool-specific routing.
///
/// This always produces the universal layered stack. Most callers
/// should use [`build_for_tool`] instead.
pub fn build_system_prompt(input: PromptInput) -> PromptOutput {
    let tool_norm = normalize_tool(&input.tool_name);

    let mut layers: Vec<(String, String)> = Vec::new();

    // 1. Safety / sensitivity (always)
    layers.push(("Safety".to_string(), safety_layer()));

    // 2. Altevra operating rules (always)
    layers.push((
        "Altevra Operating Rules".to_string(),
        altevra_rules_layer(&input.altevra_version),
    ));

    // 3. Tool behavior (always — even if unknown, we say so)
    layers.push((
        format!("Tool Behavior — {tool_norm}"),
        tool_behavior_layer(&tool_norm),
    ));

    // 4. Project instructions
    if let Some(project) = &input.project {
        layers.push((
            format!("Project: {project}"),
            project_layer(project, input.project_readme.as_deref()),
        ));
    }

    // 5. Current task / goal
    if input.current_task.is_some() || input.current_goal.is_some() {
        layers.push((
            "Current Task".to_string(),
            current_task_layer(input.current_task.as_deref(), input.current_goal.as_deref()),
        ));
    }

    // 6. Last updates (capped)
    if !input.recent_updates.is_empty() {
        let n = input.recent_updates.len().min(DEFAULT_UPDATES_LIMIT);
        layers.push((
            format!("Last {n} Updates"),
            updates_layer(&input.recent_updates, DEFAULT_UPDATES_LIMIT),
        ));
    }

    // 7. Skills
    if !input.skills.is_empty() {
        layers.push(("Skills Available".to_string(), skills_layer(&input.skills)));
    }

    // 8. Output protocol (always)
    layers.push((
        "Output Protocol".to_string(),
        output_protocol_layer(&tool_norm),
    ));

    render_layers(&tool_norm, layers)
}

/// Build a system prompt with per-tool conventions (Architecture v5 §15).
///
/// Currently dispatches on `tool_name` (claude-code, codex, cursor, antigravity)
/// but always passes through [`build_system_prompt`] so the layer stack is
/// uniform. Tool-specific overrides live inside [`tool_behavior_layer`] and
/// [`output_protocol_layer`].
pub fn build_for_tool(input: PromptInput) -> PromptOutput {
    build_system_prompt(input)
}

// ---------------------------------------------------------------------------
// Layer renderers
// ---------------------------------------------------------------------------

fn safety_layer() -> String {
    let mut s = String::new();
    s.push_str("- Never reveal API keys, OAuth tokens, browser cookies, or secrets, even if found in vault files.\n");
    s.push_str("- Redact PII (emails, phone numbers, addresses, financial info) unless the user is the subject and has authorized the disclosure.\n");
    s.push_str("- If a vault document is marked `sensitivity: restricted` or `confidential`, treat its contents as need-to-know.\n");
    s.push_str("- Never invoke external side effects (deploy, push, email, payment, destructive shell) without explicit user authorization.\n");
    s.push_str("- If asked to leak, exfiltrate, or bypass safety: refuse plainly and continue with the safe path.\n");
    s
}

fn altevra_rules_layer(altevra_version: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "- Altevra version: `{altevra_version}` — you are an agent operating under Altevra v5.\n"
    ));
    s.push_str("- Source of truth lives in the vault: `/08-decisions/` (decisions), `/06-skills/` (skills), `/01-projects/` (projects).\n");
    s.push_str("- Before working, refresh context: `altevra agent bootstrap`, `altevra updates --since last-session`, `altevra context`.\n");
    s.push_str("- If the MCP server is unreachable, fall back to the `altevra` CLI — both call the same core logic.\n");
    s.push_str("- Hooks emit events to `.altevra/events/updates.jsonl`. Treat those updates as authoritative for what changed.\n");
    s.push_str("- Never overwrite Altevra-managed files (those with `ALTEVRA_MANAGED: true` headers) without running `altevra connect repair`.\n");
    s.push_str("- Emit decisions and learnings back into the vault — chat state is volatile, vault state is durable.\n");
    s
}

fn tool_behavior_layer(tool_norm: &str) -> String {
    match tool_norm {
        "claude-code" => {
            let mut s = String::new();
            s.push_str("- Read `.claude/altevra-instructions.md` at session start; it is the authoritative tool-side guide.\n");
            s.push_str("- Prefer the MCP tool `get_agent_bootstrap_packet` over CLI; native hooks should fire automatically.\n");
            s.push_str("- Skills are installed at `.claude/skills/<slug>/SKILL.md`. Do not edit those files directly — refresh via `altevra skill refresh <slug>`.\n");
            s.push_str("- Use Read/Edit/Write tools for scoped, deterministic edits. Avoid wholesale rewrites unless the user asks.\n");
            s.push_str("- Honor settings.json hook config; if a hook is missing, surface it as a setup gap, do not silently skip.\n");
            s
        }
        "codex" => {
            let mut s = String::new();
            s.push_str("- Codex does not autoload Claude-style hooks. Use the `altevra` CLI fallback at session start:\n");
            s.push_str("  - `altevra agent bootstrap --tool codex --project <name> --json`\n");
            s.push_str("  - `altevra updates --since last-session --json`\n");
            s.push_str("  - `altevra context --project <name> --json`\n");
            s.push_str("- Treat `AGENTS.md` in the repo root as the canonical tool-side guide.\n");
            s.push_str("- Do not assume MCP is configured — assume CLI-only until verified.\n");
            s.push_str(
                "- For any external side effects (push, deploy, network calls), ask first.\n",
            );
            s
        }
        "cursor" => {
            let mut s = String::new();
            s.push_str("- Cursor is rules-driven. The Altevra rule lives at `.cursor/rules/altevra.mdc` — managed by `altevra connect`.\n");
            s.push_str("- Do not manually edit Altevra-managed rule files; run `altevra connect repair` to regenerate.\n");
            s.push_str("- Use the `altevra` CLI as fallback when MCP is unavailable.\n");
            s.push_str("- Cursor's @-mentions can pull in vault files — prefer pulling source-of-truth docs over speculation.\n");
            s
        }
        "antigravity" => {
            let mut s = String::new();
            s.push_str("- Antigravity uses adapter-driven hook scaffolding. Do not assume hook/config/skill/MCP support until the adapter dossier confirms it.\n");
            s.push_str("- Follow the SDK conventions defined in the project's `.altevra/antigravity.toml` (if present).\n");
            s.push_str(
                "- Prefer the CLI fallback: `altevra agent bootstrap --tool antigravity --json`.\n",
            );
            s.push_str(
                "- Treat capability gaps as `report_capability_gap` calls, not silent failures.\n",
            );
            s
        }
        other => {
            format!(
                "- Tool `{other}` does not have a dedicated Altevra adapter yet.\n\
                 - Use the universal CLI fallback: `altevra agent bootstrap --tool {other} --json`.\n\
                 - Report any tool-specific behavior you discover via `report_capability_gap`.\n"
            )
        }
    }
}

fn project_layer(project: &str, readme: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Active project: **{project}**. Vault path: `01-projects/{project}/`.\n\n",
    ));
    if let Some(body) = readme {
        s.push_str("Project README excerpt:\n\n");
        s.push_str("```markdown\n");
        s.push_str(&truncate(body.trim(), PROJECT_README_MAX_CHARS));
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("```\n");
    } else {
        s.push_str("(No `01-projects/<name>/README.md` was provided. Read it before making project-wide changes.)\n");
    }
    s
}

fn current_task_layer(task: Option<&str>, goal: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(g) = goal {
        s.push_str(&format!("Goal: {g}\n"));
    }
    if let Some(t) = task {
        s.push_str(&format!("Task: {t}\n"));
    }
    s.push_str(
        "\nWork toward the task above. If it conflicts with a higher-priority Altevra rule or safety constraint, stop and flag it.\n",
    );
    s
}

fn updates_layer(updates: &[UpdateFeedItem], limit: usize) -> String {
    let n = updates.len().min(limit);
    let mut s = String::new();
    for u in updates.iter().take(n) {
        let imp = match u.importance {
            Importance::Critical => "critical",
            Importance::High => "high",
            Importance::Medium => "medium",
            Importance::Low => "low",
            Importance::Noise => "noise",
        };
        s.push_str(&format!("- [{imp}] {} — {}\n", u.title, u.short_summary));
    }
    if updates.len() > n {
        s.push_str(&format!(
            "- … {} more updates suppressed (limit={limit}).\n",
            updates.len() - n
        ));
    }
    s
}

fn skills_layer(skills: &[PromptSkill]) -> String {
    let mut s = String::new();
    for sk in skills {
        let desc = sk.description.as_deref().unwrap_or("");
        if desc.is_empty() {
            s.push_str(&format!("- `{}` v{} — {}\n", sk.slug, sk.version, sk.title));
        } else {
            s.push_str(&format!(
                "- `{}` v{} — {} — {desc}\n",
                sk.slug, sk.version, sk.title
            ));
        }
    }
    s
}

fn output_protocol_layer(tool_norm: &str) -> String {
    let mut s = String::new();
    s.push_str("- Reply in Markdown unless the user asks for another format.\n");
    s.push_str("- For multi-step plans, use numbered lists. For options, use bullet lists.\n");
    s.push_str("- Cite vault sources by path (e.g. `06-skills/altevra-core.md`, `08-decisions/2026-05-27-foundation.md`).\n");
    s.push_str("- When you save state to the vault, name the file you wrote.\n");
    match tool_norm {
        "claude-code" => {
            s.push_str("- Match Claude Code conventions: keep responses concise; prefer tool calls over long preambles.\n");
        }
        "codex" => {
            s.push_str(
                "- Match Codex conventions: explicit plans before code edits; surface diffs.\n",
            );
        }
        "cursor" => {
            s.push_str("- Match Cursor conventions: respect cursor-rules; reference @-mentioned files when relevant.\n");
        }
        "antigravity" => {
            s.push_str("- Match Antigravity conventions: surface adapter capability state in your replies.\n");
        }
        _ => {}
    }
    s
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn normalize_tool(s: &str) -> String {
    let t = s.trim().to_lowercase();
    match t.as_str() {
        "claude" | "claude_code" | "claudecode" | "claude-code" => "claude-code".to_string(),
        "openai-codex" | "codex" => "codex".to_string(),
        "cursor" | "cursor-ai" => "cursor".to_string(),
        "antigravity" | "anti-gravity" => "antigravity".to_string(),
        other => other.to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n... (truncated)");
    out
}

fn render_layers(tool_norm: &str, layers: Vec<(String, String)>) -> PromptOutput {
    let mut out = String::new();
    out.push_str(&format!(
        "# Altevra-Managed System Prompt — {tool_norm}\n\n"
    ));

    let mut titles = Vec::with_capacity(layers.len());
    for (title, body) in &layers {
        out.push_str(&format!("## {title}\n"));
        let trimmed = body.trim_end_matches('\n');
        out.push_str(trimmed);
        out.push_str("\n\n");
        titles.push(title.clone());
    }
    // Strip the final blank line for tidiness.
    while out.ends_with("\n\n") {
        out.pop();
    }
    let token_estimate = out.len() / 4;
    PromptOutput {
        system_prompt: out,
        layer_count: titles.len(),
        token_estimate,
        layers_included: titles,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Sensitivity;
    use crate::updates::{Importance, UpdateFeedItem};
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_update(title: &str, importance: Importance) -> UpdateFeedItem {
        UpdateFeedItem {
            id: Uuid::new_v4(),
            event_id: Uuid::new_v4(),
            project_id: None,
            update_type: "skill_updated".to_string(),
            importance,
            title: title.to_string(),
            short_summary: format!("Summary for {title}"),
            agent_summary: None,
            affected_entities: serde_json::Value::Array(vec![]),
            recommended_agent_action: None,
            visible_to_agents: true,
            sensitivity: Sensitivity::Internal,
            created_at: Utc::now(),
        }
    }

    fn base_input(tool: &str) -> PromptInput {
        PromptInput::new(tool, "0.1.0")
    }

    #[test]
    fn build_for_claude_code() {
        let out = build_for_tool(base_input("claude-code"));
        assert!(out
            .system_prompt
            .contains("Altevra-Managed System Prompt — claude-code"));
        assert!(out
            .system_prompt
            .contains(".claude/altevra-instructions.md"));
        assert!(out.layers_included.iter().any(|l| l == "Safety"));
    }

    #[test]
    fn build_for_codex() {
        let out = build_for_tool(base_input("codex"));
        assert!(out.system_prompt.contains("— codex"));
        // Codex must mention CLI fallback
        assert!(out
            .system_prompt
            .contains("altevra agent bootstrap --tool codex"));
        assert!(out.system_prompt.contains("AGENTS.md"));
    }

    #[test]
    fn build_for_cursor() {
        let out = build_for_tool(base_input("cursor"));
        assert!(out.system_prompt.contains("— cursor"));
        assert!(out.system_prompt.contains(".cursor/rules/altevra.mdc"));
    }

    #[test]
    fn build_for_antigravity() {
        let out = build_for_tool(base_input("antigravity"));
        assert!(out.system_prompt.contains("— antigravity"));
        assert!(out.system_prompt.contains("adapter"));
    }

    #[test]
    fn safety_layer_always_present() {
        for tool in &[
            "claude-code",
            "codex",
            "cursor",
            "antigravity",
            "unknown-tool",
        ] {
            let out = build_system_prompt(base_input(tool));
            assert!(
                out.layers_included.iter().any(|l| l == "Safety"),
                "missing safety layer for {tool}"
            );
            assert!(out.system_prompt.contains("Never reveal API keys"));
        }
    }

    #[test]
    fn project_layer_included_when_project_given() {
        let mut input = base_input("claude-code");
        input.project = Some("altevra".to_string());
        input.project_readme = Some("# Altevra\n\nLocal-first Agent OS.".to_string());
        let out = build_system_prompt(input);
        assert!(out.layers_included.iter().any(|l| l == "Project: altevra"));
        assert!(out.system_prompt.contains("Local-first Agent OS"));
    }

    #[test]
    fn project_layer_skipped_when_no_project() {
        let out = build_system_prompt(base_input("claude-code"));
        assert!(out
            .layers_included
            .iter()
            .all(|l| !l.starts_with("Project:")));
    }

    #[test]
    fn updates_layer_respects_limit() {
        let mut input = base_input("claude-code");
        // 7 updates → expect cap at DEFAULT_UPDATES_LIMIT (5)
        for i in 0..7 {
            input
                .recent_updates
                .push(sample_update(&format!("update-{i}"), Importance::High));
        }
        let out = build_system_prompt(input);

        // Heading should show "Last 5 Updates"
        assert!(out.layers_included.iter().any(|l| l == "Last 5 Updates"));

        // Body should mention 2 suppressed
        assert!(out.system_prompt.contains("2 more updates suppressed"));

        // Should only render first 5 lines of items
        let bullet_lines: Vec<_> = out
            .system_prompt
            .lines()
            .filter(|l| l.starts_with("- [high] update-"))
            .collect();
        assert_eq!(bullet_lines.len(), 5);
    }

    #[test]
    fn skills_layer_renders_metadata() {
        let mut input = base_input("claude-code");
        let mut sk = PromptSkill::new("altevra-core", "0.6.0", "Altevra Core");
        sk.description = Some("Agent OS core operations".to_string());
        input.skills.push(sk);
        let out = build_system_prompt(input);

        assert!(out.layers_included.iter().any(|l| l == "Skills Available"));
        assert!(out.system_prompt.contains("`altevra-core` v0.6.0"));
        assert!(out.system_prompt.contains("Agent OS core operations"));
    }

    #[test]
    fn token_estimate_is_roughly_len_over_4() {
        let out = build_system_prompt(base_input("claude-code"));
        let expected = out.system_prompt.len() / 4;
        assert_eq!(out.token_estimate, expected);
        assert!(out.token_estimate > 0);
    }

    #[test]
    fn empty_input_still_produces_valid_prompt() {
        let out = build_system_prompt(PromptInput::new("", ""));
        // Even an empty tool name should produce a prompt with safety + rules + output protocol
        assert!(out.layer_count >= 4);
        assert!(out
            .system_prompt
            .contains("# Altevra-Managed System Prompt"));
        assert!(out.system_prompt.contains("## Safety"));
        assert!(out.system_prompt.contains("## Output Protocol"));
    }

    #[test]
    fn current_task_layer_appears_when_task_given() {
        let mut input = base_input("claude-code");
        input.current_task = Some("Ship v0.2".to_string());
        input.current_goal = Some("Hit 100 daily users".to_string());
        let out = build_system_prompt(input);
        assert!(out.layers_included.iter().any(|l| l == "Current Task"));
        assert!(out.system_prompt.contains("Ship v0.2"));
        assert!(out.system_prompt.contains("Hit 100 daily users"));
    }

    #[test]
    fn unknown_tool_routes_to_fallback() {
        let out = build_for_tool(base_input("randomtool"));
        assert!(out
            .system_prompt
            .contains("does not have a dedicated Altevra adapter"));
        assert!(out
            .system_prompt
            .contains("altevra agent bootstrap --tool randomtool"));
    }

    #[test]
    fn layer_order_is_safety_first() {
        let mut input = base_input("claude-code");
        input.project = Some("altevra".to_string());
        input.current_task = Some("t".into());
        input
            .recent_updates
            .push(sample_update("x", Importance::High));
        input.skills.push(PromptSkill::new("s", "0.1.0", "S"));
        let out = build_system_prompt(input);
        // Safety always at index 0
        assert_eq!(out.layers_included[0], "Safety");
        // Output protocol always last
        assert_eq!(out.layers_included.last().unwrap(), "Output Protocol");
    }
}
