//! Capability registry repositories (BUILD_TASKS T2.x, §5).
//!
//! Enforces the two load-bearing §5 guarantees at the persistence boundary:
//!  - **T7 honesty:** a `capability_record` may only be `supported` WITH an
//!    `evidence_ref`; without one it is rejected (no unproven native surface).
//!  - **dedup (T12):** a `skill_proposal` is keyed by `dedup_hash` — the same
//!    workflow proposes once; repeats increment `occurrences`, never duplicate.

use altevra_core::capability::TrustLevel;
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;
use chrono::Utc;

/// Honest can/cannot/unverified ledger entry.
#[derive(Debug, Clone)]
pub struct CapabilityRecordRow {
    pub id: String,
    pub actor: String,
    pub capability_key: String,
    pub support: String, // supported|unsupported|unverified|fallback
    pub evidence_ref: Option<String>,
    pub verification_method: Option<String>,
}

pub struct CapabilityRecordsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> CapabilityRecordsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert by (actor, capability_key). **T7:** rejects `supported` without an
    /// `evidence_ref` — honesty is enforced here, not trusted from the caller.
    pub async fn upsert(&self, row: &CapabilityRecordRow) -> anyhow::Result<()> {
        if row.support == "supported" && row.evidence_ref.as_deref().unwrap_or("").is_empty() {
            anyhow::bail!(
                "capability honesty (T7): '{}' cannot be 'supported' without an evidence_ref",
                row.capability_key
            );
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO capability_records \
             (id, actor, capability_key, support, evidence_ref, verification_method, verified_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(actor, capability_key) DO UPDATE SET \
               support=excluded.support, evidence_ref=excluded.evidence_ref, \
               verification_method=excluded.verification_method, verified_at=excluded.verified_at, \
               updated_at=excluded.updated_at",
        )
        .bind(&row.id)
        .bind(&row.actor)
        .bind(&row.capability_key)
        .bind(&row.support)
        .bind(row.evidence_ref.as_deref())
        .bind(row.verification_method.as_deref())
        .bind(if row.support == "supported" { Some(now.clone()) } else { None })
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, actor: &str, key: &str) -> anyhow::Result<Option<CapabilityRecordRow>> {
        let row = sqlx::query(
            "SELECT id, actor, capability_key, support, evidence_ref, verification_method \
             FROM capability_records WHERE actor = ? AND capability_key = ?",
        )
        .bind(actor)
        .bind(key)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|r| CapabilityRecordRow {
            id: r.get("id"),
            actor: r.get("actor"),
            capability_key: r.get("capability_key"),
            support: r.get("support"),
            evidence_ref: r.get("evidence_ref"),
            verification_method: r.get("verification_method"),
        }))
    }

    /// List records, optionally filtered by actor.
    pub async fn list(&self, actor: Option<&str>) -> anyhow::Result<Vec<CapabilityRecordRow>> {
        let rows = match actor {
            Some(a) => sqlx::query(
                "SELECT id, actor, capability_key, support, evidence_ref, verification_method \
                 FROM capability_records WHERE actor = ? ORDER BY capability_key",
            )
            .bind(a)
            .fetch_all(self.pool)
            .await?,
            None => sqlx::query(
                "SELECT id, actor, capability_key, support, evidence_ref, verification_method \
                 FROM capability_records ORDER BY actor, capability_key",
            )
            .fetch_all(self.pool)
            .await?,
        };
        Ok(rows
            .into_iter()
            .map(|r| CapabilityRecordRow {
                id: r.get("id"),
                actor: r.get("actor"),
                capability_key: r.get("capability_key"),
                support: r.get("support"),
                evidence_ref: r.get("evidence_ref"),
                verification_method: r.get("verification_method"),
            })
            .collect())
    }
}

pub struct SkillProposalsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SkillProposalsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Propose a skill. **Dedup (T12):** if `dedup_hash` exists, increments
    /// `occurrences` and returns the existing id; never creates a second row.
    /// Returns `(id, is_new)`.
    pub async fn propose(
        &self,
        id: &str,
        dedup_hash: &str,
        proposed_slug: &str,
        proposed_body_json: &str,
    ) -> anyhow::Result<(String, bool)> {
        let existing = sqlx::query("SELECT id FROM skill_proposals WHERE dedup_hash = ?")
            .bind(dedup_hash)
            .fetch_optional(self.pool)
            .await?;
        if let Some(r) = existing {
            let existing_id: String = r.get("id");
            sqlx::query(
                "UPDATE skill_proposals SET occurrences = occurrences + 1, \
                 updated_at = ? WHERE dedup_hash = ?",
            )
            .bind(ts_to_text(&Utc::now()))
            .bind(dedup_hash)
            .execute(self.pool)
            .await?;
            return Ok((existing_id, false));
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO skill_proposals \
             (id, dedup_hash, proposed_slug, proposed_body, occurrences, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 1, 'proposed', ?, ?)",
        )
        .bind(id)
        .bind(dedup_hash)
        .bind(proposed_slug)
        .bind(proposed_body_json)
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok((id.to_string(), true))
    }

    pub async fn occurrences(&self, dedup_hash: &str) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT occurrences FROM skill_proposals WHERE dedup_hash = ?")
            .bind(dedup_hash)
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(|r| r.get::<i64, _>("occurrences")).unwrap_or(0))
    }
}

/// A cross-agent capability grant (migration 023, Constitution §10, T9). One
/// row per `(grantee, subject_kind, subject_ref)`; a grant never auto-elevates
/// — `install`/`execute` POWER is presence-gated even in aggressive mode.
#[derive(Debug, Clone)]
pub struct CapabilityGrantRow {
    pub id: String,
    pub grantee: String,
    pub subject_kind: String, // skill|capability
    pub subject_ref: String,  // slug|capability_key
    pub trust_level: String,  // none|read|propose|render|install|execute
    pub requires_approval: bool,
    pub approval_ref: Option<String>, // review_item that approved (when required)
    pub status: String,               // pending|granted|revoked
    pub granted_at: Option<String>,
}

pub struct CapabilityGrantsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> CapabilityGrantsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a pending grant for `(grantee, subject_kind, subject_ref)`. The
    /// `requires_approval` flag is **derived by core** from the trust level
    /// ([`TrustLevel::requires_review`]) — never trusted from the caller — so a
    /// grant cannot claim it needs no review when its power says otherwise.
    /// Unique on the triple: re-creating the same target updates trust/flag and
    /// resets it to `pending`, never a 2nd row.
    pub async fn create_pending(
        &self,
        id: &str,
        grantee: &str,
        subject_kind: &str,
        subject_ref: &str,
        trust_level: TrustLevel,
    ) -> anyhow::Result<()> {
        // requires_approval is re-derived, not asserted (mirrors SI-9 tier re-derive).
        let requires_approval = trust_level.requires_review();
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO capability_grants \
             (id, grantee, subject_kind, subject_ref, trust_level, requires_approval, \
              approval_ref, status, granted_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, 'pending', NULL, ?, ?) \
             ON CONFLICT(grantee, subject_kind, subject_ref) DO UPDATE SET \
               trust_level=excluded.trust_level, requires_approval=excluded.requires_approval, \
               approval_ref=NULL, status='pending', granted_at=NULL, updated_at=excluded.updated_at",
        )
        .bind(id)
        .bind(grantee)
        .bind(subject_kind)
        .bind(subject_ref)
        .bind(trust_level.to_string())
        .bind(requires_approval as i64)
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Move a grant to `granted`, stamping `approval_ref` + `granted_at`.
    ///
    /// **Presence-gate invariant (T9, Constitution §10):** a grant whose
    /// `trust_level` [`requires_review`](TrustLevel::requires_review) (install /
    /// execute) can NOT reach `granted` without a non-empty `approval_ref` — and
    /// in this system that ref is only minted after a human-presence review.
    /// Granting POWER to another agent is presence-gated even in aggressive mode.
    /// A read/propose/render grant auto-grants (an empty `approval_ref` is fine).
    /// Enforced HERE against the row's persisted trust level, not the caller's word.
    pub async fn approve(&self, id: &str, approval_ref: Option<&str>) -> anyhow::Result<()> {
        let row = sqlx::query("SELECT trust_level FROM capability_grants WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("capability_grant '{id}' not found"))?;
        let trust: TrustLevel = row.get::<String, _>("trust_level").parse().expect("infallible");

        let has_ref = !approval_ref.unwrap_or("").trim().is_empty();
        if trust.requires_review() && !has_ref {
            anyhow::bail!(
                "grant power gate (T9): a '{trust}' grant cannot be granted without a \
                 non-empty approval_ref (human-presence review required, even in aggressive mode)"
            );
        }

        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "UPDATE capability_grants SET status = 'granted', approval_ref = ?, \
             granted_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(approval_ref)
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Revoke a grant (terminal). Idempotent on an already-revoked row.
    pub async fn revoke(&self, id: &str) -> anyhow::Result<()> {
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "UPDATE capability_grants SET status = 'revoked', updated_at = ? WHERE id = ?",
        )
        .bind(&now)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<CapabilityGrantRow>> {
        let row = sqlx::query(
            "SELECT id, grantee, subject_kind, subject_ref, trust_level, requires_approval, \
             approval_ref, status, granted_at FROM capability_grants WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_grant))
    }

    /// List grants, optionally filtered by grantee and/or status. Newest first.
    pub async fn list(
        &self,
        grantee: Option<&str>,
        status: Option<&str>,
    ) -> anyhow::Result<Vec<CapabilityGrantRow>> {
        let rows = match (grantee, status) {
            (Some(g), Some(s)) => sqlx::query(
                "SELECT id, grantee, subject_kind, subject_ref, trust_level, requires_approval, \
                 approval_ref, status, granted_at FROM capability_grants \
                 WHERE grantee = ? AND status = ? ORDER BY created_at DESC",
            )
            .bind(g)
            .bind(s)
            .fetch_all(self.pool)
            .await?,
            (Some(g), None) => sqlx::query(
                "SELECT id, grantee, subject_kind, subject_ref, trust_level, requires_approval, \
                 approval_ref, status, granted_at FROM capability_grants \
                 WHERE grantee = ? ORDER BY created_at DESC",
            )
            .bind(g)
            .fetch_all(self.pool)
            .await?,
            (None, Some(s)) => sqlx::query(
                "SELECT id, grantee, subject_kind, subject_ref, trust_level, requires_approval, \
                 approval_ref, status, granted_at FROM capability_grants \
                 WHERE status = ? ORDER BY created_at DESC",
            )
            .bind(s)
            .fetch_all(self.pool)
            .await?,
            (None, None) => sqlx::query(
                "SELECT id, grantee, subject_kind, subject_ref, trust_level, requires_approval, \
                 approval_ref, status, granted_at FROM capability_grants ORDER BY created_at DESC",
            )
            .fetch_all(self.pool)
            .await?,
        };
        Ok(rows.into_iter().map(row_to_grant).collect())
    }
}

/// Per-AI-agent capability matrix row (migration 023 `adapter_dossiers`).
/// Distinct from invocable tools (036 `tool_records`); `tool_records.adapter_ref`
/// links by `tool_name` for entities living in both worlds (hermes/codex/cursor).
#[derive(Debug, Clone)]
pub struct AdapterDossierRow {
    pub id: String,
    pub tool_name: String, // claude-code|codex|cursor|antigravity|hermes
    pub adapter_version: String,
    pub support_tier: String, // native|partial|fallback_only|unsupported|unverified
    /// JSON — per-surface support; the capability-YAML seed stores
    /// `{"can": [...], "cannot": [...]}` here.
    pub surfaces: serde_json::Value,
    pub hook_events_supported: serde_json::Value,
    pub skill_format: Option<String>,
    pub detection: Option<String>,
}

pub struct AdapterDossiersRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> AdapterDossiersRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert by `tool_name`. All fields are guarded (same mandatory rule as
    /// `tool_records` §P1.3 — the source YAMLs can embed credentials); sightings
    /// are recorded under `adapter_dossier:{tool_name}`.
    pub async fn upsert(&self, row: &AdapterDossierRow) -> anyhow::Result<usize> {
        use super::tool_records::{guard_opt, guard_value, record_sightings};

        let mut sightings = Vec::new();
        let g_version = {
            let g = altevra_secrets::guard_text(
                &row.adapter_version,
                altevra_core::security::Sensitivity::Internal,
            );
            sightings.extend(g.sightings);
            g.value
        };
        let g_skill_format = guard_opt(&row.skill_format, &mut sightings);
        let g_detection = guard_opt(&row.detection, &mut sightings);
        let (g_surfaces, s) = guard_value(&row.surfaces);
        sightings.extend(s);
        let (g_hooks, s) = guard_value(&row.hook_events_supported);
        sightings.extend(s);

        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO adapter_dossiers \
             (id, tool_name, adapter_version, support_tier, surfaces, \
              hook_events_supported, skill_format, detection, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(tool_name) DO UPDATE SET \
               adapter_version=excluded.adapter_version, support_tier=excluded.support_tier, \
               surfaces=excluded.surfaces, \
               hook_events_supported=excluded.hook_events_supported, \
               skill_format=excluded.skill_format, detection=excluded.detection, \
               updated_at=excluded.updated_at",
        )
        .bind(&row.id)
        .bind(&row.tool_name)
        .bind(&g_version)
        .bind(&row.support_tier)
        .bind(g_surfaces.to_string())
        .bind(g_hooks.to_string())
        .bind(g_skill_format.as_deref())
        .bind(g_detection.as_deref())
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;

        let source_ref = format!("adapter_dossier:{}", row.tool_name);
        record_sightings(self.pool, &sightings, &source_ref, "adapter_dossier_fields").await?;
        Ok(sightings.len())
    }

    pub async fn get(&self, tool_name: &str) -> anyhow::Result<Option<AdapterDossierRow>> {
        let row = sqlx::query(
            "SELECT id, tool_name, adapter_version, support_tier, surfaces, \
             hook_events_supported, skill_format, detection \
             FROM adapter_dossiers WHERE tool_name = ?",
        )
        .bind(tool_name)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_dossier))
    }

    pub async fn list(&self) -> anyhow::Result<Vec<AdapterDossierRow>> {
        let rows = sqlx::query(
            "SELECT id, tool_name, adapter_version, support_tier, surfaces, \
             hook_events_supported, skill_format, detection \
             FROM adapter_dossiers ORDER BY tool_name",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_dossier).collect())
    }
}

fn row_to_dossier(r: sqlx::sqlite::SqliteRow) -> AdapterDossierRow {
    AdapterDossierRow {
        id: r.get("id"),
        tool_name: r.get("tool_name"),
        adapter_version: r.get("adapter_version"),
        support_tier: r.get("support_tier"),
        surfaces: serde_json::from_str(&r.get::<String, _>("surfaces"))
            .unwrap_or(serde_json::json!({})),
        hook_events_supported: serde_json::from_str(
            &r.get::<String, _>("hook_events_supported"),
        )
        .unwrap_or(serde_json::json!([])),
        skill_format: r.get("skill_format"),
        detection: r.get("detection"),
    }
}

fn row_to_grant(r: sqlx::sqlite::SqliteRow) -> CapabilityGrantRow {
    CapabilityGrantRow {
        id: r.get("id"),
        grantee: r.get("grantee"),
        subject_kind: r.get("subject_kind"),
        subject_ref: r.get("subject_ref"),
        trust_level: r.get("trust_level"),
        requires_approval: r.get::<i64, _>("requires_approval") != 0,
        approval_ref: r.get("approval_ref"),
        status: r.get("status"),
        granted_at: r.get("granted_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn supported_without_evidence_is_rejected() {
        let p = pool().await;
        let repo = CapabilityRecordsRepository::new(&p);
        let bad = CapabilityRecordRow {
            id: "c1".into(),
            actor: "claude-code".into(),
            capability_key: "hook.session_start".into(),
            support: "supported".into(),
            evidence_ref: None,
            verification_method: Some("declared".into()),
        };
        assert!(
            repo.upsert(&bad).await.is_err(),
            "T7: supported needs evidence"
        );

        let good = CapabilityRecordRow {
            evidence_ref: Some("verify_run:abc".into()),
            ..bad
        };
        assert!(repo.upsert(&good).await.is_ok());
        let got = repo
            .get("claude-code", "hook.session_start")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.support, "supported");
        assert_eq!(got.evidence_ref.as_deref(), Some("verify_run:abc"));
    }

    #[tokio::test]
    async fn skill_proposal_dedups_by_hash() {
        let p = pool().await;
        let repo = SkillProposalsRepository::new(&p);
        let (id1, new1) = repo
            .propose("sp1", "hash-xyz", "do-thing", "{}")
            .await
            .unwrap();
        assert!(new1);
        let (id2, new2) = repo
            .propose("sp2", "hash-xyz", "do-thing", "{}")
            .await
            .unwrap();
        assert!(!new2, "same workflow must not create a 2nd proposal");
        assert_eq!(id1, id2);
        assert_eq!(repo.occurrences("hash-xyz").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn install_grade_grant_requires_approval_ref() {
        let p = pool().await;
        let repo = CapabilityGrantsRepository::new(&p);

        // An install-grade create_pending stays pending and is flagged as
        // requiring approval (derived by core from the trust level, not asserted).
        repo.create_pending("g-install", "hermes", "skill", "deploy-thing", TrustLevel::Install)
            .await
            .unwrap();
        let g = repo.get("g-install").await.unwrap().unwrap();
        assert_eq!(g.status, "pending");
        assert!(g.requires_approval, "install grade is review-gated");
        assert!(g.approval_ref.is_none());

        // T9: approve without an approval_ref errors — granting POWER is
        // presence-gated (the ref is minted only after a human-presence review).
        assert!(
            repo.approve("g-install", None).await.is_err(),
            "install grant must not be granted without an approval_ref"
        );
        // An empty/whitespace ref does not count either.
        assert!(repo.approve("g-install", Some("   ")).await.is_err());
        // Still pending after the rejected attempts.
        assert_eq!(repo.get("g-install").await.unwrap().unwrap().status, "pending");

        // approve WITH a (review-minted) approval_ref succeeds and stamps it.
        repo.approve("g-install", Some("review_item:42"))
            .await
            .unwrap();
        let granted = repo.get("g-install").await.unwrap().unwrap();
        assert_eq!(granted.status, "granted");
        assert_eq!(granted.approval_ref.as_deref(), Some("review_item:42"));
        assert!(granted.granted_at.is_some());

        // A read-grade grant auto-grants: no approval_ref needed.
        repo.create_pending("g-read", "codex", "capability", "search.memory", TrustLevel::Read)
            .await
            .unwrap();
        let r = repo.get("g-read").await.unwrap().unwrap();
        assert!(!r.requires_approval, "read grade is not review-gated");
        repo.approve("g-read", None).await.unwrap();
        assert_eq!(repo.get("g-read").await.unwrap().unwrap().status, "granted");

        // revoke is terminal; list filters by grantee/status.
        repo.revoke("g-read").await.unwrap();
        assert_eq!(repo.get("g-read").await.unwrap().unwrap().status, "revoked");
        assert_eq!(repo.list(Some("hermes"), None).await.unwrap().len(), 1);
        assert_eq!(repo.list(None, Some("granted")).await.unwrap().len(), 1);
    }
}
