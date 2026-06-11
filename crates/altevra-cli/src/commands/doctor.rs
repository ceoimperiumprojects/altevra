use clap::Args;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let backups_dir = home.join(".altevra/backups/auto");
    let claude_projects_dir = home.join(".claude/projects");
    let codex_history = home.join(".codex/history.jsonl");

    let mut checks = run_checks_with_paths(
        &repo,
        &vault,
        &backups_dir,
        &claude_projects_dir,
        &codex_history,
    );
    // Environment-level checks (read $HOME) that live outside the hermetic set.
    checks.push(check_spool_empty());
    checks.push(check_embedding_lag_db());

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

/// Hermetic check set — all paths injectable for testing.
pub fn run_checks_with_paths(
    repo: &Path,
    vault: &Path,
    backups_dir: &Path,
    claude_projects_dir: &Path,
    codex_history_file: &Path,
) -> Vec<DoctorCheck> {
    vec![
        check_vault_initialized(vault),
        check_skills_dir(vault),
        check_capabilities_dir(vault),
        check_claude_connected(repo),
        check_instructions_managed(repo),
        check_settings_managed(repo),
        check_skills_installed(repo),
        check_skills_parseable(vault),
        check_brain_service_active(),
        check_backup_freshness(backups_dir),
        check_embedding_lag_files(vault),
        check_unimported_history(claude_projects_dir, codex_history_file),
        check_installed_skill_visibility(repo, vault),
    ]
}

/// Convenience wrapper used by CLI (derives paths from $HOME).
pub fn run_checks(repo: &Path, vault: &Path) -> Vec<DoctorCheck> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    run_checks_with_paths(
        repo,
        vault,
        &home.join(".altevra/backups/auto"),
        &home.join(".claude/projects"),
        &home.join(".codex/history.jsonl"),
    )
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

/// R6: Check if the altevra-brain systemd user service is active.
/// Graceful when systemd is absent (returns Warn, not Fail).
fn check_brain_service_active() -> DoctorCheck {
    use std::process::Command;
    match Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "altevra-brain"])
        .status()
    {
        Ok(s) if s.success() => DoctorCheck {
            name: "brain_service_active".into(),
            status: CheckStatus::Ok,
            message: "altevra-brain service is active".into(),
            fix_hint: None,
        },
        Ok(_) => DoctorCheck {
            name: "brain_service_active".into(),
            status: CheckStatus::Warn,
            message: "altevra-brain service is not active".into(),
            fix_hint: Some(
                "Run: systemctl --user start altevra-brain".into(),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DoctorCheck {
            name: "brain_service_active".into(),
            status: CheckStatus::Warn,
            message: "systemd not available — cannot check brain service".into(),
            fix_hint: Some("Start altevra-brain manually if needed".into()),
        },
        Err(e) => DoctorCheck {
            name: "brain_service_active".into(),
            status: CheckStatus::Warn,
            message: format!("Could not query brain service: {e}"),
            fix_hint: None,
        },
    }
}

/// R6: Newest file in `backups_dir` must be <48h old.
fn check_backup_freshness(backups_dir: &Path) -> DoctorCheck {
    let threshold = Duration::from_secs(48 * 3600);
    let now = SystemTime::now();

    let newest = match walkdir::WalkDir::new(backups_dir)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .max_by_key(|t| *t)
    {
        Some(t) => t,
        None => {
            return DoctorCheck {
                name: "backup_freshness".into(),
                status: CheckStatus::Warn,
                message: format!(
                    "No backups found in {} — run a backup first",
                    backups_dir.display()
                ),
                fix_hint: Some("Run: altevra backup create".into()),
            };
        }
    };

    let age = now.duration_since(newest).unwrap_or(threshold + Duration::from_secs(1));
    if age < threshold {
        DoctorCheck {
            name: "backup_freshness".into(),
            status: CheckStatus::Ok,
            message: format!("Most recent backup is {:.1}h old", age.as_secs_f64() / 3600.0),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "backup_freshness".into(),
            status: CheckStatus::Warn,
            message: format!(
                "Most recent backup is {:.1}h old (threshold: 48h)",
                age.as_secs_f64() / 3600.0
            ),
            fix_hint: Some("Run: altevra backup create".into()),
        }
    }
}

/// R6: Count of files in vault's `.altevra/pending_indexing` spool.
/// <10 = Ok, <50 = Warn, ≥50 = Fail.
fn check_embedding_lag_files(vault: &Path) -> DoctorCheck {
    let spool = vault.join(".altevra/pending_indexing");
    if !spool.exists() {
        return DoctorCheck {
            name: "embedding_lag_files".into(),
            status: CheckStatus::Ok,
            message: "embedding spool directory absent — indexing not started yet".into(),
            fix_hint: None,
        };
    }
    let count = std::fs::read_dir(&spool)
        .map(|d| d.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0);
    let (status, msg) = if count == 0 {
        (CheckStatus::Ok, "embedding spool empty — indexer up to date".to_string())
    } else if count < 50 {
        (
            CheckStatus::Warn,
            format!("{count} file(s) pending indexing in embedding spool"),
        )
    } else {
        (
            CheckStatus::Fail,
            format!("{count} files pending indexing — embedding lag is HIGH"),
        )
    };
    DoctorCheck {
        name: "embedding_lag_files".into(),
        status,
        message: msg,
        fix_hint: if count > 0 {
            Some("Run: altevra embed run --bge".into())
        } else {
            None
        },
    }
}

/// R6: DB-backed embedding queue depth via rusqlite (sync).
/// This check reads $HOME/…/altevra.db directly — outside the hermetic set.
fn check_embedding_lag_db() -> DoctorCheck {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let db_path = home.join(".altevra/altevra.db");
    if !db_path.exists() {
        return DoctorCheck {
            name: "embedding_lag_db".into(),
            status: CheckStatus::Ok,
            message: "DB not yet created — no embedding queue".into(),
            fix_hint: None,
        };
    }
    match rusqlite::Connection::open(&db_path) {
        Ok(conn) => {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM embedder_queue WHERE status='pending'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let (status, msg) = if count == 0 {
                (CheckStatus::Ok, "embedding DB queue empty".to_string())
            } else if count < 100 {
                (
                    CheckStatus::Warn,
                    format!("{count} turn(s) pending embedding in DB queue"),
                )
            } else {
                (
                    CheckStatus::Fail,
                    format!("{count} turns pending embedding — DB queue is HIGH"),
                )
            };
            DoctorCheck {
                name: "embedding_lag_db".into(),
                status,
                message: msg,
                fix_hint: if count > 0 {
                    Some("Run: altevra embed run --bge".into())
                } else {
                    None
                },
            }
        }
        Err(e) => DoctorCheck {
            name: "embedding_lag_db".into(),
            status: CheckStatus::Warn,
            message: format!("Could not open DB to check embedding queue: {e}"),
            fix_hint: None,
        },
    }
}

/// R6: Count unimported AI session files.
/// Counts `*.jsonl` files under `claude_projects_dir/**` and checks if
/// `codex_history_file` exists (as a simple presence hint).
fn check_unimported_history(claude_projects_dir: &Path, codex_history_file: &Path) -> DoctorCheck {
    let jsonl_count = if claude_projects_dir.exists() {
        walkdir::WalkDir::new(claude_projects_dir)
            .min_depth(2) // skip top-level project dirs themselves
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file()
                    && e.path().extension().map(|x| x == "jsonl").unwrap_or(false)
            })
            .count()
    } else {
        0
    };

    let codex_present = codex_history_file.exists();

    let total_hint = jsonl_count + if codex_present { 1 } else { 0 };

    if total_hint == 0 {
        DoctorCheck {
            name: "unimported_history".into(),
            status: CheckStatus::Ok,
            message: "No unimported AI session files detected".into(),
            fix_hint: None,
        }
    } else {
        let mut parts = vec![];
        if jsonl_count > 0 {
            parts.push(format!("{jsonl_count} Claude JSONL session file(s)"));
        }
        if codex_present {
            parts.push(format!("Codex history at {}", codex_history_file.display()));
        }
        DoctorCheck {
            name: "unimported_history".into(),
            status: CheckStatus::Warn,
            message: format!("Potential unimported sessions: {}", parts.join("; ")),
            fix_hint: Some("Run: altevra import claude-code  # or altevra import codex".into()),
        }
    }
}

/// R6: Cross-check vault `06-skills/*.md` slugs vs `.claude/skills/<slug>/` dirs.
/// Reports skills present in vault but not installed to `.claude/skills/`.
fn check_installed_skill_visibility(repo: &Path, vault: &Path) -> DoctorCheck {
    let skills_dir = vault.join("06-skills");
    let installed_dir = repo.join(".claude/skills");

    // Gather vault skill slugs.
    let vault_slugs: Vec<String> = if skills_dir.exists() {
        std::fs::read_dir(&skills_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.path().extension().map(|x| x == "md").unwrap_or(false)
            })
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .collect()
    } else {
        vec![]
    };

    if vault_slugs.is_empty() {
        return DoctorCheck {
            name: "installed_skill_visibility".into(),
            status: CheckStatus::Ok,
            message: "No vault skills to cross-check".into(),
            fix_hint: None,
        };
    }

    // Gather installed skill dirs.
    let installed_slugs: std::collections::HashSet<String> = if installed_dir.exists() {
        std::fs::read_dir(&installed_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                e.path()
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    let missing: Vec<&str> = vault_slugs
        .iter()
        .filter(|s| !installed_slugs.contains(*s))
        .map(|s| s.as_str())
        .collect();

    if missing.is_empty() {
        DoctorCheck {
            name: "installed_skill_visibility".into(),
            status: CheckStatus::Ok,
            message: format!(
                "All {} vault skill(s) are installed in .claude/skills/",
                vault_slugs.len()
            ),
            fix_hint: None,
        }
    } else {
        DoctorCheck {
            name: "installed_skill_visibility".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{} vault skill(s) not installed to .claude/skills/: {}",
                missing.len(),
                missing.join(", ")
            ),
            fix_hint: Some("Run: altevra skill sync --apply".into()),
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
        // Use run_checks_with_paths with all paths pointing to tmp so nothing
        // exists — the CORE infrastructure checks (vault, skills dir, etc.)
        // must not return Ok.
        let nonexistent = tmp.path().join("nonexistent");
        let checks = run_checks_with_paths(
            tmp.path(),
            tmp.path(),
            &nonexistent,
            &nonexistent,
            &nonexistent,
        );
        let core_check_names = [
            "vault_initialized",
            "skills_dir",
            "capabilities_dir",
            "claude_connected",
            "instructions_managed",
            "settings_managed",
            "skills_installed",
        ];
        for name in &core_check_names {
            let check = checks.iter().find(|c| c.name.as_str() == *name).expect(name);
            assert!(
                !matches!(check.status, CheckStatus::Ok),
                "Core check '{name}' must not be Ok in empty dir — got: {:?}",
                check.message
            );
        }
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

    // ─── R6 doctor extension tests ──────────────────────────────────────────

    #[test]
    fn backup_freshness_missing_dir_is_warn() {
        let tmp = TempDir::new().unwrap();
        let result = check_backup_freshness(&tmp.path().join("no_such_dir"));
        assert!(
            matches!(result.status, CheckStatus::Warn),
            "missing backups dir must be Warn"
        );
        assert_eq!(result.name, "backup_freshness");
    }

    #[test]
    fn backup_freshness_fresh_file_is_ok() {
        use filetime::FileTime;
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("backup.tar.zst");
        std::fs::write(&file, b"data").unwrap();
        // Explicitly set mtime to NOW so the test is deterministic.
        let now_ft = FileTime::now();
        filetime::set_file_mtime(&file, now_ft).unwrap();
        let result = check_backup_freshness(tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Ok),
            "fresh backup must be Ok, got: {:?}",
            result.message
        );
    }

    #[test]
    fn backup_freshness_stale_file_is_warn() {
        use filetime::FileTime;
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("old_backup.tar.zst");
        std::fs::write(&file, b"data").unwrap();
        // Set mtime to 72h ago.
        let stale = std::time::SystemTime::now()
            .checked_sub(Duration::from_secs(72 * 3600))
            .unwrap();
        filetime::set_file_mtime(&file, FileTime::from_system_time(stale)).unwrap();
        let result = check_backup_freshness(tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Warn),
            "stale backup must be Warn, got: {:?}",
            result.message
        );
    }

    #[test]
    fn unimported_history_no_dirs_is_ok() {
        let tmp = TempDir::new().unwrap();
        let result = check_unimported_history(
            &tmp.path().join("no_claude"),
            &tmp.path().join("no_codex.jsonl"),
        );
        assert!(
            matches!(result.status, CheckStatus::Ok),
            "no history dirs must be Ok"
        );
    }

    #[test]
    fn unimported_history_jsonl_files_is_warn() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("projects/-home-user-proj/conversations");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("session1.jsonl"), b"{}").unwrap();
        let result = check_unimported_history(
            &tmp.path().join("projects"),
            &tmp.path().join("no_codex.jsonl"),
        );
        assert!(
            matches!(result.status, CheckStatus::Warn),
            "jsonl files present must be Warn, got: {:?}",
            result.message
        );
    }

    #[test]
    fn unimported_history_codex_only_is_warn() {
        let tmp = TempDir::new().unwrap();
        let codex = tmp.path().join("codex_history.jsonl");
        std::fs::write(&codex, b"{}").unwrap();
        let result = check_unimported_history(
            &tmp.path().join("no_projects"),
            &codex,
        );
        assert!(
            matches!(result.status, CheckStatus::Warn),
            "codex history present must be Warn"
        );
    }

    #[test]
    fn embedding_lag_files_empty_is_ok() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join(".altevra/pending_indexing");
        std::fs::create_dir_all(&spool).unwrap();
        let result = check_embedding_lag_files(tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Ok),
            "empty spool must be Ok"
        );
    }

    #[test]
    fn embedding_lag_files_few_is_warn() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join(".altevra/pending_indexing");
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..5 {
            std::fs::write(spool.join(format!("file{i}.json")), b"{}").unwrap();
        }
        let result = check_embedding_lag_files(tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Warn),
            "5 files in spool must be Warn, got: {:?}",
            result.message
        );
    }

    #[test]
    fn embedding_lag_files_many_is_fail() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join(".altevra/pending_indexing");
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..60 {
            std::fs::write(spool.join(format!("file{i}.json")), b"{}").unwrap();
        }
        let result = check_embedding_lag_files(tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Fail),
            "60 files in spool must be Fail, got: {:?}",
            result.message
        );
    }

    #[test]
    fn skill_visibility_empty_vault_is_ok() {
        let tmp = TempDir::new().unwrap();
        let result = check_installed_skill_visibility(tmp.path(), tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Ok),
            "no vault skills must be Ok"
        );
    }

    #[test]
    fn skill_visibility_installed_is_ok() {
        let tmp = TempDir::new().unwrap();
        // Create vault skill.
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("my-skill.md"),
            "---\nslug: my-skill\nversion: 1.0.0\ntitle: T\n---\n",
        )
        .unwrap();
        // Create matching installed dir.
        let installed = tmp.path().join(".claude/skills/my-skill");
        std::fs::create_dir_all(&installed).unwrap();
        let result = check_installed_skill_visibility(tmp.path(), tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Ok),
            "installed skill must be Ok, got: {:?}",
            result.message
        );
    }

    #[test]
    fn skill_visibility_not_installed_is_warn() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("my-skill.md"),
            "---\nslug: my-skill\nversion: 1.0.0\ntitle: T\n---\n",
        )
        .unwrap();
        // .claude/skills/ exists but the subdirectory for "my-skill" is absent.
        std::fs::create_dir_all(tmp.path().join(".claude/skills")).unwrap();
        let result = check_installed_skill_visibility(tmp.path(), tmp.path());
        assert!(
            matches!(result.status, CheckStatus::Warn),
            "uninstalled skill must be Warn, got: {:?}",
            result.message
        );
    }
}
