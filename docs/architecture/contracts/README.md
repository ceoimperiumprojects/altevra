# Altevra Architecture Contracts

**Status (2026-06-01): P0.0 contract phase COMPLETE at the document level.**

## Where the contracts actually live

The deep per-domain contracts are the **seven sections of the working draft** —
they are detailed, invariant-numbered, test-mapped contract law, not sketches:

| Domain | Contract source (working draft section) |
|---|---|
| Object model / envelope / edges | `../ALTEVRA_ARCHITECTURE_WORKING_DRAFT.md` §1 |
| Data safety / secrets / source-of-truth | §2 |
| Context + retrieval + packet | §3 |
| Agent prompts + self-improvement | §4 |
| Tools + skills + interfaces | §5 |
| Domains + lifecycle | §6 |
| Synthesis | §7 |

Three documents make those sections executable:

1. **`../RECONCILIATION.md`** — locks every cross-section contradiction + open
   question (R1–R11). **The authority** when sections disagree. Read first.
2. **`P0_CONTRACTS.md`** (this folder) — the ratified short-form: enums, gates,
   source-of-truth classes, domain defaults, P0 non-negotiables — reconciled.
3. **`P0_ACCEPTANCE_TESTS.md`** + **`P0_IMPLEMENTATION_PLAN.md`** (this folder)
   + **`../BUILD_TASKS.md`** — the granular execution plan (73 atomic tasks,
   mapped to the live code, phased P0.0→P0.9).

## On the 14 "planned" standalone files

The original plan listed `00_object_model.md` … `13_p0_vertical_loop_acceptance.md`.
**Decision (RECONCILIATION):** do NOT duplicate the working-draft sections into 14
separate files — that's redundant maintenance surface. The sections ARE the
low-level contracts; RECONCILIATION + P0_CONTRACTS + BUILD_TASKS bind them. If a
future need arises to extract one (e.g. a standalone `01_data_safety` for an
external Codex audit), extract it from §2 verbatim at that time — don't pre-build all 14.

## Outstanding gate before P0.1 safety merge

Codex skeptical breaker review (security + implementation) was blocked
out-of-credits and only a Hermes fallback ran. **Re-run Codex on the safety
crate before merging `ingest_guard`/`exposure_gate`** (RECONCILIATION R11).
