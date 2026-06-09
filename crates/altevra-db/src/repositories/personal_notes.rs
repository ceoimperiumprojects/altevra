//! Personal brain notes (PLAN-ALIVE §P5) — `personal_notes` (migration 039)
//! plus the FK-pointer write paths into the 029 canonical stores.
//!
//! The LOCKED schema decision: kinds that already have canonical stores are
//! FK-POINTERS, never parallel rows. This repository therefore owns:
//!  - the NET-NEW kinds table `personal_notes` (place/idea/mood/health/memory/
//!    reference/habit/routine/value/identity_shift/life_event);
//!  - thin guarded write paths into the 029 `persons` / `relationships` /
//!    `preferences` tables (person/relationship/preference notes route THERE).
//!  Decision/goal notes route to `TasksRepository::save_decision_indexed` and
//!  the goals.json store at the CLI layer (the exact stores P2's
//!  `gather_session_context` reads — two sources of truth is how drift starts).
//!
//! Security (mandatory, same contract as `tool_records`): EVERY free-text
//! field passes `guard_text` at the persistence boundary — a personal note is
//! exactly where a pasted token/PII would otherwise persist raw. Detections
//! land in `secret_sightings` (fingerprint only).
//!
//! High-water enforcement: a note in a high-water domain (personal/
//! relationship/health/financial/client/legal — seeded `local_private` +
//! `cloud_sync='disabled'` in 024) is raised to the domain policy's
//! `default_sensitivity` floor and forced `review_required = 1` (trust
//! ladder: sensitive memory needs Pavle's review).

use altevra_core::security::Sensitivity;
use altevra_secrets::{guard_text, SecretSighting};
use chrono::Utc;
use sqlx::{Row, SqlitePool};

use super::domain_policy::{DomainPolicyRepository, EmbeddingModelRole};
use super::tool_records::record_sightings;
use crate::util::ts_to_text;

/// The net-new note kinds stored in `personal_notes` (LOCKED §P5.1).
pub const PERSONAL_NOTE_KINDS: &[&str] = &[
    "place",
    "idea",
    "mood",
    "health",
    "memory",
    "reference",
    "habit",
    "routine",
    "value",
    "identity_shift",
    "life_event",
];

/// Kinds whose canonical store is elsewhere — FK-pointers, never rows here.
pub const CANONICAL_STORE_KINDS: &[&str] =
    &["person", "relationship", "preference", "decision", "goal"];

/// The default domain a net-new kind lands in when the caller doesn't say.
pub fn default_domain_for_kind(kind: &str) -> &'static str {
    match kind {
        "health" | "mood" => "health",
        _ => "personal",
    }
}

#[derive(Debug, Clone)]
pub struct PersonalNoteRow {
    pub id: String,
    pub kind: String,
    pub body: String,
    pub domain: String,
    pub sensitivity: String,
    pub review_required: bool,
    pub status: String,
    pub tags: serde_json::Value,
    pub categories: serde_json::Value,
    pub redaction_status: String,
    pub person_id: Option<String>,
    pub project_id: Option<String>,
    pub created_at: String,
}

impl PersonalNoteRow {
    /// A minimal row: domain defaults per kind, sensitivity/review/redaction
    /// are resolved at insert (policy floor + guard verdict).
    pub fn new(kind: &str, body: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: kind.to_string(),
            body: body.to_string(),
            domain: default_domain_for_kind(kind).to_string(),
            sensitivity: "confidential".to_string(),
            review_required: false,
            status: "active".to_string(),
            tags: serde_json::json!([]),
            categories: serde_json::json!(["personal_note"]),
            redaction_status: "unscanned".to_string(),
            person_id: None,
            project_id: None,
            created_at: ts_to_text(&Utc::now()),
        }
    }
}

pub struct PersonalNotesRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> PersonalNotesRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a NET-NEW-kind note. The body is guarded HERE (never trusted
    /// from the caller); sensitivity is raised to max(declared, guard verdict,
    /// domain-policy default); high-water domains force `review_required`.
    /// Returns the number of secret sightings recorded.
    pub async fn insert(&self, row: &PersonalNoteRow) -> anyhow::Result<usize> {
        if !PERSONAL_NOTE_KINDS.contains(&row.kind.as_str()) {
            anyhow::bail!(
                "personal_note kind '{}' is not one of {PERSONAL_NOTE_KINDS:?} \
                 (person/relationship/preference/decision/goal live in their canonical stores)",
                row.kind
            );
        }

        // ---- guard the body at the persistence boundary (mandatory) ----
        let declared: Sensitivity = row.sensitivity.parse().unwrap_or_default();
        let g = guard_text(&row.body, declared.clone());
        let mut sensitivity = g.sensitivity.combine(&declared);

        // ---- high-water enforcement: policy floor + forced review ----
        let policy_repo = DomainPolicyRepository::new(self.pool);
        let policy = policy_repo.get(&row.domain).await?;
        let high_water = match &policy {
            Some(p) => {
                let floor: Sensitivity = p.default_sensitivity.parse().unwrap_or_default();
                sensitivity = sensitivity.combine(&floor);
                EmbeddingModelRole::parse(&p.embedding_model_role)
                    == EmbeddingModelRole::LocalPrivate
            }
            // Unknown domain → fail-closed: treat as high-water (the same
            // posture as embedding_role_for / cloud_sync_for).
            None => {
                sensitivity = sensitivity.combine(&Sensitivity::Restricted);
                true
            }
        };
        let review_required = row.review_required || high_water;

        let (g_tags, mut sightings) = super::tool_records::guard_value(&row.tags);
        sightings.extend(g.sightings.clone());

        sqlx::query(
            "INSERT INTO personal_notes \
             (id, kind, body, domain, sensitivity, review_required, status, \
              tags, categories, redaction_status, person_id, project_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.kind)
        .bind(&g.value)
        .bind(&row.domain)
        .bind(sensitivity.to_string())
        .bind(review_required as i64)
        .bind(&row.status)
        .bind(g_tags.to_string())
        .bind(row.categories.to_string())
        .bind(g.redaction_status.to_string())
        .bind(row.person_id.as_deref())
        .bind(row.project_id.as_deref())
        .bind(&row.created_at)
        .bind(&row.created_at)
        .execute(self.pool)
        .await?;

        let source_ref = format!("personal_note:{}", row.id);
        record_sightings(self.pool, &sightings, &source_ref, "personal_note_body").await?;
        Ok(sightings.len())
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<PersonalNoteRow>> {
        let row = sqlx::query(&format!("SELECT {COLS} FROM personal_notes WHERE id = ?"))
            .bind(id)
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(row_to_note))
    }

    /// List active notes, optionally filtered by kind and/or domain. Newest
    /// first.
    pub async fn list(
        &self,
        kind: Option<&str>,
        domain: Option<&str>,
    ) -> anyhow::Result<Vec<PersonalNoteRow>> {
        let order = "AND status = 'active' ORDER BY created_at DESC";
        let rows = match (kind, domain) {
            (Some(k), Some(d)) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM personal_notes WHERE kind = ? AND domain = ? {order}"
                ))
                .bind(k)
                .bind(d)
                .fetch_all(self.pool)
                .await?
            }
            (Some(k), None) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM personal_notes WHERE kind = ? {order}"
                ))
                .bind(k)
                .fetch_all(self.pool)
                .await?
            }
            (None, Some(d)) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM personal_notes WHERE domain = ? {order}"
                ))
                .bind(d)
                .fetch_all(self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM personal_notes WHERE 1=1 {order}"
                ))
                .fetch_all(self.pool)
                .await?
            }
        };
        Ok(rows.into_iter().map(row_to_note).collect())
    }

    /// Notes linked to a 029 person row.
    pub async fn list_for_person(&self, person_id: &str) -> anyhow::Result<Vec<PersonalNoteRow>> {
        let rows = sqlx::query(&format!(
            "SELECT {COLS} FROM personal_notes WHERE person_id = ? AND status = 'active' \
             ORDER BY created_at DESC"
        ))
        .bind(person_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_note).collect())
    }

    /// Record sightings produced by a CALLER-side guard — for the note paths
    /// whose store is a plain file (goals.json) or another repository
    /// (decision titles). Fingerprints only, idempotent on
    /// `(fingerprint, source_ref)`.
    pub async fn record_external_sightings(
        &self,
        sightings: &[SecretSighting],
        source_ref: &str,
        location: &str,
    ) -> anyhow::Result<usize> {
        record_sightings(self.pool, sightings, source_ref, location).await
    }

    // -----------------------------------------------------------------------
    // FK-pointer write paths into the 029 canonical stores (LOCKED §P5.1).
    // -----------------------------------------------------------------------

    /// Upsert into the 029 `persons` table by exact name (guarded). Returns
    /// the person id (existing or new). A provided `note` replaces the stored
    /// one (029 keeps `updated_at`).
    pub async fn upsert_person(
        &self,
        name: &str,
        note: Option<&str>,
    ) -> anyhow::Result<String> {
        let g_name = guard_text(name, Sensitivity::Restricted);
        let mut sightings = g_name.sightings.clone();
        let g_note = note.map(|n| {
            let g = guard_text(n, Sensitivity::Restricted);
            sightings.extend(g.sightings.clone());
            g
        });

        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM persons WHERE name = ?")
                .bind(&g_name.value)
                .fetch_optional(self.pool)
                .await?;
        let id = match existing {
            Some(id) => {
                if let Some(g) = &g_note {
                    sqlx::query("UPDATE persons SET note = ?, updated_at = ? WHERE id = ?")
                        .bind(&g.value)
                        .bind(ts_to_text(&Utc::now()))
                        .bind(&id)
                        .execute(self.pool)
                        .await?;
                }
                id
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                // 029 defaults carry domain='relationship', sensitivity='restricted'.
                sqlx::query("INSERT INTO persons (id, name, note) VALUES (?, ?, ?)")
                    .bind(&id)
                    .bind(&g_name.value)
                    .bind(g_note.as_ref().map(|g| g.value.as_str()))
                    .execute(self.pool)
                    .await?;
                id
            }
        };
        record_sightings(
            self.pool,
            &sightings,
            &format!("person:{id}"),
            "persons_fields",
        )
        .await?;
        Ok(id)
    }

    /// Insert into the 029 `relationships` table (guarded), linked to a
    /// person row. Returns the relationship id.
    pub async fn add_relationship(
        &self,
        person_id: &str,
        rel_kind: &str,
        note: Option<&str>,
    ) -> anyhow::Result<String> {
        let g_kind = guard_text(rel_kind, Sensitivity::Restricted);
        let mut sightings = g_kind.sightings.clone();
        let g_note = note.map(|n| {
            let g = guard_text(n, Sensitivity::Restricted);
            sightings.extend(g.sightings.clone());
            g
        });
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO relationships (id, person_id, kind, note) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(person_id)
            .bind(&g_kind.value)
            .bind(g_note.as_ref().map(|g| g.value.as_str()))
            .execute(self.pool)
            .await?;
        record_sightings(
            self.pool,
            &sightings,
            &format!("relationship:{id}"),
            "relationships_fields",
        )
        .await?;
        Ok(id)
    }

    /// Insert into the 029 `preferences` table (guarded). Returns the
    /// preference id.
    pub async fn add_preference(&self, key: &str, value: &str) -> anyhow::Result<String> {
        let g_key = guard_text(key, Sensitivity::Confidential);
        let g_value = guard_text(value, Sensitivity::Confidential);
        let mut sightings = g_key.sightings.clone();
        sightings.extend(g_value.sightings.clone());
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO preferences (id, pref_key, pref_value) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(&g_key.value)
            .bind(&g_value.value)
            .execute(self.pool)
            .await?;
        record_sightings(
            self.pool,
            &sightings,
            &format!("preference:{id}"),
            "preferences_fields",
        )
        .await?;
        Ok(id)
    }
}

const COLS: &str = "id, kind, body, domain, sensitivity, review_required, status, \
                    tags, categories, redaction_status, person_id, project_id, created_at";

fn row_to_note(r: sqlx::sqlite::SqliteRow) -> PersonalNoteRow {
    PersonalNoteRow {
        id: r.get("id"),
        kind: r.get("kind"),
        body: r.get("body"),
        domain: r.get("domain"),
        sensitivity: r.get("sensitivity"),
        review_required: r.get::<i64, _>("review_required") != 0,
        status: r.get("status"),
        tags: serde_json::from_str(&r.get::<String, _>("tags"))
            .unwrap_or(serde_json::json!([])),
        categories: serde_json::from_str(&r.get::<String, _>("categories"))
            .unwrap_or(serde_json::json!(["personal_note"])),
        redaction_status: r.get("redaction_status"),
        person_id: r.get("person_id"),
        project_id: r.get("project_id"),
        created_at: r.get("created_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};
    use crate::repositories::domain_policy::CloudSync;

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn round_trip_insert_get_list_by_kind_and_domain() {
        let p = pool().await;
        let repo = PersonalNotesRepository::new(&p);
        repo.insert(&PersonalNoteRow::new("idea", "embed wiki pages nightly"))
            .await
            .unwrap();
        repo.insert(&PersonalNoteRow::new("place", "Kalemegdan at sunset"))
            .await
            .unwrap();
        repo.insert(&PersonalNoteRow::new("health", "slept 7h, no headache"))
            .await
            .unwrap();

        let ideas = repo.list(Some("idea"), None).await.unwrap();
        assert_eq!(ideas.len(), 1);
        assert_eq!(ideas[0].body, "embed wiki pages nightly");
        let got = repo.get(&ideas[0].id).await.unwrap().unwrap();
        assert_eq!(got.kind, "idea");
        assert_eq!(got.domain, "personal");

        // domain filter: health-kind note defaults into the health domain.
        let health = repo.list(None, Some("health")).await.unwrap();
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].kind, "health");
        assert_eq!(repo.list(None, None).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn canonical_store_kinds_are_rejected_here() {
        // person/relationship/preference/decision/goal are FK-pointers to
        // their canonical stores — a parallel row here is the drift the
        // LOCKED decision forbids.
        let p = pool().await;
        let repo = PersonalNotesRepository::new(&p);
        for kind in CANONICAL_STORE_KINDS {
            assert!(
                repo.insert(&PersonalNoteRow::new(kind, "x")).await.is_err(),
                "kind '{kind}' must be refused by personal_notes"
            );
        }
    }

    #[tokio::test]
    async fn note_body_with_embedded_token_is_redacted_and_sighted() {
        // §P5 gate (guard at the persistence boundary, same as tool_records):
        // a pasted key in a personal note must never persist raw.
        let p = pool().await;
        let repo = PersonalNotesRepository::new(&p);
        let row = PersonalNoteRow::new(
            "memory",
            "told Danilo the staging key sk-FIXTUREfixtureFIXTUREfixture0000 over call",
        );
        let n = repo.insert(&row).await.unwrap();
        assert!(n >= 1, "the embedded key must be sighted");

        let got = repo.get(&row.id).await.unwrap().unwrap();
        assert!(!got.body.contains("sk-FIXTURE"), "raw secret persisted: {}", got.body);
        assert!(got.body.contains("[REDACTED]"));
        assert_eq!(got.redaction_status, "redacted");

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM secret_sightings WHERE source_ref = ?",
        )
        .bind(format!("personal_note:{}", row.id))
        .fetch_one(&p)
        .await
        .unwrap();
        assert!(count >= 1, "sighting must be logged");
    }

    #[tokio::test]
    async fn high_water_health_note_is_restricted_review_required_and_local_only() {
        // §P5.3 gate: a health note resolves to ONLY local providers — the
        // external/cloud route is denied by policy BEFORE any prompt
        // construction (domain_policies seeds health local_private +
        // cloud_sync=disabled; the LLM router's SI-7 invariant refuses any
        // non-local provider for the local_private role).
        let p = pool().await;
        let repo = PersonalNotesRepository::new(&p);
        let row = PersonalNoteRow::new("health", "knee pain after deadlifts");
        assert_eq!(row.domain, "health");
        repo.insert(&row).await.unwrap();

        let got = repo.get(&row.id).await.unwrap().unwrap();
        // 024 dp_health default_sensitivity = restricted → floor applied.
        assert_eq!(got.sensitivity, "restricted");
        assert!(got.review_required, "high-water note must require review");

        let policy = DomainPolicyRepository::new(&p);
        assert_eq!(
            policy
                .embedding_role_for(&[got.domain.clone()])
                .await
                .unwrap(),
            EmbeddingModelRole::LocalPrivate,
            "health note must route local_private ONLY"
        );
        assert_eq!(
            policy.get(&got.domain).await.unwrap().unwrap().cloud_sync,
            CloudSync::Disabled,
            "the external/cloud route must be denied for a health note"
        );

        // Every high-water domain a note can carry behaves the same.
        for d in ["personal", "relationship", "financial"] {
            assert_eq!(
                policy.embedding_role_for(&[d.to_string()]).await.unwrap(),
                EmbeddingModelRole::LocalPrivate,
                "{d}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_domain_fails_closed_high_water() {
        let p = pool().await;
        let repo = PersonalNotesRepository::new(&p);
        let mut row = PersonalNoteRow::new("idea", "note in a domain we know nothing about");
        row.domain = "mystery".to_string();
        repo.insert(&row).await.unwrap();
        let got = repo.get(&row.id).await.unwrap().unwrap();
        assert_eq!(got.sensitivity, "restricted", "unknown domain → fail-closed");
        assert!(got.review_required);
    }

    #[tokio::test]
    async fn person_preference_relationship_land_in_029_tables_and_link() {
        let p = pool().await;
        let repo = PersonalNotesRepository::new(&p);

        // person → 029 persons upsert (second call merges, never duplicates).
        let id1 = repo.upsert_person("Srđan Jovanović", None).await.unwrap();
        let id2 = repo
            .upsert_person("Srđan Jovanović", Some("VP People @ HTEC, lični mentor"))
            .await
            .unwrap();
        assert_eq!(id1, id2, "person upsert must merge by name");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM persons")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let note: Option<String> = sqlx::query_scalar("SELECT note FROM persons WHERE id = ?")
            .bind(&id1)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(note.as_deref(), Some("VP People @ HTEC, lični mentor"));

        // relationship → 029 relationships, FK to the person row.
        let rel = repo
            .add_relationship(&id1, "mentor", Some("monthly calls"))
            .await
            .unwrap();
        let rel_person: String =
            sqlx::query_scalar("SELECT person_id FROM relationships WHERE id = ?")
                .bind(&rel)
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(rel_person, id1);

        // preference → 029 preferences.
        repo.add_preference("coding.style", "small verified increments")
            .await
            .unwrap();
        let pv: String =
            sqlx::query_scalar("SELECT pref_value FROM preferences WHERE pref_key = ?")
                .bind("coding.style")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(pv, "small verified increments");

        // a net-new note can LINK to the 029 person row.
        let mut linked = PersonalNoteRow::new("memory", "coffee with Srđan, talked hiring");
        linked.person_id = Some(id1.clone());
        repo.insert(&linked).await.unwrap();
        let for_person = repo.list_for_person(&id1).await.unwrap();
        assert_eq!(for_person.len(), 1);
        assert_eq!(for_person[0].person_id.as_deref(), Some(id1.as_str()));
    }
}
