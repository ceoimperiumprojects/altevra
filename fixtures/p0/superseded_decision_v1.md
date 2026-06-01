---
id: dec_fixture_super_v1
type: decision
title: Use Postgres for P0
domain: project
sensitivity: internal
status: superseded
superseded_by: dec_fixture_super_v2
categories: [architecture, storage]
---

## Decision

(v1 — SUPERSEDED) Use Postgres + pgvector for P0.

## Rationale

Later reversed: see v2. This object must be EXCLUDED from default retrieval and
appear only under a *_history intent (I3).
