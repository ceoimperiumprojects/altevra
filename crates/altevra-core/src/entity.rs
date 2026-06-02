//! Entity extraction → mention graph (keyless, deterministic — NO LLM/NER).
//!
//! Pavle's vision §4.1 (cross-category: "what did I do with Đorđe this month") +
//! §3.6 (proactive "you haven't talked to Srđan in 6 weeks"). When captured text
//! mentions a KNOWN person or project, we link it. The known-entity set comes from
//! the vault itself — `Memory/People.md` `## <Name>` headings + the project
//! registry — so the dictionary stays current with no model and no training.
//!
//! This module is PURE + I/O-free: the [`EntityDictionary`] is built from already-
//! read text (the CLI/db layer does the file reads), [`detect_mentions`] is a
//! deterministic word-boundary + diacritic-folded scan, and [`last_contact`]
//! computes a most-recent-mention date. All testable with no fixtures on disk.
//!
//! Diacritics: SQLite FTS `unicode61` splits `đ`, and Pavle writes names both with
//! and without diacritics (`Đorđe`/`Djordje`, `Srđan`/`Srdjan`). Matching folds
//! BOTH sides to an ASCII form so every spelling hits the same entity.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// What kind of thing an entity is. Mirrors the two cross-link targets Pavle
/// cares about first (people + projects); extensible later (place, org, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Person,
    Project,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityKind::Person => "person",
            EntityKind::Project => "project",
        }
    }
}

/// A known entity from the vault: a stable id, a display name, and the set of
/// surface forms (aliases) that should all resolve to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    /// Stable id, e.g. `person:djordje` or `project:revesta`.
    pub id: String,
    /// Display name (the canonical heading / registry name).
    pub name: String,
    pub kind: EntityKind,
    /// Surface forms to match (includes the name itself + folded variants).
    pub aliases: Vec<String>,
}

impl Entity {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: EntityKind) -> Self {
        let name = name.into();
        Self {
            id: id.into(),
            aliases: vec![name.clone()],
            name,
            kind,
        }
    }

    /// Add an alias (deduped, never empty).
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        let a = alias.into();
        if !a.trim().is_empty() && !self.aliases.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            self.aliases.push(a);
        }
        self
    }
}

/// A detected mention: which entity, where in the text (byte span), and the
/// surface form that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRef {
    pub entity_id: String,
    pub kind: EntityKind,
    /// Byte span `[start, end)` in the scanned text.
    pub span: (usize, usize),
    /// The alias surface form that matched (display-cased as in the dictionary).
    pub matched: String,
}

/// The known-entity dictionary, built from the vault.
#[derive(Debug, Clone, Default)]
pub struct EntityDictionary {
    pub people: Vec<Entity>,
    pub projects: Vec<Entity>,
}

impl EntityDictionary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.people.is_empty() && self.projects.is_empty()
    }

    pub fn len(&self) -> usize {
        self.people.len() + self.projects.len()
    }

    /// All entities (people then projects) for scanning.
    pub fn all(&self) -> impl Iterator<Item = &Entity> {
        self.people.iter().chain(self.projects.iter())
    }

    /// Look up an entity by id.
    pub fn get(&self, id: &str) -> Option<&Entity> {
        self.all().find(|e| e.id == id)
    }

    /// Parse people from a `Memory/People.md`-shaped document: each `## <Heading>`
    /// becomes a person, taking the name as the portion BEFORE the first `—`/`-`/
    /// `(` separator (Pavle writes `## Luka — ReVesta landing page …`). Dated
    /// brainstorm headings (`## 2026-05-27 — …`) and parenthetical-only headings
    /// are skipped (they aren't people). Aliases: the full name, each capitalized
    /// token ≥3 chars (first name / surname), and ascii-folded variants — all added
    /// by [`finalize_aliases`].
    pub fn add_people_from_md(&mut self, content: &str) {
        for line in content.lines() {
            let Some(raw) = line.strip_prefix("## ") else {
                continue;
            };
            let heading = raw.trim();
            // Skip dated headings (start with YYYY-MM-DD) — those are notes, not people.
            if starts_with_date(heading) {
                continue;
            }
            // Name = before the first em-dash / en-dash / hyphen-with-spaces / paren.
            let name = split_person_name(heading);
            if name.chars().filter(|c| c.is_alphabetic()).count() < 3 {
                continue; // too short / not a real name
            }
            // Skip placeholder names ("Co-founder #1", "Ime TBD").
            let lower = name.to_lowercase();
            if lower.contains("tbd") || lower.contains('#') {
                continue;
            }
            let id = format!("person:{}", slug(&name));
            let mut ent = Entity::new(id, &name, EntityKind::Person);
            // Per-token aliases: first name + surname individually (≥3 chars).
            for tok in name.split_whitespace() {
                if tok.chars().filter(|c| c.is_alphabetic()).count() >= 3 {
                    ent = ent.with_alias(tok);
                }
            }
            ent = finalize_aliases(ent);
            // Dedup by id.
            if !self.people.iter().any(|p| p.id == ent.id) {
                self.people.push(ent);
            }
        }
    }

    /// Add a project entity (id + name + explicit aliases from the registry).
    /// Folded aliases are added by [`finalize_aliases`].
    pub fn add_project(&mut self, id: &str, name: &str, aliases: &[String]) {
        let full_id = if id.starts_with("project:") {
            id.to_string()
        } else {
            format!("project:{id}")
        };
        if self.projects.iter().any(|p| p.id == full_id) {
            return;
        }
        let mut ent = Entity::new(full_id, name, EntityKind::Project);
        for a in aliases {
            ent = ent.with_alias(a);
        }
        // Also add the bare registry id token (e.g. "revesta") as an alias.
        ent = ent.with_alias(id.trim_start_matches("project:"));
        ent = finalize_aliases(ent);
        self.projects.push(ent);
    }

    /// Add extra known people not present as a People.md heading (e.g. mentors
    /// referenced only in body text). Used to seed Đorđe/Srđan/Saša from identity.
    pub fn add_person(&mut self, id: &str, name: &str, aliases: &[String]) {
        let full_id = if id.starts_with("person:") {
            id.to_string()
        } else {
            format!("person:{id}")
        };
        if self.people.iter().any(|p| p.id == full_id) {
            return;
        }
        let mut ent = Entity::new(full_id, name, EntityKind::Person);
        for a in aliases {
            ent = ent.with_alias(a);
        }
        // Per-token aliases (first name / surname individually), like People.md
        // parsing — so resolving by surname (`Dimitrijević`) finds the person.
        for tok in name.split_whitespace() {
            if tok.chars().filter(|c| c.is_alphabetic()).count() >= 3 {
                ent = ent.with_alias(tok);
            }
        }
        ent = finalize_aliases(ent);
        self.people.push(ent);
    }
}

/// Add ascii-folded variants of every existing alias (so `Đorđe` also matches
/// `Djordje`), dedup, and drop aliases shorter than 3 alphabetic chars (avoids
/// false positives on tiny common tokens like "ss" or "VC").
pub fn finalize_aliases(mut ent: Entity) -> Entity {
    let mut out: Vec<String> = Vec::new();
    for a in &ent.aliases {
        push_unique(&mut out, a.clone());
        let folded = ascii_fold(a);
        if &folded != a {
            push_unique(&mut out, folded);
        }
    }
    out.retain(|a| a.chars().filter(|c| c.is_alphanumeric()).count() >= 3);
    // Longest-first so multi-word names are tried before their parts.
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    ent.aliases = out;
    ent
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !s.trim().is_empty() && !v.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        v.push(s);
    }
}

/// Fold Serbian/diacritic letters to a plain-ASCII surface form. `đ`→`dj`, `š`→`s`,
/// `č`/`ć`→`c`, `ž`→`z`, plus common accented Latin. Lowercase-insensitive input;
/// preserves case where a 1:1 mapping exists.
pub fn ascii_fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'đ' => out.push_str("dj"),
            'Đ' => out.push_str("Dj"),
            'š' => out.push('s'),
            'Š' => out.push('S'),
            'č' | 'ć' => out.push('c'),
            'Č' | 'Ć' => out.push('C'),
            'ž' => out.push('z'),
            'Ž' => out.push('Z'),
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => out.push('a'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'í' | 'ì' | 'î' | 'ï' => out.push('i'),
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => out.push('o'),
            'ú' | 'ù' | 'û' | 'ü' => out.push('u'),
            'ñ' => out.push('n'),
            other => out.push(other),
        }
    }
    out
}

/// Detect mentions of dictionary entities in `text`. Word-boundary, case- and
/// diacritic-insensitive (both text and alias are ascii-folded + lowercased).
/// Per text position the LONGEST alias wins (so "Ivan Kadić" beats bare "Ivan");
/// each (entity, span) is reported once. Aliases <3 alnum chars never match
/// (filtered at dictionary build), avoiding false hits on tiny tokens.
pub fn detect_mentions(text: &str, dict: &EntityDictionary) -> Vec<EntityRef> {
    // Fold the haystack once; folding can change byte length (đ→dj), so we map
    // folded-byte positions back to original-byte positions via a parallel index.
    let (folded, idx_map) = fold_with_index(text);
    let folded_lc = folded.to_lowercase();
    // Note: lowercasing ascii (post-fold it's ascii) is 1:1 in byte length, so the
    // idx_map stays valid against folded_lc.

    let mut hits: Vec<EntityRef> = Vec::new();
    // Build (alias_folded_lc, entity) pairs, longest alias first across ALL entities
    // so a longer match suppresses a contained shorter one at the same start.
    let mut alias_pairs: Vec<(String, &Entity, &str)> = Vec::new();
    for ent in dict.all() {
        for a in &ent.aliases {
            let af = ascii_fold(a).to_lowercase();
            if af.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
                alias_pairs.push((af, ent, a.as_str()));
            }
        }
    }
    alias_pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));

    // Track covered folded-byte ranges to prevent overlapping/duplicate hits.
    let mut covered: Vec<(usize, usize)> = Vec::new();

    for (alias_fl, ent, display) in &alias_pairs {
        let mut from = 0usize;
        while let Some(rel) = folded_lc[from..].find(alias_fl.as_str()) {
            let start = from + rel;
            let mut end = start + alias_fl.len();
            from = start + 1;
            // The LEFT edge must always be a word boundary (no matching mid-word).
            if !left_boundary(&folded_lc, start) {
                continue;
            }
            // RIGHT edge: a clean boundary, OR — for a name-like PERSON alias
            // (≥4 chars) — a short Serbian inflectional suffix (Đorđe→Đorđetova,
            // Srđanu, …). This catches case/possessive forms without the
            // false-positive risk of substring matching (alias must still start a
            // word; tiny aliases like "ss" never inflect-match).
            if right_boundary(&folded_lc, end) {
                // exact word — keep `end`.
            } else if ent.kind == EntityKind::Person
                && alias_fl.chars().filter(|c| c.is_alphanumeric()).count() >= 4
            {
                match inflection_end(&folded_lc, end) {
                    Some(infl_end) => end = infl_end, // extend over the suffix
                    None => continue,
                }
            } else {
                continue;
            }
            // Skip if this folded range overlaps an already-accepted (longer) hit.
            if covered.iter().any(|(s, e)| start < *e && end > *s) {
                continue;
            }
            // Map folded span back to ORIGINAL byte span.
            let orig_start = idx_map.get(start).copied().unwrap_or(start);
            let orig_end = idx_map.get(end).copied().unwrap_or(text.len());
            // Dedup: one ref per (entity, original-start).
            if hits
                .iter()
                .any(|h| h.entity_id == ent.id && h.span.0 == orig_start)
            {
                continue;
            }
            covered.push((start, end));
            hits.push(EntityRef {
                entity_id: ent.id.clone(),
                kind: ent.kind,
                span: (orig_start, orig_end),
                matched: (*display).to_string(),
            });
        }
    }
    hits.sort_by_key(|h| h.span.0);
    hits
}

/// The set of distinct entity ids mentioned in `text` (dedup of [`detect_mentions`]).
pub fn mentioned_entity_ids(text: &str, dict: &EntityDictionary) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for m in detect_mentions(text, dict) {
        if !ids.contains(&m.entity_id) {
            ids.push(m.entity_id);
        }
    }
    ids
}

/// Most-recent date among `dated` items that mention `entity_id`. Foundation for
/// "you haven't talked to X in N weeks". Each item is `(entity_ids, date)`; the
/// caller supplies the per-object mention sets + dates (computed elsewhere).
pub fn last_contact(
    entity_id: &str,
    dated: &[(Vec<String>, Option<NaiveDate>)],
) -> Option<NaiveDate> {
    dated
        .iter()
        .filter(|(ids, _)| ids.iter().any(|i| i == entity_id))
        .filter_map(|(_, d)| *d)
        .max()
}

// ---- internals ----

/// Fold a string to ASCII and return (folded, map) where `map[i]` is the ORIGINAL
/// byte offset corresponding to folded byte offset `i`. `map` has folded.len()+1
/// entries (the last maps to text.len()) so an end position is always resolvable.
fn fold_with_index(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut map: Vec<usize> = Vec::with_capacity(text.len() + 1);
    for (orig_off, c) in text.char_indices() {
        let f = ascii_fold(&c.to_string());
        for _ in 0..f.len() {
            map.push(orig_off);
        }
        folded.push_str(&f);
    }
    map.push(text.len());
    (folded, map)
}

/// Left word boundary: char before `start` is non-alphanumeric (or string edge).
/// Prevents matching "Ana" starting inside "Banana".
fn left_boundary(s: &str, start: usize) -> bool {
    start == 0
        || s[..start]
            .chars()
            .next_back()
            .map(|c| !c.is_alphanumeric())
            .unwrap_or(true)
}

/// Right word boundary: char after `end` is non-alphanumeric (or string edge).
fn right_boundary(s: &str, end: usize) -> bool {
    end >= s.len()
        || s[end..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric())
            .unwrap_or(true)
}

/// If the alphabetic run immediately after `end` is a SHORT (≤4 char) Serbian
/// inflectional suffix followed by a word boundary, return the end of that run
/// (so the match extends over `Đorđe|tova`). Else `None`. This keeps person-name
/// case/possessive forms linkable without opening up arbitrary substring matches.
fn inflection_end(s: &str, end: usize) -> Option<usize> {
    let rest = &s[end..];
    let suffix: String = rest.chars().take_while(|c| c.is_alphabetic()).collect();
    if suffix.is_empty() || suffix.chars().count() > 4 {
        return None;
    }
    let new_end = end + suffix.len();
    // The inflected word must itself end at a boundary (not run into more letters
    // beyond the 4-char window, and not be glued to digits).
    if !right_boundary(s, new_end) {
        return None;
    }
    // Only accept recognized Serbian inflection shapes (vowel/short consonantal
    // endings) — avoids gluing an unrelated word onto the name.
    const INFLECTIONS: &[&str] = &[
        "a", "u", "e", "i", "m", "om", "em", "ev", "ov", "evi", "ovi", "tova", "tovu", "tov", "tu",
        "ta", "te", "ka", "ku", "ko", "in", "ina", "inu",
    ];
    if INFLECTIONS.contains(&suffix.as_str()) {
        Some(new_end)
    } else {
        None
    }
}

/// True if a heading begins with a `YYYY-MM-DD` date.
fn starts_with_date(h: &str) -> bool {
    let b = h.as_bytes();
    b.len() >= 10
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

/// Take the person name from a People.md heading: the part before the first
/// separator (`—`, `–`, ` - `, `(`).
fn split_person_name(heading: &str) -> String {
    // em/en dash first (Pavle's style: "Name — role").
    for sep in ["—", "–"] {
        if let Some(idx) = heading.find(sep) {
            return heading[..idx].trim().to_string();
        }
    }
    // " - " (spaced hyphen) — avoid splitting hyphenated surnames.
    if let Some(idx) = heading.find(" - ") {
        return heading[..idx].trim().to_string();
    }
    if let Some(idx) = heading.find(" (") {
        return heading[..idx].trim().to_string();
    }
    heading.trim().to_string()
}

fn slug(s: &str) -> String {
    let raw: String = ascii_fold(s)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    raw.trim_matches('-')
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict_with_djordje() -> EntityDictionary {
        let mut d = EntityDictionary::new();
        d.add_person(
            "djordje",
            "Đorđe Dimitrijević",
            &["Đorđe".into(), "Dimitrijević".into()],
        );
        d.add_person("srdjan", "Srđan Jovanović", &["Srđan".into()]);
        d.add_project(
            "revesta",
            "ReVesta",
            &["Simple Surplus".into(), "ss".into()],
        );
        d
    }

    #[test]
    fn ascii_fold_handles_serbian() {
        assert_eq!(ascii_fold("Đorđe"), "Djordje");
        assert_eq!(ascii_fold("Srđan"), "Srdjan");
        assert_eq!(ascii_fold("Dimitrijević"), "Dimitrijevic");
        assert_eq!(ascii_fold("Žarko"), "Zarko");
    }

    #[test]
    fn people_md_parse_takes_name_before_dash_skips_dated_and_tbd() {
        let md = "# People\n\n\
                  ## Luka — ReVesta landing page + lead scraping\n\
                  ## Ivan Kadić — ReVesta thread participant\n\
                  ## Danilo\n\
                  ## Co-founder #1 (Natal VC) — Ime TBD\n\
                  ## 2026-05-27 — Natal VC formal mentor trio\n";
        let mut d = EntityDictionary::new();
        d.add_people_from_md(md);
        let names: Vec<&str> = d.people.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Luka"));
        assert!(names.contains(&"Ivan Kadić"));
        assert!(names.contains(&"Danilo"));
        // placeholder + dated headings excluded
        assert!(!names.iter().any(|n| n.contains("Co-founder")));
        assert!(!names.iter().any(|n| n.contains("2026")));
    }

    #[test]
    fn detect_djordje_all_spellings_hit_same_entity() {
        let d = dict_with_djordje();
        for text in [
            "Razgovarao sam sa Đorđe o ReVesta.",
            "Djordje je rekao da prodajem.",
            "Mentor Dimitrijević je dao direktivu.",
        ] {
            let ids = mentioned_entity_ids(text, &d);
            assert!(
                ids.iter().any(|i| i == "person:djordje"),
                "'{text}' should mention Đorđe → {ids:?}"
            );
        }
    }

    #[test]
    fn no_false_match_inside_other_words() {
        let d = dict_with_djordje();
        // "ss" is a project alias but must NOT match inside "assistant" / "boss".
        let ids = mentioned_entity_ids("the assistant talked to the boss", &d);
        assert!(
            !ids.iter().any(|i| i == "project:revesta"),
            "no substring false-positive: {ids:?}"
        );
    }

    #[test]
    fn serbian_inflection_of_person_name_is_matched() {
        let d = dict_with_djordje();
        // Real Decisions.md form: "Đorđetova direktiva" (possessive). Must link.
        for text in [
            "Đorđetova direktiva i dalje važi",
            "rekao sam Srđanu juče",
            "pričao sam sa Đorđem o tome",
        ] {
            let ids = mentioned_entity_ids(text, &d);
            assert!(
                ids.iter()
                    .any(|i| i == "person:djordje" || i == "person:srdjan"),
                "'{text}' should match an inflected mentor name → {ids:?}"
            );
        }
    }

    #[test]
    fn inflection_does_not_glue_unrelated_word() {
        let mut d = EntityDictionary::new();
        // "Luka" (4 chars) must NOT match inside "Lukavac" (a place) — the run after
        // "Luka" is "vac" (3 chars) but not a recognized inflection.
        d.add_person("luka", "Luka", &[]);
        let ids = mentioned_entity_ids("putovao je u Lukavac prošle godine", &d);
        assert!(
            !ids.iter().any(|i| i == "person:luka"),
            "Lukavac must not match Luka: {ids:?}"
        );
        // the bare name still matches.
        assert!(mentioned_entity_ids("Luka je tu", &d).contains(&"person:luka".to_string()));
    }

    #[test]
    fn multiword_name_beats_its_parts() {
        let mut d = EntityDictionary::new();
        d.add_person("ivan-kadic", "Ivan Kadić", &[]);
        let refs = detect_mentions("met Ivan Kadić yesterday", &d);
        // exactly one ref, the full name (not also a bare "Ivan")
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].matched.to_lowercase(), "ivan kadić".to_lowercase());
    }

    #[test]
    fn project_alias_and_id_match() {
        let d = dict_with_djordje();
        assert!(mentioned_entity_ids("working on ReVesta GTM", &d)
            .contains(&"project:revesta".to_string()));
        assert!(mentioned_entity_ids("the Simple Surplus pipeline", &d)
            .contains(&"project:revesta".to_string()));
    }

    #[test]
    fn span_maps_back_through_diacritic_fold() {
        let d = dict_with_djordje();
        let text = "ok Đorđe ok";
        let refs = detect_mentions(text, &d);
        assert_eq!(refs.len(), 1);
        let (s, e) = refs[0].span;
        // The original substring at the mapped span must be the diacritic name.
        assert_eq!(&text[s..e], "Đorđe");
    }

    #[test]
    fn last_contact_picks_most_recent() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day);
        let items = vec![
            (vec!["person:djordje".to_string()], d(2026, 4, 10)),
            (vec!["person:djordje".to_string()], d(2026, 6, 1)),
            (vec!["person:srdjan".to_string()], d(2026, 5, 20)),
            (vec!["person:djordje".to_string()], None),
        ];
        assert_eq!(last_contact("person:djordje", &items), d(2026, 6, 1));
        assert_eq!(last_contact("person:srdjan", &items), d(2026, 5, 20));
        assert_eq!(last_contact("person:nobody", &items), None);
    }

    #[test]
    fn dict_dedups_by_id() {
        let mut d = EntityDictionary::new();
        d.add_project("revesta", "ReVesta", &[]);
        d.add_project("revesta", "ReVesta dup", &[]);
        assert_eq!(d.projects.len(), 1);
    }
}
