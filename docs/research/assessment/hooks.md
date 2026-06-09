# Claude Code Hook Pipeline Assessment
**Date:** 2026-06-09
**Branch:** s0-foundation
**Assessor:** Claude Code subagent (audit workflow)

---

## 1. Hook Configuration (`~/.claude/settings.json`)

All 5 hook points are wired. Exact commands:

| Hook | Command |
|------|---------|
| SessionStart | `altevra hook-handle session_start --tool claude-code --project $ALTEVRA_PROJECT` |
| Stop | `altevra hook-handle session_end --tool claude-code` |
| UserPromptSubmit | `altevra hook-handle user_prompt_submit --tool claude-code` |
| PreToolUse | `altevra hook-handle pre_tool_use --tool claude-code` |
| PostToolUse | `altevra hook-handle post_tool_use --tool claude-code` |

**NO `--db` flag in any hook command.** The binary's `--db` defaults to `default_db_path()` = `$HOME/.altevra/altevra.db` (absolute, HOME-anchored — A2 fix is in effect). This is correct: no stale CWD-relative path, no hardcoded path needed in config.

**Binary resolution:** `altevra` resolves via `~/.local/bin/altevra` → symlink → `/home/pavle/projekti/ai-tooling/altevra/target/release/altevra` (version 0.3.0, s0-foundation build). `~/.local/bin` is in `$PATH`. Hooks can locate the binary.

---

## 2. Verified: Live Capture IS Working

This session (started 2026-06-09T10:13:21Z) is being actively captured:

- Session `00900bc4-ffff-43df-8be1-bb639e9c0f0e` in `/home/pavle/.altevra/altevra.db`
- `tool = claude-code`
- `working_dir = /home/pavle/projekti/ai-tooling/altevra` (A2 fix confirmed working)
- Turn count grew from 7 → 159 over the course of this assessment
- Latest turn: `2026-06-09T10:16:12Z`

**sqlite3 evidence:**
```
SELECT id, tool, working_dir, started_at FROM sessions WHERE id='00900bc4-ffff-43df-8be1-bb639e9c0f0e';
→ 00900bc4-...|claude-code|/home/pavle/projekti/ai-tooling/altevra|2026-06-09T10:13:21.390Z

SELECT COUNT(*) FROM turns WHERE source_tool='claude-code';
→ 159 (growing in real time)
```

---

## 3. Session Keying (A2)

`current_session_path(tool, cwd)` = `$HOME/.altevra/state/session-<tool_safe>-<cwd_hash>.txt`

Hash is `DefaultHasher` of the CWD path. This is **per-tool, per-CWD** — not a global singleton. The old `.altevra/state/current_session.txt` global pointer is **deleted** (confirmed by gitStatus `D .altevra/state/current_session.txt`).

**Concurrent-session keying: works** for different CWDs or different tools. Two Claude Code sessions from different project directories get different pointer files and never collide.

**Same-CWD collision risk confirmed:** During this assessment, running a second `session_start` test from the same CWD (`/home/pavle/projekti/ai-tooling/altevra`) overwrote the pointer file. The second test used `--db /tmp/test-...db`, creating a session ID that does not exist in the canonical DB. This caused the pointer to point to a ghost session — subsequent turns from the real session would have FK-failed and been dropped silently. **I restored the pointer manually** (`echo "00900bc4-..." > ...session-claude-code-17765d84ac77f5f3.txt`).

This is an inherent limitation of the file-pointer approach: if two processes in the same CWD call `session_start` for the same tool, the second overwrites the first. In normal usage (one Claude Code session per project directory), this does not occur.

---

## 4. Schema Fields: working_dir and source_tool

- `turns.source_tool` is populated: all live turns show `source_tool = 'claude-code'`. **Verified.**
- `turns.working_dir` is populated: all live turns show `working_dir = /home/pavle/projekti/ai-tooling/altevra`. **Verified.**
- `sessions.working_dir` is populated at session_start via `resolve_working_dir()` which prefers `$CLAUDE_PROJECT_DIR` env var, falls back to `std::env::current_dir()`. **Verified.**
- Old sessions (before S0 build: `04ccfabb`, `89a422af`) have `working_dir = NULL`. Expected.

---

## 5. Context Injection: NOT Implemented

**The hooks are capture-only. There is no context injection into Claude Code prompts.**

- `session_start` returns `{"session_id":"<uuid>"}` — not the `{"additionalContext":"..."}` format Claude Code uses for injection.
- `user_prompt_submit` returns **no output** — just records the turn silently.
- `pre_tool_use` / `post_tool_use` — return no output.

Context injection (e.g., delivering past decisions / active goals / context packet at session start or on each prompt) is **not yet built** into the hook pipeline. It would require `session_start` or `UserPromptSubmit` to return `{"additionalContext": "..."}`.

This is by design for S0 (recorder phase), but should be noted as a gap before S3 (context injection from wiki + memory).

---

## 6. ALTEVRA_PROJECT env var

`$ALTEVRA_PROJECT` is **not set** in the environment. The `SessionStart` hook passes `--project $ALTEVRA_PROJECT` which expands to empty string / nothing. In `hook_handle.rs`, `args.project = None`, so `sessions.project_name = NULL`. Sessions are not tagged with a project name.

This is a usability gap: project-level grouping of sessions is disabled until `ALTEVRA_PROJECT` is set (e.g., in `.env` or shell profile, or via `altevra connect --project foo`).

---

## 7. FK Error Handling

When `read_current_session()` returns an ID that doesn't exist in the DB (stale pointer, wrong `--db`), `record_turn` gets an FK constraint violation. The code handles this gracefully:
- Logs `[altevra] turn not recorded (FK mismatch — stale session pointer?): ...` to stderr
- Returns `Ok(())` so the hook exits 0 and never blocks Claude Code

This is correct behavior. The only observable symptom is turns being silently dropped — no user-visible breakage.

---

## 8. Stop Hook

The `Stop` hook calls `session_end` which:
1. Reads the pointer file → gets session ID
2. Calls `repo.end_session(id, summary)` → sets `ended_at`
3. Inserts a `SessionEnded` event (non-fatal)
4. Calls `enqueue_session_signal` → creates one `improvement_signal` row (C1 producer, SI-6 gated)
5. Removes the pointer file via `clear_current_session`

This was not tested destructively (to avoid ending the live session), but the code path is straightforward and matches the session_start pattern.

---

## 9. Summary of Verified vs Unverified

| Claim | Status | Evidence |
|-------|--------|---------|
| Hooks are wired in settings.json | VERIFIED | Direct read of file |
| `altevra` binary in PATH for hooks | VERIFIED | `which altevra` + turns accumulating |
| Binary defaults to canonical DB (no --db needed) | VERIFIED | `--db` default = `default_db_path()` = `$HOME/.altevra/altevra.db` |
| A2 absolute path fix is active | VERIFIED | Session and turns show absolute `working_dir` |
| Old global pointer deleted | VERIFIED | `current_session.txt` does not exist |
| Per-CWD session keying works | VERIFIED | Pointer file named `session-claude-code-<cwd_hash>.txt` |
| Turns accumulate in real time | VERIFIED | 7 → 159 during assessment |
| `source_tool = 'claude-code'` on turns | VERIFIED | sqlite3 query |
| `working_dir` populated on turns | VERIFIED | sqlite3 query |
| Context injection into prompts | NOT IMPLEMENTED | hook output is empty / non-additionalContext |
| ALTEVRA_PROJECT tagging | NOT WORKING (env unset) | `ALTEVRA_PROJECT` not in env |
| Same-CWD session pointer collision | BUG (low-impact in normal use) | Reproduced and documented above |

---

## 10. Break Points / Gaps

1. **No context injection** — hooks are recorders, not injectors. Claude Code sessions start "blind" (no past context surfaced). This is the S0→S3 gap.
2. **ALTEVRA_PROJECT unset** — `sessions.project_name` is always NULL. Set it in shell profile or via `altevra connect`.
3. **Same-CWD session collision** — if two `session_start` calls happen for the same tool+CWD, the second overwrites the pointer. Low risk in normal single-session-per-directory use, but a real footgun in subagent/worktree scenarios (like this assessment!).
4. **`altevra` uses non-absolute command in settings.json** — relies on PATH. If hooks ever run in a restricted PATH environment, they will silently fail. Consider using `/home/pavle/.local/bin/altevra` as absolute path.
5. **Old sessions have no working_dir** — pre-S0 sessions (04ccfabb, 89a422af) have `working_dir = NULL`. This is historical, not a current bug.
