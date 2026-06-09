# Real-Data Behavior Assessment — Altevra S0 Branch
**Date:** 2026-06-09
**Branch:** s0-foundation
**Binary:** /home/pavle/projekti/ai-tooling/altevra/target/release/altevra
**Real DB:** /home/pavle/.altevra/altevra.db (1205 turns, 13 sessions: 1157 hermes + 48 claude-code)
**Migration level:** 034 (working_dir) — confirmed present

---

## 1. Build status

```
Finished `release` profile [optimized] target(s) in 0.30s
```
Binary pre-built. No compilation errors.

---

## 2. `recall` — VERIFIED WORKING (no panic)

### `altevra recall "ReVesta"`
Returns 10 hits, all from hermes sessions, 2-3 weeks ago. Prose snippets are coherent:
- Customer Discovery Questions file references
- "ReVesta is focused specifically on surplus recovery research workflows"
- "ReVesta GTM — no-build execution day"
- Scoring (NOT_SAFE_TO_SEND) from pipeline validation
- Source correctly labeled as `hermes · ?` (tool=hermes, project=null — expected since imported sessions have no project_name)

### `altevra recall "Hermes"`
Returns 10 hits from hermes + claude-code sessions:
- Hermes context window notes (2d ago)
- hermes-agent AGENTS.md, skills (2-3w ago)
- No panic. Coherent output.

**A1 UTF-8 panic fix: VERIFIED — no crash on either query.**

---

## 3. `turn-search` — VERIFIED WORKING

### `altevra turn-search "ReVesta"`
Returns 10 scored hits with BM25-style scores:
```
[3.64] a5ac14bd idx109 (tool_result) — "/home/pavle/Obsidian/Imperium/Projects/ReVesta/..."
[3.04] aa695b75 idx92 (tool_result) — ...codex-revesta-full-fix...
```

### `altevra turn-search "Hermes" --json`
Returns valid JSON with full provenance:
- `session_id`, `turn_idx`, `created_at`, `role`, `tool_name`, `score`, `snippet`
- Scores range 1.79–3.30, correct ordering
- No panic, valid JSON structure

**A1 fix: VERIFIED on turn-search too.**

---

## 4. `memory search` — WORKING WITH CAVEAT

### Syntax
The `--query` flag does NOT exist. Correct syntax is positional: `altevra memory search "voice gateway"`.

### Behavior with default `--vault .` (runs from repo root)
- Scans altevra repo files only
- Returns 1321 chunks indexed (PLAN.md, CLAUDE.md, docs/, etc.)
- Results are repo-internal — useful for altevra development context, not Pavle's life context

### Behavior with `--vault /home/pavle/Obsidian/Imperium`
- Returns 13427 chunks indexed (full Obsidian vault)
- Returns rich, relevant results: ReVesta pipeline, Djordje directive, GTM notes, PhoneAgent data
- Genuinely useful for session bootstrap context

**Critical gap:** `memory search` has no persistent vault path config. Every call requires explicit `--vault` pointing to Obsidian. The default (`--vault .`) makes sense for testing inside the repo but produces wrong results for real-world use from any other cwd. The MCP tools likely have the correct vault configured via config.toml, so this may not affect MCP consumers.

---

## 5. `observer scan` / `observer insights` — STRUCTURAL ISSUE

### Command behavior
```
altevra observer scan     → "No patterns detected in last 7d."
altevra observer insights → "No insight files in ./10-insights/."
altevra observer scan --since 30d → "No patterns detected in last 30d."
```

### Root cause
The events table has exactly **1 row** (a `session_started` event from 2026-06-09T10:13:21). Pattern detectors require specific event types:
- `detect_recurring_drift` → needs `SkillDriftDetected` events
- `detect_repeated_hook_failure` → needs hook failure events
- `detect_low_task_velocity`, `detect_high_session_volume`, `detect_stale_project` → need corresponding event types

None of these event types have been emitted yet. Hook runs table has **0 rows**. Only 22 entries exist in `updates.jsonl`, all of type `hook.session_start`/`hook.session_end` (low-importance hook scaffolding events). These don't map to the pattern detector event types.

**Observer is structurally correct (A4 rewrite is in place) but data-starved.** It needs real hook-handle runs that emit SkillDriftDetected, hook failure, task events, etc. to produce insights. This is a chicken-and-egg: patterns emerge only after sustained live use.

---

## 6. `doctor` — 6/8 OK

```
✓ vault_initialized        .altevra/config.toml found
✓ skills_dir               3 skill(s) in 06-skills/
✓ capabilities_dir         07-capabilities/ found
✓ claude_connected         .claude/ directory found
✓ instructions_managed     altevra-instructions.md present and managed
⚠ settings_managed         settings.json exists but not managed by Altevra
  Fix: Run: altevra connect --tool claude-code (manual edit detected)
✗ skills_parseable         Parse errors in: resident-agent-core.md
  Fix: Fix YAML frontmatter (slug, version, title required)
```

### Warning: `settings_managed`
`~/.claude/settings.json` exists but was manually edited (not fully managed by altevra connect). Not a blocker but means hook wiring is partially hand-stitched.

### Failure: `skills_parseable` — resident-agent-core.md
`06-skills/resident-agent-core.md` has frontmatter with keys `id`, `type`, `mode`, `version`, `status` — but the skill parser `altevra_skills::parser::parse_skill` expects `slug`, `version`, `title`. This file is a **resident agent prompt manifest**, not a regular skill file, and shouldn't be in `06-skills/` root or the doctor needs to skip non-skill manifests. False positive failure.

---

## 7. `context` — WORKING, but updates are low-signal

```
altevra context --query "ReVesta GTM status"
```
Returns:
- **Recent Updates**: 10 entries — all `hook.session_start`/`hook.session_end` at [low] importance (no substantive content)
- **Applicable Skills**: altevra-agent-operations v0.2.0, altevra-core v0.5.0
- **Relevant Vault Excerpts**: 8 chunks from altevra repo (CLAUDE.md, docs/architecture/, etc.)
- **Gated context packet**: 24 tokens, 0 excluded

The context command works mechanically. The vault excerpts search from the altevra repo, not Obsidian — same vault gap as memory search. For a Pavle-facing session bootstrap, the context should be pointed at Obsidian to return decisions, learnings, goals.

---

## 8. Session list — WORKING

```
altevra session list → 13 sessions (12 hermes, 1 claude-code active)
```
All 13 sessions visible. Token counts show 0 for imported sessions (expected — hermes sessions don't record per-session token aggregates). The one live claude-code session shows 135 turns.

---

## 9. Schema / migration state

- Migration 034 (working_dir): APPLIED
- `sessions.working_dir`: 1/13 populated (the current live session at `/home/pavle/projekti/ai-tooling/altevra`)
- `turns.working_dir`: 258/1205 populated (turns from the current live session)
- Old imported sessions: working_dir = NULL (expected — can't backfill)
- `turns.source_tool`: correctly populated (hermes=1157, claude-code=48, 5 cross-tool edge cases)

---

## 10. Real use-case verdict: "Would Altevra surface useful past context on a new session?"

**Recall (turns-based):** YES — `recall "ReVesta"` returns coherent, dated, sourced breadcrumbs. No panics. A1 fix works.

**Turn-search:** YES — BM25 over turn content, scored, JSON output clean.

**Memory search (repo vault):** PARTIAL — works but defaults to altevra repo. Needs `--vault ~/Obsidian/Imperium` for real context. 13427-chunk Obsidian search is rich and relevant.

**Observer insights:** NO — data-starved. Needs sustained live hook events to generate patterns. Structurally ready (A4 in place), but won't produce insights until hundreds of hook-handle events have been emitted.

**Context command:** PARTIAL — mechanically works, but update feed is low-signal (only hook scaffolding events) and vault excerpts default to the repo.

**Doctor health:** 6/8 OK — one benign false positive (resident-agent-core.md wrong directory or wrong parser expectation), one actionable warning (settings_managed).

---

## 11. Gaps summary

| Gap | Impact | Fix direction |
|-----|--------|---------------|
| memory search / context default vault is repo root, not Obsidian | High — real-world use requires `--vault` flag every time | Config-driven vault default or auto-detect from config.toml |
| Observer events table nearly empty | High — no patterns ever generated | Hook-handle live use; or backfill synthesized events from turns |
| `resident-agent-core.md` in `06-skills/` fails skill parser | Low — false positive doctor FAIL | Move to separate `06-skills/modes/` or teach doctor to skip `type: resident_agent_prompt` files |
| `settings_managed` warning | Low — hooks work manually | Run `altevra connect --tool claude-code` to fully adopt settings |
| All hermes sessions have `project_name = NULL` | Medium — recall shows `?` instead of project | Import script should extract project from session metadata |
| Token counts 0 for imported sessions | Info — cosmetic | Not fixable retroactively |
