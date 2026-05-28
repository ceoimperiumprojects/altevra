<!-- source: ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md §15 -->
<!-- adopted: 2026-05-28 -->

# Daily Capture Protocol

Altevra needs a daily input/output loop. If it does not run daily, it will not compound.

## 1. Daily Input — questions Altevra should ask (or Pavle answers)

Every day, capture:

1. What did Pavle work on today?
2. What did he learn?
3. What decision was made?
4. What changed?
5. What should happen tomorrow?
6. Was anything personally important?
7. What should Altevra remember?
8. Did any relationship/person context change?
9. Did any goal change?
10. Did any project status change?

## 2. Daily Output — what Altevra produces

Altevra should produce:

- what changed
- what matters
- active tasks
- decisions
- risks
- useful research
- personal signals
- suggested focus

## 3. Output destination

- Primary: `~/Obsidian/Imperium/Daily/YYYY-MM-DD-altevra-brief.md`
- Mirror: `journal_entries` table in SQLite (for fast query / cross-project surfacing)

## 4. Trigger surface

- Brain job `daily_briefing` (Phase 7, default 07:00 local)
- CLI: `altevra capture today` (Phase 7 — interactive evening prompt)
- MCP: `get_daily_briefing(date)` (Phase 7)
