---
id: dec_fixture_001
type: decision
title: Adopt SQLite as P0 canonical store
domain: project
sensitivity: internal
categories: [architecture, storage]
tags: [sqlite, local-first]
---

## Decision

Altevra P0 uses SQLite local-first as the canonical store. Postgres/pgvector is a
future opt-in cloud adapter only.

## Rationale

The live code is already SQLite; local-first is a Constitution axiom; deferring
the cloud backend keeps P0 deterministic and offline-capable.
