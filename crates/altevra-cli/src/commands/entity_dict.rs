//! Build the known-entity dictionary from the vault + identity registry.
//!
//! Sources (all read-only):
//!   * `<vault>/Memory/People.md` — `## <Name>` headings → person entities
//!     (parsed by `altevra_core::EntityDictionary::add_people_from_md`).
//!   * `~/.imperium/identity/projects.yaml` — `id` + `name` + `aliases` → project
//!     entities (the canonical project registry).
//!   * `<vault>/Projects/<P>/` directory names → project entities (fallback when
//!     a project isn't in the registry yet).
//!   * A small built-in mentor seed (Đorđe / Srđan / Saša) — they appear in body
//!     text (Decisions) but not as People.md headings, and the cross-link
//!     "what did I do with Đorđe" is Pavle's headline use case.
//!
//! Pure entity logic lives in `altevra_core::entity`; this module only does the
//! file reads + yaml parsing (serde_yaml is a CLI dep, not a core dep).

use altevra_core::EntityDictionary;
use std::path::{Path, PathBuf};

/// Resolve the vault root from a file path: walk up to the dir that contains a
/// `Memory/` subdir (the Imperium vault root), else the file's parent's parent.
fn vault_root_for(file: &Path) -> Option<PathBuf> {
    let mut cur = file.parent();
    while let Some(dir) = cur {
        if dir.join("Memory").is_dir() || dir.join("Daily").is_dir() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Build the dictionary for a capture of `file`. `vault_override` (e.g. the watch
/// root's parent) wins; else inferred from the file path; else just the registry +
/// mentor seed. Never errors — a missing source is simply skipped.
pub fn build_dictionary(file: &Path, vault_override: Option<&Path>) -> EntityDictionary {
    let mut dict = EntityDictionary::new();

    // 1. People.md headings.
    let vault = vault_override
        .map(|p| p.to_path_buf())
        .or_else(|| vault_root_for(file));
    if let Some(root) = &vault {
        let people_md = root.join("Memory").join("People.md");
        if let Ok(text) = std::fs::read_to_string(&people_md) {
            dict.add_people_from_md(&text);
        }
        // 3. Projects/<P>/ dir names (fallback project source).
        let projects_dir = root.join("Projects");
        if let Ok(entries) = std::fs::read_dir(&projects_dir) {
            for e in entries.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    if let Some(name) = e.file_name().to_str() {
                        if !name.starts_with('.') && !name.starts_with('_') {
                            let id = name.to_lowercase();
                            dict.add_project(&id, name, &[]);
                        }
                    }
                }
            }
        }
    }

    // 2. projects.yaml registry (canonical; wins over dir-name fallback by id dedup).
    load_projects_yaml(&mut dict);

    // 4. mentor seed (body-text-only people central to Pavle's cross-link queries).
    seed_mentors(&mut dict);

    dict
}

/// Parse `~/.imperium/identity/projects.yaml` (`projects: [{id, name, aliases}]`).
fn load_projects_yaml(dict: &mut EntityDictionary) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let path = home.join(".imperium/identity/projects.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return;
    };
    let Some(projects) = doc.get("projects").and_then(|p| p.as_sequence()) else {
        return;
    };
    for proj in projects {
        let id = proj.get("id").and_then(|v| v.as_str());
        let name = proj.get("name").and_then(|v| v.as_str());
        let (Some(id), Some(name)) = (id, name) else {
            continue;
        };
        let aliases: Vec<String> = proj
            .get("aliases")
            .and_then(|a| a.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        dict.add_project(id, name, &aliases);
    }
}

/// Pavle's mentors (CLAUDE.md identity) — referenced in body text, not People.md.
/// This is the headline "what did I do with Đorđe" connection.
fn seed_mentors(dict: &mut EntityDictionary) {
    dict.add_person(
        "djordje",
        "Đorđe Dimitrijević",
        &["Đorđe".into(), "Djordje".into(), "Dimitrijević".into()],
    );
    dict.add_person(
        "srdjan",
        "Srđan Jovanović",
        &["Srđan".into(), "Srdjan".into(), "Jovanović".into()],
    );
    dict.add_person(
        "sasa-popovic",
        "Saša Popović",
        &["Saša".into(), "Sasa".into(), "Popović".into()],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn builds_from_people_md_and_seeds_mentors() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(
            mem.join("People.md"),
            "# People\n\n## Luka — ReVesta landing\n## Danilo\n",
        )
        .unwrap();
        let dict = build_dictionary(&mem.join("Decisions.md"), None);
        // People.md people present...
        assert!(dict.people.iter().any(|p| p.name == "Luka"));
        assert!(dict.people.iter().any(|p| p.name == "Danilo"));
        // ...and the mentor seed (Đorđe) is always there for the headline query.
        assert!(dict.get("person:djordje").is_some());
    }

    #[test]
    fn vault_root_detected_from_memory_sibling() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let root = vault_root_for(&mem.join("Decisions.md"));
        assert_eq!(root.as_deref(), Some(tmp.path()));
    }
}
