# Altevra Deep Audit — 2026-06-07

## 1. Executive summary

Altevra's core infrastructure (build, binary, test suite) is healthy — 892 tests pass, zero compiler warnings, the binary resolves correctly from PATH. However, four of seven subsystems have critical failures that make the system functionally broken for its primary purpose: the hook pipeline writes zero turns to the canonical database, the observer-insights subsystem returns zero insights universally, and the memory/recall CLI panics on any real-world query containing multi-byte UTF-8 characters (Serbian diacritics, arrows). The single most important problem is a CWD-relative default DB path (`".altevra/altevra.db"`) that permeates every subsystem — hooks, brain, MCP server, and CLI — causing at least five separate SQLite databases to accumulate silently across the filesystem, with no subsystem reading the same database as any other. Until that one-line fix in `paths.rs` lands and hooks are regenerated, Altevra is recording data into isolated shadow stores that no tool consumes.

---

## 2. Bug board (sorted by severity)

| Severity | Subsystem | Bug | Location | Repro | Suggested fix |
|----------|-----------|-----|----------|-------|---------------|
| Critical | hook-pipeline | Relative DB path creates per-project shadow databases — all claude-code turns lost | `crates/altevra-core/src/paths.rs:15` | `cd /tmp && echo '{}' \| altevra hook-handle session_start --tool claude-code; ls /tmp/.altevra/altevra.db` | Change `DEFAULT_DB_PATH` to `dirs::home_dir().join(".altevra/altevra.db")`; bake absolute path into generated hook commands |
| Critical | hook-pipeline | Relative CURRENT_SESSION_FILE isolates session state per project directory | `crates/altevra-cli/src/commands/hook_handle.rs:27` | `mkdir /tmp/proj1 && cd /tmp/proj1 && altevra hook-handle session_start --db ~/.altevra/altevra.db; ls /tmp/proj1/.altevra/state/current_session.txt` | Change `CURRENT_SESSION_FILE` to an absolute path computed from `$HOME` at runtime |
| Critical | observer-insights | `events` table is never populated in production — `detect_patterns` always gets empty input | `crates/altevra-brain/src/selfimprove.rs:322-326`; no production caller of `EventsRepository::insert()` | `sqlite3 ~/.altevra/altevra.db 'SELECT COUNT(*) FROM events;'` → 0 | `hook_handle.rs` session_end/session_start must call `EventsRepository::insert()` with appropriate `EventType`; or add a bridge job |
| Critical | observer-insights | Observer scan CLI and MCP tool read flat JSONL files that are never written | `crates/altevra-cli/src/commands/observer.rs:191-228`; `crates/altevra-mcp/src/tools_observer.rs:79-115` | `ls ~/.altevra/events/` → empty; `altevra observer scan --json` → `{count:0}` | Rewrite `load_events_for_observer` to query SQLite via `EventsRepository` instead of reading flat files |
| Critical | brain-daemon | All DB and event paths are CWD-relative — brain started from different directories silently creates multiple isolated SQLite databases | `crates/altevra-core/src/paths.rs:15`; `crates/altevra-cli/src/commands/brain.rs:24,39,55,59` | `cd ~ && altevra brain status` vs `cd repo && altevra brain status` — different `last_runs` | Change `DEFAULT_DB_PATH` to absolute `$HOME`-relative; change PID file default to `$HOME/.altevra/brain.pid` |
| Critical | memory-search-recall | `snippet()` panics on multi-byte UTF-8 content — `turn-search --json` and `recall` crash | `crates/altevra-cli/src/commands/turn_search.rs:92-96`; `crates/altevra-cli/src/commands/recall.rs:431-436` | `altevra turn-search 'ReVesta' --json` → panic `start byte index 531 is not a char boundary` | Snap byte offsets to char boundaries using `is_char_boundary` before slicing, matching existing `snap_left`/`snap_right` in `altevra-memory/src/search.rs:218-228` |
| Critical | import-pipeline | Codex `history.jsonl` parser produces empty/garbage sessions — field name mismatch (`session_id`/`ts`/`text` vs `thread_id`/`timestamp`/`content`) | `crates/altevra-cli/src/commands/analyze/parsers/codex.rs:29-42` | `python3 -c "import json; print(list(json.loads(open('/home/pavle/.codex/history.jsonl').readline()).keys()))"` → `['session_id','ts','text']` | Add `#[serde(alias="session_id")]`, `#[serde(alias="ts")]` as `Option<i64>`, `#[serde(alias="text")]` on content field |
| High | hook-pipeline | FK constraint hard failure (exit 1) when session pointer crosses DB boundaries — Claude Code hook blocks | `crates/altevra-cli/src/commands/hook_handle.rs:257-263` | `cd repo && echo '{"user_prompt":"test"}' \| altevra hook-handle user_prompt_submit --tool claude-code` → exit 1, `FOREIGN KEY constraint failed` | Fix absolute path bugs 1+2; additionally wrap `record_turn` FK errors as warnings (never exit 1 from a hook) |
| High | observer-insights | `run_observer_scan` brain job uses hardcoded relative path, ignores `ctx.vault_path` | `crates/altevra-brain/src/jobs.rs:204-218` line 205 | `grep -n 'std::path::Path::new' crates/altevra-brain/src/jobs.rs` shows `.altevra/events/updates.jsonl` hardcoded | Replace with `ctx.vault_path.join(".altevra/events/updates.jsonl")` |
| High | observer-insights | `selfimprove.rs` observer-insight-to-proposal path is dead code in practice | `crates/altevra-brain/src/selfimprove.rs:320-436` | `sqlite3 ~/.altevra/altevra.db 'SELECT source_mode FROM proposals;'` → only `self_improve`, zero observer-sourced | Follows from fixing events table population (Bug 3 in observer) |
| High | brain-daemon | `run_event_classifier` and `run_observer_scan` hardcode CWD-relative event file paths, ignoring `ctx.vault_path` | `crates/altevra-brain/src/jobs.rs:134-136, 205` | `cd /tmp && altevra brain tick` → event_classifier checks `/tmp/.altevra/events/file_changes.jsonl` (wrong) | Replace all `std::path::Path::new(".altevra/events/...")` with `ctx.vault_path.join(...)` |
| High | brain-daemon | `vault_indexer` perpetually re-queues 50 files to `pending_indexing` but brain has no consumer — queue grows unboundedly | `crates/altevra-brain/src/jobs.rs:225-236` | `sqlite3 ~/.altevra/altevra.db 'SELECT status,COUNT(*) FROM pending_indexing GROUP BY status;'` → only `pending` rows, never decreases | Wire embedder as a brain job (new `JobKind::EmbedWorker`) OR fix `ON CONFLICT` to only reset `failed` rows; document that `altevra embed run` must share the same DB |
| High | mcp-server | MCP config `vault=/home/pavle` points to empty directory — skill/wiki tools return 0 results | `/home/pavle/.claude.json` mcpServers.altevra.args; `crates/altevra-mcp/src/tools_bootstrap.rs:21` | `list_skills` with vault `/home/pavle` → `{count:0}` because `/home/pavle/06-skills/` is empty | Change MCP config args to `--vault /home/pavle/projekti/ai-tooling/altevra`; keep `search_memory` vault as `/home/pavle` (Obsidian) |
| High | mcp-server | DB split-brain: MCP session tools default to CWD-relative DB while live hooks write to `~/.altevra/altevra.db` | `crates/altevra-core/src/paths.rs:15`; `crates/altevra-mcp/src/tools_sessions.rs:36-41` | `recall_window` returns sessions from project-local DB, misses today's claude-code sessions in home DB | Add `env: {ALTEVRA_DB_PATH: "/home/pavle/.altevra/altevra.db"}` to `~/.claude.json` MCP config |
| High | import-pipeline | Codex `state_5.sqlite` metadata query silently fails — no `project` column, `created_at` is INTEGER not RFC3339 | `crates/altevra-cli/src/commands/analyze/parsers/codex.rs:63-67` | `sqlite3 ~/.codex/state_5.sqlite 'SELECT id,title,project,created_at FROM threads LIMIT 1;'` → `Parse error: no such column: project` | Change query to `SELECT id, title, cwd, created_at FROM threads`; parse `created_at` as Unix epoch integer |
| High | import-pipeline | Claude Code import (`--tool claude-code`) not wired into `import` CLI — 875 session files unimported | `crates/altevra-cli/src/commands/import.rs:89-99` | `altevra import --tool claude-code` → `Error: unsupported --tool claude-code` | Add `"claude-code"` match arm calling `parsers::claude_code::parse_file`; walk `~/.claude/projects/**/*.jsonl` |
| High | import-pipeline | Codex import (`--tool codex`) not wired into `import` CLI — 285 history entries unimported | `crates/altevra-cli/src/commands/import.rs:89-99` | `altevra import --tool codex` → `Error: unsupported --tool codex` | Add `"codex"` match arm after fixing field-name (Bug above) and state DB (Bug above) parsers |
| High | memory-search-recall | Relative `DEFAULT_DB_PATH` causes two separate databases depending on CWD | `crates/altevra-core/src/paths.rs:15` | `sqlite3 repo/.altevra/altevra.db 'SELECT COUNT(*) FROM turns;'` → 1101 vs `sqlite3 ~/.altevra/altevra.db` → 1157 | Change `default_db_path()` to use `dirs::home_dir().join(".altevra/altevra.db")` |
| Medium | hook-pipeline | Hook commands include `$ALTEVRA_PROJECT` which Claude Code never sets — stored as `Some("")` | `crates/altevra-adapters/src/claude_code.rs:127` | `grep ALTEVRA_PROJECT ~/.claude/settings.json; echo $ALTEVRA_PROJECT` → empty | Remove `--project $ALTEVRA_PROJECT`; derive project name from `$CLAUDE_PROJECT_DIR` inside `hook_handle.rs` |
| Medium | hook-pipeline | `connect` adapter installs project-level hooks inside altevra repo itself — violates SI-6 | `crates/altevra-adapters/src/claude_code.rs` install method | `cat repo/.claude/settings.json \| python3 -c "import sys,json;print(json.load(sys.stdin).get('_altevra_managed'))"` → true | Add SI-6 self-skip check (from `install_hooks.rs:130`) into `ClaudeCodeAdapter::install()` |
| Medium | brain-daemon | `insight_synthesizer` (SI-14) writes LLM refusal text as insight cards — empty-string guard insufficient | `crates/altevra-brain/src/jobs.rs:280-285` | `sqlite3 ~/.altevra/altevra.db "SELECT title FROM insight_cards ORDER BY created_at DESC LIMIT 3;"` → refusal strings | Detect refusal phrases in SI-14 guard OR inject real event context into the synthesizer prompt |
| Medium | brain-daemon | `brain tick` default `--disabled` only excludes `daily_summary` — network jobs block tick and trigger 30s timeout | `crates/altevra-cli/src/commands/brain.rs:59` | `timeout 30 altevra brain tick` → exit 124 with RSS parse errors in stderr | Change default `--disabled` to also exclude `research_fetcher,github_trending_fetch,feed_discovery,project_research_sweep`; fix broken feed URLs |
| Medium | mcp-server | `handle_replay_session` has dead `rt` variable; all session handlers call `futures::executor::block_on` inside tokio — latent deadlock | `crates/altevra-mcp/src/tools_sessions.rs:68-104` | `grep -n 'let rt\|block_on' crates/altevra-mcp/src/tools_sessions.rs` | Adopt `tools_capabilities.rs` pattern: `std::thread::spawn` + `tokio::runtime::Builder::new_current_thread().block_on()` |
| Medium | mcp-server | `tools_tasks`, `tools_updates`, `tools_capabilities` use CWD-relative state paths — tasks scatter across working directories | `crates/altevra-mcp/src/tools_tasks.rs:8-9`, `tools_capabilities.rs:6-9` | `cd /tmp && save_task "test"; cd ~ && get_active_tasks` → 0 tasks | Replace CWD-relative constants with `vault_path.join(".altevra/state/...")` or `~/.altevra/state/` |
| Medium | memory-search-recall | `pending_indexing` queue is a dead-end — 51 items inserted by watcher but no code ever reads the table | `crates/altevra-watcher/src/daemon.rs:203`; `crates/altevra-brain/src/jobs.rs:229` | `sqlite3 ~/.altevra/altevra.db 'SELECT status,COUNT(*) FROM pending_indexing GROUP BY status;'` → `pending\|51` | Wire `altevra embed` worker to drain `pending_indexing`, or remove indirection and have watcher feed `memory_chunks` directly |
| Medium | build-and-binary | `resident-agent-core.md` uses wrong frontmatter schema (`id` not `slug`, missing `title`) — doctor FAIL, skill silently excluded | `06-skills/resident-agent-core.md:1-9` | `altevra doctor` → `✗ skills_parseable — Parse errors in: resident-agent-core.md`; `altevra skill list` → 2 skills not 3 | Replace `id: skill_resident_agent_core` with `slug: resident-agent-core`; add `title: Altevra Resident Agent Core` |
| Medium | import-pipeline | Silent data loss in `--since` filter: non-standard hermes JSONL filenames silently excluded | `crates/altevra-cli/src/commands/import.rs:153-162` | Create `test.jsonl` in custom source-dir, run with `--since`; file absent from all stats counters | Change `.unwrap_or(false)` to `.unwrap_or(true)` or emit a warning and count in `errors` |
| Low | build-and-binary | `file_change_count` in `import.rs` is `let` not `let mut` — permanently 0, skill candidacy never fires for file-change-only sessions | `crates/altevra-cli/src/commands/import.rs:245` | `grep -n 'file_change_count' crates/altevra-cli/src/commands/import.rs` — no `mut`, no increment | Change to `let mut file_change_count = 0_i64;` and add increment in the turns loop |
| Low | build-and-binary | `doctor` WARN: `settings.json` not managed by Altevra (`ALTEVRA_MANAGED` marker absent) | `.claude/settings.json` in repo root | `altevra doctor` → `⚠ settings_managed` | Run `altevra connect --tool claude-code` or add `// ALTEVRA_MANAGED: true` to `.claude/settings.json` |
| Low | hook-pipeline | Unknown hook event exits 0 (silent success) — undetectable misconfiguration | `crates/altevra-cli/src/commands/hook_handle.rs:72-74` | `echo '{}' \| altevra hook-handle bad_event --tool claude-code; echo $?` → 0 | Exit with code 2 for unknown events to surface mismatches during install-time testing |
| Low | mcp-server | `get_capabilities` returns hardcoded `mcp_tools: 22` but `tools/list` exposes 40 tools | `crates/altevra-mcp/src/tools_capabilities.rs:58` | `tools/list` → 40 tools; `get_capabilities` → `mcp_tools: 22` | Replace hardcoded 22 with dynamic count via `tool_count()` function |
| Low | memory-search-recall | `turn-search` silently returns 0 results for 2-character tokens (AI, DB, UI, etc.) | `crates/altevra-cli/src/commands/turn_search.rs:83-85` | `altevra turn-search 'AI'` → `No turns match AI` despite thousands of matching rows | Change filter from `t.len() > 2` to `t.len() >= 2` |

---

## 3. Per-subsystem status

### build-and-binary — Partial
Build is healthy: `cargo build --release` succeeds incrementally, zero compiler warnings, zero clippy hits, all 892 workspace tests pass. Two real bugs lurk in uncommitted code: `file_change_count` is a non-`mut` binding permanently stuck at zero in `import.rs`, and `resident-agent-core.md` uses the wrong frontmatter key (`id` instead of `slug`) causing `doctor` to report `skills_parseable: FAIL` and silently exclude the skill from the registry. Evidence: `altevra skill list` shows 2 skills, not 3; `altevra doctor` shows one FAIL and one WARN.

### brain-daemon (crates/altevra-brain) — Partial
The scheduler registers and fires all 14 `JobKind` variants without panics, and 189 jobs have completed with zero failures in the live DB. However five bugs collectively break the end-to-end pipeline: CWD-relative DB and event paths mean the running daemon (PID 401596, started from `/home/pavle`) and the CLI tools (run from the repo dir) read from completely different SQLite files; `vault_indexer` re-queues 50 files every run but the `pending_indexing` table has no consumer; and `insight_synthesizer` writes LLM refusal strings as insight cards because it sends no actual context to the LLM. Evidence: five separate `altevra.db` files found on disk; `pending_indexing` shows 51 permanently-pending rows; insight_cards contains `"No recent activity data was provided..."` as a card title.

### observer-insights — Broken
The observer subsystem returns zero insights universally despite 1157 turns in DB. The `detect_patterns` detectors are logically correct (unit tests pass when given seeded data) but are permanently starved of input because no production code path writes to the `events` table or the flat JSONL files the CLI and MCP tool read. The entire `improvement_signals → proposals` path does function (9 hermes signals produced 1 applied proposal), but the observer half of the self-improve loop is completely inert. Evidence: `sqlite3 ~/.altevra/altevra.db 'SELECT COUNT(*) FROM events;'` → 0; `ls ~/.altevra/events/` → empty directory; `altevra observer scan --json` → `{count:0}`.

### hook-pipeline — Broken
Hooks fire correctly at the OS level (wired in `~/.claude/settings.json`) but zero claude-code turns reach the canonical database. The root cause is that both the DB path and the session-state pointer file are hardcoded as relative paths, creating per-project shadow DBs invisible to every other Altevra subsystem. A secondary cascading failure causes FK violation exit-1 crashes when a session created against one DB is referenced by a hook running against another. Five shadow DBs accumulate on disk containing 1101 turns and 12 sessions entirely invisible to `~/.altevra/altevra.db`. Evidence: `~/.altevra/altevra.db` has 11 sessions all tool=hermes; the repo-local shadow DB has 1 claude-code session with 1100 turns.

### import-pipeline — Partial
Hermes JSONL import is fully working and idempotent (9 sessions, correct deduplication, `--since` filter, dry-run safety all verified). The Cursor `ai-tracking` SQLite read works in dry-run and preserves the source file. However Codex import is broken at the parser level (field name mismatch: real file uses `session_id`/`ts`/`text`, struct expects `thread_id`/`timestamp`/`content`) and at the state DB query level (no `project` column, `created_at` is INTEGER not RFC3339). Neither Claude Code nor Codex have match arms in the `import` CLI, leaving 875 Claude Code JSONL files and 285 Codex history entries unimported. Evidence: `altevra import --tool claude-code` → `Error: unsupported`; Codex fields confirmed by `python3 -c "import json; print(list(json.loads(open('/home/pavle/.codex/history.jsonl').readline()).keys()))"` → `['session_id','ts','text']`.

### mcp-server — Partial
The MCP server starts cleanly, completes the JSON-RPC handshake, and registers 40 tools. Core tools (`search_memory`, `recall_window`, `search_turns`, `replay_session`, `propose_improvement`) return structurally valid responses. However, the vault path in the MCP config (`/home/pavle`) points to an empty `06-skills/` directory causing skill and wiki tools to return 0 results; the DB path defaults CWD-relative so session tools miss today's live sessions; `get_capabilities` hardcodes `mcp_tools: 22` when 40 tools are registered; and all session handlers use `futures::executor::block_on` with a dead `rt` variable creating a latent deadlock if the outer runtime ever becomes current-thread. Evidence: `list_skills --vault /home/pavle` → `{count:0}`; `get_capabilities` → `mcp_tools: 22`; `tools/list` → 40 tools.

### memory-search-recall — Partial
The underlying data (1157+ turns, 65 learnings, FTS index) is present and lexical search works for purely ASCII queries. However both `turn-search --json` and `recall` crash with a panic on any query that matches a turn containing multi-byte UTF-8 characters near the snippet window boundary — which covers the majority of real project data (Serbian text, → arrows, etc.). The fix is one or two lines per function, matching the already-correct `snap_left`/`snap_right` pattern in `altevra-memory/src/search.rs`. Additionally, `pending_indexing` has 51 stuck items, 2-character tokens like "AI" return no results due to an overly-aggressive filter, and the CWD-relative DB split means `recall` may read different data depending on where it is invoked. Evidence: `altevra turn-search 'ReVesta' --json` → `thread 'main' panicked at ... start byte index 531 is not a char boundary`.

---

## 4. How to test Altevra (runnable checklist)

### A. Build and Binary

```bash
# A1. Clean release build — zero warnings
cd /home/pavle/projekti/ai-tooling/altevra
cargo build --release 2>&1
# Expected: "Finished `release` profile" — no lines starting with "warning:" or "error:"

# A2. Binary version
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra --version
# Expected: altevra 0.3.0

# A3. PATH symlink resolves correctly
ls -la /home/pavle/.local/bin/altevra
/home/pavle/.local/bin/altevra --version
# Expected: symlink → target/release/altevra; version: altevra 0.3.0

# A4. Doctor — known state (2 failures before fix, 0 after)
cd /home/pavle/projekti/ai-tooling/altevra
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra doctor
# Expected BEFORE FIX: 6/8 OK, 1 WARN (settings_managed), 1 FAIL (skills_parseable: resident-agent-core.md)
# Expected AFTER FIX: 8/8 OK

# A5. All workspace tests pass
cd /home/pavle/projekti/ai-tooling/altevra
cargo test --workspace 2>&1 | grep -E '^test result'
# Expected: every line "test result: ok. N passed; 0 failed;"

# A6. Skill list — confirms resident-agent-core frontmatter bug
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra skill list 2>&1
# Expected BEFORE FIX: 2 skills (altevra-agent-operations, altevra-core) — resident-agent-core absent
# Expected AFTER FIX: 3 skills

# A7. file_change_count bug documented (no test exists yet — gap confirmed)
cd /home/pavle/projekti/ai-tooling/altevra
cargo test -p altevra-db -- improvement_signals 2>&1 | tail -10
# Expected: all improvement_signals tests pass; NOTE no test asserts file_change_count>0 triggers candidacy
```

### B. Brain Daemon

```bash
# B1. Non-network tick completes without panic
timeout 60 /home/pavle/projekti/ai-tooling/altevra/target/release/altevra brain tick \
  --disabled daily_summary,research_fetcher,github_trending_fetch,feed_discovery,project_research_sweep \
  2>&1; echo "exit: $?"
# Expected: "Ran N job(s) in this tick." (N>=9), exit 0, no PANIC lines

# B2. Brain status reads correct (home-absolute) DB
ALTEVRA_DB_PATH=/home/pavle/.altevra/altevra.db \
  /home/pavle/projekti/ai-tooling/altevra/target/release/altevra brain status \
  --pid-file /home/pavle/.altevra/brain.pid 2>&1
# Expected: shows correct job counts from home DB; confirms CWD-relative bug without env override

# B3. Pending_indexing queue never drains (bug confirmation)
sqlite3 /home/pavle/.altevra/altevra.db 'SELECT status, COUNT(*) FROM pending_indexing GROUP BY status;'
# Expected BEFORE FIX: "pending|51" — never decreases across brain tick runs

# B4. Insight cards contain LLM refusal text (SI-14 bug)
sqlite3 /home/pavle/.altevra/altevra.db \
  "SELECT COUNT(*), title FROM insight_cards WHERE title LIKE '%no recent activity%' OR title LIKE '%cannot identify%' GROUP BY title;"
# Expected BEFORE FIX: >=1 row with refusal text; AFTER FIX: 0 rows

# B5. Brain unit tests all pass
cd /home/pavle/projekti/ai-tooling/altevra
cargo test -p altevra-brain 2>&1 | tail -10
# Expected: "test result: ok. 50 passed; 0 failed;"

# B6. Daemon alive check matches brain status
kill -0 $(cat /home/pavle/.altevra/brain.pid 2>/dev/null) 2>/dev/null && echo 'daemon alive' || echo 'daemon dead'
ALTEVRA_DB_PATH=/home/pavle/.altevra/altevra.db \
  /home/pavle/projekti/ai-tooling/altevra/target/release/altevra brain status \
  --pid-file /home/pavle/.altevra/brain.pid 2>&1 | grep 'Brain status'
# Expected: both lines agree on running/stopped state
```

### C. Observer Insights

```bash
# C1. Events table is empty — core data gap
sqlite3 /home/pavle/.altevra/altevra.db \
  'SELECT COUNT(*) FROM events; SELECT COUNT(*) FROM improvement_signals; SELECT kind, source_mode, status FROM proposals;'
# Expected: events=0, improvement_signals=9, 1 proposal row (source_mode=self_improve)

# C2. Observer scan returns 0 despite 1157 turns
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra observer scan --json 2>&1
# Expected: {"count":0,"insights":[],"since":"7d"} — bug reproduced

# C3. Flat events files do not exist
ls -la /home/pavle/.altevra/events/
# Expected: directory is empty — no events.jsonl, no updates.jsonl, no file_changes.jsonl

# C4. detect_patterns works when given input (logic is correct, source is broken)
cd /home/pavle/projekti/ai-tooling/altevra
cargo test -p altevra-core -- observer::tests::drift_three_same_slug_emits_one_insight 2>&1 | tail -5
# Expected: "test result: ok. 1 passed"

# C5. Integration test showing correct pipeline (fix pattern reference)
cd /home/pavle/projekti/ai-tooling/altevra
cargo test -p altevra-brain -- tests::daily_briefing_surfaces_patterns_and_contacts 2>&1 | tail -5
# Expected: "test result: ok. 1 passed"

# C6. Verify CLI works once events.jsonl exists (flat-file path is correct, just never written)
mkdir -p /tmp/altevra-test/.altevra/events
python3 -c "
import json, uuid, datetime
events=[]
for i in range(3):
    events.append(json.dumps({'id':str(uuid.uuid4()),'event_type':'skill_drift_detected','project_id':None,'actor_type':'system','actor_id':None,'source':'test','entity_type':'skill','entity_id':'altevra-core','title':'drift altevra-core','summary':None,'payload':'{}','sensitivity':'internal','created_at':(datetime.datetime.utcnow()-datetime.timedelta(hours=i+1)).strftime('%Y-%m-%dT%H:%M:%S.000Z'),'processed_at':None,'status':'processed'}))
with open('/tmp/altevra-test/.altevra/events/events.jsonl','w') as f: f.write('\n'.join(events)+'\n')
print('seeded',len(events),'events')
"
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra observer scan \
  --vault /tmp/altevra-test --json 2>&1
# Expected: {"count":1,"insights":[{"kind":"recurring_drift",...}],...}
```

### D. Hook Pipeline

```bash
# D1. session_start with explicit absolute DB — writes to canonical store
BEFORE=$(sqlite3 ~/.altevra/altevra.db 'SELECT count(*) FROM sessions;')
echo '{}' | /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  hook-handle session_start --tool claude-code --db ~/.altevra/altevra.db 2>&1
AFTER=$(sqlite3 ~/.altevra/altevra.db 'SELECT count(*) FROM sessions;')
echo "Sessions: $BEFORE -> $AFTER"
# Expected: JSON with session_id; count increments by 1

# D2. Full hook round-trip (session_start → user_prompt_submit → session_end)
echo '{}' | /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  hook-handle session_start --tool claude-code --db ~/.altevra/altevra.db
SESSION=$(cat ~/.altevra/state/current_session.txt 2>/dev/null)
echo '{"user_prompt":"test turn"}' | /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  hook-handle user_prompt_submit --tool claude-code --db ~/.altevra/altevra.db
sqlite3 ~/.altevra/altevra.db "SELECT role, content, source_tool FROM turns WHERE session_id='$SESSION';"
# Expected: both hook-handle calls exit 0; sqlite shows "user|test turn|claude-code"

# D3. FK constraint failure reproduced (relative path bug)
cat /home/pavle/projekti/ai-tooling/altevra/.altevra/state/current_session.txt 2>/dev/null
cd /home/pavle/projekti/ai-tooling/altevra
echo '{"user_prompt":"test"}' | /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  hook-handle user_prompt_submit --tool claude-code; echo $?
# Expected BEFORE FIX: exit 1, "FOREIGN KEY constraint failed" on stderr

# D4. Shadow DB created in /tmp (absolute path fix proof)
cd /tmp
echo '{}' | /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  hook-handle session_start --tool claude-code; echo $?
ls /tmp/.altevra/altevra.db 2>/dev/null || echo 'no shadow DB created'
# Expected AFTER FIX: "no shadow DB created"; BEFORE FIX: shadow DB appears

# D5. Verify installed hook commands in settings.json
python3 -c "
import json
d = json.load(open('/home/pavle/.claude/settings.json'))
for ev, entries in d.get('hooks', {}).items():
    for entry in entries:
        for h in entry.get('hooks', []):
            print(h.get('command',''))
"
# Expected AFTER FIX: each command contains '--db /home/pavle/.altevra/altevra.db'; no $ALTEVRA_PROJECT

# D6. SI-6 — altevra repo should NOT have wired hooks
cat /home/pavle/projekti/ai-tooling/altevra/.claude/settings.json | \
  python3 -c "import sys,json; d=json.load(sys.stdin); print('hooks present:', bool(d.get('hooks')))"
# Expected AFTER FIX: "hooks present: False"
```

### E. Import Pipeline

```bash
# E1. Hermes import idempotency
BEFORE=$(sqlite3 /home/pavle/.altevra/altevra.db 'SELECT COUNT(*) FROM turns;')
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra import --tool hermes 2>&1
AFTER=$(sqlite3 /home/pavle/.altevra/altevra.db 'SELECT COUNT(*) FROM turns;')
echo "turns before=$BEFORE after=$AFTER (delta should be 0)"
# Expected: "imported 0 new sessions, 0 new turns, 0 new signals; skipped 9"

# E2. Hermes dry-run creates no DB
rm -f /tmp/test-dry.db
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra import \
  --tool hermes --db /tmp/test-dry.db --dry-run 2>&1
[ -f /tmp/test-dry.db ] && echo 'FAIL: DB created' || echo 'PASS: no DB created'
# Expected: "[dry-run] would process 9 sessions" and "PASS: no DB created"

# E3. Hermes --since filter
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra import \
  --tool hermes --since 2026-05-19 --dry-run 2>&1
# Expected: "[dry-run] would process 2 sessions (7 filtered out by --since)"

# E4. Codex field-name mismatch documented
python3 -c "
import json
lines = [json.loads(l) for l in open('/home/pavle/.codex/history.jsonl') if l.strip()]
print('fields:', sorted(set(k for l in lines for k in l.keys())))
print('has thread_id:', any('thread_id' in l for l in lines))
print('has session_id:', any('session_id' in l for l in lines))
"
# Expected: fields: ['session_id','text','ts'], has thread_id: False, has session_id: True

# E5. Codex state DB has no project column and created_at is INTEGER
sqlite3 /home/pavle/.codex/state_5.sqlite '.schema threads' 2>&1 | grep -E 'project|created_at'
# Expected: "created_at INTEGER NOT NULL" — no project column line

# E6. Claude-code import not wired
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra import --tool claude-code 2>&1
# Expected: "Error: unsupported --tool claude-code"

# E7. Cursor import dry-run is read-only
sha256sum /home/pavle/.cursor/ai-tracking/ai-code-tracking.db
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra cursor import 2>&1
sha256sum /home/pavle/.cursor/ai-tracking/ai-code-tracking.db
# Expected: identical SHA-256 hashes; "(no rows written — re-run with --apply to persist)"

# E8. Import unit tests
cd /home/pavle/projekti/ai-tooling/altevra
timeout 120 cargo test -p altevra-cli -- import 2>&1 | tail -10
# Expected: "test result: ok. 7 passed; 0 failed;"
```

### F. MCP Server

```bash
# F1. Server starts, handshake succeeds, 40 tools registered
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
' | timeout 20 /home/pavle/projekti/ai-tooling/altevra/target/release/altevra serve \
  --vault /home/pavle 2>/dev/null | python3 -c "
import sys,json
lines=[json.loads(l) for l in sys.stdin if l.strip()]
print('init_ok:',lines[0]['result']['protocolVersion'])
print('tool_count:',len(lines[1]['result']['tools']))
"
# Expected: init_ok: 2024-11-05  tool_count: 40

# F2. list_skills with correct vault returns 3 skills
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_skills","arguments":{"vault":"/home/pavle/projekti/ai-tooling/altevra"}}}
' | timeout 20 /home/pavle/projekti/ai-tooling/altevra/target/release/altevra serve \
  --vault /home/pavle/projekti/ai-tooling/altevra 2>/dev/null | python3 -c "
import sys,json
[print('skills:',d['result']['structuredContent']['count']) for l in sys.stdin for d in [json.loads(l)] if d.get('id')==2]
"
# Expected AFTER FIX: skills: 3 (currently 0 because --vault /home/pavle points to empty dir)

# F3. recall_window returns sessions from home DB with ALTEVRA_DB_PATH set
ALTEVRA_DB_PATH=/home/pavle/.altevra/altevra.db \
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"recall_window","arguments":{"window":"last_week"}}}
' | timeout 20 /home/pavle/projekti/ai-tooling/altevra/target/release/altevra serve \
  --vault /home/pavle 2>/dev/null | python3 -c "
import sys,json
[print('count:',d['result']['structuredContent']['count']) for l in sys.stdin for d in [json.loads(l)] if d.get('id')==2]
"
# Expected: count >= 3 (includes today's claude-code sessions from home DB)

# F4. get_capabilities returns correct tool count
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_capabilities","arguments":{}}}
' | timeout 10 /home/pavle/projekti/ai-tooling/altevra/target/release/altevra serve \
  --vault /home/pavle 2>/dev/null | python3 -c "
import sys,json
[print('mcp_tools:',d['result']['structuredContent']['mcp_tools']) for l in sys.stdin for d in [json.loads(l)] if d.get('id')==2]
"
# Expected AFTER FIX: mcp_tools: 40  (currently returns 22)

# F5. search_memory returns Obsidian chunks (vault=/home/pavle is correct for this tool)
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_memory","arguments":{"query":"ReVesta GTM","vault":"/home/pavle","limit":3}}}
' | timeout 30 /home/pavle/projekti/ai-tooling/altevra/target/release/altevra serve \
  --vault /home/pavle 2>/dev/null | python3 -c "
import sys,json
[print('total_chunks:',d['result']['structuredContent']['total_chunks'],'hits:',len(d['result']['structuredContent']['hits'])) for l in sys.stdin for d in [json.loads(l)] if d.get('id')==2]
"
# Expected: total_chunks > 100000  hits: 3

# F6. Full MCP unit test suite
cd /home/pavle/projekti/ai-tooling/altevra
timeout 120 cargo test -p altevra-mcp --quiet 2>&1 | tail -5
# Expected: "test result: ok. 56 passed; 0 failed; 0 ignored"
```

### G. Memory, Search, Recall

```bash
# G1. Reproduce UTF-8 panic in turn-search --json (primary bug)
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra turn-search 'ReVesta' --json; echo "exit: $?"
# Expected BEFORE FIX: panic "start byte index 531 is not a char boundary", exit 101
# Expected AFTER FIX: valid JSON with count>=3, exit 0

# G2. Reproduce UTF-8 panic in recall
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra recall 'ReVesta'; echo "exit: $?"
# Expected BEFORE FIX: panic "start byte index 531 is not a char boundary", exit 101
# Expected AFTER FIX: provenance breadcrumbs with hits, exit 0

# G3. Plain turn-search (non-JSON) works for ASCII-safe queries
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra turn-search 'ReVesta' 2>&1
# Expected: "Top N matches for ReVesta:" with >=3 scored results, no panic

# G4. Two-character token filter bug
/home/pavle/projekti/ai-tooling/altevra/target/release/altevra turn-search 'AI'
# Expected BEFORE FIX: "No turns match 'AI'"
# Expected AFTER FIX: results (AI appears in many turns)

# G5. DB path split — different counts from different CWDs
cd ~ && /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  turn-search 'hermes' 2>&1 | head -2
cd /home/pavle/projekti/ai-tooling/altevra && ./target/release/altevra \
  turn-search 'hermes' 2>&1 | head -2
# Expected BEFORE FIX: different result counts (different DBs)
# Expected AFTER FIX: same counts from both CWDs

# G6. Pending_indexing stuck queue
sqlite3 /home/pavle/.altevra/altevra.db \
  'SELECT path, status FROM pending_indexing LIMIT 5;'
sqlite3 /home/pavle/.altevra/altevra.db \
  'SELECT status, fail_count, COUNT(*) FROM pending_indexing GROUP BY status, fail_count;'
# Expected: 51 rows all status=pending, fail_count=0 — never drains

# G7. Memory search (BM25 in-process — already char-boundary safe)
cd /home/pavle && /home/pavle/projekti/ai-tooling/altevra/target/release/altevra \
  memory search 'voice gateway' 2>&1 | head -5
# Expected: "Results for: voice gateway (N chunks indexed)" N>0, at least one hit, exit 0
```

---

## 5. Brief corrections

The session brief (`ALTEVRA-NEXT-SESSION-BRIEF.md`) contained the following verified factual errors:

| Error in brief | Verified reality |
|----------------|-----------------|
| References `turns.tool` column | Column is named `source_tool` in the actual schema — `turns.tool` does not exist |
| Claims "all turns captured to `~/.altevra/altevra.db`" | Zero claude-code turns are in the canonical home DB; they are in the project-local shadow DB at `repo/.altevra/altevra.db` due to the relative path bug |
| Claims "brain daemon is healthy and running" without qualification | The daemon runs but writes to `~/.altevra/altevra.db` while CLI tools started from the repo dir read from a different file; `brain status` from repo dir shows stale/wrong data |
| Claims `altevra embed status` shows pending queue | `embed status` reads from CWD-relative DB (shows 0 for all counters); the real `pending_indexing` table with 51 rows is in `~/.altevra/altevra.db` which `embed status` never reads |
| Claims observer scan "may return limited insights" | Observer scan returns exactly 0 insights universally — the events table is empty and the flat JSONL files do not exist; there is no circumstance where it returns non-zero without a code fix |
| Claims `mcp_tools: 22` is accurate | MCP server registers 40 tools; the value 22 is a stale hardcoded constant in `tools_capabilities.rs:58` |
| Claims "Codex sessions imported" or "import pipeline supports codex" | `altevra import --tool codex` returns an unsupported error; 0 Codex sessions are in any DB |
| Claims `resident-agent-core` skill is registered | The skill is silently excluded due to wrong frontmatter (`id` instead of `slug`, missing `title`); `altevra skill list` shows 2 skills, not 3 |
| Claims `CURRENT_SESSION_FILE` is at a stable path | The path `.altevra/state/current_session.txt` is relative; two different files exist at different CWDs (hermes session in `~/.altevra/state/`, claude-code session in `repo/.altevra/state/`) |
| Claims the brain job `run_event_classifier` reads events from vault_path | Both `run_event_classifier` and `run_observer_scan` use hardcoded `std::path::Path::new(".altevra/events/...")` ignoring `ctx.vault_path` |

---

## 6. Recommended fix order

Ordered by leverage (fixes that unblock multiple subsystems first):

1. **`crates/altevra-core/src/paths.rs:15` — change `DEFAULT_DB_PATH` to absolute `$HOME`-relative path** *(safe to auto-apply)*
   One-line change: `dirs::home_dir().unwrap_or_default().join(".altevra/altevra.db")`. This is the single root cause behind the split-brain DB in hooks, brain, MCP, and memory/recall. All shadow DBs stop being created. Also change the PID file default to `$HOME/.altevra/brain.pid`.

2. **`crates/altevra-cli/src/commands/hook_handle.rs:27` — change `CURRENT_SESSION_FILE` to absolute path** *(safe to auto-apply)*
   Compute from `std::env::var("HOME")` at runtime. Eliminates cross-directory FK failures and ensures session pointer is visible to all hook invocations regardless of CWD.

3. **Regenerate hook commands in `~/.claude/settings.json`** *(requires Pavle authorization — modifies settings.json)*
   After fixes 1+2, run `altevra connect --tool claude-code` (or re-run `altevra install-hooks --global`) to bake `--db /home/pavle/.altevra/altevra.db` into the installed hook commands. Also removes `$ALTEVRA_PROJECT` and replaces with `$CLAUDE_PROJECT_DIR`-based project detection.

4. **`crates/altevra-cli/src/commands/turn_search.rs:92-96` and `crates/altevra-cli/src/commands/recall.rs:431-436` — fix UTF-8 snippet panic** *(safe to auto-apply)*
   Apply `is_char_boundary` snapping before byte-slicing. Unblocks `recall` and `turn-search --json` for all real-world queries. Copy the `snap_left`/`snap_right` pattern from `altevra-memory/src/search.rs:218-228`.

5. **`crates/altevra-brain/src/jobs.rs:134-136, 205` — replace hardcoded relative event paths with `ctx.vault_path`** *(safe to auto-apply)*
   Makes `run_event_classifier` and `run_observer_scan` use the correct paths regardless of CWD. Required before the observer events pipeline can work.

6. **Populate `events` table from `hook_handle.rs`** *(safe to auto-apply)*
   Add `EventsRepository::insert()` calls in `session_start` and `session_end` handlers with `EventType::SessionStarted` / `EventType::SessionEnded`. This unblocks `detect_patterns`, the observer scan CLI, the MCP observer tool, and the self-improve observer-insight-to-proposal path.

7. **`06-skills/resident-agent-core.md` — fix frontmatter (`slug` + `title`)** *(safe to auto-apply)*
   Replace `id: skill_resident_agent_core` with `slug: resident-agent-core`, add `title: Altevra Resident Agent Core`. Clears the `doctor` FAIL and makes the skill appear in `altevra skill list`.

8. **`crates/altevra-cli/src/commands/analyze/parsers/codex.rs` — fix field name aliases and state DB query** *(safe to auto-apply)*
   Add `alias="session_id"` to `thread_id`, `alias="ts"` on new `Option<i64>` field, `alias="text"` on content. Change state DB query from `SELECT id, title, project, created_at` to `SELECT id, title, cwd, created_at` with integer epoch parsing. Prerequisite for wiring Codex import.

9. **`crates/altevra-cli/src/commands/import.rs:89-99` — wire claude-code and codex match arms** *(safe to auto-apply after fix 8)*
   Add `"claude-code"` and `"codex"` arms. Imports 875 Claude Code JSONL files and 285 Codex history entries into the canonical DB for the first time.

10. **`~/.claude.json` MCP config — change `--vault` to project dir, add `ALTEVRA_DB_PATH` env** *(requires Pavle authorization — modifies global MCP config)*
    Change `args` from `['serve','--vault','/home/pavle']` to `['serve','--vault','/home/pavle/projekti/ai-tooling/altevra']`. Add `"env": {"ALTEVRA_DB_PATH": "/home/pavle/.altevra/altevra.db"}`. Note: `search_memory` tool accepts a `vault` argument at call time so Obsidian coverage is unaffected.

11. **`crates/altevra-brain/src/jobs.rs:225-236` — fix `vault_indexer` ON CONFLICT + wire embed consumer** *(safe to auto-apply the ON CONFLICT fix; wiring embed as a brain job is a larger change)*
    Short-term: change `ON CONFLICT ... DO UPDATE SET status = 'pending'` to only reset `failed` rows. Long-term: add `JobKind::EmbedWorker` or document the `altevra embed run` companion requirement with shared `--db` path.

12. **`crates/altevra-mcp/src/tools_sessions.rs` — fix dead `rt` variable and `block_on` pattern** *(safe to auto-apply)*
    Adopt `std::thread::spawn` + `tokio::runtime::Builder::new_current_thread().block_on()` pattern from `tools_capabilities.rs`. Eliminates latent deadlock risk.

13. **`crates/altevra-mcp/src/tools_tasks.rs`, `tools_capabilities.rs`, `tools_updates.rs` — replace CWD-relative state paths with vault-anchored absolute paths** *(safe to auto-apply)*
    Pass `vault_path` into handlers; replace `const` string paths with `vault_path.join(".altevra/state/...")`.

14. **`crates/altevra-mcp/src/tools_capabilities.rs:58` — replace hardcoded `mcp_tools: 22` with dynamic count** *(safe to auto-apply)*
    Add `pub fn tool_count() -> usize` and use it in `get_capabilities` response.

15. **`crates/altevra-brain/src/jobs.rs:280-285` — improve SI-14 guard to detect LLM refusals** *(safe to auto-apply)*
    Check for refusal phrases or, better, inject actual event context into the synthesizer prompt before calling the LLM.

16. **`crates/altevra-adapters/src/claude_code.rs` — add SI-6 self-skip gate** *(safe to auto-apply)*
    Import `is_altevra_workspace()` check from `install_hooks.rs:130` into `ClaudeCodeAdapter::install()` to prevent the altevra repo from wiring hooks on itself.

17. **`crates/altevra-cli/src/commands/import.rs:245` — `let mut file_change_count`** *(safe to auto-apply)*
    One-character fix; add increment logic in the turns loop. Low urgency but silently wrong.

18. **`crates/altevra-cli/src/commands/turn_search.rs:83-85` — token filter `>= 2` instead of `> 2`** *(safe to auto-apply)*
    Allows 2-character tokens (AI, DB, UI, etc.) in turn-search queries.

19. **`crates/altevra-cli/src/commands/brain.rs:59` — expand default `--disabled` for `brain tick`** *(safe to auto-apply)*
    Add `research_fetcher,github_trending_fetch,feed_discovery,project_research_sweep` to the default disabled list; separately fix broken RSS feed URLs in `feeds.yaml`.