//! Hermes adapter (P0.7 T7.3, R10 Q7) — the cross-agent skill-sharing target.
//!
//! Hermes is Pavle's orchestrator sibling. Altevra renders shared skills into
//! `~/.imperium/skills/shared/<slug>/SKILL.md` so a skill the factory proposes
//! (and Pavle approves) becomes available to Hermes too — the "skill manufacturing
//! layer for the whole AI tool ecosystem" (CLAUDE.md §12). The Hermes base
//! (`~/.imperium`) is passed as `repo_path` by `altevra connect --tool hermes`.
//!
//! No-secret-in-render (T6): a skill body is scanned before rendering; if it
//! carries a credential it is REFUSED (never written to a shared dir).

use crate::base::{
    AdapterDetectionResult, GeneratedFile, InstallPlan, InstallPlanFile, InstallResult,
    InstructionRenderInput, RepairPlan, ToolAdapter, VerifyResult,
};
use altevra_hooks::universal::UniversalHook;
use altevra_skills::parser::ParsedSkill;
use std::path::Path;
use tracing::{info, warn};

const ADAPTER_VERSION: &str = "0.1.0";
const SHARED_DIR: &str = "skills/shared";
const CONTEXT_FILE: &str = "skills/shared/_altevra-context.md";

pub struct HermesAdapter;

impl HermesAdapter {
    pub fn new() -> Self {
        Self
    }

    fn context_md(project: Option<&str>) -> String {
        let project_line = project
            .map(|p| format!("Project: {p}"))
            .unwrap_or_else(|| "Project: (shared)".to_string());
        format!(
            "# Altevra ↔ Hermes shared context\n\n{project_line}\n\n\
             Skills under `skills/shared/` are rendered by Altevra's skill factory \
             (Pavle-approved). Call `altevra agent bootstrap --tool hermes` at session \
             start. Never edit ALTEVRA_MANAGED files by hand.\n"
        )
    }

    fn skill_md_content(skill: &ParsedSkill) -> String {
        let checksum = altevra_skills::checksum::compute(&skill.raw);
        format!(
            "<!-- ALTEVRA_MANAGED: true -->\n\
             <!-- source: skill:{slug} -->\n\
             <!-- generated_by: altevra -->\n\
             <!-- adapter: hermes -->\n\
             <!-- version: {ADAPTER_VERSION} -->\n\
             <!-- checksum: {checksum} -->\n\n{body}\n",
            slug = skill.slug(),
            body = skill.body.trim()
        )
    }

    /// No-secret-in-render gate (T6): true if the skill body is free of detectable
    /// credentials. A skill carrying a secret is never written to a shared dir.
    fn skill_is_clean(skill: &ParsedSkill) -> bool {
        altevra_secrets::detect_secrets(&skill.raw).is_empty()
    }

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
                        "{} exists without Altevra managed header — remove it first",
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

impl Default for HermesAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolAdapter for HermesAdapter {
    fn tool_name(&self) -> &'static str {
        "hermes"
    }

    fn adapter_version(&self) -> &'static str {
        ADAPTER_VERSION
    }

    fn detect(&self, repo_path: &Path) -> AdapterDetectionResult {
        // The Hermes base is ~/.imperium; connect passes it as repo_path.
        let imperium = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".imperium"))
            .filter(|p| p.exists());
        let mut notes = vec![];
        if let Some(p) = &imperium {
            notes.push(format!("{} found", p.display()));
        }
        let shared = repo_path.join(SHARED_DIR);
        if shared.exists() {
            notes.push(format!("{} found", shared.display()));
        }
        AdapterDetectionResult {
            tool_name: self.tool_name().to_string(),
            detected: imperium.is_some() || shared.exists(),
            repo_path: Some(repo_path.to_path_buf()),
            notes,
        }
    }

    fn render_instructions(
        &self,
        input: InstructionRenderInput,
    ) -> anyhow::Result<Vec<GeneratedFile>> {
        let file = GeneratedFile::new(CONTEXT_FILE, Self::context_md(input.project.as_deref()))
            .with_managed_header("skills/shared", self.tool_name(), &input.altevra_version);
        Ok(vec![file])
    }

    /// Render shared skills to `skills/shared/<slug>/SKILL.md`. A skill carrying a
    /// secret is REFUSED (T6 no-secret-in-render) — skipped with a warning.
    fn render_skills(&self, skills: Vec<&ParsedSkill>) -> anyhow::Result<Vec<GeneratedFile>> {
        let mut files = vec![];
        for skill in skills {
            if !Self::skill_is_clean(skill) {
                warn!(
                    "refusing to render skill '{}' to Hermes shared dir: contains a secret",
                    skill.slug()
                );
                continue;
            }
            let path = format!("{SHARED_DIR}/{}/SKILL.md", skill.slug());
            files.push(GeneratedFile::new(path, Self::skill_md_content(skill)));
        }
        Ok(files)
    }

    /// Hermes has its own native hook/cron system — Altevra does not render hooks
    /// for it (Hermes owns scheduling, per the Symbiosis split).
    fn render_hooks(&self, _hooks: Vec<&UniversalHook>) -> anyhow::Result<Vec<GeneratedFile>> {
        Ok(vec![])
    }

    fn build_install_plan(
        &self,
        repo_path: &Path,
        project: Option<&str>,
    ) -> anyhow::Result<InstallPlan> {
        let mut files_to_create = vec![];
        let mut files_to_update = vec![];
        let mut files_drifted = vec![];
        Self::classify_path(
            &repo_path.join(CONTEXT_FILE),
            "Altevra shared context",
            &mut files_to_create,
            &mut files_to_update,
            &mut files_drifted,
        );
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
                "Manual edits detected on {} — remove the file(s) first. Refusing to overwrite.",
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
        let mut files_created = vec![];
        let mut files_updated = vec![];
        for gen in self.render_instructions(input)? {
            let dest = repo_path.join(&gen.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let existed = dest.exists();
            std::fs::write(&dest, &gen.content)?;
            if existed {
                info!("Updated managed file: {}", dest.display());
                files_updated.push(dest);
            } else {
                info!("Created managed file: {}", dest.display());
                files_created.push(dest);
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
        let path = repo_path.join(CONTEXT_FILE);
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
                "Re-run: altevra connect --tool hermes to restore shared files".to_string(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_skills::parser::parse_skill;

    fn skill(slug: &str, body: &str) -> ParsedSkill {
        parse_skill(&format!(
            "---\nslug: {slug}\nversion: 1.0.0\ntitle: {slug}\ndescription: test\n---\n\n{body}"
        ))
        .unwrap()
    }

    #[test]
    fn renders_skill_to_shared_dir() {
        let a = HermesAdapter::new();
        let s = skill("gtm-playbook", "# GTM Playbook\nCold-call first.");
        let files = a.render_skills(vec![&s]).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            std::path::PathBuf::from("skills/shared/gtm-playbook/SKILL.md")
        );
        assert!(files[0].content.contains("ALTEVRA_MANAGED: true"));
        assert!(files[0].content.contains("adapter: hermes"));
        assert!(files[0].content.contains("Cold-call first"));
    }

    #[test]
    fn refuses_skill_with_secret() {
        // T6: a skill body carrying a credential is never rendered to a shared dir.
        let a = HermesAdapter::new();
        let leaky = skill(
            "leaky",
            &format!(
                "use {}",
                concat!("ghp_", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            ),
        );
        let files = a.render_skills(vec![&leaky]).unwrap();
        assert!(files.is_empty(), "skill with a secret must be refused");
    }

    #[test]
    fn install_writes_context_then_verify_ok() {
        let a = HermesAdapter::new();
        let tmp = tempfile::tempdir().unwrap();
        let mut plan = a.build_install_plan(tmp.path(), Some("shared")).unwrap();
        plan.dry_run = false;
        let r = a.install(&plan, tmp.path()).unwrap();
        assert!(r.success);
        assert!(tmp.path().join(CONTEXT_FILE).exists());
        assert!(a.verify(tmp.path()).unwrap().all_ok);
    }
}
