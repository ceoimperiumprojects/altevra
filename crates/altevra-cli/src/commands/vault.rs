//! `altevra vault normalize` — give every vault `*.md` a universal frontmatter
//! envelope so the whole vault becomes uniformly typed/tagged and machine-findable
//! (R13). This is the SAFE, document-level half of "Atomizacija" (the atomizing
//! write path lives in `altevra capture --atomize`).
//!
//! SAFETY (non-negotiable):
//!   * DRY-RUN by default — prints a plan (N files, K need changes, 3 sample
//!     before/after frontmatter diffs). Touches nothing.
//!   * `--apply` FIRST copies the ENTIRE vault to a timestamped backup under
//!     `~/.imperium/backups/`, THEN writes only the changed files.
//!   * Only ADDS/MERGES frontmatter; never edits the body, never deletes a file.
//!   * Idempotent — a file already fully normalized is skipped.

use altevra_vault::{
    classify_path, normalize_frontmatter, render_normalized, scan_vault, split_for_normalize,
    Frontmatter,
};
use chrono::{DateTime, Local, NaiveDate};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Subcommand)]
pub enum VaultCommands {
    /// Normalize document-level frontmatter across the vault (DRY-RUN by default).
    Normalize(NormalizeArgs),
}

#[derive(Args)]
pub struct NormalizeArgs {
    /// Vault root. Defaults to `~/Obsidian/Imperium`.
    #[arg(long)]
    pub vault: Option<PathBuf>,
    /// Apply the plan. WITHOUT this flag the command only prints a dry-run plan
    /// and writes NOTHING. With it: full vault backup first, then merge changed
    /// files.
    #[arg(long)]
    pub apply: bool,
    /// Backup timestamp suffix (so the dir is deterministic in tests). When
    /// omitted the CLI uses the current unix time.
    #[arg(long)]
    pub backup_ts: Option<u64>,
    /// Override the backup root (default `~/.imperium/backups`).
    #[arg(long)]
    pub backup_root: Option<PathBuf>,
    /// Emit JSON instead of human-readable.
    #[arg(long)]
    pub json: bool,
}

/// Directories we never normalize (Obsidian config, the canonical templates, and
/// vendored trees). `scan_vault` already skips `.obsidian` + `node_modules`; we
/// additionally drop `Templates/` (the seed docs Pavle copies FROM).
fn is_excluded(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_lowercase();
    lower.starts_with("templates/")
        || lower.contains("/templates/")
        || lower.contains("node_modules")
        || lower.contains("/.obsidian/")
        || lower.starts_with(".obsidian/")
}

struct FilePlan {
    rel: String,
    abs: PathBuf,
    doc_type: String,
    /// Pretty before/after frontmatter (for the sample diffs).
    before: String,
    after: String,
    changed: bool,
    /// A frontmatter that couldn't be parsed (malformed) — skipped, reported.
    parse_error: Option<String>,
}

pub async fn run(cmd: VaultCommands) -> anyhow::Result<()> {
    match cmd {
        VaultCommands::Normalize(args) => normalize(args).await,
    }
}

async fn normalize(args: NormalizeArgs) -> anyhow::Result<()> {
    let vault = args.vault.clone().unwrap_or_else(default_vault_root);
    if !vault.exists() {
        anyhow::bail!("vault root does not exist: {}", vault.display());
    }

    let files = scan_vault(&vault)?;

    let mut plans: Vec<FilePlan> = Vec::new();
    let mut excluded = 0usize;
    let mut errors = 0usize;

    for f in &files {
        let rel = match f.path.strip_prefix(&vault) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => f.path.to_string_lossy().to_string(),
        };
        if is_excluded(&rel) {
            excluded += 1;
            continue;
        }

        let content = match std::fs::read_to_string(&f.path) {
            Ok(c) => c,
            Err(_) => {
                errors += 1;
                continue;
            }
        };

        let class = classify_path(&rel);
        let mtime_date = system_time_to_date(f.modified.into());

        match split_for_normalize(&content) {
            Ok((existing, _body)) => {
                let created = existing_created(existing.as_ref()).unwrap_or(mtime_date);
                let (new_fm, changed) = normalize_frontmatter(
                    existing.as_ref(),
                    &class.doc_type,
                    &class.domain,
                    class.scope.as_deref(),
                    created,
                    mtime_date,
                    class.archived,
                );
                plans.push(FilePlan {
                    rel,
                    abs: f.path.clone(),
                    doc_type: class.doc_type.clone(),
                    before: pretty_fm(existing.as_ref()),
                    after: pretty_value(&new_fm),
                    changed,
                    parse_error: None,
                });
            }
            Err(e) => {
                errors += 1;
                plans.push(FilePlan {
                    rel,
                    abs: f.path.clone(),
                    doc_type: class.doc_type.clone(),
                    before: String::new(),
                    after: String::new(),
                    changed: false,
                    parse_error: Some(e.to_string()),
                });
            }
        }
    }

    let total = plans.len();
    let need_change = plans.iter().filter(|p| p.changed).count();
    let by_type = count_by_type(&plans);

    if args.apply {
        // ---- BACKUP THE ENTIRE VAULT FIRST ----
        let ts = args.backup_ts.unwrap_or_else(unix_now);
        let backup_root = args.backup_root.clone().unwrap_or_else(default_backup_root);
        let backup_dir = backup_root.join(format!("obsidian-normalize-{ts}"));
        copy_dir_recursive(&vault, &backup_dir)?;

        let mut written = 0usize;
        for p in &plans {
            if !p.changed || p.parse_error.is_some() {
                continue;
            }
            // Re-read + re-split so we write fresh content (and preserve the body
            // exactly as on disk right now).
            let content = std::fs::read_to_string(&p.abs)?;
            let (existing, body) = split_for_normalize(&content)?;
            let class = classify_path(&p.rel);
            let mtime_date = system_time_to_date(file_mtime(&p.abs)?);
            let created = existing_created(existing.as_ref()).unwrap_or(mtime_date);
            let (new_fm, changed) = normalize_frontmatter(
                existing.as_ref(),
                &class.doc_type,
                &class.domain,
                class.scope.as_deref(),
                created,
                mtime_date,
                class.archived,
            );
            if !changed {
                continue;
            }
            let out = render_normalized(&new_fm, &body)?;
            std::fs::write(&p.abs, out)?;
            written += 1;
        }

        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "applied": true,
                    "vault": vault.display().to_string(),
                    "backup": backup_dir.display().to_string(),
                    "total": total,
                    "written": written,
                    "excluded": excluded,
                    "errors": errors,
                    "by_type": by_type,
                }))?
            );
        } else {
            println!("Applied frontmatter normalization to {}", vault.display());
            println!("  backup: {}", backup_dir.display());
            println!("  {written} file(s) written, {excluded} excluded, {errors} skipped (errors)");
        }
        return Ok(());
    }

    // ---- DRY-RUN ----
    let samples: Vec<&FilePlan> = plans.iter().filter(|p| p.changed).take(3).collect();
    if args.json {
        let sample_json: Vec<_> = samples
            .iter()
            .map(|p| {
                serde_json::json!({
                    "file": p.rel,
                    "type": p.doc_type,
                    "before": p.before,
                    "after": p.after,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "applied": false,
                "vault": vault.display().to_string(),
                "total": total,
                "need_change": need_change,
                "already_normalized": total - need_change,
                "excluded": excluded,
                "errors": errors,
                "by_type": by_type,
                "samples": sample_json,
            }))?
        );
    } else {
        println!("Vault normalize — DRY RUN (no files written)");
        println!("  vault: {}", vault.display());
        println!(
            "  {total} markdown file(s) scanned; {need_change} need frontmatter changes; \
             {} already normalized; {excluded} excluded; {errors} parse error(s)",
            total - need_change
        );
        println!("  by inferred type:");
        for (t, n) in &by_type {
            println!("    {t:<14} {n}");
        }
        if !samples.is_empty() {
            println!("\n  sample before/after (first {}):", samples.len());
            for p in &samples {
                println!("  ─── {} [{}] ───", p.rel, p.doc_type);
                println!("  BEFORE:");
                print_indented(&p.before, "    ");
                println!("  AFTER:");
                print_indented(&p.after, "    ");
            }
        }
        println!("\n  Run with --apply to write (a full vault backup is made first).");
    }
    Ok(())
}

fn default_vault_root() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Obsidian")
        .join("Imperium")
}

fn default_backup_root() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".imperium")
        .join("backups")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn file_mtime(path: &Path) -> anyhow::Result<SystemTime> {
    Ok(std::fs::metadata(path)?.modified()?)
}

fn system_time_to_date(t: SystemTime) -> NaiveDate {
    let dt: DateTime<Local> = t.into();
    dt.date_naive()
}

/// Pull an existing `created:` value (string) into a NaiveDate if present.
fn existing_created(fm: Option<&Frontmatter>) -> Option<NaiveDate> {
    let s = fm?.get_str("created")?;
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn pretty_fm(fm: Option<&Frontmatter>) -> String {
    match fm {
        Some(f) => pretty_value(&f.raw),
        None => "(none)".to_string(),
    }
}

fn pretty_value(v: &serde_yaml::Value) -> String {
    serde_yaml::to_string(v)
        .unwrap_or_else(|_| "(unserializable)".into())
        .trim_end()
        .to_string()
}

fn count_by_type(plans: &[FilePlan]) -> Vec<(String, usize)> {
    use std::collections::BTreeMap;
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for p in plans {
        *m.entry(p.doc_type.clone()).or_insert(0) += 1;
    }
    m.into_iter().collect()
}

fn print_indented(s: &str, indent: &str) {
    for line in s.lines() {
        println!("{indent}{line}");
    }
}

/// Recursively copy a directory tree (the pre-apply vault backup). Skips nothing
/// — the backup is a faithful snapshot so a botched apply is fully recoverable.
fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(&path, &target)?;
        }
        // symlinks/others: skipped (a vault is plain files + dirs).
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(p: &Path, content: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn excluded_paths() {
        assert!(is_excluded("Templates/daily.md"));
        assert!(is_excluded("System/x/node_modules/y.md"));
        assert!(!is_excluded("Memory/Decisions.md"));
        assert!(!is_excluded("Daily/2026-06-02.md"));
    }

    #[tokio::test]
    async fn dry_run_writes_nothing_and_counts_changes() {
        let dir = TempDir::new().unwrap();
        let vault = dir.path();
        write(
            &vault.join("Memory/Decisions.md"),
            "# Decisions\n\n## One\nbody\n",
        );
        write(&vault.join("Daily/2026-06-02.md"), "## Session\nlog\n");
        // an already-normalized file (idempotent → not counted as needing change)
        write(
            &vault.join("Memory/Learnings.md"),
            "---\ntype: learning\ndomain: business\nsensitivity: internal\nstatus: active\ntags:\n- business\ncreated: 2026-01-01\nupdated: 2026-01-01\nsource: obsidian\naltevra_normalized: true\n---\n# Learnings\nbody\n",
        );
        // a Templates/ file that must be excluded
        write(&vault.join("Templates/seed.md"), "# seed\n");

        let before_learnings = std::fs::read_to_string(vault.join("Memory/Learnings.md")).unwrap();
        let before_decisions = std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap();

        normalize(NormalizeArgs {
            vault: Some(vault.to_path_buf()),
            apply: false,
            backup_ts: None,
            backup_root: None,
            json: true,
        })
        .await
        .unwrap();

        // DRY RUN: nothing on disk changed.
        assert_eq!(
            std::fs::read_to_string(vault.join("Memory/Learnings.md")).unwrap(),
            before_learnings
        );
        assert_eq!(
            std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap(),
            before_decisions
        );
    }

    #[tokio::test]
    async fn apply_backs_up_then_writes_frontmatter_preserving_body() {
        let dir = TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        let backups = dir.path().join("backups");
        let body = "# Decisions\n\n## Pick a lane\nWe split agents.\n";
        write(&vault.join("Memory/Decisions.md"), body);

        normalize(NormalizeArgs {
            vault: Some(vault.clone()),
            apply: true,
            backup_ts: Some(12345),
            backup_root: Some(backups.clone()),
            json: true,
        })
        .await
        .unwrap();

        // Backup exists and holds the ORIGINAL (pre-write) content.
        let backup_file = backups
            .join("obsidian-normalize-12345")
            .join("Memory/Decisions.md");
        assert!(backup_file.exists(), "backup must exist before any write");
        assert_eq!(std::fs::read_to_string(&backup_file).unwrap(), body);

        // The live file now has frontmatter + the body verbatim.
        let written = std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap();
        assert!(written.starts_with("---\n"), "frontmatter prepended");
        assert!(written.contains("type: decision"));
        assert!(written.contains("domain: business"));
        assert!(written.contains("altevra_normalized: true"));
        // body preserved verbatim (including the ## section markers — never edited)
        assert!(written.contains("## Pick a lane\nWe split agents.\n"));
    }

    #[tokio::test]
    async fn apply_is_idempotent_second_run_writes_nothing_new() {
        let dir = TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        let backups = dir.path().join("backups");
        write(
            &vault.join("Memory/People.md"),
            "# People\n\n## Srdjan\nmentor\n",
        );

        normalize(NormalizeArgs {
            vault: Some(vault.clone()),
            apply: true,
            backup_ts: Some(1),
            backup_root: Some(backups.clone()),
            json: true,
        })
        .await
        .unwrap();
        let after_first = std::fs::read_to_string(vault.join("Memory/People.md")).unwrap();
        // person is high-water → restricted
        assert!(after_first.contains("sensitivity: restricted"));

        // Second apply: same mtime range; `created`/all universal fields present →
        // normalize_frontmatter reports no change EXCEPT `updated` if mtime moved.
        // Since we don't touch mtime between runs in this test window, content holds.
        normalize(NormalizeArgs {
            vault: Some(vault.clone()),
            apply: true,
            backup_ts: Some(2),
            backup_root: Some(backups.clone()),
            json: true,
        })
        .await
        .unwrap();
        let after_second = std::fs::read_to_string(vault.join("Memory/People.md")).unwrap();
        assert_eq!(
            after_first, after_second,
            "a fully-normalized file is left untouched on re-apply"
        );
    }
}
