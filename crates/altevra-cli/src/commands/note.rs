//! `altevra note` — the personal brain capture surface (PLAN-ALIVE §P5).
//!
//! ROUTING (LOCKED §P5.1 — kinds with canonical stores are FK-pointers,
//! never parallel rows):
//!   person       → 029 `persons` upsert (merge by name)
//!   relationship → 029 `relationships` (requires --person)
//!   preference   → 029 `preferences` ("key = value" / "key: value" text)
//!   decision     → `TasksRepository::save_decision_indexed` — the
//!                  object-envelope store P2's `gather_session_context`
//!                  reads, so a note-added decision shows up in the next
//!                  SessionStart injection
//!   goal         → the goals.json store (`default_goals_path`) P2 reads —
//!                  same SessionStart visibility
//!   place|idea|mood|health|memory|reference|habit|routine|value|
//!   identity_shift|life_event → `personal_notes` (migration 039)
//!
//! Every free-text write is guarded at the persistence boundary (the
//! repository guards DB rows; this layer guards the file-backed goal store)
//! and sightings land in `secret_sightings`. High-water domains
//! (personal/relationship/health/financial/...) get the 024 policy floor +
//! `review_required` inside `PersonalNotesRepository`.

use altevra_core::security::Sensitivity;
use altevra_db::{
    create_pool, run_migrations, DecisionIndexEnvelope, DecisionRow, ObjectIndexRepository,
    PersonalNoteRow, PersonalNotesRepository, TasksRepository, CANONICAL_STORE_KINDS,
    PERSONAL_NOTE_KINDS,
};
use altevra_secrets::guard_text;
use clap::{Args, Subcommand};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum NoteCommands {
    /// Capture a note. Kind decides the canonical store it lands in.
    Add(NoteAddArgs),
    /// List notes across the reconciled stores.
    List(NoteListArgs),
}

#[derive(Args)]
pub struct NoteAddArgs {
    /// Note kind: person|relationship|preference|decision|goal|place|idea|
    /// mood|health|memory|reference|habit|routine|value|identity_shift|life_event.
    pub kind: String,

    /// The note text.
    pub text: String,

    /// Person to link/target (required for `relationship`; for `person` the
    /// text becomes the note about this person).
    #[arg(long)]
    pub person: Option<String>,

    /// Declared sensitivity floor (the guard/domain policy only ever raises it).
    #[arg(long)]
    pub sensitivity: Option<String>,

    /// Domain override (personal|health|relationship|business|...).
    #[arg(long)]
    pub domain: Option<String>,

    /// Goals store override (tests; defaults to ~/.altevra/state/goals.json).
    #[arg(long, hide = true)]
    pub goals_file: Option<PathBuf>,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct NoteListArgs {
    /// Filter by kind (any add-able kind).
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by domain.
    #[arg(long)]
    pub domain: Option<String>,

    #[arg(long)]
    pub json: bool,

    /// Goals store override (tests; defaults to ~/.altevra/state/goals.json).
    #[arg(long, hide = true)]
    pub goals_file: Option<PathBuf>,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

pub async fn run(cmd: NoteCommands) -> anyhow::Result<()> {
    match cmd {
        NoteCommands::Add(args) => run_add(args).await,
        NoteCommands::List(args) => run_list(args).await,
    }
}

fn goals_path(over: &Option<PathBuf>) -> PathBuf {
    over.clone()
        .unwrap_or_else(altevra_bootstrap::session_context::default_goals_path)
}

async fn run_add(args: NoteAddArgs) -> anyhow::Result<()> {
    if crate::commands::brain::refuse_if_maintenance_locked("note add") {
        return Ok(());
    }
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let gp = goals_path(&args.goals_file);
    let summary = add_note(
        &pool,
        &gp,
        &args.kind,
        &args.text,
        args.person.as_deref(),
        args.sensitivity.as_deref(),
        args.domain.as_deref(),
    )
    .await?;
    println!("{summary}");
    Ok(())
}

/// Route one note into its canonical store. Returns a one-line summary.
/// Hermetic core — tests pass a temp pool + temp goals path.
pub async fn add_note(
    pool: &SqlitePool,
    goals_file: &Path,
    kind: &str,
    text: &str,
    person: Option<&str>,
    sensitivity: Option<&str>,
    domain: Option<&str>,
) -> anyhow::Result<String> {
    // Declared floor: decision/goal live in the business-facing stores P2
    // injects (Internal default — the work-audience ceiling); everything
    // personal defaults Confidential (the 029/dp_personal posture). The
    // guard / domain policy only ever RAISES from here.
    let declared: Sensitivity = sensitivity
        .map(|s| s.parse().unwrap_or_default())
        .unwrap_or(match kind {
            "decision" | "goal" => Sensitivity::Internal,
            _ => Sensitivity::Confidential,
        });
    let repo = PersonalNotesRepository::new(pool);

    match kind {
        // ---- FK-pointer kinds → 029 canonical stores ----------------------
        "person" => {
            let (name, note) = match person {
                Some(p) => (p, Some(text)),
                None => (text, None),
            };
            let id = repo.upsert_person(name, note).await?;
            Ok(format!("person upserted in 029 persons (id {id})"))
        }
        "relationship" => {
            let Some(name) = person else {
                anyhow::bail!("`note add relationship` requires --person <name>");
            };
            let person_id = repo.upsert_person(name, None).await?;
            // "mentor: monthly calls" → kind "mentor", note "monthly calls".
            let (rel_kind, note) = match text.split_once(':') {
                Some((k, n)) => (k.trim(), Some(n.trim())),
                None => (text.trim(), None),
            };
            let id = repo.add_relationship(&person_id, rel_kind, note).await?;
            Ok(format!("relationship recorded in 029 relationships (id {id})"))
        }
        "preference" => {
            // "key = value" / "key: value" → split; bare text → general pref.
            let (key, value) = text
                .split_once('=')
                .or_else(|| text.split_once(':'))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .unwrap_or_else(|| ("general".to_string(), text.trim().to_string()));
            let id = repo.add_preference(&key, &value).await?;
            Ok(format!("preference recorded in 029 preferences (id {id})"))
        }

        // ---- decision → object-envelope store P2 reads --------------------
        "decision" => {
            let g = guard_text(text, declared.clone());
            let d = DecisionRow {
                id: uuid::Uuid::new_v4(),
                project_id: None,
                title: g.value.clone(),
                rationale: None,
                decided_at: chrono::Utc::now(),
                decided_by: Some("pavle".to_string()),
                metadata: serde_json::json!({"source": "note_add"}),
            };
            let idx = DecisionIndexEnvelope {
                status: "active".to_string(),
                sensitivity: g.sensitivity.combine(&declared).to_string(),
                domain: domain.unwrap_or("business").to_string(),
                scope: None,
                categories: "[\"decision\"]".to_string(),
                tags: "[]".to_string(),
                redaction_status: g.redaction_status.to_string(),
            };
            TasksRepository::new(pool).save_decision_indexed(&d, &idx).await?;
            repo.record_external_sightings(
                &g.sightings,
                &format!("decision_note:{}", d.id),
                "decision_title",
            )
            .await?;
            Ok(format!(
                "decision saved + indexed (id {}) — visible to SessionStart injection",
                d.id
            ))
        }

        // ---- goal → goals.json store P2 reads -----------------------------
        "goal" => {
            let g = guard_text(text, declared);
            repo.record_external_sightings(
                &g.sightings,
                &format!("goal_note:{}", uuid::Uuid::new_v4()),
                "goal_title",
            )
            .await?;
            append_goal(goals_file, &g.value, domain, sensitivity)?;
            Ok("goal appended to goals store — visible to SessionStart injection".to_string())
        }

        // ---- net-new kinds → personal_notes (039) --------------------------
        k if PERSONAL_NOTE_KINDS.contains(&k) => {
            let mut row = PersonalNoteRow::new(k, text);
            if let Some(d) = domain {
                row.domain = d.to_string();
            }
            row.sensitivity = declared.to_string();
            if let Some(name) = person {
                row.person_id = Some(repo.upsert_person(name, None).await?);
            }
            let sightings = repo.insert(&row).await?;
            Ok(format!(
                "{k} note saved in personal_notes (id {}){}",
                row.id,
                if sightings > 0 {
                    format!(" — {sightings} secret(s) redacted + logged")
                } else {
                    String::new()
                }
            ))
        }
        other => anyhow::bail!(
            "unknown note kind '{other}' — expected one of {CANONICAL_STORE_KINDS:?} or {PERSONAL_NOTE_KINDS:?}"
        ),
    }
}

/// Append one goal object to the goals.json array (the exact store
/// `gather_session_context` loads). Creates the file/dirs on first use.
fn append_goal(
    path: &Path,
    title: &str,
    domain: Option<&str>,
    sensitivity: Option<&str>,
) -> anyhow::Result<()> {
    let mut goals: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    let mut entry = serde_json::json!({"title": title, "status": "open"});
    if let Some(d) = domain {
        entry["domain"] = serde_json::json!(d);
    }
    if let Some(s) = sensitivity {
        entry["sensitivity"] = serde_json::json!(s);
    }
    match goals.as_array_mut() {
        Some(arr) => arr.push(entry),
        None => goals = serde_json::json!([entry]),
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&goals)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// List — reads ACROSS the reconciled stores.
// ---------------------------------------------------------------------------

async fn run_list(args: NoteListArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let gp = goals_path(&args.goals_file);
    let entries = list_notes(&pool, &gp, args.kind.as_deref(), args.domain.as_deref()).await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": entries.len(),
                "notes": entries,
            }))?
        );
    } else if entries.is_empty() {
        println!("No notes (try `altevra note add idea \"...\"`).");
    } else {
        println!("{} note(s):", entries.len());
        for e in &entries {
            println!(
                "  [{:13}] ({:12}) {}",
                e["kind"].as_str().unwrap_or("-"),
                e["store"].as_str().unwrap_or("-"),
                e["text"].as_str().unwrap_or("-"),
            );
        }
    }
    Ok(())
}

/// Gather notes across personal_notes + 029 persons/relationships/preferences
/// + the decision object-envelope store + goals.json. Hermetic core.
pub async fn list_notes(
    pool: &SqlitePool,
    goals_file: &Path,
    kind: Option<&str>,
    domain: Option<&str>,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    let want = |k: &str| kind.is_none() || kind == Some(k);
    let domain_ok = |d: &str| domain.is_none() || domain == Some(d);

    // personal_notes (net-new kinds) — kind filter pushed into SQL.
    let net_new_kind = kind.filter(|k| PERSONAL_NOTE_KINDS.contains(k));
    if kind.is_none() || net_new_kind.is_some() {
        for n in PersonalNotesRepository::new(pool)
            .list(net_new_kind, domain)
            .await?
        {
            out.push(serde_json::json!({
                "store": "personal_notes",
                "kind": n.kind,
                "text": n.body,
                "domain": n.domain,
                "sensitivity": n.sensitivity,
                "review_required": n.review_required,
                "person_id": n.person_id,
                "created_at": n.created_at,
            }));
        }
    }

    // 029 persons (domain: relationship).
    if want("person") && domain_ok("relationship") {
        let rows = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT name, note FROM persons WHERE status = 'active' ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
        for (name, note) in rows {
            out.push(serde_json::json!({
                "store": "persons",
                "kind": "person",
                "text": match &note { Some(n) => format!("{name} — {n}"), None => name },
                "domain": "relationship",
            }));
        }
    }

    // 029 relationships.
    if want("relationship") && domain_ok("relationship") {
        let rows = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT p.name, r.kind, r.note FROM relationships r \
             JOIN persons p ON p.id = r.person_id \
             WHERE r.status = 'active' ORDER BY p.name",
        )
        .fetch_all(pool)
        .await?;
        for (name, rkind, note) in rows {
            out.push(serde_json::json!({
                "store": "relationships",
                "kind": "relationship",
                "text": match &note {
                    Some(n) => format!("{name}: {rkind} — {n}"),
                    None => format!("{name}: {rkind}"),
                },
                "domain": "relationship",
            }));
        }
    }

    // 029 preferences (domain: personal).
    if want("preference") && domain_ok("personal") {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT pref_key, pref_value FROM preferences WHERE status = 'active' \
             ORDER BY pref_key",
        )
        .fetch_all(pool)
        .await?;
        for (k, v) in rows {
            out.push(serde_json::json!({
                "store": "preferences",
                "kind": "preference",
                "text": format!("{k} = {v}"),
                "domain": "personal",
            }));
        }
    }

    // decisions — the SAME object-envelope store P2 reads.
    if want("decision") {
        let rows = ObjectIndexRepository::new(pool).candidates(None).await?;
        for r in rows.into_iter().filter(|r| r.object_type == "decision") {
            if !domain_ok(&r.domain) {
                continue;
            }
            out.push(serde_json::json!({
                "store": "object_index",
                "kind": "decision",
                "text": r.title.unwrap_or(r.id),
                "domain": r.domain,
                "sensitivity": r.sensitivity,
            }));
        }
    }

    // goals — the SAME goals.json store P2 reads.
    if want("goal") {
        if let Ok(raw) = std::fs::read_to_string(goals_file) {
            if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(&raw) {
                for item in arr {
                    let (title, gdomain) = match &item {
                        serde_json::Value::String(s) => (Some(s.clone()), None),
                        serde_json::Value::Object(m) => (
                            ["title", "text", "goal"]
                                .iter()
                                .find_map(|k| m.get(*k).and_then(|v| v.as_str()))
                                .map(String::from),
                            m.get("domain").and_then(|v| v.as_str()).map(String::from),
                        ),
                        _ => (None, None),
                    };
                    let Some(title) = title else { continue };
                    if let Some(d) = domain {
                        if gdomain.as_deref() != Some(d) {
                            continue;
                        }
                    }
                    out.push(serde_json::json!({
                        "store": "goals",
                        "kind": "goal",
                        "text": title,
                        "domain": gdomain,
                    }));
                }
            }
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests — hermetic (per-test TempDir DBs + temp goals files, never ~/.altevra)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_bootstrap::gather_session_context;
    use tempfile::TempDir;

    async fn temp_pool(tmp: &TempDir) -> SqlitePool {
        let db = tmp.path().join("altevra.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn goal_note_lands_in_goals_store_and_session_start_injection() {
        // THE §P5 round-trip: a note-added goal must appear in the EXACT
        // store P2's gather_session_context reads — and therefore in the
        // next SessionStart injection.
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("state/goals.json");

        add_note(&pool, &goals, "goal", "2 paying Simple Surplus clients", None, None, None)
            .await
            .unwrap();

        let raw = std::fs::read_to_string(&goals).unwrap();
        assert!(raw.contains("2 paying Simple Surplus clients"));

        let data = gather_session_context(&pool, "note_test:goal", Some(&goals)).await;
        assert_eq!(
            data.goals,
            vec!["2 paying Simple Surplus clients".to_string()],
            "note-added goal must show up in the SessionStart context"
        );
    }

    #[tokio::test]
    async fn decision_note_lands_in_object_envelope_store_p2_reads() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("goals.json");

        add_note(&pool, &goals, "decision", "ONE canonical DB, no silos", None, None, None)
            .await
            .unwrap();

        // The decision is in object_index (the P2 candidate source)...
        let rows = ObjectIndexRepository::new(&pool).candidates(None).await.unwrap();
        let decisions: Vec<_> = rows.iter().filter(|r| r.object_type == "decision").collect();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].title.as_deref(), Some("ONE canonical DB, no silos"));

        // ...and visible to the SessionStart assembly itself.
        let data = gather_session_context(&pool, "note_test:decision", Some(&goals)).await;
        assert_eq!(data.decisions, vec!["ONE canonical DB, no silos".to_string()]);
    }

    #[tokio::test]
    async fn person_relationship_preference_route_to_029_tables() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("goals.json");

        add_note(&pool, &goals, "person", "Mentor, VP People @ HTEC", Some("Srđan Jovanović"), None, None)
            .await
            .unwrap();
        add_note(&pool, &goals, "relationship", "mentor: monthly calls", Some("Srđan Jovanović"), None, None)
            .await
            .unwrap();
        add_note(&pool, &goals, "preference", "coding.style = small verified increments", None, None, None)
            .await
            .unwrap();

        let persons: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM persons")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persons, 1, "person + relationship notes share ONE 029 person row");
        let rels: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM relationships")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rels, 1);
        let prefs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM preferences")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(prefs, 1);
        // NO parallel rows in personal_notes for FK kinds (the LOCKED rule).
        let parallel: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_notes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(parallel, 0, "FK-pointer kinds must never land in personal_notes");
    }

    #[tokio::test]
    async fn net_new_kinds_land_in_personal_notes_with_links() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("goals.json");

        add_note(&pool, &goals, "idea", "embed wiki pages nightly", None, None, None)
            .await
            .unwrap();
        add_note(&pool, &goals, "memory", "coffee with Elena at Kalemegdan", Some("Elena"), None, None)
            .await
            .unwrap();

        let repo = PersonalNotesRepository::new(&pool);
        assert_eq!(repo.list(Some("idea"), None).await.unwrap().len(), 1);
        let memories = repo.list(Some("memory"), None).await.unwrap();
        assert_eq!(memories.len(), 1);
        assert!(memories[0].person_id.is_some(), "--person links the 029 person row");
        let pname: String = sqlx::query_scalar("SELECT name FROM persons WHERE id = ?")
            .bind(memories[0].person_id.as_deref().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pname, "Elena");
    }

    #[tokio::test]
    async fn unknown_kind_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("goals.json");
        assert!(add_note(&pool, &goals, "wishlist", "x", None, None, None).await.is_err());
    }

    #[tokio::test]
    async fn list_reads_across_all_reconciled_stores() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("goals.json");

        add_note(&pool, &goals, "goal", "Ship Altevra P5", None, None, None).await.unwrap();
        add_note(&pool, &goals, "decision", "Extend 029, never migrate away", None, None, None)
            .await
            .unwrap();
        add_note(&pool, &goals, "person", "", Some("Elena"), None, None).await.unwrap();
        add_note(&pool, &goals, "preference", "music: Nils Frahm", None, None, None)
            .await
            .unwrap();
        add_note(&pool, &goals, "idea", "morning brief in Obsidian", None, None, None)
            .await
            .unwrap();

        let all = list_notes(&pool, &goals, None, None).await.unwrap();
        let stores: Vec<_> = all.iter().filter_map(|e| e["store"].as_str()).collect();
        for store in ["goals", "object_index", "persons", "preferences", "personal_notes"] {
            assert!(stores.contains(&store), "missing store {store} in {stores:?}");
        }

        // kind filter routes to a single store.
        let only_goals = list_notes(&pool, &goals, Some("goal"), None).await.unwrap();
        assert_eq!(only_goals.len(), 1);
        assert_eq!(only_goals[0]["text"], "Ship Altevra P5");

        // domain filter: the idea note is personal-domain; decisions are business.
        let personal = list_notes(&pool, &goals, None, Some("personal")).await.unwrap();
        assert!(personal.iter().any(|e| e["kind"] == "idea"));
        assert!(!personal.iter().any(|e| e["kind"] == "decision"));
    }

    #[tokio::test]
    async fn goal_note_with_embedded_token_is_redacted_before_the_file_store() {
        // goals.json is a plain file — the guard must run BEFORE the write
        // (the DB-side stores guard inside the repository).
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let goals = tmp.path().join("goals.json");
        add_note(
            &pool,
            &goals,
            "goal",
            "rotate key sk-FIXTUREfixtureFIXTUREfixture0000 in prod",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let raw = std::fs::read_to_string(&goals).unwrap();
        assert!(!raw.contains("sk-FIXTURE"), "raw secret written to goals.json: {raw}");
        assert!(raw.contains("[REDACTED]"));
        let sightings: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM secret_sightings WHERE location = 'goal_title'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(sightings >= 1, "goal-note secret must be sighted");
    }
}
