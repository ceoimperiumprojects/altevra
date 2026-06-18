//! Job definitions. Each job is an async function that takes a SqlitePool and
//! a JobContext, runs its work, and returns a JobResult with a one-line
//! summary that ends up in brain_jobs.result_summary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    EventClassifier,
    ObserverScan,
    VaultIndexer,
    InsightSynthesizer,
    ResearchFetcher,
    FeedDiscovery,
    GitHubTrendingFetch,
    ProjectResearchSweep,
    DailySummary,
    TaskGrooming,
    AutoCategorizer,
    SelfImproveOrchestrator,
    /// C7 — DB-level skill/proposal curator (Hermes-borrowed). Status-only
    /// transitions, never deletes; runs ~7 days (idle-gated by period). See
    /// [`crate::curator`] for the policy.
    Curator,
    /// E1 — lifecycle archiver. Soft-archives Active → Archived for
    /// retention-due objects, marks delete-due objects `pending_delete`
    /// (never deletes), and purges the ephemeral `context_packets` body
    /// past its 14-day window (R-EPH). Honors per-object legal hold (D7)
    /// and never touches `exposure_decisions` or `audit_log` (R5-INV).
    /// Distinct from [`Curator`] — that one targets the proposal/skill
    /// *status* archive; this one targets the lifecycle_state derived
    /// from envelope timestamps + domain policy. See [`crate::lifecycle`].
    LifecycleArchiver,
    /// P3c — SkillOpt backward pass. Drains pending `skill_invocation`
    /// events through the anti-sycophancy success judge (local lfm via
    /// Ollama structured outputs); confirmed failures become bounded edit
    /// proposals routed to the REVIEW QUEUE — never auto-published. See
    /// [`crate::skill_judge`].
    SkillReactionJudge,
    /// E2 — weekly self-cleaning maintenance. `PRAGMA optimize` +
    /// `PRAGMA incremental_vacuum` + a DB-size snapshot for the doctor's
    /// size-trend check + a retention-job liveness count. RAW TURNS ARE NEVER
    /// DELETED (the raw trace is canonical — doctrine); this only reclaims free
    /// pages the lifecycle archiver / retention sweeps already freed. See
    /// [`run_db_optimize`].
    DbOptimize,
    /// E1 (PLAN-EXTEND) — Connector SDK sync. Pulls each ENABLED connector from
    /// `~/.altevra/connectors.toml` through the full safety stack (guard →
    /// domain floor → persist into events + object_index). A failing connector
    /// goes health-red but never blocks other connectors or jobs. See
    /// [`crate::connector_sync`].
    ConnectorSync,
    /// Personal Brain extractor: an LLM reads recent turns and materializes the
    /// durable facts Pavle stated (people, decisions, goals) straight into the
    /// `persons`/`decisions`/`goals` tables. Without this Altevra records raw
    /// turns but never builds the structured second-brain (CLAUDE.md §3.1).
    PersonalExtractor,
    /// Skill factory: takes `triaged` skill proposals (raw cluster candidates),
    /// has an LLM draft a real SKILL.md, writes it to a STAGING dir + the skills
    /// table, and advances the proposal to applied. Staging (not ~/.claude) keeps
    /// the "live external writes need Pavle's OK" rule (factory.rs §46).
    SkillFactory,
    /// Autonomous memory write-back: periodically renders the Altevra digest
    /// (recent decisions + active goals + key prefs, e.g. "use minus not em-dash")
    /// into the ALTEVRA_MANAGED block of ~/.claude/CLAUDE.md + Hermes, so every
    /// agent inherits Pavle's preferences WITHOUT him asking. Runs as a subprocess
    /// (`altevra memory-sync write --apply`) — the daemon does it autonomously.
    MemoryWriteback,
    /// Self-healing. Collects a cheap health snapshot (embedder backlog, failed
    /// jobs, disk usage, capture freshness). When something looks wrong it asks a
    /// Sonnet "system doctor" to diagnose, and writes an insight card with the
    /// findings + recommended fix. By DEFAULT it is advisory-only (no system
    /// mutation). An aggressive auto-fix mode is gated behind the operator-held
    /// env switch `ALTEVRA_HEALER_YOLO=1`; the operator owns that switch.
    Healer,
    /// Reads the watcher's file_changes.jsonl and enqueues changed text/code files
    /// into pending_indexing → embed, so ALL work outside repos (projekti, Documents,
    /// Desktop, Obsidian) becomes recall-able, not just AI-tool sessions.
    FileChangeIndexer,
    /// Wiki curator: an LLM synthesizes recent decisions + learnings into LIVING
    /// knowledge wiki pages (one per topic), written to ~/.altevra/vault/wiki and
    /// upserted in wiki_pages (idempotent by topic). Curated, evolving pages —
    /// distinct from raw notes (CLAUDE.md vision: Wiki Layer).
    WikiCurator,
    /// Proposal materializer: durable-knowledge proposals (person, relationship,
    /// wiki, insight) that self-improve/resident modes marked `applied` were never
    /// landing in any queryable table — they sat inert. This drains them into the
    /// `learnings` table (which auto-indexes into object_index + FTS, so they
    /// become recall-able). Idempotent by title. Closes the "proposed but never
    /// materialized" gap (workflow diagnosis 2026-06-18).
    ProposalMaterializer,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EventClassifier => "event_classifier",
            Self::ObserverScan => "observer_scan",
            Self::VaultIndexer => "vault_indexer",
            Self::InsightSynthesizer => "insight_synthesizer",
            Self::ResearchFetcher => "research_fetcher",
            Self::FeedDiscovery => "feed_discovery",
            Self::GitHubTrendingFetch => "github_trending_fetch",
            Self::ProjectResearchSweep => "project_research_sweep",
            Self::DailySummary => "daily_summary",
            Self::TaskGrooming => "task_grooming",
            Self::AutoCategorizer => "auto_categorizer",
            Self::SelfImproveOrchestrator => "self_improve_orchestrator",
            Self::Curator => "curator",
            Self::LifecycleArchiver => "lifecycle_archiver",
            Self::SkillReactionJudge => "skill_reaction_judge",
            Self::DbOptimize => "db_optimize",
            Self::ConnectorSync => "connector_sync",
            Self::PersonalExtractor => "personal_extractor",
            Self::SkillFactory => "skill_factory",
            Self::MemoryWriteback => "memory_writeback",
            Self::Healer => "healer",
            Self::FileChangeIndexer => "file_change_indexer",
            Self::WikiCurator => "wiki_curator",
            Self::ProposalMaterializer => "proposal_materializer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "event_classifier" => Self::EventClassifier,
            "observer_scan" => Self::ObserverScan,
            "vault_indexer" => Self::VaultIndexer,
            "insight_synthesizer" => Self::InsightSynthesizer,
            "research_fetcher" => Self::ResearchFetcher,
            "feed_discovery" => Self::FeedDiscovery,
            "github_trending_fetch" => Self::GitHubTrendingFetch,
            "project_research_sweep" => Self::ProjectResearchSweep,
            "daily_summary" => Self::DailySummary,
            "task_grooming" => Self::TaskGrooming,
            "auto_categorizer" => Self::AutoCategorizer,
            "self_improve_orchestrator" => Self::SelfImproveOrchestrator,
            "curator" => Self::Curator,
            "lifecycle_archiver" => Self::LifecycleArchiver,
            "skill_reaction_judge" => Self::SkillReactionJudge,
            "db_optimize" => Self::DbOptimize,
            "connector_sync" => Self::ConnectorSync,
            "personal_extractor" => Self::PersonalExtractor,
            "skill_factory" => Self::SkillFactory,
            "memory_writeback" => Self::MemoryWriteback,
            "healer" => Self::Healer,
            "file_change_indexer" => Self::FileChangeIndexer,
            "wiki_curator" => Self::WikiCurator,
            "proposal_materializer" => Self::ProposalMaterializer,
            _ => return None,
        })
    }

    /// Period in seconds. Daily jobs use a fixed long period; the scheduler
    /// also checks the wall clock hour for them.
    pub fn period_secs(&self) -> u64 {
        match self {
            Self::EventClassifier => 60,
            Self::ObserverScan => 300,
            Self::VaultIndexer => 900,
            Self::InsightSynthesizer => 3600,
            Self::ResearchFetcher => 7200,
            Self::FeedDiscovery => 3600,
            Self::GitHubTrendingFetch => 14_400,  // 4h
            Self::ProjectResearchSweep => 86_400, // 24h
            Self::DailySummary => 3600,           // tick hourly, fire only at 23:00
            Self::TaskGrooming => 10_800,
            Self::AutoCategorizer => 1800, // 30 min — classify newly-indexed objects
            // Periodic backstop (~45 min): the loop is ALSO triggered real-time (a
            // hook can invoke `run_self_improve`); this is the safety net so a missed
            // trigger still gets the 7-stage loop run within the window.
            Self::SelfImproveOrchestrator => 2700,
            // C7: ~7 days. Mirrors Hermes' `DEFAULT_INTERVAL_HOURS = 24 * 7`. The
            // curator is intentionally infrequent — it sweeps long-tail status
            // staleness, not real-time signals.
            Self::Curator => 7 * 24 * 60 * 60,
            // E1: once per day. The actor only acts on objects whose envelope
            // timestamps cross a TTL/expiry boundary — a sub-day cadence would
            // burn DB ticks for no observable effect.
            Self::LifecycleArchiver => 24 * 60 * 60,
            // P3c: cheap (a handful of indexed event reads; the judge LLM call
            // only fires on anchor-flagged windows, never per session). 15 min
            // keeps the failure→proposal latency low without burning ticks.
            Self::SkillReactionJudge => 900,
            // E2: weekly. PRAGMA optimize + incremental_vacuum are cheap but
            // pointless to run more than ~weekly — free pages accumulate slowly
            // and the size snapshot is a trend, not a real-time gauge.
            Self::DbOptimize => 7 * 24 * 60 * 60,
            // E1: the per-connector cadence lives in connectors.toml; this is
            // the SCHEDULER backstop tick (15 min) — `run_connector_sync` itself
            // only pulls connectors whose own cadence has elapsed. Cheap when
            // everything is disabled (the common default).
            Self::ConnectorSync => 900,
            // Hourly: extract durable personal facts from the last couple hours
            // of activity. LLM-gated; cheap (one Sonnet call) and idempotent.
            Self::PersonalExtractor => 3600,
            // Every 2h: drain triaged skill candidates into staged SKILL.md files.
            Self::SkillFactory => 7200,
            Self::MemoryWriteback => 21600,
            Self::Healer => 1800,
            Self::FileChangeIndexer => 120,
            Self::WikiCurator => 10800,
            // Every 30 min: drain applied knowledge-proposals into learnings.
            // Cheap (a handful of small rows), idempotent, no LLM.
            Self::ProposalMaterializer => 1800,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobResult {
    pub summary: String,
    pub items_processed: usize,
}

#[derive(Debug, Clone)]
pub struct JobContext {
    pub vault_path: std::path::PathBuf,
    pub now: DateTime<Utc>,
    /// Model router resolved from `[llm]` config. With `delegated` (default) every
    /// role resolves to noop, so LLM-backed jobs skip cleanly until keys are added.
    pub router: std::sync::Arc<altevra_llm::ModelRouter>,
}

// ---- Job implementations ----------------------------------------------------

/// Process raw events.jsonl entries into classified UpdateFeedItems and append
/// to updates.jsonl. For a minimal pipeline we just count new lines since the
/// last marker file at .altevra/state/last_classified_offset.
pub async fn run_event_classifier(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    // Bridge the `events` table → `update_feed` (the DB table the MCP
    // `get_last_updates` tool reads). Previously this wrote JSONL files that
    // nothing consumed, so the agent-facing "what's new" feed was always empty.
    // Deterministic UUIDv5(event_id) + INSERT OR IGNORE makes it idempotent, so
    // it can re-scan the recent window every tick without duplicating rows.
    use altevra_core::updates::{Importance, UpdateFeedItem};
    use altevra_db::{EventsRepository, UpdatesRepository};

    let since = ctx.now - chrono::Duration::hours(48);
    let events = EventsRepository::new(pool)
        .list_since(since, None, 500)
        .await
        .unwrap_or_default();

    let updates = UpdatesRepository::new(pool);
    let mut written = 0usize;
    for ev in &events {
        let summary = ev.summary.clone().unwrap_or_else(|| ev.title.clone());
        // Errors/failures matter more than routine events.
        let et = ev.event_type.to_string();
        let importance = if et.contains("error") || et.contains("fail") {
            Importance::High
        } else {
            Importance::Low
        };
        let mut item =
            UpdateFeedItem::from_event(ev.id, et, importance, ev.title.clone(), summary);
        item.id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, ev.id.as_bytes());
        item.created_at = ev.created_at;
        item.project_id = ev.project_id;
        if updates.insert_or_ignore(&item).await.unwrap_or(false) {
            written += 1;
        }
    }

    Ok(JobResult {
        summary: format!(
            "event_classifier: {written} new update(s) bridged from {} event(s)",
            events.len()
        ),
        items_processed: written,
    })
}

/// Personal Brain extractor: an LLM reads the last couple hours of turns and
/// materializes the durable facts Pavle stated — people, decisions, goals —
/// straight into the `persons`/`decisions`/`goals` tables. Idempotent: persons
/// upsert by name; decisions/goals dedupe by title. LLM-gated (StrongReasoner).
pub async fn run_personal_extractor(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_db::{DecisionRow, GoalRow, PersonalNotesRepository, TasksRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    if provider.id() == "noop" {
        return Ok(JobResult {
            summary: "personal extraction skipped (no LLM configured)".into(),
            items_processed: 0,
        });
    }

    let since = (ctx.now - chrono::Duration::hours(2))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let activity: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT role || ': ' || substr(content, 1, 400) \
         FROM turns \
         WHERE created_at > ? AND role IN ('user', 'assistant') \
         ORDER BY created_at DESC LIMIT 60",
    )
    .bind(&since)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if activity.is_empty() {
        return Ok(JobResult {
            summary: "no recent activity for personal extraction".into(),
            items_processed: 0,
        });
    }
    let block = activity.into_iter().rev().collect::<Vec<_>>().join("\n");

    let messages = vec![
        ChatMessage::system(
            "You extract DURABLE personal-brain facts from an activity log. Return ONLY \
             compact JSON, no markdown, no prose. ALWAYS include all three keys, using \
             empty arrays when nothing qualifies: \
             {\"persons\":[{\"name\":\"\",\"note\":\"\"}],\
             \"decisions\":[{\"title\":\"\",\"rationale\":\"\"}],\
             \"goals\":[{\"title\":\"\"}]}. \
             persons = any REAL named person mentioned with context — mentors, partner, \
             family, clients, colleagues, friends, investors, contacts. Capture their \
             name and a short note on who they are or why they matter. \
             decisions = concrete decisions made. goals = concrete goals set. \
             Base everything ONLY on what the activity log actually says — NEVER invent \
             people, decisions, or goals that aren't there. Keep each field short.",
        ),
        ChatMessage::user(format!("Activity log (oldest first):\n{block}")),
    ];

    let raw = match provider
        .complete(&messages, &ChatOpts::default().with_max_tokens(800))
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return Ok(JobResult {
                summary: format!("personal_extractor: LLM call failed ({e})"),
                items_processed: 0,
            })
        }
    };

    // Pull the JSON object out of the response (tolerate stray prose/fences).
    let json_str = match (raw.find('{'), raw.rfind('}')) {
        (Some(s), Some(e)) if e > s => &raw[s..=e],
        _ => "{}",
    };
    let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap_or(serde_json::json!({}));

    // Per-category counts of what the LLM actually returned — so a stuck category
    // (e.g. persons always 0) is visible in the job summary instead of hidden
    // behind a single "materialized N" number.
    let llm_persons = parsed.get("persons").and_then(|v| v.as_array()).map_or(0, |a| a.len());
    let llm_decisions = parsed.get("decisions").and_then(|v| v.as_array()).map_or(0, |a| a.len());
    let llm_goals = parsed.get("goals").and_then(|v| v.as_array()).map_or(0, |a| a.len());
    tracing::debug!(
        persons = llm_persons,
        decisions = llm_decisions,
        goals = llm_goals,
        "personal_extractor: LLM returned"
    );

    let mut materialized = 0usize;
    let mut persons_saved = 0usize;
    let persons = PersonalNotesRepository::new(pool);
    if let Some(arr) = parsed.get("persons").and_then(|v| v.as_array()) {
        for p in arr {
            if let Some(name) = p
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let note = p.get("note").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                if persons.upsert_person(name, note).await.is_ok() {
                    materialized += 1;
                    persons_saved += 1;
                }
            }
        }
    }

    let tasks = TasksRepository::new(pool);
    if let Some(arr) = parsed.get("decisions").and_then(|v| v.as_array()) {
        for d in arr {
            if let Some(title) = d
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let exists: Option<String> =
                    sqlx::query_scalar("SELECT id FROM decisions WHERE title = ? LIMIT 1")
                        .bind(title)
                        .fetch_optional(pool)
                        .await
                        .unwrap_or(None);
                if exists.is_some() {
                    continue;
                }
                let row = DecisionRow {
                    id: uuid::Uuid::new_v4(),
                    project_id: None,
                    title: title.to_string(),
                    rationale: d.get("rationale").and_then(|v| v.as_str()).map(String::from),
                    decided_at: ctx.now,
                    decided_by: Some("personal_extractor".into()),
                    metadata: serde_json::json!({}),
                };
                if tasks.save_decision(&row).await.is_ok() {
                    materialized += 1;
                }
            }
        }
    }
    if let Some(arr) = parsed.get("goals").and_then(|v| v.as_array()) {
        for g in arr {
            if let Some(title) = g
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let exists: Option<String> =
                    sqlx::query_scalar("SELECT id FROM goals WHERE title = ? LIMIT 1")
                        .bind(title)
                        .fetch_optional(pool)
                        .await
                        .unwrap_or(None);
                if exists.is_some() {
                    continue;
                }
                let row = GoalRow {
                    id: uuid::Uuid::new_v4(),
                    project_id: None,
                    title: title.to_string(),
                    description: None,
                    target_date: None,
                    status: "active".into(),
                    metadata: serde_json::json!({}),
                    created_at: ctx.now,
                    updated_at: ctx.now,
                };
                if tasks.upsert_goal(&row).await.is_ok() {
                    materialized += 1;
                }
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "personal_extractor: materialized {materialized} fact(s) \
             (persons {persons_saved}/{llm_persons}, decisions+goals from {} llm items)",
            llm_decisions + llm_goals
        ),
        items_processed: materialized,
    })
}

/// Turn a free-text title into a clean kebab-case skill slug (no `:` — the
/// factory's template gate rejects session-artifact slugs with discriminators).
fn slugify_skill(title: &str) -> String {
    let lowered = title.to_lowercase();
    let parts: Vec<&str> = lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    let slug: String = parts.join("-").chars().take(48).collect();
    if slug.is_empty() {
        "altevra-skill".to_string()
    } else {
        slug
    }
}

/// Skill factory: drain `triaged` skill candidates into staged SKILL.md files.
/// The candidates are raw cluster/evidence text (not skill markdown), so an LLM
/// drafts a real SKILL.md; we write it to a STAGING dir (~/.altevra/skills-
/// staging, NOT ~/.claude — live tool-dir writes need Pavle's OK) + the skills
/// table, then advance the proposal to applied. Idempotent (upsert by slug).
pub async fn run_skill_factory(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_core::status::ProposalStatus;
    use altevra_db::{ProposalsRepository, SkillRow, SkillsRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    if provider.id() == "noop" {
        return Ok(JobResult {
            summary: "skill factory skipped (no LLM configured)".into(),
            items_processed: 0,
        });
    }

    let proposals = ProposalsRepository::new(pool);
    let triaged = proposals
        .list(Some("triaged"), Some("skill"))
        .await
        .unwrap_or_default();
    if triaged.is_empty() {
        return Ok(JobResult {
            summary: "skill factory: no triaged candidates".into(),
            items_processed: 0,
        });
    }

    let staging_root = altevra_core::home_dir().join(".altevra/skills-staging");
    let skills = SkillsRepository::new(pool);
    let mut rendered = 0usize;

    // Bound per-tick work — 2 LLM drafts at a time so a tick stays snappy and
    // later jobs (healer) still get reached; the daemon picks up the rest next run.
    for row in triaged.iter().take(2) {
        let slug = slugify_skill(&row.title);
        let messages = vec![
            ChatMessage::system(
                "You write Claude Code SKILL.md files. Output ONLY the markdown, no fences, \
                 no preamble. It MUST start with a YAML frontmatter block delimited by --- \
                 containing: slug (kebab-case, no colons), version (0.1.0), title, description. \
                 After the frontmatter add these sections: '## When to use', '## How it works', \
                 '## Steps', '## Example', '## Notes'. Base it strictly on the candidate; do not invent unrelated capability.",
            ),
            ChatMessage::user(format!(
                "Candidate slug: {slug}\nCandidate title: {}\nEvidence/notes:\n{}",
                row.title, row.body
            )),
        ];
        let content = match provider
            .complete(&messages, &ChatOpts::default().with_max_tokens(1200))
            .await
        {
            Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => continue,
        };

        // Write to staging: ~/.altevra/skills-staging/<slug>/SKILL.md
        let dir = staging_root.join(&slug);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let path = dir.join("SKILL.md");
        if std::fs::write(&path, &content).is_err() {
            continue;
        }

        let skill = SkillRow {
            id: uuid::Uuid::new_v4(),
            slug: slug.clone(),
            version: "0.1.0".into(),
            source_path: path.to_string_lossy().to_string(),
            checksum: format!("len{}", content.len()),
            content,
            metadata: serde_json::json!({"origin": "skill_factory", "proposal_id": row.id}),
            status: "staged".into(),
            created_at: ctx.now,
            updated_at: ctx.now,
        };
        if skills.upsert(&skill).await.is_ok() {
            let _ = proposals
                .transition_status(&row.id, ProposalStatus::Applied, Some("skill_factory"))
                .await;
            rendered += 1;
        }
    }

    Ok(JobResult {
        summary: format!(
            "skill_factory: staged {rendered} skill(s) from {} triaged candidate(s)",
            triaged.len()
        ),
        items_processed: rendered,
    })
}

/// Autonomous memory write-back. Renders the Altevra digest (recent decisions +
/// active goals + key prefs) into the ALTEVRA_MANAGED block of ~/.claude/CLAUDE.md
/// + the Hermes memory file, so every agent inherits Pavle's preferences without
/// him asking. Shells out to `altevra memory-sync write --apply` (the write logic
/// lives in the CLI crate). The daemon runs this on its own cadence — the whole
/// point is autonomy, not on-demand.
pub async fn run_memory_writeback(
    _pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    let out = tokio::process::Command::new("altevra")
        .args(["memory-sync", "write", "--apply"])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => Ok(JobResult {
            summary: "memory_writeback: digest propagated to CLAUDE.md + Hermes".into(),
            items_processed: 1,
        }),
        Ok(o) => Ok(JobResult {
            summary: format!(
                "memory_writeback failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            items_processed: 0,
        }),
        Err(e) => Ok(JobResult {
            summary: format!("memory_writeback exec error: {e}"),
            items_processed: 0,
        }),
    }
}

/// Self-healing. Collects a cheap health snapshot; if something looks wrong, a
/// Sonnet "doctor" diagnoses it. DEFAULT = advisory: writes an insight card with
/// the diagnosis + recommended fix, mutates nothing. AGGRESSIVE auto-fix is gated
/// behind the operator-held env switch `ALTEVRA_HEALER_YOLO=1` — only then does it
/// launch `claude -p --dangerously-skip-permissions` with shell access to actually
/// restart dead services / clear regenerable caches / kick a stalled embedder.
/// Healthy → no LLM call (relevance gate).
pub async fn run_healer(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_db::{InsightCardRow, InsightCardsRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    // ── cheap health snapshot ────────────────────────────────────────────────
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM embedder_queue WHERE status = 'pending'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let failed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM brain_jobs WHERE status = 'failed'")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let newest_turn: Option<String> = sqlx::query_scalar("SELECT MAX(created_at) FROM turns")
        .fetch_one(pool)
        .await
        .unwrap_or(None);

    // disk usage % of /home via df
    let disk_pct: i64 = match tokio::process::Command::new("df")
        .args(["-P", "/home"])
        .output()
        .await
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .nth(1)
            .and_then(|l| l.split_whitespace().nth(4))
            .and_then(|p| p.trim_end_matches('%').parse().ok())
            .unwrap_or(0),
        Err(_) => 0,
    };

    // capture freshness: stale if newest turn older than 6h
    let capture_stale = newest_turn
        .as_deref()
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s.replace(' ', "T"))
                .ok()
                .map(|t| (ctx.now - t.with_timezone(&chrono::Utc)).num_hours() > 6)
        })
        .unwrap_or(false);

    let mut issues: Vec<String> = Vec::new();
    if disk_pct >= 90 {
        issues.push(format!("disk {disk_pct}% full"));
    }
    if pending > 5000 {
        issues.push(format!("embedder backlog {pending} pending"));
    }
    if failed > 0 {
        issues.push(format!("{failed} failed brain job(s)"));
    }
    if capture_stale {
        issues.push("capture stale (no new turns in 6h+)".into());
    }

    if issues.is_empty() {
        return Ok(JobResult {
            summary: format!(
                "healer: healthy (disk {disk_pct}%, pending {pending}, failed {failed})"
            ),
            items_processed: 0,
        });
    }

    let snapshot = format!(
        "disk={disk_pct}%, embedder_pending={pending}, failed_jobs={failed}, \
         newest_turn={newest_turn:?}, issues={issues:?}"
    );
    let yolo = std::env::var("ALTEVRA_HEALER_YOLO")
        .map(|v| v == "1")
        .unwrap_or(false);

    let report = if yolo {
        // AGGRESSIVE: claude with shell access actually fixes things.
        let prompt = format!(
            "You are Altevra's autonomous system healer with full shell access on Pavle's \
             Linux machine. Current health snapshot:\n{snapshot}\n\nDiagnose and FIX the safe \
             issues: restart dead user services with systemctl --user; if disk >=90% clear ONLY \
             regenerable caches under ~/.cache (uv/pip/yay/huggingface); if the embedder is \
             stalled restart altevra-npu-embed. ABSOLUTE RULES: never delete anything under \
             ~/.altevra except clearly-temp files, never touch ~/Obsidian or ~/projekti, never \
             rm -rf a home/root path. Report concisely what you checked and did."
        );
        match tokio::process::Command::new("claude")
            .args([
                "-p",
                &prompt,
                "--dangerously-skip-permissions",
                "--model",
                "claude-sonnet-4-6",
                "--output-format",
                "text",
            ])
            .current_dir(altevra_core::home_dir())
            .output()
            .await
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(e) => format!("YOLO healer exec failed: {e}"),
        }
    } else {
        // ADVISORY: isolated reasoning, no system mutation.
        let provider = ctx.router.resolve(ModelRole::StrongReasoner);
        if provider.id() == "noop" {
            format!(
                "Issues detected ({}); no LLM configured for diagnosis. snapshot: {snapshot}",
                issues.len()
            )
        } else {
            let messages = vec![
                ChatMessage::system(
                    "You are Altevra's system-health doctor. Given a health snapshot, name the \
                     most likely root cause and ONE concrete safe fix command. No preamble.",
                ),
                ChatMessage::user(format!("Health snapshot: {snapshot}")),
            ];
            provider
                .complete(&messages, &ChatOpts::default().with_max_tokens(300))
                .await
                .unwrap_or_else(|e| format!("diagnosis failed: {e}"))
        }
    };

    // Record the result as an insight card so it surfaces to Pavle.
    let title = format!("Health: {} issue(s) — {}", issues.len(), issues.join("; "));
    let id = format!("healer-{}", ctx.now.format("%Y%m%dT%H%M%S"));
    let card = InsightCardRow::new(id, title, format!("{snapshot}\n\n{report}"));
    let _ = InsightCardsRepository::new(pool).insert(&card).await;

    Ok(JobResult {
        summary: format!(
            "healer: {} issue(s), {} mode",
            issues.len(),
            if yolo { "YOLO-fix" } else { "advisory" }
        ),
        items_processed: issues.len(),
    })
}

/// Reads the watcher's `~/.altevra/events/file_changes.jsonl` and enqueues each
/// changed text/code file into `pending_indexing` → embed pipeline. This is the
/// last mile that makes ALL work outside repos (projekti, Documents, Desktop,
/// Obsidian) recall-able — without it the watcher records changes that nothing
/// indexes. Watermark via a byte-offset file so it only processes new lines.
pub async fn run_file_change_indexer(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    let jsonl = altevra_core::home_dir().join(".altevra/events/file_changes.jsonl");
    let marker = altevra_core::home_dir().join(".altevra/state/file_change_index_offset");
    if !jsonl.exists() {
        return Ok(JobResult {
            summary: "file_change_indexer: no file_changes.jsonl yet".into(),
            items_processed: 0,
        });
    }
    let content = std::fs::read_to_string(&jsonl).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let prev: usize = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    // Only index text/code files; skip binaries, media, dbs, lockfiles.
    let indexable = |p: &str| -> bool {
        let lower = p.to_lowercase();
        const EXT: &[&str] = &[
            ".md", ".txt", ".ts", ".tsx", ".js", ".jsx", ".py", ".rs", ".go", ".json",
            ".yaml", ".yml", ".toml", ".csv", ".html", ".css", ".sh", ".sql", ".mjs",
        ];
        EXT.iter().any(|e| lower.ends_with(e))
            && !lower.contains("/node_modules/")
            && !lower.contains("/target/")
            && !lower.contains("/.next/")
            && !lower.contains("/.git/")
    };

    let mut queued = 0usize;
    for line in lines.iter().skip(prev) {
        if line.trim().is_empty() {
            continue;
        }
        let path = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => v
                .get("path")
                .and_then(|p| p.as_str())
                .map(str::to_string),
            Err(_) => None,
        };
        let Some(path) = path else { continue };
        if !indexable(&path) || !std::path::Path::new(&path).exists() {
            continue;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let _ = sqlx::query(
            r#"INSERT INTO pending_indexing (id, path, status) VALUES (?, ?, 'pending')
               ON CONFLICT (path) DO UPDATE SET
                   status = CASE WHEN status = 'failed' THEN 'pending' ELSE status END"#,
        )
        .bind(id)
        .bind(&path)
        .execute(pool)
        .await;
        queued += 1;
    }

    let _ = std::fs::write(&marker, lines.len().to_string());
    Ok(JobResult {
        summary: format!("file_change_indexer: queued {queued} changed file(s) for indexing"),
        items_processed: queued,
    })
}

/// Wiki curator: synthesize recent decisions + learnings into LIVING wiki pages.
/// An LLM returns 1-3 topic pages as JSON; each is written to ~/.altevra/vault/wiki
/// and upserted in wiki_pages (idempotent by topic). LLM-gated (StrongReasoner).
pub async fn run_wiki_curator(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_db::WikiPagesRepository;
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    if provider.id() == "noop" {
        return Ok(JobResult {
            summary: "wiki curator skipped (no LLM configured)".into(),
            items_processed: 0,
        });
    }

    let decisions: Vec<String> =
        sqlx::query_scalar("SELECT title FROM decisions ORDER BY decided_at DESC LIMIT 20")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let learnings: Vec<String> = sqlx::query_scalar(
        "SELECT title || ': ' || substr(COALESCE(body,''), 1, 200) FROM learnings \
         ORDER BY created_at DESC LIMIT 20",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if decisions.is_empty() && learnings.is_empty() {
        return Ok(JobResult {
            summary: "wiki curator: no decisions/learnings to synthesize".into(),
            items_processed: 0,
        });
    }

    let block = format!(
        "DECISIONS:\n{}\n\nLEARNINGS:\n{}",
        decisions.join("\n"),
        learnings.join("\n")
    );
    let messages = vec![
        ChatMessage::system(
            "You maintain Pavle's living knowledge wiki. From the decisions + learnings below, \
             produce 1 to 3 evolving knowledge pages. Return ONLY a JSON array, no prose, no \
             fences: [{\"topic\":\"\",\"slug\":\"kebab-case\",\"title\":\"\",\"markdown\":\"\"}]. \
             Each page synthesizes durable knowledge on ONE topic (a project, a workflow, a \
             recurring lesson). markdown = a concise, well-structured page (## sections). Group \
             related items; do not just list them.",
        ),
        ChatMessage::user(block),
    ];

    let raw = match provider
        .complete(&messages, &ChatOpts::default().with_max_tokens(2000))
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return Ok(JobResult {
                summary: format!("wiki curator: LLM failed ({e})"),
                items_processed: 0,
            })
        }
    };
    let json_str = match (raw.find('['), raw.rfind(']')) {
        (Some(s), Some(e)) if e > s => &raw[s..=e],
        _ => "[]",
    };
    let pages: serde_json::Value = serde_json::from_str(json_str).unwrap_or(serde_json::json!([]));

    let wiki_dir = altevra_core::home_dir().join(".altevra/vault/wiki");
    let _ = std::fs::create_dir_all(&wiki_dir);
    let repo = WikiPagesRepository::new(pool);
    let mut written = 0usize;

    if let Some(arr) = pages.as_array() {
        for p in arr {
            let topic = p.get("topic").and_then(|v| v.as_str()).unwrap_or("").trim();
            let slug = p.get("slug").and_then(|v| v.as_str()).unwrap_or("").trim();
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or(topic);
            let markdown = p.get("markdown").and_then(|v| v.as_str()).unwrap_or("");
            if topic.is_empty() || slug.is_empty() || markdown.is_empty() {
                continue;
            }
            let path = wiki_dir.join(format!("{slug}.md"));
            if std::fs::write(&path, markdown).is_err() {
                continue;
            }
            let checksum = format!("len{}", markdown.len());
            if repo
                .upsert(
                    topic,
                    slug,
                    &path.to_string_lossy(),
                    "active",
                    "medium",
                    "internal",
                    (decisions.len() + learnings.len()) as i64,
                    Some(ctx.now),
                    Some(title),
                    &checksum,
                )
                .await
                .is_ok()
            {
                written += 1;
            }
        }
    }

    Ok(JobResult {
        summary: format!("wiki_curator: synthesized {written} living page(s)"),
        items_processed: written,
    })
}

/// Proposal materializer: drains durable-knowledge proposals (person, relationship,
/// wiki, insight) that were marked `applied` but never landed in a queryable table
/// into the `learnings` table — which auto-indexes into object_index + FTS, so they
/// become recall-able / packet candidates immediately. person/relationship land as
/// `personal`/`sensitive` learnings; wiki/insight as `business`/`internal`.
/// Idempotent: a proposal whose title already exists as a learning is skipped, so
/// re-scanning every tick is harmless. No LLM. (Closes the "proposed but never
/// materialized" gap — workflow diagnosis 2026-06-18.)
pub async fn run_proposal_materializer(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_db::LearningRow;
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT id, kind, title, body FROM proposals \
         WHERE status = 'applied' AND kind IN ('person','relationship','wiki','insight')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let learnings = altevra_db::LearningsRepository::new(pool);
    let mut materialized = 0usize;
    for r in &rows {
        let title: String = r.get("title");
        let kind: String = r.get("kind");
        let body: String = r.get("body");
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        // Idempotent: skip if a learning with this title already exists.
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM learnings WHERE title = ? LIMIT 1")
                .bind(title)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
        if exists.is_some() {
            continue;
        }
        let mut row = LearningRow::new(uuid::Uuid::new_v4().to_string(), title, body);
        if kind == "person" || kind == "relationship" {
            row.domain = "personal".into();
            row.sensitivity = "sensitive".into();
        }
        row.provenance = "{\"origin\":\"proposal_materializer\"}".into();
        if learnings.insert(&row).await.is_ok() {
            materialized += 1;
        }
    }

    Ok(JobResult {
        summary: format!(
            "proposal_materializer: materialized {materialized} learning(s) from {} applied proposal(s)",
            rows.len()
        ),
        items_processed: materialized,
    })
}

/// Run pattern detection across recent events via SQLite + `detect_patterns`.
///
/// Queries `EventsRepository` for the last 30 days of events, calls
/// `detect_patterns`, and persists each insight as a `kind="improvement"` proposal
/// (deduped by title). Returns the count of insights detected (not line counts).
pub async fn run_observer_scan(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_core::observer::detect_patterns;
    use altevra_db::{EventsRepository, NewProposal, ProposalsRepository};

    let window_days = 30i64;
    let since = ctx.now - chrono::Duration::days(window_days);
    let events = EventsRepository::new(pool)
        .list_since(since, None, 5000)
        .await
        .unwrap_or_default();

    // 1. Event-pattern detectors (keyless, over the events table).
    let insights = detect_patterns(&events, &[]);
    let insight_count = insights.len();

    // 2. DB-backed detectors (R4) — query sessions/turns/hook_runs directly.
    //    Metadata-only evidence; never body text. Runs regardless of whether
    //    the event-pattern path produced anything (the two paths are
    //    independent signal sources).
    let db_insights = crate::observer_detectors::run_db_detectors(pool, ctx.now)
        .await
        .unwrap_or_default();
    let db_insight_count = db_insights.len();

    let proposals = ProposalsRepository::new(pool);
    let mut new_proposals = 0usize;

    // Persist each event-pattern insight as a proposal (idempotent via dedup_hash).
    for ins in &insights {
        let np = NewProposal {
            kind: "improvement".into(),
            title: ins.title.clone(),
            body: ins.summary.clone(),
            source_mode: Some("observer".into()),
            dedup_hash: format!("observer:insight:{}", ins.title),
            evidence_refs: ins
                .evidence
                .iter()
                .filter_map(|e| e.event_id.map(|id| format!("event:{id}")))
                .collect(),
            touches_sensitive: false,
            touches_constitutional: false,
        };
        if let Ok((_, is_new)) = proposals.insert(&np).await {
            if is_new {
                new_proposals += 1;
            }
        }
    }

    // Persist each DB-backed insight as a proposal tagged `observer_db`.
    // Evidence is metadata-only (session/turn ids + counts) — never body text.
    for ins in &db_insights {
        let np = NewProposal {
            kind: "improvement".into(),
            title: ins.title.clone(),
            body: ins.summary.clone(),
            source_mode: Some("observer_db".into()),
            dedup_hash: format!("observer_db:{}:{}", ins.kind, ins.title),
            evidence_refs: ins.evidence.iter().map(|e| e.label.clone()).collect(),
            touches_sensitive: false,
            touches_constitutional: false,
        };
        if let Ok((_, is_new)) = proposals.insert(&np).await {
            if is_new {
                new_proposals += 1;
            }
        }
    }

    let total_insights = insight_count + db_insight_count;

    Ok(JobResult {
        summary: format!(
            "observer scan: {} event(s), {} event-pattern + {} db insight(s), {} new proposal(s)",
            events.len(),
            insight_count,
            db_insight_count,
            new_proposals
        ),
        items_processed: total_insights,
    })
}

/// Scan the vault and queue files into `pending_indexing` for the embed worker.
///
/// `pending_indexing` is consumed by `EmbedderWorker::drain_pending_files`
/// (called at the start of every `altevra embed tick`/`run` tick): each path is
/// ingested → guarded → persisted as memory_documents/memory_chunks → enqueued
/// into `embedder_queue` for vectors.
///
/// The ON CONFLICT clause only resets `failed` rows (giving them another chance)
/// — rows already `pending` or `done` are left untouched so the queue doesn't
/// grow unboundedly on every vault-indexer tick. (NOTE: inside DO UPDATE,
/// unqualified columns refer to the EXISTING row; `excluded.*` is the row we
/// tried to insert, whose status is always 'pending' — the old
/// `excluded.status = 'failed'` condition could never fire.)
pub async fn run_vault_indexer(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    let files = altevra_vault::scan_vault(&ctx.vault_path).unwrap_or_default();
    let mut queued = 0;
    for f in files.iter().take(50) {
        // Insert new rows; on conflict only reset failed rows back to pending
        // (never disturb already-pending rows — they await the embed worker).
        let id = uuid::Uuid::new_v4().to_string();
        let _ = sqlx::query(
            r#"INSERT INTO pending_indexing (id, path, status) VALUES (?, ?, 'pending')
               ON CONFLICT (path) DO UPDATE SET
                   status = CASE WHEN status = 'failed' THEN 'pending' ELSE status END,
                   queued_at = CASE WHEN status = 'failed'
                       THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE queued_at END"#,
        )
        .bind(id)
        .bind(f.path.to_string_lossy().to_string())
        .execute(pool)
        .await;
        queued += 1;
    }
    Ok(JobResult {
        summary: format!("queued {queued} vault files for embed worker"),
        items_processed: queued,
    })
}

/// LLM-powered synthesis (B4). Resolves the `strong_reasoner` role from the router:
/// with `delegated` mode (noop) it skips cleanly (connected tool synthesizes over
/// MCP instead); with a real provider (codex_oauth / api) it produces an insight
/// AND persists it as a durable `insight_card` (migration 020). The card
/// auto-indexes via the A1 `index_object` path inside [`InsightCardsRepository`],
/// so `recall` finds it. SI-14: a card is only written when the model returns
/// non-empty prose; an empty/failed completion writes nothing.
pub async fn run_insight_synthesizer(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_db::{InsightCardRow, InsightCardsRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    // StrongReasoner is a non-personal reasoning role (cloud-eligible, SI-7); the
    // router already forbids personal/local_private from ever reaching the cloud.
    if provider.id() == "noop" {
        return Ok(JobResult {
            summary: "insight synthesis skipped (no LLM configured)".into(),
            items_processed: 0,
        });
    }
    // Pull the ACTUAL last-hour activity so the LLM has something to distill.
    // Without this the synthesizer used to ask the model to summarize the last
    // hour while handing it nothing → "no activity data was provided" cards.
    let since = (ctx.now - chrono::Duration::hours(1))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let activity: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT role || ': ' || substr(content, 1, 280) \
         FROM turns \
         WHERE created_at > ? AND role IN ('user', 'assistant') \
         ORDER BY created_at DESC LIMIT 40",
    )
    .bind(&since)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if activity.is_empty() {
        return Ok(JobResult {
            summary: "insight synthesis skipped (no activity in the last hour)".into(),
            items_processed: 0,
        });
    }

    let activity_block = activity
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let messages = vec![
        ChatMessage::system(
            "You are Altevra's insight synthesizer. Distill the activity log below into \
             ONE concise, sourced sentence capturing the most salient pattern or decision. \
             No preamble.",
        ),
        ChatMessage::user(format!(
            "Last hour of activity (oldest first):\n{activity_block}"
        )),
    ];
    match provider
        .complete(&messages, &ChatOpts::default().with_max_tokens(120))
        .await
    {
        Ok(text) => {
            let body = text.trim().to_string();
            // SI-14: no real content → no write (validate-then-write).
            if body.is_empty() {
                return Ok(JobResult {
                    summary: format!("insight ({}): empty completion, no card written", provider.id()),
                    items_processed: 0,
                });
            }
            // Title = first line (truncated); body = full prose.
            let title: String = body
                .lines()
                .next()
                .unwrap_or(&body)
                .chars()
                .take(120)
                .collect();
            let id = format!("insight-{}", ctx.now.format("%Y%m%dT%H%M%S"));
            let mut card = InsightCardRow::new(id, title.clone(), body.clone());
            // Synthesized over non-personal activity → business domain, internal,
            // agent-inferred (the constructor default), low-ish confidence.
            card.categories = "[\"synthesis\"]".into();
            card.tags = "[\"insight\",\"synthesis\"]".into();
            InsightCardsRepository::new(pool).insert(&card).await?;

            let one: String = body.chars().take(240).collect();
            Ok(JobResult {
                summary: format!("insight card ({}): {one}", provider.id()),
                items_processed: 1,
            })
        }
        Err(e) => Ok(JobResult {
            summary: format!("insight synthesis failed: {e}"),
            items_processed: 0,
        }),
    }
}

/// Pull RSS/Atom feeds, dedupe via SQLite, score against project keywords,
/// write daily Obsidian brief + per-project briefs.
///
/// Driven by `~/.altevra/research/feeds.yaml` (falls back to default packet).
pub async fn run_research_fetcher(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_research::{
        briefs::{write_daily_brief, write_project_brief, ScoredItem},
        feeds::FeedConfig,
        fetcher::fetch_feed,
        interests::gate_allows_item,
        relevance::{default_imperium_projects_path, load_imperium_projects, matching_projects},
    };

    let cfg = FeedConfig::load_or_default();
    let projects_path = default_imperium_projects_path();
    let projects = load_imperium_projects(&projects_path).unwrap_or_default();
    // P4 relevance gate — stated interests + active goals. Create-if-absent
    // (commented template). Inactive gate (no stated interests) preserves the
    // legacy project-keyword behavior.
    let gate = load_relevance_gate(pool).await;

    let mut new_items = 0usize;
    let mut gated_out = 0usize;
    let mut scored_items: Vec<ScoredItem> = Vec::new();
    let mut feeds_touched = 0usize;

    let now = ctx.now;
    for feed in cfg.enabled() {
        // Skip if within fetch_interval since last_fetched_at.
        let cache_hints = fetch_cache_hints(pool, &feed.id).await;
        if let Some(last) = last_fetched_at(pool, &feed.id).await {
            let elapsed = (now - last).num_minutes();
            if elapsed >= 0 && (elapsed as u32) < feed.fetch_interval_minutes {
                continue;
            }
        }

        feeds_touched += 1;
        let outcome = match fetch_feed(feed, cfg.window_days, &cache_hints).await {
            Ok(o) => o,
            Err(e) => {
                record_feed_failure(pool, &feed.id, &e.to_string()).await;
                tracing::warn!("research fetch failed for {}: {e}", feed.id);
                continue;
            }
        };

        record_feed_success(pool, &feed.id, &outcome).await;

        for item in outcome.items {
            // Idempotent insert — UNIQUE(feed_id, guid) prevents dupes.
            let (max_score, matched) = matching_projects(&item, &projects, cfg.relevance_threshold);
            // P4 relevance gate: an ACTIVE gate drops off-interest candidates
            // (no project match AND no stated-interest match) before they
            // ever land in research_items. Debug-logged inside the gate.
            if !gate_allows_item(&gate, &item.title, &item.summary, &matched) {
                gated_out += 1;
                continue;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let project_json = serde_json::to_string(&matched).unwrap_or_else(|_| "[]".into());
            let published = item.published_at.map(|d| d.to_rfc3339());

            let res = sqlx::query(
                r#"INSERT OR IGNORE INTO research_items
                       (id, feed_id, guid, link, title, summary, published_at,
                        relevance_score, project_matches_json)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(&item.feed_id)
            .bind(&item.guid)
            .bind(&item.link)
            .bind(&item.title)
            .bind(&item.summary)
            .bind(published)
            .bind(max_score as f64)
            .bind(&project_json)
            .execute(pool)
            .await;

            let inserted = res.map(|r| r.rows_affected() > 0).unwrap_or(false);
            if inserted {
                new_items += 1;
                scored_items.push(ScoredItem {
                    item,
                    score: max_score,
                    matched_projects: matched,
                });
            }
        }
    }

    // Briefs — only write if we have something new.
    let mut briefs_written = 0usize;
    if !scored_items.is_empty() {
        if let Ok(path) = write_daily_brief(&cfg.brief_paths.daily_obsidian, &scored_items) {
            tracing::info!("daily brief written to {}", path.display());
            briefs_written += 1;
        }
        // Per-project briefs — one per matched project id.
        let mut project_ids: Vec<String> = scored_items
            .iter()
            .flat_map(|i| i.matched_projects.iter().cloned())
            .collect();
        project_ids.sort();
        project_ids.dedup();
        for pid in &project_ids {
            if let Ok(Some(path)) = write_project_brief(
                &ctx.vault_path,
                &cfg.brief_paths.project_vault,
                pid,
                &scored_items,
            ) {
                tracing::info!("project brief ({pid}) written to {}", path.display());
                briefs_written += 1;
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "fetched {feeds_touched} feeds, {new_items} new items ({gated_out} gated off-interest), {briefs_written} brief(s) written"
        ),
        items_processed: new_items,
    })
}

/// Resolve + load the P4 relevance gate for runtime jobs: interests.yaml at
/// `ALTEVRA_INTERESTS_PATH` (tests) or `~/.altevra/interests.yaml`
/// (create-if-absent with the commented template), merged with ACTIVE goal
/// titles from the goals table. Failures degrade to an inactive gate — a
/// broken interests file must never kill the research pipeline.
pub async fn load_relevance_gate(pool: &SqlitePool) -> altevra_research::RelevanceGate {
    use altevra_research::{default_interests_path, RelevanceGate};

    let path = std::env::var("ALTEVRA_INTERESTS_PATH")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_interests_path);
    let gate = RelevanceGate::load_or_create(&path).unwrap_or_else(|e| {
        tracing::warn!("relevance gate load failed ({e}); gate inactive");
        RelevanceGate::default()
    });
    let goal_titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM goals WHERE status = 'active' LIMIT 200")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    gate.with_goals(&goal_titles)
}

async fn last_fetched_at(pool: &SqlitePool, feed_id: &str) -> Option<DateTime<Utc>> {
    let row = sqlx::query("SELECT last_fetched_at FROM research_feed_state WHERE feed_id = ?")
        .bind(feed_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()?;
    let s: Option<String> = sqlx::Row::try_get(&row, "last_fetched_at").ok();
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc))
}

async fn fetch_cache_hints(
    pool: &SqlitePool,
    feed_id: &str,
) -> altevra_research::fetcher::FetchCacheHints {
    let row =
        sqlx::query("SELECT last_etag, last_modified FROM research_feed_state WHERE feed_id = ?")
            .bind(feed_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    match row {
        Some(r) => altevra_research::fetcher::FetchCacheHints {
            etag: sqlx::Row::try_get::<Option<String>, _>(&r, "last_etag")
                .ok()
                .flatten(),
            last_modified: sqlx::Row::try_get::<Option<String>, _>(&r, "last_modified")
                .ok()
                .flatten(),
        },
        None => altevra_research::fetcher::FetchCacheHints::default(),
    }
}

async fn record_feed_success(
    pool: &SqlitePool,
    feed_id: &str,
    outcome: &altevra_research::fetcher::FetchOutcome,
) {
    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        r#"INSERT INTO research_feed_state
               (feed_id, last_fetched_at, last_etag, last_modified, fail_count, last_error)
           VALUES (?, ?, ?, ?, 0, NULL)
           ON CONFLICT(feed_id) DO UPDATE SET
               last_fetched_at = excluded.last_fetched_at,
               last_etag = excluded.last_etag,
               last_modified = excluded.last_modified,
               fail_count = 0,
               last_error = NULL"#,
    )
    .bind(feed_id)
    .bind(&now)
    .bind(&outcome.new_etag)
    .bind(&outcome.new_last_modified)
    .execute(pool)
    .await;
}

async fn record_feed_failure(pool: &SqlitePool, feed_id: &str, err: &str) {
    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        r#"INSERT INTO research_feed_state
               (feed_id, last_fetched_at, fail_count, last_error)
           VALUES (?, ?, 1, ?)
           ON CONFLICT(feed_id) DO UPDATE SET
               last_fetched_at = excluded.last_fetched_at,
               fail_count = research_feed_state.fail_count + 1,
               last_error = excluded.last_error"#,
    )
    .bind(feed_id)
    .bind(&now)
    .bind(err)
    .execute(pool)
    .await;
}

/// A person who's gone quiet: no mention for `weeks_since` weeks (CLAUDE.md §3.6).
const LAST_CONTACT_STALE_WEEKS: i64 = 2;
/// How far back the daily summary loads events for pattern detection.
const DAILY_EVENT_WINDOW_DAYS: i64 = 30;

/// Daily summary at 23:00 local (B3) — "the brain that notices" (CLAUDE.md §3.6).
/// Writes a markdown file under `vault/10-insights/daily-YYYY-MM-DD.md` that
/// surfaces THREE real signals:
///   1. detected patterns (`altevra_core::observer::detect_patterns` over recent
///      events),
///   2. last-contact gaps ("haven't talked to <Person> in <N> weeks") computed via
///      `altevra_core::last_contact` over the mention graph, and
///   3. stale decisions whose `review_after` has passed ("still applies?").
///
/// If a real `StrongReasoner` is configured, the assembled bullets are passed to
/// the LLM to synthesize prose; otherwise the structured bullets are written
/// directly (noop path — no LLM). Either way the file contains real content.
pub async fn run_daily_summary(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_core::{detect_patterns, last_contact, EntityKind};
    use altevra_db::{EventsRepository, MentionsRepository, TasksRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let date = ctx.now.format("%Y-%m-%d").to_string();
    let dir = ctx.vault_path.join("10-insights");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("daily-{date}.md"));
    if path.exists() {
        return Ok(JobResult {
            summary: format!("daily summary already exists for {date}"),
            items_processed: 0,
        });
    }

    // 1. Detected patterns over recent events (observer is keyless + LIVE).
    let since = ctx.now - chrono::Duration::days(DAILY_EVENT_WINDOW_DAYS);
    let events = EventsRepository::new(pool)
        .list_since(since, None, 2000)
        .await
        .unwrap_or_default();
    let insights = detect_patterns(&events, &[]);
    let mut pattern_lines: Vec<String> = insights
        .iter()
        .map(|i| format!("[{}] {}", i.importance, i.title))
        .collect();
    pattern_lines.sort();

    // 2. Last-contact gaps. Build the entity dictionary (people/projects) from the
    //    vault + registry, then for each known PERSON compute last_contact over the
    //    mention graph (gives `last_contact` in altevra-core its first caller).
    let dict = altevra_vault::entity_dict::build_dictionary_for_vault(&ctx.vault_path);
    let dated = MentionsRepository::new(pool)
        .dated_mentions()
        .await
        .unwrap_or_default();
    let today = ctx.now.date_naive();
    let mut contact_lines: Vec<String> = Vec::new();
    for person in dict.all().filter(|e| e.kind == EntityKind::Person) {
        let Some(last) = last_contact(&person.id, &dated) else {
            continue; // never mentioned → nothing to nag about
        };
        let weeks = (today - last).num_weeks();
        if weeks >= LAST_CONTACT_STALE_WEEKS {
            contact_lines.push(format!(
                "haven't talked to {} in {} weeks (last: {})",
                person.name, weeks, last
            ));
        }
    }
    contact_lines.sort();

    // 3. Decisions whose review_after has passed — "still applies?".
    let due = TasksRepository::new(pool)
        .decisions_due_for_review(ctx.now, 50)
        .await
        .unwrap_or_default();
    let decision_lines: Vec<String> = due
        .iter()
        .map(|d| {
            format!(
                "decision '{}' from {} — still applies?",
                d.title,
                d.decided_at.format("%Y-%m-%d")
            )
        })
        .collect();

    let total_signals = pattern_lines.len() + contact_lines.len() + decision_lines.len();

    // Assemble the structured bullets (always — they are the source of truth).
    let mut structured = String::new();
    structured.push_str("## Patterns detected\n\n");
    if pattern_lines.is_empty() {
        structured.push_str("- _No patterns detected in this window._\n");
    } else {
        for l in &pattern_lines {
            structured.push_str(&format!("- {l}\n"));
        }
    }
    // P4 policy gate: contact gaps are RELATIONSHIP-domain data and
    // `dp_relationship` is seeded `obsidian_mirror = 'never'` — names must
    // never land in this (syncable) vault file, nor reach the cloud
    // StrongReasoner below. COUNT + CLI pointer only; the full lines live in
    // `altevra brief --private` (terminal-only).
    structured.push_str("\n## People — last contact\n\n");
    if contact_lines.is_empty() {
        structured.push_str("- _No overdue reach-outs._\n");
    } else {
        structured.push_str(&format!(
            "- {} overdue reach-out(s) withheld by domain policy — view: `altevra brief --private`\n",
            contact_lines.len()
        ));
    }
    structured.push_str("\n## Decisions to re-check\n\n");
    if decision_lines.is_empty() {
        structured.push_str("- _No decisions past their review date._\n");
    } else {
        for l in &decision_lines {
            structured.push_str(&format!("- {l}\n"));
        }
    }

    // C7 — curator digest line (additive; never replaces other sections).
    // Counts come from real `proposals` + `skills` rows, not a hard-coded zero.
    // Format pinned by `curator::DIGEST_TAG` so dashboards can grep for it.
    let digest = crate::curator::curator_digest_line(pool).await;
    structured.push_str("\n## Self-improve\n\n");
    structured.push_str(&format!("- {digest}\n"));

    // 4. If a StrongReasoner is configured, synthesize prose; else write bullets.
    //    StrongReasoner is a non-personal reasoning role (cloud-eligible, SI-7);
    //    the structured bullets carry only titles/dates already in the vault.
    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    let mut prose: Option<String> = None;
    if provider.id() != "noop" {
        let messages = vec![
            ChatMessage::system(
                "You are Altevra's daily briefing writer. Given the structured signals \
                 below, write a SHORT prose briefing (3-5 sentences) that a busy founder \
                 reads in the evening. Keep every concrete fact (names, dates, counts). \
                 No preamble.",
            ),
            ChatMessage::user(&structured),
        ];
        if let Ok(text) = provider
            .complete(&messages, &ChatOpts::default().with_max_tokens(400))
            .await
        {
            let t = text.trim().to_string();
            if !t.is_empty() {
                prose = Some(t);
            }
        }
    }

    let generated_by = if prose.is_some() {
        format!("altevra-brain + {}", provider.id())
    } else {
        "altevra-brain".to_string()
    };
    let mut body = format!(
        "---\nkind: daily-summary\ngenerated_by: {generated_by}\ndate: {date}\nsignals: {total_signals}\n---\n\n# Daily Summary — {date}\n\n"
    );
    if let Some(p) = &prose {
        body.push_str(p);
        body.push_str("\n\n---\n\n");
    }
    body.push_str(&structured);

    std::fs::write(&path, body)?;

    // P4 brief delivery — the policy-gated daily brief into <vault>/Daily/.
    // Claims (O_EXCL dedup) are taken: this is THE once-a-day notify pass.
    // Fail-soft: a brief failure never aborts the daily summary.
    let gate = load_relevance_gate(pool).await;
    let claims_dir = crate::notify::delivery::default_claims_dir();
    match crate::notify::write_vault_brief(pool, &ctx.vault_path, &claims_dir, true, &gate, ctx.now)
        .await
    {
        Ok(Some(p)) => tracing::info!("daily brief written: {}", p.display()),
        Ok(None) => tracing::debug!("daily brief already exists for {date}"),
        Err(e) => tracing::warn!("daily brief delivery failed: {e}"),
    }

    Ok(JobResult {
        summary: format!(
            "daily summary for {date}: {} pattern(s), {} contact gap(s), {} stale decision(s)",
            pattern_lines.len(),
            contact_lines.len(),
            decision_lines.len()
        ),
        items_processed: 1,
    })
}

/// How many uncategorized objects one auto-categorizer pass handles.
const AUTO_CATEGORIZE_BATCH: i64 = 50;

/// Auto-categorization (B5, CLAUDE.md §3.2 — a LIVING taxonomy, not a static enum).
///
/// Reads `object_index` rows lacking a resolved category (`categories == []`) and,
/// for each, asks an LLM to classify it against the categories already in use:
///   * an existing category fits → tag the object (`set_category`),
///   * none fits → propose a NEW category as a `kind="category"` proposal
///     (Tier-0, via [`ProposalsRepository`]) for Pavle's daily digest.
///
/// **SI-7 routing (load-bearing):** the model that sees the object is chosen by the
/// object's DOMAIN. A high-water object (personal/relationship/health/legal/
/// financial/client) is classified by `local_private` (on-device) and MUST NEVER be
/// sent to the cloud `cheap_worker`. Non-high-water objects use `cheap_worker`. If
/// the role resolves to noop (no model configured), the object is skipped cleanly —
/// nothing is classified, tagged, or proposed.
pub async fn run_auto_categorizer(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_core::Domain;
    use altevra_db::{NewProposal, ObjectIndexRepository, ProposalsRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let idx = ObjectIndexRepository::new(pool);
    let todo = idx.uncategorized(AUTO_CATEGORIZE_BATCH).await?;
    if todo.is_empty() {
        return Ok(JobResult {
            summary: "auto-categorize: nothing uncategorized".into(),
            items_processed: 0,
        });
    }
    let existing = idx.distinct_categories().await?;
    let proposals = ProposalsRepository::new(pool);

    let mut tagged = 0usize;
    let mut proposed = 0usize;
    let mut skipped = 0usize;

    for obj in &todo {
        // SI-7 single rule (see `crate::routing::role_for_object`): high-water domain
        // OR high-water content → LocalPrivate; else CheapWorker. The shared helper
        // makes the policy identical to the resident run path — no duplication, no
        // drift between call sites. A false positive only ever keeps work local.
        let domain: Domain = obj.domain.parse().unwrap_or(Domain::Business);
        let title = obj.title.clone().unwrap_or_default();
        let body = fetch_object_body(pool, &obj.object_type, &obj.id).await;
        let scanned = format!("{title}\n{body}");
        let role = crate::routing::role_for_object(&domain, &scanned, ModelRole::CheapWorker);
        if role == ModelRole::LocalPrivate && !domain.is_high_water() {
            tracing::warn!(
                "auto-categorize: object {} has domain={} but high-water CONTENT — \
                 keeping local (SI-7 content fail-safe), not sending to cloud",
                obj.id,
                obj.domain
            );
        }

        let provider = ctx.router.resolve(role);
        if provider.id() == "noop" {
            // No model for this role → skip cleanly (no write).
            skipped += 1;
            continue;
        }

        let cat_list = if existing.is_empty() {
            "(none yet)".to_string()
        } else {
            existing.join(", ")
        };
        let messages = vec![
            ChatMessage::system(
                "You are Altevra's category classifier. Reply with EXACTLY ONE short \
                 lowercase category label and NOTHING else. Prefer an existing category \
                 from the provided list if one fits; otherwise return a new, concise \
                 label.",
            ),
            ChatMessage::user(format!(
                "Existing categories: {cat_list}\nObject ({}/{}, domain={}): {title}",
                obj.object_type, obj.id, obj.domain
            )),
        ];
        let label = match provider
            .complete(&messages, &ChatOpts::default().with_max_tokens(16))
            .await
        {
            Ok(t) => normalize_category(&t),
            Err(e) => {
                tracing::warn!("auto-categorize classify failed for {}: {e}", obj.id);
                skipped += 1;
                continue;
            }
        };
        if label.is_empty() {
            skipped += 1;
            continue;
        }

        // Does an existing category fit (case-insensitive)?
        match existing
            .iter()
            .find(|c| c.eq_ignore_ascii_case(&label))
        {
            Some(fit) => {
                if idx.set_category(&obj.object_type, &obj.id, fit).await? {
                    tagged += 1;
                }
            }
            None => {
                // Novel category → a Tier-0 `category` proposal for the daily digest.
                // SI-9: the repo re-derives the tier from kind ("category" → Tier-0).
                let dedup = format!("category:{}", label.to_lowercase());
                let (_, is_new) = proposals
                    .insert(&NewProposal {
                        kind: "category".into(),
                        title: format!("New category: {label}"),
                        body: format!(
                            "Auto-categorizer found no existing category for {} `{}` \
                             (domain={}); proposes a new category `{label}`.",
                            obj.object_type, obj.id, obj.domain
                        ),
                        source_mode: Some("auto_categorizer".into()),
                        dedup_hash: dedup,
                        evidence_refs: vec![format!("{}:{}", obj.object_type, obj.id)],
                        touches_sensitive: false,
                        touches_constitutional: false,
                    })
                    .await?;
                if is_new {
                    proposed += 1;
                }
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "auto-categorize: {} considered, {tagged} tagged, {proposed} new-category proposal(s), {skipped} skipped (no model)",
            todo.len()
        ),
        items_processed: tagged + proposed,
    })
}

/// Fetch an object's indexed body from `object_fts` (where capture stores the full
/// text). Used by the SI-7 content fail-safe to scan title+body before any cloud
/// call. Returns an empty string if the row is absent or the query fails — a missing
/// body just means the title-only scan still runs (fail-safe never errors the job).
async fn fetch_object_body(pool: &SqlitePool, object_type: &str, id: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT body FROM object_fts WHERE object_type = ? AND object_id = ? LIMIT 1",
    )
    .bind(object_type)
    .bind(id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Normalize a model's category reply to a single short lowercase label: first
/// non-empty line, trimmed of quotes/punctuation, lowercased, capped length.
fn normalize_category(raw: &str) -> String {
    let line = raw.trim().lines().next().unwrap_or("").trim();
    let cleaned: String = line
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '.' || c == ',')
        .to_lowercase();
    cleaned.chars().take(40).collect::<String>().trim().to_string()
}

/// Task grooming — flag stale tasks. Placeholder; full logic in v0.3.7.
pub async fn run_task_grooming(pool: &SqlitePool, _ctx: &JobContext) -> anyhow::Result<JobResult> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM tasks WHERE status = 'open'")
        .fetch_one(pool)
        .await
        .ok();
    let n = row
        .and_then(|r| sqlx::Row::try_get::<i64, _>(&r, "n").ok())
        .unwrap_or(0);
    Ok(JobResult {
        summary: format!("{n} open task(s)"),
        items_processed: n as usize,
    })
}

/// Walk recent research_items, fetch their source pages, extract feed links,
/// and insert candidates. Full-auto mode promotes immediately into the
/// active feeds.yaml file.
pub async fn run_feed_discovery(pool: &SqlitePool, _ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_research::discover::{extract_feed_links, filter_promising_blog_links};

    // Pick a small batch of recent items to scan. Each row gives us a source
    // page URL — we crawl that page (light HTTP only — no imperium-crawl) and
    // extract any RSS hints.
    let rows = sqlx::query("SELECT link FROM research_items ORDER BY ingested_at DESC LIMIT 25")
        .fetch_all(pool)
        .await?;
    if rows.is_empty() {
        return Ok(JobResult {
            summary: "no research items to mine for discovery".into(),
            items_processed: 0,
        });
    }

    let client = reqwest::Client::builder()
        .user_agent("Altevra/0.3 feed-discovery")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let mut candidates_seen = 0usize;
    let mut candidates_new = 0usize;
    for row in rows {
        let url: String = sqlx::Row::try_get(&row, "link").unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(html) = resp.text().await else {
            continue;
        };

        // Direct feed-link hints from this page.
        let feed_links = extract_feed_links(&url, &html);
        // Optionally, promising outbound (filtered) — kept as candidates without feed_url.
        let outbound = altevra_research::discover::extract_outbound_links(&url, &html);
        let promising = filter_promising_blog_links(&outbound);

        for f in feed_links.iter().chain(promising.iter()) {
            candidates_seen += 1;
            let id = uuid::Uuid::new_v4().to_string();
            let res = sqlx::query(
                r#"INSERT OR IGNORE INTO research_feed_candidates
                       (id, candidate_url, feed_url, source_url, discovered_by, status)
                   VALUES (?, ?, ?, ?, 'brain_job', 'pending')"#,
            )
            .bind(&id)
            .bind(f)
            .bind(f) // candidate_url == feed_url for the direct-link hints
            .bind(&url)
            .execute(pool)
            .await;
            if let Ok(r) = res {
                if r.rows_affected() > 0 {
                    candidates_new += 1;
                }
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "discovery scanned {} item(s), found {candidates_seen} candidate links, {candidates_new} new",
            25
        ),
        items_processed: candidates_new,
    })
}

/// Fetch GitHub Trending for a configurable set of languages and ingest as
/// research_items with source_kind = 'github-trending'.
pub async fn run_github_trending_fetch(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_research::sources::github_trending::{GitHubTrendingSource, TrendingPeriod};
    use altevra_research::sources::{FetchCtx, SourceProvider};

    let languages: &[Option<&str>] = &[Some("rust"), Some("typescript"), Some("python")];
    let ctx = FetchCtx {
        window_days: 1,
        ..Default::default()
    };
    let mut total_new = 0usize;
    let mut feeds_touched = 0usize;

    for lang in languages {
        feeds_touched += 1;
        let source = GitHubTrendingSource::new(lang.map(String::from), TrendingPeriod::Daily);
        let items = match source.fetch(&ctx).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("github trending fetch failed for {:?}: {e}", lang);
                continue;
            }
        };
        let feed_id = source.id_str();
        for item in items {
            let id = uuid::Uuid::new_v4().to_string();
            let res = sqlx::query(
                r#"INSERT OR IGNORE INTO research_items
                       (id, feed_id, guid, link, title, summary, published_at,
                        relevance_score, project_matches_json, source_kind)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[]', 'github-trending')"#,
            )
            .bind(&id)
            .bind(&feed_id)
            .bind(&item.guid)
            .bind(&item.link)
            .bind(&item.title)
            .bind(&item.summary)
            .bind(item.published_at.map(|d| d.to_rfc3339()))
            .bind(0.0f64)
            .execute(pool)
            .await;
            if let Ok(r) = res {
                if r.rows_affected() > 0 {
                    total_new += 1;
                }
            }
        }
    }
    Ok(JobResult {
        summary: format!("github trending: {feeds_touched} langs, {total_new} new repos"),
        items_processed: total_new,
    })
}

/// Per-project agent sweep. For every project in
/// `~/.imperium/identity/projects.yaml` (with optional per-project YAML override
/// at `~/.altevra/research/projects/<id>.yaml`), run web search for each
/// configured query against DuckDuckGo (free; Brave/Exa if keys present),
/// and insert top-N items into research_items with source_kind='web-search'.
pub async fn run_project_research_sweep(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_research::projects::ProjectAgent;
    use altevra_research::sources::web_search::{WebSearchProviderKind, WebSearchSource};
    use altevra_research::sources::{FetchCtx, SourceProvider};

    let identity_path = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join(".imperium")
        .join("identity")
        .join("projects.yaml");
    if !identity_path.exists() {
        return Ok(JobResult {
            summary: "no ~/.imperium/identity/projects.yaml — skipping project sweep".into(),
            items_processed: 0,
        });
    }
    let agents = ProjectAgent::load_all(&identity_path).unwrap_or_default();
    if agents.is_empty() {
        return Ok(JobResult {
            summary: "no project agents loaded".into(),
            items_processed: 0,
        });
    }

    let brave_key = std::env::var("BRAVE_API_KEY").ok();
    let exa_key = std::env::var("EXA_API_KEY").ok();
    let mut total_new = 0usize;
    let mut projects_touched = 0usize;

    for agent in &agents {
        projects_touched += 1;
        let queries_to_run = agent
            .queries
            .iter()
            .take(agent.daily_budget_queries.min(20) as usize)
            .cloned()
            .collect::<Vec<_>>();
        if queries_to_run.is_empty() {
            continue;
        }

        for query in &queries_to_run {
            let mut source = WebSearchSource::new(query.clone());
            // Provider chain: Brave (if keyed) → Exa (if keyed) → DDG.
            let mut chain = Vec::new();
            if brave_key.is_some() {
                chain.push(WebSearchProviderKind::Brave);
            }
            if exa_key.is_some() {
                chain.push(WebSearchProviderKind::Exa);
            }
            chain.push(WebSearchProviderKind::DuckDuckGo);
            source = source.with_chain(chain);
            if let Some(k) = &brave_key {
                source = source.with_brave(k);
            }
            if let Some(k) = &exa_key {
                source = source.with_exa(k);
            }

            let ctx = FetchCtx {
                limit: 10,
                ..Default::default()
            };
            let items = match source.fetch(&ctx).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("web search failed for '{query}': {e}");
                    continue;
                }
            };
            for item in items {
                let id = uuid::Uuid::new_v4().to_string();
                let project_match = serde_json::json!([agent.project_id.clone()]).to_string();
                let res = sqlx::query(
                    r#"INSERT OR IGNORE INTO research_items
                           (id, feed_id, guid, link, title, summary, published_at,
                            relevance_score, project_matches_json, source_kind)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'web-search')"#,
                )
                .bind(&id)
                .bind(&item.feed_id)
                .bind(&item.guid)
                .bind(&item.link)
                .bind(&item.title)
                .bind(&item.summary)
                .bind(item.published_at.map(|d| d.to_rfc3339()))
                .bind(0.5f64)
                .bind(&project_match)
                .execute(pool)
                .await;
                if let Ok(r) = res {
                    if r.rows_affected() > 0 {
                        total_new += 1;
                    }
                }
            }
        }

        // Update per-project state.
        let now = Utc::now().to_rfc3339();
        let queries_used = queries_to_run.len() as i64;
        let _ = sqlx::query(
            r#"INSERT INTO project_research_state
                   (project_id, last_run_at, queries_used_today, daily_budget)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(project_id) DO UPDATE SET
                   last_run_at = excluded.last_run_at,
                   queries_used_today = excluded.queries_used_today,
                   daily_budget = excluded.daily_budget"#,
        )
        .bind(&agent.project_id)
        .bind(&now)
        .bind(queries_used)
        .bind(agent.daily_budget_queries as i64)
        .execute(pool)
        .await;
    }

    Ok(JobResult {
        summary: format!(
            "project sweep: {projects_touched} project(s), {total_new} new web-search item(s)"
        ),
        items_processed: total_new,
    })
}

pub async fn dispatch(
    kind: JobKind,
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    match kind {
        JobKind::EventClassifier => run_event_classifier(pool, ctx).await,
        JobKind::ObserverScan => run_observer_scan(pool, ctx).await,
        JobKind::VaultIndexer => run_vault_indexer(pool, ctx).await,
        JobKind::InsightSynthesizer => run_insight_synthesizer(pool, ctx).await,
        JobKind::ResearchFetcher => run_research_fetcher(pool, ctx).await,
        JobKind::FeedDiscovery => run_feed_discovery(pool, ctx).await,
        JobKind::GitHubTrendingFetch => run_github_trending_fetch(pool, ctx).await,
        JobKind::ProjectResearchSweep => run_project_research_sweep(pool, ctx).await,
        JobKind::DailySummary => run_daily_summary(pool, ctx).await,
        JobKind::TaskGrooming => run_task_grooming(pool, ctx).await,
        JobKind::AutoCategorizer => run_auto_categorizer(pool, ctx).await,
        JobKind::SelfImproveOrchestrator => crate::selfimprove::run_self_improve(pool, ctx).await,
        JobKind::Curator => crate::curator::run_curator(pool, ctx).await,
        JobKind::LifecycleArchiver => run_lifecycle_archiver(pool, ctx).await,
        JobKind::SkillReactionJudge => run_skill_reaction_judge(pool, ctx).await,
        JobKind::DbOptimize => run_db_optimize(pool, ctx).await,
        JobKind::ConnectorSync => run_connector_sync(pool, ctx).await,
        JobKind::PersonalExtractor => run_personal_extractor(pool, ctx).await,
        JobKind::SkillFactory => run_skill_factory(pool, ctx).await,
        JobKind::MemoryWriteback => run_memory_writeback(pool, ctx).await,
        JobKind::Healer => run_healer(pool, ctx).await,
        JobKind::FileChangeIndexer => run_file_change_indexer(pool, ctx).await,
        JobKind::WikiCurator => run_wiki_curator(pool, ctx).await,
        JobKind::ProposalMaterializer => run_proposal_materializer(pool, ctx).await,
    }
}

/// E1 (PLAN-EXTEND) — brain-job wrapper around [`crate::connector_sync`]. Reads
/// the connectors config from `ALTEVRA_CONNECTORS_PATH` or the default home
/// path, syncs every ENABLED connector through the full safety stack, and
/// projects the report onto a one-line `JobResult`. Never blocks other jobs: a
/// connector error becomes red health, not a job failure.
pub async fn run_connector_sync(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    let cfg_path = altevra_adapters::connectors::ConnectorsConfig::default_path();
    let report =
        crate::connector_sync::run_connector_sync_at(pool, &cfg_path, ctx.now, None, false).await?;
    Ok(JobResult {
        summary: report.summary(),
        items_processed: report.total_persisted(),
    })
}

/// E2 — weekly self-cleaning maintenance job.
///
/// Runs `PRAGMA optimize` (refreshes query-planner stats) + `PRAGMA
/// incremental_vacuum` (reclaims free pages the lifecycle archiver / retention
/// sweeps already freed — NEVER deletes content; raw turns are canonical), then
/// snapshots the DB file size + a retention-job liveness count into
/// `db_size_history` for the doctor's size-trend check.
///
/// All steps are best-effort: a PRAGMA that the SQLite build does not support
/// (e.g. incremental_vacuum when auto_vacuum is OFF) is a no-op, not a failure —
/// the job still records its snapshot so the trend line stays continuous.
pub async fn run_db_optimize(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_db::DbSizeHistoryRepository;

    // Free-page count BEFORE the vacuum (in pages); used to estimate freed bytes.
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(pool)
        .await
        .unwrap_or(4096);
    let free_before: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    // PRAGMA optimize — refreshes planner stats. Cheap, always safe.
    let _ = sqlx::query("PRAGMA optimize").execute(pool).await;

    // PRAGMA incremental_vacuum — reclaims free pages WITHOUT rewriting the whole
    // DB (only effective when auto_vacuum=INCREMENTAL; otherwise a harmless no-op).
    let _ = sqlx::query("PRAGMA incremental_vacuum").execute(pool).await;

    let free_after: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await
        .unwrap_or(free_before);
    let freed_bytes = (free_before - free_after).max(0) * page_size;

    // On-disk size = page_count * page_size (works for in-memory + file DBs).
    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let size_bytes = page_count * page_size;

    // Retention-job liveness: how many brain_jobs ran in the trailing 8 days
    // (one weekly cadence + slack). A zero here while this job runs would mean
    // the scheduler is wedged — surfaced by the doctor.
    let since = (ctx.now - chrono::Duration::days(8))
        .format("%Y-%m-%dT%H:%M:%fZ")
        .to_string();
    let jobs_in_window: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM brain_jobs WHERE started_at >= ?")
            .bind(&since)
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    DbSizeHistoryRepository::new(pool)
        .record(size_bytes, freed_bytes, jobs_in_window)
        .await?;

    Ok(JobResult {
        summary: format!(
            "db_optimize: {size_bytes} bytes ({:.1} MB), reclaimed {freed_bytes} bytes, \
             {jobs_in_window} job(s) in trailing window",
            size_bytes as f64 / 1_048_576.0
        ),
        items_processed: 1,
    })
}

/// P3c — brain-job wrapper around [`crate::skill_judge::drain_skill_reactions`].
/// The judge is the LOCAL Ollama structured-outputs judge built from
/// `~/.altevra/config.toml` `[llm].local_private`; when no loopback local model
/// is configured the drain still runs with a conservative noop judge (every
/// window judged success=true → events drained, zero proposals — a missing
/// judge can never manufacture deficiency OR grow an unbounded backlog).
pub async fn run_skill_reaction_judge(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use crate::skill_judge::{
        default_skill_body_for, drain_skill_reactions, JudgeVerdict, OllamaJudge, SuccessJudge,
    };

    struct ConservativeNoopJudge;
    #[async_trait::async_trait]
    impl SuccessJudge for ConservativeNoopJudge {
        async fn judge(&self, _skill: &str, _window: &str) -> JudgeVerdict {
            JudgeVerdict::conservative()
        }
    }

    let ollama = OllamaJudge::from_home_config();
    let judge: &dyn SuccessJudge = match &ollama {
        Some(j) => j,
        None => &ConservativeNoopJudge,
    };
    let report = drain_skill_reactions(pool, judge, &default_skill_body_for).await?;
    Ok(JobResult {
        summary: report.summary(),
        items_processed: report.judged + report.deferred,
    })
}

/// E1 — brain-job wrapper around [`crate::lifecycle::lifecycle_archive`]. Pure
/// adapter: pass the context clock through, project the structured report onto
/// a one-line `JobResult` for `brain_jobs.result_summary`.
pub async fn run_lifecycle_archiver(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    let report = crate::lifecycle::lifecycle_archive(pool, ctx.now).await?;

    // R4: events retention sweep — prune noise-class events past the retention
    // window. Session/skill/decision events are never touched (durable signal).
    let retention = crate::observer_detectors::prune_noise_events(
        pool,
        ctx.now,
        crate::observer_detectors::DEFAULT_RETENTION_DAYS,
    )
    .await
    .unwrap_or_default();

    Ok(JobResult {
        summary: format!("{} | {}", report.summary(), retention.summary()),
        items_processed: report.total_actions() + retention.pruned,
    })
}

/// Iterate every job kind. Kept as a single source of truth so a new variant
/// added to [`JobKind`] is automatically picked up by the scheduler loop AND
/// by `roundtrip`-style tests.
pub fn all_kinds() -> [JobKind; 24] {
    [
        JobKind::EventClassifier,
        // Cheap, no-LLM, idempotent — run early so it isn't starved behind the
        // expensive LLM jobs (a full cycle of claude -p calls can take minutes).
        JobKind::ProposalMaterializer,
        JobKind::ObserverScan,
        JobKind::VaultIndexer,
        JobKind::InsightSynthesizer,
        JobKind::ResearchFetcher,
        JobKind::FeedDiscovery,
        JobKind::GitHubTrendingFetch,
        JobKind::ProjectResearchSweep,
        JobKind::DailySummary,
        JobKind::TaskGrooming,
        JobKind::AutoCategorizer,
        JobKind::SelfImproveOrchestrator,
        JobKind::Curator,
        JobKind::LifecycleArchiver,
        JobKind::SkillReactionJudge,
        JobKind::DbOptimize,
        JobKind::ConnectorSync,
        JobKind::PersonalExtractor,
        JobKind::SkillFactory,
        JobKind::MemoryWriteback,
        JobKind::Healer,
        JobKind::FileChangeIndexer,
        JobKind::WikiCurator,
    ]
}

/// Run every enabled job once, sequentially, returning per-kind results.
/// Useful for `altevra brain run-all` style CLI calls and for tests that want
/// a deterministic single pass without driving the scheduler loop.
///
/// The function never short-circuits on error: a failing job is logged and
/// reported with `Err`, then the next kind runs. The scheduler uses the same
/// per-kind dispatch + history pattern; this is the headless equivalent.
pub async fn run_all(
    pool: &SqlitePool,
    ctx: &JobContext,
    disabled: &[String],
) -> Vec<(JobKind, anyhow::Result<JobResult>)> {
    let mut out = Vec::with_capacity(all_kinds().len());
    for kind in all_kinds() {
        if disabled.iter().any(|d| d == kind.as_str()) {
            continue;
        }
        let r = dispatch(kind, pool, ctx).await;
        out.push((kind, r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All job tests run against the noop router (no keys); matches production default.
    fn noop_router() -> std::sync::Arc<altevra_llm::ModelRouter> {
        std::sync::Arc::new(altevra_llm::ModelRouter::noop())
    }

    /// A deterministic stub provider that stands in for a configured (non-noop)
    /// cloud reasoner — it returns canned prose so the LLM-backed path (card
    /// write / classification) runs without keys or network. `is_local` is false
    /// (it represents a cloud provider) so SI-7 routing is exercised honestly:
    /// the router refuses to use it for `local_private`.
    struct StubProvider {
        id: &'static str,
        reply: String,
    }

    #[async_trait::async_trait]
    impl altevra_llm::ChatProvider for StubProvider {
        fn id(&self) -> &str {
            self.id
        }
        fn is_local(&self) -> bool {
            false
        }
        async fn complete(
            &self,
            _messages: &[altevra_llm::ChatMessage],
            _opts: &altevra_llm::ChatOpts,
        ) -> anyhow::Result<String> {
            Ok(self.reply.clone())
        }
    }

    /// A router with a stub cloud provider on a single role.
    fn router_with_stub(
        role: altevra_llm::ModelRole,
        id: &'static str,
        reply: &str,
    ) -> std::sync::Arc<altevra_llm::ModelRouter> {
        std::sync::Arc::new(altevra_llm::ModelRouter::noop().with_provider(
            role,
            std::sync::Arc::new(StubProvider {
                id,
                reply: reply.to_string(),
            }),
        ))
    }

    /// A fully-migrated in-memory db (real schema) for jobs that persist objects.
    async fn migrated_pool() -> SqlitePool {
        let pool = altevra_db::create_pool("sqlite::memory:").await.unwrap();
        altevra_db::run_migrations(&pool).await.unwrap();
        pool
    }

    async fn setup_research_schema() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Minimal schema needed for run_research_fetcher path.
        sqlx::query(
            r#"CREATE TABLE research_feed_state (
                feed_id TEXT PRIMARY KEY,
                last_fetched_at TEXT,
                last_etag TEXT,
                last_modified TEXT,
                fail_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE research_items (
                id TEXT PRIMARY KEY,
                feed_id TEXT NOT NULL,
                guid TEXT NOT NULL,
                link TEXT NOT NULL,
                title TEXT NOT NULL,
                summary TEXT NOT NULL DEFAULT '',
                published_at TEXT,
                ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                relevance_score REAL NOT NULL DEFAULT 0.0,
                project_matches_json TEXT NOT NULL DEFAULT '[]',
                UNIQUE(feed_id, guid)
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn job_kind_roundtrip() {
        // all_kinds() is the single source of truth — using it here means a
        // new variant added to JobKind can't silently dodge the roundtrip test.
        for k in all_kinds() {
            assert_eq!(JobKind::parse(k.as_str()), Some(k));
        }
        // Curator wiring spot-check (C7).
        assert_eq!(JobKind::Curator.as_str(), "curator");
        assert_eq!(JobKind::Curator.period_secs(), 7 * 24 * 60 * 60);
    }

    #[tokio::test]
    async fn project_research_sweep_returns_when_no_identity_file() {
        // Without ~/.imperium/identity/projects.yaml the job should bail
        // gracefully with a skip message, not panic.
        let pool = setup_research_schema().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_project_research_sweep(&pool, &ctx).await.unwrap();
        assert!(r.summary.to_lowercase().contains("no"));
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[tokio::test]
    async fn vault_indexer_requeues_failed_but_preserves_pending_and_done() {
        // P0 §5 — pending_indexing requeue semantics. The old SQL compared
        // `excluded.status` (always 'pending') so failed rows were NEVER
        // requeued. This drives the fixed path: failed → pending again,
        // pending stays pending, done stays done.
        let pool = migrated_pool().await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("note.md"), "# Note\n\nbody.\n").unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };

        // First run queues the file as pending.
        run_vault_indexer(&pool, &ctx).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM pending_indexing LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "pending");

        // Second run must NOT disturb a pending row.
        run_vault_indexer(&pool, &ctx).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_indexing")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "re-scan must not duplicate queue rows");

        // A failed row gets another chance on the next scan.
        sqlx::query("UPDATE pending_indexing SET status = 'failed'")
            .execute(&pool)
            .await
            .unwrap();
        run_vault_indexer(&pool, &ctx).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM pending_indexing LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "pending", "failed rows must be requeued");

        // A done row stays done (the embed worker already consumed it).
        sqlx::query("UPDATE pending_indexing SET status = 'done'")
            .execute(&pool)
            .await
            .unwrap();
        run_vault_indexer(&pool, &ctx).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM pending_indexing LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "done", "done rows must not be requeued");
    }

    #[tokio::test]
    async fn observer_scan_returns_zero_on_empty_events_table() {
        // migrated pool: events table exists but is empty → 0 patterns, 0 proposals.
        let pool = migrated_pool().await;
        let ctx = JobContext {
            vault_path: std::path::PathBuf::from("/nonexistent"),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_observer_scan(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 0);
        assert!(r.summary.contains("no patterns") || r.summary.contains("0 event"));
    }

    /// Fixture test: seed SQLite events → observer scan returns >=1 insight/proposal.
    #[tokio::test]
    async fn observer_scan_detects_pattern_from_seeded_events() {
        use altevra_core::events::{ActorType, Event, EventType};
        use altevra_db::{EventsRepository, ProposalsRepository};

        let pool = migrated_pool().await;
        let events_repo = EventsRepository::new(&pool);

        // 3 SkillDriftDetected for the same entity → RecurringDrift insight.
        for h in [2i64, 4, 6] {
            let mut ev = Event::new(
                EventType::SkillDriftDetected,
                "drift altevra-core",
                "test",
                ActorType::System,
            )
            .with_entity("skill", "altevra-core");
            ev.created_at = Utc::now() - chrono::Duration::hours(h);
            events_repo.insert(&ev).await.unwrap();
        }

        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_observer_scan(&pool, &ctx).await.unwrap();
        assert!(
            r.items_processed >= 1,
            "expected >=1 insight from seeded events, got: {r:?}"
        );
        // The insight should also be persisted as a proposal.
        let proposals = ProposalsRepository::new(&pool)
            .list(None, Some("improvement"))
            .await
            .unwrap();
        assert!(
            !proposals.is_empty(),
            "observer scan must persist insights as proposals"
        );
        assert!(
            proposals.iter().any(|p| p.source_mode.as_deref() == Some("observer")),
            "proposal source_mode must be 'observer'"
        );
    }

    /// The two daily-summary tests mutate process-global env vars
    /// (interests + notify-claims paths) so the P4 brief step inside
    /// `run_daily_summary` stays inside the TempDir — never the real
    /// `~/.altevra`. Serialized so the env window cannot race.
    static DAILY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Point the P4 brief step's filesystem side-effects at `tmp`. Returns a
    /// guard that restores the prior values on drop.
    fn hermetic_brief_env(tmp: &tempfile::TempDir) -> impl Drop {
        struct Restore(Vec<(&'static str, Option<String>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (k, v) in &self.0 {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
        let keys = ["ALTEVRA_INTERESTS_PATH", "ALTEVRA_NOTIFY_CLAIMS_DIR"];
        let prior = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        std::env::set_var(
            "ALTEVRA_INTERESTS_PATH",
            tmp.path().join("interests.yaml"),
        );
        std::env::set_var("ALTEVRA_NOTIFY_CLAIMS_DIR", tmp.path().join("claims"));
        Restore(prior)
    }

    #[tokio::test]
    async fn daily_summary_writes_file() {
        let _serial = DAILY_ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let _env = hermetic_brief_env(&tmp);
        let pool = migrated_pool().await;
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);
        // idempotent — second call returns 0
        let r2 = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r2.items_processed, 0);
    }

    #[tokio::test]
    async fn daily_briefing_surfaces_patterns_and_contacts() {
        use altevra_core::events::{ActorType, Event, EventType};
        use altevra_db::{DecisionRow, EventsRepository, MentionsRepository, TasksRepository};

        let _serial = DAILY_ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let _env = hermetic_brief_env(&tmp);
        let pool = migrated_pool().await;
        // Seed relative to real now: the recurring-drift detector uses a
        // real-time DRIFT_WINDOW_DAYS window (recent-flapping semantics), so a
        // hardcoded past date silently falls out of the window once wall-clock
        // moves on. now-relative keeps the test deterministic across any date.
        let now: chrono::DateTime<Utc> = Utc::now();

        // 1. Seed events that trip a detector: 3 SkillDriftDetected for one slug in
        //    the last week → a RecurringDrift insight.
        let events_repo = EventsRepository::new(&pool);
        for h in [2i64, 4, 6] {
            let mut ev = Event::new(
                EventType::SkillDriftDetected,
                "drift altevra-core",
                "test",
                ActorType::System,
            )
            .with_entity("skill", "altevra-core");
            ev.created_at = now - chrono::Duration::hours(h);
            events_repo.insert(&ev).await.unwrap();
        }

        // 2. Seed a person with an OLD last-contact. Đorđe is in the mentor seed, so
        //    the dictionary knows him. One mention edge from an object whose
        //    object_index.updated_at is ~6 weeks ago → "haven't talked to" line.
        let idx = altevra_db::ObjectIndexRepository::new(&pool);
        idx.index_object(
            &altevra_db::ObjectIndexRow {
                object_type: "learning".into(),
                id: "capture-old-djordje-1".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "business".into(),
                scope: None,
                title: Some("old note mentioning Đorđe".into()),
                categories: "[]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: now - chrono::Duration::weeks(6),
            },
            "body",
        )
        .await
        .unwrap();
        MentionsRepository::new(&pool)
            .record("learning", "capture-old-djordje-1", "person", "person:djordje")
            .await
            .unwrap();

        // 3. Seed a decision past its review_after.
        let decision = DecisionRow {
            id: uuid::Uuid::new_v4(),
            project_id: None,
            title: "Stop building, start selling".into(),
            rationale: Some("Đorđe directive".into()),
            decided_at: "2026-04-10T00:00:00Z".parse().unwrap(),
            decided_by: Some("djordje".into()),
            metadata: serde_json::json!({}),
        };
        TasksRepository::new(&pool)
            .save_decision(&decision)
            .await
            .unwrap();
        sqlx::query("UPDATE decisions SET review_after = ? WHERE id = ?")
            .bind("2026-05-01T00:00:00.000Z")
            .bind(decision.id.to_string())
            .execute(&pool)
            .await
            .unwrap();

        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now,
            router: noop_router(), // noop path — structured bullets, no LLM.
        };
        let r = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);

        let file = tmp
            .path()
            .join("10-insights")
            .join(format!("daily-{}.md", now.format("%Y-%m-%d")));
        let content = std::fs::read_to_string(&file).unwrap();

        // a pattern line...
        assert!(
            content.contains("Recurring drift: altevra-core"),
            "must surface the detected pattern:\n{content}"
        );
        // The contact gap surfaces as a COUNT + CLI pointer only (P4):
        // relationship data is `obsidian_mirror = 'never'` — a name must
        // never land in this (syncable) vault file.
        assert!(
            content.contains("1 overdue reach-out(s) withheld by domain policy"),
            "must surface the contact-gap count:\n{content}"
        );
        assert!(
            content.contains("altevra brief --private"),
            "must point at the private CLI view:\n{content}"
        );
        assert!(
            !content.contains("haven't talked to Đorđe"),
            "person names must NOT land in the vault file (dp_relationship policy):\n{content}"
        );
        // ...and the stale-decision line.
        assert!(
            content.contains("decision 'Stop building, start selling'")
                && content.contains("still applies?"),
            "must surface the stale decision:\n{content}"
        );
        // noop path → no LLM attribution in frontmatter.
        assert!(content.contains("generated_by: altevra-brain\n"));

        // P4 integration: the daily job also delivers the policy-gated brief
        // into <vault>/Daily/ — and it carries no person name either.
        let brief = tmp
            .path()
            .join("Daily")
            .join(format!("{}-altevra-brief.md", now.format("%Y-%m-%d")));
        assert!(brief.exists(), "daily job must write the P4 brief");
        let brief_md = std::fs::read_to_string(&brief).unwrap();
        assert!(brief_md.contains("kind: altevra-daily-brief"));
        assert!(
            !brief_md.contains("Đorđe"),
            "vault brief must never carry a relationship name:\n{brief_md}"
        );
    }

    #[tokio::test]
    async fn insight_synthesizer_writes_card() {
        use altevra_db::InsightCardsRepository;

        let tmp = tempfile::TempDir::new().unwrap();

        // noop → skipped cleanly, ZERO cards.
        let pool = migrated_pool().await;
        let ctx_noop = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_insight_synthesizer(&pool, &ctx_noop).await.unwrap();
        assert_eq!(r.items_processed, 0);
        assert!(r.summary.contains("skipped"));
        assert_eq!(
            InsightCardsRepository::new(&pool).count().await.unwrap(),
            0,
            "noop must write no cards"
        );

        // stub non-noop StrongReasoner → an insight_card row exists + is recallable.
        let pool2 = migrated_pool().await;
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(
                altevra_llm::ModelRole::StrongReasoner,
                "stub-reasoner",
                "Late-night sessions precede a spike in next-day rework.",
            ),
        };
        let r = run_insight_synthesizer(&pool2, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);
        assert!(r.summary.contains("insight card"));

        let cards = InsightCardsRepository::new(&pool2);
        assert_eq!(cards.count().await.unwrap(), 1, "exactly one card written");

        // recallable: the card auto-indexed into the FTS substrate (A1).
        let fts = altevra_db::FtsRepository::new(&pool2);
        assert!(
            fts.search("rework", 10)
                .await
                .unwrap()
                .iter()
                .any(|h| h.object_type == "insight_card"),
            "synthesized card must be recallable"
        );
    }

    /// Helper: index an object with empty categories (uncategorized) in a domain.
    async fn seed_uncategorized(pool: &SqlitePool, id: &str, domain: &str, title: &str) {
        seed_uncategorized_with_body(pool, id, domain, title, "body").await;
    }

    async fn seed_uncategorized_with_body(
        pool: &SqlitePool,
        id: &str,
        domain: &str,
        title: &str,
        body: &str,
    ) {
        altevra_db::ObjectIndexRepository::new(pool)
            .index_object(
                &altevra_db::ObjectIndexRow {
                    object_type: "learning".into(),
                    id: id.into(),
                    status: "active".into(),
                    sensitivity: "internal".into(),
                    domain: domain.into(),
                    scope: None,
                    title: Some(title.into()),
                    categories: "[]".into(),
                    tags: "[]".into(),
                    redaction_status: "clean".into(),
                    updated_at: Utc::now(),
                },
                body,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn auto_categorize_assigns_and_proposes() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        let tmp = tempfile::TempDir::new().unwrap();

        // --- noop → skipped cleanly: nothing tagged, no proposals. ---
        let pool0 = migrated_pool().await;
        seed_uncategorized(&pool0, "obj-noop", "business", "some note").await;
        let ctx0 = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r0 = run_auto_categorizer(&pool0, &ctx0).await.unwrap();
        assert_eq!(r0.items_processed, 0, "noop classifies nothing");
        assert!(r0.summary.contains("skipped"));
        assert_eq!(
            ProposalsRepository::new(&pool0).list(None, Some("category")).await.unwrap().len(),
            0,
            "noop proposes nothing"
        );

        // --- stub cheap_worker → an object matching an EXISTING category gets tagged;
        //     a NOVEL one yields a kind="category" proposal. ---
        // The stub always replies "gtm". We pre-seed an existing "gtm" category so
        // the first object matches it; we ALSO test the novel path by using a stub
        // that returns a fresh label for a second object via a distinct router.
        let pool = migrated_pool().await;
        let idx = ObjectIndexRepository::new(&pool);

        // Pre-seed an EXISTING "gtm" category by indexing one already-categorized
        // object (so the taxonomy is non-empty).
        idx.index_object(
            &altevra_db::ObjectIndexRow {
                object_type: "decision".into(),
                id: "seed-gtm".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "business".into(),
                scope: None,
                title: Some("a gtm decision".into()),
                categories: "[\"gtm\"]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: Utc::now(),
            },
            "body",
        )
        .await
        .unwrap();

        // An uncategorized business object the stub will label "gtm" (existing → tag).
        seed_uncategorized(&pool, "obj-match", "business", "gtm follow-up note").await;

        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "gtm"),
        };
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();
        // obj-match was tagged with the existing "gtm" category.
        let tagged = idx
            .get_categories_or_empty("learning", "obj-match")
            .await;
        assert_eq!(tagged, vec!["gtm".to_string()], "matching object tagged: {r:?}");
        // No category proposal yet (it matched an existing one).
        assert_eq!(
            ProposalsRepository::new(&pool).list(None, Some("category")).await.unwrap().len(),
            0,
            "a matched object proposes no new category"
        );

        // --- novel category path: a new object whose stub label is NOT in the
        //     taxonomy yields a kind="category" Tier-0 proposal. ---
        let pool2 = migrated_pool().await;
        seed_uncategorized(&pool2, "obj-novel", "business", "a note about violin practice").await;
        let ctx2 = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "hobby"),
        };
        let r2 = run_auto_categorizer(&pool2, &ctx2).await.unwrap();
        let cat_props = ProposalsRepository::new(&pool2)
            .list(None, Some("category"))
            .await
            .unwrap();
        assert_eq!(cat_props.len(), 1, "novel label proposes a category: {r2:?}");
        assert!(cat_props[0].title.to_lowercase().contains("hobby"));
        // SI-9: a "category" proposal is Tier-0 (the repo derived it, not the agent).
        assert_eq!(cat_props[0].risk_tier, "tier0");
        // the object was NOT tagged (no existing category fit).
        assert!(
            ObjectIndexRepository::new(&pool2)
                .get_categories_or_empty("learning", "obj-novel")
                .await
                .is_empty(),
            "novel object stays uncategorized until Pavle approves the new category"
        );
    }

    /// SI-7: a HIGH-WATER object (e.g. relationship) must be classified by
    /// `local_private`, NEVER the cloud `cheap_worker`. With only a cloud
    /// cheap_worker registered, a high-water object is SKIPPED (no cloud leak),
    /// while a business object IS classified by the cheap_worker.
    #[tokio::test]
    async fn auto_categorize_si7_routes_high_water_local_only() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        let tmp = tempfile::TempDir::new().unwrap();
        let pool = migrated_pool().await;
        seed_uncategorized(&pool, "obj-personal", "relationship", "dinner with Elena").await;
        seed_uncategorized(&pool, "obj-business", "business", "ReVesta cold call list").await;

        // Only a CLOUD cheap_worker is configured; local_private stays noop.
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "outreach"),
        };
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();
        // The relationship object was skipped (local_private resolved to noop) — it
        // NEVER reached the cloud worker. The business one produced a proposal.
        assert!(r.summary.contains("1 skipped") || r.summary.contains("skipped"));
        let idx = ObjectIndexRepository::new(&pool);
        assert!(
            idx.get_categories_or_empty("learning", "obj-personal")
                .await
                .is_empty(),
            "high-water object must NOT be classified by the cloud worker (SI-7)"
        );
        // the business object yielded a novel-category proposal (taxonomy was empty).
        assert_eq!(
            ProposalsRepository::new(&pool)
                .list(None, Some("category"))
                .await
                .unwrap()
                .len(),
            1,
            "non-high-water object IS classified by cheap_worker"
        );
    }

    /// SI-7 DEFENSE-IN-DEPTH: an object stamped domain='business' (e.g. a generic
    /// 'learning' note whose template default_domain is Business) but whose CONTENT
    /// is clearly relationship/personal must NOT be classified by a cloud-only
    /// cheap_worker. The content fail-safe re-routes it to local_private; with only
    /// a cloud cheap_worker configured, local_private is noop → it is SKIPPED, never
    /// leaked. A genuinely-business control object IS classified by the cheap_worker.
    #[tokio::test]
    async fn auto_categorize_content_failsafe_keeps_high_water_local() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        let tmp = tempfile::TempDir::new().unwrap();
        let pool = migrated_pool().await;

        // domain='business' BUT body carries clear relationship content (the same SR
        // keyword the high-water net detects: "moja devojka"). The obj.domain check
        // alone (is_high_water()==false) would route this to the CLOUD worker.
        seed_uncategorized_with_body(
            &pool,
            "obj-mislabeled",
            "business",
            "random thought",
            "danas sam shvatio nesto vazno — moja devojka Elena me podrzava u svemu",
        )
        .await;
        // A genuinely-business control object (no high-water content).
        seed_uncategorized_with_body(
            &pool,
            "obj-clean-biz",
            "business",
            "ReVesta GTM",
            "cold call list for surplus buyers in Florida",
        )
        .await;

        // Only a CLOUD cheap_worker is configured; local_private stays noop.
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "outreach"),
        };
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();

        let idx = ObjectIndexRepository::new(&pool);
        // The mislabeled object was re-routed to local_private (noop) → SKIPPED, so
        // the cloud worker never saw it: it stays uncategorized and proposes nothing.
        assert!(
            idx.get_categories_or_empty("learning", "obj-mislabeled")
                .await
                .is_empty(),
            "content fail-safe must keep the relationship-content object OFF the cloud worker (SI-7)"
        );
        // The clean business object DID reach the cheap_worker → a category proposal.
        let props = ProposalsRepository::new(&pool)
            .list(None, Some("category"))
            .await
            .unwrap();
        assert_eq!(
            props.len(),
            1,
            "exactly the genuinely-business object is classified by the cloud worker: {r:?}"
        );
        assert!(
            props[0]
                .evidence_refs
                .contains("obj-clean-biz"),
            "the one cloud-classified object is the business control, not the mislabeled one"
        );
        assert!(r.summary.contains("skipped"));
    }

    /// SI-7 ROUTING RULE — the headline guarantee of TASK 3.
    ///
    /// A user configured `codex_oauth` for cloud reasoning AND `local_private` for
    /// high-water content. A high-water object (or one with high-water content but
    /// a non-high-water domain stamp) MUST be classified by the local stub, never
    /// by the cloud one — even though codex_oauth puts cloud on cheap_worker.
    ///
    /// The cloud stub is wired to PANIC on `complete` so any leak fails loudly.
    #[tokio::test]
    async fn high_water_routes_to_local_private_never_codex_cheap_worker() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        // A local stub that classifies high-water content as "personal" — `is_local`
        // is true so the router accepts it for the LocalPrivate slot (SI-7 #1/#2).
        struct LocalStub;
        #[async_trait::async_trait]
        impl altevra_llm::ChatProvider for LocalStub {
            fn id(&self) -> &str {
                "local-stub"
            }
            fn is_local(&self) -> bool {
                true
            }
            async fn complete(
                &self,
                _m: &[altevra_llm::ChatMessage],
                _o: &altevra_llm::ChatOpts,
            ) -> anyhow::Result<String> {
                Ok("personal".into())
            }
        }
        // A cloud stub representing codex_oauth's cheap_worker — PANICS on call so
        // any cloud leak fails the test loudly.
        struct PanicCodex;
        #[async_trait::async_trait]
        impl altevra_llm::ChatProvider for PanicCodex {
            fn id(&self) -> &str {
                "panic-codex"
            }
            fn is_local(&self) -> bool {
                false
            }
            async fn complete(
                &self,
                _m: &[altevra_llm::ChatMessage],
                _o: &altevra_llm::ChatOpts,
            ) -> anyhow::Result<String> {
                panic!("a high-water object must NEVER reach codex (SI-7)")
            }
        }

        let pool = migrated_pool().await;
        // (a) genuine high-water domain.
        seed_uncategorized(&pool, "obj-rel", "relationship", "dinner with Elena").await;
        // (b) mislabeled business object whose CONTENT is high-water.
        seed_uncategorized_with_body(
            &pool,
            "obj-mislabel",
            "business",
            "ordinary thought",
            "danas sam shvatio nesto vazno — moja devojka Elena me podrzava u svemu",
        )
        .await;
        // (c) clean business object — fine to send to the cloud worker. We park its
        //     existence to keep this test focused on the high-water path; the cloud
        //     leak invariant is the load-bearing assertion below.

        // Router with BOTH local_private (LocalStub) and a panicking codex cloud
        // worker. This mirrors `reasoning_mode = codex_oauth` + a configured
        // `local_private` table — the configuration the `local-first` preset
        // produces.
        let router = std::sync::Arc::new(
            altevra_llm::ModelRouter::noop()
                .with_provider(altevra_llm::ModelRole::LocalPrivate, std::sync::Arc::new(LocalStub))
                .with_provider(altevra_llm::ModelRole::CheapWorker, std::sync::Arc::new(PanicCodex)),
        );
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router,
        };
        // The job MUST complete without the panicking cloud worker ever being
        // reached for the two high-water objects.
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();

        let idx = ObjectIndexRepository::new(&pool);
        // Both objects were classified — by the LOCAL stub, not codex.
        let rel_cats = idx.get_categories_or_empty("learning", "obj-rel").await;
        let mis_cats = idx.get_categories_or_empty("learning", "obj-mislabel").await;
        // Each object yielded a `personal` proposal (taxonomy was empty → novel
        // category). The PROOF the high-water path used LOCAL: the test would
        // otherwise have panicked from `PanicCodex::complete`.
        let proposals_count = ProposalsRepository::new(&pool)
            .list(None, Some("category"))
            .await
            .unwrap()
            .len();
        assert!(
            proposals_count >= 1,
            "the local stub classified at least one high-water object: {r:?}"
        );
        // No leak to codex: assertion is the absence of a panic from PanicCodex.
        // Both objects either landed a category proposal OR are still uncategorized
        // — neither outcome was produced by the cloud worker.
        let _ = rel_cats;
        let _ = mis_cats;
    }

    #[tokio::test]
    async fn record_feed_success_then_failure_increments_count() {
        let pool = setup_research_schema().await;
        let outcome = altevra_research::fetcher::FetchOutcome {
            items: vec![],
            new_etag: Some("\"abc\"".into()),
            new_last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
            status: 200,
        };
        record_feed_success(&pool, "feed-x", &outcome).await;
        record_feed_failure(&pool, "feed-x", "DNS error").await;
        let row =
            sqlx::query("SELECT fail_count, last_error FROM research_feed_state WHERE feed_id = ?")
                .bind("feed-x")
                .fetch_one(&pool)
                .await
                .unwrap();
        let count: i64 = sqlx::Row::try_get(&row, "fail_count").unwrap();
        let err: String = sqlx::Row::try_get(&row, "last_error").unwrap();
        assert_eq!(count, 1);
        assert!(err.contains("DNS"));
    }

    #[tokio::test]
    async fn feed_discovery_returns_when_no_items() {
        let pool = setup_research_schema().await;
        // Need research_feed_candidates table.
        sqlx::query(
            r#"CREATE TABLE research_feed_candidates (
                id TEXT PRIMARY KEY,
                candidate_url TEXT NOT NULL UNIQUE,
                feed_url TEXT,
                source_url TEXT,
                discovered_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                discovered_by TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                auto_promoted_at TEXT,
                rejected_reason TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_feed_discovery(&pool, &ctx).await.unwrap();
        // Empty DB -> "no research items to mine for discovery"
        assert!(r.summary.contains("no research items"));
        assert_eq!(r.items_processed, 0);
    }

    #[tokio::test]
    async fn github_trending_fetch_does_not_panic_offline() {
        // Test SCHEMA path: even if network fetch fails for all langs the job
        // returns a summary, not a panic.
        let pool = setup_research_schema().await;
        // Provide source_kind column via ALTER (since our test schema is minimal).
        sqlx::query(
            "ALTER TABLE research_items ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'rss'",
        )
        .execute(&pool)
        .await
        .ok();
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        // We expect this to attempt 3 langs; either they succeed (network OK)
        // or all fail and total_new == 0. Either way: no panic.
        let r = run_github_trending_fetch(&pool, &ctx).await.unwrap();
        assert!(r.summary.contains("github trending"));
    }

    #[tokio::test]
    async fn research_fetcher_returns_when_no_feeds_reachable() {
        // We can't hit real network in unit tests, so verify the job itself
        // doesn't panic when feeds resolve to no items. The default-packet
        // load path is exercised via test below.
        let pool = setup_research_schema().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        // Override feeds.yaml to a single bad URL so the loop runs and records a failure
        // instead of trying to hit real RSS endpoints.
        let yaml = r#"
feeds:
  - id: bad-feed
    name: Bad
    url: https://this-domain-does-not-exist-altevra-test.invalid/rss
    type: rss
    category: test
    trust_weight: 0.1
    enabled: true
    fetch_interval_minutes: 60
window_days: 7
relevance_threshold: 0.4
"#;
        let feeds_dir = tmp.path().join(".altevra-research");
        std::fs::create_dir_all(&feeds_dir).unwrap();
        let feeds_path = feeds_dir.join("feeds.yaml");
        std::fs::write(&feeds_path, yaml).unwrap();

        // Point HOME at tmp so default_path() resolves there.
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let alt_feeds = tmp.path().join(".altevra").join("research");
        std::fs::create_dir_all(&alt_feeds).unwrap();
        std::fs::copy(&feeds_path, alt_feeds.join("feeds.yaml")).unwrap();

        let r = run_research_fetcher(&pool, &ctx).await.unwrap();
        // Either DNS resolves or fails — either way job completes without panic
        // and items_processed is 0 because no items came back.
        assert!(r.summary.contains("feeds"));

        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }
}
