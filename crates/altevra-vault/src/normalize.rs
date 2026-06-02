//! Document-level frontmatter normalization (the SAFE half of "Atomizacija").
//!
//! Walks an Obsidian vault and gives every `*.md` a universal frontmatter
//! envelope so the whole vault becomes uniformly typed/tagged and machine-findable
//! (R13: structure + governed tags = the search substrate). This is
//! NON-DESTRUCTIVE by axiom:
//!
//!   * Only ADDS/MERGES frontmatter — never edits the body, never deletes a file.
//!   * PRESERVES every existing frontmatter key verbatim (we only fill MISSING
//!     universal fields).
//!   * IDEMPOTENT — a file already carrying `altevra_normalized: true` with every
//!     universal field present yields `changed = false` (no rewrite).
//!
//! The pure core is [`normalize_frontmatter`]; the CLI walks the vault, builds the
//! per-file `(doc_type, domain)` from the folder map, and either prints a dry-run
//! plan or (with `--apply`, after a full vault backup) writes the merged file.

use crate::frontmatter::{parse_frontmatter, serialize_frontmatter, Frontmatter};
use altevra_core::domain::Domain;
use altevra_core::security::Sensitivity;
use chrono::NaiveDate;
use serde_yaml::{Mapping, Value};

/// The universal frontmatter keys Altevra guarantees on every normalized doc.
/// (Existing keys beyond these are always preserved; these are only FILLED when
/// missing.)
pub const UNIVERSAL_KEYS: &[&str] = &[
    "type",
    "domain",
    "sensitivity",
    "status",
    "tags",
    "created",
    "updated",
    "source",
    "altevra_normalized",
];

/// What the folder map resolved a file to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocClass {
    pub doc_type: String,
    pub domain: Domain,
    /// Optional project scope (e.g. `Projects/ReVesta/*` → `Some("ReVesta")`).
    pub scope: Option<String>,
    /// `true` when the file lives under `Archive/` — normalize sets `status: archived`.
    pub archived: bool,
}

/// Map a vault-relative path to its `(type, domain, scope, archived)`.
///
/// Folder rules (RECONCILIATION R13 / VAULT_DOCUMENT_TEMPLATE.md):
///   * `Daily/*`                  → daily_brief / business
///   * `Memory/Decisions*`        → decision / business
///   * `Memory/Learnings*`        → learning / business
///   * `Memory/People*`           → person / relationship
///   * `Memory/*` (other)         → note / business
///   * `Projects/<P>/*`           → note / project, scope=<P>
///   * `Wiki/*` | `Library/Wiki/*`→ wiki_page / business
///   * `Archive/*`                → archived=true, type kept from inner inference
///   * `Ideas/*`                  → idea / business
///   * `Research/*`               → research / business
///   * `Content/*`                → content / business
///   * `System/*`                 → reference / business
///   * else                       → note / business
pub fn classify_path(rel: &str) -> DocClass {
    // Normalize separators + lowercase for matching; keep original for scope.
    let normalized = rel.replace('\\', "/");
    let lower = normalized.to_lowercase();
    let parts: Vec<&str> = normalized.split('/').collect();

    // Archive is a wrapper: detect, then classify the path AS IF un-archived so the
    // inner type is preserved (R: "Archive/* → status=archived + keep inferred type").
    if lower.starts_with("archive/") || lower.contains("/archive/") {
        let inner = strip_archive_prefix(rel);
        let mut c = classify_path(&inner);
        c.archived = true;
        return c;
    }

    let (doc_type, domain, scope): (&str, Domain, Option<String>) = if lower.starts_with("daily/") {
        ("daily_brief", Domain::Business, None)
    } else if lower.starts_with("memory/") {
        let stem = file_stem_lower(&lower);
        if stem.starts_with("decision") {
            ("decision", Domain::Business, None)
        } else if stem.starts_with("learning") {
            ("learning", Domain::Business, None)
        } else if stem.starts_with("people") || stem.starts_with("person") {
            ("person", Domain::Relationship, None)
        } else {
            ("note", Domain::Business, None)
        }
    } else if lower.starts_with("projects/") {
        // scope = the project directory name (the component right after Projects/).
        let scope = parts.get(1).map(|s| s.to_string());
        ("note", Domain::Project, scope)
    } else if lower.starts_with("wiki/") || lower.starts_with("library/wiki/") {
        ("wiki_page", Domain::Business, None)
    } else if lower.starts_with("ideas/") {
        ("idea", Domain::Business, None)
    } else if lower.starts_with("research/") {
        ("research", Domain::Business, None)
    } else if lower.starts_with("content/") {
        ("content", Domain::Business, None)
    } else if lower.starts_with("system/") {
        ("reference", Domain::Business, None)
    } else {
        ("note", Domain::Business, None)
    };

    DocClass {
        doc_type: doc_type.to_string(),
        domain,
        scope,
        archived: false,
    }
}

/// Drop a leading `Archive/` (or the segment up to and including `/Archive/`) so
/// the remainder can be classified by its inner folder.
fn strip_archive_prefix(rel: &str) -> String {
    let norm = rel.replace('\\', "/");
    let lower = norm.to_lowercase();
    if let Some(idx) = lower.find("/archive/") {
        return norm[idx + "/archive/".len()..].to_string();
    }
    if let Some(rest) = norm.get("Archive/".len()..) {
        // case-insensitive leading "archive/"
        if lower.starts_with("archive/") {
            return rest.to_string();
        }
    }
    norm
}

fn file_stem_lower(lower_path: &str) -> String {
    lower_path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .strip_suffix(".md")
        .unwrap_or_else(|| lower_path.rsplit('/').next().unwrap_or(""))
        .to_string()
}

/// Pure frontmatter merge. Returns the normalized YAML mapping + whether anything
/// changed (so the CLI can skip untouched files and stay idempotent).
///
/// Fills ONLY missing universal fields; every existing key is preserved verbatim:
///   * `type`                = `doc_type`
///   * `domain`              = `domain`
///   * `sensitivity`         = `internal`, or `restricted` for a high-water domain
///   * `status`              = `archived` when `archived`, else `active`
///   * `tags`                = seed `[domain]` if empty/absent
///   * `created`             = existing else `created` arg (file mtime date)
///   * `updated`             = `now` arg (file mtime date)
///   * `source`              = `obsidian`
///   * `altevra_normalized`  = `true`
///
/// `existing` is the parsed frontmatter (or `None`). `created`/`now` are dates the
/// CLI derives from file mtime (Date::now is unavailable in tests; the CLI passes
/// real `SystemTime`-derived dates). `changed` is `false` iff the input already had
/// every universal field with the same values (idempotent second pass).
pub fn normalize_frontmatter(
    existing: Option<&Frontmatter>,
    doc_type: &str,
    domain: &Domain,
    scope: Option<&str>,
    created: NaiveDate,
    now: NaiveDate,
    archived: bool,
) -> (Value, bool) {
    // Start from the existing mapping (preserve everything), or an empty one.
    let mut map: Mapping = match existing.map(|f| &f.raw) {
        Some(Value::Mapping(m)) => m.clone(),
        _ => Mapping::new(),
    };
    let before = map.clone();

    // Helper: set only if the key is absent (preserve existing).
    let set_if_absent = |m: &mut Mapping, key: &str, val: Value| {
        let k = Value::String(key.to_string());
        if !m.contains_key(&k) {
            m.insert(k, val);
        }
    };

    let sensitivity = if domain.is_high_water() {
        Sensitivity::Restricted
    } else {
        Sensitivity::Internal
    };
    let status = if archived { "archived" } else { "active" };

    set_if_absent(&mut map, "type", Value::String(doc_type.to_string()));
    set_if_absent(&mut map, "domain", Value::String(domain.to_string()));
    set_if_absent(
        &mut map,
        "sensitivity",
        Value::String(sensitivity.to_string()),
    );
    set_if_absent(&mut map, "status", Value::String(status.to_string()));

    // scope only when present (Projects/<P>/*).
    if let Some(s) = scope {
        set_if_absent(&mut map, "scope", Value::String(s.to_string()));
    }

    // tags: seed [domain] only if missing OR present-but-empty.
    let tags_key = Value::String("tags".to_string());
    let tags_empty = match map.get(&tags_key) {
        None => true,
        Some(Value::Sequence(seq)) => seq.is_empty(),
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Null) => true,
        _ => false,
    };
    if tags_empty {
        map.insert(
            tags_key,
            Value::Sequence(vec![Value::String(domain.to_string())]),
        );
    }

    set_if_absent(
        &mut map,
        "created",
        Value::String(created.format("%Y-%m-%d").to_string()),
    );
    // `updated` is seeded ONLY when absent — never bumped on a re-run. Bumping it to
    // the file's mtime each pass is non-idempotent: the act of writing changes the
    // mtime, so the next run would see a new `updated` and rewrite forever. It marks
    // when the doc was first normalized; a genuine future content-versioning bump is
    // a separate concern.
    set_if_absent(
        &mut map,
        "updated",
        Value::String(now.format("%Y-%m-%d").to_string()),
    );

    set_if_absent(&mut map, "source", Value::String("obsidian".to_string()));
    set_if_absent(&mut map, "altevra_normalized", Value::Bool(true));

    let changed = map != before;
    (Value::Mapping(map), changed)
}

/// Build the full file content (frontmatter block + original body) for writing.
/// `body` is the post-frontmatter body returned by `parse_frontmatter`.
pub fn render_normalized(frontmatter: &Value, body: &str) -> anyhow::Result<String> {
    let fm = Frontmatter::new(frontmatter.clone());
    let block = serialize_frontmatter(&fm)?;
    // serialize_frontmatter ends with "---\n"; join with a blank line then the body.
    Ok(format!("{block}\n{body}"))
}

/// Parse a file's content into `(existing_frontmatter, body)` for normalization.
/// A malformed frontmatter block surfaces as an error the CLI can skip-and-report.
pub fn split_for_normalize(content: &str) -> anyhow::Result<(Option<Frontmatter>, String)> {
    let (fm, body) = parse_frontmatter(content)?;
    Ok((fm, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn fm_from(yaml: &str) -> Frontmatter {
        let v: Value = serde_yaml::from_str(yaml).unwrap();
        Frontmatter::new(v)
    }

    #[test]
    fn classify_folder_map() {
        assert_eq!(classify_path("Daily/2026-06-02.md").doc_type, "daily_brief");
        assert_eq!(classify_path("Memory/Decisions.md").doc_type, "decision");
        assert_eq!(classify_path("Memory/Learnings.md").doc_type, "learning");
        let people = classify_path("Memory/People.md");
        assert_eq!(people.doc_type, "person");
        assert_eq!(people.domain, Domain::Relationship);

        let proj = classify_path("Projects/ReVesta/notes.md");
        assert_eq!(proj.doc_type, "note");
        assert_eq!(proj.domain, Domain::Project);
        assert_eq!(proj.scope.as_deref(), Some("ReVesta"));

        assert_eq!(
            classify_path("Library/Wiki/altevra.md").doc_type,
            "wiki_page"
        );
        assert_eq!(classify_path("Ideas/x.md").doc_type, "idea");
        assert_eq!(classify_path("Research/x.md").doc_type, "research");
        assert_eq!(classify_path("Content/x.md").doc_type, "content");
        assert_eq!(classify_path("System/x.md").doc_type, "reference");
        assert_eq!(classify_path("loose-root-note.md").doc_type, "note");
    }

    #[test]
    fn archive_keeps_inner_type_and_marks_archived() {
        let c = classify_path("Archive/Daily/2026-01-01.md");
        assert_eq!(c.doc_type, "daily_brief");
        assert!(c.archived);
        let c2 = classify_path("Archive/Memory/Decisions.md");
        assert_eq!(c2.doc_type, "decision");
        assert!(c2.archived);
    }

    #[test]
    fn missing_frontmatter_gets_all_universal_fields() {
        let (v, changed) = normalize_frontmatter(
            None,
            "decision",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        assert!(changed, "an empty file gains frontmatter → changed");
        let m = v.as_mapping().unwrap();
        for key in UNIVERSAL_KEYS {
            assert!(
                m.contains_key(Value::String(key.to_string())),
                "missing universal key: {key}"
            );
        }
        assert_eq!(m["type"], Value::String("decision".into()));
        assert_eq!(m["domain"], Value::String("business".into()));
        assert_eq!(m["sensitivity"], Value::String("internal".into()));
        assert_eq!(m["status"], Value::String("active".into()));
        assert_eq!(m["created"], Value::String("2026-01-01".into()));
        assert_eq!(m["updated"], Value::String("2026-06-02".into()));
        assert_eq!(m["source"], Value::String("obsidian".into()));
        assert_eq!(m["altevra_normalized"], Value::Bool(true));
        // tags seeded with the domain
        assert_eq!(
            m["tags"],
            Value::Sequence(vec![Value::String("business".into())])
        );
    }

    #[test]
    fn high_water_domain_gets_restricted_sensitivity() {
        let (v, _) = normalize_frontmatter(
            None,
            "person",
            &Domain::Relationship,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        assert_eq!(
            v.as_mapping().unwrap()["sensitivity"],
            Value::String("restricted".into())
        );
    }

    #[test]
    fn archived_status_set_when_archived() {
        let (v, _) = normalize_frontmatter(
            None,
            "note",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            true,
        );
        assert_eq!(
            v.as_mapping().unwrap()["status"],
            Value::String("archived".into())
        );
    }

    #[test]
    fn existing_keys_are_preserved_not_overwritten() {
        // A file with hand-authored frontmatter that DISAGREES with inference:
        // type=meeting, sensitivity=confidential, a custom key, and existing tags.
        let existing = fm_from(
            "type: meeting\nsensitivity: confidential\ncustom_field: keepme\ntags: [revesta, gtm]\ncreated: 2025-12-25\n",
        );
        let (v, changed) = normalize_frontmatter(
            Some(&existing),
            "decision", // inference says decision …
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        let m = v.as_mapping().unwrap();
        // … but the existing values WIN (preserved verbatim).
        assert_eq!(m["type"], Value::String("meeting".into()));
        assert_eq!(m["sensitivity"], Value::String("confidential".into()));
        assert_eq!(m["custom_field"], Value::String("keepme".into()));
        assert_eq!(m["created"], Value::String("2025-12-25".into()));
        // existing non-empty tags preserved (not reseeded)
        assert_eq!(
            m["tags"],
            Value::Sequence(vec![
                Value::String("revesta".into()),
                Value::String("gtm".into())
            ])
        );
        // still gained the missing universal fields → changed
        assert!(changed);
        assert_eq!(m["domain"], Value::String("business".into()));
        assert_eq!(m["altevra_normalized"], Value::Bool(true));
    }

    #[test]
    fn empty_tags_get_reseeded_with_domain() {
        let existing = fm_from("tags: []\ntype: note\n");
        let (v, _) = normalize_frontmatter(
            Some(&existing),
            "note",
            &Domain::Project,
            Some("Tunia"),
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        let m = v.as_mapping().unwrap();
        assert_eq!(
            m["tags"],
            Value::Sequence(vec![Value::String("project".into())])
        );
        assert_eq!(m["scope"], Value::String("Tunia".into()));
    }

    #[test]
    fn idempotent_second_pass_reports_no_change() {
        // First pass on an empty file.
        let (v1, c1) = normalize_frontmatter(
            None,
            "learning",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        assert!(c1);
        // Feed the result back in with the SAME `now` (mtime unchanged) → no change.
        let fm1 = Frontmatter::new(v1);
        let (_v2, c2) = normalize_frontmatter(
            Some(&fm1),
            "learning",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        assert!(
            !c2,
            "second pass with same mtime must be a no-op (idempotent)"
        );
    }

    #[test]
    fn updated_is_stable_across_mtime_advances_idempotent() {
        // `updated` is seeded once and NOT bumped on re-runs — otherwise the write
        // itself moves the mtime and every pass would rewrite (non-idempotent on a
        // real vault, which is exactly the bug this guards). A newer mtime on an
        // already-normalized file is therefore a no-op.
        let (v1, _) = normalize_frontmatter(
            None,
            "note",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        let fm1 = Frontmatter::new(v1);
        let (v2, c2) = normalize_frontmatter(
            Some(&fm1),
            "note",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 10), // mtime advanced — but updated must stay put
            false,
        );
        assert!(
            !c2,
            "already-normalized file is a no-op even if mtime advanced"
        );
        if let Value::Mapping(m) = v2 {
            assert_eq!(
                m["updated"],
                Value::String("2026-06-02".into()),
                "updated stays at first-normalized date, not bumped"
            );
        } else {
            panic!("expected mapping");
        }
    }

    #[test]
    fn render_preserves_body_verbatim() {
        let (v, _) = normalize_frontmatter(
            None,
            "note",
            &Domain::Business,
            None,
            d(2026, 1, 1),
            d(2026, 6, 2),
            false,
        );
        let body = "# My Note\n\nLine one.\nLine two with a `## fake heading` inside.\n";
        let out = render_normalized(&v, body).unwrap();
        assert!(out.starts_with("---\n"));
        assert!(out.contains("type: note"));
        // body appears UNCHANGED after the closing delimiter
        assert!(out.contains(body));
    }

    #[test]
    fn split_roundtrips_existing_frontmatter() {
        let content = "---\ntype: meeting\n---\n# Body\ntext\n";
        let (fm, body) = split_for_normalize(content).unwrap();
        assert_eq!(fm.unwrap().get_str("type"), Some("meeting"));
        assert_eq!(body, "# Body\ntext\n");
    }
}
