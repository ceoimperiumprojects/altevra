//! AntigravityAdapter — Google Antigravity (newest agent-first IDE, shared
//! integration surface with Gemini CLI).
//!
//! Generates:
//!   * `AGENTS.md` at repo root (plain Markdown — same convention as Codex).
//!   * `.agent/skills/<slug>/SKILL.md` per registered skill (folder layout).
//!   * `.gemini/config/mcp_config.json` at repo root for project-scoped MCP.
//!   * `.agent/hooks/altevra_hooks.py` — opt-in SDK scaffold (hooks are
//!     Python decorators, not static JSON).
//!
//! Polarity gotcha (vs. Codex MCP TOML):
//!   * Antigravity MCP JSON uses `"disabled": false` (NOT `enabled: true`).
//!   * Server URL key is `serverUrl` (NOT `url`).

use crate::base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallPlanFile, InstallResult,
    InstructionRenderInput, RepairPlan, ToolAdapter, VerifyResult,
};
use altevra_hooks::universal::UniversalHook;
use altevra_skills::parser::ParsedSkill;
use std::path::Path;
use tracing::info;

const ADAPTER_VERSION: &str = "0.1.0";

pub struct AntigravityAdapter;

impl AntigravityAdapter {
    pub fn new() -> Self {
        Self
    }

    /// AGENTS.md content (reused convention from Codex — plain Markdown with
    /// HTML managed header).
    fn agents_md_content(project: Option<&str>) -> String {
        let project_line = project
            .map(|p| format!("Project: {p}"))
            .unwrap_or_else(|| "Project: (set ALTEVRA_PROJECT env var)".to_string());
        format!(
            r#"# Altevra Agent Context (Antigravity)

{project_line}

## Session Startup

Antigravity agents must call the Altevra bootstrap at the start of every session:

```bash
altevra agent bootstrap --tool antigravity --project ${{ALTEVRA_PROJECT}} --json
```

Or via MCP: `get_agent_bootstrap_packet(tool_name="antigravity", project="${{ALTEVRA_PROJECT}}")`

## Quick CLI Reference

```bash
altevra updates --project ${{ALTEVRA_PROJECT}} --json
altevra skill check --all
altevra hook run session_start --tool antigravity
altevra context --project ${{ALTEVRA_PROJECT}} --json
```

## Rules

- Check `altevra updates` before working.
- Warn if any skill is outdated.
- Never edit ALTEVRA_MANAGED files manually.
- Finish session with: `altevra hook run session_end --tool antigravity --project ${{ALTEVRA_PROJECT}}`
"#
        )
    }

    /// Render a single SKILL.md body for Antigravity's folder layout.
    /// Mirrors Claude Code skill convention (`name`/`description`/`allowed-tools`).
    fn skill_md_content(skill: &ParsedSkill, checksum: &str) -> String {
        let fm = &skill.frontmatter;

        let name = fm
            .extra
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| fm.slug.clone());

        let description = fm
            .extra
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| fm.description.clone())
            .unwrap_or_else(|| fm.title.clone());

        let allowed_tools: Vec<String> = fm
            .extra
            .get("allowed-tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let allowed_tools_yaml = if allowed_tools.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", allowed_tools.join(", "))
        };

        let name_yaml = Self::yaml_string(&name);
        let description_yaml = Self::yaml_string(&description);

        format!(
            "---\n\
             name: {name_yaml}\n\
             description: {description_yaml}\n\
             allowed-tools: {allowed_tools_yaml}\n\
             ---\n\
             <!-- ALTEVRA_MANAGED: true -->\n\
             <!-- source: 06-skills/{slug}.md -->\n\
             <!-- generated_by: altevra -->\n\
             <!-- adapter: antigravity -->\n\
             <!-- version: {version} -->\n\
             <!-- checksum: {checksum} -->\n\n\
             # {title}\n\n\
             {body}\n",
            slug = fm.slug,
            version = fm.version,
            title = fm.title,
            body = skill.body,
        )
    }

    fn yaml_string(s: &str) -> String {
        let needs_quote = s.is_empty()
            || s.contains(':')
            || s.contains('#')
            || s.contains('\n')
            || s.contains('"')
            || s.starts_with(' ')
            || s.ends_with(' ');
        if needs_quote {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        } else {
            s.to_string()
        }
    }

    /// `.gemini/config/mcp_config.json` — Antigravity / Gemini CLI MCP wiring.
    ///
    /// IMPORTANT polarity gotcha:
    ///   * Uses `disabled: false` (NOT `enabled: true`)
    ///   * Uses `serverUrl` (NOT `url`)
    fn mcp_config_content() -> String {
        // Hand-written JSON ensures stable key ordering and avoids any chance
        // of HashMap-induced non-determinism. Note: `_altevra_managed` and
        // `_altevra_note` are sibling keys to `mcpServers` so the JSON can
        // be recognized as managed by Altevra without a header comment.
        let body = r#"{
  "_altevra_managed": true,
  "_altevra_note": "Generated by altevra connect. ALTEVRA_MANAGED: true — do not edit manually.",
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

    /// Antigravity hook scaffold — SDK-only Python decorators, not static JSON.
    fn hooks_py_content() -> String {
        // Use a Python comment header (# ALTEVRA_MANAGED: true) so the
        // managed-header drift check still works on this file format.
        r#"# ALTEVRA_MANAGED: true
# source: 07-capabilities/hooks.yaml
# generated_by: altevra
# adapter: antigravity
# version: 0.1.0
#
# Antigravity hook scaffold — opt-in. Drop this file into your repo and the
# Antigravity runtime will pick it up via its SDK decorators.
#
# Antigravity hooks are SDK-only (Python decorators), NOT static JSON like
# Claude Code's settings.json. So this file is the authoritative wiring.

from antigravity_sdk import hooks  # type: ignore
import subprocess


@hooks.session_start
def altevra_session_start(ctx):
    """Run Altevra session_start hook at the beginning of every session."""
    subprocess.run(
        [
            "altevra",
            "hook",
            "run",
            "session_start",
            "--tool",
            "antigravity",
            "--json",
        ],
        check=False,
    )


@hooks.pre_tool_call
def altevra_pre_tool(ctx):
    """Run Altevra before_tool_call hook before each tool invocation."""
    subprocess.run(
        [
            "altevra",
            "hook",
            "run",
            "before_tool_call",
            "--tool",
            "antigravity",
            "--json",
        ],
        check=False,
    )


@hooks.session_end
def altevra_session_end(ctx):
    """Run Altevra session_end hook at the end of every session."""
    subprocess.run(
        [
            "altevra",
            "hook",
            "run",
            "session_end",
            "--tool",
            "antigravity",
            "--json",
        ],
        check=False,
    )
"#
        .to_string()
    }

    /// Classify a destination path into create/update/drifted bucket.
    /// Drift = file exists without an ALTEVRA_MANAGED marker.
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
            } else if !existing.is_empty() {
                drifted.push(InstallPlanFile {
                    path: path.to_path_buf(),
                    action: "skip (drifted — manual edits detected)".to_string(),
                    managed: false,
                    checksum: String::new(),
                    reason: Some(
                        "File exists without Altevra managed header — will not overwrite"
                            .to_string(),
                    ),
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

impl Default for AntigravityAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolAdapter for AntigravityAdapter {
    fn tool_name(&self) -> &'static str {
        "antigravity"
    }

    fn adapter_version(&self) -> &'static str {
        ADAPTER_VERSION
    }

    fn detect(&self, repo_path: &Path) -> AdapterDetectionResult {
        let agents_md = repo_path.join("AGENTS.md");
        let agent_dir = repo_path.join(".agent");
        let gemini_dir = repo_path.join(".gemini");
        let home_gemini = dirs_home_gemini();

        let mut notes = vec![];
        if agents_md.exists() {
            notes.push("AGENTS.md found".to_string());
        }
        if agent_dir.exists() {
            notes.push(".agent/ directory found".to_string());
        }
        if gemini_dir.exists() {
            notes.push(".gemini/ directory found".to_string());
        }
        if home_gemini {
            notes.push("~/.gemini/ found (user-global)".to_string());
        }

        let detected = agent_dir.exists() || gemini_dir.exists() || home_gemini;

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
        let content = Self::agents_md_content(input.project.as_deref());
        let file = GeneratedFile::new("AGENTS.md", content).with_managed_header(
            "07-capabilities/agent-tools.yaml",
            self.tool_name(),
            &input.altevra_version,
        );
        Ok(vec![file])
    }

    fn render_skills(&self, skills: Vec<&ParsedSkill>) -> anyhow::Result<Vec<GeneratedFile>> {
        let mut files = vec![];
        for skill in skills {
            let content =
                Self::skill_md_content(skill, &altevra_skills::checksum::compute(&skill.raw));
            let path = format!(".agent/skills/{}/SKILL.md", skill.slug());
            files.push(GeneratedFile::new(path, content));
        }
        Ok(files)
    }

    fn render_hooks(&self, _hooks: Vec<&UniversalHook>) -> anyhow::Result<Vec<GeneratedFile>> {
        // Antigravity hooks are SDK-only Python decorators, not static JSON.
        // Return a single Python scaffold the user can opt into. The managed
        // marker is a Python comment so drift detection still works.
        let py = Self::hooks_py_content();
        let file = GeneratedFile::new(".agent/hooks/altevra_hooks.py", py);
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

        // Core files — always part of the antigravity install.
        for (path, label) in [
            (repo_path.join("AGENTS.md"), "AGENTS.md"),
            (
                repo_path.join(".gemini/config/mcp_config.json"),
                "mcp_config.json (project-scoped)",
            ),
            (
                repo_path.join(".agent/hooks/altevra_hooks.py"),
                "antigravity hooks scaffold",
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

        // Skills: scan vault 06-skills/*.md.
        let vault_skills_dir = repo_path.join("06-skills");
        let mut skills_to_install = vec![];
        if vault_skills_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&vault_skills_dir)
                .into_iter()
                .flatten()
                .flatten()
                .collect();
            entries.sort_by_key(|e| e.path());
            for entry in entries {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let raw = std::fs::read_to_string(&p).unwrap_or_default();
                if let Ok(skill) = altevra_skills::parser::parse_skill(&raw) {
                    let dest = repo_path.join(format!(".agent/skills/{}/SKILL.md", skill.slug()));
                    let label = format!("{} skill", skill.slug());
                    Self::classify_path(
                        &dest,
                        &label,
                        &mut files_to_create,
                        &mut files_to_update,
                        &mut files_drifted,
                    );
                    skills_to_install.push(skill);
                }
            }
        }

        Ok(InstallPlan {
            tool_name: self.tool_name().to_string(),
            project: project.map(String::from),
            files_to_create,
            files_to_update,
            files_drifted,
            skills_to_install,
            dry_run: true,
        })
    }

    fn install(&self, plan: &InstallPlan, repo_path: &Path) -> anyhow::Result<InstallResult> {
        let files_skipped: Vec<_> = plan.files_drifted.iter().map(|f| f.path.clone()).collect();

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
        // MCP config is a managed JSON file (not via render_hooks).
        let mcp = GeneratedFile::new(".gemini/config/mcp_config.json", Self::mcp_config_content());
        to_write.push(mcp);
        to_write.extend(self.render_hooks(vec![])?);
        to_write.extend(self.render_skills(plan.skills_to_install.iter().collect())?);

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

        // (path, marker_must_contain)
        let core: [(std::path::PathBuf, &str); 3] = [
            (repo_path.join("AGENTS.md"), "ALTEVRA_MANAGED: true"),
            (
                repo_path.join(".gemini/config/mcp_config.json"),
                "\"_altevra_managed\": true",
            ),
            (
                repo_path.join(".agent/hooks/altevra_hooks.py"),
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
                "Re-run: altevra connect --tool antigravity to restore managed files".to_string(),
            ],
        })
    }
}

/// Light check for `~/.gemini/` without pulling in the `dirs` crate.
fn dirs_home_gemini() -> bool {
    if let Ok(home) = std::env::var("HOME") {
        std::path::Path::new(&home).join(".gemini").exists()
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_skills::parser::parse_skill;
    use tempfile::TempDir;

    fn sample_skill() -> ParsedSkill {
        parse_skill(
            "---\n\
             slug: altevra-core\n\
             version: 0.5.0\n\
             title: Altevra Core\n\
             description: Core skill.\n\
             ---\n\n\
             Body content.\n",
        )
        .unwrap()
    }

    #[test]
    fn test_tool_name_and_version() {
        let a = AntigravityAdapter::new();
        assert_eq!(a.tool_name(), "antigravity");
        assert_eq!(a.adapter_version(), ADAPTER_VERSION);
    }

    #[test]
    fn test_detect_returns_result() {
        let tmp = TempDir::new().unwrap();
        let a = AntigravityAdapter::new();
        let res = a.detect(tmp.path());
        assert_eq!(res.tool_name, "antigravity");
        // No markers in a fresh temp dir → not detected (modulo ~/.gemini).
        // We don't strictly assert false because the host machine may have
        // a global ~/.gemini.
        assert!(res.repo_path.is_some());
    }

    #[test]
    fn test_detect_finds_agent_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agent")).unwrap();
        let a = AntigravityAdapter::new();
        let res = a.detect(tmp.path());
        assert!(res.detected, "should detect when .agent/ exists");
        assert!(res
            .notes
            .iter()
            .any(|n| n.contains(".agent/ directory found")));
    }

    #[test]
    fn test_render_instructions_writes_agents_md() {
        let a = AntigravityAdapter::new();
        let input = InstructionRenderInput {
            tool_name: "antigravity".to_string(),
            project: Some("demo".to_string()),
            repo_path: std::path::PathBuf::from("/tmp"),
            altevra_version: "0.1.0".to_string(),
        };
        let files = a.render_instructions(input).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.to_string_lossy(), "AGENTS.md");
        assert!(files[0].content.contains("ALTEVRA_MANAGED: true"));
        assert!(files[0].content.contains("Project: demo"));
        assert!(files[0].content.contains("altevra agent bootstrap"));
        // Determinism: no timestamps.
        assert!(!files[0].content.contains("generated_at"));
    }

    #[test]
    fn test_render_skills_uses_agent_folder_layout() {
        let a = AntigravityAdapter::new();
        let skill = sample_skill();
        let files = a.render_skills(vec![&skill]).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_string_lossy(),
            ".agent/skills/altevra-core/SKILL.md"
        );
        let content = &files[0].content;
        assert!(content.starts_with("---\n"));
        assert!(content.contains("name: altevra-core"));
        assert!(content.contains("allowed-tools: []"));
        assert!(content.contains("ALTEVRA_MANAGED: true"));
        assert!(content.contains("adapter: antigravity"));
    }

    #[test]
    fn test_render_skills_determinism() {
        let a = AntigravityAdapter::new();
        let skill = sample_skill();
        let f1 = a.render_skills(vec![&skill]).unwrap();
        let f2 = a.render_skills(vec![&skill]).unwrap();
        assert_eq!(f1[0].content, f2[0].content);
        assert_eq!(f1[0].checksum, f2[0].checksum);
        assert!(!f1[0].content.contains("generated_at"));
    }

    #[test]
    fn test_render_hooks_returns_python_scaffold() {
        let a = AntigravityAdapter::new();
        let files = a.render_hooks(vec![]).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path.to_string_lossy(),
            ".agent/hooks/altevra_hooks.py"
        );
        let content = &files[0].content;
        // Python managed header.
        assert!(content.contains("# ALTEVRA_MANAGED: true"));
        // SDK import.
        assert!(content.contains("from antigravity_sdk import hooks"));
        // Decorators present.
        assert!(content.contains("@hooks.session_start"));
        assert!(content.contains("@hooks.pre_tool_call"));
        // Wires to altevra CLI.
        assert!(content.contains("altevra"));
        assert!(content.contains("session_start"));
        assert!(content.contains("before_tool_call"));
    }

    #[test]
    fn test_mcp_config_polarity_disabled_false_and_server_url_key() {
        let cfg = AntigravityAdapter::mcp_config_content();
        // Critical polarity: disabled: false (NOT enabled: true).
        assert!(
            cfg.contains("\"disabled\": false"),
            "MCP config must use disabled: false (Antigravity polarity)"
        );
        assert!(
            !cfg.contains("\"enabled\": true"),
            "MCP config must NOT use enabled: true (that's Codex polarity)"
        );
        // Sanity: not using `url` key with a value — Antigravity uses
        // `serverUrl`. Our local config uses command+args (stdio transport),
        // so neither should appear with a URL value. Just confirm the
        // key `"url":` is not present.
        assert!(
            !cfg.contains("\"url\":"),
            "MCP config must NOT use 'url' key (Antigravity prefers 'serverUrl')"
        );
        // Managed sentinel.
        assert!(cfg.contains("\"_altevra_managed\": true"));
        // Valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&cfg).expect("valid JSON");
        assert!(parsed.get("mcpServers").is_some());
    }

    #[test]
    fn test_build_install_plan_lists_core_files() {
        let tmp = TempDir::new().unwrap();
        let a = AntigravityAdapter::new();
        let plan = a.build_install_plan(tmp.path(), Some("demo")).unwrap();
        let paths: Vec<String> = plan
            .files_to_create
            .iter()
            .map(|f| f.path.to_string_lossy().to_string())
            .collect();
        assert!(
            paths.iter().any(|p| p.ends_with("AGENTS.md")),
            "plan must include AGENTS.md, got {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with(".gemini/config/mcp_config.json")),
            "plan must include .gemini/config/mcp_config.json, got {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with(".agent/hooks/altevra_hooks.py")),
            "plan must include hooks scaffold, got {paths:?}"
        );
    }

    #[test]
    fn test_build_install_plan_skills_use_agent_folder_paths() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\n\nBody.\n",
        )
        .unwrap();

        let a = AntigravityAdapter::new();
        let plan = a.build_install_plan(tmp.path(), Some("demo")).unwrap();

        let has_folder = plan.files_to_create.iter().any(|f| {
            f.path
                .to_string_lossy()
                .ends_with(".agent/skills/altevra-core/SKILL.md")
        });
        assert!(
            has_folder,
            "plan must include .agent/skills/<slug>/SKILL.md, got {:?}",
            plan.files_to_create
        );
    }

    #[test]
    fn test_drift_detection_refuses_unmanaged_agents_md() {
        let tmp = TempDir::new().unwrap();
        // User-authored AGENTS.md without managed header.
        std::fs::write(
            tmp.path().join("AGENTS.md"),
            "# My hand-written agents file\nManual content.\n",
        )
        .unwrap();

        let a = AntigravityAdapter::new();
        let plan = a.build_install_plan(tmp.path(), None).unwrap();
        let drifted_has_agents = plan
            .files_drifted
            .iter()
            .any(|f| f.path.to_string_lossy().ends_with("AGENTS.md"));
        assert!(
            drifted_has_agents,
            "drift detection must refuse to overwrite unmanaged AGENTS.md"
        );
    }

    #[test]
    fn test_install_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let a = AntigravityAdapter::new();
        let mut plan = a.build_install_plan(tmp.path(), Some("demo")).unwrap();
        plan.dry_run = true;
        let result = a.install(&plan, tmp.path()).unwrap();
        assert!(result.success);
        assert!(result.files_created.is_empty());
        assert!(result.files_updated.is_empty());
        // Nothing on disk.
        assert!(!tmp.path().join("AGENTS.md").exists());
    }

    #[test]
    fn test_install_real_writes_all_managed_files() {
        let tmp = TempDir::new().unwrap();
        let a = AntigravityAdapter::new();
        let mut plan = a.build_install_plan(tmp.path(), Some("demo")).unwrap();
        plan.dry_run = false;
        let result = a.install(&plan, tmp.path()).unwrap();
        assert!(result.success);
        assert!(tmp.path().join("AGENTS.md").exists());
        assert!(tmp.path().join(".gemini/config/mcp_config.json").exists());
        assert!(tmp.path().join(".agent/hooks/altevra_hooks.py").exists());

        let mcp =
            std::fs::read_to_string(tmp.path().join(".gemini/config/mcp_config.json")).unwrap();
        assert!(mcp.contains("\"disabled\": false"));

        // Verify the freshly installed tree.
        let verify = a.verify(tmp.path()).unwrap();
        assert!(
            verify.all_ok,
            "verify after install should be clean; issues: {:?}",
            verify.issues
        );
    }

    #[test]
    fn test_verify_missing_files_reports_issues() {
        let tmp = TempDir::new().unwrap();
        let a = AntigravityAdapter::new();
        let verify = a.verify(tmp.path()).unwrap();
        assert!(!verify.all_ok);
        assert!(!verify.issues.is_empty());
    }

    #[test]
    fn test_repair_returns_plan() {
        let a = AntigravityAdapter::new();
        let plan = a.repair(std::path::Path::new("/tmp")).unwrap();
        assert_eq!(plan.tool_name, "antigravity");
        assert!(!plan.actions.is_empty());
    }
}
