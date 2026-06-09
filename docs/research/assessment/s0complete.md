# S0 Completeness Assessment — s0-foundation branch
**Date:** 2026-06-09
**Branch:** s0-foundation
**Assessor:** Claude Sonnet 4.6 (automated subagent)
**Binary:** /home/pavle/projekti/ai-tooling/altevra/target/release/altevra (v0.3.0)

---

## Branch status

`git rev-parse master` == `git rev-parse s0-foundation` == `3bd8e38`.
The S0 work lives **entirely in uncommitted working tree changes** (no commits ahead of master).
All files show as `modified:` in `git status` — the branch is effectively a working-tree-only diff.

---

## S0 Task Map

### A1 — UTF-8 snippet panic fix
**Status: DONE + TESTED**

Evidence:
- `crates/altevra-cli/src/commands/turn_search.rs:96-122`: `snap_to_char_boundary_left` + `snap_to_char_boundary_right` helpers replace the old raw `content[start..end]` slice.
- Same helpers in `crates/altevra-cli/src/commands/recall.rs:450-463`.
- Regression tests `snippet_multibyte_no_panic` present in both files with Serbian Cyrillic + arrow → (3-byte UTF-8) fixtures.
- `cargo test --workspace` green (953 total, 0 failed).
- Live smoke: `altevra recall "ReVesta" --limit 3 --json` against real DB (1598 turns) — no panic, returned 3 results with valid UTF-8 snippets.
- Live smoke: `altevra turn-search "Altevra" --limit 3 --json` against real DB — no panic, correct output.

Has tests: YES (hermetic fixture-based regression tests in both files).

---

### A2 — Absolute-path foundation
**Status: DONE + TESTED**

Evidence:
- `crates/altevra-core/src/paths.rs`: `default_db_path()` computes `$HOME/.altevra/altevra.db` anchored; `DEFAULT_DB_PATH` is a bare suffix for clap only; `current_session_path(tool, cwd)` keyed by tool + CWD hash (fixes the global pointer file problem).
- `default_brain_pid_path()` and `default_watcher_pid_path()` both use `home_dir()`.
- All tested CLI commands (`recall`, `turn_search`, `observer`, `hook_handle`, `brain`, `watch`, `context`, `prompt`) use `altevra_core::default_db_path()` as clap default, confirmed by grep.
- MCP tools (`tools_capabilities.rs`, `tools_sessions.rs`, `tools_observer.rs`) use `altevra_core::default_db_path()`.
- `tools_tasks.rs` and `tools_updates.rs` use `altevra_core::home_dir().join(...)` (correct).
- `altevra-watcher/src/daemon.rs`: `event_log_path` uses `altevra_core::home_dir().join(...)` (correct).
- 5 unit tests in `paths.rs`: `default_db_path_is_home_anchored`, `default_db_path_anchored_under_home`, `default_db_path_respects_env_override`, `default_db_path_ignores_empty_env`, `current_session_path_is_absolute_and_unique_per_tool_and_cwd`.
- Live verification: binary invoked with `ALTEVRA_DB_PATH=""` connects to `~/.altevra/altevra.db`, not to CWD-relative path.

**Gap (open, from PLAN.md S0 item 3):** Installed hooks in `~/.claude/settings.json` do NOT pass an explicit `--db` flag. They rely on `default_db_path()` at runtime. Since `default_db_path()` now correctly returns the home-anchored path, this is functionally OK, but PLAN.md calls for regenerating hooks with an explicit `--db` for belt-and-suspenders safety. Also: `$ALTEVRA_PROJECT` is still in the `session_start` hook command — the PLAN.md review log flags this as an issue.

Has tests: YES (unit tests in paths.rs).

---

### A3 — working_dir migration 034
**Status: DONE + TESTED**

Evidence:
- `crates/altevra-db/migrations/034_working_dir.sql`: `ALTER TABLE sessions ADD COLUMN working_dir TEXT; ALTER TABLE turns ADD COLUMN working_dir TEXT;` + backfill UPDATE.
- `SessionRow` has `working_dir: Option<String>` field; `TurnRow` has `working_dir: Option<String>` field.
- `sessions.rs:start_session()` binds `working_dir` in INSERT; `record_turn()` binds `working_dir` in INSERT.
- `hook_handle.rs:resolve_working_dir()` captures `$CLAUDE_PROJECT_DIR` → `current_dir()` → None.
- `handle_session_start()` and `record_turn()` both call `resolve_working_dir()` and store it.
- Tests: `recall.rs` and `turn_search.rs` test fixtures use `SessionRow { working_dir: None }` and `TurnRow { working_dir: None }` — they compile and pass.
- Real DB (`~/.altevra/altevra.db`): migration 034 applied, `working_dir` column verified via `PRAGMA table_info`.

**Gap:** No explicit roundtrip test verifying that a session with a non-None `working_dir` gets stored and retrieved correctly. The PLAN.md calls for "roundtrip tests for session-level and per-turn cwd." Existing tests only use `working_dir: None`.

Has tests: PARTIAL (compiles, schema applied, but no roundtrip test for non-null working_dir value).

---

### A4 — Observer pipeline rewrite (SQLite readers)
**Status: DONE + TESTED**

Evidence:
- `crates/altevra-cli/src/commands/observer.rs:60-207`: `load_events_from_db()` queries `EventsRepository::list_since()` first; falls back to flat JSONL only when SQLite is unreachable or events table is empty.
- `crates/altevra-mcp/src/tools_observer.rs:73-140`: same pattern (`load_events_from_db_sync()` via a spawned thread).
- `crates/altevra-brain/src/jobs.rs:211-268`: `run_observer_scan()` queries `EventsRepository::new(pool).list_since()`, calls `detect_patterns()`, and persists each insight as a `kind="improvement"` proposal via `ProposalsRepository` (deduped by title).
- `hook_handle.rs:handle_session_start()` and `handle_session_end()` insert `EventsRepository` rows on each event (best-effort, non-fatal).
- Test `scan_returns_insight_from_seeded_sqlite_events` in `observer.rs:371-408`: seeds 3 `SkillDriftDetected` events in SQLite, runs `run_scan`, verifies no panic (SQLite path exercised).
- Live smoke: `altevra observer scan --since 7d --json` against real DB — returned 0 insights (events table has 3 rows from this session — not enough to trigger pattern detectors), no error.

**Gap:** The `jobs.rs:202-218` hardcoded relative event paths mentioned in the audit are gone — the job now uses SQLite directly. However, the `run_event_classifier` job (`jobs.rs:130-204`) still reads `ctx.vault_path.join(".altevra/events/file_changes.jsonl")` (a vault-relative path, not a hardcoded literal). This is correct design (vault path is injected from brain config), not a bug.

Has tests: YES (fixture SQLite test in observer.rs; hermetic roundtrip in jobs tests).

---

### A5 — `altevra db unify` command
**Status: MISSING — NOT IMPLEMENTED**

Evidence:
- `altevra db --help` returns `error: unrecognized subcommand 'db'`.
- No `db.rs` command file exists in `crates/altevra-cli/src/commands/`.
- No grep matches for `db_unify`, `DbUnify`, `db unify` anywhere in the crates.
- No maintenance lock implementation found.
- No quarantine-before-delete logic for shadow DBs found.

**This is the primary blocking gap for S0 completion.**

Shadow DB inventory (confirmed live):
| Path | Turns | Sessions | Migration |
|------|-------|----------|-----------|
| `~/.altevra/altevra.db` (CANONICAL) | 1598 | 15 | 034 |
| `/home/pavle/projekti/ai-tooling/altevra/.altevra/altevra.db` | 1101 | 12 | **033** |
| `/home/pavle/projekti/ai-tooling/altevra/crates/altevra-mcp/.altevra/altevra.db` | 0 | 0 | — |
| `/home/pavle/.hermes/.altevra/altevra.db` | 0 | 0 | — |
| `/home/pavle/Downloads/.altevra/altevra.db` | 0 | 0 | — |
| `/home/pavle/Ideje/.altevra/altevra.db` | 0 | 0 | — |
| `/home/pavle/projekti/arhiva/projects-1/EAF-Steel-Agent/.altevra/altevra.db` | 0 | 0 | — |
| `/home/pavle/projekti/biznis/simple-surplus/.altevra/altevra.db` | 0 | 0 | — |

The most significant shadow DB is at the repo root with 1101 turns (migration 033, no `working_dir`). Those turns are NOT in the canonical DB. The PLAN.md's full spec for `db unify` (WAL safety, exclusive lock, dedup logic, quarantine-not-delete, `--dry-run`) remains unbuilt.

Has tests: MISSING.

---

### Additional S0 items from PLAN.md

#### Hook robustness (S0 item 2)
**Status: DONE**
`hook_handle.rs:416-427`: FK errors are caught and downgraded to `eprintln!` warnings; the function returns `Ok(())` in all error paths — hook never exits non-zero.

#### Session pointer keyed by tool+cwd (S0 item 1 / paths.rs)
**Status: DONE**
`current_session_path(tool, cwd)` keyed by tool name + CWD hash — verified in tests.

#### pending_indexing dead-end (S0 item 8)
**Status: PARTIAL**
`jobs.rs:run_vault_indexer()` uses `ON CONFLICT ... DO UPDATE SET status = CASE WHEN excluded.status = 'failed' THEN 'pending' ELSE status END` — existing `pending` rows are not re-queued (doesn't grow forever). The queue IS drained by `altevra embed run`. Comment in code says "consumed by `altevra embed run`". The "dead-end" from the audit is addressed in the sense that failed rows get reset. However, no test verifies that `pending_indexing` rows are actually consumed by the embed worker (embed worker itself is not part of S0 scope).

---

## `cargo test --workspace` results (ALTEVRA_DB_PATH=/tmp/... temp DB)

All 21 test binaries passed. Total: **953 tests, 0 failed, 2 ignored**.

Selected pass results by crate:
- `altevra-brain`: 185 passed
- `altevra-cli`: 57 passed
- `altevra-core`: 161 passed
- `altevra-db`: 66 passed
- `altevra-mcp`: 41 passed

---

## Summary: S0 Completion Status

| Task | Status | Has Tests |
|------|--------|-----------|
| A1 UTF-8 panic fix (recall + turn_search) | **DONE** | YES |
| A2 Absolute-path foundation (paths.rs, all commands/MCP) | **DONE** | YES |
| A2b Regenerate installed hook configs with --db | **OPEN** | NO |
| A3 working_dir migration 034 + row mappers + capture | **DONE** | PARTIAL |
| A3b working_dir roundtrip test (non-null) | **OPEN** | NO |
| A4 Observer pipeline SQLite rewrite (CLI+MCP+brain jobs) | **DONE** | YES |
| A5 `altevra db unify` command | **MISSING** | MISSING |
| A5 Maintenance lock / quarantine logic | **MISSING** | MISSING |
| Hook robustness (FK error → warning) | **DONE** | YES (guard_json test) |
| Session pointer keyed by tool+cwd | **DONE** | YES |
| pending_indexing dead-end mitigation | **PARTIAL** | NO |

**S0 is NOT complete.** The only blocking missing item is A5 (`db unify`), which is the biggest piece of the spec. All other tasks (A1-A4) are implemented and passing tests. The branch compiles cleanly and all 953 tests pass.

---

## What is needed to call S0 complete

1. **Implement `altevra db unify`** (the blocker):
   - `altevra db --help` + `altevra db unify --dry-run` / `altevra db unify`
   - WAL-safe: stop brain (refuse if PID alive), backup each shadow DB + its `-wal`/`-shm`
   - Exclusive SQLite lock, merge in one transaction
   - Dedup: `(tool, external_id)` for imported sessions; content-hash-sequence for live (null external_id) sessions
   - Turn FK remap to kept session ID
   - Quarantine (rename to `~/.altevra/backups/<ts>/`) old shadow DBs, never delete
   - Maintenance lock file (`~/.altevra/locks/unify.lock`) that hook_handle, brain, embedder check
   - Quarantine-then-replay for hook events that fire during unify
   - Hermetic fixture test: seeded multi-DB → unify → exact union counts, zero FK violations

2. **Add working_dir roundtrip test** (minor, non-blocking):
   - `seed_session` with `working_dir: Some("/home/pavle/proj".into())` → retrieve → assert round-trips correctly

3. **Regenerate installed hooks with explicit `--db`** (recommended, non-blocking for CI):
   - Run `altevra install-hooks` against the live config to push `--db /home/pavle/.altevra/altevra.db` into each hook command
   - Remove `$ALTEVRA_PROJECT` from `session_start` hook (or replace with `$CLAUDE_PROJECT_DIR`)

4. **S0 gate test**: hermetic fixture test asserting `altevra db unify` + observer scan returns >=1 insight from seeded SQLite events (observer test already exists; unify test is missing).
