use std::path::{Path, PathBuf};

/// Save synthesized research to a vault file under `05-research/`.
/// Returns the absolute path written.
pub fn save_research(vault_root: &Path, slug: &str, content: &str) -> anyhow::Result<PathBuf> {
    let safe_slug = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase();
    let dir = vault_root.join("05-research");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{safe_slug}.md"));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_writes_file_under_research_dir() {
        let tmp = TempDir::new().unwrap();
        let path = save_research(tmp.path(), "test topic", "# body").unwrap();
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("05-research"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# body");
    }

    #[test]
    fn save_sanitizes_slug() {
        let tmp = TempDir::new().unwrap();
        let path = save_research(tmp.path(), "Hello, World!", "x").unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'));
    }
}
