//! Atomic file writes for vault documents.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::frontmatter::{serialize_frontmatter, Frontmatter};

/// Errors that can occur while writing to the vault.
#[derive(Debug, Error)]
pub enum WriterError {
    #[error("failed to create parent directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to rename {from} -> {to}: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Atomically write `content` to `path`.
///
/// The function writes to a sibling `<path>.tmp` file first, fsyncs it, then
/// renames over the target.  Parent directories are created if missing.
pub fn write_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| WriterError::CreateDir {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }

    let tmp_path = tmp_path_for(path);

    {
        let mut f = fs::File::create(&tmp_path).map_err(|e| WriterError::Write {
            path: tmp_path.clone(),
            source: e,
        })?;
        f.write_all(content.as_bytes())
            .map_err(|e| WriterError::Write {
                path: tmp_path.clone(),
                source: e,
            })?;
        f.sync_all().map_err(|e| WriterError::Write {
            path: tmp_path.clone(),
            source: e,
        })?;
    }

    fs::rename(&tmp_path, path).map_err(|e| WriterError::Rename {
        from: tmp_path.clone(),
        to: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}

/// Write a markdown document with optional frontmatter using an atomic rename.
///
/// If `fm` is Some, the file is prefixed with the standard
/// `---\n<yaml>\n---\n` block.  The body is then appended verbatim.
pub fn write_document(path: &Path, fm: Option<&Frontmatter>, body: &str) -> anyhow::Result<()> {
    let mut out = String::new();
    if let Some(fm) = fm {
        out.push_str(&serialize_frontmatter(fm)?);
    }
    out.push_str(body);
    write_atomic(path, &out)
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(name),
        _ => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::parse_frontmatter;
    use tempfile::TempDir;

    #[test]
    fn writes_atomic_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("hello.md");
        write_atomic(&p, "hi there").unwrap();
        let read = fs::read_to_string(&p).unwrap();
        assert_eq!(read, "hi there");
    }

    #[test]
    fn creates_missing_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nested/deeper/file.md");
        write_atomic(&p, "x").unwrap();
        assert!(p.exists());
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.md");
        write_atomic(&p, "first").unwrap();
        write_atomic(&p, "second").unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert_eq!(s, "second");
    }

    #[test]
    fn writes_document_with_frontmatter() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("doc.md");
        let mut m = serde_yaml::Mapping::new();
        m.insert(
            serde_yaml::Value::String("title".into()),
            serde_yaml::Value::String("Hi".into()),
        );
        let fm = Frontmatter::new(serde_yaml::Value::Mapping(m));
        write_document(&p, Some(&fm), "# Body\n").unwrap();

        let full = fs::read_to_string(&p).unwrap();
        assert!(full.starts_with("---\n"));
        let (parsed_fm, body) = parse_frontmatter(&full).unwrap();
        assert_eq!(parsed_fm.unwrap().get_str("title"), Some("Hi"));
        assert_eq!(body, "# Body\n");
    }

    #[test]
    fn writes_document_without_frontmatter() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nofm.md");
        write_document(&p, None, "just body").unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert_eq!(s, "just body");
    }

    #[test]
    fn temp_file_is_cleaned_up_after_rename() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("rename.md");
        write_atomic(&p, "ok").unwrap();
        let tmp = tmp_path_for(&p);
        assert!(!tmp.exists(), "tmp file should not linger after rename");
    }
}
