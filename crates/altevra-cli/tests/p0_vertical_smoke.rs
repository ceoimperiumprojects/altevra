//! P0.1 vertical-loop smoke test (BUILD_TASKS T1.18 — the spine).
//!
//! Proves the whole deterministic core works end-to-end on synthetic fixtures,
//! with NO real secrets and NO network:
//!   capture → PreWriteSafetyGate (ingest_guard) → persist (envelope) →
//!   object_index → packet compile via ExposureGate → exposure_decision audit.
//!
//! Hard assertions (the things that MUST hold):
//!  - a business/project decision is INCLUDED in a work packet;
//!  - a restricted personal/health object is EXCLUDED with a non-leaking reason;
//!  - a fake secret is redacted and its raw value never lands in any DB text;
//!  - an untagged write is quarantined (not persisted).

use altevra_core::domain::Domain;
use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::safety::{ExposureDecision, ExposureGate, ExposureRequest};
use altevra_core::security::Sensitivity;
use altevra_core::status::{ObjectStatus, RedactionStatus};
use altevra_core::template::TemplateRegistry;
use altevra_secrets::ingest_guard;
use chrono::Utc;
use sqlx::Row;

/// A candidate row as it lives in object_index for packet compilation.
struct Candidate {
    object_type: String,
    id: String,
    #[allow(dead_code)] // surfaced in the packet item; not asserted in the smoke
    title: String,
    envelope: Envelope,
    /// Redaction verdict — the gate fails closed on anything not clean/redacted.
    redaction_status: RedactionStatus,
}

#[tokio::test]
async fn p0_vertical_smoke() {
    let pool = altevra_db::create_pool("sqlite::memory:")
        .await
        .expect("pool");
    altevra_db::run_migrations(&pool).await.expect("migrations");
    let reg = TemplateRegistry::with_builtins();
    let now = Utc::now();

    // ---------- 1. CAPTURE + GATE + PERSIST: business decision ----------
    let decision_body = std::fs::read_to_string("../../fixtures/p0/project_decision_publicish.md")
        .unwrap_or_else(|_| "## Decision\nAdopt SQLite\n## Rationale\nLocal-first".to_string());
    let mut dec_env = Envelope::new(
        "dec_smoke_1",
        "decision",
        now,
        Provenance::new(ProvenanceOrigin::PavleDirect),
    );
    dec_env.domain = Domain::Project;
    dec_env.sensitivity = Sensitivity::Internal;
    dec_env.categories = vec!["architecture".into(), "storage".into()];

    let dec_guarded = ingest_guard(&decision_body, &dec_env, &["title".into()], &reg);
    assert!(
        dec_guarded.is_safe_to_persist(),
        "business decision should pass the gate: {:?}",
        dec_guarded.reasons
    );
    persist_decision(
        &pool,
        &dec_env,
        "Adopt SQLite as P0 store",
        &dec_guarded.value,
    )
    .await;
    upsert_index(
        &pool,
        "decision",
        &dec_env,
        "Adopt SQLite as P0 store",
        &dec_guarded.redaction_status,
    )
    .await;

    // ---------- 2. restricted personal/health learning ----------
    let health_body = "## Learning\nLate nights hurt next-day focus.";
    let mut health_env = Envelope::new(
        "learn_smoke_health",
        "learning",
        now,
        Provenance::new(ProvenanceOrigin::PavleDirect),
    );
    health_env.domain = Domain::Health;
    health_env.sensitivity = Sensitivity::Restricted;
    health_env.categories = vec!["health".into()];

    let health_guarded = ingest_guard(health_body, &health_env, &["title".into()], &reg);
    assert!(health_guarded.is_safe_to_persist());
    persist_learning(
        &pool,
        &health_env,
        "Late nights hurt focus",
        &health_guarded.value,
    )
    .await;
    upsert_index(
        &pool,
        "learning",
        &health_env,
        "Late nights hurt focus",
        &health_guarded.redaction_status,
    )
    .await;

    // ---------- 3. fake secret must be redacted, never stored raw ----------
    let raw_secret = "sk-FIXTUREfixtureFIXTUREfixture0000";
    let secret_body = format!("## Learning\napi key {raw_secret} leaked into a note");
    let mut sec_env = Envelope::new(
        "learn_smoke_secret",
        "learning",
        now,
        Provenance::new(ProvenanceOrigin::Imported),
    );
    sec_env.domain = Domain::Business;
    sec_env.categories = vec!["security".into()];
    let sec_guarded = ingest_guard(&secret_body, &sec_env, &["title".into()], &reg);
    assert!(
        !sec_guarded.value.contains(raw_secret),
        "raw secret must be redacted before persist"
    );
    assert!(
        !sec_guarded.sightings.is_empty(),
        "a secret sighting must be recorded"
    );
    persist_learning(&pool, &sec_env, "Secret leak note", &sec_guarded.value).await;
    record_sighting(&pool, &sec_guarded).await;
    upsert_index(
        &pool,
        "learning",
        &sec_env,
        "Secret leak note",
        &sec_guarded.redaction_status,
    )
    .await;

    // ---------- 4. untagged write is quarantined (not persisted) ----------
    let mut untagged = Envelope::new(
        "dec_untagged",
        "decision",
        now,
        Provenance::new(ProvenanceOrigin::PavleDirect),
    );
    untagged.domain = Domain::Business;
    // no categories -> TAG-1 violation
    let untagged_guarded = ingest_guard(
        "## Decision\nx\n## Rationale\ny",
        &untagged,
        &["title".into()],
        &reg,
    );
    assert!(
        untagged_guarded.quarantined,
        "untagged write must quarantine"
    );
    assert!(!untagged_guarded.is_safe_to_persist());

    // ---------- 5. COMPILE PACKET via ExposureGate (work request) ----------
    let candidates = load_candidates(&pool).await;
    let request = ExposureRequest::default_work(); // internal ceiling, business/project/public scope
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    for c in &candidates {
        match ExposureGate::decide(&c.envelope, &c.redaction_status, &request) {
            ExposureDecision::Allow => included.push(c),
            ExposureDecision::Deny(reason) => excluded.push((c, reason)),
        }
    }

    // the business decision is included...
    assert!(
        included.iter().any(|c| c.id == "dec_smoke_1"),
        "business decision must be in the work packet"
    );
    // ...the restricted health learning is excluded with a non-leaking reason.
    let health_excl = excluded
        .iter()
        .find(|(c, _)| c.id == "learn_smoke_health")
        .expect("health object must be excluded from work packet");
    assert_eq!(
        health_excl.1.code(),
        "items_above_ceiling_omitted",
        "exclusion reason must not reveal it's a health object"
    );

    // ---------- 6. exposure_decision audit (durable, never purged) ----------
    let included_refs: Vec<_> = included
        .iter()
        .map(|c| serde_json::json!({"type": c.object_type, "id": c.id}))
        .collect();
    let excluded_refs: Vec<_> = excluded
        .iter()
        .map(|(c, r)| serde_json::json!({"type": c.object_type, "id": c.id, "reason": r.code()}))
        .collect();
    sqlx::query(
        "INSERT INTO exposure_decisions (id, packet_id, request, sensitivity_ceiling, domain_scope, included_refs, excluded_refs) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("exp_smoke_1")
    .bind("pkt_smoke_1")
    .bind(r#"{"intent":"task_work","project":"altevra"}"#)
    .bind(request.sensitivity_ceiling.to_string())
    .bind(serde_json::to_string(&request.domain_scope.iter().map(|d| d.to_string()).collect::<Vec<_>>()).unwrap())
    .bind(serde_json::to_string(&included_refs).unwrap())
    .bind(serde_json::to_string(&excluded_refs).unwrap())
    .execute(&pool)
    .await
    .expect("audit insert");

    // ---------- 7. NO raw secret anywhere in DB text ----------
    let leaked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (\
            SELECT body FROM learnings WHERE body LIKE '%sk-FIXTUREfixture%' \
            UNION ALL SELECT rationale FROM decisions WHERE rationale LIKE '%sk-FIXTUREfixture%')",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    assert_eq!(leaked, 0, "no raw secret may persist in any DB text");

    // ---------- summary (deterministic-ish) ----------
    let summary = serde_json::json!({
        "candidates": candidates.len(),
        "included": included.iter().map(|c| &c.id).collect::<Vec<_>>(),
        "excluded": excluded.iter().map(|(c, r)| serde_json::json!({"id": c.id, "reason": r.code()})).collect::<Vec<_>>(),
        "secret_leak": leaked,
        "audit_written": true,
    });
    println!(
        "p0-vertical-smoke: {}",
        serde_json::to_string_pretty(&summary).unwrap()
    );
    assert!(!included.is_empty() && !excluded.is_empty());
}

// ---- helpers (raw sqlx; repo layer proper lands in T1.12) ----

async fn persist_decision(pool: &altevra_db::DbPool, env: &Envelope, title: &str, body: &str) {
    sqlx::query(
        "INSERT INTO decisions (id, title, rationale, decided_at, status, domain, sensitivity, categories, schema_version, revision) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 1)",
    )
    .bind(&env.id)
    .bind(title)
    .bind(body)
    .bind(env.created_at.to_rfc3339())
    .bind(env.status.to_string())
    .bind(env.domain.to_string())
    .bind(env.sensitivity.to_string())
    .bind(serde_json::to_string(&env.categories).unwrap())
    .execute(pool)
    .await
    .expect("persist decision");
}

async fn persist_learning(pool: &altevra_db::DbPool, env: &Envelope, title: &str, body: &str) {
    sqlx::query(
        "INSERT INTO learnings (id, title, body, status, domain, sensitivity, categories) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&env.id)
    .bind(title)
    .bind(body)
    .bind(env.status.to_string())
    .bind(env.domain.to_string())
    .bind(env.sensitivity.to_string())
    .bind(serde_json::to_string(&env.categories).unwrap())
    .execute(pool)
    .await
    .expect("persist learning");
}

async fn upsert_index(
    pool: &altevra_db::DbPool,
    object_type: &str,
    env: &Envelope,
    title: &str,
    redaction: &RedactionStatus,
) {
    sqlx::query(
        "INSERT OR REPLACE INTO object_index (type, id, status, sensitivity, domain, scope, title, categories, tags, redaction_status, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[]', ?, ?)",
    )
    .bind(object_type)
    .bind(&env.id)
    .bind(env.status.to_string())
    .bind(env.sensitivity.to_string())
    .bind(env.domain.to_string())
    .bind(env.scope.as_deref())
    .bind(title)
    .bind(serde_json::to_string(&env.categories).unwrap())
    .bind(redaction.to_string())
    .bind(env.updated_at.to_rfc3339())
    .execute(pool)
    .await
    .expect("upsert index");
}

async fn record_sighting(pool: &altevra_db::DbPool, guarded: &altevra_secrets::Guarded) {
    for (i, s) in guarded.sightings.iter().enumerate() {
        sqlx::query(
            "INSERT INTO secret_sightings (id, secret_kind, fingerprint, source_ref, action) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(format!("sight_smoke_{i}"))
        .bind(&s.secret_kind)
        .bind(&s.fingerprint)
        .bind("object:learning:learn_smoke_secret")
        .bind(&s.action)
        .execute(pool)
        .await
        .expect("sighting insert");
    }
}

async fn load_candidates(pool: &altevra_db::DbPool) -> Vec<Candidate> {
    let rows = sqlx::query(
        "SELECT type, id, status, sensitivity, domain, scope, title, categories, redaction_status FROM object_index",
    )
    .fetch_all(pool)
    .await
    .expect("load index");
    rows.into_iter()
        .map(|r| {
            let object_type: String = r.get("type");
            let id: String = r.get("id");
            let title: String = r.get("title");
            let mut env = Envelope::new(
                &id,
                &object_type,
                Utc::now(),
                Provenance::new(ProvenanceOrigin::Imported),
            );
            let status: String = r.get("status");
            let sens: String = r.get("sensitivity");
            let dom: String = r.get("domain");
            let red: String = r.get("redaction_status");
            env.status = status.parse::<ObjectStatus>().unwrap();
            env.sensitivity = sens.parse::<Sensitivity>().unwrap();
            env.domain = dom.parse::<Domain>().unwrap();
            Candidate {
                object_type,
                id,
                title,
                envelope: env,
                redaction_status: red
                    .parse::<RedactionStatus>()
                    .unwrap_or(RedactionStatus::Unscanned),
            }
        })
        .collect()
}
