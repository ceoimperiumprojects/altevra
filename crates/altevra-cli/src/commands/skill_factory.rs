//! Skill factory CLI (PLAN-ALIVE §P3a) — deterministic edit-engine plumbing.
//!
//! `altevra skill-factory edits-preview` reads a skill file + an edits JSON
//! file, runs the pure `apply_edits` engine (budget + protected slow-update
//! regions + per-edit skip reasons) and prints the outcome WITHOUT writing
//! anything to disk. This is the preview surface P3b's renderer builds on —
//! zero LLM, zero side effects.

use altevra_skills::parser::parse_skill;
use altevra_skills::skill_edits::{apply_edits, fingerprint_edits, SkillEdit, DEFAULT_EDIT_BUDGET};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum SkillFactoryCommands {
    /// Preview deterministic skill edits (P3a engine). NEVER writes — prints
    /// applied/skipped edits, the edit-set fingerprint, and a diff preview.
    EditsPreview(EditsPreviewArgs),
}

#[derive(Args)]
pub struct EditsPreviewArgs {
    /// Path to the skill markdown file (SKILL.md). With YAML frontmatter the
    /// edits run over the BODY only; frontmatter is never edited.
    #[arg(long)]
    pub skill: PathBuf,
    /// Path to a JSON file holding the edit array, e.g.
    /// [{"op":"replace","from":"old","to":"new"}].
    #[arg(long)]
    pub edits: PathBuf,
    /// Edit budget — the "textual learning rate" (max edits applied).
    #[arg(long, default_value_t = DEFAULT_EDIT_BUDGET)]
    pub budget: usize,
    /// Emit the full EditOutcome as JSON instead of the human preview.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: SkillFactoryCommands) -> anyhow::Result<()> {
    match cmd {
        SkillFactoryCommands::EditsPreview(args) => run_edits_preview(args),
    }
}

fn run_edits_preview(args: EditsPreviewArgs) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(&args.skill)
        .map_err(|e| anyhow::anyhow!("read skill {}: {e}", args.skill.display()))?;
    let edits_raw = std::fs::read_to_string(&args.edits)
        .map_err(|e| anyhow::anyhow!("read edits {}: {e}", args.edits.display()))?;
    let edits: Vec<SkillEdit> = serde_json::from_str(&edits_raw)
        .map_err(|e| anyhow::anyhow!("parse edits JSON {}: {e}", args.edits.display()))?;

    // Frontmatter is out of bounds for the optimizer — edit the body only.
    // Files without frontmatter (plain markdown) are edited whole.
    let body = match parse_skill(&raw) {
        Ok(parsed) => parsed.body,
        Err(_) => raw.clone(),
    };

    let fingerprint = fingerprint_edits(&edits);
    let outcome = apply_edits(&body, &edits, args.budget);

    if args.json {
        let doc = serde_json::json!({
            "skill": args.skill.display().to_string(),
            "budget": args.budget,
            "fingerprint": fingerprint,
            "outcome": outcome,
        });
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(());
    }

    println!("Skill:       {}", args.skill.display());
    println!("Budget:      {}", args.budget);
    println!("Fingerprint: {fingerprint}");
    println!(
        "Result:      {} applied, {} skipped, changed={}",
        outcome.applied.len(),
        outcome.skipped.len(),
        outcome.changed
    );
    if !outcome.applied.is_empty() {
        println!("\nApplied:");
        for e in &outcome.applied {
            println!("  + {}", e.summary());
        }
    }
    if !outcome.skipped.is_empty() {
        println!("\nSkipped:");
        for s in &outcome.skipped {
            println!("  - {} ({})", s.edit.summary(), s.reason.as_str());
        }
    }
    if outcome.changed {
        println!("\nDiff preview (body):");
        print!("{}", line_diff(&body, &outcome.edited_body));
    } else {
        println!("\nNo changes — body untouched.");
    }
    println!("\n(preview only — nothing was written)");
    Ok(())
}

/// Minimal line diff: trims the common prefix/suffix and shows the changed
/// middle window as `-`/`+` lines. Deterministic, no external crates.
fn line_diff(before: &str, after: &str) -> String {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let mut start = 0usize;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end_a = a.len();
    let mut end_b = b.len();
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let mut out = String::new();
    out.push_str(&format!("@@ line {} @@\n", start + 1));
    for line in &a[start..end_a] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &b[start..end_b] {
        out.push_str(&format!("+ {line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_diff_shows_only_changed_window() {
        let before = "a\nb\nc\nd\n";
        let after = "a\nB!\nc\nd\n";
        let d = line_diff(before, after);
        assert!(d.contains("- b"));
        assert!(d.contains("+ B!"));
        assert!(!d.contains("- a"));
        assert!(!d.contains("- c"));
        assert!(d.contains("@@ line 2 @@"));
    }
}
