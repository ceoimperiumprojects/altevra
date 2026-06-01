---
id: dec_fixture_super_v2
type: decision
title: Use SQLite for P0 (supersedes Postgres decision)
domain: project
sensitivity: internal
status: active
supersedes: dec_fixture_super_v1
categories: [architecture, storage]
---

## Decision

(v2 — ACTIVE) Use SQLite local-first for P0. Supersedes dec_fixture_super_v1.

## Rationale

Local-first axiom + the live code is already SQLite. This is the only version a
default decision_lookup should return.
