//! Domain policy registry (P0.8 T8.3) — reads the seeded `domain_policies` (024)
//! so the policy is live, not a dormant SQL island. Includes the cloud-sync
//! ceiling selector (P0.9 T9.1): which domains may sync, resolved most-restrictive
//! across a multi-domain object (R3). Fail-closed: an unknown domain = no sync.

use sqlx::{Row, SqlitePool};

/// Per-domain cloud-sync ceiling (most-restrictive first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSync {
    /// Never leaves the machine (high-water domains: health/relationship/...).
    Disabled,
    /// May sync only if encrypted.
    EncryptedOnly,
    /// May sync (public/low-risk).
    Allowed,
}

impl CloudSync {
    pub fn parse(s: &str) -> Self {
        match s {
            "encrypted_only" => CloudSync::EncryptedOnly,
            "allowed" => CloudSync::Allowed,
            // anything else (incl. "disabled" / unknown) is fail-closed = Disabled.
            _ => CloudSync::Disabled,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            CloudSync::Disabled => "disabled",
            CloudSync::EncryptedOnly => "encrypted_only",
            CloudSync::Allowed => "allowed",
        }
    }
    fn rank(&self) -> u8 {
        match self {
            CloudSync::Disabled => 0,
            CloudSync::EncryptedOnly => 1,
            CloudSync::Allowed => 2,
        }
    }
    /// The more restrictive (lower) of two ceilings (R3 most-restrictive).
    pub fn most_restrictive(a: CloudSync, b: CloudSync) -> CloudSync {
        if a.rank() <= b.rank() {
            a
        } else {
            b
        }
    }
    /// Eligible for a sync set? `disabled` never syncs (P0.9: restricted domains excluded).
    pub fn eligible_for_sync(&self) -> bool {
        !matches!(self, CloudSync::Disabled)
    }
}

#[derive(Debug, Clone)]
pub struct DomainPolicyRow {
    pub domain_key: String,
    pub display_name: String,
    pub default_sensitivity: String,
    pub max_sensitivity: String,
    pub cloud_sync: CloudSync,
    pub embedding_model_role: String,
    pub obsidian_mirror: String,
    pub soft_ttl_days: Option<i64>,
    pub hard_expiry_days: Option<i64>,
    pub rtbf_required: bool,
    pub legal_hold_capable: bool,
}

pub struct DomainPolicyRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> DomainPolicyRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, domain_key: &str) -> anyhow::Result<Option<DomainPolicyRow>> {
        let row = sqlx::query(
            "SELECT domain_key, display_name, default_sensitivity, max_sensitivity, cloud_sync, \
             embedding_model_role, obsidian_mirror, soft_ttl_days, hard_expiry_days, rtbf_required, legal_hold_capable \
             FROM domain_policies WHERE domain_key = ?",
        )
        .bind(domain_key)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_policy))
    }

    pub async fn list(&self) -> anyhow::Result<Vec<DomainPolicyRow>> {
        let rows = sqlx::query(
            "SELECT domain_key, display_name, default_sensitivity, max_sensitivity, cloud_sync, \
             embedding_model_role, obsidian_mirror, soft_ttl_days, hard_expiry_days, rtbf_required, legal_hold_capable \
             FROM domain_policies ORDER BY domain_key",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_policy).collect())
    }

    /// Most-restrictive cloud-sync ceiling across an object's domains (R3). An
    /// unknown/absent domain is treated as Disabled (fail-closed). Empty → Disabled.
    pub async fn cloud_sync_for(&self, domains: &[String]) -> anyhow::Result<CloudSync> {
        if domains.is_empty() {
            return Ok(CloudSync::Disabled);
        }
        let mut acc = CloudSync::Allowed;
        for d in domains {
            let cs = match self.get(d).await? {
                Some(p) => p.cloud_sync,
                None => CloudSync::Disabled, // unknown domain → fail-closed
            };
            acc = CloudSync::most_restrictive(acc, cs);
        }
        Ok(acc)
    }

    /// P0.9 sync-set membership: true iff every domain permits sync (most-restrictive
    /// across domains is not `disabled`). Restricted domains are excluded.
    pub async fn sync_eligible(&self, domains: &[String]) -> anyhow::Result<bool> {
        Ok(self.cloud_sync_for(domains).await?.eligible_for_sync())
    }
}

fn row_to_policy(r: sqlx::sqlite::SqliteRow) -> DomainPolicyRow {
    let cs: String = r.get("cloud_sync");
    DomainPolicyRow {
        domain_key: r.get("domain_key"),
        display_name: r.get("display_name"),
        default_sensitivity: r.get("default_sensitivity"),
        max_sensitivity: r.get("max_sensitivity"),
        cloud_sync: CloudSync::parse(&cs),
        embedding_model_role: r.get("embedding_model_role"),
        obsidian_mirror: r.get("obsidian_mirror"),
        soft_ttl_days: r.get("soft_ttl_days"),
        hard_expiry_days: r.get("hard_expiry_days"),
        rtbf_required: r.get::<i64, _>("rtbf_required") != 0,
        legal_hold_capable: r.get::<i64, _>("legal_hold_capable") != 0,
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
    async fn nine_builtins_with_high_water_local_only() {
        let p = pool().await;
        let repo = DomainPolicyRepository::new(&p);
        assert_eq!(repo.list().await.unwrap().len(), 9);
        // high-water domains are local_private + cloud_sync disabled (D4).
        for d in [
            "health",
            "relationship",
            "personal",
            "client",
            "legal",
            "financial",
        ] {
            let pol = repo.get(d).await.unwrap().unwrap();
            assert_eq!(pol.embedding_model_role, "local_private", "{d}");
            assert_eq!(pol.cloud_sync, CloudSync::Disabled, "{d}");
            assert!(!pol.cloud_sync.eligible_for_sync(), "{d} must not sync");
        }
        // public syncs; business is encrypted-only.
        assert_eq!(
            repo.get("public").await.unwrap().unwrap().cloud_sync,
            CloudSync::Allowed
        );
        assert_eq!(
            repo.get("business").await.unwrap().unwrap().cloud_sync,
            CloudSync::EncryptedOnly
        );
    }

    #[tokio::test]
    async fn cloud_sync_set_excludes_restricted_and_is_most_restrictive() {
        let p = pool().await;
        let repo = DomainPolicyRepository::new(&p);
        // P0.9 T9.3: a multi-domain object touching health is NOT sync-eligible.
        assert!(!repo
            .sync_eligible(&["business".into(), "health".into()])
            .await
            .unwrap());
        // business alone is eligible (encrypted_only).
        assert!(repo.sync_eligible(&["business".into()]).await.unwrap());
        // unknown domain → fail-closed (not eligible).
        assert!(!repo.sync_eligible(&["mystery".into()]).await.unwrap());
        // most-restrictive: business(encrypted) + public(allowed) → encrypted_only.
        assert_eq!(
            repo.cloud_sync_for(&["business".into(), "public".into()])
                .await
                .unwrap(),
            CloudSync::EncryptedOnly
        );
    }
}
