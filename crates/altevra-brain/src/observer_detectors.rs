//! R4 — DB-backed observer detectors.
//!
//! Each detector queries `sessions`/`turns`/`hook_runs` (or `audit_log`) directly
//! rather than only consuming pre-emitted events. This means detectors fire on
//! the real corpus that exists from day one, not just the subset that was
//! accompanied by an event emission.
//!
//! Detectors produce [`DbInsight`] structs that carry **metadata-only evidence**:
//! turn / session IDs as string refs + counts. The turn *body* is NEVER
//! included — turns already hold the guarded payload, and a metadata-only
//! pointer is sufficient for the downstream consumer (proposal writer, daily
//! briefing, CLI display).
//!
//! ## Detector catalogue
//!
//! | Detector                | Source tables              | Signal                                            |
//! |-------------------------|----------------------------|---------------------------------------------------|
//! | `working_dir_drift`     | sessions                   | ≥ DRIFT_SESSION_THRESHOLD distinct working_dirs in window |
//! | `stale_project`         | sessions                   | project (by working_dir/project_name) silent > STALE_PROJECT_DAYS |
//! | `repeated_tool_failure` | turns                      | ≥ TOOL_FAILURE_THRESHOLD error turns for the same tool_name |
//! | `late_night_session`    | sessions                   | session started ≥ LATE_NIGHT_HOUR local time + long duration |
//! | `hook_failure_spike`    | hook_runs (audit_log proxy)| ≥ HOOK_FAILURE_THRESHOLD failed hook_runs in window |
//!
//! ## Thresholds (high-precision-or-silent)
//!
//! All thresholds are deliberately high so we emit an insight only when the
//! pattern is unmistakable. A detector that fires on noise destroys trust in
//! the observer.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Thresholds — tunable
// ---------------------------------------------------------------------------

/// ≥ this many distinct `working_dir` values in the last DRIFT_WINDOW_DAYS → insight.
pub const DRIFT_SESSION_THRESHOLD: usize = 3;
/// Window for working-dir drift detection.
pub const DRIFT_WINDOW_DAYS: i64 = 7;

/// Days since the last session for a project (keyed by working_dir+project_name) → stale.
pub const STALE_PROJECT_DAYS: i64 = 14;
/// Minimum number of historical sessions for a project before it can be flagged stale.
/// (Projects with <2 sessions have no meaningful "activity" history to go stale from.)
pub const STALE_PROJECT_MIN_SESSIONS: i64 = 2;

/// ≥ this many tool_result turns whose content starts with or contains an error
/// marker, for the same tool_name, in TOOL_FAILURE_WINDOW_DAYS → insight.
pub const TOOL_FAILURE_THRESHOLD: usize = 3;
/// Window for repeated tool_failure detection.
pub const TOOL_FAILURE_WINDOW_DAYS: i64 = 3;

/// UTC hour at or after midnight that is still considered "late night".
/// Sessions starting from hour LATE_NIGHT_HOUR_START up to LATE_NIGHT_HOUR_END
/// (exclusive) are checked. For UTC+2 (Pavle): 00:00-06:00 UTC ≈ 02:00-08:00 local;
/// the "3am" pattern is fully captured within this window.
pub const LATE_NIGHT_HOUR_START: u32 = 0; // midnight UTC
pub const LATE_NIGHT_HOUR_END: u32 = 6; // 06:00 UTC = 08:00 local (UTC+2)
/// Minimum session duration (minutes) to flag as a late-night long session.
pub const LATE_NIGHT_MIN_DURATION_MINUTES: i64 = 60;
/// How many qualifying late-night long sessions in LATE_NIGHT_WINDOW_DAYS before
/// the detector fires (≥ 1 → always fires; kept at 1 to always surface the "3am" pattern).
pub const LATE_NIGHT_WINDOW_DAYS: i64 = 7;
/// Maximum number of late-night sessions to surface in a single insight
/// (keeps the evidence list manageable).
pub const LATE_NIGHT_EVIDENCE_CAP: usize = 5;

/// ≥ this many failed hook_run rows in HOOK_FAILURE_WINDOW_HOURS → insight.
pub const HOOK_FAILURE_THRESHOLD: usize = 3;
/// Window for hook-failure spike detection.
pub const HOOK_FAILURE_WINDOW_HOURS: i64 = 24;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A piece of metadata-only evidence for a DB-backed insight.
/// **Invariant:** body / content fields MUST NOT be populated here — use
/// `turn_count` and `session_refs` (UUID strings) only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbEvidenceRef {
    /// Human-readable label ("session:<uuid>", "turn-count:N", "hook:<slug>").
    pub label: String,
    /// Optional single turn / session id.
    pub id: Option<String>,
    /// Timestamp of the evidence anchor point.
    pub at: Option<DateTime<Utc>>,
}

impl DbEvidenceRef {
    pub fn session(id: impl Into<String>, at: DateTime<Utc>) -> Self {
        let id = id.into();
        Self {
            label: format!("session:{id}"),
            id: Some(id),
            at: Some(at),
        }
    }

    pub fn count(n: usize, label: impl Into<String>) -> Self {
        Self {
            label: format!("{}: {n}", label.into()),
            id: None,
            at: None,
        }
    }

    pub fn hook(slug: impl Into<String>, at: DateTime<Utc>) -> Self {
        let slug = slug.into();
        Self {
            label: format!("hook:{slug}"),
            id: Some(slug),
            at: Some(at),
        }
    }
}

/// Severity label for a DB-backed insight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbInsightSeverity {
    High,
    Medium,
    Low,
}

impl std::fmt::Display for DbInsightSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::High => write!(f, "HIGH"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::Low => write!(f, "LOW"),
        }
    }
}

/// Kind tag for a DB-backed insight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbInsightKind {
    WorkingDirDrift,
    StaleProject,
    RepeatedToolFailure,
    LateNightSession,
    HookFailureSpike,
}

impl std::fmt::Display for DbInsightKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}").to_lowercase());
        write!(f, "{s}")
    }
}

/// An insight produced by a DB-backed detector. Metadata-only evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbInsight {
    pub id: Uuid,
    pub kind: DbInsightKind,
    pub severity: DbInsightSeverity,
    pub title: String,
    pub summary: String,
    pub recommended_action: Option<String>,
    /// Metadata-only pointers: session/turn UUIDs + counts. NEVER body text.
    pub evidence: Vec<DbEvidenceRef>,
    pub generated_at: DateTime<Utc>,
}

impl DbInsight {
    pub fn new(
        kind: DbInsightKind,
        severity: DbInsightSeverity,
        title: impl Into<String>,
        summary: impl Into<String>,
        recommended_action: Option<String>,
        evidence: Vec<DbEvidenceRef>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            severity,
            title: title.into(),
            summary: summary.into(),
            recommended_action,
            evidence,
            generated_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// DB-backed detector: working-dir drift
// ---------------------------------------------------------------------------

/// Detect excessive working-dir switching across sessions in the last
/// `DRIFT_WINDOW_DAYS`. A developer ping-ponging between ≥ 3 distinct
/// project roots in one week usually indicates fragmented focus or a
/// stuck branch situation.
///
/// Evidence: list of session UUIDs (no body). Fires iff ≥ DRIFT_SESSION_THRESHOLD
/// distinct `working_dir` values appear among sessions that HAVE a non-null
/// working_dir in the window.
pub async fn detect_working_dir_drift(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<DbInsight>> {
    let cutoff = (now - Duration::days(DRIFT_WINDOW_DAYS)).to_rfc3339();

    // Distinct working_dirs and one representative session per dir.
    let rows = sqlx::query(
        r#"SELECT working_dir, COUNT(*) AS cnt, MAX(started_at) AS last_started, MAX(id) AS sample_id
           FROM sessions
           WHERE started_at > ?
             AND working_dir IS NOT NULL
             AND working_dir != ''
           GROUP BY working_dir
           ORDER BY last_started DESC"#,
    )
    .bind(&cutoff)
    .fetch_all(pool)
    .await?;

    if rows.len() < DRIFT_SESSION_THRESHOLD {
        return Ok(vec![]);
    }

    let dirs: Vec<String> = rows
        .iter()
        .map(|r| r.get::<String, _>("working_dir"))
        .collect();

    let mut evidence: Vec<DbEvidenceRef> = vec![DbEvidenceRef::count(
        rows.len(),
        "distinct_working_dirs",
    )];
    // Include up to 5 session refs (the most recent per dir).
    for r in rows.iter().take(5) {
        let sid: String = r.get("sample_id");
        let ts: String = r.get("last_started");
        if let Ok(at) = DateTime::parse_from_rfc3339(&ts) {
            evidence.push(DbEvidenceRef::session(&sid, at.with_timezone(&Utc)));
        }
    }

    let short_dirs: Vec<String> = dirs
        .iter()
        .take(3)
        .map(|d| {
            // Show just the last 2 path components for readability.
            let p = std::path::Path::new(d);
            let comps: Vec<&str> = p
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            comps.iter().rev().take(2).rev().cloned().collect::<Vec<_>>().join("/")
        })
        .collect();

    let title = format!(
        "Working-dir drift: {} distinct project roots in last {}d",
        rows.len(),
        DRIFT_WINDOW_DAYS
    );
    let summary = format!(
        "{} distinct working directories across sessions in the last {} days. \
         Sample dirs: {}{}. Fragmented focus or a stale branch that keeps pulling \
         you back.",
        rows.len(),
        DRIFT_WINDOW_DAYS,
        short_dirs.join(", "),
        if dirs.len() > 3 { " …" } else { "" },
    );

    Ok(vec![DbInsight::new(
        DbInsightKind::WorkingDirDrift,
        DbInsightSeverity::Medium,
        title,
        summary,
        Some(
            "Review open branches per project; close or park anything not in active focus."
                .to_string(),
        ),
        evidence,
    )])
}

// ---------------------------------------------------------------------------
// DB-backed detector: stale project
// ---------------------------------------------------------------------------

/// Detect projects (keyed by `working_dir` + `project_name`) that had
/// historical activity but have gone quiet for ≥ STALE_PROJECT_DAYS.
///
/// Keyed by `working_dir`/`project_name` (not `project_id`) because hook
/// sessions do not have a resolved project_id — only the directory + name
/// are reliably populated.
pub async fn detect_stale_projects(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<DbInsight>> {
    let stale_cutoff = (now - Duration::days(STALE_PROJECT_DAYS)).to_rfc3339();

    // One row per project key: most recent session timestamp + total count.
    let rows = sqlx::query(
        r#"SELECT
               COALESCE(working_dir, '') AS project_key,
               project_name,
               MAX(started_at) AS last_started,
               COUNT(*) AS session_count
           FROM sessions
           GROUP BY COALESCE(working_dir, ''), project_name
           HAVING last_started < ?
              AND session_count >= ?
           ORDER BY last_started ASC
           LIMIT 20"#,
    )
    .bind(&stale_cutoff)
    .bind(STALE_PROJECT_MIN_SESSIONS)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(vec![]);
    }

    let mut insights = Vec::new();
    for r in rows {
        let project_key: String = r.get("project_key");
        let project_name: Option<String> = r.get("project_name");
        let last_started: String = r.get("last_started");
        let session_count: i64 = r.get("session_count");

        let last_at = match DateTime::parse_from_rfc3339(&last_started) {
            Ok(t) => t.with_timezone(&Utc),
            Err(_) => continue,
        };
        let days_silent = (now - last_at).num_days();

        let label = project_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                // Use last component of working_dir as fallback.
                std::path::Path::new(&project_key)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("unknown");

        let title = format!("Stale project: '{label}' silent for {days_silent}d");
        let summary = format!(
            "Project '{label}' (dir: {project_key}) had {session_count} session(s) but none in \
             the last {days_silent} days (threshold {}d). Last session at {}.",
            STALE_PROJECT_DAYS,
            last_at.format("%Y-%m-%d %H:%M UTC"),
        );
        let evidence = vec![
            DbEvidenceRef::count(session_count as usize, "total_sessions"),
            DbEvidenceRef {
                label: format!("last_session_at:{}", last_at.to_rfc3339()),
                id: None,
                at: Some(last_at),
            },
        ];

        insights.push(DbInsight::new(
            DbInsightKind::StaleProject,
            DbInsightSeverity::Low,
            title,
            summary,
            Some(format!(
                "Either archive '{label}' or schedule a review session."
            )),
            evidence,
        ));
    }
    Ok(insights)
}

// ---------------------------------------------------------------------------
// DB-backed detector: repeated tool failure
// ---------------------------------------------------------------------------

/// Error markers in tool_result turns. We check the START of the `content`
/// field (length-only query is not possible here without reading the
/// first N bytes). We keep the content scan to just the first 200 bytes by
/// using SQLite's `SUBSTR` so we never pull large bodies out of the DB.
///
/// Recognised error prefixes (case-insensitive, first 200 chars):
///   - "error:"  — most tool errors
///   - "failed"  — shell/bash errors
///   - "exception" — stack traces
///   - "traceback" — Python
///   - "stderr:" — captured stderr
///   - "command failed" — common CI pattern
const ERROR_MARKERS: &[&str] = &[
    "error:", "failed", "exception", "traceback", "stderr:", "command failed",
];

/// Detect ≥ TOOL_FAILURE_THRESHOLD error turns for the same `tool_name` in
/// the last TOOL_FAILURE_WINDOW_DAYS. Evidence is turn-id + count — never body.
pub async fn detect_repeated_tool_failures(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<DbInsight>> {
    let cutoff = (now - Duration::days(TOOL_FAILURE_WINDOW_DAYS)).to_rfc3339();

    // Build LIKE patterns for the error markers, applied to the first 200
    // chars of content (via SUBSTR). We use OR-joined conditions to keep the
    // query self-contained (no user input — safe string formatting).
    let like_clauses: Vec<String> = ERROR_MARKERS
        .iter()
        .map(|m| format!("LOWER(SUBSTR(t.content, 1, 200)) LIKE '{}%'", m))
        .collect();
    let where_markers = like_clauses.join(" OR ");

    let sql = format!(
        r#"SELECT t.tool_name,
                  COUNT(*) AS failure_count,
                  MAX(t.created_at) AS last_at,
                  MAX(t.id) AS sample_id
           FROM turns t
           WHERE t.role = 'tool_result'
             AND t.created_at > ?
             AND t.tool_name IS NOT NULL
             AND ({where_markers})
           GROUP BY t.tool_name
           HAVING failure_count >= ?
           ORDER BY failure_count DESC
           LIMIT 10"#
    );

    let rows = sqlx::query(&sql)
        .bind(&cutoff)
        .bind(TOOL_FAILURE_THRESHOLD as i64)
        .fetch_all(pool)
        .await?;

    if rows.is_empty() {
        return Ok(vec![]);
    }

    let mut insights = Vec::new();
    for r in rows {
        let tool_name: String = r.get("tool_name");
        let failure_count: i64 = r.get("failure_count");
        let last_at_str: String = r.get("last_at");
        let sample_id: String = r.get("sample_id");

        let last_at = DateTime::parse_from_rfc3339(&last_at_str)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or(now);

        let title = format!(
            "Repeated tool failure: '{}' — {} error(s) in last {}d",
            tool_name, failure_count, TOOL_FAILURE_WINDOW_DAYS
        );
        let summary = format!(
            "Tool '{}' produced {} error turn(s) in the last {} days. \
             Likely a config regression, missing dependency, or broken environment.",
            tool_name, failure_count, TOOL_FAILURE_WINDOW_DAYS
        );
        let evidence = vec![
            DbEvidenceRef::count(failure_count as usize, "error_turns"),
            DbEvidenceRef::session(&sample_id, last_at), // turn id, repurposed
        ];

        insights.push(DbInsight::new(
            DbInsightKind::RepeatedToolFailure,
            DbInsightSeverity::High,
            title,
            summary,
            Some(format!(
                "Inspect the last '{}' turn errors; run `altevra doctor` \
                 to check tool connectivity.",
                tool_name
            )),
            evidence,
        ));
    }
    Ok(insights)
}

// ---------------------------------------------------------------------------
// DB-backed detector: late-night long sessions
// ---------------------------------------------------------------------------

/// Detect sessions that started at or after LATE_NIGHT_HOUR (local time)
/// AND ran for ≥ LATE_NIGHT_MIN_DURATION_MINUTES. SQLite stores timestamps
/// as UTC text; we do the hour comparison using strftime in the query but
/// also validate Rust-side for the minute-duration gate.
pub async fn detect_late_night_sessions(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<DbInsight>> {
    let cutoff = (now - Duration::days(LATE_NIGHT_WINDOW_DAYS)).to_rfc3339();

    // Fetch sessions with both started_at and ended_at — only completed sessions
    // give us a reliable duration. "Late night" = UTC hour >= LATE_NIGHT_HOUR
    // (since we don't have the local timezone in the DB, we approximate with UTC;
    // for Pavle's timezone UTC+2, midnight local ≈ 22:00 UTC the prior day, so
    // this is a conservative approximation that the caller can tune via the const).
    // Hour range: [LATE_NIGHT_HOUR_START, LATE_NIGHT_HOUR_END) UTC.
    // SUBSTR(started_at, 12, 2) extracts the HH portion from the ISO-8601 string.
    let rows = sqlx::query(
        r#"SELECT id, started_at, ended_at
           FROM sessions
           WHERE started_at > ?
             AND ended_at IS NOT NULL
             AND CAST(SUBSTR(started_at, 12, 2) AS INTEGER) >= ?
             AND CAST(SUBSTR(started_at, 12, 2) AS INTEGER) < ?
           ORDER BY started_at DESC
           LIMIT 50"#,
    )
    .bind(&cutoff)
    .bind(LATE_NIGHT_HOUR_START)
    .bind(LATE_NIGHT_HOUR_END)
    .fetch_all(pool)
    .await?;

    // Filter Rust-side for duration >= threshold.
    let mut qualifying: Vec<(String, DateTime<Utc>, DateTime<Utc>, i64)> = Vec::new();
    for r in rows {
        let id: String = r.get("id");
        let started_str: String = r.get("started_at");
        let ended_str: String = r.get("ended_at");

        let Ok(started) = DateTime::parse_from_rfc3339(&started_str) else {
            continue;
        };
        let Ok(ended) = DateTime::parse_from_rfc3339(&ended_str) else {
            continue;
        };
        let duration_min = (ended.timestamp() - started.timestamp()) / 60;
        if duration_min < LATE_NIGHT_MIN_DURATION_MINUTES {
            continue;
        }
        qualifying.push((
            id,
            started.with_timezone(&Utc),
            ended.with_timezone(&Utc),
            duration_min,
        ));
    }

    if qualifying.is_empty() {
        return Ok(vec![]);
    }

    let count = qualifying.len();
    let mut evidence: Vec<DbEvidenceRef> =
        vec![DbEvidenceRef::count(count, "late_night_sessions")];
    for (id, started, _ended, dur) in qualifying.iter().take(LATE_NIGHT_EVIDENCE_CAP) {
        evidence.push(DbEvidenceRef {
            label: format!(
                "session:{id} started {}UTC dur={}min",
                started.format("%H:%M"),
                dur
            ),
            id: Some(id.clone()),
            at: Some(*started),
        });
    }

    let title = format!(
        "Late-night long sessions: {count} session(s) in last {}d",
        LATE_NIGHT_WINDOW_DAYS
    );
    let summary = format!(
        "{count} session(s) started between {:02}:00–{:02}:00 UTC with duration \
         ≥ {}min in the last {} days. \
         Pattern associated with next-day cognitive cost and error rate increase.",
        LATE_NIGHT_HOUR_START,
        LATE_NIGHT_HOUR_END,
        LATE_NIGHT_MIN_DURATION_MINUTES,
        LATE_NIGHT_WINDOW_DAYS,
    );

    Ok(vec![DbInsight::new(
        DbInsightKind::LateNightSession,
        DbInsightSeverity::Medium,
        title,
        summary,
        Some(
            "Consider scheduling deep coding before midnight; save review/planning tasks for post-midnight."
                .to_string(),
        ),
        evidence,
    )])
}

// ---------------------------------------------------------------------------
// DB-backed detector: hook failure spike
// ---------------------------------------------------------------------------

/// Detect ≥ HOOK_FAILURE_THRESHOLD hook_run rows with `success = 0` in the
/// last HOOK_FAILURE_WINDOW_HOURS. Groups by `hook_slug`; each slug with
/// enough failures emits one insight. Evidence: turn-id refs = hook_run UUIDs.
pub async fn detect_hook_failure_spike(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<DbInsight>> {
    let cutoff = (now - Duration::hours(HOOK_FAILURE_WINDOW_HOURS)).to_rfc3339();

    let rows = sqlx::query(
        r#"SELECT hook_slug,
                  COUNT(*) AS failure_count,
                  MAX(created_at) AS last_at,
                  MIN(error_message) AS sample_error
           FROM hook_runs
           WHERE success = 0
             AND created_at > ?
           GROUP BY hook_slug
           HAVING failure_count >= ?
           ORDER BY failure_count DESC
           LIMIT 10"#,
    )
    .bind(&cutoff)
    .bind(HOOK_FAILURE_THRESHOLD as i64)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(vec![]);
    }

    let mut insights = Vec::new();
    for r in rows {
        let hook_slug: String = r.get("hook_slug");
        let failure_count: i64 = r.get("failure_count");
        let last_at_str: String = r.get("last_at");
        let sample_error: Option<String> = r.get("sample_error");

        let last_at = DateTime::parse_from_rfc3339(&last_at_str)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or(now);

        let error_hint = sample_error
            .as_deref()
            .map(|e| format!(" (sample: '{}')", e.chars().take(80).collect::<String>()))
            .unwrap_or_default();

        let title = format!(
            "Hook failure spike: '{}' — {} failure(s) in last {}h",
            hook_slug, failure_count, HOOK_FAILURE_WINDOW_HOURS
        );
        let summary = format!(
            "Hook '{}' failed {} times in the last {}h{error_hint}. \
             Likely a config regression or missing dependency.",
            hook_slug, failure_count, HOOK_FAILURE_WINDOW_HOURS
        );
        let evidence = vec![
            DbEvidenceRef::count(failure_count as usize, "hook_failures"),
            DbEvidenceRef::hook(&hook_slug, last_at),
        ];

        insights.push(DbInsight::new(
            DbInsightKind::HookFailureSpike,
            DbInsightSeverity::High,
            title,
            summary,
            Some(format!(
                "Re-run `altevra hook run {} --debug` and check the audit_log for the \
                 latest error context.",
                hook_slug
            )),
            evidence,
        ));
    }
    Ok(insights)
}

// ---------------------------------------------------------------------------
// Run ALL DB-backed detectors in one pass
// ---------------------------------------------------------------------------

/// Run every DB-backed detector and return a merged, deduped insight list.
/// This is the function called by `run_observer_scan` and the brain job;
/// it replaces (and supplements) the event-only `detect_patterns` pass.
pub async fn run_db_detectors(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<DbInsight>> {
    let mut out = Vec::new();

    macro_rules! push_detector {
        ($call:expr, $name:literal) => {
            match $call.await {
                Ok(v) => out.extend(v),
                Err(e) => tracing::warn!("db_detector '{}' failed: {e}", $name),
            }
        };
    }

    push_detector!(detect_working_dir_drift(pool, now), "working_dir_drift");
    push_detector!(detect_stale_projects(pool, now), "stale_projects");
    push_detector!(detect_repeated_tool_failures(pool, now), "repeated_tool_failures");
    push_detector!(detect_late_night_sessions(pool, now), "late_night_sessions");
    push_detector!(detect_hook_failure_spike(pool, now), "hook_failure_spike");

    // Dedupe on (kind, title).
    let mut seen = std::collections::HashSet::new();
    out.retain(|i| seen.insert((i.kind, i.title.clone())));

    // Sort: High → Medium → Low.
    out.sort_by(|a, b| {
        let rank = |s: DbInsightSeverity| match s {
            DbInsightSeverity::High => 0u8,
            DbInsightSeverity::Medium => 1,
            DbInsightSeverity::Low => 2,
        };
        rank(a.severity).cmp(&rank(b.severity))
    });

    Ok(out)
}

// ---------------------------------------------------------------------------
// Events retention sweep
// ---------------------------------------------------------------------------

/// R4 — events retention job.
///
/// Prunes "raw / noise-class" events from the `events` table after N days.
/// Noise-class = event types that are purely observational scaffolding and carry
/// no durable signal: `tool_call_observed`, `prompt_sent`, `response_received`,
/// `file_changed`, `mcp_call`, `agent_thinking_step`, `error_logged`.
///
/// Session / skill / decision / research events are ALWAYS kept (they are the
/// long-term memory substrate).
///
/// Retention window is configurable; default is `DEFAULT_RETENTION_DAYS`.
pub const DEFAULT_RETENTION_DAYS: i64 = 30;

/// Event types considered "noise class" — pruned after `retention_days`.
///
/// Rule: an event is noise if it is *purely* an observational record whose
/// signal is already captured in the `turns` table (tool_calls/tool_results)
/// or in a higher-level event. Skill/session/decision events are the durable
/// signal and are NEVER pruned by this sweep.
pub const NOISE_CLASS_EVENT_TYPES: &[&str] = &[
    "tool_call_observed",
    "prompt_sent",
    "response_received",
    "file_changed",
    "mcp_call",
    "agent_thinking_step",
    "error_logged",
];

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsRetentionReport {
    /// Raw / noise-class rows deleted.
    pub pruned: usize,
    /// Retention window applied.
    pub retention_days: i64,
    /// How many event types were eligible for pruning.
    pub noise_types_checked: usize,
}

impl EventsRetentionReport {
    pub fn summary(&self) -> String {
        format!(
            "events_retention: pruned {pruned} noise-class event(s) older than {days}d \
             ({types} noise types; skill/session/decision events kept)",
            pruned = self.pruned,
            days = self.retention_days,
            types = self.noise_types_checked,
        )
    }
}

/// Prune noise-class events older than `retention_days` from the `events` table.
/// Session / skill / decision / research events are **never touched**.
pub async fn prune_noise_events(
    pool: &SqlitePool,
    now: DateTime<Utc>,
    retention_days: i64,
) -> anyhow::Result<EventsRetentionReport> {
    let cutoff = (now - Duration::days(retention_days)).to_rfc3339();

    // Build the IN list from constants — safe because NOISE_CLASS_EVENT_TYPES
    // is a compile-time &[&str], never user-supplied input.
    let placeholders: String = NOISE_CLASS_EVENT_TYPES
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "DELETE FROM events WHERE created_at < ? AND event_type IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql).bind(&cutoff);
    for t in NOISE_CLASS_EVENT_TYPES {
        q = q.bind(*t);
    }
    let result = q.execute(pool).await?;

    Ok(EventsRetentionReport {
        pruned: result.rows_affected() as usize,
        retention_days,
        noise_types_checked: NOISE_CLASS_EVENT_TYPES.len(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{create_pool, run_migrations};
    use chrono::Duration;

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    // Minimal session seeder. `working_dir` and `project_name` can be None.
    async fn seed_session(
        pool: &SqlitePool,
        id: &str,
        working_dir: Option<&str>,
        project_name: Option<&str>,
        started: DateTime<Utc>,
        ended: Option<DateTime<Utc>>,
    ) {
        let started_s = started.to_rfc3339();
        let ended_s = ended.map(|t| t.to_rfc3339());
        sqlx::query(
            r#"INSERT INTO sessions
               (id, tool, started_at, ended_at, working_dir, project_name,
                tokens_in_total, tokens_out_total, cost_usd_estimate, turn_count, metadata)
               VALUES (?, 'claude-code', ?, ?, ?, ?,
                       0, 0, 0.0, 0, '{}')"#,
        )
        .bind(id)
        .bind(&started_s)
        .bind(ended_s)
        .bind(working_dir)
        .bind(project_name)
        .execute(pool)
        .await
        .unwrap();
    }

    // Minimal turn seeder for tool_result error turns.
    async fn seed_error_turn(
        pool: &SqlitePool,
        session_id: &str,
        tool_name: &str,
        created: DateTime<Utc>,
    ) {
        let id = Uuid::new_v4().to_string();
        let created_s = created.to_rfc3339();
        sqlx::query(
            r#"INSERT INTO turns
               (id, session_id, turn_idx, role, content, tool_name, redacted_count,
                sensitivity, redaction_status, created_at)
               VALUES (?, ?, ?, 'tool_result', 'Error: command failed', ?, 0,
                       'internal', 'clean', ?)"#,
        )
        .bind(&id)
        .bind(session_id)
        .bind(created.timestamp() % 1000) // unique-ish idx
        .bind(tool_name)
        .bind(&created_s)
        .execute(pool)
        .await
        .unwrap();
    }

    // Minimal hook_run seeder for failed runs.
    async fn seed_hook_failure(
        pool: &SqlitePool,
        slug: &str,
        error: &str,
        created: DateTime<Utc>,
    ) {
        let id = Uuid::new_v4().to_string();
        let created_s = created.to_rfc3339();
        sqlx::query(
            r#"INSERT INTO hook_runs
               (id, hook_slug, tool_name, payload, result, success, error_message, duration_ms, created_at)
               VALUES (?, ?, 'Bash', '{}', '{}', 0, ?, 100, ?)"#,
        )
        .bind(&id)
        .bind(slug)
        .bind(error)
        .bind(&created_s)
        .execute(pool)
        .await
        .unwrap();
    }

    // -----------------------------------------------------------------------
    // Working-dir drift
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn working_dir_drift_fires_above_threshold() {
        let p = pool().await;
        let now = Utc::now();

        // 3 distinct working_dirs in the last 7 days → fires.
        for (i, dir) in ["/proj/alpha", "/proj/beta", "/proj/gamma"].iter().enumerate() {
            seed_session(
                &p,
                &format!("s-drift-{i}"),
                Some(dir),
                Some("test"),
                now - Duration::hours(i as i64 + 1),
                None,
            )
            .await;
        }

        let insights = detect_working_dir_drift(&p, now).await.unwrap();
        assert_eq!(insights.len(), 1, "should emit exactly one drift insight");
        assert_eq!(insights[0].kind, DbInsightKind::WorkingDirDrift);
        assert_eq!(insights[0].severity, DbInsightSeverity::Medium);
        // Evidence must be metadata-only (ids + counts, no content).
        for ev in &insights[0].evidence {
            assert!(!ev.label.is_empty());
            // Ensure no body text leaked into evidence.
            assert!(!ev.label.contains("content"), "evidence must be metadata-only");
        }
    }

    #[tokio::test]
    async fn working_dir_drift_silent_below_threshold() {
        let p = pool().await;
        let now = Utc::now();

        // Only 2 distinct dirs → silent.
        for (i, dir) in ["/proj/alpha", "/proj/beta"].iter().enumerate() {
            seed_session(
                &p,
                &format!("s-drift2-{i}"),
                Some(dir),
                Some("test"),
                now - Duration::hours(i as i64 + 1),
                None,
            )
            .await;
        }

        let insights = detect_working_dir_drift(&p, now).await.unwrap();
        assert!(
            insights.is_empty(),
            "below threshold: no insight expected (got {insights:?})"
        );
    }

    #[tokio::test]
    async fn working_dir_drift_ignores_old_sessions() {
        let p = pool().await;
        let now = Utc::now();

        // 3 distinct dirs — but all older than DRIFT_WINDOW_DAYS → silent.
        for (i, dir) in ["/proj/a", "/proj/b", "/proj/c"].iter().enumerate() {
            seed_session(
                &p,
                &format!("s-old-{i}"),
                Some(dir),
                Some("test"),
                now - Duration::days(DRIFT_WINDOW_DAYS + 1 + i as i64),
                None,
            )
            .await;
        }

        let insights = detect_working_dir_drift(&p, now).await.unwrap();
        assert!(insights.is_empty(), "old sessions must not trigger drift");
    }

    // -----------------------------------------------------------------------
    // Stale project
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stale_project_fires_for_silent_project() {
        let p = pool().await;
        let now = Utc::now();
        let stale_dir = "/proj/oldproject";

        // 3 historical sessions, all > STALE_PROJECT_DAYS ago.
        for i in 0..3i64 {
            seed_session(
                &p,
                &format!("s-stale-{i}"),
                Some(stale_dir),
                Some("OldProject"),
                now - Duration::days(STALE_PROJECT_DAYS + 10 + i),
                None,
            )
            .await;
        }

        let insights = detect_stale_projects(&p, now).await.unwrap();
        assert_eq!(insights.len(), 1, "one stale project expected");
        assert_eq!(insights[0].kind, DbInsightKind::StaleProject);
        assert!(
            insights[0].title.contains("OldProject"),
            "title must mention the project name"
        );
        // Metadata-only evidence.
        for ev in &insights[0].evidence {
            assert!(!ev.label.contains("content"));
        }
    }

    #[tokio::test]
    async fn stale_project_silent_for_active_project() {
        let p = pool().await;
        let now = Utc::now();

        // Recent sessions — not stale.
        for i in 0..3i64 {
            seed_session(
                &p,
                &format!("s-active-{i}"),
                Some("/proj/activeproject"),
                Some("Active"),
                now - Duration::days(i),
                None,
            )
            .await;
        }

        let insights = detect_stale_projects(&p, now).await.unwrap();
        assert!(insights.is_empty(), "active project must not be flagged");
    }

    #[tokio::test]
    async fn stale_project_ignores_single_session_projects() {
        let p = pool().await;
        let now = Utc::now();

        // Only 1 session (below STALE_PROJECT_MIN_SESSIONS=2) — not flagged.
        seed_session(
            &p,
            "s-singleton",
            Some("/proj/singleton"),
            Some("Singleton"),
            now - Duration::days(STALE_PROJECT_DAYS + 5),
            None,
        )
        .await;

        let insights = detect_stale_projects(&p, now).await.unwrap();
        assert!(
            insights.is_empty(),
            "single-session projects must not be flagged as stale"
        );
    }

    // -----------------------------------------------------------------------
    // Repeated tool failure
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn repeated_tool_failure_fires_above_threshold() {
        let p = pool().await;
        let now = Utc::now();
        let session_id = "s-toolerr";

        // Seed a session first (FK constraint).
        seed_session(&p, session_id, Some("/proj/x"), Some("X"), now, None).await;

        // 3 error turns for "Bash" in the window.
        for i in 0..TOOL_FAILURE_THRESHOLD as i64 {
            seed_error_turn(&p, session_id, "Bash", now - Duration::hours(i + 1)).await;
        }

        let insights = detect_repeated_tool_failures(&p, now).await.unwrap();
        assert_eq!(insights.len(), 1, "one tool-failure insight expected");
        assert_eq!(insights[0].kind, DbInsightKind::RepeatedToolFailure);
        assert_eq!(insights[0].severity, DbInsightSeverity::High);
        assert!(insights[0].title.contains("Bash"));
        // Evidence must not contain body content.
        for ev in &insights[0].evidence {
            assert!(!ev.label.to_lowercase().contains("error: command"), "body must not appear in evidence");
        }
    }

    #[tokio::test]
    async fn repeated_tool_failure_silent_below_threshold() {
        let p = pool().await;
        let now = Utc::now();
        let session_id = "s-toolerr2";

        seed_session(&p, session_id, Some("/proj/x"), Some("X"), now, None).await;

        // 2 turns — below threshold of 3.
        for i in 0..(TOOL_FAILURE_THRESHOLD as i64 - 1) {
            seed_error_turn(&p, session_id, "Bash", now - Duration::hours(i + 1)).await;
        }

        let insights = detect_repeated_tool_failures(&p, now).await.unwrap();
        assert!(insights.is_empty(), "below threshold → silent");
    }

    #[tokio::test]
    async fn repeated_tool_failure_ignores_old_turns() {
        let p = pool().await;
        let now = Utc::now();
        let session_id = "s-toolold";

        seed_session(&p, session_id, Some("/proj/x"), Some("X"), now, None).await;

        // Enough errors but all outside the window.
        for i in 0..TOOL_FAILURE_THRESHOLD as i64 {
            seed_error_turn(
                &p,
                session_id,
                "Bash",
                now - Duration::days(TOOL_FAILURE_WINDOW_DAYS + 1 + i),
            )
            .await;
        }

        let insights = detect_repeated_tool_failures(&p, now).await.unwrap();
        assert!(insights.is_empty(), "old turns must not trigger the detector");
    }

    // -----------------------------------------------------------------------
    // Late-night session
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn late_night_session_fires_for_qualifying_session() {
        let p = pool().await;
        let now = Utc::now();

        // Session at 02:30 UTC, 2h long → qualifies.
        let started = now
            .date_naive()
            .and_hms_opt(2, 30, 0)
            .map(|t| t.and_utc())
            .unwrap_or(now);
        let ended = started + Duration::hours(2);

        seed_session(
            &p,
            "s-latenight",
            Some("/proj/x"),
            Some("X"),
            started,
            Some(ended),
        )
        .await;

        let insights = detect_late_night_sessions(&p, now).await.unwrap();
        assert_eq!(insights.len(), 1, "one late-night insight expected");
        assert_eq!(insights[0].kind, DbInsightKind::LateNightSession);
        // Metadata-only: no body text in evidence.
        for ev in &insights[0].evidence {
            assert!(!ev.label.contains("content"));
        }
    }

    #[tokio::test]
    async fn late_night_session_silent_for_daytime_session() {
        let p = pool().await;
        let now = Utc::now();

        // Session at 10:00 UTC, 2h long → does NOT qualify.
        let started = now
            .date_naive()
            .and_hms_opt(10, 0, 0)
            .map(|t| t.and_utc())
            .unwrap_or(now);
        let ended = started + Duration::hours(2);

        seed_session(
            &p,
            "s-daytime",
            Some("/proj/x"),
            Some("X"),
            started,
            Some(ended),
        )
        .await;

        let insights = detect_late_night_sessions(&p, now).await.unwrap();
        assert!(insights.is_empty(), "daytime session must not trigger late-night detector");
    }

    #[tokio::test]
    async fn late_night_session_silent_for_short_session() {
        let p = pool().await;
        let now = Utc::now();

        // Session at 02:30 UTC, 20min — too short.
        let started = now
            .date_naive()
            .and_hms_opt(2, 30, 0)
            .map(|t| t.and_utc())
            .unwrap_or(now);
        let ended = started + Duration::minutes(20);

        seed_session(
            &p,
            "s-shortnight",
            Some("/proj/x"),
            Some("X"),
            started,
            Some(ended),
        )
        .await;

        let insights = detect_late_night_sessions(&p, now).await.unwrap();
        assert!(insights.is_empty(), "short late-night session must not trigger");
    }

    // -----------------------------------------------------------------------
    // Hook failure spike
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn hook_failure_fires_above_threshold() {
        let p = pool().await;
        let now = Utc::now();

        // 3 failures for "session_start" hook → fires.
        for i in 0..HOOK_FAILURE_THRESHOLD as i64 {
            seed_hook_failure(
                &p,
                "session_start",
                "altevra: command not found",
                now - Duration::hours(i + 1),
            )
            .await;
        }

        let insights = detect_hook_failure_spike(&p, now).await.unwrap();
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, DbInsightKind::HookFailureSpike);
        assert_eq!(insights[0].severity, DbInsightSeverity::High);
        assert!(insights[0].title.contains("session_start"));
        // Metadata-only evidence.
        for ev in &insights[0].evidence {
            assert!(!ev.label.contains("altevra: command not found"), "error text must not appear in evidence label");
        }
    }

    #[tokio::test]
    async fn hook_failure_silent_below_threshold() {
        let p = pool().await;
        let now = Utc::now();

        for i in 0..(HOOK_FAILURE_THRESHOLD as i64 - 1) {
            seed_hook_failure(&p, "pre_tool", "err", now - Duration::hours(i + 1)).await;
        }

        let insights = detect_hook_failure_spike(&p, now).await.unwrap();
        assert!(insights.is_empty(), "below threshold → silent");
    }

    #[tokio::test]
    async fn hook_failure_silent_for_old_failures() {
        let p = pool().await;
        let now = Utc::now();

        // Enough failures but outside the window.
        for i in 0..HOOK_FAILURE_THRESHOLD as i64 {
            seed_hook_failure(
                &p,
                "pre_tool",
                "err",
                now - Duration::hours(HOOK_FAILURE_WINDOW_HOURS + 1 + i),
            )
            .await;
        }

        let insights = detect_hook_failure_spike(&p, now).await.unwrap();
        assert!(insights.is_empty(), "old failures must not trigger");
    }

    // -----------------------------------------------------------------------
    // Events retention
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn events_retention_prunes_noise_and_keeps_skill_events() {
        use altevra_core::events::{ActorType, Event, EventType};
        use altevra_db::EventsRepository;

        let p = pool().await;
        let now = Utc::now();
        let repo = EventsRepository::new(&p);
        let old_cutoff = now - Duration::days(DEFAULT_RETENTION_DAYS + 1);

        // Insert noise-class events (old → should be pruned).
        let noise_types = [
            EventType::ToolCallObserved,
            EventType::PromptSent,
            EventType::ResponseReceived,
            EventType::FileChanged,
            EventType::McpCall,
            EventType::AgentThinkingStep,
            EventType::ErrorLogged,
        ];
        for nt in &noise_types {
            let mut ev = Event::new(nt.clone(), "noise", "test", ActorType::System);
            ev.created_at = old_cutoff - Duration::hours(1);
            repo.insert(&ev).await.unwrap();
        }

        // Insert durable events that MUST NOT be pruned (even if old).
        let durable_types = [
            EventType::SkillDriftDetected,
            EventType::SessionStarted,
            EventType::DecisionSaved,
            EventType::SkillInvocation,
            EventType::TaskCreated,
        ];
        for dt in &durable_types {
            let mut ev = Event::new(dt.clone(), "durable", "test", ActorType::System);
            ev.created_at = old_cutoff - Duration::hours(1);
            repo.insert(&ev).await.unwrap();
        }

        // Also insert a RECENT noise event — must NOT be pruned (within window).
        let mut recent_noise = Event::new(EventType::FileChanged, "recent", "test", ActorType::System);
        recent_noise.created_at = now - Duration::hours(1);
        repo.insert(&recent_noise).await.unwrap();

        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(
            before,
            (noise_types.len() + durable_types.len() + 1) as i64,
            "pre-prune count mismatch"
        );

        let report = prune_noise_events(&p, now, DEFAULT_RETENTION_DAYS).await.unwrap();

        // Noise-class events older than the window → pruned.
        assert_eq!(
            report.pruned, noise_types.len(),
            "all old noise events must be pruned; got report: {report:?}"
        );

        // Durable events must survive regardless.
        let durable_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE title = 'durable'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(
            durable_count,
            durable_types.len() as i64,
            "durable events must never be pruned"
        );

        // Recent noise event must survive.
        let recent_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE title = 'recent'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(recent_count, 1, "recent noise must not be pruned");
    }

    #[tokio::test]
    async fn events_retention_idempotent() {
        use altevra_core::events::{ActorType, Event, EventType};
        use altevra_db::EventsRepository;

        let p = pool().await;
        let now = Utc::now();
        let repo = EventsRepository::new(&p);

        // One old noise event.
        let mut ev = Event::new(EventType::ErrorLogged, "noise", "test", ActorType::System);
        ev.created_at = now - Duration::days(DEFAULT_RETENTION_DAYS + 5);
        repo.insert(&ev).await.unwrap();

        let r1 = prune_noise_events(&p, now, DEFAULT_RETENTION_DAYS).await.unwrap();
        assert_eq!(r1.pruned, 1);
        let r2 = prune_noise_events(&p, now, DEFAULT_RETENTION_DAYS).await.unwrap();
        assert_eq!(r2.pruned, 0, "second run on empty table is a no-op");
    }

    // -----------------------------------------------------------------------
    // Metadata-only emission invariant
    // -----------------------------------------------------------------------

    /// Verify that no evidence struct produced by the DB detectors carries
    /// raw turn body / session metadata (only ID refs + counts).
    #[tokio::test]
    async fn metadata_only_evidence_invariant() {
        let p = pool().await;
        let now = Utc::now();
        let session_id = "s-meta";

        // Seed enough data to trip multiple detectors.
        for (i, dir) in ["/proj/alpha", "/proj/beta", "/proj/gamma"].iter().enumerate() {
            seed_session(&p, &format!("s-meta-{i}"), Some(dir), Some("T"), now - Duration::hours(i as i64 + 1), None).await;
        }

        seed_session(&p, session_id, Some("/proj/alpha"), Some("T"), now, None).await;
        for i in 0..TOOL_FAILURE_THRESHOLD as i64 {
            seed_error_turn(&p, session_id, "Edit", now - Duration::hours(i + 1)).await;
        }

        let insights = run_db_detectors(&p, now).await.unwrap();
        for insight in &insights {
            for ev in &insight.evidence {
                // Evidence labels must not contain raw content. They must be
                // short pointer-style strings.
                assert!(
                    ev.label.len() < 300,
                    "evidence label suspiciously long ({} chars): '{}'",
                    ev.label.len(),
                    &ev.label[..ev.label.len().min(80)]
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // run_db_detectors integration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_db_detectors_returns_empty_on_empty_db() {
        let p = pool().await;
        let now = Utc::now();
        let insights = run_db_detectors(&p, now).await.unwrap();
        assert!(insights.is_empty(), "empty DB should yield no insights");
    }
}
