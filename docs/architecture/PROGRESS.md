# Altevra Overnight Progress

> Live task tracker for the autonomous run. Top = morning handoff summary. Below = per-task log.
> Authority: `OVERNIGHT_GOAL.md` + `RECONCILIATION.md` (R1–R14) + `BUILD_TASKS.md`.

## 🛑 R11 SAFETY GATE — found leaks, fixed them (2026-06-01 afternoon)

**The "P0.1 core done / leak suites = 0" claim was only true for the tested fixtures.**
A real cross-engine adversarial pass (3 Claude lenses + Codex — the R11 gate that was
never properly run) found the gate had **multiple critical raw-data-leak paths**. All
critical/high findings are now FIXED + regression-tested; baseline fully green.

Commits: `888c360` (detector regex gaps), `05fdfe4` (PII + high-water classification +
PEM block + quarantine), `b56ade1` (exposure_gate fail-closed + no existence leak),
`dde2c40` (scan tool_calls/file_changes + persist verdict + gate turn reads).

Cross-engine consensus must-fix list (A–L) — status:
- A detector missed `sk-proj-`/Stripe/`postgresql://`/AIza/AWS-STS/npm/Slack-webhook/Bearer → FIXED
- B db_url leaked password-after-first-`@` → FIXED (captures whole user:pass)
- C PEM redacted header only (key body leaked) → FIXED (full BEGIN..END block)
- D PII was email-only → FIXED (`altevra-secrets/pii.rs`: phone/IBAN-mod97/card-Luhn)
- E health/personal prose default-DOWN to Internal → FIXED (high-water tags/domain → Restricted)
- F `tool_calls`/`file_changes` persisted RAW → FIXED (`guard_json` recursive scrub)
- G exposure_gate FAIL-OPEN on `None` redaction → FIXED (mandatory `&RedactionStatus`)
- H ExclusionRecord leaked id/type of denied items → FIXED (content-free aggregate)
- I turns dropped sensitivity/redaction verdict → FIXED (migration 026 + persist)
- J MCP replay/search ungated → FIXED (`turn_exposable` work-ceiling gate)
- K import used secrets-only redact(); `--no-redact` silent bypass → FIXED
- L rejected-class sighting didn't quarantine → FIXED
- M auto_capture vaults raw secret (by-design encrypted store) — confirm store perms (low)

Re-verification (Claude red-team v2 + Codex) PROVED the leaks closed. Migration
numbering shifted: 026 = safety_columns (turns+object_index); 027 = resident (P0.5);
028 = proposals (P0.6); P0.8 projects will own 029.

### R11 re-verify result (round 2) — CLOSED
Cross-engine re-verify found regressions the round-1 patches introduced + residuals.
All fixed in `61645a6`: PEM body-when-no-footer (CRITICAL), db_url slash-in-password
(HIGH), `turn record` side-channel raw persist (HIGH, Codex), exposure_gate order
(over-ceiling+superseded id leak), generic `access_key=`, spaced/lowercase IBAN,
keyword net, 0o600 secrets file. Codex verdict moved FAIL→8/10-closed→(after fixes)
clean; Claude verifiers re-run clean. **R11 gate CLOSED.**

## 🚀 SESSION 2 PROGRESS (2026-06-01 afternoon → P0.5/P0.6)

**P0.5 resident runtime — DONE** (`2a942e4`). The "just add keys" seam:
- migration 027: resident_run cols on brain_jobs (R10); resident_modes (8 builtins
  seeded, MOD-2) + resident_budgets. personal_curator=local_private (SI-7).
- core::resident (ResidentMode/Output, SI-7 validate, SI-14 schema parse);
  db::ResidentRepository; brain::ResidentRunner (noop dry-run, proposal-only SI-6).
- CLI `altevra resident run <mode>` — LIVE-verified: personal_curator→local_private→
  noop(local), recorded as resident_run. Every role→noop until keys; keys flip live.

**P0.6 self-improve risk model + runaway firewall — CORE DONE** (`b836bc3`):
- migration 028: proposals (kind discriminator, dedup) + improvement_signals +
  prompts (safety/altevra_rules locked, SI-2) + prompt_eval_results (SI-10).
- core::selfimprove: derive_risk_tier (SI-9) + firewall_check — pure Rust below the
  LLM enforcing kill/constitutional-lock(SI-2)/no-Tier1-2-auto(SI-2)/circuit(SI-11)/
  budget/Tier-0-cap(SI-12)/cooldown(SI-13)/shadow-eval(SI-10)/injection-proof(SI-15).
  Full runaway suite green.
- Remainder (noop-stubbed / LLM-wired): proposals repo + 7-stage orchestration +
  prompt registry render/rollback (T6.2/T6.5 LLM-driven half).

**P0.3 control plane — DONE** (`a45f0ed`): `altevra control review list|show|approve|
reject` (approve/reject presence-gated, R4 — live-verified REFUSED on non-TTY),
`redact check` (guard_text report, no raw — live-verified), `audit query`
(exposure_decisions). TasksRepository review list/get/decide. HP-1 MCP regression
test (no approve/apply/grant/forget/execute tool exposed).

**P0.8 personal brain + P0.9 sync prep — DONE** (`47410e5`): migration 029
(projects+parent_id R7, persons, relationships, preferences, event_log_personal).
DomainPolicyRepository reads the seeded 9-domain policy (no longer a dormant island);
CloudSync most-restrictive resolution (R3) + sync_eligible (P0.9 T9.1/T9.3 — restricted
domains excluded from sync set; unknown=fail-closed).

**P0.4 FTS5 substrate — DONE** (`353b7fd`): migration 030 object_fts (FTS5, unicode61
+ remove_diacritics for SR+EN, NO vectors R12). FtsRepository index+bm25 search with
injection-safe MATCH. The primary lexical retrieval substrate.

### ⬜ Still pending (exact plan in GAP_MAP.json)
- **P0.7 skill factory** — proposer mode is SEEDED (027); needs the post-approval
  render path (skill_proposal→skill→ToolAdapter::render_skills→installed_component,
  no-secret-in-render) + a Hermes ToolAdapter (skills→~/.imperium/skills/shared/, R10
  Q7) + usage tracking. Foundations exist (skill_proposals dedup, render_skills in all
  4 adapters). The Hermes adapter is a full ToolAdapter impl (7 methods).
- **Runtime remainders (LLM-wired / larger refactors, all noop-ready):**
  - P0.4 T-INV14: wire FtsRepository + PacketCompiler into live CLI `context` + MCP
    `get_context_packet` (legacy scan_vault still serves live) + golden eval harness.
  - P0.6 7-stage orchestration + prompt registry render/rollback (firewall + risk-tier
    + schema are done; the LLM-driven loop that feeds them is the remainder).
  - P0.8 lifecycle/purge job (R-EPH), export/forget/legal-hold CLI (presence pattern
    ready from P0.3), Imperium generated_mirror writer, tombstone conflict model.
- **Cursor live spawn** in herdr (Claude side live-tested; MCP smoke PASS after all
  changes; Cursor interactive spawn pending).

### Baseline + push state
608 tests pass, 0 failed; clippy --workspace --all-targets -D warnings clean; MCP
live smoke PASS. All work pushed to origin/altevra-overnight-p0. Secret-scanning note:
detector test fixtures use concat!() so no contiguous secret literal lives in source.

## ☀️ MORNING HANDOFF (read this first)

**Brate — sve commitovano, sve zeleno, LIVE TESTIRANO, i PUSH-ovano na GitHub.** 🔥

### ⭐ Najvažnije (jutarnji TL;DR)
- **18 commitova** na branch `altevra-overnight-p0`, **push-ovan na GitHub** (PR link: github.com/ceoimperiumprojects/altevra/pull/new/altevra-overnight-p0).
- **LIVE TEST PROŠAO** — spawn-ovao žive `claude` agente u herdr-u, naterao ih da zovu Altevra MCP. Našao i popravio **3 prava integration bug-a**: (1) altevra nije bio u PATH → symlinkovao, (2) MCP nije registrovan → `claude mcp add --scope user`, (3) `tools/call` vraćao goli JSON umesto MCP `content` envelope → popravljen. **Konekcija sa Claude Code sad radi end-to-end.** Detalji u REAL_TEST_LOG.md.
- Sutra: **dodaj API ključeve** (P0.5+ resident/LLM); sve ostalo radi bez njih.



### Šta RADI (verifikovano, end-to-end)
- **P0.0 kompletan** — svi core tipovi: 6-nivo Sensitivity (R1), Domain+RiskTag (R3), 6 status familija (R2), Envelope+Provenance, Template sistem + 9 builtina + TemplateGate (R13), contract-validation test, P0 fixtures.
- **P0.1 vertical loop DOKAZAN** — `cargo test -p altevra-cli --test p0_vertical_smoke` prolazi: capture → PreWriteSafetyGate → persist(envelope) → object_index → packet preko ExposureGate → exposure_decision audit. Tvrde garancije zelene: business decision uključen, restricted health isključen (non-leaking), fake secret redaktovan (0 raw u DB), untagged quarantined.
- Schema (migracije 019-022), PreWriteSafetyGate (`altevra-secrets/ingest_guard.rs`), ExposureGate (`altevra-core/safety/`), presence gate (`altevra-core/presence.rs`, R4), repo sloj (`altevra-db/repositories/objects.rs`).
- **Baseline potpuno zelen:** `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + ceo `cargo test --workspace` (sve faze 0 failed). 10 commitova na branch `altevra-overnight-p0`.

### Šta ČEKA tvoje API ključeve
- P0.5+ (resident runtime, LLM klasifikacija, self-improve, skill factory) — nije još počelo; deterministički core (P0.0-P0.4) ne treba ključeve.

### Šta je SLEDEĆE (nije još urađeno)
- P0.1 hardening: full packet compiler kao pravi modul `altevra-core/packet/` (T1.15, tag/FTS5/graph — NO vektori R12); vault mirror renderer (T1.14, treba `altevra-vault`); wire ingest_guard u sve repo write-ove (T1.13); review CLI + presence wiring (T1.17).
- **Live test još NIJE urađen** (REAL_TEST_LOG.md prazan) — planiran posle P0.3: build release, symlink, spawn Claude+Cursor u herdr-u, MCP smoke.
- Onda P0.2 (capability registry), P0.3 (control plane), P0.4 (retrieval hardening), P0.5-P0.9.

### Blockers: nema (BLOCKERS.md prazan).

### Napomene za resumera/mene-posle-kompaktovanja
- `ingest_guard` je u `altevra-secrets` (NE core) — izbegava dep ciklus (secrets→core). ExposureGate je u core (treba samo Envelope+Sensitivity).
- Packet compiler MORA biti tag/structured+BM25(FTS5)+graph — **NIKAD vektori** (R12).
- Svaki faced write ide kroz TemplateGate + mandatory tag (R13/TAG-1).
- Push na GitHub tek posle live testa (Pavle autorizovao push "nakon što testiraš sve").

---

## Phase status

| Phase | Status | Notes |
|---|---|---|
| Baseline | ✅ green | flaky env test fixed (mutex-serialized) |
| P0.0 contracts+enums+templates | ✅ done | T0.1–T0.10; 81 tests |
| P0.1 vertical loop | 🟩 core done | schema 019-022, both gates, presence, repo layer, packet compiler module, p0_vertical_smoke ✅. Hardening left: T1.13 wire guard into all repo writes, T1.14 vault mirror, T1.17 review CLI. |
| P0.2 capability registry | 🔄 good progress | T2.2 TrustLevel+Support ✅, T2.1 schema 023 ✅, capability repos (T7 honesty + dedup) ✅. Next: T2.3 component-state machine wiring (altevra-skills), T2.4 dossier/evidence from verify(), T2.5 watcher before_hash+self-write marker. |
| P0.3 control plane | ⬜ pending | CLI/MCP verbs for new objects |
| P0.5 resident runtime | 🔄 seam ready | T5.1 ChatProvider trait + ModelRole routing + NoopProvider ✅ (SI-7 enforced). Next: resident_mode registry (extend brain JobKind), resident_run, dry-run loop. THIS is the 'add keys' seam. |
| P0.1 vertical loop | ⬜ pending | |
| P0.2 capability registry | ⬜ pending | |
| P0.3 control plane | ⬜ pending | |
| P0.4 retrieval (tag/FTS5/graph) | ⬜ pending | |
| P0.5 resident runtime (noop) | ⬜ pending | |
| P0.6 self-improve + firewall | ⬜ pending | |
| P0.7 skill factory | ⬜ pending | |
| P0.8 domains/lifecycle | ⬜ pending | |
| P0.9 sync prep | ⬜ pending | |

## Per-task log

(task id · status · commit · note — appended as work proceeds)

- _run started 2026-06-01 ~03:2x_
- **baseline** ✅ `a0fa7e5` — fix flaky env test (mutex), commit arch docs.
- **T0.3** ✅ Sensitivity → 6-level total order + Other (fail-closed) + combine/within_ceiling.
- **T0.4** ✅ Domain (9 governed + Other) + RiskTag (8); is_high_water.
- **T0.5** ✅ 6 status families via string_enum! macro; quarantined→RedactionStatus; transition fns.
- **T0.6** ✅ Envelope + Provenance + HasEnvelope; is_complete/is_tagged (TAG-1).
- **T0.9/T0.10** ✅ Template + 9 builtins + TemplateGate (quarantine on untagged/non-conforming/ungoverned-domain).
- **T0.7** ✅ fixtures/p0/ (decision, personal-health, fake-secrets, drift mirror, superseded v1/v2, injection).
- **T0.8** ✅ contract_validation.rs golden enum lists.
- **T1.1-T1.5** ✅ `75fce2d` migrations 019-022 (envelope backfill, relations, object_index, learnings, insight_cards, secret_sightings, audit_log, exposure_decisions, context_packets).
- **T1.7/T1.8** ✅ `67cfdb9` PreWriteSafetyGate (altevra-secrets/ingest_guard) — secrets+PII redact, classify, TemplateGate, fail-closed.
- **T1.11** ✅ `64bcb77` ExposureGate (core/safety) — non-leaking deny reasons.
- **T1.18** ✅ `c549c73` p0_vertical_smoke — full chain proven, leak=0.
- **T1.10** ✅ `000c95f` presence gate (TTY + ALTEVRA_UNLOCK).
- **T1.12** ✅ `ecf1921` LearningsRepository + ObjectIndexRepository.
- **T1.15** ✅ `9a92015` Context Packet Compiler module (gates≠weights, tag+recency, no vectors, deterministic, budget packing).
- **Baseline re-verified:** fmt+clippy(--all-targets -D warnings)+tests all green.
- _NEXT: T1.13 wire ingest_guard into all repo writes · T1.14 vault mirror renderer · T1.17 review CLI + presence wiring · then live test (build+symlink+herdr spawn) · P0.2._
