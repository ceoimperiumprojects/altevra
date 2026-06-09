# Altevra "Make It Alive" — Assessment (Opus 4.8)

**Date:** 2026-06-09
**Branch:** `s0-foundation` (no commits ahead of master — all changes uncommitted working tree)
**Canonical DB:** `/home/pavle/.altevra/altevra.db` (migration 034, live and actively growing)
**Method:** First-pass findings + adversarial verification across 5 areas: real-data behavior, claude-code hooks, S0 completeness, tool-register/capabilities, Hivemind adoption.

---

## 1. Executive truth — what ACTUALLY works on real data right now

The three critical S0 bug fixes are **verified working on the real ~1800-turn production DB**: no UTF-8 panic on `recall`/`turn-search` (A1), the default DB path is `$HOME`-anchored not CWD-relative (A2), and the `working_dir` migration 034 is applied (A3) — all confirmed by independent re-runs, not just the first agent's word. Live Claude Code capture is genuinely working end-to-end: this very assessment session (`00900bc4`) was recorded into the canonical DB in real time, growing from ~159 to 400+ turns across measurements, all tagged `source_tool=claude-code` with the correct `working_dir` — so hooks ARE firing in production, a fact the first agents underplayed. `recall`, `turn-search`, `session list`, and `doctor` all return coherent real data. **What does NOT work yet:** observer scan returns zero patterns because it is data-starved (events table has 1-3 shallow `session_started` rows, `hook_runs` is empty); `memory search` and `context` default their vault to the CWD (the altevra repo) instead of Obsidian because the CLI never calls `Config::load()` — a confirmed code-level bug, not just config drift; and `get_capabilities` always returns a hardcoded fallback because `~/.altevra/state/capabilities.json` does not exist. Two first-pass claims were corrected by the verifier: the "DailySummary produces stub output" claim is **wrong** (it produces real content, but writes to `/home/pavle/10-insights/` not Obsidian due to a vault-path misconfig), and the static turn counts (1205/1598/1159) were snapshot artifacts of a live, growing DB — not fabrications. Bottom line: the **recorder half** of Altevra is alive and verified; the **brain/proactive half** (context injection, tool register, skill factory, briefings) is absent or unwired.

---

## 2. Real-data + hook reality

### Live Claude Code capture — WORKS end-to-end (verified)
- Session `00900bc4-ffff-43df-8be1-bb639e9c0f0e`, `tool=claude-code`, `working_dir=/home/pavle/projekti/ai-tooling/altevra`, started `2026-06-09T10:13:21Z`.
- Turn count grew live during both assessments (159 → 291 → 380 → 400+) — turns are captured in real time. All carry `source_tool=claude-code`.
- All 5 hook points wired in `~/.claude/settings.json`: SessionStart, Stop, UserPromptSubmit, PreToolUse, PostToolUse. Binary resolves via `~/.local/bin/altevra` → `target/release/altevra`.
- A2 confirmed live: per-CWD pointer file `session-claude-code-<cwd_hash>.txt`; old global `current_session.txt` deleted (git shows `D`).
- FK error handling correct: `record_turn` FK failure logs a stderr warning and returns `Ok(())` — a hook never exits non-zero and never blocks Claude Code.

### recall / turn-search — WORK on real data (verified)
- `altevra recall ReVesta` → 10 coherent hits; `turn-search Hermes --json` → valid JSON, scores ~2.3–3.3. No panic on the multi-byte (Serbian Cyrillic + arrows) corpus. A1 regression tests present in both `recall.rs` and `turn_search.rs`.
- `turns.source_tool` correctly populated and indexed: claude-code/claude-code, hermes/hermes, plus a small acceptable cross-tool edge (claude-code/hermes: 5).

### observer — STRUCTURALLY correct but DATA-STARVED (verified)
- `altevra observer scan` → "No patterns detected in last 7d." The A4 SQLite-reader rewrite is in place (`observer.rs` queries `EventsRepository` + `detect_patterns`, `jobs.rs run_observer_scan` persists insights). The logic is correct; it has nothing to chew on. `events` table = 1–3 rows (all `session_started`), `hook_runs` = 0. Detectors need `SkillDriftDetected`, hook-failure, and task events that have never been emitted. No backfill from the 1205+ turn corpus exists.

### Verifier refutations / corrections (call-outs)
- **REFUTED — "DailySummary is a stub":** `jobs.rs:590-745` is a real implementation (pattern detection + relationship contact-gap + stale-decision surfacing + optional LLM prose). A real file exists at `/home/pavle/10-insights/daily-2026-06-07.md`. The actual gap is **delivery**: brain runs with `--vault /home/pavle` (per `ps aux`) so summaries land in `/home/pavle/10-insights/` not `~/Obsidian/Imperium/Daily/`. This is a config fix, not a feature build.
- **REFUTED — static turn counts:** 1205/1598/1159 were all snapshot artifacts; the DB is live and grows during measurement. Any static count claim is timestamp-relative, not a bug.
- **DETECTED — test isolation leak:** 2 `test-agent` sessions (0 turns) appeared in the canonical DB at `10:19:20` during cargo-test runs. Benign (0 turns) but indicates some test path creates a session without `ALTEVRA_DB_PATH` isolation. Needs audit.
- **CORRECTED — migration check method:** the real migration anchor is `SELECT version FROM _sqlx_migrations` (→ 34), NOT `PRAGMA user_version` (returns 0). Any future audit using `user_version` gets a false negative.

### Secondary real-data gaps (verified)
- `memory search` / `context` default `--vault` to `.` (CWD) — code-level: `context.rs` line 33 hardcodes `default_value="."` and neither command calls `Config::load()`, so `config.toml [vault].path` is ignored. With explicit `--vault ~/Obsidian/Imperium` results are rich (13,427 chunks, real GTM/decisions); without it, repo-only (≈1,397 chunks).
- All hermes sessions (10/15) have `project_name=NULL` and `working_dir=NULL` — recall always shows `?` for project.
- `memory search` syntax is **positional** (`<QUERY>`), not `--query` — old docs/briefs using `--query` will fail.

---

## 3. S0 completeness

S0 is **~80% complete**. A1–A4 + hook robustness are implemented and pass the full **953 workspace tests** (independently re-run and confirmed). The single blocking gap is **A5 `altevra db unify`**, which has **zero implementation**.

| S0 task | Status | Tested? | What remains |
|---|---|---|---|
| **A1** UTF-8 snippet panic fix (recall + turn_search) | done | yes (regression tests both files + live smoke) | nothing |
| **A2** Absolute-path foundation (`$HOME`-anchored DB + per-CWD session pointer) | done | yes (5 paths.rs unit tests + live) | nothing |
| **A3** `working_dir` migration 034 (schema + SessionRow/TurnRow + binds + backfill) | done | partial | **no non-null roundtrip test** (all fixtures use `working_dir: None`) |
| **A4** Observer SQLite reader rewrite (CLI + MCP + `run_observer_scan`) | done | yes (seeded-events test, isolated tempdir) | data backfill so it can fire (see §5); not a code gap |
| Hook robustness (FK error → warning, never exit non-zero) | done | code-verified | nothing |
| **A5** `altevra db unify` (lock, WAL-safe backup, dedup, FK remap, quarantine, `--dry-run`) | **missing** | no | **build the entire command** — S0 gate blocker |
| S0 gate test: seeded multi-DB fixture → exact union counts + zero shadow DBs + FK integrity | **missing** | no | write the hermetic fixture test |
| Regenerate installed hooks (absolute `--db`, drop `$ALTEVRA_PROJECT`) | partial | n/a | hooks work via `default_db_path()` but lack belt-and-suspenders `--db`; `$ALTEVRA_PROJECT` unset → all `project_name=NULL` |
| `pending_indexing` dead-end | partial (mitigated) | no | ON CONFLICT CASE prevents unbounded growth; ~51-53 rows stuck, `embedder_queue`=0 — wire the drain or remove the path; no drain test |
| S0.5 import arms (claude-code + codex) | not in scope of this audit / unverified-wired | — | 875 claude-code JSONL + 285 codex entries unimported; apply Hivemind watermark-oldest-mined fix before bulk import |

**Concrete A5 blocker:** a shadow DB at `/home/pavle/projekti/ai-tooling/altevra/.altevra/altevra.db` holds **1101 turns at migration 033** (no `working_dir` column) — siloed from the canonical DB until `db unify` exists. `altevra db --help` → "unrecognized subcommand db"; no `db.rs`, no `unify`/`DbUnify` anywhere in crates.

**What remains to finish S0:** (1) implement `altevra db unify` per the PLAN.md §S0.6 spec — maintenance lock (refuse if `brain.pid` alive; hooks spool to redacted `0600` quarantine, batch writers refuse non-fatally), WAL+`-wal`/`-shm` backup to `~/.altevra/backups/<ts>/`, exclusive SQLite lock, one-transaction merge with full dedup (NULL-external_id live sessions match only on session-id OR full ordered turn-sequence hash; `(session_id,turn_idx,role,content_hash,tool_calls_hash,file_changes_hash)` for turns; divergent collisions → `turns_quarantine`, never overwrite), deterministic FK remap, quarantine-not-delete shadow DBs, `--dry-run` with before/after counts + conflict report; (2) the hermetic S0 gate test; (3) `working_dir` non-null roundtrip test; (4) regenerate installed hooks; (5) resolve `pending_indexing`. Do **not** redo A1–A4 — they are verified done.

---

## 4. Tool Register / Capabilities layer

### What exists (migration 023) — solid skeleton, zero data
Four tables in the real DB, all **empty (0 rows)**:
- `adapter_dossiers` — per-**AI-agent** capability matrix (`tool_name` scoped to claude-code|codex|cursor|antigravity|hermes).
- `capability_records` — honest can/cannot/unverified ledger (T7: `supported` requires `evidence_ref`).
- `skill_proposals` — skill-factory queue (T12 dedup by `dedup_hash`).
- `capability_grants` — cross-agent grants (T9: install/execute grants require non-empty `approval_ref`).

The repository layer enforces T7/T12/T9 correctly with passing unit tests. **But** `get_capabilities` MCP reads a static `~/.altevra/state/capabilities.json` (which doesn't exist) → always returns hardcoded `{adapters:[...], mcp_tools:22}`. The bootstrap packet has **no** `available_tools` field; `build_system_prompt`'s `PromptInput` has **no** `available_tools` layer; `handle_session_start` pushes **no** `additionalContext`. (`available_tools` greps to zero across all crates.)

### What's needed: a distinct `tool_records` concept
The schema models **AI-agent adapters**, not **invocable tools**. There is no representation of phone-use, imperium-crawl, chatgpt-py, NotebookLM, etc. Pavle's rich inventory (176 skills in `~/.claude/skills/`, ~255 lines of can/cannot YAML in `~/.imperium/capabilities/`) is entirely outside Altevra with no import path.

### Concrete seed inventory (15 priority tools)
| name | kind | how invoked | status |
|---|---|---|---|
| imperium-crawl | cli | `imperium-crawl <cmd>` / browser-automation skill | can |
| chatgpt-py | cli+playwright | `chatgpt` / `/chatgpt-py` (DALL-E 3, file analysis) | can |
| notebooklm | python-api | `notebooklm` / `/notebooklm` (podcast, summary, briefing) | can |
| phone-use | adb+ssh | `$PF <cmd>` via ADB WiFi / `/phone-use` | can |
| browser-automation | skill+cli | `/browser-automation` → imperium-crawl interact | can |
| computer-use | cli | `cu <cmd>` (X11: screenshot/click/type/OCR) | can |
| transcribe | cli | faster-whisper + yt-dlp / `/transcribe` | can |
| graphify | skill+python | `/graphify <path>` → knowledge graph | can |
| hermes | binary | `~/.local/bin/hermes` (command center) | can |
| codex | binary | `~/.npm-global/bin/codex` (big-context coding) | can |
| cursor | binary | `~/.local/bin/cursor` (AI coding) | can |
| imperium-cloud | api-server | HTTP to local PM2 / `/imperium-cloud` (17+ free providers) | unverified |
| vm-deploy | skill | `/vm-deploy` (Oracle Cloud VM) | can |
| vm-up | skill | `/vm-up` (VM health) | can |
| content-pipeline | skill | `/content-pipeline` (social content) | can |

### Design to make it real
1. **Migration 035 `tool_records`** — `id, name UNIQUE, kind (skill|cli|python-api|mcp-server|web-service|adb), display_name, description, invocation JSON, can_do JSON, cannot_do JSON, unverified JSON, requires_session JSON, status, last_verified_at, categories JSON`. Keep AI-agent adapters and invocable tools in **separate** tables.
2. **`ToolRecordsRepository`** — upsert-by-name, list-by-kind, list-by-status, get-by-name; register in `repositories/mod.rs`.
3. **CLI** — `altevra tool seed` (idempotent upserts of the 15-tool `SEED_TOOLS` const), `altevra tool list/register`; `altevra capability list/record`; `altevra capability seed` to load `adapter_dossiers` from `~/.imperium/capabilities/{claude,hermes}.yaml`.
4. **MCP** — `get_capabilities` queries DB (three keys: `tools`, `adapter_dossiers`, `capability_records`), optional `actor` filter.
5. **Bootstrap + prompt** — add `available_tools: Vec<ToolSummary>` to `AgentBootstrapPacket` and a tool-register layer in `build_system_prompt` (between skills and output protocol).
6. **SessionStart injection** — `handle_session_start` emits `additionalContext` with a compact `=== ALTEVRA TOOL REGISTER ===` block for Claude/Cursor/Hermes, **NOT** Codex (Codex's `additionalContext` is user-visible / clobbers the TUI — Hivemind lesson). `render_tool_register_block()` in altevra-core, errors degrade to empty string.
7. **`altevra context` CLI** — pull-based fallback (mirrors `hivemind context`) for hook-less tools.
8. **S6 proof connector** — wire Imperium Crawl via `CapabilityGrantsRepository.create_pending()` (read grade auto-grants, no `approval_ref`), then a real invocation through the grant model.

---

## 5. "Make Altevra alive" build list (dependency-ordered, effort-sized)

The arc: **finish S0 → expose tool register/capabilities at session start → SessionStart context injection → skill factory (SkillOpt port, trust-ladder gated) → proactive briefing + relevance gate → personal brain.** Altevra has a major head-start: a complete 7-stage self-improve forward loop, firewall/trust ladder, improvement_signals, skill-factory-proposer mode, 40 MCP tools, and a real DailySummary implementation — most of the plumbing exists; the gaps are wiring and delivery.

### P0 — Finish S0 (~2–3 days) — BLOCKING for data integrity
- Implement `altevra db unify --dry-run` per §3 spec; add the hermetic gate test; add `working_dir` non-null roundtrip test; regenerate installed hooks (absolute `--db`, drop `$ALTEVRA_PROJECT`); fix `pending_indexing` drain.
- **Config fix (cheap, high value):** make `memory search` + `context` read `config.toml [vault].path` (or `ALTEVRA_VAULT`) as the `--vault` fallback (3-line fix each), and point the brain's default vault at `~/Obsidian/Imperium/` so DailySummary lands in the Daily folder.
- Audit/close the test-isolation leak (2 test-agent sessions in canonical DB).
- **Head-start:** A1–A4 done; only A5 + small fixes remain. **First slice:** `db unify --dry-run` printing union/conflict counts against the real 1101-turn shadow DB.

### P1 — Tool register / capabilities exposed at session start (~2 days, after P0)
- Migration 035 `tool_records` + `ToolRecordsRepository` + `altevra tool seed` (15-tool const) + `get_capabilities` DB-backed + bootstrap/prompt `available_tools`.
- **Head-start:** 023 schema + T7/T12/T9 repo rules already exist and are tested; just add the invocable-tool table and seed.
- **First slice:** migration 035 + `tool seed` + a DB-backed `altevra tool list` — proves the register is real before touching MCP/prompt.

### P2 — SessionStart context injection (~1 day, after P1) — single highest-value slice
- In `handle_session_start`, before the `println!`, query open goals + last 3 decisions + open proposals + tool-register summary; emit `additionalContext`. Per-tool channel matrix: full block for Claude Code, **nothing** for Codex (avoids TUI clobber). Add `altevra context` CLI for hook-less tools.
- **Head-start:** `context_packet` machinery + token-economy gating already exist; this is ~50–200 lines bolted onto an existing handler.
- **First slice:** inject goals + last 3 decisions only (skip tool register) to validate the additionalContext channel works in live Claude Code.

### P3 — Skill factory: SkillOpt port, trust-ladder gated (~3–4 days total, after S1 model routing)
- **P3a (pure Rust, no model, ~1 day):** port `skill-edits.ts` (4 deterministic edit ops + edit budget + SLOW_UPDATE region protection) and `skillopt-meta.ts` (meta-fingerprint SQLite table) — fully unit-testable, zero LLM calls. Unblocks everything else.
- **P3b (renderer C4, ~2 days, needs S1 Codex route):** `altevra skill-factory render --proposal <id>` replays raw refs via bounded packet → strong_reasoner (Codex) → frontmatter validation → stage to `docs/generated/skills/`; `--dry-run` default; refuses proposals lacking `evidence_refs`. Today triaged proposals park at status `triaged` forever ("C4 picks it up") with no consumer.
- **P3c (backward pass, ~1 day):** port `success-judge.ts` (anti-sycophancy, conservative-on-failure, cheap_worker); event-driven K-message window in `post_tool_use`; on confirmed failure → judge → bounded edit → **route to review (trust ladder), never auto-publish** (Altevra's inverted-trust head-start vs Hivemind's ungated auto-publish).
- **Head-start:** forward loop + firewall + proposals table + skill-factory-proposer mode + SI-6 self-skip gate all exist. **First slice:** P3a `skill-edits` ops as pure Rust functions with tests.

### P4 — Proactive briefing + relevance gate (~2–3 days, after P0)
- Port Hivemind's source/rule/delivery three-layer notification contract (with `userVisibleOnly` flag, atomic `O_EXCL` dedup, cadence-gating). Concrete rules: decision-staleness, relationship-cadence, resume-brief, open-proposals. Wire DailySummary delivery to the Obsidian Daily note.
- **Head-start:** DailySummary already computes real content (pattern + contact-gap + stale-decision); the missing piece is the delivery contract and correct vault path (the P0 config fix already gets summaries to Obsidian).
- **First slice:** point DailySummary at `~/Obsidian/Imperium/Daily/` and verify one real morning brief lands there.

### P5 — Personal brain layer (~1–2 days, after P0)
- Migration `personal_notes(kind, ...)` (FK-linked to 029 persons/relationships/preferences) + `NoteCommands` CLI + `~/.altevra/interests.yaml` relevance gate (enforces "no Minecraft modpack research"). Enforce `userVisibleOnly=true` for all personal/relationship/health notifications.
- **Head-start:** migration 029 already has persons/relationships/preferences; just add the generic `personal_notes(kind)` table + write path.
- **First slice:** `personal_notes` migration + `altevra note add --kind decision|learning|preference|...`.

---

## 6. Open decisions for the grill

1. **`tool_records` shape:** new dedicated table (assessment recommendation — keep invocable tools separate from AI-agent `adapter_dossiers`), or extend `adapter_dossiers` with a `kind` discriminator and one table for everything? Locking this wrong forces a second migration later.
2. **Seed source-of-truth:** is the 15-tool list a hand-curated `SEED_TOOLS` const (fast, explicit), or auto-imported from `~/.imperium/capabilities/*.yaml` + a `~/.claude/skills/` scan (DRY, but couples Altevra to external file formats)? Or both — const for priority tools, scan for the long tail?
3. **SessionStart injection aggressiveness:** how much context per session — minimal (goals + tool count) vs rich (goals + last 3 decisions + open proposals + full tool register)? And the per-tool channel matrix: confirm **Codex gets nothing** (user-visible additionalContext clobbers its TUI) while Claude/Cursor/Hermes get the full block.
4. **Skill auto-apply vs review:** keep Altevra's inverted trust ladder (route ALL SkillOpt edits to review by default, never auto-publish), or allow auto-apply for low-risk edit classes (e.g. SLOW_UPDATE-region-only edits) to reduce Pavle's review burden? Where exactly is the auto/review line?
5. **`db unify` dedup conservatism:** confirm the "quarantine on any ambiguous match, never auto-merge" stance for NULL-external_id live sessions (matches PLAN.md). Accept that some legitimate duplicates stay quarantined pending manual review rather than risk a bad auto-merge?
6. **Vault default policy:** should the CLI default `--vault` to `~/Obsidian/Imperium` globally, read `config.toml`, or require an explicit `altevra setup` step that writes the path once? The brain and the CLI must agree on one source of truth.
7. **Build order vs GTM (Đorđe directive):** Altevra is internal tooling and the active directive is "stop building, start selling, 2 paid Simple Surplus clients." How much of this 10–14 day build list runs **now** vs after a GTM sprint? Candidate carve-out: P0 (data integrity) + P2-first-slice (the "alive moment") only — defer P3–P5.
8. **Notification surface:** Obsidian Daily note only (human-canonical, per Constitution Art. 2), or also push to the agent at SessionStart, or also Hermes Telegram gateway? And confirm `userVisibleOnly=true` is mandatory for personal/relationship/health items.
9. **Observer cold-start:** backfill synthetic Event rows from the 1205+ turn corpus (one-time `system`-actor events from session boundaries / repeated file-changes / tool failures) so observer fires its first insights from history — or wait for sustained live hook emission? Backfill gives an immediate payoff but risks low-quality synthetic patterns.
10. **`hook_runs` / events emission:** what new event types must hooks emit (SkillDriftDetected, hook-failure, task events) to feed observer + the skill-factory backward pass, and at which hook points? This is the prerequisite that makes both the observer and SkillOpt loops non-starved.
