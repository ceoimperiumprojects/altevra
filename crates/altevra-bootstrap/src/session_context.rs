//! SessionStart context assembly (PLAN-ALIVE §P2) — the DB half.
//!
//! Gathers the injected items (active goals, last decisions, open proposals,
//! Tool Register summary) and enforces the two MANDATORY §P2.4 mechanisms:
//!
//!  * **Sensitivity filter:** EVERY candidate item passes
//!    [`ExposureGate::decide`] with the work/agent audience request
//!    ([`ExposureRequest::default_work`]). High-water-domain / Restricted /
//!    Unscanned items are EXCLUDED. Recency alone never selects a
//!    relationship/health decision into a coding-session context.
//!  * **Audit:** every evaluated item (included AND excluded) writes one
//!    content-free `exposure_decisions` row (migration 021) keyed by the
//!    caller's `audit_ref` — counts + coarse reason only, never an id/title.
//!
//! Failure semantics (§P2.4, locked): a filter/audit error excludes THAT item
//! (fail-closed per item); a section-level error empties that section; only a
//! total assembly error (pool/deadline, handled by the callers) yields the
//! empty block (fail-open for availability).

use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::safety::{ExposureDecision, ExposureGate, ExposureRequest};
use altevra_core::security::Sensitivity;
use altevra_core::session_context::{
    render_session_context_block, SessionContextData, ToolSummary,
};
use altevra_core::status::{ObjectStatus, RedactionStatus};
use altevra_core::Domain;
use altevra_db::{
    ExposureAudit, ExposureDecisionsRepository, ObjectIndexRepository, ProposalsRepository,
    ToolRecordsRepository,
};
use altevra_secrets::guard_text;
use chrono::Utc;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Section caps — the block is a digest, not a dump (§P2.5 budget).
pub const GOALS_CAP: usize = 5;
pub const DECISIONS_CAP: usize = 3;
pub const PROPOSALS_CAP: usize = 5;
pub const CURATED_TOOLS_CAP: usize = 20;
/// Bound on how many candidates a section evaluates (gate + audit) per run.
const SECTION_EVAL_CAP: usize = 25;

/// Default goals store — the same `$HOME/.altevra/state/goals.json` the MCP
/// `get_goals` tool reads.
pub fn default_goals_path() -> PathBuf {
    altevra_core::home_dir().join(".altevra/state/goals.json")
}

/// One evaluated candidate: what the gate saw, and the guarded text to inject
/// if allowed.
struct GatedCandidate {
    text: String,
    domain: Domain,
    sensitivity: Sensitivity,
    redaction: RedactionStatus,
}

impl GatedCandidate {
    fn decide(&self, request: &ExposureRequest) -> ExposureDecision {
        let mut env = Envelope::new(
            // The envelope id is NEVER persisted by the audit (content-free
            // aggregates only) — a constant placeholder is deliberate.
            "session-context-item",
            "session_context_item",
            Utc::now(),
            Provenance::new(ProvenanceOrigin::Imported),
        );
        env.domain = self.domain.clone();
        env.sensitivity = self.sensitivity.clone();
        env.status = ObjectStatus::Active;
        ExposureGate::decide(&env, &self.redaction, request)
    }
}

/// Write ONE content-free `exposure_decisions` row for an evaluated item
/// (§P2.4: every injection writes an audit row; excluded items are audited
/// too with their coarse reason). Returns `false` on write failure — the
/// caller MUST then exclude the item (fail-closed per item).
async fn audit_item(
    pool: &SqlitePool,
    audit_ref: &str,
    section: &str,
    request: &ExposureRequest,
    decision: &ExposureDecision,
    redaction: &RedactionStatus,
) -> bool {
    let allowed = decision.is_allowed();
    let audit = ExposureAudit {
        packet_id: Some(format!("{audit_ref}:{section}")),
        sensitivity_ceiling: request.sensitivity_ceiling.to_string(),
        domain_scope: request.domain_scope.iter().map(|d| d.to_string()).collect(),
        included_count: usize::from(allowed),
        excluded_count: usize::from(!allowed),
        excluded_by_reason: match decision {
            ExposureDecision::Allow => vec![],
            ExposureDecision::Deny(r) => vec![(r.code().to_string(), 1)],
        },
        redaction_counts: if allowed {
            vec![(redaction.to_string(), 1)]
        } else {
            vec![]
        },
        truncated: false,
    };
    ExposureDecisionsRepository::new(pool)
        .insert(&audit)
        .await
        .is_ok()
}

/// Gate + audit one candidate. `Some(text)` iff the gate allowed it AND the
/// audit row landed (audit failure ⇒ excluded, fail-closed per item).
async fn admit(
    pool: &SqlitePool,
    audit_ref: &str,
    section: &str,
    request: &ExposureRequest,
    cand: GatedCandidate,
) -> Option<String> {
    let decision = cand.decide(request);
    let audited = audit_item(pool, audit_ref, section, request, &decision, &cand.redaction).await;
    if decision.is_allowed() && audited {
        Some(cand.text)
    } else {
        None
    }
}

/// Assemble the gated, audited session context. Never errors: a failing
/// section degrades to empty (the renderer then degrades an all-empty
/// assembly to the empty string). `goals_path` defaults to
/// [`default_goals_path`] — tests pass an explicit temp path.
pub async fn gather_session_context(
    pool: &SqlitePool,
    audit_ref: &str,
    goals_path: Option<&Path>,
) -> SessionContextData {
    let request = ExposureRequest::default_work();
    let mut data = SessionContextData::default();

    // ---- Active goals (goals.json — guarded at injection, then gated) ----
    let goals_file = goals_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_goals_path);
    for cand in load_goal_candidates(&goals_file) {
        if data.goals.len() >= GOALS_CAP {
            break;
        }
        if let Some(text) = admit(pool, audit_ref, "goals", &request, cand).await {
            data.goals.push(text);
        }
    }

    // ---- Last decisions (object-envelope store; newest first, gated) ----
    let decisions = ObjectIndexRepository::new(pool)
        .candidates(None)
        .await
        .unwrap_or_default();
    for row in decisions
        .into_iter()
        .filter(|r| r.object_type == "decision")
        .take(SECTION_EVAL_CAP)
    {
        if data.decisions.len() >= DECISIONS_CAP {
            break;
        }
        let cand = GatedCandidate {
            text: row.title.clone().unwrap_or_else(|| row.id.clone()),
            // Sensitivity/Domain FromStr are infallible: unknown sensitivity
            // ranks max and unknown domain falls outside the work scope —
            // both fail-closed at the gate.
            domain: row.domain.parse().unwrap_or_default(),
            sensitivity: row.sensitivity.parse().unwrap_or_default(),
            // Unknown redaction → Unscanned → gate fails closed.
            redaction: row
                .redaction_status
                .parse()
                .unwrap_or(RedactionStatus::Unscanned),
        };
        if let Some(text) = admit(pool, audit_ref, "decisions", &request, cand).await {
            data.decisions.push(text);
        }
    }

    // ---- Open proposals (review queue; titles guarded at injection) ----
    let proposals = ProposalsRepository::new(pool)
        .list(Some("proposed"), None)
        .await
        .unwrap_or_default();
    for p in proposals.into_iter().take(SECTION_EVAL_CAP) {
        if data.proposals.len() >= PROPOSALS_CAP {
            break;
        }
        // Proposal titles are agent-authored free text: scan them HERE so the
        // gate sees a real verdict (guard raises sensitivity on risk).
        let g = guard_text(&p.title, Sensitivity::Internal);
        let cand = GatedCandidate {
            text: format!("{} — {}/{}", g.value, p.kind, p.risk_tier),
            domain: Domain::Business,
            sensitivity: g.sensitivity,
            redaction: g.redaction_status,
        };
        if let Some(text) = admit(pool, audit_ref, "proposals", &request, cand).await {
            data.proposals.push(text);
        }
    }

    // ---- Tool Register summary (counts + curated tools, gated too) ----
    let tools = ToolRecordsRepository::new(pool)
        .list(None, None)
        .await
        .unwrap_or_default();
    data.tools_total = tools.len();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in &tools {
        *counts.entry(t.kind.clone()).or_insert(0) += 1;
    }
    data.tool_counts = counts.into_iter().collect();
    // Curated/seeded rows first — `list` already orders `source='manual'`
    // ahead; they carry the human-curated invocations.
    for t in tools.iter().filter(|t| t.source == "manual") {
        if data.curated_tools.len() >= CURATED_TOOLS_CAP {
            break;
        }
        let invocation = t
            .invocation
            .get("canonical")
            .and_then(|v| v.as_str())
            .unwrap_or("(no invocation recorded)")
            .to_string();
        let cand = GatedCandidate {
            text: t.name.clone(),
            domain: Domain::Business,
            // tool_records fields were guarded at upsert (§P1.3) — Redacted is
            // the honest exposable status for guard-scrubbed text.
            sensitivity: Sensitivity::Internal,
            redaction: RedactionStatus::Redacted,
        };
        if let Some(name) = admit(pool, audit_ref, "tools", &request, cand).await {
            data.curated_tools.push(ToolSummary {
                name,
                kind: t.kind.clone(),
                invocation,
            });
        }
    }

    data
}

/// Load goal candidates from `goals.json`. Accepts plain strings or objects
/// (`title`/`text`/`goal`, optional `status`/`domain`/`sensitivity`). Done/
/// cancelled goals are skipped; declared domain/sensitivity are honored
/// (unknown values parse fail-closed) and combined with the guard verdict.
fn load_goal_candidates(path: &Path) -> Vec<GatedCandidate> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return vec![];
    };
    let Some(arr) = value.as_array() else {
        return vec![];
    };
    let mut out = Vec::new();
    for item in arr.iter().take(SECTION_EVAL_CAP) {
        let (title, status, domain, declared_sens) = match item {
            serde_json::Value::String(s) => (s.clone(), None, None, None),
            serde_json::Value::Object(m) => {
                let title = ["title", "text", "goal"]
                    .iter()
                    .find_map(|k| m.get(*k).and_then(|v| v.as_str()))
                    .map(String::from);
                let Some(title) = title else { continue };
                (
                    title,
                    m.get("status").and_then(|v| v.as_str()).map(String::from),
                    m.get("domain").and_then(|v| v.as_str()).map(String::from),
                    m.get("sensitivity")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                )
            }
            _ => continue,
        };
        if matches!(
            status.as_deref(),
            Some("done") | Some("completed") | Some("cancelled") | Some("archived")
        ) {
            continue;
        }
        // Goals are user-authored free text in a plain JSON file (Unscanned by
        // nature) — scan at injection so the gate's redaction requirement is
        // met by a REAL verdict, never assumed.
        let g = guard_text(&title, Sensitivity::Internal);
        let declared = declared_sens
            .map(|s| s.parse::<Sensitivity>().unwrap_or_default())
            .unwrap_or(Sensitivity::Internal);
        out.push(GatedCandidate {
            text: g.value,
            domain: domain
                .map(|d| d.parse::<Domain>().unwrap_or_default())
                .unwrap_or(Domain::Business),
            sensitivity: g.sensitivity.combine(&declared),
            redaction: g.redaction_status,
        });
    }
    out
}

/// Convenience for the bootstrap-packet surfaces (MCP `get_agent_bootstrap_packet`
/// + `altevra agent bootstrap`): gather, render, and return both the curated
/// tool summaries and the rendered block. Fault-tolerant by construction.
pub async fn bootstrap_context(
    pool: &SqlitePool,
    audit_ref: &str,
) -> (Vec<ToolSummary>, Option<String>) {
    let data = gather_session_context(pool, audit_ref, None).await;
    let block = render_session_context_block(&data);
    let tools = data.curated_tools;
    (tools, if block.is_empty() { None } else { Some(block) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{ObjectIndexRow, ToolRecordRow};
    use tempfile::TempDir;

    async fn pool(tmp: &TempDir) -> SqlitePool {
        let p = altevra_db::create_pool(&tmp.path().join("a.db").to_string_lossy())
            .await
            .unwrap();
        altevra_db::run_migrations(&p).await.unwrap();
        p
    }

    fn decision_row(id: &str, title: &str, domain: &str, sens: &str, red: &str) -> ObjectIndexRow {
        ObjectIndexRow {
            object_type: "decision".into(),
            id: id.into(),
            status: "active".into(),
            sensitivity: sens.into(),
            domain: domain.into(),
            scope: None,
            title: Some(title.into()),
            categories: "[\"business\"]".into(),
            tags: "[]".into(),
            redaction_status: red.into(),
            updated_at: Utc::now(),
        }
    }

    async fn audit_rows(pool: &SqlitePool, audit_ref: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM exposure_decisions WHERE packet_id LIKE ?")
            .bind(format!("{audit_ref}:%"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn gather_includes_goals_decisions_tools_and_audits_each_item() {
        let tmp = TempDir::new().unwrap();
        let p = pool(&tmp).await;

        // goals.json — explicit temp path, never $HOME.
        let goals = tmp.path().join("goals.json");
        std::fs::write(
            &goals,
            serde_json::json!([
                {"title": "2 paying Simple Surplus clients", "status": "open"},
                {"title": "old goal", "status": "done"},
                "Ship Altevra P2"
            ])
            .to_string(),
        )
        .unwrap();

        // decisions in the object-envelope store.
        let idx = ObjectIndexRepository::new(&p);
        idx.upsert(&decision_row("d1", "ONE canonical DB", "business", "internal", "clean"))
            .await
            .unwrap();

        // a curated tool.
        let mut t = ToolRecordRow::new("imperium-crawl", "cli");
        t.invocation = serde_json::json!({"canonical": "imperium-crawl <cmd>"});
        t.source = "manual".into();
        ToolRecordsRepository::new(&p).upsert(&t).await.unwrap();

        let data = gather_session_context(&p, "session_start:test1", Some(&goals)).await;
        assert_eq!(
            data.goals,
            vec!["2 paying Simple Surplus clients".to_string(), "Ship Altevra P2".to_string()],
            "active goals included, done goal skipped"
        );
        assert_eq!(data.decisions, vec!["ONE canonical DB".to_string()]);
        assert_eq!(data.curated_tools.len(), 1);
        assert_eq!(data.curated_tools[0].invocation, "imperium-crawl <cmd>");
        assert_eq!(data.tools_total, 1);

        // §P2.4: one exposure_decisions row PER evaluated item
        // (2 goals + 1 decision + 1 tool = 4).
        assert_eq!(audit_rows(&p, "session_start:test1").await, 4);

        let block = render_session_context_block(&data);
        assert!(block.contains("2 paying Simple Surplus clients"));
        assert!(block.contains("ONE canonical DB"));
        assert!(block.contains("=== ALTEVRA TOOL REGISTER ==="));
    }

    #[tokio::test]
    async fn restricted_and_high_water_and_unscanned_decisions_are_excluded() {
        // THE §P2.4 leak test: recency alone must never select a relationship/
        // health/Restricted/Unscanned decision into a coding-session context.
        let tmp = TempDir::new().unwrap();
        let p = pool(&tmp).await;
        let idx = ObjectIndexRepository::new(&p);
        idx.upsert(&decision_row("d_ok", "Business decision", "business", "internal", "clean"))
            .await
            .unwrap();
        idx.upsert(&decision_row(
            "d_restricted",
            "Private health decision",
            "health",
            "restricted",
            "clean",
        ))
        .await
        .unwrap();
        idx.upsert(&decision_row(
            "d_highwater",
            "Relationship decision",
            "relationship",
            "internal",
            "clean",
        ))
        .await
        .unwrap();
        idx.upsert(&decision_row(
            "d_unscanned",
            "Unscanned decision",
            "business",
            "internal",
            "unscanned",
        ))
        .await
        .unwrap();

        let data = gather_session_context(&p, "session_start:test2", None).await;
        assert_eq!(data.decisions, vec!["Business decision".to_string()]);
        let block = render_session_context_block(&data);
        assert!(!block.contains("Private health decision"), "Restricted leaked");
        assert!(!block.contains("Relationship decision"), "high-water domain leaked");
        assert!(!block.contains("Unscanned decision"), "Unscanned leaked (fail-closed)");

        // all 4 evaluated decisions are audited (1 included + 3 excluded).
        assert_eq!(audit_rows(&p, "session_start:test2").await, 4);
        let excluded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM exposure_decisions \
             WHERE packet_id = 'session_start:test2:decisions' \
             AND excluded_refs LIKE '%\"count\":1%'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(excluded, 3, "each excluded decision writes its own audit row");
    }

    #[tokio::test]
    async fn goal_with_secret_is_redacted_and_high_water_goal_excluded() {
        let tmp = TempDir::new().unwrap();
        let p = pool(&tmp).await;
        let goals = tmp.path().join("goals.json");
        std::fs::write(
            &goals,
            serde_json::json!([
                {"title": "rotate key sk-FIXTUREfixtureFIXTUREfixture0000 in prod"},
                {"title": "talk to therapist weekly", "domain": "health"}
            ])
            .to_string(),
        )
        .unwrap();
        let data = gather_session_context(&p, "session_start:test3", Some(&goals)).await;
        let block = render_session_context_block(&data);
        assert!(!block.contains("sk-FIXTURE"), "raw secret leaked into the block");
        assert!(!block.contains("therapist"), "health-domain goal leaked");
    }

    #[tokio::test]
    async fn empty_db_renders_empty_block() {
        let tmp = TempDir::new().unwrap();
        let p = pool(&tmp).await;
        let (tools, block) = bootstrap_context(&p, "bootstrap:test").await;
        assert!(tools.is_empty());
        assert!(block.is_none(), "empty assembly degrades to no block");
    }
}
