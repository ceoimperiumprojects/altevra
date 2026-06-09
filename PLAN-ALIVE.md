# Plan: Make Altevra ALIVE
_Locked via grill — by Claude + Pavle, 2026-06-09. Revised after adversarial round 1 (correctness + security lenses, 26 findings accepted)._

## Goal

Altevra's **recorder half is verified alive** (live Claude Code capture works — session `00900bc4` grew 159→400+ turns in real time, all `source_tool=claude-code` with correct `working_dir`; A1–A4 done; 953 workspace tests pass). This plan builds the **brain/proactive half** so Altevra becomes the living center of Pavle's AI system: one canonical DB, a Tool Register every agent reads at session start, observer insights that actually fire, a trust-ladder-gated skill factory (ported from Hivemind's SkillOpt), a proactive daily briefing, and a personal-brain layer. Built P0→P5 in **independently verifiable milestones** (commit + green tests + real-data smoke at every Pn boundary) so Pavle can pause and pivot to GTM at any boundary. Every external side effect is Pavle-authorized. **Safety guarantees are mechanisms, not assertions:** every claimed gate names the function/table that enforces it.

## Approach

Dependency-ordered milestones. **No Pn starts until P(n-1)'s gate passes and is committed.** Branch `s0-foundation` already holds verified A1–A4.

### P0 — Finish S0 (data integrity) — BLOCKING

The only S0 code blocker is `altevra db unify` (zero implementation; `altevra db --help` → "unrecognized subcommand"). A 1101-turn shadow DB at `~/projekti/ai-tooling/altevra/.altevra/altevra.db` (migration **033**, no `working_dir`) is siloed from canonical.

**Ordering inside P0 (locked):** hook-config regeneration MUST precede the real unify smoke — stale installed hooks are exactly what created the shadow DB; a live session firing a stale hook mid-/post-unify would recreate a shadow at the old path and invalidate the "zero shadow DBs" check.

1. **Regenerate installed hooks FIRST** — absolute `--db`, drop unused `$ALTEVRA_PROJECT` (fixes `project_name=NULL`); verify installed commands.
2. **`altevra db unify`** per PLAN.md §S0.6, with these mechanics corrected:
   - **Shadow 033→034 upgrade by introspection, NOT the sqlx migrator.** `run_migrations` validates checksums of applied migrations (migrations 001–006 were edited post-creation → `VersionMismatch` risk on old shadows) and would over-apply future migrations to a DB we're about to quarantine. Instead: `PRAGMA table_info(...)` + targeted `ALTER TABLE sessions/turns ADD COLUMN working_dir TEXT` mirroring 034.
   - **Backup is checkpoint-then-copy:** `PRAGMA wal_checkpoint(TRUNCATE)` (or `VACUUM INTO`) per discovered DB, then copy `db` + `-wal` + `-shm` to `~/.altevra/backups/<ts>/`.
   - **The merge transaction writes ONLY the canonical DB** (a txn spanning ATTACHed DBs is not atomic under WAL). Shadow quarantine = filesystem rename AFTER commit, never an in-txn shadow write.
   - **Dedup (locked, conservative):** non-null `external_id` → `(tool, external_id)`. NULL-external_id live sessions auto-merge ONLY on session-`id` match OR full ordered NON-EMPTY turn-sequence hash; else **quarantine, never auto-merge**. Turns collapse only on full `(session_id,turn_idx,role,content_hash,tool_calls_hash,file_changes_hash)` match; divergent `(session_id,turn_idx)` collisions → `turns_quarantine`, never overwrite.
   - **FK remap set enumerated explicitly:** `turns.session_id`, `file_changes.session_id/turn_id`, plus plain-TEXT refs in `improvement_signals` (payload refs), `proposals.evidence_refs`, and `events.entity_id`.
   - **FTS is APP-MAINTAINED, not trigger-maintained** (030 creates only the `object_fts` virtual table; there are zero triggers in the codebase — `FtsRepository` does explicit DELETE+INSERT on every durable write). Unify must **explicitly re-index merged objects via `FtsRepository::index`** (or carry `object_fts` rows inside the canonical-only txn); the gate asserts merged content is FTS-findable.
   - **Content-table merge scope (explicit):** beyond sessions/turns/file_changes, the shadow's content tables also merge — `object_index`/`object_fts` (69 rows), `learnings` (65), `wiki_pages` (4), `relations` (58), `research_items` (1427), `improvement_signals`/`proposals` — deduped by primary id (quarantine on id-collision-with-different-content), then FTS re-indexed per the bullet above. Nothing valuable stays behind in quarantine silently: the dry-run report lists per-table merge counts.
   - **Merged shadow turns are re-guarded or marked `redaction_status='unscanned'`** (the ExposureGate fail-closes on Unscanned) — 033-era rows predate current redaction hardening.
   - **Maintenance lock + hook spool, fully specified:** lock file with **stale-lock TTL**; non-hook batch writers refuse non-fatally. Hooks spool to `~/.altevra/state/spool/<tool>-<pid>-<ts>.json` — **one file per event**, `O_EXCL`, **mode 0600 at open** ($HOME-anchored, never CWD). Spool entries **embed the session_id + full turn payload** (guard_json-redacted before disk); replay goes through a **direct-by-id ingest function that errors loudly** — NEVER through `record_turn`'s pointer-lookup path (pointer may be gone by replay time → silent data loss). `altevra db replay-spool` is a mandatory idempotent unify epilogue; replay failure keeps the file + writes an `audit_log` row; `doctor` flags a non-empty spool. Acknowledged degradation: spooled content is pre-redacted, so `auto_capture` never sees raw values for spooled turns.
   - **Quarantine-not-delete** shadow DBs; `--dry-run` prints before/after counts + conflict report.
3. **Config-load fix:** `memory search` + `context` (`context.rs:33` hardcodes `default_value="."`) call `Config::load()` and use `config.toml [vault].path` (fallback `ALTEVRA_VAULT`) as the `--vault` fallback. Brain default vault → `~/Obsidian/Imperium/`.
4. **Live event emission lands HERE (not P4):** hook points emit `session_start/end`, `tool_call`+`tool_result` (PostToolUse), `tool_failure`, `file_change`, `skill_invocation`+`skill_reaction`, `prompt_submit` — all through guard/redaction, **payload size cap at emission**. (Three small inserts in hook_handle; P3c depends on this, so it cannot live in P4.) **Events retention added to the lifecycle sweep** (raw tool_call/tool_result pruned after N days; session/skill events kept) — the existing lifecycle.rs purges only `context_packets`; without this the events table grows unboundedly on a weak laptop.
5. `working_dir` non-null roundtrip test; `pending_indexing` drain-or-remove; **test-isolation fix at the source**: find and fix the specific leaking code path (the 2 `test-agent` sessions in canonical DB); each test owns a **per-test temp DB** (shared `ALTEVRA_DB_PATH` is a flake factory, env var is a backstop only).

**Gate (hermetic):** seeded multi-DB fixture (incl. a 033-schema shadow) → exact union counts + 033→034 introspection upgrade + FK integrity + quarantine behavior + spool replay-by-id test (incl. session-ended-during-unify case) + zero shadow DBs from any CWD; `working_dir` non-null roundtrip; event-emission test (each hook point → guarded event row, payload capped); `cargo test --workspace` green (≥953) with per-test DB isolation. **Manual smoke (non-blocking):** `db unify --dry-run` against the real shadow shows sane counts; then Pavle-authorized real unify with backup verified. **Milestone commit.**

### P1 — Tool Register / capabilities layer

1. **Migration 035 `tool_records`:** `id, name, kind (skill|cli|python-api|mcp-server|web-service|adb|binary), UNIQUE(name, kind)` — NOT `name` alone ("codex" is a skill AND a binary AND a wrapper) — plus `display_name, description, invocation JSON, locations JSON, can_do/cannot_do/unverified JSON, requires_session JSON, status, last_verified_at, categories JSON, source (scan|hook|manual), adapter_ref` (optional FK-by-name → `adapter_dossiers.tool_name`, since hermes/codex/cursor legitimately exist in both tables; `get_capabilities` defines precedence: adapter_dossiers wins for agent-identity fields, tool_records for invocation).
2. **Tool discovery (locked: scan + hooks, multi-location aware) — reconciliation key is `(name, kind)`,** with realpath used ONLY to dedup identical-file PATH aliases (symlinks). Realpath does NOT reconcile the motivating case (source checkout vs npm-global have different realpaths — that's name-based) and breaks on version-manager shims (mise/asdf/nvm all realpath to one shim binary) → **shim-dir denylist** (`~/.local/share/mise/shims`, nvm shim dirs) with a test fixture. Scan sources: `$PATH`, npm-global, `~/.claude/skills/`, `~/.imperium/capabilities/*.yaml`, source checkouts under `~/projekti`. All discovered locations recorded in `locations[]`, canonical invocation chosen, rest alternates. `SEED_TOOLS` const (15 priority tools) as baseline. Idempotent; `--dry-run`-able.
3. **Security (mandatory):** every `tool_records` field passes **`guard_json` at upsert** (capability YAMLs and documented invocations routinely embed bearer tokens/credentials — one credential would otherwise fan out: DB → SessionStart injection → re-recorded into turns → served over MCP). Record `secret_sightings`. The scan **inherits S3's DENY globs** (`**/auth*`, `**/*token*`, `**/*secret*`, `**/*.env*`, db files) before opening anything under source checkouts.
4. `ToolRecordsRepository` (upsert by name+kind, list by kind/status, get) in `repositories/mod.rs`; CLI `altevra tool scan/list/register/verify`, `altevra capability list/record/seed` (loads `adapter_dossiers` from `~/.imperium/capabilities/{claude,hermes}.yaml`); MCP `get_capabilities` queries the DB (keys: `tools`, `adapter_dossiers`, `capability_records`; documented precedence), replacing the hardcoded fallback.

**Gate (hermetic):** fixture with same tool in two locations → one row, both in `locations[]`; same name different kind → two rows; shim-dir fixture excluded; **yaml-with-embedded-bearer-token → redacted row + sighting logged**; `get_capabilities` returns DB rows. **Manual smoke:** real scan finds Imperium Crawl's multiple installs. **Milestone commit.**

### P2 — SessionStart context injection (the "alive moment")

1. **Transport matrix is (tool × transport), not tool-only:** Claude Code = hook `additionalContext`; Hermes = `get_agent_bootstrap_packet` (MCP — it doesn't consume hooks); Cursor = `altevra context` pull/adapter-specific (it has NO additionalContext channel); **Codex = NOTHING** (user-visible, clobbers TUI). `AgentBootstrapPacket` gains `available_tools`; `build_system_prompt` gains a tool-register layer; `render_tool_register_block()` in altevra-core.
2. **Stdout protocol:** `handle_session_start` today prints `{"session_id":...}` to stdout (hook_handle.rs:140) which Claude Code already injects — emit exactly ONE `hookSpecificOutput.additionalContext` JSON document; move session_id into it or to stderr. Never two JSON objects on stdout.
3. **Fail-open with a hard deadline:** entire context assembly wrapped in an internal deadline (≤1s) with catch-all → valid empty output + exit 0. (Today `create_pool(...).await?` propagates → a write-locked DB at session start stalls ~5–10s then errors — the opposite of "never block".) Locked-DB path is a gate test.
4. **Sensitivity filter (mandatory — the ExposureGate exists and must be used):** every injected item (goal/decision/proposal/tool) passes **`ExposureGate::decide`** with a work/agent audience request; high-water-domain and Restricted/Unscanned items are excluded. **Filter error ⇒ that item is excluded (fail-closed per item); only total assembly error ⇒ empty block (fail-open for availability).** Every injection writes an **`exposure_decisions`** audit row (the 021 table exists for exactly this). Recency alone must never select a relationship/health decision into a coding-session context.
5. **Token budget pinned: 1–2K tokens** for the SessionStart block (NOT the context_packet default of 8000), asserted in the gate.
6. `altevra context` CLI — pull fallback for hook-less tools.

**Gate (hermetic):** block contains goals+decisions+tools for claude-code; EMPTY for codex; **a Restricted/high-water decision is NOT in the block**; locked-DB → empty output exit 0 within deadline; single-JSON-document stdout; budget ≤2K asserted; exposure_decisions row written. **Manual smoke:** fresh real Claude Code session shows injected context. **First slice:** goals + last-3-filtered-decisions only, then add the register. **Milestone commit.**

### P3 — Skill factory: SkillOpt port, trust-ladder gated

Port Hivemind's optimizer (docs/research/hivemind/01-skillify-engine.md), gated by Altevra's review queue (never auto-publish — locked). **Dependency note:** the Codex StrongReasoner route ALREADY EXISTS (`altevra-llm/factory.rs` `codex_oauth` mode) — P3b is gated on a one-line smoke of that route, not on unbuilt "S1" work.

- **P3a (pure Rust, no LLM):** port `skill-edits.ts` (4 deterministic ops + edit budget + protected region) and `skillopt-meta` (meta-fingerprint table). **Protected-region grammar unified:** ONE convention — the existing `ALTEVRA_MANAGED` marker family — with the SkillOpt slow-update semantics expressed as `<!-- ALTEVRA_SLOW_UPDATE_START/END -->`; precedence documented (managed-region machinery honors both; no file carries two competing grammars).
- **P3b (renderer):** `altevra skill-factory render --proposal <id>`. **Pre-packet exposure gate is a specified mechanism, not an assumption:** the previously cited `domain_policy.rs:161-180` is `embedding_role_for()` — an embedding-role resolver, NOT a render-route gate — and **turns carry no domain** (`tools_sessions.rs` stamps Business). Therefore: the packet builder runs EVERY evidence turn through **`ExposureGate::decide` with a NEW `external_route` request profile** stricter than `default_work` — deny on sensitivity ≥ Confidential OR `redaction_status` ∉ {Redacted, Clean} OR session working_dir/project mapped to a high-water domain — **refuses the entire proposal if any ref is denied** (before any provider call), and writes an `exposure_decisions` row per packet. The secret/PII scan on rendered OUTPUT stays, but it is the second line, not the gate. **Acknowledged residual risk:** topic-level personal content in Business-stamped turns can still reach Codex; mitigation = sessions with `working_dir=None`/personal projects excluded from external replay + (optional) local_private pre-classifier pass.
- **P3c (backward pass):** `success-judge` port (anti-sycophancy, conservative-on-failure, cheap_worker); event-driven K-message reaction window consuming the `skill_invocation`/`skill_reaction` events emitted since P0; confirmed failure → judge → bounded edit → **review queue, never auto-publish**.
- **Install/sync (Pavle-authorized) — new mechanism, honestly scoped:** today's `apply_plan` (sync.rs:251-314) replaces whole files; region-scoped writes are NEW. Add a **`managed_writes` manifest table** (target path, block hash, backup path, ts) — drift = current block hash ≠ manifest hash ⇒ refuse → review ("never overwrite human edits" is undetectable without a stored baseline). Backups to `~/.altevra/backups/sync/<ts>/`; `altevra skill-sync restore` command; re-verify hash after writing temp before rename (TOCTOU); git commit **only if the target dir is a repo**.

**Gate (hermetic):** P3a edit-ops + region-protection + meta-fingerprint tests; stub-renderer refuses missing-refs; **proposal whose evidence includes a Restricted/Unscanned turn is refused before any provider call**; success-judge anti-sycophancy test; sync test (manifest drift refusal + backup + managed-block idempotent + restore). **Manual smoke:** one real staged SKILL.md; one-line Codex-route smoke. **Milestone commit.**

### P4 — Proactive daily briefing + relevance gate

DailySummary already computes real content (`jobs.rs:590-745`); the gaps are **delivery, policy enforcement, and the relevance gate**.

1. **Delivery layer consults `domain_policies.obsidian_mirror` PER ITEM, fail-closed** (lookup error or missing flag ⇒ drop item + audit_log row). This is mandatory because the current contact-gap section ("haven't talked to ‹Person› in N weeks") is relationship-domain data and `dp_relationship` is seeded `obsidian_mirror='never'` — shipping it to a syncable Obsidian vault would violate the system's own policy. The contact-gap section is policy-gated per person's domain or reduced to a count + "view in CLI" pointer.
2. Port Hivemind's Source/Rule/Delivery contract: **`userVisibleOnly=true` is the DEFAULT for all rules** (opt-out explicit) — an unflagged rule must never reach the SessionStart/agent channel; atomic `O_EXCL` dedup; cadence-gating; high-precision-or-silent. Rules: decision-staleness, relationship-cadence (CLI-only per #1), resume-brief, open-proposals.
3. **Observer cold-start backfill (here, after P0's live emission):** one-time, **metadata-only synthetic events** (counts, turn/session IDs as refs, tool names — NEVER turn body content, since 033-era content may be weakly redacted); **deterministic event ids** (UUIDv5 of `(source_turn_id, event_type)`) + `INSERT OR IGNORE` + a backfill-watermark row = true idempotency; **historical timestamps** + one explicit one-shot `observer scan --since <epoch>` over `source=backfill` to produce the cold-start insights (now()-stamped backfill would flood every 7-day window and drown live signal; historical-stamped rows are invisible to `list_since` without the explicit scan).
4. Relevance gate (`~/.altevra/interests.yaml`) — research/surfacing only on stated interests + active goals.

**Gate (hermetic):** brief from fixture matches `daily_briefing_v1`; **relationship item is dropped from the Obsidian path by policy (fail-closed test)**; unflagged rule never reaches the agent channel; backfill run twice → zero duplicate events; backfill event contains no turn body text; relevance-gate drops an off-interest item. **Manual smoke:** one real morning brief in Obsidian Daily (policy-filtered). **Milestone commit.**

### P5 — Personal brain layer (personal = business parity)

1. Migration `personal_notes(kind, ...)` — but **kinds that already have canonical stores are FK-pointers, not parallel rows**: Person/Relationship/Preference → 029 tables; Decision/Goal → the existing object-envelope/goals stores that P2 queries (two sources of truth for the exact items injected at session start is how drift starts). Net-new kinds (Place, Idea, Mood, Health, Memory, Reference, Habit, Routine, Value, IdentityShift, LifeEvent) live in `personal_notes`.
2. `NoteCommands` CLI (`altevra note add <kind> "..."`, `altevra note list`) writing to the reconciled stores.
3. High-water (personal/relationship/health/financial/client) always routes local_private; `userVisibleOnly=true` mandatory for personal/relationship/health notifications (enforced by the P4 delivery layer).

**Gate (hermetic):** `note add` round-trips (FK kinds land in canonical stores, net-new in personal_notes); high-water domains resolve to ONLY local providers (external denied before prompt construction via the P3b `external_route` profile); `userVisibleOnly` enforced. **Milestone commit.**

## Key decisions & tradeoffs

1. **Full P0→P5 in verified milestones** — Pavle's explicit choice over the GTM directive (flagged, logged); milestone boundaries are the pivot points.
2. **`tool_records` separate table, `UNIQUE(name,kind)`, `adapter_ref` link** — tools ≠ agents, but the same name legitimately exists in both worlds and across kinds.
3. **Discovery reconciles by `(name,kind)`**; realpath only dedups symlink aliases; shim denylist for version managers.
4. **SessionStart: (tool × transport) matrix** — Claude=hook, Hermes=bootstrap packet, Cursor=pull, Codex=nothing; ≤2K token budget; ≤1s hard deadline; single JSON doc on stdout.
5. **Every injected/exported item passes `ExposureGate::decide`; per-item fail-closed, whole-block fail-open; `exposure_decisions` audited.** New `external_route` profile gates Codex-bound evidence packets (the old domain_policy citation was the wrong mechanism — corrected).
6. **Obsidian delivery obeys `domain_policies.obsidian_mirror` per item, fail-closed** — relationship/health content never lands in a syncable vault.
7. **Event emission + retention live in P0** (P3c depends on it); backfill is metadata-only, deterministic-id, historically-stamped, one-shot-scanned.
8. **db unify:** introspection-based shadow upgrade, checkpoint-then-copy backup, canonical-only txn, enumerated FK remap, replay-by-id spool with TTL'd lock, quarantine-never-delete. Hook regen precedes real unify.
9. **(Prior-locked, unchanged):** quarantine-never-auto-merge dedup; skill edits always to review; ONE canonical DB; working_dir on session+turn; protected-region grammar unified under ALTEVRA_MANAGED family.
10. **External side effects Pavle-gated:** real unify, systemd, managed-block write-back, `~/.claude.json` patch.

## Risks / open questions

- **GTM opportunity cost** — milestone boundaries are the release valve; P0+P1+P2 alone deliver the "alive" payoff.
- **Residual topic-level leakage to Codex (P3b):** Business-stamped turns can carry personal topics past the sensitivity gate; mitigations listed (working_dir exclusion, optional local pre-classifier), accepted as residual.
- **Tool discovery false positives** — `(name,kind)` key + shim denylist + dry-run mitigate; long tail will need manual `tool verify`.
- **Weak laptop** — per-test temp DBs and `cargo test -p` targeting keep test runs feasible; events retention keeps the DB bounded.
- **`db unify` on real data** — most dangerous step; dry-run + checkpoint-backup + quarantine + Pavle authorization mandatory.
- **`skill_reaction` heuristic fuzziness** — start conservative; judge is conservative-on-failure by design.

## Out of scope

- Old-laptop-over-Tailscale backlog import; bulk historical import of 875 claude-code + 285 codex sessions (apply oldest-watermark when done).
- Adopting Hivemind itself (cloud-only SaaS — designs borrowed only).
- Decade-arc features; API-key-billed cloud providers; public release/packaging.
- Codex cross-model plan review re-run (workspace out of credits) — this revision used two independent same-family adversarial reviewers (correctness + security lenses) with code-verified findings; honestly logged as such in PLAN-ALIVE-REVIEW-LOG.md. A Codex re-pass on P1–P5 is recommended when credits return, before P3 (the external-route stage) begins.
