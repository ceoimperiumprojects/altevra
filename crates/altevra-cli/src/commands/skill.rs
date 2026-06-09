use altevra_skills::{
    importer::{group_by_slug, scan_all, scan_external_dir, ExternalSkill, SourceTool},
    parser::parse_skill,
    registry::{SkillRegistry, VersionCheckResult},
    sync::{apply_plan, build_plan, SyncAction},
    watcher::{watch_loop, WatchConfig},
};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum SkillCommands {
    /// List all registered skills
    List(SkillListArgs),
    /// Show details of a specific skill
    Show(SkillShowArgs),
    /// Check if installed skills are current
    Check(SkillCheckArgs),
    /// Refresh a skill from source
    Refresh(SkillRefreshArgs),
    /// Inventory skills across ALL connected tools (~/.claude, ~/.codex,
    /// ~/.cursor, ~/.hermes, ~/.imperium, and the Altevra vault). Read-only.
    Inventory(SkillInventoryArgs),
    /// Propagate skills across tools. DRY-RUN by default — pass --apply to write.
    /// Never overwrites a non-managed (user-authored) file.
    Sync(SkillSyncArgs),
}

#[derive(Args)]
pub struct SkillSyncArgs {
    /// Write changes to disk. Without --apply this is a dry-run (no writes).
    #[arg(long)]
    pub apply: bool,
    /// Target tools to sync INTO. Default: all detected tools except Altevra
    /// (Altevra is treated as a source/canonical store).
    #[arg(long, value_delimiter = ',')]
    pub to: Vec<String>,
    /// Only consider skills with slugs matching this filter (substring).
    #[arg(long)]
    pub slug: Option<String>,
    /// Include Altevra vault `06-skills` (relative to --vault) as a source.
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    /// Stay running. After the initial sync, watch every known skill directory
    /// (`~/.{claude,codex,cursor,hermes,imperium}/skills/`) for changes and
    /// re-sync within --debounce-ms of every settled burst. Ctrl+C to stop.
    #[arg(long)]
    pub watch: bool,
    /// Coalesce window in milliseconds for watch mode (default 2000).
    #[arg(long, default_value_t = 2000)]
    pub debounce_ms: u64,
    /// SQLite database path — the guarded applier's drift manifest
    /// (managed_writes) lives here. Used only with --apply.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    /// Backup root for pre-write backups (guarded --apply path).
    /// Default: ~/.altevra/backups/sync/
    #[arg(long)]
    pub backup_root: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SkillInventoryArgs {
    /// Filter to skills present in a specific source tool.
    #[arg(long)]
    pub tool: Option<String>,
    /// Show ONLY skills not in every tool (diff view — sync candidates).
    #[arg(long)]
    pub missing: bool,
    /// Include Altevra vault `06-skills` in the scan (relative to --vault).
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SkillListArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

#[derive(Args)]
pub struct SkillShowArgs {
    pub slug: String,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

#[derive(Args)]
pub struct SkillCheckArgs {
    /// Check all skills
    #[arg(long)]
    pub all: bool,
    /// Specific skill slug
    pub slug: Option<String>,
    /// Currently installed version (for single check)
    #[arg(long)]
    pub installed_version: Option<String>,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SkillRefreshArgs {
    pub slug: String,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

pub async fn run(cmd: SkillCommands) -> anyhow::Result<()> {
    match cmd {
        SkillCommands::List(args) => run_list(args).await,
        SkillCommands::Show(args) => run_show(args).await,
        SkillCommands::Check(args) => run_check(args).await,
        SkillCommands::Refresh(args) => run_refresh(args).await,
        SkillCommands::Inventory(args) => run_inventory(args).await,
        SkillCommands::Sync(args) => run_sync(args).await,
    }
}

/// Resolve where a tool keeps its skills on disk. Mirrors `default_skill_dirs`
/// but returns the canonical write location even if the dir doesn't exist yet
/// (the writer will create it).
fn skill_dir_for(tool: &SourceTool) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    match tool {
        SourceTool::Claude => Some(home.join(".claude/skills")),
        SourceTool::Codex => Some(home.join(".codex/skills")),
        SourceTool::Cursor => Some(home.join(".cursor/skills")),
        SourceTool::Hermes => Some(home.join(".hermes/skills")),
        SourceTool::Imperium => Some(home.join(".imperium/skills")),
        // Altevra/Other have no canonical external write target.
        _ => None,
    }
}

async fn run_sync(args: SkillSyncArgs) -> anyhow::Result<()> {
    // Inventory the world (all tool dirs + vault).
    let mut inventory: Vec<ExternalSkill> = scan_all();
    let vault_dir = args.vault.join("06-skills");
    if vault_dir.exists() {
        inventory.extend(scan_external_dir(&vault_dir, SourceTool::Altevra));
    }
    if let Some(filter) = args.slug.as_deref() {
        inventory.retain(|s| s.slug.contains(filter));
    }

    // Resolve targets: explicit --to, else all writable adapters.
    let targets: Vec<SourceTool> = if args.to.is_empty() {
        vec![
            SourceTool::Claude,
            SourceTool::Codex,
            SourceTool::Cursor,
            SourceTool::Hermes,
            SourceTool::Imperium,
        ]
    } else {
        let mut ts = Vec::new();
        for name in &args.to {
            match name.as_str() {
                "claude" => ts.push(SourceTool::Claude),
                "codex" => ts.push(SourceTool::Codex),
                "cursor" => ts.push(SourceTool::Cursor),
                "hermes" => ts.push(SourceTool::Hermes),
                "imperium" => ts.push(SourceTool::Imperium),
                other => anyhow::bail!(
                    "unknown --to tool '{other}' (valid: claude, codex, cursor, hermes, imperium)"
                ),
            }
        }
        ts
    };

    let plan = build_plan(&inventory, &targets, &skill_dir_for);
    // P3 install/sync: real writes go through the GUARDED applier (drift
    // manifest + backups + TOCTOU re-verify + review routing); dry-run keeps
    // the cheap no-DB path.
    let (result, drift_refused) = if args.apply {
        let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
        altevra_db::run_migrations(&pool).await?;
        let backup_root = args
            .backup_root
            .clone()
            .unwrap_or_else(|| altevra_core::home_dir().join(".altevra/backups/sync"));
        let g = crate::commands::skill_sync::guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await?;
        (
            altevra_skills::sync::SyncResult {
                created: g.created,
                refreshed: g.refreshed,
                skipped: g.skipped,
                errors: g.errors.clone(),
            },
            g.drift_refused,
        )
    } else {
        (apply_plan(&plan, false), Vec::new())
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": !args.apply,
                "targets": targets.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "creates_planned": plan.creates(),
                "refreshes_planned": plan.refreshes(),
                "skips_planned": plan.skips(),
                "applied": args.apply,
                "created": result.created,
                "refreshed": result.refreshed,
                "skipped": result.skipped,
                "drift_refused": drift_refused,
                "errors": result.errors,
                "actions": plan.actions,
            }))?
        );
    } else {
        let mode = if args.apply { "APPLY" } else { "DRY-RUN" };
        println!(
            "Skill sync [{mode}] — targets: {}\n",
            targets
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "  creates:   {}\n  refreshes: {}\n  skips:     {}",
            plan.creates(),
            plan.refreshes(),
            plan.skips()
        );
        if !plan.actions.is_empty() {
            println!("\nFirst few actions:");
            for a in plan.actions.iter().take(15) {
                match a {
                    SyncAction::Create {
                        slug,
                        from_tool,
                        to_tool,
                        ..
                    } => println!(
                        "  + create  {slug:32}  {}→{}",
                        from_tool.as_str(),
                        to_tool.as_str()
                    ),
                    SyncAction::Refresh {
                        slug,
                        from_tool,
                        to_tool,
                        ..
                    } => println!(
                        "  ↻ refresh {slug:32}  {}→{}",
                        from_tool.as_str(),
                        to_tool.as_str()
                    ),
                    SyncAction::Skip {
                        slug,
                        to_tool,
                        reason,
                        ..
                    } => println!(
                        "  - skip    {slug:32}  →{}  ({:?})",
                        to_tool.as_str(),
                        reason
                    ),
                }
            }
            if plan.actions.len() > 15 {
                println!("  …({} more)", plan.actions.len() - 15);
            }
        }
        if args.apply {
            println!(
                "\nApplied — created: {}, refreshed: {}, skipped: {}, drift-refused: {}, errors: {}",
                result.created,
                result.refreshed,
                result.skipped,
                drift_refused.len(),
                result.errors.len()
            );
            for d in &drift_refused {
                eprintln!("  ⚠ drift (refused, routed to review): {d}");
            }
            for e in &result.errors {
                eprintln!("  ! {e}");
            }
        } else {
            println!("\n(dry-run — no files written. Re-run with --apply to write.)");
        }
    }

    // --watch: block on the long-running watcher AFTER the initial sync.
    if args.watch {
        if args.apply {
            eprintln!(
                "⚠ watch-mode applies use the UNGUARDED writer (managed-marker check only — \
                 no drift manifest/backups). Foreground `skill sync --apply` runs are guarded."
            );
        }
        let cfg = WatchConfig {
            targets,
            vault_skills_dir: Some(args.vault.join("06-skills")).filter(|p| p.exists()),
            apply: args.apply,
            debounce_ms: args.debounce_ms,
        };
        let mode = if cfg.apply { "APPLY" } else { "DRY-RUN" };
        eprintln!(
            "\n📡 Watching {} dir(s) for skill changes [{mode}, debounce {}ms]. Ctrl+C to stop.",
            altevra_skills::importer::default_skill_dirs().len()
                + cfg.vault_skills_dir.as_ref().map_or(0, |_| 1),
            cfg.debounce_ms
        );
        // Spawn the blocking watcher on a dedicated thread so we can listen for
        // Ctrl+C in the async runtime in parallel.
        let cfg_clone = cfg.clone();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || -> anyhow::Result<()> {
            watch_loop(cfg_clone, |report| {
                let trig = report
                    .triggering_paths
                    .iter()
                    .take(3)
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = if report.triggering_paths.len() > 3 {
                    format!(" (+{} more)", report.triggering_paths.len() - 3)
                } else {
                    String::new()
                };
                let mode = if cfg.apply { "applied" } else { "planned" };
                eprintln!(
                    "↻ cycle: triggers={trig}{more} | {mode} creates={} refreshes={} skips={}{}",
                    report.plan_creates,
                    report.plan_refreshes,
                    report.plan_skips,
                    if !report.result.errors.is_empty() {
                        format!(" errors={}", report.result.errors.len())
                    } else {
                        String::new()
                    }
                );
                // Non-blocking check: did anyone signal stop?
                stop_rx.try_recv().is_err()
            })
        });

        // Wait for Ctrl+C or the watcher thread exiting.
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\nstopping watcher…");
        let _ = stop_tx.send(());
        // The watcher checks `stop_rx` on the NEXT event/timeout (≤500ms).
        let _ = handle.join();
    }
    Ok(())
}

async fn run_inventory(args: SkillInventoryArgs) -> anyhow::Result<()> {
    // Scan every known tool's skill dir + (optionally) the Altevra vault.
    let mut skills: Vec<ExternalSkill> = scan_all();
    let vault_skills_dir = args.vault.join("06-skills");
    if vault_skills_dir.exists() {
        skills.extend(scan_external_dir(&vault_skills_dir, SourceTool::Altevra));
    }

    // Optional --tool filter.
    if let Some(t) = args.tool.as_deref() {
        skills.retain(|s| s.source_tool.as_str() == t);
    }

    let grouped = group_by_slug(&skills);

    // --missing: show only skills NOT present in every distinct tool we found.
    let tools_in_scan: std::collections::BTreeSet<&str> =
        skills.iter().map(|s| s.source_tool.as_str()).collect();
    let total_tools = tools_in_scan.len();
    let mut rows: Vec<_> = grouped.iter().collect();
    if args.missing {
        rows.retain(|(_, list)| {
            let distinct_tools: std::collections::HashSet<_> =
                list.iter().map(|s| s.source_tool.as_str()).collect();
            distinct_tools.len() < total_tools
        });
    }
    rows.sort_by(|a, b| a.0.cmp(b.0));

    if args.json {
        let out: Vec<_> = rows
            .iter()
            .map(|(slug, list)| {
                serde_json::json!({
                    "slug": slug,
                    "tools": list.iter().map(|s| s.source_tool.as_str()).collect::<Vec<_>>(),
                    "managed": list.iter().any(|s| s.managed),
                    "instances": list.iter().map(|s| serde_json::json!({
                        "tool": s.source_tool.as_str(),
                        "path": s.path,
                        "version": s.version,
                        "description": s.description,
                        "managed": s.managed,
                        "body_len": s.body_len,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "Skill inventory — {} unique slug(s) across {} tool(s){}:\n",
            rows.len(),
            total_tools,
            if args.missing {
                " (missing-in-some only)"
            } else {
                ""
            }
        );
        for (slug, list) in &rows {
            let tools: Vec<&str> = {
                let mut v: Vec<_> = list.iter().map(|s| s.source_tool.as_str()).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            let has_managed = list.iter().any(|s| s.managed);
            println!(
                "  {slug:32}  [{tools}]{m}",
                tools = tools.join(","),
                m = if has_managed { "  (managed)" } else { "" }
            );
        }
        println!("\nNext: `altevra skill inventory --missing` to see propagation candidates.");
    }
    Ok(())
}

async fn run_list(args: SkillListArgs) -> anyhow::Result<()> {
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");

    if skills_dir.exists() {
        load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    let entries = registry.list();

    if args.json {
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "slug": e.slug(),
                    "version": e.skill.frontmatter.version,
                    "title": e.skill.frontmatter.title,
                    "source_path": e.source_path,
                    "checksum": e.checksum,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "skills": items,
                "count": items.len()
            }))?
        );
    } else {
        if entries.is_empty() {
            println!("No skills found in: {}", skills_dir.display());
            println!("Hint: Add skill files to 06-skills/");
        } else {
            println!("Skills ({}):", entries.len());
            for e in entries {
                let v = e
                    .version()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!("  {} v{} — {}", e.slug(), v, e.skill.frontmatter.title);
            }
        }
    }

    Ok(())
}

async fn run_show(args: SkillShowArgs) -> anyhow::Result<()> {
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");
    if skills_dir.exists() {
        load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    match registry.get(&args.slug) {
        Some(entry) => {
            println!("Slug:     {}", entry.slug());
            println!("Title:    {}", entry.skill.frontmatter.title);
            println!("Version:  {}", entry.skill.frontmatter.version);
            println!("Checksum: {}", entry.checksum);
            println!("Source:   {}", entry.source_path);
            if let Some(desc) = &entry.skill.frontmatter.description {
                println!("Description: {desc}");
            }
            println!("\n--- Body ---\n{}", entry.skill.body);
        }
        None => {
            anyhow::bail!("Skill not found: {}", args.slug);
        }
    }
    Ok(())
}

async fn run_check(args: SkillCheckArgs) -> anyhow::Result<()> {
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");
    if skills_dir.exists() {
        load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    let slugs_to_check: Vec<String> = if args.all {
        registry
            .list()
            .iter()
            .map(|e| e.slug().to_string())
            .collect()
    } else if let Some(slug) = args.slug.clone() {
        vec![slug]
    } else {
        vec!["altevra-core".to_string()]
    };

    let mut results = vec![];
    for slug in &slugs_to_check {
        let installed = args.installed_version.as_deref();
        let check_result = registry.check_version_opt(slug, installed);
        results.push(serde_json::json!({
            "slug": slug,
            "status": match &check_result {
                VersionCheckResult::Current => "current",
                VersionCheckResult::Outdated { .. } => "outdated",
                VersionCheckResult::Ahead { .. } => "ahead",
                VersionCheckResult::NotInstalled => "not_installed",
                VersionCheckResult::NotFound => "not_found",
                VersionCheckResult::ParseError => "parse_error",
            },
            "detail": check_result.to_string(),
        }));
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "checks": results
            }))?
        );
    } else {
        for r in &results {
            let icon = match r["status"].as_str().unwrap_or("") {
                "current" => "✓",
                "outdated" => "⚠",
                "not_installed" | "not_found" => "✗",
                _ => "?",
            };
            println!(
                "{icon} {} — {}",
                r["slug"].as_str().unwrap_or(""),
                r["detail"].as_str().unwrap_or("")
            );
        }
    }

    Ok(())
}

async fn run_refresh(args: SkillRefreshArgs) -> anyhow::Result<()> {
    use altevra_adapters::{ClaudeCodeAdapter, ToolAdapter};

    let skills_dir = args.vault.join("06-skills");
    let skill_file = skills_dir.join(format!("{}.md", args.slug));

    if !skill_file.exists() {
        anyhow::bail!(
            "Skill '{}' not found in vault (looked at {})",
            args.slug,
            skill_file.display()
        );
    }

    let content = std::fs::read_to_string(&skill_file)?;
    let skill = altevra_skills::parser::parse_skill(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse skill '{}': {e}", args.slug))?;

    // Folder-per-skill layout: .claude/skills/<slug>/SKILL.md
    let dest = std::path::Path::new(".claude/skills")
        .join(&args.slug)
        .join("SKILL.md");

    // Drift protection: if file exists without managed header, refuse to overwrite.
    if dest.exists() {
        let existing = std::fs::read_to_string(&dest).unwrap_or_default();
        if !existing.contains("ALTEVRA_MANAGED: true") {
            anyhow::bail!(
                "Drift detected: {} exists but is not managed by Altevra. Remove it manually if you want to refresh.",
                dest.display()
            );
        }
    }

    let adapter = ClaudeCodeAdapter::new();
    let files = adapter.render_skills(vec![&skill])?;
    let rendered = files
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Adapter returned no files for skill '{}'", args.slug))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, &rendered.content)?;

    println!("Refreshed: {} → {}", skill_file.display(), dest.display());
    Ok(())
}

pub fn load_skills_from_dir(registry: &mut SkillRegistry, dir: &Path) -> anyhow::Result<usize> {
    let mut count = 0;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            let content = std::fs::read_to_string(&path)?;
            if let Ok(_parsed) = parse_skill(&content) {
                let path_str = path.display().to_string();
                if registry.register(&path_str, &content).is_ok() {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_skill_file(dir: &Path, slug: &str, version: &str) {
        let content = format!(
            "---\nslug: {slug}\nversion: {version}\ntitle: Test Skill {slug}\n---\n\nBody here."
        );
        std::fs::write(dir.join(format!("{slug}.md")), content).unwrap();
    }

    #[tokio::test]
    async fn test_skill_list_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();
        let args = SkillListArgs {
            json: true,
            vault: tmp.path().to_path_buf(),
        };
        run_list(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_skill_list_with_skills() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        make_skill_file(&skills_dir, "altevra-core", "0.5.0");
        make_skill_file(&skills_dir, "test-skill", "1.2.3");

        let args = SkillListArgs {
            json: true,
            vault: tmp.path().to_path_buf(),
        };
        run_list(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_skill_check_all() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        make_skill_file(&skills_dir, "altevra-core", "0.5.0");

        let args = SkillCheckArgs {
            all: true,
            slug: None,
            installed_version: Some("0.5.0".to_string()),
            vault: tmp.path().to_path_buf(),
            json: true,
        };
        run_check(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_skill_refresh_missing_slug_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();
        let args = SkillRefreshArgs {
            slug: "nonexistent-skill".to_string(),
            vault: tmp.path().to_path_buf(),
        };
        assert!(run_refresh(args).await.is_err());
    }

    #[tokio::test]
    async fn test_skill_refresh_writes_file() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        make_skill_file(&skills_dir, "altevra-core", "0.5.0");

        // Change to tmp dir so .claude/skills/ is created there.
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let args = SkillRefreshArgs {
            slug: "altevra-core".to_string(),
            vault: tmp.path().to_path_buf(),
        };
        let result = run_refresh(args).await;
        std::env::set_current_dir(orig_dir).unwrap();

        assert!(result.is_ok());
        // Folder-per-skill layout.
        assert!(tmp
            .path()
            .join(".claude/skills/altevra-core/SKILL.md")
            .exists());
    }
}
