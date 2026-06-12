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
use chrono::{DateTime, Local, Utc};
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
    /// E1 — today/tomorrow calendar events pulled by the ICS connector.
    pub calendar: Vec<String>,
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
        self.calendar.len()
            + self.what_changed.len()
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
    // Use LOCAL timezone for the date stamp — Pavle's vault dates must match
    // his wall-clock day, not UTC (which can differ by hours at night).
    let local_date = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
    let mut data = BriefData {
        date: local_date,
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

    // --- What Changed: wire recent sessions + file_changes from DB so the
    // section renders non-empty when data exists (R6 brief polish).
    // The delivery already contributes via RULE_RESUME_BRIEF; supplement with
    // the last few distinct projects/sessions and recent file edits.
    let recent_changes = recent_changes_lines(pool, now).await;
    for line_str in recent_changes {
        if !data.what_changed.contains(&line_str) {
            data.what_changed.push(line_str);
        }
    }

    // --- Decisions: wire object_index decisions from DB (supplement delivery).
    // Delivery contributes via RULE_DECISION_STALENESS (past review_after date);
    // here we also surface recent decisions that are still current.
    let recent_decisions = recent_decision_lines(pool).await;
    for line_str in recent_decisions {
        if !data.decisions.contains(&line_str) {
            data.decisions.push(line_str);
        }
    }

    // --- Tasks: wire active tasks from DB (supplement delivery).
    // Delivery contributes via RULE_OPEN_PROPOSALS; here we directly surface
    // active tasks (not proposals) so the Tasks section renders when tasks exist.
    let active_tasks = active_task_lines(pool).await;
    for line_str in active_tasks {
        if !data.tasks.contains(&line_str) {
            data.tasks.push(line_str);
        }
    }

    // Patterns over recent events → What Matters / Risks.
    let (matters, risks) = pattern_lines(pool, now).await;
    data.what_matters.extend(matters);
    data.risks.extend(risks);

    // Gate-filtered research — only items that matched a project or a stated
    // interest at fetch time (relevance_score / project_matches recorded),
    // re-checked against the CURRENT relevance gate at selection time (an
    // interest removed since fetch stops surfacing immediately).
    data.research = research_lines(pool, gate, 5).await;

    // E1 — Calendar: today/tomorrow events the ICS connector pulled (sourced
    // from connector_synced events whose entity_type = calendar_event).
    data.calendar = calendar_lines(pool, now).await;

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

/// E1: Calendar — today/tomorrow events the ICS connector ingested. Sourced
/// from `events` rows (event_type = connector_synced, entity_type =
/// calendar_event); the source START time lives in the JSON payload `ts`. Only
/// events whose start date is today or tomorrow (relative to `now`) are shown,
/// sorted chronologically. Fail-soft: DB/parse errors → empty.
async fn calendar_lines(pool: &SqlitePool, now: DateTime<Utc>) -> Vec<String> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT title, payload FROM events \
         WHERE event_type = 'connector_synced' AND entity_type = 'calendar_event' \
         ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let today = now.date_naive();
    let tomorrow = today.succ_opt().unwrap_or(today);
    let mut dated: Vec<(DateTime<Utc>, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (title, payload) in rows {
        let ts = payload
            .as_deref()
            .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
            .and_then(|v| v.get("ts").and_then(|t| t.as_str().map(String::from)))
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc));
        let Some(start) = ts else { continue };
        let d = start.date_naive();
        if d != today && d != tomorrow {
            continue;
        }
        // dedup by title+time (re-sync writes a fresh event row each pass).
        let key = format!("{}-{title}", start.to_rfc3339());
        if !seen.insert(key) {
            continue;
        }
        let label = if d == today { "today" } else { "tomorrow" };
        dated.push((start, format!("{} {} — {title}", label, start.format("%H:%M"))));
    }
    dated.sort_by_key(|(t, _)| *t);
    dated.into_iter().map(|(_, l)| l).collect()
}

/// R6: What-Changed supplement — recent sessions (last 3 days) and
/// recent file_changes (last 24h, up to 5 distinct paths) from DB.
/// Fail-soft: DB errors → empty vec.
async fn recent_changes_lines(pool: &SqlitePool, now: DateTime<Utc>) -> Vec<String> {
    use altevra_db::SessionsRepository;
    let mut lines = Vec::new();

    // Recent sessions from the last 3 days with a known project.
    let window = now - chrono::Duration::days(3);
    let sessions = SessionsRepository::new(pool)
        .list_sessions(None, None, 20)
        .await
        .unwrap_or_default();
    let mut seen_projects = std::collections::HashSet::new();
    for s in &sessions {
        if s.started_at < window {
            break;
        }
        if let Some(proj) = &s.project_name {
            if seen_projects.insert(proj.clone()) {
                let summary = s
                    .summary
                    .as_deref()
                    .map(|x| x.chars().take(80).collect::<String>())
                    .unwrap_or_else(|| format!("{} turns", s.turn_count));
                lines.push(format!("{proj}: {summary}"));
                if lines.len() >= 3 {
                    break;
                }
            }
        }
    }

    // Recent file_changes (last 24h) — distinct paths, most-recent 5.
    let fc_window = now - chrono::Duration::hours(24);
    let fc_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT path, diff_summary FROM file_changes \
         WHERE created_at >= ? \
         ORDER BY created_at DESC LIMIT 30",
    )
    .bind(fc_window.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut seen_paths = std::collections::HashSet::new();
    let mut fc_lines: Vec<String> = Vec::new();
    for (path, diff) in fc_rows {
        if seen_paths.insert(path.clone()) {
            let short_path = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let line = if diff.trim().is_empty() {
                format!("edited: {short_path}")
            } else {
                let short_diff: String = diff.trim().chars().take(60).collect();
                format!("{short_path}: {short_diff}")
            };
            fc_lines.push(line);
            if fc_lines.len() >= 5 {
                break;
            }
        }
    }
    lines.extend(fc_lines);
    lines
}

/// R6: Decisions supplement — recent active decisions from `object_index`
/// (type = 'decision', status = 'active'), up to 5 most-recent.
async fn recent_decision_lines(pool: &SqlitePool) -> Vec<String> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT oi.title, d.rationale \
         FROM object_index oi \
         LEFT JOIN decisions d ON d.id = oi.id \
         WHERE oi.type = 'decision' AND oi.status = 'active' \
           AND oi.redaction_status IN ('clean','redacted') \
         ORDER BY oi.updated_at DESC LIMIT 5",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|(title, _)| {
            if title.trim().is_empty() {
                None
            } else {
                Some(title)
            }
        })
        .collect()
}

/// R6: Tasks supplement — active (non-completed, non-cancelled) tasks from
/// `tasks` table, ordered by priority then due date, up to 5.
async fn active_task_lines(pool: &SqlitePool) -> Vec<String> {
    use altevra_db::TasksRepository;
    let tasks = TasksRepository::new(pool)
        .list_active(None, 5)
        .await
        .unwrap_or_default();
    tasks
        .into_iter()
        .map(|t| {
            if let Some(due) = t.due_at {
                format!(
                    "{} [{}] (due {})",
                    t.title,
                    t.priority,
                    due.format("%Y-%m-%d")
                )
            } else {
                format!("{} [{}]", t.title, t.priority)
            }
        })
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

    // E1 — Calendar section, rendered ONLY when there are events today/tomorrow
    // (the other sections render even when empty; Calendar is additive signal).
    if !data.calendar.is_empty() {
        section("Calendar", &data.calendar);
    }
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
    // LOCAL date for the vault filename — matches Pavle's wall-clock day.
    let date = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
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

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{create_pool, run_migrations};
    use altevra_research::RelevanceGate;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn setup_pool(dir: &TempDir) -> sqlx::SqlitePool {
        let db = dir.path().join("brief_test.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    // Seed fixture data into the DB so every brief section has something to
    // render. We insert: a session with project + summary (What Changed),
    // a decision in object_index (Decisions), an active task (Tasks).
    async fn seed_fixtures(pool: &sqlx::SqlitePool, now: DateTime<Utc>) {
        use uuid::Uuid;

        // --- Session (What Changed via recent_changes_lines)
        sqlx::query(
            "INSERT INTO sessions \
             (id, tool, started_at, ended_at, summary, project_name, \
              turn_count, tokens_in_total, tokens_out_total, cost_usd_estimate, metadata) \
             VALUES (?, 'claude-code', ?, ?, ?, ?, 5, 100, 200, 0.01, '{}')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(
            (now - chrono::Duration::hours(2))
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
        )
        .bind(now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .bind("fixed the auth flow")
        .bind("altevra")
        .execute(pool)
        .await
        .unwrap();

        // --- Decision in decisions + object_index (Decisions section)
        let dec_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO decisions (id, title, rationale, decided_at, metadata) \
             VALUES (?, ?, ?, ?, '{}')",
        )
        .bind(&dec_id)
        .bind("Use SQLite as the single store")
        .bind("Simplest self-contained option; no separate server needed")
        .bind(now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT OR REPLACE INTO object_index \
             (type, id, status, sensitivity, domain, title, categories, tags, \
              redaction_status, updated_at) \
             VALUES ('decision', ?, 'active', 'internal', 'business', \
                     'Use SQLite as the single store', '[]', '[]', 'clean', ?)",
        )
        .bind(&dec_id)
        .bind(now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .execute(pool)
        .await
        .unwrap();

        // --- Active task (Tasks section)
        sqlx::query(
            "INSERT INTO tasks \
             (id, title, status, priority, metadata, created_at, updated_at) \
             VALUES (?, ?, 'in_progress', 'high', '{}', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind("Ship R6 doctor checks")
        .bind(now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .bind(now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .execute(pool)
        .await
        .unwrap();
    }

    /// R6 fixture test: when DB has sessions, decisions, and tasks,
    /// `render_brief` produces non-empty What Changed, Decisions, and Tasks
    /// sections.
    #[tokio::test]
    async fn brief_renders_all_sections_with_fixture_data() {
        let tmp = TempDir::new().unwrap();
        let pool = setup_pool(&tmp).await;
        let now = Utc::now();
        seed_fixtures(&pool, now).await;

        let gate = RelevanceGate::default();
        let delivery = super::Delivery {
            obsidian: vec![],
            obsidian_blocked: vec![],
            ..Default::default()
        };
        let data = build_brief_data(&pool, &delivery, &gate, now).await;
        let rendered = render_brief(&data, false);

        // Local date is in frontmatter.
        let local_date = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        assert!(
            rendered.contains(&format!("date: {local_date}")),
            "frontmatter must use LOCAL date: {local_date}, got:\n{rendered}"
        );
        assert!(
            rendered.contains("# Daily Brief"),
            "must have title, got:\n{rendered}"
        );

        // What Changed: session with project name.
        assert!(
            rendered.contains("altevra"),
            "What Changed should mention the project 'altevra', got:\n{rendered}"
        );

        // Decisions: the seeded decision.
        assert!(
            rendered.contains("Use SQLite as the single store"),
            "Decisions section must contain seeded decision, got:\n{rendered}"
        );

        // Tasks: the seeded task.
        assert!(
            rendered.contains("Ship R6 doctor checks"),
            "Tasks section must contain seeded task, got:\n{rendered}"
        );

        // All major section headers present (even if empty).
        for header in &[
            "## What Changed",
            "## What Matters",
            "## Decisions",
            "## Tasks Needing Attention",
            "## Useful Research",
            "## Risks",
            "## Suggested Focus",
        ] {
            assert!(
                rendered.contains(header),
                "section header '{header}' missing from rendered brief:\n{rendered}"
            );
        }
    }

    #[tokio::test]
    async fn brief_local_date_in_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let pool = setup_pool(&tmp).await;
        // Use a UTC "now" that is likely to differ from local date near midnight.
        let now = Utc::now();
        let gate = RelevanceGate::default();
        let delivery = super::Delivery {
            obsidian: vec![],
            obsidian_blocked: vec![],
            ..Default::default()
        };
        let data = build_brief_data(&pool, &delivery, &gate, now).await;
        let local_date = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        assert_eq!(
            data.date, local_date,
            "BriefData.date must be local-tz date"
        );
    }

    /// E1 fixture test: an ICS event ingested through the connector path feeds a
    /// Calendar section in the brief. Drives the REAL ingest + the REAL
    /// `calendar_lines` query — no hand-inserted events.
    #[tokio::test]
    async fn brief_calendar_section_from_ingested_ics() {
        use altevra_adapters::connectors::{ingest_items, IcsConnector, Connector, ConnectorCtx};
        use altevra_adapters::connectors::config::ConnectorConfig;
        use chrono::TimeZone;
        use std::collections::BTreeMap;

        let tmp = TempDir::new().unwrap();
        let pool = setup_pool(&tmp).await;
        let now = Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap();

        // A real ICS file with a today event + a far-future event.
        let ics = tmp.path().join("cal.ics");
        std::fs::write(
            &ics,
            "BEGIN:VEVENT\r\nUID:e-today\r\nSUMMARY:Investor call\r\nDTSTART:20260612T150000Z\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:e-far\r\nSUMMARY:Later\r\nDTSTART:20270101T100000Z\r\nEND:VEVENT\r\n",
        )
        .unwrap();

        let mut params = BTreeMap::new();
        params.insert("path".to_string(), ics.to_str().unwrap().to_string());
        let ctx = ConnectorCtx {
            config: ConnectorConfig {
                enabled: true,
                auth_secret: String::new(),
                cadence_minutes: 60,
                domain: None,
                params,
            },
            auth_value: None,
            now,
        };
        let items = IcsConnector::new().pull(&ctx).unwrap();
        ingest_items(&pool, "ics", &items).await.unwrap();

        let gate = RelevanceGate::default();
        let delivery = super::Delivery {
            obsidian: vec![],
            obsidian_blocked: vec![],
            ..Default::default()
        };
        let data = build_brief_data(&pool, &delivery, &gate, now).await;
        assert!(
            data.calendar.iter().any(|l| l.contains("Investor call")),
            "today's event must appear in calendar: {:?}",
            data.calendar
        );
        assert!(
            !data.calendar.iter().any(|l| l.contains("Later")),
            "far-future event must NOT appear: {:?}",
            data.calendar
        );

        let rendered = render_brief(&data, false);
        assert!(rendered.contains("## Calendar"), "Calendar section must render:\n{rendered}");
        assert!(rendered.contains("Investor call"));
    }
}
