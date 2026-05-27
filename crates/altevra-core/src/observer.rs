//! Observer brain — higher-level pattern detector that turns raw events into
//! structured insights.
//!
//! While the classifier (see `crate::classifier`) labels a single event with an
//! importance score, the observer looks at *collections* of events and detects
//! recurring patterns the user should know about: repeated drift, hook flaps,
//! decision conflicts, stale projects, etc.
//!
//! See `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md` §19 "Event-to-Update Pipeline".

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::events::{Event, EventType};
use crate::updates::{Importance, UpdateFeedItem};

// -----------------------------------------------------------------------------
// Thresholds — tunable constants for detection.
// -----------------------------------------------------------------------------

/// ≥ this many SkillDriftDetected for the same slug within the window → insight.
pub const DRIFT_THRESHOLD: usize = 3;
/// Window for recurring-drift detection (days).
pub const DRIFT_WINDOW_DAYS: i64 = 7;

/// ≥ this many HookFailed for the same hook slug within the window → insight.
pub const HOOK_FAILURE_THRESHOLD: usize = 3;
/// Window for repeated-hook-failure detection (hours).
pub const HOOK_FAILURE_WINDOW_HOURS: i64 = 24;

/// ≥ this many SessionStarted within the window → insight.
pub const HIGH_SESSION_VOLUME_THRESHOLD: usize = 10;
/// Window for high-session-volume detection (hours).
pub const HIGH_SESSION_VOLUME_WINDOW_HOURS: i64 = 24;

/// Days without any activity for a known project → insight.
pub const STALE_PROJECT_DAYS: i64 = 14;

/// ≥ this many SecretChanged for the same key in the window → insight.
pub const SECRET_CHURN_THRESHOLD: usize = 3;
/// Window for secret churn detection (days).
pub const SECRET_CHURN_WINDOW_DAYS: i64 = 7;

/// Jaro-Winkler similarity threshold for treating two decision titles as the
/// "same topic".
pub const DECISION_SIMILARITY_THRESHOLD: f64 = 0.85;
/// Window for decision-conflict detection (days).
pub const DECISION_CONFLICT_WINDOW_DAYS: i64 = 30;

/// Open tasks (TaskCreated without matching TaskCompleted) above this count
/// within the velocity window → LowTaskVelocity insight.
pub const LOW_TASK_VELOCITY_OPEN_THRESHOLD: usize = 5;
/// Days of low completion activity before flagging.
pub const LOW_TASK_VELOCITY_WINDOW_DAYS: i64 = 7;

// -----------------------------------------------------------------------------
// Types.
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightKind {
    /// Same skill drift detected ≥ 3 times in last 7 days.
    RecurringDrift,
    /// Same hook failed ≥ 3 times in last 24h.
    RepeatedHookFailure,
    /// Many open tasks with no completions in the velocity window.
    LowTaskVelocity,
    /// Session starts exceed baseline in last 24h.
    HighSessionVolume,
    /// No events for a previously-active project in last 14 days.
    StaleProject,
    /// Two decisions on a fuzzy-similar title within 30 days.
    DecisionConflict,
    /// Same secret key changed > 3 times in 7 days.
    SecretChurn,
    /// Multiple installed versions of the same skill across tools.
    SkillVersionDivergence,
}

impl std::fmt::Display for InsightKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}").to_lowercase());
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub event_id: Option<Uuid>,
    pub update_id: Option<Uuid>,
    /// Human-readable origin (e.g. `.altevra/events/updates.jsonl#L42`).
    pub source: String,
    pub timestamp: DateTime<Utc>,
}

impl EvidenceRef {
    pub fn from_event(event: &Event) -> Self {
        Self {
            event_id: Some(event.id),
            update_id: None,
            source: event.source.clone(),
            timestamp: event.created_at,
        }
    }

    pub fn from_update(update: &UpdateFeedItem) -> Self {
        Self {
            event_id: Some(update.event_id),
            update_id: Some(update.id),
            source: update.update_type.clone(),
            timestamp: update.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Insight {
    pub id: Uuid,
    pub kind: InsightKind,
    pub title: String,
    pub summary: String,
    pub evidence: Vec<EvidenceRef>,
    pub recommended_action: Option<String>,
    pub importance: Importance,
    pub generated_at: DateTime<Utc>,
}

impl Insight {
    fn new(
        kind: InsightKind,
        title: impl Into<String>,
        summary: impl Into<String>,
        evidence: Vec<EvidenceRef>,
        recommended_action: Option<String>,
        importance: Importance,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            title: title.into(),
            summary: summary.into(),
            evidence,
            recommended_action,
            importance,
            generated_at: Utc::now(),
        }
    }
}

// -----------------------------------------------------------------------------
// Public API — full pass.
// -----------------------------------------------------------------------------

/// Run every pattern detector over the supplied window and return a
/// deduplicated list of insights, ordered by importance (high → low) then
/// generation order.
pub fn detect_patterns(events: &[Event], _updates: &[UpdateFeedItem]) -> Vec<Insight> {
    let mut out = Vec::new();
    out.extend(detect_recurring_drift(events));
    out.extend(detect_repeated_hook_failure(events));
    out.extend(detect_low_task_velocity(events));
    out.extend(detect_high_session_volume(events));
    out.extend(detect_stale_project(events));
    out.extend(detect_decision_conflict(events));
    out.extend(detect_secret_churn(events));
    out.extend(detect_skill_version_divergence(events));

    // Dedupe on (kind, title) — two passes shouldn't yield two insights about
    // the same thing.
    let mut seen: std::collections::HashSet<(InsightKind, String)> =
        std::collections::HashSet::new();
    out.retain(|i| seen.insert((i.kind, i.title.clone())));

    // Sort high → low importance for stable agent consumption.
    out.sort_by(|a, b| b.importance.cmp(&a.importance));
    out
}

// -----------------------------------------------------------------------------
// Individual detectors.
// -----------------------------------------------------------------------------

/// Recurring skill drift — ≥ DRIFT_THRESHOLD events for the same `entity_id`
/// (the skill slug) within DRIFT_WINDOW_DAYS.
pub fn detect_recurring_drift(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::days(DRIFT_WINDOW_DAYS);
    let mut by_slug: HashMap<String, Vec<&Event>> = HashMap::new();

    for event in events {
        if event.event_type != EventType::SkillDriftDetected {
            continue;
        }
        if event.created_at < cutoff {
            continue;
        }
        let slug = slug_for(event);
        by_slug.entry(slug).or_default().push(event);
    }

    let mut out = Vec::new();
    for (slug, group) in by_slug {
        if group.len() < DRIFT_THRESHOLD {
            continue;
        }
        let evidence: Vec<EvidenceRef> = group.iter().map(|e| EvidenceRef::from_event(e)).collect();
        let title = format!("Recurring drift: {}", slug);
        let summary = format!(
            "Skill `{}` drifted {} times in the last {} days. Repeated drift usually means another \
             tool is overwriting Altevra's canonical skill copy.",
            slug,
            group.len(),
            DRIFT_WINDOW_DAYS
        );
        let action = Some(format!(
            "Review .claude/skills/{} for manual edits; run `altevra skill refresh {}` and inspect \
             tool sync hooks.",
            slug, slug
        ));
        out.push(Insight::new(
            InsightKind::RecurringDrift,
            title,
            summary,
            evidence,
            action,
            Importance::High,
        ));
    }
    out
}

/// Repeated hook failures — ≥ HOOK_FAILURE_THRESHOLD for the same hook within
/// HOOK_FAILURE_WINDOW_HOURS. Emits one insight per failing hook.
pub fn detect_repeated_hook_failure(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::hours(HOOK_FAILURE_WINDOW_HOURS);
    let mut by_hook: HashMap<String, Vec<&Event>> = HashMap::new();

    for event in events {
        if event.event_type != EventType::HookFailed {
            continue;
        }
        if event.created_at < cutoff {
            continue;
        }
        let slug = slug_for(event);
        by_hook.entry(slug).or_default().push(event);
    }

    let mut out = Vec::new();
    for (slug, group) in by_hook {
        if group.len() < HOOK_FAILURE_THRESHOLD {
            continue;
        }
        let evidence: Vec<EvidenceRef> = group.iter().map(|e| EvidenceRef::from_event(e)).collect();
        let title = format!("Repeated hook failure: {}", slug);
        let summary = format!(
            "Hook `{}` failed {} times in the last {}h. Likely a config regression, missing \
             dependency, or upstream tool change.",
            slug,
            group.len(),
            HOOK_FAILURE_WINDOW_HOURS
        );
        let action = Some(format!(
            "Re-run `altevra hook run {} --debug` and inspect `.altevra/events/updates.jsonl` for \
             the latest error payload.",
            slug
        ));
        out.push(Insight::new(
            InsightKind::RepeatedHookFailure,
            title,
            summary,
            evidence,
            action,
            Importance::High,
        ));
    }
    out
}

/// Low task velocity — many TaskCreated within the window but no TaskCompleted
/// to match. Emits one insight when open count exceeds threshold.
pub fn detect_low_task_velocity(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::days(LOW_TASK_VELOCITY_WINDOW_DAYS);
    let mut opened: Vec<&Event> = Vec::new();
    let mut completed_count = 0usize;

    for event in events {
        if event.created_at < cutoff {
            continue;
        }
        match event.event_type {
            EventType::TaskCreated => opened.push(event),
            EventType::TaskCompleted => completed_count += 1,
            _ => {}
        }
    }

    if opened.len() < LOW_TASK_VELOCITY_OPEN_THRESHOLD {
        return vec![];
    }
    if completed_count >= opened.len() {
        return vec![]; // Healthy throughput.
    }

    let open_excess = opened.len() - completed_count;
    let evidence: Vec<EvidenceRef> = opened.iter().map(|e| EvidenceRef::from_event(e)).collect();
    let title = format!(
        "Low task velocity: {} open tasks, {} completed (last {}d)",
        opened.len(),
        completed_count,
        LOW_TASK_VELOCITY_WINDOW_DAYS
    );
    let summary = format!(
        "{} tasks created vs {} completed in the last {} days — {} more open than closed. \
         Backlog is growing.",
        opened.len(),
        completed_count,
        LOW_TASK_VELOCITY_WINDOW_DAYS,
        open_excess
    );
    let action = Some(
        "Review `altevra updates --important` and prune or close stale tasks; consider WIP limits."
            .to_string(),
    );
    vec![Insight::new(
        InsightKind::LowTaskVelocity,
        title,
        summary,
        evidence,
        action,
        Importance::Medium,
    )]
}

/// High session volume — ≥ HIGH_SESSION_VOLUME_THRESHOLD SessionStarted events
/// in the trailing window.
pub fn detect_high_session_volume(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::hours(HIGH_SESSION_VOLUME_WINDOW_HOURS);
    let starts: Vec<&Event> = events
        .iter()
        .filter(|e| e.event_type == EventType::SessionStarted && e.created_at >= cutoff)
        .collect();

    if starts.len() < HIGH_SESSION_VOLUME_THRESHOLD {
        return vec![];
    }

    let evidence: Vec<EvidenceRef> = starts.iter().map(|e| EvidenceRef::from_event(e)).collect();
    let title = format!(
        "High session volume: {} sessions in last {}h",
        starts.len(),
        HIGH_SESSION_VOLUME_WINDOW_HOURS
    );
    let summary = format!(
        "Detected {} session starts in the last {}h (threshold {}). Either heavy active work or a \
         stuck tool that restarts sessions in a loop.",
        starts.len(),
        HIGH_SESSION_VOLUME_WINDOW_HOURS,
        HIGH_SESSION_VOLUME_THRESHOLD
    );
    let action = Some(
        "If you've been heads-down — ignore. If not, check for a flapping IDE / hook loop with \
         `altevra doctor`."
            .to_string(),
    );
    vec![Insight::new(
        InsightKind::HighSessionVolume,
        title,
        summary,
        evidence,
        action,
        Importance::Medium,
    )]
}

/// Stale project — a project with historical events but no activity in the
/// last STALE_PROJECT_DAYS. Emits one insight per stale project.
pub fn detect_stale_project(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::days(STALE_PROJECT_DAYS);
    let mut latest: HashMap<Uuid, &Event> = HashMap::new();
    let mut any_old: HashMap<Uuid, bool> = HashMap::new();

    for event in events {
        let Some(pid) = event.project_id else {
            continue;
        };
        // Track historical presence (before cutoff).
        if event.created_at < cutoff {
            any_old.entry(pid).or_insert(true);
        }
        // Track latest event for the project.
        match latest.get(&pid) {
            Some(existing) if existing.created_at >= event.created_at => {}
            _ => {
                latest.insert(pid, event);
            }
        }
    }

    let mut out = Vec::new();
    for (pid, last_event) in latest {
        if last_event.created_at >= cutoff {
            continue; // Active.
        }
        if !any_old.get(&pid).copied().unwrap_or(false) {
            continue; // No historical activity either — nothing to flag.
        }
        let evidence = vec![EvidenceRef::from_event(last_event)];
        let days = (Utc::now() - last_event.created_at).num_days();
        let title = format!("Stale project: {}", pid);
        let summary = format!(
            "Project {} has had no events for {} days (threshold {}). Last event: `{}`.",
            pid, days, STALE_PROJECT_DAYS, last_event.title
        );
        let action = Some(format!(
            "Either archive the project (`altevra project archive {}`) or schedule a status review.",
            pid
        ));
        out.push(Insight::new(
            InsightKind::StaleProject,
            title,
            summary,
            evidence,
            action,
            Importance::Low,
        ));
    }
    out
}

/// Decision conflict — two DecisionSaved events with fuzzy-similar titles
/// (Jaro-Winkler ≥ DECISION_SIMILARITY_THRESHOLD) within
/// DECISION_CONFLICT_WINDOW_DAYS.
pub fn detect_decision_conflict(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::days(DECISION_CONFLICT_WINDOW_DAYS);
    let decisions: Vec<&Event> = events
        .iter()
        .filter(|e| e.event_type == EventType::DecisionSaved && e.created_at >= cutoff)
        .collect();

    let mut out = Vec::new();
    let mut emitted_pairs: std::collections::HashSet<(Uuid, Uuid)> =
        std::collections::HashSet::new();

    for (i, a) in decisions.iter().enumerate() {
        for b in decisions.iter().skip(i + 1) {
            if a.id == b.id {
                continue;
            }
            // Avoid duplicate (a,b) / (b,a) pairs.
            let key = if a.id < b.id {
                (a.id, b.id)
            } else {
                (b.id, a.id)
            };
            if !emitted_pairs.insert(key) {
                continue;
            }
            let sim = strsim::jaro_winkler(&a.title, &b.title);
            if sim < DECISION_SIMILARITY_THRESHOLD {
                continue;
            }
            let evidence = vec![EvidenceRef::from_event(a), EvidenceRef::from_event(b)];
            let title = format!("Decision conflict: \"{}\" ~ \"{}\"", a.title, b.title);
            let summary = format!(
                "Two decisions with similar titles (Jaro-Winkler {:.2}) within {} days: \"{}\" and \
                 \"{}\". Possible flip-flop or duplicate ruling.",
                sim, DECISION_CONFLICT_WINDOW_DAYS, a.title, b.title
            );
            let action = Some(
                "Open `~/Obsidian/Imperium/Memory/Decisions.md`, reconcile or mark one as \
                 superseded."
                    .to_string(),
            );
            out.push(Insight::new(
                InsightKind::DecisionConflict,
                title,
                summary,
                evidence,
                action,
                Importance::High,
            ));
        }
    }
    out
}

/// Secret churn — same secret key changed ≥ SECRET_CHURN_THRESHOLD times within
/// SECRET_CHURN_WINDOW_DAYS. Grouped by `entity_id` (the secret key).
pub fn detect_secret_churn(events: &[Event]) -> Vec<Insight> {
    let cutoff = Utc::now() - Duration::days(SECRET_CHURN_WINDOW_DAYS);
    let mut by_key: HashMap<String, Vec<&Event>> = HashMap::new();

    for event in events {
        if event.event_type != EventType::SecretChanged {
            continue;
        }
        if event.created_at < cutoff {
            continue;
        }
        let key = event
            .entity_id
            .clone()
            .or_else(|| slug_from_payload(event))
            .unwrap_or_else(|| event.title.clone());
        by_key.entry(key).or_default().push(event);
    }

    let mut out = Vec::new();
    for (key, group) in by_key {
        if group.len() < SECRET_CHURN_THRESHOLD {
            continue;
        }
        let evidence: Vec<EvidenceRef> = group.iter().map(|e| EvidenceRef::from_event(e)).collect();
        let title = format!("Secret churn: {}", key);
        let summary = format!(
            "Secret `{}` changed {} times in the last {} days. Frequent rotation can indicate a \
             leaked credential or a misconfigured rotation script.",
            key,
            group.len(),
            SECRET_CHURN_WINDOW_DAYS
        );
        let action = Some(format!(
            "Audit access to `{}`, confirm rotation is intentional, and check that dependents \
             reload via `altevra doctor`.",
            key
        ));
        out.push(Insight::new(
            InsightKind::SecretChurn,
            title,
            summary,
            evidence,
            action,
            Importance::High,
        ));
    }
    out
}

/// Skill version divergence — multiple distinct installed versions for the
/// same skill slug seen across SkillInstalled events.
pub fn detect_skill_version_divergence(events: &[Event]) -> Vec<Insight> {
    let mut versions_by_slug: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    let mut sample_events: HashMap<String, Vec<&Event>> = HashMap::new();

    for event in events {
        if event.event_type != EventType::SkillInstalled
            && event.event_type != EventType::SkillUpdated
        {
            continue;
        }
        let slug = slug_for(event);
        let version = event
            .payload
            .get("version")
            .and_then(|v| v.as_str())
            .or_else(|| {
                event
                    .payload
                    .get("installed_version")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();
        if version.is_empty() {
            continue;
        }
        versions_by_slug
            .entry(slug.clone())
            .or_default()
            .insert(version);
        sample_events.entry(slug).or_default().push(event);
    }

    let mut out = Vec::new();
    for (slug, versions) in versions_by_slug {
        if versions.len() < 2 {
            continue;
        }
        let evidence: Vec<EvidenceRef> = sample_events
            .get(&slug)
            .map(|evs| evs.iter().map(|e| EvidenceRef::from_event(e)).collect())
            .unwrap_or_default();
        let mut vlist: Vec<String> = versions.into_iter().collect();
        vlist.sort();
        let title = format!("Skill version divergence: {}", slug);
        let summary = format!(
            "Skill `{}` has {} distinct installed versions: {}. Tools are out of sync.",
            slug,
            vlist.len(),
            vlist.join(", ")
        );
        let action = Some(format!(
            "Run `altevra skill refresh {} --all-tools` to converge installations.",
            slug
        ));
        out.push(Insight::new(
            InsightKind::SkillVersionDivergence,
            title,
            summary,
            evidence,
            action,
            Importance::Medium,
        ));
    }
    out
}

// -----------------------------------------------------------------------------
// Helpers.
// -----------------------------------------------------------------------------

/// Best-effort slug extraction — prefer `entity_id`, then payload fields, then
/// title.
fn slug_for(event: &Event) -> String {
    if let Some(id) = event.entity_id.as_ref().filter(|s| !s.is_empty()) {
        return id.clone();
    }
    if let Some(slug) = slug_from_payload(event) {
        return slug;
    }
    event.title.clone()
}

fn slug_from_payload(event: &Event) -> Option<String> {
    for key in ["slug", "skill_slug", "hook_slug", "key", "name"] {
        if let Some(v) = event.payload.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Insights writer.
// -----------------------------------------------------------------------------

pub mod writer {
    use super::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// Write the supplied insights to `vault_root/10-insights/auto-YYYYMMDD.md`.
    /// Atomic: writes to a temp file alongside the destination and renames.
    /// Returns the destination path.
    pub fn write_insights_markdown(
        insights: &[Insight],
        vault_root: &Path,
    ) -> anyhow::Result<PathBuf> {
        let dir = vault_root.join("10-insights");
        std::fs::create_dir_all(&dir)?;
        let stamp = Utc::now().format("%Y%m%d");
        let dest = dir.join(format!("auto-{stamp}.md"));

        let mut body = String::new();
        body.push_str("---\n");
        body.push_str("kind: auto-insight\n");
        body.push_str("generated_by: altevra-observer\n");
        body.push_str(&format!("count: {}\n", insights.len()));
        body.push_str(&format!("generated_at: {}\n", Utc::now().to_rfc3339()));
        body.push_str("---\n\n");
        body.push_str(&format!("# Observer Insights — {}\n\n", stamp));

        if insights.is_empty() {
            body.push_str("_No patterns detected in this window._\n");
        } else {
            for ins in insights {
                body.push_str(&format!("## [{}] {}\n\n", ins.importance, ins.title));
                body.push_str(&format!("- **Kind:** `{}`\n", ins.kind));
                body.push_str(&format!(
                    "- **Generated:** {}\n",
                    ins.generated_at.to_rfc3339()
                ));
                body.push_str(&format!("- **Evidence count:** {}\n\n", ins.evidence.len()));
                body.push_str(&format!("{}\n\n", ins.summary));
                if let Some(action) = ins.recommended_action.as_ref() {
                    body.push_str(&format!("**Recommended action:** {}\n\n", action));
                }
                if !ins.evidence.is_empty() {
                    body.push_str("<details><summary>Evidence</summary>\n\n");
                    for ev in &ins.evidence {
                        body.push_str(&format!(
                            "- `{}` @ {}",
                            ev.source,
                            ev.timestamp.to_rfc3339()
                        ));
                        if let Some(eid) = ev.event_id {
                            body.push_str(&format!(" (event `{}`)", eid));
                        }
                        body.push('\n');
                    }
                    body.push_str("\n</details>\n\n");
                }
            }
        }

        // Atomic write: tmp file in same dir, rename onto dest.
        let tmp = dir.join(format!(".auto-{stamp}.md.tmp"));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(body.as_bytes())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, &dest)?;
        Ok(dest)
    }

    /// List previously-written insight files (sorted descending by filename
    /// stamp, which is also chronological).
    pub fn list_insight_files(vault_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
        let dir = vault_root.join("10-insights");
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("auto-") && n.ends_with(".md"))
                    .unwrap_or(false)
            })
            .collect();
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        Ok(entries)
    }
}

// -----------------------------------------------------------------------------
// Tests.
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ActorType;

    fn drift_event(slug: &str, ago: Duration) -> Event {
        let mut ev = Event::new(
            EventType::SkillDriftDetected,
            format!("drift {slug}"),
            "test",
            ActorType::System,
        )
        .with_entity("skill", slug);
        ev.created_at = Utc::now() - ago;
        ev
    }

    fn hook_failed(slug: &str, ago: Duration) -> Event {
        let mut ev = Event::new(
            EventType::HookFailed,
            format!("hook {slug}"),
            "test",
            ActorType::Hook,
        )
        .with_entity("hook", slug);
        ev.created_at = Utc::now() - ago;
        ev
    }

    fn task_created(title: &str, ago: Duration) -> Event {
        let mut ev = Event::new(EventType::TaskCreated, title, "test", ActorType::User);
        ev.created_at = Utc::now() - ago;
        ev
    }

    fn task_completed(title: &str, ago: Duration) -> Event {
        let mut ev = Event::new(EventType::TaskCompleted, title, "test", ActorType::User);
        ev.created_at = Utc::now() - ago;
        ev
    }

    fn session_started(ago: Duration) -> Event {
        let mut ev = Event::new(
            EventType::SessionStarted,
            "session",
            "test",
            ActorType::Hook,
        );
        ev.created_at = Utc::now() - ago;
        ev
    }

    fn secret_changed(key: &str, ago: Duration) -> Event {
        let mut ev = Event::new(
            EventType::SecretChanged,
            format!("rotate {key}"),
            "test",
            ActorType::System,
        )
        .with_entity("secret", key);
        ev.created_at = Utc::now() - ago;
        ev
    }

    fn decision_saved(title: &str, ago: Duration) -> Event {
        let mut ev = Event::new(EventType::DecisionSaved, title, "test", ActorType::User);
        ev.created_at = Utc::now() - ago;
        ev
    }

    // -- detector tests --

    #[test]
    fn drift_three_same_slug_emits_one_insight() {
        let events = vec![
            drift_event("foo", Duration::hours(1)),
            drift_event("foo", Duration::hours(2)),
            drift_event("foo", Duration::hours(3)),
        ];
        let insights = detect_recurring_drift(&events);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::RecurringDrift);
        assert!(insights[0].title.contains("foo"));
        assert_eq!(insights[0].evidence.len(), 3);
        assert_eq!(insights[0].importance, Importance::High);
    }

    #[test]
    fn drift_two_below_threshold_no_insight() {
        let events = vec![
            drift_event("foo", Duration::hours(1)),
            drift_event("foo", Duration::hours(2)),
        ];
        assert!(detect_recurring_drift(&events).is_empty());
    }

    #[test]
    fn hook_failures_across_hooks_separate_insights() {
        let events = vec![
            hook_failed("session_start", Duration::hours(1)),
            hook_failed("session_start", Duration::hours(2)),
            hook_failed("session_start", Duration::hours(3)),
            hook_failed("pre_tool", Duration::minutes(30)),
            hook_failed("pre_tool", Duration::minutes(45)),
            hook_failed("pre_tool", Duration::minutes(60)),
        ];
        let mut insights = detect_repeated_hook_failure(&events);
        insights.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(insights.len(), 2);
        assert!(insights.iter().any(|i| i.title.contains("session_start")));
        assert!(insights.iter().any(|i| i.title.contains("pre_tool")));
    }

    #[test]
    fn low_task_velocity_emits_when_open_exceeds() {
        let mut events = Vec::new();
        for i in 0..6 {
            events.push(task_created(
                &format!("task-{i}"),
                Duration::days(1 + i as i64),
            ));
        }
        events.push(task_completed("task-0", Duration::days(1)));
        let insights = detect_low_task_velocity(&events);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::LowTaskVelocity);
    }

    #[test]
    fn low_task_velocity_quiet_when_balanced() {
        let mut events = Vec::new();
        for i in 0..6 {
            events.push(task_created(
                &format!("task-{i}"),
                Duration::days(1 + i as i64),
            ));
            events.push(task_completed(
                &format!("task-{i}"),
                Duration::hours(2 + i as i64),
            ));
        }
        assert!(detect_low_task_velocity(&events).is_empty());
    }

    #[test]
    fn secret_churn_three_same_key_one_insight() {
        let events = vec![
            secret_changed("OPENAI_API_KEY", Duration::days(1)),
            secret_changed("OPENAI_API_KEY", Duration::days(2)),
            secret_changed("OPENAI_API_KEY", Duration::days(3)),
        ];
        let insights = detect_secret_churn(&events);
        assert_eq!(insights.len(), 1);
        assert!(insights[0].title.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn decision_conflict_fuzzy_similar_titles() {
        let events = vec![
            decision_saved("Use PostgreSQL for primary database", Duration::days(1)),
            decision_saved("Use PostgreSQL for primary databases", Duration::days(2)),
        ];
        let insights = detect_decision_conflict(&events);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::DecisionConflict);
    }

    #[test]
    fn high_session_volume_ten_plus() {
        let events: Vec<Event> = (0..10)
            .map(|i| session_started(Duration::minutes(i * 5)))
            .collect();
        let insights = detect_high_session_volume(&events);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::HighSessionVolume);
    }

    #[test]
    fn high_session_volume_five_no_insight() {
        let events: Vec<Event> = (0..5)
            .map(|i| session_started(Duration::minutes(i * 5)))
            .collect();
        assert!(detect_high_session_volume(&events).is_empty());
    }

    #[test]
    fn detect_patterns_deduplicates() {
        let events = vec![
            drift_event("foo", Duration::hours(1)),
            drift_event("foo", Duration::hours(2)),
            drift_event("foo", Duration::hours(3)),
            drift_event("foo", Duration::hours(4)),
        ];
        let insights = detect_patterns(&events, &[]);
        // Only one recurring-drift insight for slug "foo", even with 4 events.
        let count = insights
            .iter()
            .filter(|i| i.kind == InsightKind::RecurringDrift)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn insight_serializes_round_trip() {
        let events = vec![
            drift_event("foo", Duration::hours(1)),
            drift_event("foo", Duration::hours(2)),
            drift_event("foo", Duration::hours(3)),
        ];
        let insights = detect_recurring_drift(&events);
        let json = serde_json::to_string(&insights[0]).unwrap();
        let back: Insight = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, InsightKind::RecurringDrift);
        assert_eq!(back.evidence.len(), 3);
        assert_eq!(back.importance, Importance::High);
    }

    #[test]
    fn writer_writes_file_with_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let events = vec![
            drift_event("foo", Duration::hours(1)),
            drift_event("foo", Duration::hours(2)),
            drift_event("foo", Duration::hours(3)),
        ];
        let insights = detect_recurring_drift(&events);
        let dest = writer::write_insights_markdown(&insights, tmp.path()).unwrap();
        assert!(dest.exists());
        let body = std::fs::read_to_string(&dest).unwrap();
        assert!(body.starts_with("---\n"));
        assert!(body.contains("kind: auto-insight"));
        assert!(body.contains("generated_by: altevra-observer"));
        assert!(body.contains("count: 1"));
        assert!(body.contains("Recurring drift: foo"));
        // Path matches expected pattern.
        let name = dest.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("auto-") && name.ends_with(".md"));
    }

    #[test]
    fn writer_overwrites_existing_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        // First write — empty.
        let dest1 = writer::write_insights_markdown(&[], tmp.path()).unwrap();
        let body1 = std::fs::read_to_string(&dest1).unwrap();
        assert!(body1.contains("count: 0"));

        // Second write — non-empty, same day so same filename.
        let events = vec![
            drift_event("bar", Duration::hours(1)),
            drift_event("bar", Duration::hours(2)),
            drift_event("bar", Duration::hours(3)),
        ];
        let insights = detect_recurring_drift(&events);
        let dest2 = writer::write_insights_markdown(&insights, tmp.path()).unwrap();
        assert_eq!(dest1, dest2);
        let body2 = std::fs::read_to_string(&dest2).unwrap();
        assert!(body2.contains("count: 1"));
        assert!(body2.contains("Recurring drift: bar"));
        // No leftover tmp file.
        let leftover = std::fs::read_dir(tmp.path().join("10-insights"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.ends_with(".tmp"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(leftover, 0);
    }

    #[test]
    fn list_insight_files_returns_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("10-insights");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("auto-20260101.md"), "x").unwrap();
        std::fs::write(dir.join("auto-20260301.md"), "x").unwrap();
        std::fs::write(dir.join("auto-20260201.md"), "x").unwrap();
        // Non-matching file should be ignored.
        std::fs::write(dir.join("manual.md"), "x").unwrap();
        let listed = writer::list_insight_files(tmp.path()).unwrap();
        assert_eq!(listed.len(), 3);
        let names: Vec<String> = listed
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names[0], "auto-20260301.md");
        assert_eq!(names[2], "auto-20260101.md");
    }

    #[test]
    fn skill_version_divergence_multi_versions() {
        let mut ev1 = Event::new(
            EventType::SkillInstalled,
            "install",
            "test",
            ActorType::System,
        )
        .with_entity("skill", "altevra-core")
        .with_payload(serde_json::json!({"version": "1.0.0"}));
        ev1.created_at = Utc::now() - Duration::hours(2);

        let mut ev2 = Event::new(
            EventType::SkillInstalled,
            "install",
            "test",
            ActorType::System,
        )
        .with_entity("skill", "altevra-core")
        .with_payload(serde_json::json!({"version": "1.1.0"}));
        ev2.created_at = Utc::now() - Duration::hours(1);

        let insights = detect_skill_version_divergence(&[ev1, ev2]);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::SkillVersionDivergence);
        assert!(insights[0].summary.contains("1.0.0"));
        assert!(insights[0].summary.contains("1.1.0"));
    }
}
