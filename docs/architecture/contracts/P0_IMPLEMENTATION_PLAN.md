# Altevra P0 Implementation Plan

Status: synthesized-by-Hermes

## Phase P0.0 — contracts before code

1. Add schema enum contracts for object/status/domain/source-of-truth/gate outputs.
2. Add fixture vault with synthetic data only.
3. Add validation/snapshot harness for architecture contracts.
4. Add migration plan for minimal object envelope fields in SQLite.

## Phase P0.1 — one vertical loop

1. Implement `PreWriteSafetyGate` with fake-secret detector + domain defaults.
2. Implement minimal object insert/read in SQLite.
3. Implement Obsidian mirror renderer for allowed object types.
4. Implement `ExposureGate` and deterministic structured/BM25-ish packet builder (no vector dependency yet).
5. Implement packet audit record.
6. Implement review item creation for generated-mirror edit and self-improvement proposal fixture.
7. Implement `altevra p0-vertical-smoke --fixture fixtures/p0 --json`.

## Phase P0.2 — registry foundation

1. Read-only tool/skill capability inventory.
2. Capability state health check.
3. Proposal path for skill/tool updates; no auto-install.

## Explicit deferrals

- Dashboard.
- External research/connectors.
- Cloud sync daemon.
- Vector DB/embedding dependency.
- Auto-apply self-improvement.
- Production customer/personal data ingestion.
