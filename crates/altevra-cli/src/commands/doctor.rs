use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct DoctorArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
    /// Repository root to inspect
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Vault path (where 06-skills/ and 07-capabilities/ live)
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, serde::Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
}

pub async fn run(args: DoctorArgs) -> anyhow::Result<()> {
    let repo = args
        .repo
        .canonicalize()
        .unwrap_or_else(|_| args.repo.clone());
    let vault = args
        .vault
        .canonicalize()
        .unwrap_or_else(|_| args.vault.clone());

    let mut checks = run_checks(&repo, &vault);
    // Environment-level check (reads $HOME, so it lives outside the hermetic
    // `run_checks` set): a non-empty hook spool needs `altevra db replay-spool`.
    checks.push(check_spool_empty());

    let ok = checks
        .iter()
        .filter(|c| matches!(c.status, CheckStatus::Ok))
        .count();
    let warn = checks
        .iter()
        .filter(|c| matches!(c.status, CheckStatus::Warn))
        .count();
    let fail = checks
        .iter()
        .filter(|c| matches!(c.status, CheckStatus::Fail))
        .count();
    let overall = if fail > 0 {
        "fail"
    } else if warn > 0 {
        "warn"
    } else {
        "ok"
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "overall": overall,
                "checks": checks,
                "ok": ok,
                "warn": warn,
                "fail": fail,
            }))?
        );
    } else {
        for check in &checks {
            let icon = match check.status {
                CheckStatus::Ok => "✓",
                CheckStatus::Warn => "⚠",
                CheckStatus::Fail => "✗",
            };
            println!("  {icon} {} — {}", check.name, check.message);
            if let Some(hint) = &check.fix_hint {
                println!("    Fix: {hint}");
            }
        }
        println!();
        let total = ok + warn + fail;
        println!("Overall: {ok}/{total} OK");
        if warn > 0 {
            println!("  {warn} warning(s)");
        }
        if fail > 0 {
            println!("  {fail} failure(s)");
        }
    }

    Ok(())
}

pub fn run_checks(repo: &Path, vault: &Path) -> Vec<DoctorCheck> {
    vec![
        check_vault_initialized(vault),
        check_skills_dir(vault),
        check_capabilities_dir(vault),
        check_claude_connected(repo),
        check_instructions_managed(repo),
        check_settings_managed(repo),
        check_skills_installed(repo),
        check_skills_parseable(vault),
    ]
}

/// A non-empty hook spool means events captured during a `db unify` were never
/// replayed (or a replay failed) — recorded turns are sitting on disk instead
/// of in the canonical DB.
fn check_spool_empty() -> DoctorCheck {
    let dir = altevra_core::maintenance::spool_dir();
    let count = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    if count == 0 {
        DoctorCheck {
            name: "hook_spool_empty".into(),
            status: CheckStatus::Ok,
            message: "no spooled hook events pending replay".into(),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "hook_spool_empty".into(),
            status: CheckStatus::Warn,
            message: format!("{count} spooled hook event(s) pending in {}", dir.display()),
            fix_hint: Some("Run: altevra db replay-spool".into()),
        }
    }
}

fn check_vault_initialized(vault: &Path) -> DoctorCheck {
    let config = vault.join(".altevra/config.toml");
    if config.exists() {
        DoctorCheck {
            name: "vault_initialized".into(),
            status: CheckStatus::Ok,
            message: ".altevra/config.toml found".into(),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "vault_initialized".into(),
            status: CheckStatus::Fail,
            message: ".altevra/config.toml missing".into(),
            fix_hint: Some("Run: altevra init".into()),
        }
    }
}

fn check_skills_dir(vault: &Path) -> DoctorCheck {
    let dir = vault.join("06-skills");
    if !dir.exists() {
        return DoctorCheck {
            name: "skills_dir".into(),
            status: CheckStatus::Fail,
            message: "06-skills/ directory missing".into(),
            fix_hint: Some("Run: altevra init".into()),
        };
    }
    let count = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    if count == 0 {
        DoctorCheck {
            name: "skills_dir".into(),
            status: CheckStatus::Warn,
            message: "06-skills/ exists but has no .md files".into(),
            fix_hint: Some("Add skill files to 06-skills/".into()),
        }
    } else {
        DoctorCheck {
            name: "skills_dir".into(),
            status: CheckStatus::Ok,
            message: format!("{count} skill(s) in 06-skills/"),
            fix_hint: None,
        }
    }
}

fn check_capabilities_dir(vault: &Path) -> DoctorCheck {
    if vault.join("07-capabilities").exists() {
        DoctorCheck {
            name: "capabilities_dir".into(),
            status: CheckStatus::Ok,
            message: "07-capabilities/ found".into(),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "capabilities_dir".into(),
            status: CheckStatus::Warn,
            message: "07-capabilities/ missing".into(),
            fix_hint: Some("Run: altevra init".into()),
        }
    }
}

fn check_claude_connected(repo: &Path) -> DoctorCheck {
    if repo.join(".claude").exists() {
        DoctorCheck {
            name: "claude_connected".into(),
            status: CheckStatus::Ok,
            message: ".claude/ directory found".into(),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "claude_connected".into(),
            status: CheckStatus::Warn,
            message: ".claude/ not found — claude-code not connected".into(),
            fix_hint: Some("Run: altevra connect --tool claude-code --project <name>".into()),
        }
    }
}

fn check_instructions_managed(repo: &Path) -> DoctorCheck {
    check_managed_file(
        "instructions_managed",
        &repo.join(".claude/altevra-instructions.md"),
        "altevra-instructions.md",
        "altevra connect --tool claude-code",
    )
}

fn check_settings_managed(repo: &Path) -> DoctorCheck {
    check_managed_file(
        "settings_managed",
        &repo.join(".claude/settings.json"),
        "settings.json",
        "altevra connect --tool claude-code",
    )
}

fn check_managed_file(name: &str, path: &Path, label: &str, fix_cmd: &str) -> DoctorCheck {
    if !path.exists() {
        return DoctorCheck {
            name: name.into(),
            status: CheckStatus::Warn,
            message: format!("{label} not found"),
            fix_hint: Some(format!("Run: {fix_cmd}")),
        };
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    if content.contains("ALTEVRA_MANAGED: true") {
        DoctorCheck {
            name: name.into(),
            status: CheckStatus::Ok,
            message: format!("{label} present and managed"),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: name.into(),
            status: CheckStatus::Warn,
            message: format!("{label} exists but not managed by Altevra"),
            fix_hint: Some(format!("Run: {fix_cmd} (manual edit detected)")),
        }
    }
}

fn check_skills_installed(repo: &Path) -> DoctorCheck {
    let dir = repo.join(".claude/skills");
    if !dir.exists() {
        return DoctorCheck {
            name: "skills_installed".into(),
            status: CheckStatus::Warn,
            message: ".claude/skills/ not found".into(),
            fix_hint: Some("Run: altevra connect --tool claude-code".into()),
        };
    }
    let count = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    if count == 0 {
        DoctorCheck {
            name: "skills_installed".into(),
            status: CheckStatus::Warn,
            message: ".claude/skills/ empty — no skills installed".into(),
            fix_hint: Some("Run: altevra connect --tool claude-code".into()),
        }
    } else {
        DoctorCheck {
            name: "skills_installed".into(),
            status: CheckStatus::Ok,
            message: format!("{count} skill(s) installed in .claude/skills/"),
            fix_hint: None,
        }
    }
}

fn check_skills_parseable(vault: &Path) -> DoctorCheck {
    let dir = vault.join("06-skills");
    if !dir.exists() {
        return DoctorCheck {
            name: "skills_parseable".into(),
            status: CheckStatus::Warn,
            message: "06-skills/ not found — nothing to parse".into(),
            fix_hint: None,
        };
    }
    let mut errors: Vec<String> = vec![];
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if altevra_skills::parser::parse_skill(&content).is_err() {
                    errors.push(
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
    }
    if errors.is_empty() {
        DoctorCheck {
            name: "skills_parseable".into(),
            status: CheckStatus::Ok,
            message: "All skills parse cleanly".into(),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "skills_parseable".into(),
            status: CheckStatus::Fail,
            message: format!("Parse errors in: {}", errors.join(", ")),
            fix_hint: Some("Fix YAML frontmatter (slug, version, title required)".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_doctor_empty_dir_no_ok() {
        let tmp = TempDir::new().unwrap();
        let checks = run_checks(tmp.path(), tmp.path());
        let ok_count = checks
            .iter()
            .filter(|c| matches!(c.status, CheckStatus::Ok))
            .count();
        assert_eq!(ok_count, 0, "Empty dir should have no passing checks");
    }

    #[tokio::test]
    async fn test_doctor_initialized_vault_passes_core_checks() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();
        std::fs::write(
            tmp.path().join(".altevra/config.toml"),
            "vault_path = \".\"\nversion = \"0.1.0\"\n[database]\nurl = \"postgres://localhost/altevra\"\nmax_connections = 10\n",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();
        std::fs::write(
            tmp.path().join("06-skills/altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Test\n---\nBody.",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("07-capabilities")).unwrap();

        let checks = run_checks(tmp.path(), tmp.path());
        let vault = checks
            .iter()
            .find(|c| c.name == "vault_initialized")
            .unwrap();
        assert!(matches!(vault.status, CheckStatus::Ok));
        let skills = checks.iter().find(|c| c.name == "skills_dir").unwrap();
        assert!(matches!(skills.status, CheckStatus::Ok));
        let caps = checks
            .iter()
            .find(|c| c.name == "capabilities_dir")
            .unwrap();
        assert!(matches!(caps.status, CheckStatus::Ok));
    }

    #[tokio::test]
    async fn test_doctor_json_output() {
        let tmp = TempDir::new().unwrap();
        let args = DoctorArgs {
            json: true,
            repo: tmp.path().to_path_buf(),
            vault: tmp.path().to_path_buf(),
        };
        run(args).await.unwrap();
    }
}
