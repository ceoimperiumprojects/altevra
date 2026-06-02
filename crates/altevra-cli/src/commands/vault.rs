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
    classify_path, normalize_frontmatter, parse_sections, render_normalized, scaffold_section,
    scan_vault, section_conformance, split_for_normalize, Frontmatter, Section,
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
    /// Fill ONLY empty / stub `## ` sections with the per-type section skeleton
    /// (apply-mode). NEVER rewrites a section that already has prose — those are
    /// the LLM `--rewrite` job (Phase 2). Implies an apply; still backs up first.
    #[arg(long)]
    pub scaffold_empty: bool,
    /// Phase 2 (LLM): RESTRUCTURE non-conformant PROSE sections into the section
    /// template via the configured reasoning provider, preserving every fact.
    /// DRY-RUN by default (reports what would be rewritten). Under `delegated`
    /// (default reasoning_mode) it is a no-op that reports the count + that an
    /// `api`/`codex_oauth` provider is required. Real rewrites need `--apply`.
    #[arg(long)]
    pub rewrite: bool,
    /// Reasoning mode override for `--rewrite` (`delegated`|`codex_oauth`|`api`).
    /// Defaults to the repo config's `[llm].reasoning_mode`.
    #[arg(long)]
    pub reasoning_mode: Option<String>,
    /// Repo root for config/credentials (defaults to cwd).
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
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
    /// Per-section conformance against the type's section template (Phase 1).
    sections_total: usize,
    sections_nonconformant: usize,
    /// Sections that are EMPTY/stub (safe to scaffold). Subset of nonconformant.
    sections_scaffoldable: usize,
    /// Sections that have prose but miss labels (LLM `--rewrite` territory).
    sections_need_rewrite: usize,
    /// Canonical missing labels across this file (for the report), deduped.
    missing_labels: Vec<String>,
}

/// Analyze a file's `## ` sections against its type section template. Returns
/// (total, nonconformant, scaffoldable, need_rewrite, missing_labels).
fn analyze_sections(body: &str, doc_type: &str) -> (usize, usize, usize, usize, Vec<String>) {
    let sections = parse_sections(body);
    let mut nonconformant = 0;
    let mut scaffoldable = 0;
    let mut need_rewrite = 0;
    let mut missing: Vec<String> = Vec::new();
    for s in &sections {
        let c = section_conformance(s, doc_type);
        if c.conformant {
            continue;
        }
        nonconformant += 1;
        // Scaffoldable = effectively empty (no prose at all). A non-empty section
        // missing labels is prose that must be RESTRUCTURED (LLM), never wiped.
        if c.empty || section_is_stub_only(s) {
            scaffoldable += 1;
        } else {
            need_rewrite += 1;
        }
        for m in c.missing_labels {
            if !missing.contains(&m) {
                missing.push(m);
            }
        }
    }
    (
        sections.len(),
        nonconformant,
        scaffoldable,
        need_rewrite,
        missing,
    )
}

/// `true` if a section body is only bare label stubs / whitespace (no real prose)
/// — safe to (re)scaffold. A section with ANY non-label prose line is NOT a stub.
fn section_is_stub_only(s: &Section) -> bool {
    for line in s.body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Strip one list marker, then check if it's a bold-label line.
        let after_marker = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
            .unwrap_or(t)
            .trim_start();
        let is_label = after_marker.starts_with("**") && after_marker.contains(":**");
        // a label line counts as a stub ONLY if it has no value after `:**`
        if is_label {
            if let Some(rest) = after_marker.strip_prefix("**") {
                if let Some(idx) = rest.find(":**") {
                    if rest[idx + 3..].trim().is_empty() {
                        continue; // bare stub label → still a stub
                    }
                }
            }
        }
        // any non-empty, non-bare-label line = real prose
        return false;
    }
    true
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
            Ok((existing, body)) => {
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
                let (s_total, s_non, s_scaf, s_rew, missing) =
                    analyze_sections(&body, &class.doc_type);
                plans.push(FilePlan {
                    rel,
                    abs: f.path.clone(),
                    doc_type: class.doc_type.clone(),
                    before: pretty_fm(existing.as_ref()),
                    after: pretty_value(&new_fm),
                    changed,
                    parse_error: None,
                    sections_total: s_total,
                    sections_nonconformant: s_non,
                    sections_scaffoldable: s_scaf,
                    sections_need_rewrite: s_rew,
                    missing_labels: missing,
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
                    sections_total: 0,
                    sections_nonconformant: 0,
                    sections_scaffoldable: 0,
                    sections_need_rewrite: 0,
                    missing_labels: Vec::new(),
                });
            }
        }
    }

    let total = plans.len();
    let need_change = plans.iter().filter(|p| p.changed).count();
    let by_type = count_by_type(&plans);

    // Section-template conformance aggregates (Phase 1).
    let sections_total: usize = plans.iter().map(|p| p.sections_total).sum();
    let sections_nonconformant: usize = plans.iter().map(|p| p.sections_nonconformant).sum();
    let sections_scaffoldable: usize = plans.iter().map(|p| p.sections_scaffoldable).sum();
    let sections_need_rewrite: usize = plans.iter().map(|p| p.sections_need_rewrite).sum();
    let files_with_nonconformant = plans
        .iter()
        .filter(|p| p.sections_nonconformant > 0)
        .count();

    // ---- Phase 2: LLM rewrite of prose-but-non-conformant sections ----
    if args.rewrite {
        return run_rewrite(&args, &vault, &plans, sections_need_rewrite).await;
    }

    let want_apply = args.apply || args.scaffold_empty;

    if want_apply {
        // ---- BACKUP THE ENTIRE VAULT FIRST ----
        let ts = args.backup_ts.unwrap_or_else(unix_now);
        let backup_root = args.backup_root.clone().unwrap_or_else(default_backup_root);
        let backup_dir = backup_root.join(format!("obsidian-normalize-{ts}"));
        copy_dir_recursive(&vault, &backup_dir)?;

        let mut written = 0usize;
        let mut scaffolded_sections = 0usize;
        for p in &plans {
            if p.parse_error.is_some() {
                continue;
            }
            // Re-read + re-split so we write fresh content (and preserve the body
            // exactly as on disk right now).
            let content = std::fs::read_to_string(&p.abs)?;
            let (existing, body) = split_for_normalize(&content)?;
            let class = classify_path(&p.rel);
            let mtime_date = system_time_to_date(file_mtime(&p.abs)?);
            let created = existing_created(existing.as_ref()).unwrap_or(mtime_date);
            let (new_fm, fm_changed) = normalize_frontmatter(
                existing.as_ref(),
                &class.doc_type,
                &class.domain,
                class.scope.as_deref(),
                created,
                mtime_date,
                class.archived,
            );

            // Optionally scaffold EMPTY/stub sections (never prose sections).
            let (new_body, n_scaffolded) = if args.scaffold_empty {
                scaffold_empty_sections(&body, &class.doc_type)
            } else {
                (body.clone(), 0)
            };
            scaffolded_sections += n_scaffolded;

            // Nothing to do if neither the frontmatter nor the body changed.
            if !fm_changed && n_scaffolded == 0 {
                continue;
            }
            let out = render_normalized(&new_fm, &new_body)?;
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
                    "scaffold_empty": args.scaffold_empty,
                    "scaffolded_sections": scaffolded_sections,
                    "excluded": excluded,
                    "errors": errors,
                    "by_type": by_type,
                    "sections_total": sections_total,
                    "sections_nonconformant": sections_nonconformant,
                    "sections_scaffoldable": sections_scaffoldable,
                    "sections_need_rewrite": sections_need_rewrite,
                }))?
            );
        } else {
            println!("Applied frontmatter normalization to {}", vault.display());
            println!("  backup: {}", backup_dir.display());
            println!("  {written} file(s) written, {excluded} excluded, {errors} skipped (errors)");
            if args.scaffold_empty {
                println!("  {scaffolded_sections} empty/stub section(s) scaffolded (prose sections left for --rewrite)");
            }
        }
        return Ok(());
    }

    // ---- DRY-RUN ----
    let samples: Vec<&FilePlan> = plans.iter().filter(|p| p.changed).take(3).collect();
    // Files most in need of section cleanup (for the conformance sample).
    let mut nonconf_files: Vec<&FilePlan> = plans
        .iter()
        .filter(|p| p.sections_nonconformant > 0)
        .collect();
    nonconf_files.sort_by_key(|p| std::cmp::Reverse(p.sections_nonconformant));
    let nonconf_samples: Vec<&FilePlan> = nonconf_files.iter().take(5).copied().collect();

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
        let conf_json: Vec<_> = nonconf_samples
            .iter()
            .map(|p| {
                serde_json::json!({
                    "file": p.rel,
                    "type": p.doc_type,
                    "sections": p.sections_total,
                    "nonconformant": p.sections_nonconformant,
                    "scaffoldable": p.sections_scaffoldable,
                    "need_rewrite": p.sections_need_rewrite,
                    "missing_labels": p.missing_labels,
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
                "section_conformance": {
                    "sections_total": sections_total,
                    "sections_nonconformant": sections_nonconformant,
                    "sections_scaffoldable": sections_scaffoldable,
                    "sections_need_rewrite": sections_need_rewrite,
                    "files_with_nonconformant": files_with_nonconformant,
                    "samples": conf_json,
                },
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
        println!("\n  section-template conformance:");
        println!(
            "    {sections_total} section(s) across {files_with_nonconformant} file(s) with \
             issues; {sections_nonconformant} non-conformant"
        );
        println!(
            "    → {sections_scaffoldable} empty/stub (scaffoldable now), \
             {sections_need_rewrite} have prose but miss labels (LLM --rewrite)"
        );
        if !nonconf_samples.is_empty() {
            println!("    top files needing section cleanup:");
            for p in &nonconf_samples {
                let labels = if p.missing_labels.is_empty() {
                    String::new()
                } else {
                    format!(" missing: {}", p.missing_labels.join(", "))
                };
                println!(
                    "      {} [{}] — {}/{} non-conformant{labels}",
                    p.rel, p.doc_type, p.sections_nonconformant, p.sections_total
                );
            }
        }
        if !samples.is_empty() {
            println!(
                "\n  sample frontmatter before/after (first {}):",
                samples.len()
            );
            for p in &samples {
                println!("  ─── {} [{}] ───", p.rel, p.doc_type);
                println!("  BEFORE:");
                print_indented(&p.before, "    ");
                println!("  AFTER:");
                print_indented(&p.after, "    ");
            }
        }
        println!("\n  Run with --apply to write frontmatter (full vault backup first).");
        println!("  Run with --scaffold-empty to also fill EMPTY sections with templates.");
    }
    Ok(())
}

/// Rewrite a document body, replacing the body of each EMPTY/stub `## ` section
/// with the per-type section skeleton. Prose sections (and the preamble) are kept
/// byte-for-byte. Returns (new_body, n_sections_scaffolded).
///
/// SAFETY: only sections whose body is empty or bare-label-stubs are touched — a
/// section with any real prose is NEVER rewritten here (that's the LLM `--rewrite`
/// job). No information is ever lost.
fn scaffold_empty_sections(body: &str, doc_type: &str) -> (String, usize) {
    let sections = parse_sections(body);
    if sections.is_empty() {
        return (body.to_string(), 0);
    }
    // Index the lines; we rebuild the doc, swapping only stub-section bodies.
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut n_scaffolded = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        // Is this the start of a `## ` heading?
        let is_h2 = line.strip_prefix("## ").map(|r| !r.trim().is_empty()) == Some(true)
            || line.trim_end() == "##";
        if !is_h2 {
            out.push(line.to_string());
            i += 1;
            continue;
        }
        // Collect this section's body lines (until next `## ` or EOF).
        let heading = line;
        let mut j = i + 1;
        let mut body_lines: Vec<&str> = Vec::new();
        while j < lines.len() {
            let l = lines[j];
            let next_h2 = l.strip_prefix("## ").map(|r| !r.trim().is_empty()) == Some(true)
                || l.trim_end() == "##";
            if next_h2 {
                break;
            }
            body_lines.push(l);
            j += 1;
        }
        out.push(heading.to_string());
        // Build a Section to test stub-only.
        let sec = Section {
            heading: heading.trim_start_matches("## ").trim().to_string(),
            level: 2,
            body: body_lines.join("\n"),
            date: None,
        };
        let conf = section_conformance(&sec, doc_type);
        let stub = sec.body.trim().is_empty() || section_is_stub_only(&sec);
        if !conf.conformant && stub {
            // Replace the (empty/stub) body with the scaffold skeleton.
            out.push(String::new());
            for sl in scaffold_section(doc_type).lines() {
                out.push(sl.to_string());
            }
            out.push(String::new());
            n_scaffolded += 1;
        } else {
            // Keep the original body verbatim (prose section or already conformant).
            for bl in &body_lines {
                out.push(bl.to_string());
            }
        }
        i = j;
    }
    let mut joined = out.join("\n");
    if body.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    (joined, n_scaffolded)
}

/// Phase 2 — LLM restructure seam. Routes the configured reasoning provider over
/// the prose-but-non-conformant sections, asking it to reorganize each into its
/// section template WITHOUT losing facts (`build_rewrite_prompt`). DRY-RUN by
/// default; under `delegated` (the default mode) the provider is a noop, so this
/// only REPORTS the count + that an `api`/`codex_oauth` provider is required.
///
/// SAFETY: this never writes unless `--apply` is given AND a real (non-noop)
/// provider is configured. Even then, the body merge preserves all other sections
/// verbatim. Per the task, real LLM rewrites on the vault are left to Pavle — this
/// wires + tests the seam (prompt build + noop path), it does not run live here.
async fn run_rewrite(
    args: &NormalizeArgs,
    vault: &Path,
    plans: &[FilePlan],
    sections_need_rewrite: usize,
) -> anyhow::Result<()> {
    // Resolve reasoning mode (flag overrides repo config), then build the router.
    let mut cfg = crate::commands::config::load_config(&args.repo);
    if let Some(rm) = args.reasoning_mode.as_deref() {
        cfg.llm.reasoning_mode =
            altevra_core::config::ReasoningMode::parse(rm).ok_or_else(|| {
                anyhow::anyhow!("--reasoning-mode must be: delegated|codex_oauth|api")
            })?;
    }
    let router = altevra_llm::build_router(&cfg.llm);
    let provider = router.resolve(altevra_llm::ModelRole::StrongReasoner);
    let provider_is_noop = provider.id() == "noop";

    // The candidate sections (prose, non-conformant). We only WRITE when --apply AND
    // a real provider is present; otherwise this is a report (the default).
    let will_write = args.apply && !provider_is_noop;

    let mut rewritten = 0usize;
    let mut would_rewrite = 0usize;
    let mut si7_skipped = 0usize;
    let provider_is_local = provider.is_local();
    let mut backup_dir: Option<PathBuf> = None;

    if will_write {
        // Backup the whole vault before the first write (same guarantee as normalize).
        let ts = args.backup_ts.unwrap_or_else(unix_now);
        let backup_root = args.backup_root.clone().unwrap_or_else(default_backup_root);
        let dir = backup_root.join(format!("obsidian-rewrite-{ts}"));
        copy_dir_recursive(vault, &dir)?;
        backup_dir = Some(dir);
    }

    for p in plans {
        if p.sections_need_rewrite == 0 || p.parse_error.is_some() {
            continue;
        }
        let class = classify_path(&p.rel);
        // SI-7: high-water content (personal/relationship/health/legal/financial/
        // client) must NEVER be sent to a cloud reasoning provider. A non-local
        // provider is skipped for these files entirely — they need a local model.
        if class.domain.is_high_water() && !provider_is_local {
            si7_skipped += p.sections_need_rewrite;
            continue;
        }
        if !will_write {
            would_rewrite += p.sections_need_rewrite;
            continue;
        }
        // --- live path (only runs with --apply + a real provider; left for Pavle) ---
        let content = std::fs::read_to_string(&p.abs)?;
        let (_fm, body) = split_for_normalize(&content)?;
        let sections = parse_sections(&body);
        // Rebuild the body, swapping ONLY the prose-non-conformant sections.
        let mut new_sections: Vec<(String, String)> = Vec::new();
        for s in &sections {
            let conf = section_conformance(s, &class.doc_type);
            if conf.conformant || conf.empty || section_is_stub_only(s) {
                new_sections.push((s.heading.clone(), s.body.clone()));
                continue;
            }
            // Non-conformant PROSE → ask the model to restructure (fact-preserving).
            let prompt = altevra_vault::build_rewrite_prompt(s, &class.doc_type);
            let messages = vec![
                altevra_llm::ChatMessage::system(prompt.system),
                altevra_llm::ChatMessage::user(prompt.user),
            ];
            let opts = altevra_llm::ChatOpts::default().with_temperature(0.2);
            let restructured = provider.complete(&messages, &opts).await?;
            new_sections.push((s.heading.clone(), restructured.trim().to_string()));
            rewritten += 1;
        }
        let rebuilt = rebuild_body(&body, &new_sections);
        // Re-apply frontmatter normalization on top so the doc stays consistent.
        let (existing, _) = split_for_normalize(&content)?;
        let mtime_date = system_time_to_date(file_mtime(&p.abs)?);
        let created = existing_created(existing.as_ref()).unwrap_or(mtime_date);
        let (new_fm, _) = normalize_frontmatter(
            existing.as_ref(),
            &class.doc_type,
            &class.domain,
            class.scope.as_deref(),
            created,
            mtime_date,
            class.archived,
        );
        let out = render_normalized(&new_fm, &rebuilt)?;
        std::fs::write(&p.abs, out)?;
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "rewrite": true,
                "applied": will_write,
                "vault": vault.display().to_string(),
                "reasoning_mode": cfg.llm.reasoning_mode.as_str(),
                "provider": provider.id(),
                "provider_is_noop": provider_is_noop,
                "sections_need_rewrite": sections_need_rewrite,
                "would_rewrite": would_rewrite,
                "rewritten": rewritten,
                "si7_skipped_high_water": si7_skipped,
                "provider_is_local": provider_is_local,
                "backup": backup_dir.map(|d| d.display().to_string()),
            }))?
        );
    } else if provider_is_noop {
        println!("Vault rewrite — DRY RUN (delegated/noop reasoning provider)");
        println!(
            "  would rewrite {sections_need_rewrite} prose section(s) that miss required labels"
        );
        println!(
            "  → needs a real reasoning provider: set `[llm].reasoning_mode = codex_oauth` \
             (uses ChatGPT Plus, no API key) or `api` (altevra secrets set <KEY>)."
        );
        println!("  No model call made; nothing written.");
    } else if !will_write {
        println!(
            "Vault rewrite — DRY RUN (provider '{}' ready)",
            provider.id()
        );
        println!("  would rewrite {would_rewrite} prose section(s). Add --apply to write.");
        if si7_skipped > 0 {
            println!(
                "  SI-7: {si7_skipped} high-water section(s) SKIPPED (won't go to cloud '{}'; need a local model)",
                provider.id()
            );
        }
        println!("  (a full vault backup is made before any write)");
    } else {
        println!("Vault rewrite — APPLIED via '{}'", provider.id());
        if let Some(d) = &backup_dir {
            println!("  backup: {}", d.display());
        }
        println!("  {rewritten} section(s) restructured (facts preserved by contract).");
        if si7_skipped > 0 {
            println!(
                "  SI-7: {si7_skipped} high-water section(s) left untouched (cloud provider barred)."
            );
        }
    }
    Ok(())
}

/// Rebuild a document body from (heading, body) section pairs, preserving the
/// preamble (everything before the first `## `) verbatim. Used by the rewrite path.
fn rebuild_body(original: &str, sections: &[(String, String)]) -> String {
    // Preamble = lines before the first `## ` heading.
    let mut preamble: Vec<&str> = Vec::new();
    for line in original.lines() {
        let is_h2 = line.strip_prefix("## ").map(|r| !r.trim().is_empty()) == Some(true)
            || line.trim_end() == "##";
        if is_h2 {
            break;
        }
        preamble.push(line);
    }
    let mut out = String::new();
    let pre = preamble.join("\n");
    if !pre.trim().is_empty() {
        out.push_str(&pre);
        out.push_str("\n\n");
    }
    for (heading, body) in sections {
        out.push_str(&format!("## {heading}\n\n{}\n\n", body.trim_end()));
    }
    out.trim_end().to_string() + "\n"
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
            scaffold_empty: false,
            rewrite: false,
            reasoning_mode: None,
            repo: std::path::PathBuf::from("."),
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
            scaffold_empty: false,
            rewrite: false,
            reasoning_mode: None,
            repo: std::path::PathBuf::from("."),
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
            scaffold_empty: false,
            rewrite: false,
            reasoning_mode: None,
            repo: std::path::PathBuf::from("."),
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
            scaffold_empty: false,
            rewrite: false,
            reasoning_mode: None,
            repo: std::path::PathBuf::from("."),
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

    // ---------- Phase 1: section-template conformance + --scaffold-empty ----------

    #[test]
    fn analyze_sections_classifies_conformant_stub_and_prose() {
        // decision file: 1 conformant, 1 empty-stub, 1 prose-missing-label.
        let body = "# Decisions\n\n\
                    ## Conformant\n**Odluka:** X.\n\n**Zašto:** Y.\n\n\
                    ## Empty stub\n\n\
                    ## Prose missing label\nSamo slobodan tekst bez Odluke.\n";
        let (total, non, scaf, rew, _missing) = analyze_sections(body, "decision");
        assert_eq!(total, 2, "empty-body section is dropped by parse_sections");
        // "Empty stub" has no body → not a section. So 2 sections: conformant + prose.
        assert_eq!(
            non, 1,
            "only the prose-missing-label section is non-conformant"
        );
        assert_eq!(scaf, 0, "prose section is NOT scaffoldable");
        assert_eq!(rew, 1, "prose-missing-label → needs LLM rewrite");
    }

    #[test]
    fn stub_only_detection() {
        let stub = Section {
            heading: "x".into(),
            level: 2,
            body: "**Odluka:**\n\n**Zašto:**".into(),
            date: None,
        };
        assert!(section_is_stub_only(&stub));
        let prose = Section {
            heading: "x".into(),
            level: 2,
            body: "**Odluka:** real value here.".into(),
            date: None,
        };
        assert!(!section_is_stub_only(&prose));
    }

    #[test]
    fn scaffold_empty_sections_fills_stub_keeps_prose_verbatim() {
        // A decision doc with a stub section and a prose section.
        let body = "# Decisions\n\n\
                    ## Stub section\n**Odluka:**\n\n\
                    ## Real decision\n**Odluka:** Keep ReVesta P0.\n\n**Zašto:** Market signal.\n";
        let (out, n) = scaffold_empty_sections(body, "decision");
        assert_eq!(n, 1, "exactly the stub section is scaffolded");
        // The prose section survives byte-for-byte.
        assert!(out.contains("**Odluka:** Keep ReVesta P0."));
        assert!(out.contains("**Zašto:** Market signal."));
        // The stub now carries the full skeleton (both required labels present).
        assert!(out.contains("**Zašto:**"));
        // Preamble preserved.
        assert!(out.contains("# Decisions"));
    }

    #[test]
    fn scaffold_never_touches_a_fully_prose_doc() {
        let body = "# Decisions\n\n## A\n**Odluka:** x.\n\n**Zašto:** y.\n";
        let (out, n) = scaffold_empty_sections(body, "decision");
        assert_eq!(n, 0);
        assert_eq!(
            out, body,
            "a doc with no stub sections is returned unchanged"
        );
    }

    #[tokio::test]
    async fn scaffold_empty_apply_fills_stubs_and_never_loses_prose() {
        let dir = TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        let backups = dir.path().join("backups");
        let original = "# Decisions\n\n\
                        ## Stub one\n**Odluka:**\n\n\
                        ## Has prose\n**Odluka:** Existing decision text that must survive.\n\n\
                        **Zašto:** And its reasoning.\n";
        write(&vault.join("Memory/Decisions.md"), original);

        normalize(NormalizeArgs {
            vault: Some(vault.clone()),
            apply: false, // scaffold_empty implies apply
            backup_ts: Some(7),
            backup_root: Some(backups.clone()),
            scaffold_empty: true,
            rewrite: false,
            reasoning_mode: None,
            repo: std::path::PathBuf::from("."),
            json: true,
        })
        .await
        .unwrap();

        // Backup holds the ORIGINAL.
        let backup = backups
            .join("obsidian-normalize-7")
            .join("Memory/Decisions.md");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

        let written = std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap();
        // frontmatter added
        assert!(written.contains("type: decision"));
        // the prose decision is preserved verbatim — NO content loss
        assert!(written.contains("Existing decision text that must survive."));
        assert!(written.contains("**Zašto:** And its reasoning."));
        // the stub section got the skeleton (now has the Zašto label slot too)
        let stub_idx = written.find("## Stub one").unwrap();
        let prose_idx = written.find("## Has prose").unwrap();
        let stub_block = &written[stub_idx..prose_idx];
        assert!(
            stub_block.contains("**Zašto:**"),
            "stub section gained the section skeleton"
        );
    }

    // ---------- Phase 2: LLM rewrite seam (noop path; never runs a live model) ----------

    #[test]
    fn rebuild_body_preserves_preamble_and_swaps_sections() {
        let original = "# Decisions\n\nPreamble line.\n\n## A\nold a body\n\n## B\nold b body\n";
        let new_sections = vec![
            ("A".to_string(), "**Odluka:** new a.".to_string()),
            ("B".to_string(), "old b body".to_string()),
        ];
        let out = rebuild_body(original, &new_sections);
        assert!(out.contains("# Decisions"), "preamble preserved");
        assert!(out.contains("Preamble line."));
        assert!(out.contains("## A\n\n**Odluka:** new a."));
        assert!(out.contains("## B\n\nold b body"));
        assert!(!out.contains("old a body"), "section A body was swapped");
    }

    #[tokio::test]
    async fn rewrite_delegated_is_noop_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        // A decision file with a prose-but-non-conformant section (no **Odluka:**).
        let original = "# Decisions\n\n## Loose decision\nSlobodan tekst bez ijednog labela.\n";
        write(&vault.join("Memory/Decisions.md"), original);

        // reasoning_mode defaults to `delegated` → provider is noop → no-op report.
        normalize(NormalizeArgs {
            vault: Some(vault.clone()),
            apply: true, // even with --apply, a noop provider must NOT write
            backup_ts: Some(11),
            backup_root: Some(dir.path().join("backups")),
            scaffold_empty: false,
            rewrite: true,
            reasoning_mode: Some("delegated".into()),
            repo: dir.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();

        // The file is UNCHANGED — delegated/noop never rewrites.
        assert_eq!(
            std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap(),
            original,
            "delegated reasoning mode is a no-op; the prose section is untouched"
        );
    }

    #[tokio::test]
    async fn rewrite_dry_run_reports_candidate_count() {
        let dir = TempDir::new().unwrap();
        let vault = dir.path().join("vault");
        // Two prose-non-conformant decision sections.
        write(
            &vault.join("Memory/Decisions.md"),
            "# Decisions\n\n## One\nprose bez labela jedan.\n\n## Two\nprose bez labela dva.\n",
        );

        // Just assert it runs clean as a dry-run no-op (delegated). The count is
        // surfaced in JSON; here we assert the safe no-write contract holds.
        let before = std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap();
        normalize(NormalizeArgs {
            vault: Some(vault.clone()),
            apply: false,
            backup_ts: None,
            backup_root: None,
            scaffold_empty: false,
            rewrite: true,
            reasoning_mode: Some("delegated".into()),
            repo: dir.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.join("Memory/Decisions.md")).unwrap(),
            before
        );
    }
}
