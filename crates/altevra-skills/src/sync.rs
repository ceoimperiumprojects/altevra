//! Cross-tool skill propagation — answers Pavle's "kad ubacim skill u Claude da se
//! automatski vidi u svima". Builds on `importer::ExternalSkill` (read-only scan)
//! and turns the diff into a `SyncPlan` of writes, executed only when the caller
//! explicitly opts in via `apply_plan(..., apply: true)`.
//!
//! Safety invariants (HARD — Pavle has 200+ third-party skills on disk):
//! * **NEVER overwrite a non-`ALTEVRA_MANAGED` file.** A target slot already
//!   holding a user-authored skill becomes a `Skip { reason: UserAuthored }`.
//!   Only Altevra-written copies (carrying the managed marker) may be replaced.
//! * Renderer ALWAYS injects the managed header so the next sync can detect
//!   "we wrote this".
//! * Every action is recorded — `dry_run = true` returns the full plan without
//!   touching disk, exactly what `--dry-run` shows.

use crate::importer::{ExternalSkill, SourceTool};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const MANAGED_MARKER: &str = "<!-- ALTEVRA_MANAGED: true -->";
const ADAPTER_VERSION: &str = "0.1.0";

/// One concrete operation the sync wants to perform on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncAction {
    /// Skill is missing in the target tool — write a new managed copy.
    Create {
        slug: String,
        from_tool: SourceTool,
        to_tool: SourceTool,
        target_path: PathBuf,
        source_path: PathBuf,
    },
    /// Target already has an Altevra-managed copy — refresh it if content drifted.
    Refresh {
        slug: String,
        from_tool: SourceTool,
        to_tool: SourceTool,
        target_path: PathBuf,
        source_path: PathBuf,
    },
    /// Target holds a user-authored (NOT managed) skill — never touch.
    Skip {
        slug: String,
        to_tool: SourceTool,
        target_path: PathBuf,
        reason: SkipReason,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Target file exists but lacks `<!-- ALTEVRA_MANAGED -->` — user-owned.
    UserAuthored,
    /// Existing managed content is byte-identical to what we'd write.
    AlreadyInSync,
    /// Source could not be read.
    SourceUnreadable,
    /// Target tool isn't a recognized adapter (no destination resolved).
    UnsupportedTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncPlan {
    pub actions: Vec<SyncAction>,
}

impl SyncPlan {
    pub fn creates(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Create { .. }))
            .count()
    }
    pub fn refreshes(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Refresh { .. }))
            .count()
    }
    pub fn skips(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Skip { .. }))
            .count()
    }
}

/// Result of applying a plan to disk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncResult {
    pub created: usize,
    pub refreshed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Compute the per-slug propagation plan from `inventory` to `targets`. For each
/// (slug, target_tool) pair where the target lacks the skill (or has a stale
/// managed copy), emit a `Create`/`Refresh`. `from` selects which source instance
/// to copy when a slug exists in multiple tools (default: prefer Altevra, then
/// Hermes, then anything; first-found within that order).
pub fn build_plan(
    inventory: &[ExternalSkill],
    targets: &[SourceTool],
    skill_dir_for: &dyn Fn(&SourceTool) -> Option<PathBuf>,
) -> SyncPlan {
    let mut by_slug: HashMap<String, Vec<&ExternalSkill>> = HashMap::new();
    for s in inventory {
        by_slug.entry(s.slug.clone()).or_default().push(s);
    }

    let mut actions = Vec::new();
    for (slug, instances) in &by_slug {
        // Pick the source: prefer a user-authored copy over a managed one (so we
        // propagate the original, not someone else's rendering of it). Within
        // that, prefer Altevra > Hermes > Claude > Codex > Cursor > Imperium.
        let source = pick_source(instances);
        for target in targets {
            if instances
                .iter()
                .any(|i| i.source_tool == *target && !i.managed)
            {
                // Target already has a user-authored copy — never replace.
                let existing = instances.iter().find(|i| i.source_tool == *target).unwrap();
                actions.push(SyncAction::Skip {
                    slug: slug.clone(),
                    to_tool: target.clone(),
                    target_path: existing.path.clone(),
                    reason: SkipReason::UserAuthored,
                });
                continue;
            }
            // Resolve where this target keeps its skills.
            let target_dir = match skill_dir_for(target) {
                Some(d) => d,
                None => {
                    actions.push(SyncAction::Skip {
                        slug: slug.clone(),
                        to_tool: target.clone(),
                        target_path: PathBuf::new(),
                        reason: SkipReason::UnsupportedTarget,
                    });
                    continue;
                }
            };
            let target_path = target_dir.join(slug).join("SKILL.md");
            // Source must be readable.
            let source_body = match std::fs::read_to_string(&source.path) {
                Ok(b) => b,
                Err(_) => {
                    actions.push(SyncAction::Skip {
                        slug: slug.clone(),
                        to_tool: target.clone(),
                        target_path,
                        reason: SkipReason::SourceUnreadable,
                    });
                    continue;
                }
            };
            let rendered = wrap_with_managed_header(&source_body, source.source_tool.as_str());
            // Managed copy exists?
            let existing_managed = instances
                .iter()
                .find(|i| i.source_tool == *target && i.managed);
            if let Some(existing) = existing_managed {
                let existing_body = std::fs::read_to_string(&existing.path).unwrap_or_default();
                if existing_body == rendered {
                    actions.push(SyncAction::Skip {
                        slug: slug.clone(),
                        to_tool: target.clone(),
                        target_path: existing.path.clone(),
                        reason: SkipReason::AlreadyInSync,
                    });
                } else {
                    actions.push(SyncAction::Refresh {
                        slug: slug.clone(),
                        from_tool: source.source_tool.clone(),
                        to_tool: target.clone(),
                        target_path: existing.path.clone(),
                        source_path: source.path.clone(),
                    });
                }
            } else {
                actions.push(SyncAction::Create {
                    slug: slug.clone(),
                    from_tool: source.source_tool.clone(),
                    to_tool: target.clone(),
                    target_path,
                    source_path: source.path.clone(),
                });
            }
        }
    }
    actions.sort_by_key(sync_key);
    SyncPlan { actions }
}

fn pick_source<'a>(instances: &[&'a ExternalSkill]) -> &'a ExternalSkill {
    let order = [
        SourceTool::Altevra,
        SourceTool::Hermes,
        SourceTool::Claude,
        SourceTool::Codex,
        SourceTool::Cursor,
        SourceTool::Imperium,
    ];
    // Prefer NON-managed first (user-authored source > rendered copy).
    for t in &order {
        if let Some(s) = instances.iter().find(|i| !i.managed && i.source_tool == *t) {
            return s;
        }
    }
    // Else fall through to any (even managed).
    for t in &order {
        if let Some(s) = instances.iter().find(|i| i.source_tool == *t) {
            return s;
        }
    }
    instances[0]
}

fn sync_key(a: &SyncAction) -> (String, String) {
    match a {
        SyncAction::Create { slug, to_tool, .. } => (slug.clone(), to_tool.as_str().to_string()),
        SyncAction::Refresh { slug, to_tool, .. } => (slug.clone(), to_tool.as_str().to_string()),
        SyncAction::Skip { slug, to_tool, .. } => (slug.clone(), to_tool.as_str().to_string()),
    }
}

/// Inject `<!-- ALTEVRA_MANAGED -->` (if absent) so the next sync recognises this
/// file as something it owns. Preserves the body as-is otherwise. Public so the
/// CLI's GUARDED applier (P3 install/sync — drift manifest + backups) renders
/// byte-identical content to what [`apply_plan`] would write.
pub fn wrap_with_managed_header(body: &str, source_tool: &str) -> String {
    if body.contains(MANAGED_MARKER) {
        return body.to_string();
    }
    format!(
        "{marker}\n<!-- source_tool: {source_tool} -->\n<!-- adapter: altevra-sync -->\n<!-- version: {ADAPTER_VERSION} -->\n{body}",
        marker = MANAGED_MARKER,
        source_tool = source_tool,
        body = body
    )
}

/// Apply a plan to disk. With `apply: false` performs no writes — caller uses it
/// for `--dry-run`. Returns counts + any per-action errors (e.g. parent mkdir
/// failure); never panics, never partially-corrupts (each write is atomic via
/// write-to-temp + rename).
pub fn apply_plan(plan: &SyncPlan, apply: bool) -> SyncResult {
    let mut r = SyncResult::default();
    for action in &plan.actions {
        match action {
            SyncAction::Create {
                target_path,
                source_path,
                from_tool,
                ..
            }
            | SyncAction::Refresh {
                target_path,
                source_path,
                from_tool,
                ..
            } => {
                let is_refresh = matches!(action, SyncAction::Refresh { .. });
                if !apply {
                    if is_refresh {
                        r.refreshed += 1;
                    } else {
                        r.created += 1;
                    }
                    continue;
                }
                let body = match std::fs::read_to_string(source_path) {
                    Ok(b) => b,
                    Err(e) => {
                        r.errors
                            .push(format!("read {}: {e}", source_path.display()));
                        continue;
                    }
                };
                let content = wrap_with_managed_header(&body, from_tool.as_str());
                if let Some(parent) = target_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        r.errors.push(format!("mkdir {}: {e}", parent.display()));
                        continue;
                    }
                }
                // Atomic: write-temp + rename so a crash mid-write never leaves a
                // half-file in someone else's skill dir.
                let tmp = target_path.with_extension("md.altevra-tmp");
                if let Err(e) = std::fs::write(&tmp, &content) {
                    r.errors.push(format!("write {}: {e}", tmp.display()));
                    continue;
                }
                if let Err(e) = std::fs::rename(&tmp, target_path) {
                    let _ = std::fs::remove_file(&tmp);
                    r.errors
                        .push(format!("rename to {}: {e}", target_path.display()));
                    continue;
                }
                if is_refresh {
                    r.refreshed += 1;
                } else {
                    r.created += 1;
                }
            }
            SyncAction::Skip { .. } => r.skipped += 1,
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_skill(slug: &str, tool: SourceTool, path: PathBuf, managed: bool) -> ExternalSkill {
        ExternalSkill {
            slug: slug.into(),
            source_tool: tool,
            path,
            version: Some("1.0.0".into()),
            description: Some("test".into()),
            managed,
            body_len: 100,
        }
    }

    fn write_skill(dir: &std::path::Path, slug: &str, body: &str) -> PathBuf {
        let bundle = dir.join(slug);
        fs::create_dir_all(&bundle).unwrap();
        let p = bundle.join("SKILL.md");
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn build_plan_creates_in_missing_target_and_skips_user_authored() {
        let tmp = TempDir::new().unwrap();
        let claude_dir = tmp.path().join("claude");
        let hermes_dir = tmp.path().join("hermes");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::create_dir_all(&hermes_dir).unwrap();

        // Slug A: only in Claude → expect Create in Hermes.
        let a_path = write_skill(&claude_dir, "a", "---\nname: a\n---\nbody\n");
        // Slug B: in both, Hermes copy is user-authored → expect Skip(UserAuthored).
        let b_claude = write_skill(&claude_dir, "b", "---\nname: b\n---\nfrom-claude\n");
        let b_hermes = write_skill(&hermes_dir, "b", "---\nname: b\n---\nuser-edit\n");

        let inventory = vec![
            make_skill("a", SourceTool::Claude, a_path, false),
            make_skill("b", SourceTool::Claude, b_claude, false),
            make_skill("b", SourceTool::Hermes, b_hermes, false),
        ];

        let claude_dir_owned = claude_dir.clone();
        let hermes_dir_owned = hermes_dir.clone();
        let resolver = move |t: &SourceTool| -> Option<PathBuf> {
            match t {
                SourceTool::Claude => Some(claude_dir_owned.clone()),
                SourceTool::Hermes => Some(hermes_dir_owned.clone()),
                _ => None,
            }
        };

        let plan = build_plan(&inventory, &[SourceTool::Hermes], &resolver);
        assert_eq!(plan.creates(), 1, "a should be created in hermes");
        assert_eq!(plan.skips(), 1, "b should be skipped (user-authored)");
        // The Create targets slug 'a' in hermes.
        let create = plan
            .actions
            .iter()
            .find(|a| matches!(a, SyncAction::Create { slug, .. } if slug == "a"))
            .unwrap();
        if let SyncAction::Create {
            from_tool,
            to_tool,
            target_path,
            ..
        } = create
        {
            assert_eq!(*from_tool, SourceTool::Claude);
            assert_eq!(*to_tool, SourceTool::Hermes);
            assert!(
                target_path.starts_with(&hermes_dir),
                "target lands in hermes dir"
            );
            assert!(target_path.ends_with("a/SKILL.md"));
        }
    }

    #[test]
    fn apply_writes_with_managed_header_and_refresh_updates_existing() {
        let tmp = TempDir::new().unwrap();
        let claude_dir = tmp.path().join("claude");
        let hermes_dir = tmp.path().join("hermes");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::create_dir_all(&hermes_dir).unwrap();
        let src = write_skill(
            &claude_dir,
            "a",
            "---\nname: a\nversion: 1.0.0\n---\nv1 body\n",
        );

        let inventory = vec![make_skill("a", SourceTool::Claude, src.clone(), false)];
        let claude_dir_owned = claude_dir.clone();
        let hermes_dir_owned = hermes_dir.clone();
        let resolver = move |t: &SourceTool| -> Option<PathBuf> {
            match t {
                SourceTool::Claude => Some(claude_dir_owned.clone()),
                SourceTool::Hermes => Some(hermes_dir_owned.clone()),
                _ => None,
            }
        };

        // 1) Dry-run: plan reports a Create but disk is untouched.
        let plan = build_plan(&inventory, &[SourceTool::Hermes], &resolver);
        let dry = apply_plan(&plan, false);
        assert_eq!(dry.created, 1);
        assert!(
            !hermes_dir.join("a/SKILL.md").exists(),
            "dry-run never writes"
        );

        // 2) Real apply: file lands with managed header.
        let real = apply_plan(&plan, true);
        assert_eq!(real.created, 1);
        assert_eq!(real.errors.len(), 0);
        let written = fs::read_to_string(hermes_dir.join("a/SKILL.md")).unwrap();
        assert!(
            written.contains("ALTEVRA_MANAGED: true"),
            "managed marker injected"
        );
        assert!(written.contains("v1 body"));

        // 3) Re-scan now sees the managed copy in Hermes; re-plan should be Skip
        //    (AlreadyInSync) because content matches what we'd render.
        let inventory2 = vec![
            make_skill("a", SourceTool::Claude, src.clone(), false),
            make_skill("a", SourceTool::Hermes, hermes_dir.join("a/SKILL.md"), true),
        ];
        let plan2 = build_plan(&inventory2, &[SourceTool::Hermes], &resolver);
        assert_eq!(plan2.creates(), 0);
        assert_eq!(plan2.refreshes(), 0);
        assert_eq!(plan2.skips(), 1);
        if let SyncAction::Skip { reason, .. } = &plan2.actions[0] {
            assert!(matches!(reason, SkipReason::AlreadyInSync));
        }

        // 4) Source changed (v2) → re-plan emits Refresh; apply updates the file.
        fs::write(&src, "---\nname: a\nversion: 2.0.0\n---\nv2 body\n").unwrap();
        let plan3 = build_plan(&inventory2, &[SourceTool::Hermes], &resolver);
        assert_eq!(plan3.refreshes(), 1, "drift detected -> refresh");
        let real3 = apply_plan(&plan3, true);
        assert_eq!(real3.refreshed, 1);
        let updated = fs::read_to_string(hermes_dir.join("a/SKILL.md")).unwrap();
        assert!(updated.contains("v2 body"));
    }

    #[test]
    fn pick_source_prefers_user_authored_then_canonical_order() {
        // When the same slug is both user-authored and managed, prefer the user one.
        let s1 = make_skill("x", SourceTool::Claude, PathBuf::from("/c"), true); // managed
        let s2 = make_skill("x", SourceTool::Hermes, PathBuf::from("/h"), false); // user-authored
        let pick = pick_source(&[&s1, &s2]);
        assert_eq!(pick.source_tool, SourceTool::Hermes);
    }
}
