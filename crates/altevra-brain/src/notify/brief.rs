//! Daily brief rendering (P4) — output matches `00-system/schemas/
//! daily_briefing_v1.md`: section headers verbatim, sections-with-no-signal
//! present but empty, Personal Signals omitted entirely when nothing allowed.
//!
//! Two render modes:
//!   * **gated** (`private = false`) — what lands in the (syncable) Obsidian
//!     vault. Relationship/policy-blocked items appear ONLY as a count +
//!     `altevra brief --private` pointer; never a name.
//!   * **private** (`private = true`) — terminal-only full version including
//!     the policy-blocked items verbatim.

use altevra_research::RelevanceGate;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

use super::delivery::Delivery;
use super::types::{
    NotifyItem, RULE_DECISION_STALENESS, RULE_OPEN_PROPOSALS, RULE_RESUME_BRIEF,
};

/// Assembled brief content, grouped into daily_briefing_v1 sections.
#[derive(Debug, Default)]
pub struct BriefData {
    pub date: String,
    pub what_changed: Vec<String>,
    pub what_matters: Vec<String>,
    pub decisions: Vec<String>,
    pub tasks: Vec<String>,
    pub research: Vec<String>,
    pub risks: Vec<String>,
    /// Full personal lines (policy-blocked from Obsidian) — PRIVATE render only.
    pub personal_private: Vec<String>,
    /// How many personal signals were withheld from the gated render.
    pub personal_withheld: usize,
    pub focus: Vec<String>,
}

impl BriefData {
    fn signal_count(&self) -> usize {
        self.what_changed.len()
            + self.what_matters.len()
            + self.decisions.len()
            + self.tasks.len()
            + self.research.len()
            + self.risks.len()
            + self.personal_private.len()
    }
}

/// Build the section data from a routed [`Delivery`] plus pattern/research
/// context. `obsidian`-side items fill the policy-cleared sections; the
/// `obsidian_blocked` items become `personal_private` + the withheld count.
pub async fn build_brief_data(
    pool: &SqlitePool,
    delivery: &Delivery,
    gate: &RelevanceGate,
    now: DateTime<Utc>,
) -> BriefData {
    let mut data = BriefData {
        date: now.format("%Y-%m-%d").to_string(),
        ..Default::default()
    };

    let line = |i: &NotifyItem| {
        if i.body.trim().is_empty() {
            i.title.clone()
        } else {
            format!("{} — {}", i.title, i.body)
        }
    };

    for item in &delivery.obsidian {
        match item.rule.as_str() {
            RULE_RESUME_BRIEF => data.what_changed.push(line(item)),
            RULE_DECISION_STALENESS => data.decisions.push(item.title.clone()),
            RULE_OPEN_PROPOSALS => data.tasks.push(line(item)),
            _ => data.what_matters.push(line(item)),
        }
    }
    for item in &delivery.obsidian_blocked {
        data.personal_private.push(line(item));
    }
    data.personal_withheld = delivery.obsidian_blocked.len();

    // Patterns over recent events → What Matters / Risks.
    let (matters, risks) = pattern_lines(pool, now).await;
    data.what_matters.extend(matters);
    data.risks.extend(risks);

    // Gate-filtered research — only items that matched a project or a stated
    // interest at fetch time (relevance_score / project_matches recorded),
    // re-checked against the CURRENT relevance gate at selection time (an
    // interest removed since fetch stops surfacing immediately).
    data.research = research_lines(pool, gate, 5).await;

    // Suggested Focus — deterministic top-3 derived from the sections.
    if let Some(d) = data.decisions.first() {
        data.focus.push(format!("re-check: {d}"));
    }
    if let Some(t) = data.tasks.first() {
        data.focus.push(format!("review queue: {t}"));
    }
    if data.personal_withheld > 0 {
        data.focus.push(format!(
            "{} overdue reach-out(s) — `altevra brief --private`",
            data.personal_withheld
        ));
    }
    data.focus.truncate(3);
    data
}

/// Detected patterns split into What Matters (medium) vs Risks (high+).
async fn pattern_lines(pool: &SqlitePool, now: DateTime<Utc>) -> (Vec<String>, Vec<String>) {
    use altevra_core::detect_patterns;
    use altevra_core::updates::Importance;
    use altevra_db::EventsRepository;

    let since = now - chrono::Duration::days(30);
    let events = EventsRepository::new(pool)
        .list_since(since, None, 2000)
        .await
        .unwrap_or_default();
    let insights = detect_patterns(&events, &[]);
    let mut matters = Vec::new();
    let mut risks = Vec::new();
    for i in &insights {
        match i.importance {
            Importance::Critical | Importance::High => risks.push(i.title.clone()),
            Importance::Medium => matters.push(i.title.clone()),
            _ => {}
        }
    }
    matters.sort();
    risks.sort();
    (matters, risks)
}

/// Recent relevance-gated research items. Two gates stack: the fetch-time
/// score/project filter (recorded columns) and the CURRENT relevance gate —
/// an active gate drops any item whose title+summary matches no stated
/// interest/goal at selection time (P4 §4: briefing item selection).
async fn research_lines(pool: &SqlitePool, gate: &RelevanceGate, limit: i64) -> Vec<String> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT title, link, summary, project_matches_json FROM research_items \
         WHERE relevance_score >= 0.4 OR project_matches_json != '[]' \
         ORDER BY published_at DESC LIMIT ?",
    )
    .bind(limit * 4)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .filter(|(title, _link, summary, matches_json)| {
            if !gate.is_active() {
                return true;
            }
            // A recorded project match passes (projects ARE active work).
            let has_project_match = matches_json.trim() != "[]" && !matches_json.trim().is_empty();
            has_project_match || gate.matching_interest(&format!("{title} {summary}")).is_some()
        })
        .take(limit as usize)
        .map(|(title, link, _, _)| format!("{title} — {link}"))
        .collect()
}

/// Render the brief per daily_briefing_v1. Headers verbatim; empty sections
/// stay present (empty); Personal Signals omitted when nothing to say.
pub fn render_brief(data: &BriefData, private: bool) -> String {
    let confidence = match data.signal_count() {
        0..=1 => "low",
        2..=5 => "medium",
        _ => "high",
    };
    let mut out = format!(
        "---\nkind: altevra-daily-brief\ngenerated_by: altevra-brain\ndate: {date}\nmode: daily_briefing\nschema_version: 1\nconfidence: {confidence}\n---\n\n# Daily Brief — {date}\n",
        date = data.date,
    );

    let mut section = |name: &str, lines: &[String]| {
        out.push_str(&format!("\n## {name}\n\n"));
        for l in lines {
            out.push_str(&format!("- {l}\n"));
        }
    };

    section("What Changed", &data.what_changed);
    section("What Matters", &data.what_matters);
    section("Decisions", &data.decisions);
    section("Tasks Needing Attention", &data.tasks);
    section("Useful Research", &data.research);
    section("Risks", &data.risks);

    // Personal Signals — omitted entirely when there is nothing allowed.
    if private {
        if !data.personal_private.is_empty() {
            section("Personal Signals", &data.personal_private);
        }
    } else if data.personal_withheld > 0 {
        // Gated render: COUNT + CLI pointer only — never names in a
        // syncable vault (dp_relationship obsidian_mirror = never).
        let pointer = vec![format!(
            "{} private signal(s) withheld by domain policy — view: `altevra brief --private`",
            data.personal_withheld
        )];
        section("Personal Signals", &pointer);
    }

    section("Suggested Focus", &data.focus);
    out
}

/// P4 brief delivery: collect sources → route through delivery (O_EXCL dedup
/// when `claim`, per-item obsidian_mirror gate, FAIL-CLOSED) → render the
/// **gated** brief → write `<vault>/Daily/YYYY-MM-DD-altevra-brief.md`.
///
/// Returns `Ok(None)` when today's brief already exists (idempotent — a
/// re-run never double-claims or rewrites). Only the GATED render ever
/// touches the vault; the private render is terminal-only
/// (`altevra brief --private`).
pub async fn write_vault_brief(
    pool: &SqlitePool,
    vault: &Path,
    claims_dir: &Path,
    claim: bool,
    gate: &RelevanceGate,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<PathBuf>> {
    let date = now.format("%Y-%m-%d").to_string();
    let dir = vault.join("Daily");
    let path = dir.join(format!("{date}-altevra-brief.md"));
    if path.exists() {
        return Ok(None);
    }

    let items = super::sources::collect_all(pool, vault, now).await;
    let cfg = super::delivery::DeliveryConfig {
        claims_dir: claims_dir.to_path_buf(),
        claim,
    };
    let delivery = super::delivery::deliver(pool, &cfg, items, now).await?;
    let data = build_brief_data(pool, &delivery, gate, now).await;

    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, render_brief(&data, false))?;
    Ok(Some(path))
}
