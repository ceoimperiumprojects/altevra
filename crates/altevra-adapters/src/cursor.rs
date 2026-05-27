//! Cursor adapter — Cursor IDE integration.
//!
//! Generates four managed files in the repo:
//!
//! 1. `AGENTS.md` — Markdown instructions at repo root (Cursor 2026 reads this
//!    plus `.cursorrules`/`.cursor/rules/` automatically).
//! 2. `.cursor/mcp.json` — project-scoped MCP server registration.
//! 3. `.cursor/hooks.json` — 21 lifecycle hooks pointing at
//!    `altevra hook-handle <event>` (the v0.3.1 stdin-pipe handler).
//! 4. `.cursor/rules/altevra.mdc` — auto-load rule that always-attaches Altevra
//!    context to the chat panel.
//!
//! Hook events covered (21 total):
//!   agent:      beforeAgentStart, afterAgentStop
//!   prompt:     beforeSubmitPrompt, afterSubmitPrompt
//!   shell:      beforeShellExecution, afterShellExecution
//!   read:       beforeReadFile, afterReadFile
//!   edit:       beforeFileEdit, afterFileEdit
//!   create:     beforeFileCreate, afterFileCreate
//!   delete:     beforeFileDelete, afterFileDelete
//!   mcp:        beforeMCPExecution, afterMCPExecution
//!   attach:     onAttachment
//!   chat:       onMessage
//!   apply:      onCodeApply
//!   lint:       onLintError
//!   stop:       stop
//!
//! Polarity gotcha (vs. Antigravity MCP JSON):
//!   * Cursor MCP JSON uses `disabled: false` (matches Antigravity)
//!   * Hook commands receive a JSON payload on stdin which `altevra
//!     hook-handle` parses.

use crate::base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallPlanFile, InstallResult,
    InstructionRenderInput, RepairPlan, ToolAdapter, VerifyResult,
};
use altevra_hooks::universal::UniversalHook;
use altevra_skills::parser::ParsedSkill;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing::info;

const ADAPTER_VERSION: &str = "0.1.0";

/// (cursor_event_name, altevra_canonical_event) — one entry per supported hook.
/// 21 entries total. Keep this list sorted alphabetically by cursor event.
const HOOK_EVENTS: &[(&str, &str)] = &[
    ("afterAgentStop", "session_end"),
    ("afterFileCreate", "post_tool_use"),
    ("afterFileEdit", "post_tool_use"),
    ("afterFileDelete", "post_tool_use"),
    ("afterMCPExecution", "post_tool_use"),
    ("afterReadFile", "post_tool_use"),
    ("afterShellExecution", "post_tool_use"),
    ("afterSubmitPrompt", "response_received"),
    ("beforeAgentStart", "session_start"),
    ("beforeFileCreate", "pre_tool_use"),
    ("beforeFileDelete", "pre_tool_use"),
    ("beforeFileEdit", "pre_tool_use"),
    ("beforeMCPExecution", "pre_tool_use"),
    ("beforeReadFile", "pre_tool_use"),
    ("beforeShellExecution", "pre_tool_use"),
    ("beforeSubmitPrompt", "user_prompt_submit"),
    ("onAttachment", "file_changed"),
    ("onCodeApply", "post_tool_use"),
    ("onLintError", "notification"),
    ("onMessage", "tool_call_observed"),
    ("stop", "session_end"),
];

pub struct CursorAdapter;

impl CursorAdapter {
    pub fn new() -> Self {
        Self
    }

    /// AGENTS.md body — same pattern as Codex/Antigravity, with HTML-comment
    /// managed header so drift detection works.
    fn agents_md_content(project: Option<&str>) -> String {
        let project_line = project
            .map(|p| format!("Project: {p}"))
            .unwrap_or_else(|| "Project: (set ALTEVRA_PROJECT env var)".to_string());
        format!(
            r#"<!-- ALTEVRA_MANAGED: true -->
<!-- source: 07-capabilities/agent-tools.yaml -->
<!-- generated_by: altevra -->
<!-- adapter: cursor -->

# Altevra Context

{project_line}

## Session Startup

At the start of every Cursor agent session, call:

```bash
altevra agent bootstrap --tool cursor --project ${{ALTEVRA_PROJECT}} --json
```

Or via MCP: `get_agent_bootstrap_packet(tool_name="cursor", project="${{ALTEVRA_PROJECT}}")`

## Quick CLI Reference

```bash
altevra updates --project ${{ALTEVRA_PROJECT}} --json
altevra skill check --all
altevra context --project ${{ALTEVRA_PROJECT}} --json
```

## Rules

- Check `altevra updates` before working.
- Never edit ALTEVRA_MANAGED files manually.
- Hooks under .cursor/hooks.json record every tool call into Altevra's session recorder.
"#
        )
    }

    /// Build `.cursor/hooks.json` containing 21 lifecycle hooks. Each command
    /// pipes a JSON envelope to `altevra hook-handle <event>` via stdin (the
    /// CLI tolerates empty stdin and synthesises a minimal record).
    fn hooks_json_content() -> String {
        // Build hooks map deterministically (alphabetical key order).
        let mut hooks_obj = serde_json::Map::new();
        for (cursor_event, altevra_event) in HOOK_EVENTS {
            let cmd = format!(
                "altevra hook-handle {altevra_event} --tool cursor --source cursor:{cursor_event}"
            );
            let entry = serde_json::json!([{
                "type": "command",
                "command": cmd,
            }]);
            hooks_obj.insert((*cursor_event).to_string(), entry);
        }

        // Outer doc: managed sentinel keys siblings to hooks.
        let body_json = serde_json::json!({
            "_altevra_managed": true,
            "_altevra_source": "07-capabilities/hooks.yaml",
            "_altevra_adapter": "cursor",
            "_altevra_version": ADAPTER_VERSION,
            "version": 1,
            "hooks": serde_json::Value::Object(hooks_obj),
        });

        // Pretty-print so humans can read the file. Stable key order via BTreeMap-ish
        // semantics — serde_json::Map preserves insertion order, and we inserted
        // alphabetically above. The outer object iteration order is also stable
        // for serde_json.
        let body = serde_json::to_string_pretty(&body_json).expect("valid JSON");

        // Compute checksum of body and stash it in a sibling key. We rebuild the
        // doc once more so the checksum appears in the JSON (versus a comment,
        // since JSON can't have comments).
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        let checksum = hex::encode(hasher.finalize());

        // Reinsert checksum into the doc and re-render.
        let mut final_obj = body_json.as_object().cloned().unwrap();
        final_obj.insert(
            "_altevra_checksum".into(),
            serde_json::Value::String(checksum),
        );
        // Reorder keys: managed sentinels first, then version, hooks.
        let mut ordered = serde_json::Map::new();
        for k in [
            "_altevra_managed",
            "_altevra_source",
            "_altevra_adapter",
            "_altevra_version",
            "_altevra_checksum",
            "version",
            "hooks",
        ] {
            if let Some(v) = final_obj.remove(k) {
                ordered.insert(k.to_string(), v);
            }
        }
        let final_doc = serde_json::Value::Object(ordered);
        serde_json::to_string_pretty(&final_doc).expect("valid JSON") + "\n"
    }

    /// `.cursor/mcp.json` — project-scoped MCP wiring.
    fn mcp_json_content() -> String {
        let body = r#"{
  "_altevra_managed": true,
  "_altevra_source": "07-capabilities/mcp.yaml",
  "_altevra_adapter": "cursor",
  "_altevra_version": "0.1.0",
  "_altevra_note": "ALTEVRA_MANAGED: true — do not edit manually.",
  "mcpServers": {
    "altevra": {
      "command": "altevra",
      "args": ["serve"],
      "disabled": false
    }
  }
}
"#;
        body.to_string()
    }

    /// `.cursor/rules/altevra.mdc` — always-applied rule that attaches Altevra
    /// CLI cheatsheet to every Cursor chat. MDC = Markdown with YAML frontmatter.
    fn rule_mdc_content(project: Option<&str>) -> String {
        let project = project.unwrap_or("(unset)");
        format!(
            r#"---
description: Altevra agent OS context for Cursor
globs:
  - "**/*"
alwaysApply: true
---

<!-- ALTEVRA_MANAGED: true -->
<!-- adapter: cursor -->

# Altevra (auto-applied)

Project: {project}

Before any non-trivial change run:

```bash
altevra updates --project {project} --json
altevra context --project {project} --json
```

Hooks under `.cursor/hooks.json` automatically record this session into
Altevra's recorder (sessions/turns tables).
"#
        )
    }

    /// Same drift-detection helper as other adapters.
    fn classify_path(
        path: &Path,
        label: &str,
        creates: &mut Vec<InstallPlanFile>,
        updates: &mut Vec<InstallPlanFile>,
        drifted: &mut Vec<InstallPlanFile>,
    ) {
        if path.exists() {
            let existing = std::fs::read_to_string(path).unwrap_or_default();
            if existing.contains("ALTEVRA_MANAGED: true")
                || existing.contains("\"_altevra_managed\": true")
            {
                updates.push(InstallPlanFile {
                    path: path.to_path_buf(),
                    action: "update".to_string(),
                    managed: true,
                    checksum: String::new(),
                    reason: Some(format!("Refresh {label}")),
                });
            } else if !existing.trim().is_empty() {
                drifted.push(InstallPlanFile {
                    path: path.to_path_buf(),
                    action: "skip (drifted — manual edits detected)".to_string(),
                    managed: false,
                    checksum: String::new(),
                    reason: Some(format!(
                        "{} exists without Altevra managed header — remove or back up first",
                        path.display()
                    )),
                });
            } else {
                creates.push(InstallPlanFile {
                    path: path.to_path_buf(),
                    action: "create".to_string(),
                    managed: true,
                    checksum: String::new(),
                    reason: Some(format!("Install {label}")),
                });
            }
        } else {
            creates.push(InstallPlanFile {
                path: path.to_path_buf(),
                action: "create".to_string(),
                managed: true,
                checksum: String::new(),
                reason: Some(format!("Install {label}")),
            });
        }
    }
}

impl Default for CursorAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolAdapter for CursorAdapter {
    fn tool_name(&self) -> &'static str {
        "cursor"
    }

    fn adapter_version(&self) -> &'static str {
        ADAPTER_VERSION
    }

    fn detect(&self, repo_path: &Path) -> AdapterDetectionResult {
        let mut notes = vec![];
        let cursor_dir = repo_path.join(".cursor");
        let cursorrules = repo_path.join(".cursorrules");
        if cursor_dir.exists() {
            notes.push(".cursor/ directory found".to_string());
        }
        if cursorrules.exists() {
            notes.push(".cursorrules found".to_string());
        }
        let agents_md = repo_path.join("AGENTS.md");
        if agents_md.exists() {
            notes.push("AGENTS.md found".to_string());
        }

        let detected = cursor_dir.exists() || cursorrules.exists();
        AdapterDetectionResult {
            tool_name: self.tool_name().to_string(),
            detected,
            repo_path: Some(repo_path.to_path_buf()),
            notes,
        }
    }

    fn render_instructions(
        &self,
        input: InstructionRenderInput,
    ) -> anyhow::Result<Vec<GeneratedFile>> {
        let body = Self::agents_md_content(input.project.as_deref());
        // Body already contains its own managed header (HTML comment style).
        // Construct GeneratedFile directly to avoid double-header.
        let file = GeneratedFile::new("AGENTS.md", body);
        Ok(vec![file])
    }

    /// Cursor has no per-project skills directory of its own. Skills are
    /// surfaced via the auto-applied rule at `.cursor/rules/altevra.mdc`
    /// which Cursor attaches to every chat.
    fn render_skills(&self, _skills: Vec<&ParsedSkill>) -> anyhow::Result<Vec<GeneratedFile>> {
        Ok(vec![])
    }

    fn render_hooks(&self, _hooks: Vec<&UniversalHook>) -> anyhow::Result<Vec<GeneratedFile>> {
        let content = Self::hooks_json_content();
        let file = GeneratedFile::new(".cursor/hooks.json", content);
        Ok(vec![file])
    }

    fn build_install_plan(
        &self,
        repo_path: &Path,
        project: Option<&str>,
    ) -> anyhow::Result<InstallPlan> {
        let mut files_to_create = vec![];
        let mut files_to_update = vec![];
        let mut files_drifted = vec![];

        for (path, label) in [
            (repo_path.join("AGENTS.md"), "AGENTS.md instructions"),
            (repo_path.join(".cursor/mcp.json"), "Cursor MCP config"),
            (
                repo_path.join(".cursor/hooks.json"),
                "Cursor hooks (21 events)",
            ),
            (
                repo_path.join(".cursor/rules/altevra.mdc"),
                "Cursor auto-applied rule",
            ),
        ] {
            Self::classify_path(
                &path,
                label,
                &mut files_to_create,
                &mut files_to_update,
                &mut files_drifted,
            );
        }

        Ok(InstallPlan {
            tool_name: self.tool_name().to_string(),
            project: project.map(String::from),
            files_to_create,
            files_to_update,
            files_drifted,
            skills_to_install: vec![],
            dry_run: true,
        })
    }

    fn install(&self, plan: &InstallPlan, repo_path: &Path) -> anyhow::Result<InstallResult> {
        let files_skipped: Vec<_> = plan.files_drifted.iter().map(|f| f.path.clone()).collect();

        if !plan.files_drifted.is_empty() {
            let detail: Vec<String> = plan
                .files_drifted
                .iter()
                .map(|f| f.path.display().to_string())
                .collect();
            anyhow::bail!(
                "Manual edits detected on {} — remove the file(s) first or back them up. Refusing to overwrite.",
                detail.join(", ")
            );
        }

        if plan.dry_run {
            return Ok(InstallResult {
                tool_name: self.tool_name().to_string(),
                files_created: vec![],
                files_updated: vec![],
                files_skipped,
                success: true,
                error: None,
            });
        }

        let input = InstructionRenderInput {
            tool_name: self.tool_name().to_string(),
            project: plan.project.clone(),
            repo_path: repo_path.to_path_buf(),
            altevra_version: ADAPTER_VERSION.to_string(),
        };

        let mut to_write: Vec<GeneratedFile> = vec![];
        to_write.extend(self.render_instructions(input.clone())?);
        to_write.push(GeneratedFile::new(
            ".cursor/mcp.json",
            Self::mcp_json_content(),
        ));
        to_write.extend(self.render_hooks(vec![])?);
        to_write.push(GeneratedFile::new(
            ".cursor/rules/altevra.mdc",
            Self::rule_mdc_content(input.project.as_deref()),
        ));

        let creates: std::collections::HashSet<_> = plan
            .files_to_create
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let updates: std::collections::HashSet<_> = plan
            .files_to_update
            .iter()
            .map(|f| f.path.clone())
            .collect();

        let mut files_created = vec![];
        let mut files_updated = vec![];
        for gen in &to_write {
            let dest = repo_path.join(&gen.path);
            if creates.contains(&dest) {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &gen.content)?;
                info!("Created managed file: {}", dest.display());
                files_created.push(dest);
            } else if updates.contains(&dest) {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &gen.content)?;
                info!("Updated managed file: {}", dest.display());
                files_updated.push(dest);
            }
        }

        Ok(InstallResult {
            tool_name: self.tool_name().to_string(),
            files_created,
            files_updated,
            files_skipped,
            success: true,
            error: None,
        })
    }

    fn verify(&self, repo_path: &Path) -> anyhow::Result<VerifyResult> {
        let mut issues = vec![];
        let mut drifted = vec![];

        let core: [(std::path::PathBuf, &str); 4] = [
            (repo_path.join("AGENTS.md"), "ALTEVRA_MANAGED: true"),
            (
                repo_path.join(".cursor/mcp.json"),
                "\"_altevra_managed\": true",
            ),
            (
                repo_path.join(".cursor/hooks.json"),
                "\"_altevra_managed\": true",
            ),
            (
                repo_path.join(".cursor/rules/altevra.mdc"),
                "ALTEVRA_MANAGED: true",
            ),
        ];

        for (path, marker) in core {
            if !path.exists() {
                issues.push(format!("Missing: {}", path.display()));
            } else {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                if !content.contains(marker) {
                    issues.push(format!(
                        "Drift detected (no managed header): {}",
                        path.display()
                    ));
                    drifted.push(path);
                }
            }
        }

        Ok(VerifyResult {
            tool_name: self.tool_name().to_string(),
            all_ok: issues.is_empty(),
            issues,
            drifted_files: drifted,
        })
    }

    fn repair(&self, _repo_path: &Path) -> anyhow::Result<RepairPlan> {
        Ok(RepairPlan {
            tool_name: self.tool_name().to_string(),
            actions: vec![
                "Re-run: altevra connect --tool cursor to restore managed files".to_string(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_tool_name_and_version() {
        let a = CursorAdapter::new();
        assert_eq!(a.tool_name(), "cursor");
        assert_eq!(a.adapter_version(), ADAPTER_VERSION);
    }

    #[test]
    fn test_hook_events_count_is_21() {
        assert_eq!(
            HOOK_EVENTS.len(),
            21,
            "Cursor adapter must cover 21 lifecycle events per v0.3.6 plan"
        );
    }

    #[test]
    fn test_hooks_json_lists_all_21_events() {
        let body = CursorAdapter::hooks_json_content();
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        let hooks = parsed
            .get("hooks")
            .and_then(|v| v.as_object())
            .expect("hooks object");
        assert_eq!(hooks.len(), 21);
        for (cursor_event, _) in HOOK_EVENTS {
            assert!(
                hooks.contains_key(*cursor_event),
                "missing cursor event: {cursor_event}"
            );
        }
    }

    #[test]
    fn test_hooks_command_uses_hook_handle() {
        let body = CursorAdapter::hooks_json_content();
        assert!(
            body.contains("altevra hook-handle"),
            "hooks must call altevra hook-handle (the v0.3.1 stdin handler)"
        );
        // Must NOT use the legacy hook-run path.
        assert!(
            !body.contains("altevra hook run "),
            "hooks must not use the legacy 'hook run' command"
        );
    }

    #[test]
    fn test_hooks_json_is_deterministic() {
        let a = CursorAdapter::hooks_json_content();
        let b = CursorAdapter::hooks_json_content();
        assert_eq!(a, b, "hooks.json content must be deterministic");
    }

    #[test]
    fn test_mcp_json_polarity() {
        let cfg = CursorAdapter::mcp_json_content();
        assert!(cfg.contains("\"disabled\": false"));
        assert!(!cfg.contains("\"enabled\": true"));
        let parsed: serde_json::Value = serde_json::from_str(&cfg).expect("valid JSON");
        assert!(parsed.get("mcpServers").is_some());
        assert_eq!(
            parsed.get("_altevra_managed").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn test_render_instructions_managed_header() {
        let a = CursorAdapter::new();
        let input = InstructionRenderInput {
            tool_name: "cursor".to_string(),
            project: Some("altevra".to_string()),
            repo_path: std::path::PathBuf::from("/tmp"),
            altevra_version: "0.1.0".to_string(),
        };
        let files = a.render_instructions(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.to_string_lossy(), "AGENTS.md");
        assert!(files[0].content.contains("ALTEVRA_MANAGED: true"));
        assert!(files[0].content.contains("Project: altevra"));
        assert!(!files[0].content.contains("generated_at"));
    }

    #[test]
    fn test_render_hooks_emits_cursor_hooks_json() {
        let a = CursorAdapter::new();
        let files = a.render_hooks(vec![]).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.to_string_lossy(), ".cursor/hooks.json");
    }

    #[test]
    fn test_render_skills_is_empty_for_cursor() {
        let a = CursorAdapter::new();
        let files = a.render_skills(vec![]).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_build_install_plan_lists_four_core_files() {
        let tmp = tempdir().unwrap();
        let a = CursorAdapter::new();
        let plan = a.build_install_plan(tmp.path(), Some("altevra")).unwrap();
        let total = plan.files_to_create.len() + plan.files_to_update.len();
        assert_eq!(
            total, 4,
            "expect AGENTS.md + .cursor/mcp.json + .cursor/hooks.json + .cursor/rules/altevra.mdc"
        );
    }

    #[test]
    fn test_install_writes_all_four_files() {
        let tmp = tempdir().unwrap();
        let a = CursorAdapter::new();
        let mut plan = a.build_install_plan(tmp.path(), Some("altevra")).unwrap();
        plan.dry_run = false;
        let result = a.install(&plan, tmp.path()).unwrap();
        assert!(result.success);
        assert!(tmp.path().join("AGENTS.md").exists());
        assert!(tmp.path().join(".cursor/mcp.json").exists());
        assert!(tmp.path().join(".cursor/hooks.json").exists());
        assert!(tmp.path().join(".cursor/rules/altevra.mdc").exists());

        let verify = a.verify(tmp.path()).unwrap();
        assert!(
            verify.all_ok,
            "verify after install should be clean; issues: {:?}",
            verify.issues
        );
    }

    #[test]
    fn test_drift_detection_refuses_overwrite_for_cursor() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".cursor")).unwrap();
        // Hand-written hooks.json without managed marker.
        std::fs::write(
            tmp.path().join(".cursor/hooks.json"),
            r#"{"hooks": {"beforeShellExecution": []}}"#,
        )
        .unwrap();

        let a = CursorAdapter::new();
        let plan = a.build_install_plan(tmp.path(), None).unwrap();
        assert!(
            plan.files_drifted
                .iter()
                .any(|f| f.path.to_string_lossy().ends_with(".cursor/hooks.json")),
            "drift detection must catch user-edited hooks.json"
        );

        let err = a.install(&plan, tmp.path()).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("manual edits"));
    }

    #[test]
    fn test_repair_returns_plan() {
        let a = CursorAdapter::new();
        let plan = a.repair(std::path::Path::new("/tmp")).unwrap();
        assert_eq!(plan.tool_name, "cursor");
        assert!(!plan.actions.is_empty());
    }
}
