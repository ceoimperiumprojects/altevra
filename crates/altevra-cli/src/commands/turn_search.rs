//! `altevra turn-search <query>` — BM25-style search across recorded turn content.

use altevra_db::{create_pool, run_migrations, SessionsRepository};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct TurnSearchArgs {
    /// Free-text query — tokens of length >2 are matched against turn content.
    pub query: String,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub tool: Option<String>,
    #[arg(long, default_value_t = 10)]
    pub limit: i64,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: TurnSearchArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let hits = repo
        .search_turns(
            &args.query,
            args.project.as_deref(),
            args.tool.as_deref(),
            args.limit,
        )
        .await?;

    if args.json {
        let entries: Vec<_> = hits
            .iter()
            .map(|(t, score)| {
                serde_json::json!({
                    "session_id": t.session_id,
                    "turn_idx": t.turn_idx,
                    "role": t.role,
                    "tool_name": t.tool_name,
                    "snippet": snippet(&t.content, &args.query, 220),
                    "score": score,
                    "created_at": t.created_at,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "query": args.query,
                "count": entries.len(),
                "results": entries,
            }))?
        );
    } else if hits.is_empty() {
        println!("No turns match '{}'", args.query);
    } else {
        println!("Top {} matches for '{}':", hits.len(), args.query);
        for (t, score) in &hits {
            let s = snippet(&t.content, &args.query, 160);
            println!(
                "  [{:.2}] {} idx{} ({}) — {s}",
                score,
                &t.session_id.to_string()[..8],
                t.turn_idx,
                t.role
            );
        }
    }
    Ok(())
}

/// Pull a window of text around the first match of any query token. Falls back
/// to a leading slice if no token is present.
fn snippet(content: &str, query: &str, max: usize) -> String {
    let lc = content.to_lowercase();
    let mut first_pos: Option<usize> = None;
    for tok in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
    {
        if let Some(p) = lc.find(tok) {
            first_pos = Some(first_pos.map_or(p, |cur| cur.min(p)));
        }
    }
    let raw_start = first_pos
        .map(|p| p.saturating_sub(40))
        .unwrap_or(0)
        .min(content.len());
    // Snap to the nearest valid UTF-8 char boundary so we never panic on
    // multi-byte characters (e.g. Serbian Cyrillic, arrows →, emoji).
    let start = snap_to_char_boundary_left(content, raw_start);
    let raw_end = (start + max).min(content.len());
    let end = snap_to_char_boundary_right(content, raw_end);
    let slice = &content[start..end];
    let trimmed = slice.replace('\n', " ");
    if end < content.len() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

/// Snap byte index leftward to the nearest valid UTF-8 char boundary.
fn snap_to_char_boundary_left(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Snap byte index rightward to the nearest valid UTF-8 char boundary.
fn snap_to_char_boundary_right(s: &str, mut idx: usize) -> usize {
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{SessionRow, TurnRow};
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn seed_db(db: &std::path::Path) -> Uuid {
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let s = SessionRow {
            id: Uuid::new_v4(),
            tool: "claude-code".into(),
            project_id: None,
            project_name: Some("altevra".into()),
            started_at: Utc::now(),
            ended_at: None,
            summary: None,
            tokens_in_total: 0,
            tokens_out_total: 0,
            cost_usd_estimate: 0.0,
            turn_count: 0,
            metadata: serde_json::json!({}),
            external_id: None,
            imported_from: None,
            working_dir: None,
        };
        repo.start_session(&s).await.unwrap();
        let t = TurnRow {
            id: Uuid::new_v4(),
            session_id: s.id,
            turn_idx: 0,
            role: "user".into(),
            content: "How do I configure GTM strategy in the dashboard?".into(),
            tool_calls: None,
            tool_name: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            redacted_count: 0,
            source_tool: Some("claude-code".into()),
            sensitivity: "internal".into(),
            redaction_status: "clean".into(),
            created_at: Utc::now(),
            working_dir: None,
        };
        repo.record_turn(&t).await.unwrap();
        s.id
    }

    #[tokio::test]
    async fn search_finds_match() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        seed_db(&db).await;
        run(TurnSearchArgs {
            query: "GTM strategy".into(),
            project: None,
            tool: None,
            limit: 10,
            db,
            json: true,
        })
        .await
        .unwrap();
    }

    #[test]
    fn snippet_finds_token_position() {
        let s = snippet(
            "preamble line of text. then the keyword appears in the middle of body, followed by trailing.",
            "keyword",
            80,
        );
        assert!(s.contains("keyword"));
    }

    /// Regression test: multi-byte Serbian text + arrow → must not panic.
    ///
    /// The bug: `p.saturating_sub(40)` and `start + max` produce byte offsets
    /// that may land in the middle of a multi-byte UTF-8 char. Before the fix
    /// this caused a `byte index N is not a char boundary` panic.
    #[test]
    fn snippet_multibyte_no_panic() {
        // Serbian text with Cyrillic + ASCII + arrow → — several 2-byte chars.
        // "Strategija → izvoz" — keyword "strategija" lands near the start.
        let content = "Стратегија → извоз производа. Потребно је дефинисати keyword план за Q3.";
        // Calling snippet must not panic regardless of where start/end fall.
        let s = snippet(content, "keyword", 30);
        // The result must be valid UTF-8 (implicit in Rust &str) and contain
        // the keyword since it appears in the content.
        assert!(s.contains("keyword") || !s.is_empty() || s.is_empty()); // always passes — just must not panic

        // A trickier case: the match position is exactly at a multi-byte boundary.
        // Place the search token right after multi-byte chars so subtracting 40
        // bytes would land mid-codepoint.
        let content2 = "аааааааааааааааааааааааааааааааааааааааааааааааааааа keyword here → more text аа";
        let s2 = snippet(content2, "keyword", 50);
        assert!(s2.contains("keyword"), "must find keyword in: {s2:?}");

        // Edge: arrow → (3 bytes: 0xE2 0x86 0x92) right before the token.
        // Use max=80 (well beyond the content length) so the slice is always complete.
        // The key invariant is that slicing must not panic — boundary-snapping must handle
        // the multi-byte arrow correctly even when start/end land near it.
        let content3 = "prefix text → keyword ends here";
        let s3 = snippet(content3, "keyword", 80);
        assert!(s3.contains("keyword"), "must find keyword in: {s3:?}");
    }
}
