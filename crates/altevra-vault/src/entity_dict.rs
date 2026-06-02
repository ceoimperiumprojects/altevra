//! Build the known-entity dictionary from the vault + identity registry.
//!
//! Lives in `altevra-vault` (not the CLI) so BOTH the CLI capture path AND the
//! MCP `recall_about` tool can load the same dictionary — the entity graph is
//! universal, reachable by every AI tool, not just the terminal.
//!
//! Sources (all read-only):
//!   * `<vault>/Memory/People.md` — `## <Name>` headings → person entities
//!     (parsed by `altevra_core::EntityDictionary::add_people_from_md`).
//!   * `~/.imperium/identity/projects.yaml` — `id` + `name` + `aliases` → projects
//!     (the canonical project registry).
//!   * `<vault>/Projects/<P>/` dir names → project entities (fallback).
//!   * A built-in mentor seed (Đorđe / Srđan / Saša) — they live in body text
//!     (Decisions), not as People.md headings, and "what did I do with Đorđe" is
//!     the headline cross-link.
//!
//! Pure entity logic (types, `detect_mentions`, inflection) stays in
//! `altevra_core::entity`; this module only does the file reads + yaml parsing
//! (serde_yaml is already a vault dep).

use altevra_core::EntityDictionary;
use std::path::{Path, PathBuf};

/// Resolve the vault root from a file path: walk up to the dir that contains a
/// `Memory/` or `Daily/` subdir (the Imperium vault root).
pub fn vault_root_for(file: &Path) -> Option<PathBuf> {
    let mut cur = file.parent();
    while let Some(dir) = cur {
        if dir.join("Memory").is_dir() || dir.join("Daily").is_dir() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Build the dictionary for a capture/query rooted at `file`. `vault_override`
/// (e.g. an explicit `--vault`) wins; else inferred from the file path; else just
/// the registry + mentor seed. Never errors — a missing source is skipped.
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

    // 2. projects.yaml registry (canonical; wins over dir-name fallback by dedup).
    load_projects_yaml(&mut dict);

    // 4. mentor seed (body-text-only people central to the cross-link queries).
    seed_mentors(&mut dict);

    dict
}

/// Build a dictionary directly from a vault root (no probe file). Convenience for
/// the MCP tool, which has a `vault_path` already.
pub fn build_dictionary_for_vault(vault: &Path) -> EntityDictionary {
    build_dictionary(&vault.join("Memory").join("People.md"), Some(vault))
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

/// Resolve a free-text name to a dictionary entity: exact-id, then any alias match
/// (ascii-folded, case-insensitive), longest-name first so a full name wins. Shared
/// by the CLI `recall --with` and the MCP `recall_about` tool so both resolve a
/// name identically (diacritic/case-insensitive; `Đorđe`/`Djordje` → same entity).
pub fn resolve_entity<'a>(
    dict: &'a EntityDictionary,
    name: &str,
) -> Option<&'a altevra_core::Entity> {
    if let Some(e) = dict.get(name) {
        return Some(e);
    }
    let want = altevra_core::ascii_fold(name).to_lowercase();
    let mut best: Option<&altevra_core::Entity> = None;
    for e in dict.all() {
        let hit = e
            .aliases
            .iter()
            .any(|a| altevra_core::ascii_fold(a).to_lowercase() == want)
            || altevra_core::ascii_fold(&e.name).to_lowercase() == want;
        if hit && best.map(|b| e.name.len() > b.name.len()).unwrap_or(true) {
            best = Some(e);
        }
    }
    best
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
        assert!(dict.people.iter().any(|p| p.name == "Luka"));
        assert!(dict.people.iter().any(|p| p.name == "Danilo"));
        assert!(dict.get("person:djordje").is_some());
    }

    #[test]
    fn build_for_vault_root_finds_people() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join("People.md"), "## Stefan — GLF\n").unwrap();
        let dict = build_dictionary_for_vault(tmp.path());
        assert!(dict.people.iter().any(|p| p.name == "Stefan"));
    }

    #[test]
    fn vault_root_detected_from_memory_sibling() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let root = vault_root_for(&mem.join("Decisions.md"));
        assert_eq!(root.as_deref(), Some(tmp.path()));
    }

    #[test]
    fn resolve_diacritic_and_ascii_to_same_entity() {
        let mut d = EntityDictionary::new();
        d.add_person("djordje", "Đorđe Dimitrijević", &["Đorđe".into()]);
        assert_eq!(resolve_entity(&d, "Đorđe").unwrap().id, "person:djordje");
        assert_eq!(resolve_entity(&d, "djordje").unwrap().id, "person:djordje");
        assert_eq!(
            resolve_entity(&d, "DIMITRIJEVIĆ").unwrap().id,
            "person:djordje"
        );
        assert!(resolve_entity(&d, "Nobody").is_none());
    }
}
