//! Retrieval context — the "knowledge brief" we hand to an agent.
//!
//! This is the data shape produced by `altevra context build --query X` and
//! consumed by the prompt generator. It packages everything the agent needs to
//! think about a topic: relevant memory chunks, related decisions/learnings,
//! active tasks, recent updates, and applicable skills.
//!
//! The crate keeps only the types here — assembling a `RetrievalContext` is
//! done in `altevra-cli` (where it has access to vault, memory, and DB layers).

use serde::{Deserialize, Serialize};

use crate::updates::UpdateFeedItem;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalChunk {
    pub source_path: Option<String>,
    pub heading_path: Vec<String>,
    pub snippet: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalDecision {
    pub title: String,
    pub rationale: Option<String>,
    pub decided_at: Option<String>,
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalLearning {
    pub title: String,
    pub body: String,
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalTask {
    pub title: String,
    pub status: String,
    pub priority: Option<String>,
    pub due_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalSkill {
    pub slug: String,
    pub version: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrievalContext {
    pub query: String,
    pub project: Option<String>,
    pub chunks: Vec<RetrievalChunk>,
    pub decisions: Vec<RetrievalDecision>,
    pub learnings: Vec<RetrievalLearning>,
    pub tasks: Vec<RetrievalTask>,
    pub updates: Vec<UpdateFeedItem>,
    pub skills: Vec<RetrievalSkill>,
    /// Approximate token count (chars/4).
    pub token_estimate: usize,
}

impl RetrievalContext {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }

    /// Render this context as a dense Markdown "agent brief" — drops into a
    /// system prompt or is read by a human.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# Agent Brief — query: {}\n\n", self.query));
        if let Some(p) = &self.project {
            out.push_str(&format!("**Project:** {p}\n\n"));
        }

        if !self.tasks.is_empty() {
            out.push_str("## Active Tasks\n\n");
            for t in &self.tasks {
                let pri = t.priority.as_deref().unwrap_or("medium");
                let due = t
                    .due_at
                    .as_deref()
                    .map(|d| format!(" (due {d})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- **{}** [{}] [{}{}]\n",
                    t.title, t.status, pri, due
                ));
            }
            out.push('\n');
        }

        if !self.updates.is_empty() {
            out.push_str("## Recent Updates\n\n");
            for u in &self.updates {
                out.push_str(&format!(
                    "- [{}] **{}** — {}\n",
                    u.importance, u.title, u.short_summary
                ));
            }
            out.push('\n');
        }

        if !self.decisions.is_empty() {
            out.push_str("## Related Decisions\n\n");
            for d in &self.decisions {
                out.push_str(&format!("- **{}**", d.title));
                if let Some(r) = &d.rationale {
                    out.push_str(&format!(" — {r}"));
                }
                if let Some(src) = &d.source_path {
                    out.push_str(&format!(" _(see {src})_"));
                }
                out.push('\n');
            }
            out.push('\n');
        }

        if !self.learnings.is_empty() {
            out.push_str("## Learnings\n\n");
            for l in &self.learnings {
                out.push_str(&format!("- **{}** — {}\n", l.title, l.body));
            }
            out.push('\n');
        }

        if !self.skills.is_empty() {
            out.push_str("## Applicable Skills\n\n");
            for s in &self.skills {
                out.push_str(&format!("- `{}` v{} — {}\n", s.slug, s.version, s.title));
            }
            out.push('\n');
        }

        if !self.chunks.is_empty() {
            out.push_str("## Relevant Vault Excerpts\n\n");
            for (i, c) in self.chunks.iter().enumerate() {
                let src = c.source_path.as_deref().unwrap_or("(inline)");
                let heading = if c.heading_path.is_empty() {
                    String::new()
                } else {
                    format!(" › {}", c.heading_path.join(" › "))
                };
                out.push_str(&format!(
                    "### {} [{:.3}] {}{}\n\n{}\n\n",
                    i + 1,
                    c.score,
                    src,
                    heading,
                    c.snippet,
                ));
            }
        }

        if out.trim() == &format!("# Agent Brief — query: {}", self.query) {
            out.push_str("_No related context found in vault._\n");
        }

        out
    }

    pub fn recompute_token_estimate(&mut self) {
        self.token_estimate = self.to_markdown().len() / 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::{Importance, UpdateFeedItem};

    #[test]
    fn empty_context_renders_disclaimer() {
        let ctx = RetrievalContext::new("test query");
        let md = ctx.to_markdown();
        assert!(md.contains("query: test query"));
        assert!(md.contains("No related context"));
    }

    #[test]
    fn full_context_renders_sections() {
        let mut ctx = RetrievalContext::new("ship v0.2");
        ctx.project = Some("altevra".into());
        ctx.tasks.push(RetrievalTask {
            title: "Wire SQLite".into(),
            status: "open".into(),
            priority: Some("high".into()),
            due_at: None,
        });
        ctx.updates.push(UpdateFeedItem::from_event(
            uuid::Uuid::new_v4(),
            "skill_updated",
            Importance::High,
            "Skill altevra-core updated",
            "0.5.0 → 0.6.0",
        ));
        ctx.decisions.push(RetrievalDecision {
            title: "Use SQLite instead of Postgres".into(),
            rationale: Some("Zero setup, embedded".into()),
            decided_at: Some("2026-05-27".into()),
            source_path: Some("08-decisions/sqlite.md".into()),
        });
        ctx.skills.push(RetrievalSkill {
            slug: "altevra-core".into(),
            version: "0.6.0".into(),
            title: "Altevra Core".into(),
        });
        ctx.chunks.push(RetrievalChunk {
            source_path: Some("06-skills/altevra-core.md".into()),
            heading_path: vec!["Operations".into()],
            snippet: "Read the bootstrap packet at session start...".into(),
            score: 0.95,
        });
        let md = ctx.to_markdown();
        assert!(md.contains("Active Tasks"));
        assert!(md.contains("Wire SQLite"));
        assert!(md.contains("Recent Updates"));
        assert!(md.contains("Related Decisions"));
        assert!(md.contains("Applicable Skills"));
        assert!(md.contains("Relevant Vault Excerpts"));
        assert!(md.contains("Altevra Core"));
    }

    #[test]
    fn token_estimate_roughly_quarter_of_chars() {
        let mut ctx = RetrievalContext::new("x");
        ctx.recompute_token_estimate();
        let md = ctx.to_markdown();
        assert!(ctx.token_estimate >= md.len() / 5);
        assert!(ctx.token_estimate <= md.len());
    }
}
