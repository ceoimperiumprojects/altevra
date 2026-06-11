# Plan: Extension Wave — Modular Connectors, Self-Improvement Loop, Packaging
_By Claude (Fable 5) under Pavle's "top-tier tonight" goal directive, 2026-06-11. Builds ONLY on already-adversarially-reviewed primitives (guard_text, relevance gate, domain_policy, ExposureGate, block-writer, tool_records, brain jobs, trust ladder) — no new security surface classes._

## Goal (from Pavle's directive, distilled)

Altevra must be: usable for years, scalable, model-swappable by config, insight-producing, **modular so ANY external tool connects in minutes not months** (Google Calendar, Gmail/Workspace, Linear, …), self-cleaning, self-improving (tweaks its own prompts through the trust ladder), business=personal, daily-value, and at the very end packaged for easy install (Claude/Imperium marketplace skill + npm prep) — packaging LAST so it never blocks the substance.

## E1 — Connector SDK (the modularity core)

The pattern that makes "povezivanje sa svime" a config exercise:

1. **`Connector` trait** (new module `crates/altevra-adapters/src/connectors/` or small new crate): `descriptor()` (name, kind, auth_mode: none|api_key|app_password|ics_url, domains touched), `pull(&ctx) -> Vec<ConnectorItem>`, `health() -> ConnectorHealth`. `ConnectorItem` is typed (CalendarEvent | EmailHeader | Issue | Note) with provenance (connector, external_id, ts) and a declared domain.
2. **Registry = `tool_records`** (kind `connector`, source `manual|scan`) — connectors ARE tools; config in `~/.altevra/connectors.toml` (per-connector: enabled=false default, auth ref into the EXISTING secrets/keyring crate, cadence, domain overrides).
3. **Ingest path is the existing safety stack:** every item → guard_text → domain_policy sensitivity floor → relevance gate (research-class items) → object_index/personal_notes/events as appropriate. NOTHING new bypasses the gates.
4. **`connector_sync` brain job** — per-connector cadence; failures → health red + doctor; never blocks other jobs.
5. **Reference connectors (tonight, fixture-tested; real creds are a Pavle 2-min config later):**
   - **ICS calendar** (file path or URL): parses VEVENT → CalendarEvents → today/tomorrow events feed the daily brief "Calendar" section. Works with Google Calendar's private-ICS URL with ZERO OAuth.
   - **IMAP email headers** (app-password auth, e.g. Gmail app password): pulls UNSEEN headers + first-N-chars snippet ONLY (never bodies by default), domain=comms, guarded; config-gated OFF.
   - **Linear** (API key, GraphQL viewer issues): open issues → tasks surface in brief.
   - **Obsidian vault** registered as a connector for uniformity (already-ingested, descriptor only).
6. **CLI:** `altevra connector list/health/sync [--name] [--dry-run]`; doctor gains connector-health.
7. **Tests:** trait round-trip with a mock connector; ICS fixture parse → brief section; IMAP against a mock server fixture; Linear against a recorded GraphQL fixture; disabled-by-default; guard applied to item text; relevance gate drops off-interest items; secrets never logged.

## E2 — Self-improvement closing loop (prompt self-tweaking, trust-laddered)

1. **`prompt_tweak` proposal flow:** observer/selfimprove signal (low-quality mode output, repeated user corrections) → proposal kind=`prompt_tweak` with a UNIFIED-DIFF body against a resident mode prompt file (06-skills/resident-agent-modes/*) → review queue (NEVER auto-applied — trust ladder) → on Pavle approve: applied via the R5 **block-level guarded writer** (managed region inside the prompt file), versioned + reversible.
2. **`altevra prompt tweaks list/show/approve/reject`** CLI on the review queue.
3. **Self-cleaning:** weekly brain job `db_optimize` — `PRAGMA optimize; PRAGMA incremental_vacuum;` + verify retention jobs ran (events R4, context_packets existing); raw turns are NEVER deleted (raw trace is canonical — doctrine); report DB size trend in doctor.
4. Tests: tweak proposal lifecycle on fixture prompt; guarded apply preserves non-managed prompt content; optimize job runs and records.

## E3 — Model-swap UX (config-only swapping, proven)

1. `altevra llm models` — lists Ollama-available models + the active role→provider table from config.
2. `altevra llm use` presets already exist — add `--role` targeted override (`altevra llm use ollama --model X --role cheap_worker`) writing only that role.
3. Test: config-only swap of cheap_worker between two models with zero code changes; doctor reflects active roles.

## E4 — Packaging prep (LAST, prep-not-publish)

1. **Claude marketplace plugin manifest**: `.claude-plugin/` with plugin.json/marketplace.json referencing the altevra skills + MCP server launch (format per anthropics/claude-plugins-official) — ready for Imperium/Claude marketplace listing, NOT published tonight.
2. **Install story**: README quickstart (clone → cargo build --release → `altevra setup` → `altevra auth codex` → `altevra service install`) verified against a fresh-eyes read; `deploy/systemd/` shipped.
3. **npm wrapper: SKETCH ONLY** (docs/PACKAGING.md — napi/binary-download tradeoffs) per Pavle's "ne sme da te zaustavi".

## E5 — Final verification (main loop, me) + ship

Real-data: capture recovery proof (multi-cwd), real import (949+285), backfill kickoff, systemd install + reboot-survivability check, doctor 100%, observer real insight, memory-sync smoke, live skill install, connector ICS smoke with a local fixture file, brief with calendar section, model-swap smoke. Then: milestone commits, push, version bump + CHANGELOG (R7), PR-ready state; merge = Pavle's morning sign-off.

## Honest notes

- No separate Codex review round for THIS wave (Pavle granted engineering freedom; every mechanism reuses already-reviewed primitives; the riskiest novel surface — connector ingest — runs through the full existing gate stack and ships disabled-by-default).
- Google Workspace OAuth (full Gmail/Calendar API) is NOT tonight — ICS-URL + app-password IMAP deliver the same daily value with zero OAuth infrastructure; a proper OAuth connector is a clean follow-up on the same trait.
- Linear/IMAP real credentials are Pavle's 2-minute config when he wants them; tonight proves the rails with fixtures.
