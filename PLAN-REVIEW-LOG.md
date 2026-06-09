# Plan Review Log: Altevra Flywheel Realization
Act 1 (grill) complete — plan locked with Pavle. MAX_ROUNDS=5.

## Round 1 — Codex

**Findings**

- S0 claims DB event inserts will unblock `observer scan` and MCP, but both still read `vault/.altevra/events/*.jsonl`, not SQLite (`observer.rs:59-64,191-228`, `tools_observer.rs:16-20,79-115`). Fix: rewrite CLI/MCP observer loaders to query `EventsRepository`/`UpdatesRepository`, or explicitly emit matching JSONL/update rows.

- `db unify` is unsafe as specified: it backs up/deletes live SQLite files with no quiesce step, no exclusive lock, and no mention of `-wal`/`-shm`; `create_pool` runs WAL mode (`pool.rs:53-58`). Fix: require brain/hooks/embedder stopped, take an exclusive SQLite lock, copy DB + WAL + SHM, run merge in a transaction, then rename old DBs to quarantine instead of deleting.

- The plan’s dedup key for live sessions is incomplete: `(tool, external_id)` only applies when `external_id IS NOT NULL` (`017_sessions_external_id.sql:15-19`), while live hook sessions have `external_id: None` (`hook_handle.rs:84-99`). Fix: for live sessions, preserve session IDs and dedupe by `(id)` or a computed content/time hash, then remap turn FKs deterministically.

- S0 omits regenerated installed hook configs, even though the audit says changing `default_db_path()` alone is insufficient because existing hooks can keep old command args. Fix: include `install-hooks`/adapter regeneration and verify installed commands include absolute `--db` and no `$ALTEVRA_PROJECT`.

- The plan says “No subsystem may ever resolve the DB relative to CWD,” but it only names core/defaults; MCP state paths and brain PID defaults are still relative (`brain.rs:33,41,49,61`; audit lines 35-36). Fix: add a repo-wide path audit/gate covering DB, PID, state, task, update, and MCP config paths.

- `working_dir` migration is under-scoped: adding columns requires changes to `SessionRow`, `TurnRow`, every INSERT and row mapper in `sessions.rs`, import paths, tests, and likely FTS/provenance outputs (`sessions.rs:13-60,96-113,195-220,342-358,371-395`). Fix: define the full repository/API migration patch and add roundtrip tests for session and per-turn cwd.

- The plan ignores the audit’s high import blockers for Claude Code and Codex (`ALTEVRA-DEEP-AUDIT` lines 19,27-29), yet S2/S3 depend on cross-tool raw traces. Fix: add an S0/S1 import stabilization substage for `claude-code` and `codex` parser + CLI match arms before skill factory and memory sync.

- S1 proposes `~/.altevra/llm.yaml`, but the code already uses `.altevra/config.toml`, `AltevraConfig.llm`, env overrides, and `altevra llm use`; there is no `llm test` subcommand (`config.rs:49-69`, `llm.rs:23-26`). Fix: reuse the existing config/TOML router and add a `llm test` subcommand there, not a second config file.

- S1’s “embedding dim matches stored vectors” is vague and conflicts with current embedder behavior: the CLI still defaults to Gemini or NoOp zero-dim vectors (`embed.rs:9-11,103-116,163-172`), while the plan wants `nomic-embed-text`. Fix: specify the concrete local embedding provider implementation, model dimensions, migration compatibility, and re-embed policy.

- S1 says “nothing paid in the cloud” while routing `strong_reasoner` and renderer to Codex CLI/OAuth (`PLAN.md:77-79`), which is still an external cloud route and not appropriate for high-water/private data. Fix: say “no API-key billing” and explicitly require local-private routing plus exposure gates before any Codex renderer call.

- S2 duplicates existing skill-factory storage paths: there is both `skill_proposals` in migration 023 and unified `proposals(kind='skill')` in migration 028 (`023_capability.sql:51-72`, `028_proposals.sql:6-20`). Fix: choose one canonical proposal table for the vertical slice and write adapters/migrations to bridge or retire the other.

- S2 claims “cheap_worker emits pointer-only skill_candidate signals,” but that already exists for imports via `signal_for_skill_candidate`; live hooks only enqueue `signal_for_session`, so live workflows will not become skill candidates (`import.rs:320-330`, `hook_handle.rs:118-124`, `improvement_signals.rs:241-268`). Fix: add live hook/session-end skill-candidate production with tool/file evidence counts, or state S2 only backfills imports.

- S2 install/sync promises backup-before-overwrite and git-versioned writes, but existing `altevra-skills::sync::apply_plan` writes temp+rename with no backup and no git commit (`sync.rs:247-314`). Fix: extend the actual sync/apply path with backups, checksum/drift checks, and an optional explicit commit step before claiming that invariant.

- S3 “read each tool’s memory surface (`~/.claude` memory, Hermes memory)” is too broad and risks secrets/session/config exposure under the repo rule forbidding secrets/session data reads. Fix: enumerate exact allowed files/globs, deny auth/session/cache paths, run pre-read sensitivity/secret filtering, and make ingest dry-run first.

- S4 assumes events from S0 are enough for “8 pattern detectors,” but `run_observer_scan` currently only counts update JSONL lines and does not call `detect_patterns` (`jobs.rs:202-218`). Fix: wire the brain observer job to SQLite events/update_feed and persist actual insight objects/proposals, not just line counts.

- S5 proposes a new polymorphic “one table + kind column,” but migration 029 already created separate `persons`, `relationships`, `preferences`, and `event_log_personal` tables with high-water defaults (`029_projects_personal.sql:24-82`). Fix: reconcile with existing schema instead of adding a parallel notes table; if a polymorphic table is desired, add a migration plan from the existing tables.

- S5’s gate “personal data never leaves the machine” is not testable as written; current domain policy has local-private rules (`domain_policy.rs:161-180`), but S5 does not require tracing/assertions around every LLM/embedding/render call. Fix: add tests that high-water domains resolve only to local providers and that Codex/cloud providers are denied before prompt construction.

- S6 capability registry is not future work; `capability_records`, `adapter_dossiers`, `capability_grants`, and repository guardrails already exist (`023_capability.sql:6-105`, `capability.rs:35-65,174-249`). Fix: reframe S6 as completing/using the existing registry, with specific missing commands/UI/tests, not creating it.

- The stage ordering is wrong for `pending_indexing`: audit identifies it as a dead-end, but the plan never fixes it; S1 embeddings and S4 observer/briefing will keep stale queues (`jobs.rs:220-236`, audit line 37). Fix: add an S0/S1 task to either drain `pending_indexing` into memory chunks/embedder queue or remove that queue path.

- The verification gates are too subjective and not hermetic: “Pavle judges useful” and “one real staged SKILL.md” can pass while regressions remain. Fix: add fixture-based smoke tests for unify, observer SQLite scan, local-private routing, skill render refusal, managed-block idempotency, and personal-data no-cloud denial.

- The graph claim is overstated: graph freshness matches HEAD, but the report says 54 `create_pool()` edges are inferred and need verification (`GRAPH_REPORT.md:12-15,1413-1414`), so “graph confirms” should not justify a one-fix cascade. Fix: treat graph centrality as navigation only; base dependencies on actual call-site audit.

VERDICT: REVISE
### Claude's response (final arbiter)

ACCEPTED essentially all 21 findings — verified against the code: migrations run to 033, proposals(028) + skill_proposals(023) both exist, personal tables in 029, capability registry in 023, LLM lives in [llm] of config.toml (no llm.yaml, no llm-test subcommand). Codex read the code and was right that the plan under-counted what already exists. Revisions applied to PLAN.md:
- S0: observer fix now rewrites CLI/MCP/brain READERS to query SQLite + call detect_patterns (not just insert events); added repo-wide absolute-path gate test; added installed-hook-config regeneration; full working_dir repository-layer migration (034) with roundtrip tests; db unify made WAL-safe (stop daemons, backup wal/shm, exclusive lock, txn, quarantine-not-delete) with corrected dedup for external_id=NULL live sessions + deterministic turn-FK remap; added pending_indexing drain/remove.
- NEW S0.5 import stabilization (claude-code + codex parsers/CLI arms) before S2/S3 depend on cross-tool traces.
- S1: reuse existing [llm] config.toml + 'altevra llm use' (no second config file); add real 'llm test' subcommand; concrete local nomic-embed-text 768-dim + dim-compat/re-embed policy; reworded 'no API-key-billed cloud' + exposure gate denying high-water to Codex before prompt construction.
- S2: canonical = proposals(028, kind='skill'), bridge/retire 023 skill_proposals; add live session-end skill-candidate production; extend the ACTUAL sync::apply_plan with backup/drift/git + ALTEVRA_MANAGED + never-overwrite-human.
- S3: explicit read allowlist + deny auth/session/cache + dry-run first.
- S4: wire run_observer_scan to SQLite + detect_patterns; schema-based gate (not 'Pavle judges').
- S5: reconcile with existing 029 persons/relationships/preferences (extend or migrate-from, decided at impl); routing tests for high-water-local-only.
- S6: reframed capability registry as completing existing 023, not creating.
- Verification: replaced subjective gates with fixture-based smoke tests throughout. Softened graph claim to navigation-only.

REJECTED: none. All findings were grounded and material.

## Round 2 — Codex

**Findings**

- The `DEFAULT_DB_PATH` edit is still mechanically wrong: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:35) says a `&'static str` constant becomes `dirs::home_dir().join(...)`, but [paths.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-core/src/paths.rs:12) exists specifically because clap/static defaults used to need a string. Fix: keep the const as a suffix/name or remove it, and make `default_db_path()` compute `$HOME/.altevra/altevra.db` with updated tests.

- `db unify` is safer but still not safe enough for `external_id = NULL`: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:64) says “dedup live sessions by id (and a content/time hash fallback)” without defining the fingerprint or ambiguous-conflict behavior. Fix: define the exact fingerprint `(tool, working_dir/project, started_at bucket, turn_count, first/last turn content hash)` and quarantine, never auto-merge, if fingerprints conflict or partially match.

- Turn dedup can still drop divergent data: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:65) dedups turns by `(session_id, turn_idx)`, but copied/forked shadow DBs can have the same pair with different content/tool payloads. Fix: only collapse turns when `(session_id, turn_idx, role, content_hash, tool_calls_hash, file_changes_hash)` match; otherwise preserve as a conflict report.

- S1 role routing does not match the existing router: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:106) wants `cheap_worker` local and `strong_reasoner` Codex, but current `ReasoningMode::CodexOauth` registers Codex for both cheap and strong, while `Api` has no per-role `codex_oauth` provider path. Fix: either extend `LlmConfig`/`build_router` for true per-role providers including Codex OAuth, or revise the plan to the routing the code can actually express.

- The embedding plan is still underspecified at the implementation boundary: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:108) names `nomic-embed-text`, but the live embedder CLI is Gemini/NoOp oriented and separate from `ModelRole::Embedding`. Fix: add a concrete local embedding provider abstraction/CLI path and tests proving 768-dim vectors are written/read before promising dim compatibility.

- The S0 “repo-wide absolute-path gate” says “grep” ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:33)), which will be noisy and miss generated/default-path construction. Fix: add focused unit/CLI tests around exported path functions and selected command defaults, plus an allowlisted static scan for literal `.altevra/` defaults.

- Verification gates are improved but not hermetic yet: S0.5 still imports a “real small window” from home dirs, S2 requires “one real imported workflow,” and S6 requires reboot/real Imperium Crawl ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:96), [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:156), [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:217)). Fix: split gates into hermetic fixture tests that decide stage pass/fail, and separate manual/live smoke checks that cannot block CI determinism.

- S3’s memory ingest allowlist is still not actually enumerated: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:162) says “e.g. Claude `CLAUDE.md`,” leaving the dangerous part to implementation. Fix: list exact allowed paths/globs and exact denied globs in the plan before implementation.

- S5’s gate references `altevra note add`, but the current CLI has `Capture`, not `Note`, and `rg` shows no `NoteCommands`/notes repository. Fix: explicitly add the note CLI/repository work to S5, or change the gate to use the existing `capture` path plus the 029 tables.

- Reused-schema decision for proposals is directionally right, but “bridge or retire the older `skill_proposals` — pick one” ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:130)) is still a deferred schema decision inside the implementation stage. Fix: decide in the plan: canonical `proposals(kind='skill')`, read-only migration/bridge from `skill_proposals`, then deprecate writes to 023.

- S5 similarly defers the personal schema choice to implementation time ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:189)), which is exactly where parallel-table drift can creep in. Fix: decide now whether S5 extends 029’s separate tables or introduces a polymorphic table with a one-way migration, and make the gate target that choice.

VERDICT: REVISE
### Claude's response (final arbiter)

ACCEPTED all 11 findings — all are precise implementation-boundary corrections. Revisions:
- S0.1: DEFAULT_DB_PATH is &'static str for clap defaults — keep const as suffix/remove; default_db_path() FN computes $HOME path with unit tests. Path gate = focused unit/CLI tests + allowlisted static scan, not raw grep.
- S0.6: defined exact NULL-external_id fingerprint (tool, working_dir/project, started_at-minute-bucket, turn_count, first/last turn content hash); conflict/partial-match → quarantine NEVER auto-merge. Turn dedup collapses only on full (session_id,turn_idx,role,content_hash,tool_calls_hash,file_changes_hash) match, else conflict report.
- S1: per-role routing must FIRST extend LlmConfig/build_router for per-ModelRole providers incl Codex-OAuth-for-one-role; fallback documented if too large. Concrete local EmbeddingProvider (Ollama nomic-embed-text 768d) wired to embed CLI + ModelRole::Embedding with write/read-back test before any dim claim.
- S2: DECIDED canonical = proposals(028,kind='skill'); stop writes to 023; one-way read-only bridge; deprecate 023. Gate split hermetic vs manual smoke.
- S3: enumerated exact ALLOW globs (CLAUDE.md, projects/*/memory/*.md, Obsidian Memory/*) and DENY globs (*.json/auth, .codex, .cursor, *.db, *token*, *secret*, secrets, .ssh) in the plan.
- S5: DECIDED extend 029 (persons/relationships/preferences canonical) + new personal_notes(kind) table for remaining kinds with FK links, no migration out of 029. Added NoteCommands CLI as explicit work (CLI currently has Capture not Note).
- Gates: S0.5/S2/S6 split into hermetic fixture tests (gate CI) + manual live smoke (non-blocking).

REJECTED: none.

## Round 3 — Codex

Most round-2 findings are addressed in the revised plan. The remaining material problems are narrower, but still real.

**Findings**

- `db unify` still has an unsafe auto-merge case for live `external_id = NULL` sessions: [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:72) treats identical `(tool, working_dir|project, started_at minute, turn_count, first/last content hashes)` as the same session, but two empty/short hook sessions in the same minute can collide. Fix: only auto-merge NULL-external-id sessions when `id` matches or the full ordered turn-sequence hash matches and is non-empty; quarantine empty/minimal or first-last-only matches.

- `db unify` says “Require brain/hooks/embedder stopped” but hooks are not a daemon with a PID; Claude Code can still fire `hook_handle` during the merge. Fix: add an explicit maintenance lock that `hook_handle`, brain, embedder, MCP write paths, and imports check; during unify, writes either refuse non-fatally or spool to a quarantine file for replay.

- The turn conflict rule preserves both divergent turns, but the plan does not say how to keep `(session_id, turn_idx)` unique when both are inserted into the same `turns` table, which currently has `UNIQUE(session_id, turn_idx)`. Fix: on divergent collision, create a fork/quarantine table or remap the conflicting turn into a synthetic conflict session instead of inserting both with the same key.

- S0 gate still says “absolute-path grep test” ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:101)) after the plan correctly rejected raw grep earlier. Fix: rename this gate to the allowlisted static scan + focused command-default tests described in S0.1.

- S1 has an escape hatch that weakens the core claim: “If extending the router… proves too large, fall back” ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:130)) conflicts with the stage’s promised per-role routing. Fix: make per-role routing a hard S1 gate, or explicitly move the full cheap-local/strong-Codex topology to a later stage.

- S3 allowlist still permits `~/Obsidian/Imperium/Memory/People.md` ([PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:198)), which is likely relationship/personal high-water data; “pre-read filtering” happens after the file is opened. Fix: classify People/relationship memory as local-only ingest and forbid any write-back/external sync of derived content unless explicitly reviewed.

- S5 has a stale contradiction: it decides to extend 029 at [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:229), but the risks section still says exact 029 reconciliation is deferred at [PLAN.md](/home/pavle/projekti/ai-tooling/altevra/PLAN.md:313). Fix: remove the stale open question or replace it with implementation details for the chosen `personal_notes` extension.

VERDICT: REVISE
### Claude's response (final arbiter)

ACCEPTED all 7 findings. Revisions:
- db unify NULL-external_id: auto-merge ONLY when id matches OR full ordered NON-EMPTY turn-sequence hash matches; same-minute/first-last-only/empty → quarantine.
- Added explicit maintenance lock (hook_handle/brain/embedder/MCP-write/imports check it); writers refuse non-fatally or spool to quarantine for replay during unify (hooks are not a daemon).
- Turn divergent collision now respects UNIQUE(session_id,turn_idx): route divergent turn to turns_quarantine table / synthetic conflict session, never double-insert under same key.
- S0 gate: replaced 'absolute-path grep test' with allowlisted static scan + focused command-default tests.
- S1: removed the router escape hatch — per-role routing (cheap=local, strong/render=Codex, embedding=local) is now a HARD S1 gate.
- S3: People.md / person-relationship memory reclassified LOCAL-ONLY ingest, local_private only, no write-back/external route without explicit review.
- Risks: removed stale 'deferred 029 reconciliation' — replaced with the decided extend-029 + personal_notes approach.

REJECTED: none.

## Round 4 — Codex

The round-3 items are mostly addressed. The NULL-`external_id` auto-merge rule is now conservative enough: `id` match or full non-empty ordered turn-sequence hash only is the right shape. I still see material blockers.

1. `CURRENT_SESSION_FILE` becomes more dangerous when home-anchored. A single `~/.altevra/state/current_session.txt` can mix concurrent Claude/Codex sessions across projects; current code reads/writes one pointer in [hook_handle.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-cli/src/commands/hook_handle.rs:27).
Fix: key current-session state by tool + project/cwd hash + host session identity, or pass the session id through hook env/args instead of using one global file.

2. S0 says “repo-wide absolute-path gate,” but the allowlist misses live relative state/event writers: `prompt.rs`, `updates.rs`, `journal.rs`, `context.rs`, `watch.rs`, `altevra-watcher/src/daemon.rs`, and MCP prompt/update paths still use `.altevra/...`.
Fix: make the gate enumerate every state writer/reader found by static scan, with explicit exceptions only for intentional repo-vault config paths.

3. Maintenance-lock spooling can leak raw hook payloads. PLAN.md says hooks may “spool to a quarantine file,” but does not require the existing `guard_json`/redaction path before disk.
Fix: hook spooling must redact/guard first, write atomically with `0600`, and replay through the normal guarded ingest path; non-hook batch writers can refuse.

4. S1 verification is not hermetic. `altevra llm test cheap_worker` with Ollama and `<2s` latency is a live-machine smoke, not a fixture gate, and will be flaky on the stated weak laptop.
Fix: split S1 into blocking mock/provider-contract tests plus non-blocking Ollama/manual latency smoke.

5. The reused `proposals` schema still has a bridge conflict risk. Migration 028 has `UNIQUE(dedup_hash)` globally, not per kind, so bridging legacy `skill_proposals` can collide with non-skill proposals.
Fix: prefix bridged skill hashes (`skill:<legacy_hash>`) or migrate the unique index to `(kind, dedup_hash)` and update repository tests.

6. S3 says “pre-read sensitivity + secret filtering,” but content secret scanning cannot happen before reading the file.
Fix: enforce path allow/deny before open; then immediately content-scan/redact before persist, indexing, routing, or logs.

7. S6 still says to fix `~/.claude.json`, while S3 correctly denies ingesting it as sensitive config. Editing it can read/print auth-adjacent state unless tightly scoped.
Fix: add a hermetic config fixture and a targeted redacted updater that backs up, logs no values, and only patches the MCP command fields.

8. Import idempotency still assumes non-null `external_id`. The unique index only applies when `external_id IS NOT NULL`, so parser bugs can silently create duplicates.
Fix: S0.5 gate must assert every imported Claude/Codex fixture session has a non-null external id, with null imports quarantined or hash-deduped.

VERDICT: REVISE
### Claude's response (final arbiter)

ACCEPTED all 8 findings. Revisions:
- CURRENT_SESSION_FILE: a single $HOME pointer is worse (mixes concurrent sessions) — key state by tool+hash(cwd/project)+host session id, or pass session id via hook env/args.
- Path gate now enumerates ALL state readers/writers (prompt.rs, updates.rs, journal.rs, context.rs, watch.rs, watcher daemon, MCP prompt) not just core.
- Maintenance-lock spool: hooks spool REDACTED (guard_json) first, atomic 0600, replay through guarded ingest; non-hook batch writers refuse. No raw payload on disk.
- S1 gate split: hermetic = provider-contract/mock (per-role resolution, 768d write/read, high-water-denied-Codex, insight non-stub vs mock); manual smoke = live Ollama latency (non-blocking).
- proposals bridge collision: 028 UNIQUE(dedup_hash) is global — prefix bridged hashes skill:<legacy> (or index (kind,dedup_hash)) + repo tests.
- S3 ordering clarified: path allow/deny BEFORE open, content-scan/redact BEFORE persist/index/route/log.
- S6 ~/.claude.json: distinguished from S3 (S3 denies INGEST; S6 only PATCHES MCP fields) via safe redacted updater (backup, log no values, hermetic fixture test).
- S0.5 gate asserts non-null external_id on every imported fixture; null-id quarantined/hash-deduped.

REJECTED: none.

## Round 5 — Codex (FINAL)

No material blockers remain. The round-4 fixes are present in `PLAN.md` and the prior unsafe areas are now explicitly constrained: per-session keying replaces the global pointer, `db unify` dedup is conservative, hook spooling is redacted/atomic, S1 has a hermetic mock gate, proposal dedup collisions are handled, S3 read ordering is correct, and `~/.claude.json` patching is scoped/redacted.

Two implementation cautions, but not enough to block the plan:

1. S3’s gate line says “ingest dry-run lists only allowlisted files and redacts secrets”; the body already clarifies dry-run only prints file paths, so implement this as two tests: dry-run path allow/deny, then fixture ingest redaction.
Fix: split that gate wording during implementation.

2. S6 mentions the hermetic `~/.claude.json` fixture test in the step text, but not in the formal gate list.
Fix: include that fixture test in the S6 gate checklist.

VERDICT: APPROVED
### Claude's response (final arbiter)

APPROVED on round 5. Folded in both non-blocking cautions: S3 gate split into (a) dry-run path allow/deny test + (b) fixture ingest redaction test; S6 gate now lists the ~/.claude.json redacted-updater fixture test explicitly.

## Resolution: APPROVED after 5 rounds
Plan survived 5 rounds of adversarial cross-model review. 21+11+7+8 findings accepted across rounds 1-4, all grounded in the actual code; 0 rejected. Awaiting Pavle's final sign-off before any implementation.
