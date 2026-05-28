# Daily Briefing v1 — output template

This is a markdown template (not JSON) because the daily brief is consumed
by Pavle in Obsidian, not by another agent.

Resident agent in `daily_briefing` mode must produce output matching this
shape. Section headers MUST appear verbatim. Sections with no signal stay
present but empty (so future diffs detect when they stop being empty).

```markdown
---
kind: altevra-daily-brief
generated_by: resident-agent
date: YYYY-MM-DD
mode: daily_briefing
schema_version: 1
confidence: medium
---

# Daily Brief — YYYY-MM-DD

## What Changed

- bullet 1
- bullet 2

## What Matters

- bullet 1

## Decisions

- decision A (link/source)

## Tasks Needing Attention

- task title — why it matters today

## Useful Research

- 1-line title — link

## Risks

- risk → mitigation

## Personal Signals

- relationship / health / mood pattern surfaced (only if `sensitivity:internal` or below)

## Suggested Focus

- recommended top-3 actions for today
```

Rules:

1. No empty prose. Each bullet must be actionable or evidence-backed.
2. Personal Signals section is omitted entirely when no allowed signal is available — never fabricate.
3. `confidence` in frontmatter: `low | medium | high` based on input volume + recency.
4. Total target length: 250–600 words. Anything longer is noise.
