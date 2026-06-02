//! External skill importer — scans `~/.claude/skills/`, `~/.codex/skills/`,
//! `~/.hermes/skills/`, `~/.imperium/skills/`, and Altevra's own `06-skills/` for
//! skill files written by other tools and produces a unified `ExternalSkill` view.
//!
//! Foreign formats (Claude, Hermes, Codex) use `name:` in YAML frontmatter, not
//! Altevra's stricter `slug:`/`title:`. This loose importer accepts BOTH shapes — it
//! is INTENTIONALLY separate from the strict `parse_skill` (which keeps the
//! authoring contract for Altevra-owned skills tight). The importer never overwrites
//! anything; it is read-only and feeds the `skills inventory` command (and later
//! `skills sync`).
//!
//! Layout it understands:
//! * `<dir>/<slug>/SKILL.md` (Claude, Hermes, Codex bundle style — most common)
//! * `<dir>/<slug>.md` (Altevra vault flat style — `06-skills/foo.md`)
//!
//! Detects `<!-- ALTEVRA_MANAGED: true -->` so re-scans don't double-count
//! Altevra-generated copies — they're flagged `managed: true` and the source tool
//! reflects the adapter that wrote them.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SourceTool {
    Claude,
    Codex,
    Cursor,
    Hermes,
    Imperium,
    Altevra,
    Other,
}

impl SourceTool {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceTool::Claude => "claude",
            SourceTool::Codex => "codex",
            SourceTool::Cursor => "cursor",
            SourceTool::Hermes => "hermes",
            SourceTool::Imperium => "imperium",
            SourceTool::Altevra => "altevra",
            SourceTool::Other => "other",
        }
    }
}

/// A skill found on disk in some tool's directory. Loose schema by design — works
/// for Claude (`name:`), Hermes (`name:`), and Altevra (`slug:`+`title:`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSkill {
    /// Slug used to identify this skill (from `slug:` if present, else `name:`,
    /// else directory name as fallback).
    pub slug: String,
    pub source_tool: SourceTool,
    pub path: PathBuf,
    pub version: Option<String>,
    pub description: Option<String>,
    /// True if the file carries `<!-- ALTEVRA_MANAGED: true -->` (i.e. Altevra
    /// generated it in this tool's dir; not authored externally).
    pub managed: bool,
    pub body_len: usize,
}

/// Default skill directories per known tool. Returns `(SourceTool, absolute path)`
/// pairs that exist on disk; missing ones are silently skipped.
pub fn default_skill_dirs() -> Vec<(SourceTool, PathBuf)> {
    let home = match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h),
        None => return vec![],
    };
    let candidates = [
        (SourceTool::Claude, home.join(".claude/skills")),
        (SourceTool::Codex, home.join(".codex/skills")),
        (SourceTool::Cursor, home.join(".cursor/skills")),
        (SourceTool::Hermes, home.join(".hermes/skills")),
        (SourceTool::Imperium, home.join(".imperium/skills")),
    ];
    candidates.into_iter().filter(|(_, p)| p.exists()).collect()
}

/// Scan one directory for skills. Recognizes both `<slug>/SKILL.md` and
/// `<slug>.md` layouts; flat subdirectories only (no deep recursion — avoids
/// pulling in shared/ assets twice).
pub fn scan_external_dir(dir: &Path, source_tool: SourceTool) -> Vec<ExternalSkill> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        // Bundle layout: <slug>/SKILL.md
        if p.is_dir() {
            let skill_md = p.join("SKILL.md");
            if skill_md.exists() {
                if let Some(s) = parse_one(&skill_md, source_tool.clone()) {
                    out.push(s);
                }
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some("md") {
            // Flat layout: <slug>.md
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                // Avoid reading top-level README.md / similar.
                if stem.eq_ignore_ascii_case("readme") {
                    continue;
                }
                if let Some(s) = parse_one(&p, source_tool.clone()) {
                    out.push(s);
                }
            }
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// Walk all known skill dirs and return a flat list.
pub fn scan_all() -> Vec<ExternalSkill> {
    default_skill_dirs()
        .into_iter()
        .flat_map(|(tool, dir)| scan_external_dir(&dir, tool))
        .collect()
}

/// Group an inventory by slug → which tools have it (for sync planning).
pub fn group_by_slug(skills: &[ExternalSkill]) -> HashMap<String, Vec<&ExternalSkill>> {
    let mut by: HashMap<String, Vec<&ExternalSkill>> = HashMap::new();
    for s in skills {
        by.entry(s.slug.clone()).or_default().push(s);
    }
    by
}

fn parse_one(path: &Path, source_tool: SourceTool) -> Option<ExternalSkill> {
    let content = std::fs::read_to_string(path).ok()?;
    let managed = content.contains("ALTEVRA_MANAGED");
    let (frontmatter, body) = split_frontmatter(&content);

    // Loose YAML parse: tolerate any shape, look for slug/name/version/description.
    let mut slug: Option<String> = None;
    let mut version: Option<String> = None;
    let mut description: Option<String> = None;
    if let Some(yaml) = frontmatter {
        if let Ok(v) = serde_yaml::from_str::<Value>(yaml) {
            slug = v.get("slug").and_then(|s| s.as_str()).map(str::to_string);
            if slug.is_none() {
                slug = v.get("name").and_then(|s| s.as_str()).map(str::to_string);
            }
            version = v
                .get("version")
                .and_then(|s| s.as_str())
                .map(str::to_string);
            description = v
                .get("description")
                .and_then(|s| s.as_str())
                .map(str::to_string);
        }
    }
    // Fallback: bundle dir name (`audit/SKILL.md` → "audit") or file stem.
    let slug = slug.unwrap_or_else(|| {
        if path.file_name().and_then(|f| f.to_str()) == Some("SKILL.md") {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string()
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string()
        }
    });

    Some(ExternalSkill {
        slug,
        source_tool,
        path: path.to_path_buf(),
        version,
        description,
        managed,
        body_len: body.len(),
    })
}

/// Split a markdown string into `(Some(frontmatter_yaml), body)` or `(None, full)`.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let rest = match content.strip_prefix("---\n") {
        Some(r) => r,
        None => return (None, content),
    };
    match rest.find("\n---\n") {
        Some(end) => {
            let yaml = &rest[..end];
            let body = &rest[end + 5..];
            (Some(yaml), body)
        }
        None => (None, content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn split_frontmatter_handles_present_and_absent() {
        let (y, b) = split_frontmatter("---\nname: x\n---\nbody\n");
        assert_eq!(y, Some("name: x"));
        assert!(b.starts_with("body"));
        let (y2, b2) = split_frontmatter("no frontmatter");
        assert!(y2.is_none());
        assert_eq!(b2, "no frontmatter");
    }

    #[test]
    fn scan_finds_claude_style_bundle_and_altevra_flat() {
        // Claude / Hermes layout: <slug>/SKILL.md with `name:`
        // Altevra layout: <slug>.md with `slug:` / `title:`
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        write(
            dir,
            "audit/SKILL.md",
            "---\nname: audit\nversion: 2.1.1\ndescription: Tech checks\n---\n# Audit\nbody\n",
        );
        write(
            dir,
            "altevra-core.md",
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\nbody\n",
        );
        // README.md must NOT be treated as a skill.
        write(dir, "README.md", "# do not import\n");
        // Managed file (Altevra-generated) is flagged.
        write(
            dir,
            "rendered/SKILL.md",
            "<!-- ALTEVRA_MANAGED: true -->\n---\nname: rendered\nversion: 1.0.0\n---\nbody\n",
        );

        let skills = scan_external_dir(dir, SourceTool::Claude);
        let slugs: Vec<&str> = skills.iter().map(|s| s.slug.as_str()).collect();
        assert!(slugs.contains(&"audit"), "Claude bundle parsed by name");
        assert!(
            slugs.contains(&"altevra-core"),
            "Altevra flat parsed by slug"
        );
        assert!(slugs.contains(&"rendered"));
        assert!(!slugs.contains(&"README"), "README is filtered out");

        let rendered = skills.iter().find(|s| s.slug == "rendered").unwrap();
        assert!(rendered.managed, "ALTEVRA_MANAGED marker detected");
        let audit = skills.iter().find(|s| s.slug == "audit").unwrap();
        assert_eq!(audit.version.as_deref(), Some("2.1.1"));
        assert_eq!(audit.description.as_deref(), Some("Tech checks"));
        assert!(!audit.managed);
        assert_eq!(audit.source_tool, SourceTool::Claude);
    }

    #[test]
    fn group_by_slug_aggregates_across_tools() {
        let s = vec![
            ExternalSkill {
                slug: "audit".into(),
                source_tool: SourceTool::Claude,
                path: PathBuf::from("/c/audit/SKILL.md"),
                version: Some("2.1.1".into()),
                description: None,
                managed: false,
                body_len: 100,
            },
            ExternalSkill {
                slug: "audit".into(),
                source_tool: SourceTool::Hermes,
                path: PathBuf::from("/h/audit/SKILL.md"),
                version: Some("2.0.0".into()),
                description: None,
                managed: false,
                body_len: 80,
            },
            ExternalSkill {
                slug: "unique".into(),
                source_tool: SourceTool::Codex,
                path: PathBuf::from("/c/unique/SKILL.md"),
                version: None,
                description: None,
                managed: false,
                body_len: 50,
            },
        ];
        let g = group_by_slug(&s);
        assert_eq!(g["audit"].len(), 2, "audit is in both Claude and Hermes");
        assert_eq!(g["unique"].len(), 1);
    }

    #[test]
    fn scan_missing_dir_returns_empty() {
        let skills =
            scan_external_dir(Path::new("/nonexistent/altevra/skills"), SourceTool::Claude);
        assert!(skills.is_empty());
    }
}
