//! Codex adapter — OpenAI Codex CLI integration.
//!
//! Generates two managed files in the repo:
//!
//! 1. `AGENTS.md` — plain Markdown instructions at the repo root. Codex reads
//!    this file automatically as project context.
//! 2. `.codex/config.toml` — project-scoped Codex configuration. Contains
//!    `[mcp_servers.altevra]` (so Codex can call the Altevra MCP server) and a
//!    `[hooks]` block wired to the `altevra hook run …` CLI fallback.
//!
//! We use `toml_edit` so comments (including the `ALTEVRA_MANAGED` header) are
//! preserved on re-render. Codex does **not** have a per-project skills
//! directory — its "prompts" live under `~/.codex/prompts/` user-globally — so
//! `render_skills()` returns an empty vector and skills are surfaced through
//! the CLI fallback (`altevra skill …`).
//!
//! Detection: any of `~/.codex/`, `.codex/config.toml`, or a top-level
//! `AGENTS.md`.

use crate::base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallPlanFile, InstallResult,
    InstructionRenderInput, RepairPlan, ToolAdapter, VerifyResult,
};
use altevra_hooks::universal::UniversalHook;
use altevra_skills::parser::ParsedSkill;
use sha2::{Digest, Sha256};
use std::path::Path;
use toml_edit::{value, Array, DocumentMut, InlineTable, Item, Table, Value};
use tracing::info;

const ADAPTER_VERSION: &str = "0.1.0";

pub struct CodexAdapter;

impl CodexAdapter {
    pub fn new() -> Self {
        Self
    }

    /// AGENTS.md body, parameterized by project name.
    fn agents_md_content(project: Option<&str>) -> String {
        let project_line = project
            .map(|p| format!("Project: {p}"))
            .unwrap_or_else(|| "Project: (set ALTEVRA_PROJECT env var)".to_string());
        format!(
            r#"<!-- ALTEVRA_MANAGED: true -->
<!-- source: 07-capabilities/agent-tools.yaml -->
<!-- generated_by: altevra -->
<!-- adapter: codex -->

# Altevra Context

{project_line}

## Session Startup

At the start of every session, call:

```bash
altevra agent bootstrap --tool codex --project ${{ALTEVRA_PROJECT}} --json
```

Or via MCP: `get_agent_bootstrap_packet(tool_name="codex", project="${{ALTEVRA_PROJECT}}")`

## Quick CLI Reference

```bash
altevra updates --project ${{ALTEVRA_PROJECT}} --json          # What changed since last session
altevra skill check --all                                      # Are my skills fresh?
altevra hook run session_start --tool codex                    # Run startup hook
altevra context --project ${{ALTEVRA_PROJECT}} --json          # Current project context
```

## Rules

- Check last updates before working.
- Warn if any skill is outdated.
- Use CLI fallback if MCP is unavailable.
- Never edit ALTEVRA_MANAGED files manually.
- Finish session with: `altevra hook run session_end --tool codex --project ${{ALTEVRA_PROJECT}}`
"#
        )
    }

    /// Build the `.codex/config.toml` content. Combines MCP server registration
    /// and lifecycle hooks into a single managed file. Uses `toml_edit` so the
    /// comment header (carrying `ALTEVRA_MANAGED: true` + checksum) is preserved
    /// on re-render.
    fn config_toml_content() -> String {
        // Build the TOML body first (without header) so we can checksum it.
        let mut doc = DocumentMut::new();

        // [mcp_servers.altevra]
        let mut mcp_args = Array::new();
        mcp_args.push("serve");
        let mut altevra_server = Table::new();
        altevra_server["command"] = value("altevra");
        altevra_server["args"] = value(mcp_args);
        altevra_server["enabled"] = value(true);
        altevra_server["startup_timeout_sec"] = value(10i64);

        let mut mcp_servers = Table::new();
        mcp_servers.set_implicit(true);
        mcp_servers["altevra"] = Item::Table(altevra_server);
        doc["mcp_servers"] = Item::Table(mcp_servers);

        // [hooks] — 10 lifecycle events covering session, prompt, tool-call,
        // compaction, error and notification phases. All wired to
        // `altevra hook-handle <event>` which reads JSON from stdin (v0.3.1).
        let mut hooks = Table::new();
        for (event, altevra_event) in [
            ("SessionStart", "session_start"),
            ("Stop", "session_end"),
            ("UserPromptSubmit", "user_prompt_submit"),
            ("ResponseReceived", "response_received"),
            ("PreToolUse", "pre_tool_use"),
            ("PostToolUse", "post_tool_use"),
            ("PreCompaction", "pre_compaction"),
            ("PostCompaction", "post_compaction"),
            ("Error", "error"),
            ("Notification", "notification"),
        ] {
            let mut entry = InlineTable::new();
            entry.insert("type", Value::from("command"));
            entry.insert(
                "command",
                Value::from(format!("altevra hook-handle {altevra_event} --tool codex")),
            );
            let mut arr = Array::new();
            arr.push(Value::InlineTable(entry));
            hooks[event] = value(arr);
        }
        doc["hooks"] = Item::Table(hooks);

        let body = doc.to_string();

        // Checksum the body, then prepend the managed-header comment block.
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        let checksum = hex::encode(hasher.finalize());

        format!(
            "# ALTEVRA_MANAGED: true\n\
             # source: 07-capabilities/hooks.yaml\n\
             # generated_by: altevra\n\
             # adapter: codex\n\
             # version: {ADAPTER_VERSION}\n\
             # checksum: {checksum}\n\n{body}"
        )
    }

    /// Classify a destination path into create/update/drifted bucket.
    /// Drift = file exists, is non-empty, and lacks the `ALTEVRA_MANAGED`
    /// header — meaning a human edited it. We refuse to overwrite in that case.
    fn classify_path(
        path: &Path,
        label: &str,
        creates: &mut Vec<InstallPlanFile>,
        updates: &mut Vec<InstallPlanFile>,
        drifted: &mut Vec<InstallPlanFile>,
    ) {
        if path.exists() {
            let existing = std::fs::read_to_string(path).unwrap_or_default();
            if existing.contains("ALTEVRA_MANAGED: true") {
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
                        "{} exists without Altevra managed header — remove it manually first",
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

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolAdapter for CodexAdapter {
    fn tool_name(&self) -> &'static str {
        "codex"
    }

    fn adapter_version(&self) -> &'static str {
        ADAPTER_VERSION
    }

    fn detect(&self, repo_path: &Path) -> AdapterDetectionResult {
        let mut notes = vec![];

        let home_codex = dirs_home().map(|h| h.join(".codex")).filter(|p| p.exists());
        if let Some(p) = &home_codex {
            notes.push(format!("{} found", p.display()));
        }

        let project_config = repo_path.join(".codex/config.toml");
        if project_config.exists() {
            notes.push(".codex/config.toml found".to_string());
        }

        let agents_md = repo_path.join("AGENTS.md");
        if agents_md.exists() {
            notes.push("AGENTS.md found".to_string());
        }

        let detected = home_codex.is_some() || project_config.exists() || agents_md.exists();

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
        // AGENTS.md already has its managed header baked in (so the body itself
        // contains `ALTEVRA_MANAGED: true`), and we set `version: <input>` here
        // by appending it once. Simpler path: build content, attach via
        // GeneratedFile::new (auto-checksums), do NOT call with_managed_header
        // because we want a single `# Altevra Context` heading at the top of
        // the file (with_managed_header would add HTML-comment headers *and*
        // the body already starts with them).
        //
        // Note: with_managed_header would prepend a duplicate header. The body
        // already encodes adapter/source — we just need to also emit version &
        // checksum. We do that by re-rendering with the actual values via
        // GeneratedFile::with_managed_header — but to avoid duplication we
        // build the body without the header here.
        let body = Self::agents_md_content(input.project.as_deref());
        // Strip the inline header from body so we let with_managed_header add
        // the canonical one (matching claude_code.rs pattern).
        let body_no_header = body
            .split_once("\n\n")
            .map(|(_, after)| after)
            .unwrap_or(&body)
            .to_string();
        let file = GeneratedFile::new("AGENTS.md", body_no_header).with_managed_header(
            "07-capabilities/agent-tools.yaml",
            self.tool_name(),
            &input.altevra_version,
        );
        Ok(vec![file])
    }

    /// Codex has no per-project skills directory. Its prompts live in
    /// `~/.codex/prompts/` user-globally; we surface skills via the
    /// `altevra skill …` CLI fallback instead.
    fn render_skills(&self, _skills: Vec<&ParsedSkill>) -> anyhow::Result<Vec<GeneratedFile>> {
        Ok(vec![])
    }

    /// Hooks for Codex live in `.codex/config.toml` alongside the MCP server
    /// block. We emit a single file containing both.
    fn render_hooks(&self, _hooks: Vec<&UniversalHook>) -> anyhow::Result<Vec<GeneratedFile>> {
        let content = Self::config_toml_content();
        // content already contains its own header (TOML `#` comment style), so
        // we construct the file directly without re-applying with_managed_header.
        let file = GeneratedFile::new(".codex/config.toml", content);
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
            (
                repo_path.join(".codex/config.toml"),
                "Codex config (MCP + hooks)",
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

        // Codex has no per-project skills; skills_to_install stays empty.
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
            // Surface drift loudly: refuse to silently overwrite human edits.
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
        to_write.extend(self.render_instructions(input)?);
        to_write.extend(self.render_hooks(vec![])?);

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

        for path in [
            repo_path.join("AGENTS.md"),
            repo_path.join(".codex/config.toml"),
        ] {
            if !path.exists() {
                issues.push(format!("Missing: {}", path.display()));
            } else {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                if !content.contains("ALTEVRA_MANAGED: true") {
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
                "Re-run: altevra connect --tool codex to restore managed files".to_string(),
            ],
        })
    }
}

/// Best-effort home dir lookup (no extra crate dep — we use $HOME).
fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_render_instructions_returns_managed_file() {
        let adapter = CodexAdapter::new();
        let input = InstructionRenderInput {
            tool_name: "codex".to_string(),
            project: Some("altevra".to_string()),
            repo_path: std::path::PathBuf::from("/tmp"),
            altevra_version: "0.1.0".to_string(),
        };
        let files = adapter.render_instructions(input).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.path, std::path::PathBuf::from("AGENTS.md"));
        assert!(
            f.content.contains("ALTEVRA_MANAGED: true"),
            "missing managed header in AGENTS.md"
        );
        assert!(f.content.contains("adapter: codex"));
        assert!(
            !f.content.contains("generated_at"),
            "header must not contain generated_at"
        );
    }

    #[test]
    fn test_render_hooks_emits_config_toml_with_mcp_and_hooks() {
        let adapter = CodexAdapter::new();
        let files = adapter.render_hooks(vec![]).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.path, std::path::PathBuf::from(".codex/config.toml"));
        assert!(f.content.contains("ALTEVRA_MANAGED: true"));
        assert!(f.content.contains("[mcp_servers.altevra]"));
        assert!(f.content.contains("[hooks]"));
        assert!(f.content.contains("SessionStart"));
        assert!(f.content.contains("PreToolUse"));
        assert!(!f.content.contains("generated_at"), "no timestamps allowed");

        // Parse the TOML to make sure it is valid (after stripping the
        // comment header which toml_edit is happy with anyway).
        let _doc: DocumentMut = f.content.parse().expect("config.toml must parse");
    }

    #[test]
    fn test_codex_hooks_cover_ten_events() {
        let adapter = CodexAdapter::new();
        let f = &adapter.render_hooks(vec![]).unwrap()[0];
        for ev in [
            "SessionStart",
            "Stop",
            "UserPromptSubmit",
            "ResponseReceived",
            "PreToolUse",
            "PostToolUse",
            "PreCompaction",
            "PostCompaction",
            "Error",
            "Notification",
        ] {
            assert!(
                f.content.contains(ev),
                "Codex config.toml missing hook event {ev}\n{}",
                f.content
            );
        }
    }

    #[test]
    fn test_codex_hooks_use_hook_handle_not_hook_run() {
        let adapter = CodexAdapter::new();
        let f = &adapter.render_hooks(vec![]).unwrap()[0];
        assert!(
            f.content.contains("altevra hook-handle"),
            "Codex hooks must call altevra hook-handle (v0.3.1 stdin handler)"
        );
        assert!(
            !f.content.contains("altevra hook run "),
            "Codex hooks must not use the legacy 'hook run' path"
        );
    }

    #[test]
    fn test_detect_picks_up_agents_md() {
        let adapter = CodexAdapter::new();
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# hi").unwrap();
        let result = adapter.detect(tmp.path());
        assert!(result.detected, "detect() should fire on AGENTS.md");
        assert!(result.notes.iter().any(|n| n.contains("AGENTS.md")));
    }

    #[test]
    fn test_build_install_plan_is_non_empty() {
        let adapter = CodexAdapter::new();
        let tmp = tempdir().unwrap();
        let plan = adapter
            .build_install_plan(tmp.path(), Some("altevra"))
            .unwrap();
        assert!(plan.dry_run);
        assert_eq!(plan.tool_name, "codex");
        assert_eq!(
            plan.files_to_create.len() + plan.files_to_update.len(),
            2,
            "expect AGENTS.md + .codex/config.toml to be planned"
        );
        assert!(plan.skills_to_install.is_empty());
    }

    #[test]
    fn test_render_skills_is_empty() {
        let adapter = CodexAdapter::new();
        let files = adapter.render_skills(vec![]).unwrap();
        assert!(
            files.is_empty(),
            "Codex has no per-project skills directory"
        );
    }

    #[test]
    fn test_drift_detection_refuses_overwrite() {
        let adapter = CodexAdapter::new();
        let tmp = tempdir().unwrap();
        // Write a human-edited AGENTS.md without the managed header.
        std::fs::write(tmp.path().join("AGENTS.md"), "# my hand-written notes").unwrap();

        let plan = adapter.build_install_plan(tmp.path(), None).unwrap();
        assert_eq!(plan.files_drifted.len(), 1);
        assert_eq!(plan.files_drifted[0].path.file_name().unwrap(), "AGENTS.md");

        // install() must bail with a clear message when drift is present.
        let err = adapter.install(&plan, tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("manual edits"),
            "expected drift error, got: {msg}"
        );
    }

    #[test]
    fn test_install_writes_files_when_no_drift() {
        let adapter = CodexAdapter::new();
        let tmp = tempdir().unwrap();
        let mut plan = adapter
            .build_install_plan(tmp.path(), Some("test"))
            .unwrap();
        plan.dry_run = false;

        let result = adapter.install(&plan, tmp.path()).unwrap();
        assert!(result.success);
        assert!(tmp.path().join("AGENTS.md").exists());
        assert!(tmp.path().join(".codex/config.toml").exists());

        // verify() should pass right after install.
        let v = adapter.verify(tmp.path()).unwrap();
        assert!(v.all_ok, "verify failed: {:?}", v.issues);
    }
}
