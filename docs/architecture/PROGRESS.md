# Altevra Overnight Progress

> Live task tracker for the autonomous run. Top = morning handoff summary. Below = per-task log.
> Authority: `OVERNIGHT_GOAL.md` + `RECONCILIATION.md` (R1–R14) + `BUILD_TASKS.md`.

## Morning handoff (updated as run progresses)

- **Status:** P0.0 done. P0.1 in progress — schema (019-022) ✅, PreWriteSafetyGate ✅. NEXT: presence gate (T1.10), ExposureGate (T1.11), repos for new tables (T1.12), route writes through guard (T1.13), vault mirror (T1.14), tag/FTS5/graph packet compiler (T1.15), p0-vertical-smoke (T1.18).
- **Done & committed:** baseline fix; P0.0 core types (Sensitivity 6-level, Domain+RiskTag, 6 status families, Envelope, Template+9 builtins+TemplateGate, contract-validation, fixtures); P0.1 schema migrations 019-022 (envelope backfill, relations, object_index, learnings, insight_cards, secret_sightings, audit_log, exposure_decisions, context_packets); **PreWriteSafetyGate (altevra-secrets/ingest_guard.rs)** — detect→redact→classify→template-gate, fail-closed.
- **Live-tested:** (after P0.3)
- **Awaits API keys:** (P0.5+ resident/LLM — not reached yet)
- **Blockers:** none (see BLOCKERS.md)
- **Baseline:** green — fmt+clippy clean; 81 core + 5 ingest_guard + 24 db tests pass.
- **Note for resumer:** ingest_guard lives in `altevra-secrets` (NOT core) to avoid a dep cycle (secrets→core). ExposureGate should go in altevra-secrets too (same reason) or a new altevra-safety crate; it needs Envelope+Sensitivity (core) only, so core is also fine for ExposureGate specifically. Packet compiler (T1.15) is tag/structured+BM25(FTS5)+graph — NO vectors (R12).

---

## Phase status

| Phase | Status | Notes |
|---|---|---|
| Baseline | ✅ green | flaky env test fixed (mutex-serialized) |
| P0.0 contracts+enums+templates | ✅ done | T0.1–T0.10; 81 tests |
| P0.1 vertical loop | 🔄 in progress | schema 019-022 ✅, PreWriteSafetyGate ✅; next: presence/ExposureGate/repos/packet/smoke |
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
