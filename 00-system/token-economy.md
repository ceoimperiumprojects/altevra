<!-- source: ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md §8 -->
<!-- adopted: 2026-05-28 -->

# Altevra Token Economy Rules

Altevra should make cheap models useful by giving them clean, small, structured context.

Rules:

1. Always start with summaries, not full documents.
2. Hydrate full content only when needed.
3. Never load more than 5 full chunks at once.
4. Prefer source-of-truth files.
5. Ignore archived/deprecated content unless asked.
6. Compress repeated context.
7. Use IDs and summaries in first pass.
8. Use full text only in second pass.
9. Save outputs as structured data, not verbose prose.
10. If context is too large, ask Altevra to rerank.
11. Do not send raw internet scrape directly to strong reasoner.
12. Use cheap model for cleanup, strong model for synthesis.
13. Avoid generic insights.
14. Avoid long prose when structured JSON is enough.
