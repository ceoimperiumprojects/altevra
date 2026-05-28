//! Vault directory scanner.
//!
//! Walks an Obsidian-style vault root and surfaces the canonical section
//! directories (`00-inbox`, `01-projects`, ...) plus all markdown files
//! beneath them.  Hidden / build / vendor directories are skipped.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

/// Errors emitted while scanning a vault.
#[derive(Debug, Error)]
pub enum ScannerError {
    #[error("vault root does not exist: {0}")]
    MissingRoot(PathBuf),

    #[error("vault root is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("failed to read entry: {0}")]
    Walk(#[from] walkdir::Error),

    #[error("failed to stat {path}: {source}")]
    Stat {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A canonical Altevra vault section directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultSection {
    /// Directory name on disk, e.g. `"06-skills"`.
    pub slug: String,
    /// Section name without the numeric prefix, e.g. `"skills"`.
    pub name: String,
    /// Absolute path on disk.
    pub path: PathBuf,
}

/// A markdown file found inside the vault.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    /// Slug of the top-level section the file lives under, e.g. `"06-skills"`.
    /// `None` for files that live directly in the vault root or under an
    /// unrecognized top-level directory.
    pub section: Option<String>,
    pub size_bytes: u64,
    pub modified: DateTime<Utc>,
}

/// Directories that are always skipped during a vault scan.
const SKIP_DIRS: &[&str] = &[".altevra", ".git", "target", "node_modules", ".obsidian"];

/// Recursively scan a vault root, returning every `*.md` file found.
pub fn scan_vault(root: &Path) -> anyhow::Result<Vec<ScannedFile>> {
    validate_root(root)?;

    let mut out = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_skip(e));

    for entry in walker {
        let entry = entry.map_err(ScannerError::Walk)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let metadata = entry.metadata().map_err(ScannerError::Walk)?;
        let modified_system = metadata.modified().map_err(|e| ScannerError::Stat {
            path: path.to_path_buf(),
            source: e,
        })?;
        let modified: DateTime<Utc> = modified_system.into();

        out.push(ScannedFile {
            path: path.to_path_buf(),
            section: detect_section(root, path),
            size_bytes: metadata.len(),
            modified,
        });
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// List the canonical top-level section directories under the vault root.
///
/// A section is any directory whose name matches `NN-name` (two digits, a
/// hyphen, then a slug).  Returned in lexicographic / numeric order.
pub fn list_sections(root: &Path) -> anyhow::Result<Vec<VaultSection>> {
    validate_root(root)?;

    let mut sections = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| ScannerError::Stat {
        path: root.to_path_buf(),
        source: e,
    })? {
        let entry = entry.map_err(|e| ScannerError::Stat {
            path: root.to_path_buf(),
            source: e,
        })?;
        let file_type = entry.file_type().map_err(|e| ScannerError::Stat {
            path: entry.path(),
            source: e,
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if let Some(section_name) = parse_section_slug(&name) {
            sections.push(VaultSection {
                slug: name.clone(),
                name: section_name,
                path: entry.path(),
            });
        }
    }

    sections.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(sections)
}

fn validate_root(root: &Path) -> Result<(), ScannerError> {
    if !root.exists() {
        return Err(ScannerError::MissingRoot(root.to_path_buf()));
    }
    if !root.is_dir() {
        return Err(ScannerError::NotADirectory(root.to_path_buf()));
    }
    Ok(())
}

fn should_skip(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    if SKIP_DIRS.iter().any(|s| *s == name) {
        return true;
    }
    // Skip dotfile-style hidden directories besides the explicit list above.
    if entry.file_type().is_dir() && name.starts_with('.') {
        return true;
    }
    false
}

/// Determine which top-level section a file belongs to, if any.
fn detect_section(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut components = rel.components();
    let first = components.next()?;
    let name = first.as_os_str().to_str()?;

    // Must have at least one more component (i.e. file is inside the section,
    // not the section directory itself which can't happen for a file anyway).
    components.next()?;
    if parse_section_slug(name).is_some() {
        Some(name.to_string())
    } else {
        None
    }
}

/// If `name` looks like `NN-slug`, return the slug portion (without prefix).
fn parse_section_slug(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    if !(bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit() && bytes[2] == b'-') {
        return None;
    }
    let slug = &name[3..];
    if slug.is_empty() {
        return None;
    }
    Some(slug.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_vault() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        for section in [
            "00-inbox",
            "01-projects",
            "02-areas",
            "06-skills",
            "08-decisions",
        ] {
            fs::create_dir_all(root.join(section)).unwrap();
        }
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        fs::create_dir_all(root.join("not-a-section")).unwrap();
        fs::create_dir_all(root.join("01-projects/altevra")).unwrap();
        fs::write(root.join("06-skills/skill.md"), "# Skill\n").unwrap();
        fs::write(root.join("01-projects/altevra/README.md"), "# Altevra\n").unwrap();
        fs::write(root.join("README.md"), "# root\n").unwrap();
        fs::write(root.join("not-a-section/loose.md"), "# loose\n").unwrap();
        fs::write(root.join("06-skills/notes.txt"), "ignored").unwrap();
        fs::write(root.join("node_modules/foo/junk.md"), "ignored\n").unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        // deeply nested
        fs::create_dir_all(root.join("06-skills/deep/a/b/c")).unwrap();
        fs::write(root.join("06-skills/deep/a/b/c/leaf.md"), "# leaf\n").unwrap();
        dir
    }

    #[test]
    fn lists_canonical_sections_only() {
        let dir = make_vault();
        let sections = list_sections(dir.path()).unwrap();
        let slugs: Vec<_> = sections.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(
            slugs,
            vec![
                "00-inbox",
                "01-projects",
                "02-areas",
                "06-skills",
                "08-decisions"
            ]
        );
        assert_eq!(sections[3].name, "skills");
    }

    #[test]
    fn scan_returns_only_markdown_files() {
        let dir = make_vault();
        let files = scan_vault(dir.path()).unwrap();
        // Should include leaf.md, skill.md, README.md (root), README.md (project), loose.md
        let names: Vec<_> = files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "skill.md"));
        assert!(names.iter().any(|n| n == "leaf.md"));
        assert!(names.iter().any(|n| n == "loose.md"));
        assert!(names.iter().all(|n| !n.ends_with(".txt")));
    }

    #[test]
    fn scan_skips_excluded_dirs() {
        let dir = make_vault();
        let files = scan_vault(dir.path()).unwrap();
        for f in &files {
            let p = f.path.to_string_lossy();
            assert!(!p.contains("node_modules"), "found in node_modules: {p}");
            assert!(!p.contains("/.git/"), "found in .git: {p}");
        }
    }

    #[test]
    fn assigns_section_when_under_known_dir() {
        let dir = make_vault();
        let files = scan_vault(dir.path()).unwrap();
        let skill = files
            .iter()
            .find(|f| f.path.ends_with("06-skills/skill.md"))
            .expect("skill present");
        assert_eq!(skill.section.as_deref(), Some("06-skills"));

        let leaf = files
            .iter()
            .find(|f| f.path.ends_with("leaf.md"))
            .expect("leaf present");
        assert_eq!(leaf.section.as_deref(), Some("06-skills"));
    }

    #[test]
    fn files_outside_sections_have_no_section() {
        let dir = make_vault();
        let files = scan_vault(dir.path()).unwrap();
        let root_readme = files
            .iter()
            .find(|f| f.path.parent() == Some(dir.path()))
            .expect("root README.md");
        assert!(root_readme.section.is_none());

        let loose = files
            .iter()
            .find(|f| f.path.ends_with("not-a-section/loose.md"))
            .unwrap();
        assert!(loose.section.is_none());
    }

    #[test]
    fn missing_root_errors() {
        let p = PathBuf::from("/nonexistent/altevra/vault");
        let err = scan_vault(&p).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn root_must_be_directory() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("notavault.md");
        fs::write(&file_path, "x").unwrap();
        let err = scan_vault(&file_path).unwrap_err();
        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn empty_vault_scans_clean() {
        let dir = TempDir::new().unwrap();
        let files = scan_vault(dir.path()).unwrap();
        assert!(files.is_empty());
        let sections = list_sections(dir.path()).unwrap();
        assert!(sections.is_empty());
    }

    #[test]
    fn deeply_nested_files_are_found() {
        let dir = make_vault();
        let files = scan_vault(dir.path()).unwrap();
        assert!(
            files
                .iter()
                .any(|f| f.path.ends_with("06-skills/deep/a/b/c/leaf.md")),
            "deeply nested leaf.md should be discovered"
        );
    }
}
