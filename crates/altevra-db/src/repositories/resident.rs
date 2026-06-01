//! Resident mode registry + resident_run history (P0.5, migration 027).
//!
//! Modes are the small single-purpose agents (MOD-2). Runs are recorded into the
//! existing `brain_jobs` table (R10: one history table) via the additive
//! resident_* columns, so `altevra brain jobs` still works unchanged.

use altevra_core::resident::{ResidentMode, ResidentRunStatus};
use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

pub struct ResidentRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ResidentRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// All registered modes (optionally only the enabled ones).
    pub async fn list_modes(&self, only_enabled: bool) -> anyhow::Result<Vec<ResidentMode>> {
        let sql = if only_enabled {
            "SELECT name, description, model_role, sensitivity_ceiling, personal_data_allowed, enabled \
             FROM resident_modes WHERE enabled = 1 ORDER BY name"
        } else {
            "SELECT name, description, model_role, sensitivity_ceiling, personal_data_allowed, enabled \
             FROM resident_modes ORDER BY name"
        };
        let rows = sqlx::query(sql).fetch_all(self.pool).await?;
        Ok(rows.into_iter().map(row_to_mode).collect())
    }

    pub async fn get_mode(&self, name: &str) -> anyhow::Result<Option<ResidentMode>> {
        let row = sqlx::query(
            "SELECT name, description, model_role, sensitivity_ceiling, personal_data_allowed, enabled \
             FROM resident_modes WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_mode))
    }

    pub async fn upsert_mode(&self, m: &ResidentMode) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO resident_modes \
             (name, description, model_role, sensitivity_ceiling, personal_data_allowed, enabled) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&m.name)
        .bind(&m.description)
        .bind(&m.model_role)
        .bind(m.sensitivity_ceiling.to_string())
        .bind(i64::from(m.personal_data_allowed))
        .bind(i64::from(m.enabled))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Record a resident run into brain_jobs. `status` maps to the legacy
    /// running|done|failed column (Completed→done, else→failed) so existing
    /// tooling still works; the precise resident status + outputs live in the
    /// resident_* columns. Returns the run id.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_run(
        &self,
        mode: &str,
        model_role: &str,
        provider: &str,
        status: ResidentRunStatus,
        dry_run: bool,
        output_json: &str,
        proposals_emitted: i64,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();
        let legacy = if status == ResidentRunStatus::Completed {
            "done"
        } else {
            "failed"
        };
        let now = crate::util::ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO brain_jobs \
             (id, kind, status, started_at, finished_at, result_summary, \
              resident_mode, model_role, provider, output_json, proposals_emitted, dry_run) \
             VALUES (?, 'resident_run', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(legacy)
        .bind(&now)
        .bind(&now)
        .bind(format!(
            "resident:{mode} {} proposals={proposals_emitted}",
            status.as_str()
        ))
        .bind(mode)
        .bind(model_role)
        .bind(provider)
        .bind(output_json)
        .bind(proposals_emitted)
        .bind(i64::from(dry_run))
        .execute(self.pool)
        .await?;
        Ok(id)
    }
}

fn row_to_mode(r: sqlx::sqlite::SqliteRow) -> ResidentMode {
    let sens: String = r.get("sensitivity_ceiling");
    ResidentMode {
        name: r.get("name"),
        description: r.get("description"),
        model_role: r.get("model_role"),
        sensitivity_ceiling: sens
            .parse()
            .unwrap_or(altevra_core::security::Sensitivity::Internal),
        personal_data_allowed: r.get::<i64, _>("personal_data_allowed") != 0,
        enabled: r.get::<i64, _>("enabled") != 0,
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
    async fn seeded_modes_present_and_si7() {
        let p = pool().await;
        let repo = ResidentRepository::new(&p);
        let modes = repo.list_modes(false).await.unwrap();
        assert_eq!(modes.len(), 8, "8 builtin modes seeded");
        // personal_curator must be local_private (SI-7) and the contract holds.
        let pc = repo.get_mode("personal_curator").await.unwrap().unwrap();
        assert!(pc.personal_data_allowed);
        assert_eq!(pc.model_role, "local_private");
        assert!(pc.validate_role_ceiling().is_ok());
        // every seeded mode satisfies the SI-7 contract.
        for m in &modes {
            assert!(
                m.validate_role_ceiling().is_ok(),
                "{} violates SI-7",
                m.name
            );
        }
    }

    #[tokio::test]
    async fn record_run_lands_in_brain_jobs() {
        let p = pool().await;
        let repo = ResidentRepository::new(&p);
        let id = repo
            .record_run(
                "memory_curator",
                "cheap_worker",
                "noop",
                ResidentRunStatus::Completed,
                true,
                r#"{"proposals":[]}"#,
                0,
            )
            .await
            .unwrap();
        let row =
            sqlx::query("SELECT kind, status, resident_mode, dry_run FROM brain_jobs WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(row.get::<String, _>("kind"), "resident_run");
        assert_eq!(row.get::<String, _>("status"), "done");
        assert_eq!(row.get::<String, _>("resident_mode"), "memory_curator");
        assert_eq!(row.get::<i64, _>("dry_run"), 1);
    }
}
