# Plan: Altevra Flywheel Realization
_Locked via grill — by Claude + Pavle, 2026-06-07. Revised after Codex round 1._

## Goal

Turn Altevra from its current broken-but-promising state (5+ shadow
databases, observer returning zero, recall panicking on UTF-8, no live
skill factory, no memory sync) into the **center of Pavle's entire AI
system**: a single canonical local store that records everything (business
AND personal as equal first-class), tagged by working folder, that
auto-generates AND refines skills across every tool, syncs memory across
all tools, surfaces insights, and runs the compounding self-improvement
flywheel from VISION.md / the Skill Factory Doctrine. Codex is the "big
brain" renderer; small local models do cheap work; no API-key-billed cloud.
Built in dependency order, **reusing the substantial schema/runtime that
already exists** (migrations through 033, capability registry in 023,
personal tables in 029, unified proposals in 028, `[llm]` config in
config.toml). Each stage independently verifiable via fixture-based smoke
tests; every external side effect gated on Pavle's explicit authorization.

## Approach

7 stages (S0→S6), dependency-ordered. Each ends with a **hermetic,
fixture-based** verification gate (named tests, not subjective judgement) +
`cargo test --workspace` green. No stage starts until the prior gate passes.

### S0 — Foundation: ONE database + critical bug fixes
Root cause per audit: a CWD-relative DB path spawns shadow DBs. The graph's
`create_pool()`/`SqlitePool` centrality is **navigation only** (54 of those
edges are INFERRED per GRAPH_REPORT.md:12-15) — actual dependency claims
below are grounded in call-site audit, not graph centrality.

1. **Repo-wide absolute-path gate** (not just core). Make every state path
   `$HOME`-anchored. `DEFAULT_DB_PATH` is a `&'static str` used for clap
   static defaults (`paths.rs:12`) — do NOT assign `home_dir().join(...)` to
   it. Instead keep the const as a path *suffix/name* (or remove it) and make
   the `default_db_path()` **function** compute `$HOME/.altevra/altevra.db`,
   with updated unit tests on that function. Gate = focused unit/CLI tests
   around the exported path functions + selected command defaults, PLUS an
   allowlisted static scan for literal `.altevra/` default constructions (not
   a noisy raw grep). The scan must enumerate **every** state reader/writer,
   not just core — explicit exceptions only for intentional repo-vault config:
   - `crates/altevra-core/src/paths.rs:12` — `default_db_path()` computes `$HOME` path
   - `crates/altevra-cli/src/commands/brain.rs:33,41,49,61` PID/state defaults → `$HOME/.altevra/...`
   - MCP state/task/update/prompt paths (`tools_tasks.rs`, `tools_capabilities.rs`, `tools_updates.rs`, prompt tool)
   - other live relative writers Codex flagged: `commands/prompt.rs`,
     `commands/updates.rs`, `commands/journal.rs`, `commands/context.rs`,
     `commands/watch.rs`, `altevra-watcher/src/daemon.rs`
   - `crates/altevra-cli/src/commands/hook_handle.rs:27` `CURRENT_SESSION_FILE`:
     a single `$HOME`-anchored pointer is WORSE — it mixes concurrent
     Claude/Codex sessions across projects. **Key current-session state by
     `tool + hash(cwd/project) + host session id`** (or pass the session id
     through hook env/args), never one global pointer file.
2. Hook robustness — wrap `record_turn` FK errors as warnings; a hook must
   never exit non-zero and block the host tool (`hook_handle.rs:257-263`).
3. **Regenerate installed hook configs** — `default_db_path()` alone is
   insufficient because installed hook commands keep old args (audit
   lines 31,35-36). Re-run `install-hooks`/adapter regen; verify installed
   commands carry an absolute `--db` and no `$ALTEVRA_PROJECT`.
4. UTF-8 snippet panic — `turn_search.rs:92-96`, `recall.rs:431-436` snap to
   `is_char_boundary` (reuse `altevra-memory/src/search.rs:218-228`).
5. **`working_dir` migration 034 — fully scoped.** Add `working_dir TEXT NULL`
   to `sessions` AND `turns`, and patch every touchpoint: `SessionRow`,
   `TurnRow`, all INSERTs and row mappers (`sessions.rs:13-60,96-113,195-220,
   342-358,371-395`), import paths, FTS/provenance outputs. Capture order at
   hook time: `$CLAUDE_PROJECT_DIR` → `std::env::current_dir()` → null
   (absolute). Turn inherits session's value but records its own when cwd
   differs (Pavle's "run from ~, project elsewhere" case). Add roundtrip
   tests for session-level and per-turn cwd. Backfill existing turns from
   their session where derivable; Hermes imports stay null.
6. **`altevra db unify` — safe migration.** WAL is on (`pool.rs:53-58`):
   - **Maintenance lock** (hooks are NOT a daemon — Claude Code can fire
     `hook_handle` mid-merge): introduce an explicit lock file that
     `hook_handle`, brain, embedder, MCP write paths, and imports all check.
     While `db unify` holds it, non-hook batch writers (brain/embedder/import)
     **refuse non-fatally**; hooks (which must not block the host tool) **spool
     to a quarantine file — but redacted first**: run the existing
     `guard_json`/redaction path before disk, write atomically with `0600`,
     and replay through the normal guarded ingest path after unify (never a
     raw unredacted payload on disk). Refuse unify if the brain PID is alive.
   - Back up each discovered DB **plus its `-wal`/`-shm`** to
     `~/.altevra/backups/<ts>/`; take an exclusive SQLite lock.
   - Canonical = `~/.altevra/altevra.db`. Merge unique rows inside one
     transaction. **Dedup keys, fully specified:** the `(tool, external_id)`
     unique index is partial (`017:15-19`, only when `external_id IS NOT
     NULL`); live hook sessions have `external_id = None`
     (`hook_handle.rs:84-99`). For live (NULL-external_id) sessions, compute a
     **auto-merge ONLY when** the session `id` matches OR the **full ordered
     turn-sequence hash** matches AND is non-empty. A same-minute or
     first/last-content-hash match alone is NOT sufficient (two empty/short
     hook sessions in the same minute can collide). Empty/minimal or
     partial-only matches → **quarantine, NEVER auto-merge** (conflict report
     for Pavle). Remap turn FKs deterministically to the kept session id.
   - **Turn dedup must not drop divergent data, and must respect
     `UNIQUE(session_id, turn_idx)`:** collapse two turns only when
     `(session_id, turn_idx, role, content_hash, tool_calls_hash,
     file_changes_hash)` ALL match. If `(session_id, turn_idx)` collide but a
     hash differs (forked/copied shadow DB), do NOT insert both under the same
     key — route the divergent turn into a **`turns_quarantine` table** (or a
     synthetic conflict session) and record a conflict report. Never silently
     overwrite, never violate the unique constraint.
   - **Quarantine** (rename) old shadow DBs, do not delete, until Pavle
     confirms. Provide `--dry-run` printing before/after counts + the conflict
     report.
7. **Observer pipeline — fix the readers, not just the writers.** Inserting
   `events` rows is necessary but NOT sufficient: `observer.rs:59-64,191-228`
   and `tools_observer.rs:16-20,79-115` read `vault/.altevra/events/*.jsonl`,
   not SQLite; `jobs.rs:202-218` counts update lines and never calls
   `detect_patterns`. Rewrite the CLI loader, MCP loader, and brain
   `run_observer_scan` to query `EventsRepository`/`UpdatesRepository` and
   call `detect_patterns`, persisting real insight objects. Also fix
   `jobs.rs:134-136,205` hardcoded relative event paths → `ctx.vault_path.join(...)`.
8. **`pending_indexing` dead-end** (audit line 37) — either drain it into the
   embedder queue/`memory_chunks` or remove the path; don't leave a queue
   that grows forever and confuses S1/S4.

**Gate (fixture tests):** `db_unify` test on seeded multi-DB fixture asserts
exact union counts + zero shadow DBs creatable from any CWD + FK integrity;
`observer scan` returns ≥1 insight from seeded SQLite events; `recall
'ReVesta'` and `turn-search --json` no longer panic on multi-byte fixtures;
the allowlisted static scan + focused command-default tests (from S0.1) pass;
`cargo test --workspace` green.

### S0.5 — Import stabilization (prereq for S2/S3 cross-tool traces)
S2 (skill factory) and S3 (memory sync) depend on real claude-code + codex
traces, which the audit shows are unimported/broken (audit lines 19,27-29).
1. Fix Codex parser field aliases + `state_5.sqlite` query
   (`analyze/parsers/codex.rs:29-42,63-67`).
2. Wire `import --tool claude-code` and `import --tool codex` match arms
   (`commands/import.rs:89-99`); walk `~/.claude/projects/**/*.jsonl`.
3. Idempotent on `(tool, external_id)`; secret/PII guard before persist.

**Gate (hermetic):** fixture-based parser tests for claude-code + codex
sample files assert correct session/turn extraction, idempotency (re-run →
zero dups), and secret-guard redaction. Because the `(tool, external_id)`
unique index is partial (NULL external_id bypasses it), the gate must assert
**every imported fixture session has a non-null `external_id`**; null-id
imports are quarantined or content-hash-deduped, never inserted unguarded.
**Manual smoke (non-blocking, does not gate CI):** dry-run then real import of
a small window of Pavle's actual home-dir sessions.

### S1 — Model runtime (small-local-assist + Codex big-brain)
**Reuse the existing config**, do not add a second config file: routing lives
in `[llm]` of `.altevra/config.toml` via `AltevraConfig.llm`
(`config.rs:96-176`), set through `altevra llm use <preset>` (`llm.rs`).
1. Start Ollama; pull ONE small chat model (`qwen2.5:3b`/`llama3.2:3b`) and
   `nomic-embed-text` (768-dim) for embeddings. Weak laptop → small only.
2. **Per-role routing must be expressible by the router.** Today
   `ReasoningMode::CodexOauth` registers Codex for BOTH cheap and strong, and
   `Api` has no per-role `codex_oauth` path. So FIRST extend `LlmConfig` +
   `build_router` to select a provider **per `ModelRole`** (incl. a Codex
   OAuth provider usable for one role while another is local), with tests for
   per-role resolution. **Per-role routing (cheap=local, strong/render=Codex,
   embedding=local) is a HARD S1 gate — no escape hatch.** If it cannot land
   in S1, S1 does not pass; we do not silently degrade the topology. Then map:
   - `cheap_worker`, `local_private` → local Ollama small model
   - `embedding` → local `nomic-embed-text` (768-dim)
   - `strong_reasoner`, skill `renderer` → **Codex** (big brain), conserved, raw-ref access.
   - **Local embedding provider — concrete:** the live embedder CLI is
     Gemini/NoOp-oriented and separate from `ModelRole::Embedding`
     (`embed.rs:9-11,103-116,163-172`). Add a concrete local
     `EmbeddingProvider` (Ollama `/api/embeddings`, `nomic-embed-text`,
     768-dim) wired to BOTH the `embed` CLI and `ModelRole::Embedding`, with a
     test that writes and reads back 768-dim vectors BEFORE any dim-compat
     claim. If stored vectors differ in dim, define + run a re-embed policy;
     otherwise refuse mixed-dim search.
3. Add a real `altevra llm test <role|provider>` subcommand to the existing
   `llm` command (none exists today — `llm.rs` only has `use`).
4. Wire the 3 placeholder call sites (insight_synthesizer, leverage
   distillation, research synthesize) to role routing.
5. **Wording + safety:** "no API-key-billed cloud" (Codex via ChatGPT
   subscription is still an external route). High-water/private data must be
   **denied to Codex/any non-local route by an exposure gate before prompt
   construction** (`domain_policy.rs:161-180`).

**Gate (hermetic):** provider-contract/mock tests — per-`ModelRole`
resolution is correct; local embedding provider writes+reads back 768-dim
vectors; a high-water domain is denied a Codex/non-local route before prompt
construction; insight_synthesizer produces non-stub output against a mock
provider.
**Manual smoke (non-blocking, flaky on weak laptop):** `altevra llm test
cheap_worker` against live Ollama + latency check.

### S2 — Skill factory vertical slice
Per the Skill Factory Doctrine; reuse existing tables, don't duplicate.
1. **Canonical proposal store = `proposals` with `kind='skill'` (migration
   028). DECIDED (not deferred):** stop ALL new writes to the older
   `skill_proposals` (023); add a one-way read-only bridge (view or migration)
   that surfaces any pre-existing 023 rows as `kind='skill'` proposals;
   deprecate 023 (kept readable for back-compat, never written again).
   **Collision guard:** migration 028's `UNIQUE(dedup_hash)` is global, not
   per-kind — bridged legacy skill hashes could collide with non-skill
   proposals. Prefix bridged hashes `skill:<legacy_hash>` (or migrate the
   index to `(kind, dedup_hash)`), with repository tests proving no collision.
2. Candidate production: `signal_for_skill_candidate` already exists for
   imports (`improvement_signals.rs:241-268`); live hooks only call
   `signal_for_session` (`hook_handle.rs:118-124`). Add **session-end
   skill-candidate production with tool/file evidence counts** so live
   workflows also become candidates (or, if deferred, S2 explicitly states it
   backfills imports only — chosen: add live production, it's core to "Altevra
   as center").
3. `altevra skill-factory render --proposal <id>` — Codex renderer **replays
   raw refs** (bounded raw-replay packet to conserve tokens), dedupes vs
   existing skills, validates frontmatter + sections, secret/PII scans.
   Default `--dry-run`, staged to `docs/generated/skills/<slug>/SKILL.md`.
   Refuses any proposal lacking raw refs.
4. **Refine-existing** — renderer may also emit a patch to an existing skill.
5. **Install/sync = separate Pavle-authorized action, with real safety rails
   added to the actual code.** Today `altevra-skills::sync::apply_plan`
   (`sync.rs:247-314`) does temp+rename with **no backup, no git, no drift
   check**. Extend that path: backup target before overwrite, checksum/drift
   detection, write only inside an `ALTEVRA_MANAGED` region, **never overwrite
   human-edited content** (refuse → queue review), optional explicit git
   commit. Only then is the "versioned + reversible" invariant true.
6. MCP exposes `list_skill_candidates`, `get_skill_proposal` (visibility
   only). Never approve/apply/install via MCP (HP-1 forbidden-tool test holds).

**Gate (hermetic):** stub renderer test — refuses missing-raw-refs proposal,
produces deterministic staged draft; sync test — backup created, human edit
preserved, managed block idempotent on re-run; proposals-bridge test — 023
rows surface read-only as kind='skill'.
**Manual smoke (non-blocking):** one real staged SKILL.md from a real imported
workflow, deduped + validated.

### S3 — Memory sync hub (Altevra as center)
1. One-way **ingest** with an **explicit, enumerated allowlist** (in the plan,
   not deferred to impl):
   - **ALLOW:** `~/.claude/CLAUDE.md`, `~/.claude/projects/*/memory/*.md`
     (file-based memory), `~/Obsidian/Imperium/Memory/{Decisions,Learnings}.md`.
   - **LOCAL-ONLY ingest (never write-back, never external/cloud route for
     derived content without explicit review):**
     `~/Obsidian/Imperium/Memory/People.md` + any person/relationship memory —
     it is high-water; route through `local_private` only.
   - **DENY (hard):** `~/.claude.json`, `~/.claude/**/*.json`
     (settings/auth), `~/.codex/**`, `~/.cursor/**`, `**/*.db`, `**/*.sqlite*`,
     `**/auth*`, `**/*token*`, `**/*secret*`, `~/.imperium/secrets/**`,
     `~/.ssh/**`, any cache dirs.
   - **Ordering matters:** enforce path allow/deny *before opening* any file
     (you cannot content-scan a file you refuse to read); then, for ALLOW
     files, content-scan + redact *before* any persist, index, route, or log.
     **Dry-run first** (prints the exact file list it would read).
2. **Write-back** into each tool ONLY inside a delimited `ALTEVRA_MANAGED`
   block, backup-before-write, reversible, human content never touched.
3. Conflict rule: human edits win; Altevra re-syncs around them.

**Gate (hermetic):** two separate tests — (a) dry-run path allow/deny test
(lists only allowlisted files, never opens denied globs); (b) fixture ingest
redaction test (secrets redacted before persist). Plus sync test: hand-edit a
tool's non-managed memory, run sync twice → hand edit survives, managed block
idempotent, backup exists.

### S4 — Observer insights alive + daily briefing
1. With S0's reader rewrite, verify the 8 detectors emit real insights from
   real turns (drift, stale_projects, decision_conflicts…), persisted as
   insight objects/proposals (not line counts).
2. Daily briefing mode → `~/Obsidian/Imperium/Daily/YYYY-MM-DD-altevra-brief.md`.

**Gate (fixture):** seeded multi-session fixture yields ≥1 insight per
applicable detector with stable JSON; daily-brief generated from fixture
matches the `daily_briefing_v1` schema (schema test, not "Pavle judges").

### S5 — Personal brain layer (personal = business parity)
**Reconcile with existing schema** — migration 029 already created `persons`,
`relationships`, `preferences`, `event_log_personal` (high-water defaults);
024 is domain_policy; 027 is resident. Do NOT add a parallel notes table
blindly:
1. **DECIDED (not deferred): EXTEND 029, do not migrate away from it.**
   `persons` → Person, `relationships` → Relationship, `preferences` →
   Preference stay canonical in their existing 029 tables. For the remaining
   free-form kinds (Decision, Learning, Idea, Goal, Mood, Health, Place,
   Reference, Habit, Routine, Value, IdentityShift, LifeEvent) add ONE new
   `personal_notes(kind, ...)` table in a new migration, with FK links to
   029's tables where a note references a person/relationship. No data
   migration out of 029 (avoids parallel-table drift by making 029 canonical
   for its three types and `personal_notes` canonical for the rest).
2. **Note CLI:** the current CLI has `Capture`, not `Note`, and there is no
   notes repository. Add `NoteCommands` (`altevra note add <kind> "..."`,
   `altevra note list`) mapped to the reconciled schema above (029 tables +
   `personal_notes`).
2. Sensitivity labels + `review_required`; high-water (personal,
   relationship, health, financial, client) **always local_private** via
   `domain_policy.rs:161-180`, regardless of metadata label.
3. Relevance gate (`~/.altevra/interests.yaml`) — research only on stated
   interests + active goals; trivia filtered ("no Minecraft modpacks").

**Gate (fixture):** `note add preference/person` round-trips against the
reconciled schema; **test asserts high-water domains resolve to ONLY local
providers and Codex/cloud routes are denied before prompt construction**;
relevance-gate test drops an off-interest research item.

### S6 — Complete capability registry + tool integration + persistence
**The capability registry already exists** (`023_capability.sql:6-105`:
`capability_records`, `adapter_dossiers`, `capability_grants`; repo guards in
`capability.rs:35-65,174-249`). Reframe as **completing/using** it:
1. Add the missing CLI/MCP surface + tests to record/query `can`/`cannot`/
   `unverified` capabilities and grants.
2. **Imperium Crawl** wired as ONE concrete capability/connector via the
   existing grant model — proof of the integration pattern. Long tail
   deferred (backlog), built on this pattern.
3. Persistence: `altevra-brain.service` systemd **user** unit. Patch
   `~/.claude.json` MCP config (absolute `--db`, project-dir vault). NOTE the
   distinction: S3 **denies ingesting** `~/.claude.json` (auth-adjacent); S6
   only **patches** it via a targeted redacted updater that backs up first,
   logs NO values, and edits ONLY the MCP command/args fields. Hermetic config
   fixture test proves it patches the right fields and leaks nothing.

**Gate (hermetic):** capability record/query/grant round-trip test;
systemd-unit-file generation unit test (correct ExecStart absolute path,
Restart, no CWD dependence); `~/.claude.json` redacted-updater fixture test
(patches only MCP fields, backs up, leaks no values).
**Manual smoke (non-blocking):** brain survives a real reboot as a user
service; one real Imperium Crawl capability invoked through Altevra.

## Key decisions & tradeoffs

1. **Scope = full flywheel now** — Pavle's explicit choice against the GTM
   ("stop building, start selling") directive. Logged. Mitigated by strict
   staging + per-stage gates so it can pause for GTM at any boundary.
2. **ONE canonical DB**, never CWD-relative, enforced by a repo-wide path
   gate test. Merge-with-dedup + **quarantine (not delete)** + WAL-safe
   backup; corrected dedup for `external_id = NULL` live sessions.
3. **working_dir on session AND turn**, full repository-layer migration (not
   just two columns).
4. **Auto-refine skills + memory sync, safety-railed in the ACTUAL code**:
   extend `sync::apply_plan` with backup/drift/git + `ALTEVRA_MANAGED` blocks;
   never clobber human edits; install/sync separate from render, Pavle-gated.
5. **Codex = big brain, conserved (bounded raw-replay packet); small local
   models assist; no API-key-billed cloud; high-water denied to Codex by an
   exposure gate before prompt construction.**
6. **Reuse existing schema** (028 proposals, 023 capability, 029 personal) —
   no parallel tables; bridge/retire duplicates.
7. **Personal = business parity is structural**, reconciled with 029.
8. **Observer fix = readers AND writers** — insert events AND rewrite the
   CLI/MCP/brain loaders to read SQLite + call `detect_patterns`.
9. **External side effects Pavle-gated**: systemd, managed-block writes into
   `~/.claude`/Hermes, `~/.claude.json` edits, shadow-DB deletion.
10. **Graph is navigation only** — dependencies grounded in call-site audit,
    not inferred-edge centrality.

## Risks / open questions

- **GTM opportunity cost** — full flywheel is days of internal tooling vs the
  2-paid-clients directive. Stage gates are the release valve.
- **Weak laptop** — 3B local models + Ollama may be slow; embedding a backlog
  is heavy. Throttle/batch; small models only.
- **`db unify` correctness** — the corrected dedup (NULL external_id live
  sessions + FK remap) is the single most dangerous step; dry-run + count
  assertions + quarantine-not-delete are mandatory before trusting it.
- **Memory write-back safety** — writing into other tools' files is the
  riskiest surface; `ALTEVRA_MANAGED` delimiter + backup + human-edit refusal
  must be bulletproof.
- **Embedding dim migration** — switching to local 768-dim may invalidate any
  stored vectors; re-embed policy required, not optional.
- **Codex raw-replay vs token budget** — bounded packet may omit evidence a
  good skill needs; tune packet size empirically.
- **029 reconciliation is DECIDED (not open):** extend 029 (its
  persons/relationships/preferences stay canonical) + add a new
  `personal_notes(kind)` table for the remaining kinds, FK-linked to 029. No
  migration out of 029.

## Out of scope (this plan)

- Old-laptop-over-Tailscale session backlog (remote-import path deferred).
- The long tail of "every tool on the computer" beyond the one Imperium Crawl
  proof connector.
- Decade-arc features (pattern-over-years, identity-evolution graph,
  multi-modal/voice/wearables — VISION.md §5).
- API-key-billed cloud providers (no budget).
- Public release / packaging (v0.4+).
