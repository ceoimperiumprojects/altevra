//! Wiki pages repository — indexed view of disk-resident wiki markdown.
//!
//! Pages are authored on disk under `wiki/<category>/<topic>.md`. This
//! repository keeps SQLite in sync so queries (`list`, `search`, link graph)
//! don't have to re-walk the filesystem on every call.
//!
//! Idempotent upsert: keyed on `topic` (UNIQUE). Existing rows update in
//! place, preserving `created_at`.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::repositories::objects::{ObjectIndexRepository, ObjectIndexRow};
use crate::util::ts_to_text;

#[derive(Debug, Clone)]
pub struct WikiPageRow {
    pub id: Uuid,
    pub topic: String,
    pub slug: String,
    pub path: String,
    pub status: String,
    pub confidence: String,
    pub sensitivity: String,
    pub source_count: i64,
    pub last_synthesized_at: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub checksum: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct WikiPageLinkRow {
    pub id: Uuid,
    pub from_page_id: Uuid,
    pub to_topic: String,
    pub link_type: String,
}

pub struct WikiPagesRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> WikiPagesRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert-or-update by topic. Returns the row id (UUID).
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        topic: &str,
        slug: &str,
        path: &str,
        status: &str,
        confidence: &str,
        sensitivity: &str,
        source_count: i64,
        last_synthesized_at: Option<DateTime<Utc>>,
        title: Option<&str>,
        checksum: &str,
    ) -> anyhow::Result<Uuid> {
        // Probe existing.
        let existing = sqlx::query("SELECT id FROM wiki_pages WHERE topic = ?")
            .bind(topic)
            .fetch_optional(self.pool)
            .await?;

        let id = match existing {
            Some(row) => {
                let id_text: String = row.get("id");
                let id = Uuid::parse_str(&id_text)?;
                let now = ts_to_text(&Utc::now());
                sqlx::query(
                    r#"UPDATE wiki_pages
                       SET slug = ?, path = ?, status = ?, confidence = ?,
                           sensitivity = ?, source_count = ?,
                           last_synthesized_at = ?, title = ?, checksum = ?,
                           updated_at = ?
                       WHERE id = ?"#,
                )
                .bind(slug)
                .bind(path)
                .bind(status)
                .bind(confidence)
                .bind(sensitivity)
                .bind(source_count)
                .bind(last_synthesized_at.map(|t| ts_to_text(&t)))
                .bind(title)
                .bind(checksum)
                .bind(now)
                .bind(id_text)
                .execute(self.pool)
                .await?;
                id
            }
            None => {
                let id = Uuid::new_v4();
                sqlx::query(
                    r#"INSERT INTO wiki_pages
                        (id, topic, slug, path, status, confidence, sensitivity,
                         source_count, last_synthesized_at, title, checksum)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(id.to_string())
                .bind(topic)
                .bind(slug)
                .bind(path)
                .bind(status)
                .bind(confidence)
                .bind(sensitivity)
                .bind(source_count)
                .bind(last_synthesized_at.map(|t| ts_to_text(&t)))
                .bind(title)
                .bind(checksum)
                .execute(self.pool)
                .await?;
                id
            }
        };
        Ok(id)
    }

    /// Upsert a wiki page AND route it into the retrieval substrate (T1.13): the
    /// page becomes a packet candidate (`object_index`) + full-text searchable
    /// (`object_fts`), the same single-maintenance-point contract the
    /// `LearningsRepository` already honors. The page body lives on disk, so the
    /// caller passes the (already-guarded) `body` text to index along with the
    /// `domain`, `categories`/`tags`, and the `redaction_status` verdict.
    ///
    /// Fail-closed: if `redaction_status` is not a scanned verdict
    /// (`clean`/`redacted`), the metadata row is still upserted but the page is
    /// NOT indexed — un-guarded text must never enter the index (R11 / TAG-1).
    /// The caller is responsible for having run `guard_text`/`ingest_guard`
    /// upstream and passing the verdict (caller-guards, no double-guard).
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_indexed(
        &self,
        topic: &str,
        slug: &str,
        path: &str,
        status: &str,
        confidence: &str,
        sensitivity: &str,
        source_count: i64,
        last_synthesized_at: Option<DateTime<Utc>>,
        title: Option<&str>,
        checksum: &str,
        domain: &str,
        categories: &str,
        tags: &str,
        body: &str,
        redaction_status: &str,
    ) -> anyhow::Result<Uuid> {
        let id = self
            .upsert(
                topic,
                slug,
                path,
                status,
                confidence,
                sensitivity,
                source_count,
                last_synthesized_at,
                title,
                checksum,
            )
            .await?;
        if matches!(redaction_status, "clean" | "redacted") {
            ObjectIndexRepository::new(self.pool)
                .index_object(
                    &ObjectIndexRow {
                        object_type: "wiki".into(),
                        id: id.to_string(),
                        status: status.into(),
                        sensitivity: sensitivity.into(),
                        domain: domain.into(),
                        scope: None,
                        title: title.map(|t| t.to_string()).or_else(|| Some(topic.into())),
                        categories: categories.into(),
                        tags: tags.into(),
                        redaction_status: redaction_status.into(),
                        updated_at: last_synthesized_at.unwrap_or_else(Utc::now),
                    },
                    body,
                )
                .await?;
        }
        Ok(id)
    }

    pub async fn find_by_topic(&self, topic: &str) -> anyhow::Result<Option<WikiPageRow>> {
        let row = sqlx::query("SELECT * FROM wiki_pages WHERE topic = ?")
            .bind(topic)
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(Self::map_row))
    }

    pub async fn list(&self) -> anyhow::Result<Vec<WikiPageRow>> {
        let rows = sqlx::query("SELECT * FROM wiki_pages ORDER BY topic ASC")
            .fetch_all(self.pool)
            .await?;
        Ok(rows.into_iter().map(Self::map_row).collect())
    }

    /// Substring search over topic + title. Body search lives on disk —
    /// callers walk the file via path when needed.
    pub async fn search(&self, query: &str, limit: i64) -> anyhow::Result<Vec<WikiPageRow>> {
        let like = format!("%{}%", query.to_lowercase());
        let rows = sqlx::query(
            r#"SELECT * FROM wiki_pages
               WHERE LOWER(topic) LIKE ? OR LOWER(COALESCE(title, '')) LIKE ?
               ORDER BY topic ASC LIMIT ?"#,
        )
        .bind(&like)
        .bind(&like)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(Self::map_row).collect())
    }

    pub async fn replace_links(
        &self,
        from_page_id: Uuid,
        to_topics: &[String],
    ) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM wiki_page_links WHERE from_page_id = ?")
            .bind(from_page_id.to_string())
            .execute(self.pool)
            .await?;
        for to_topic in to_topics {
            sqlx::query(
                r#"INSERT INTO wiki_page_links (id, from_page_id, to_topic, link_type)
                   VALUES (?, ?, ?, 'reference')"#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(from_page_id.to_string())
            .bind(to_topic)
            .execute(self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn related(&self, topic: &str) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            r#"SELECT DISTINCT l.to_topic
               FROM wiki_page_links l
               JOIN wiki_pages p ON p.id = l.from_page_id
               WHERE p.topic = ?
               ORDER BY l.to_topic ASC"#,
        )
        .bind(topic)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| r.get::<String, _>("to_topic"))
            .collect())
    }

    fn map_row(r: sqlx::sqlite::SqliteRow) -> WikiPageRow {
        WikiPageRow {
            id: Uuid::parse_str(&r.get::<String, _>("id")).unwrap_or_else(|_| Uuid::nil()),
            topic: r.get("topic"),
            slug: r.get("slug"),
            path: r.get("path"),
            status: r.get("status"),
            confidence: r.get("confidence"),
            sensitivity: r.get("sensitivity"),
            source_count: r.get("source_count"),
            last_synthesized_at: r
                .get::<Option<String>, _>("last_synthesized_at")
                .and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
            title: r.get("title"),
            checksum: r.get("checksum"),
            created_at: crate::util::ts_from_text(r.get::<String, _>("created_at")),
            updated_at: crate::util::ts_from_text(r.get::<String, _>("updated_at")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::pool::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn upsert_inserts_then_updates() {
        let pool = setup().await;
        let repo = WikiPagesRepository::new(&pool);
        let id1 = repo
            .upsert(
                "alpha",
                "alpha",
                "wiki/concepts/alpha.md",
                "living",
                "medium",
                "internal",
                3,
                None,
                Some("Alpha"),
                "sha-1",
            )
            .await
            .unwrap();
        let id2 = repo
            .upsert(
                "alpha",
                "alpha",
                "wiki/concepts/alpha.md",
                "living",
                "high",
                "internal",
                5,
                None,
                Some("Alpha"),
                "sha-2",
            )
            .await
            .unwrap();
        assert_eq!(id1, id2, "topic-keyed upsert must reuse the row id");
        let fetched = repo.find_by_topic("alpha").await.unwrap().unwrap();
        assert_eq!(fetched.confidence, "high");
        assert_eq!(fetched.source_count, 5);
        assert_eq!(fetched.checksum, "sha-2");
    }

    #[tokio::test]
    async fn search_matches_topic_or_title() {
        let pool = setup().await;
        let repo = WikiPagesRepository::new(&pool);
        repo.upsert(
            "rust-traits",
            "rust-traits",
            "/p/rust-traits.md",
            "living",
            "medium",
            "internal",
            1,
            None,
            Some("Rust Traits"),
            "x",
        )
        .await
        .unwrap();
        repo.upsert(
            "go-channels",
            "go-channels",
            "/p/go-channels.md",
            "living",
            "medium",
            "internal",
            1,
            None,
            Some("Go Channels"),
            "x",
        )
        .await
        .unwrap();
        let hits = repo.search("rust", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].topic, "rust-traits");
    }

    #[tokio::test]
    async fn replace_links_and_related_query() {
        let pool = setup().await;
        let repo = WikiPagesRepository::new(&pool);
        let id = repo
            .upsert(
                "from",
                "from",
                "/p/from.md",
                "living",
                "medium",
                "internal",
                0,
                None,
                None,
                "x",
            )
            .await
            .unwrap();
        repo.replace_links(id, &["topic-a".into(), "topic-b".into()])
            .await
            .unwrap();
        // Replace again with a different set to confirm DELETE+INSERT semantics.
        repo.replace_links(id, &["topic-c".into()]).await.unwrap();
        let related = repo.related("from").await.unwrap();
        assert_eq!(related, vec!["topic-c"]);
    }
}
