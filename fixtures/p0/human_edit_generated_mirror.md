<!-- ALTEVRA_MANAGED: true -->
<!-- source: 06-skills/example.md -->
<!-- generated_by: altevra -->
<!-- adapter: claude-code -->
<!-- version: 0.1.0 -->
<!-- checksum: 0000000000000000000000000000000000000000000000000000000000000000 -->

---
id: skill_fixture_drift_001
type: skill
slug: example-skill
version: 0.1.0
title: Example Skill
domain: business
sensitivity: internal
categories: [tooling]
---

## Trigger
When the example pattern appears.

## Steps
1. A human EDITED this generated mirror by hand (this very line).

## Commands
altevra skill list

## Pitfalls
The body no longer matches the managed-header checksum → this is DRIFT.

## Verification
The watcher must detect the checksum mismatch and route a 3-way-diff review item,
NEVER silently overwrite (§2.7 / T4).
