# Altevra Architecture Constitution

Status: synthesized-by-Hermes / pending Pavle acceptance
Date: 2026-06-01
Owner: Pavle + Hermes

## Purpose

This constitution defines how Altevra/VVLT architecture is designed before implementation.

Altevra is a schema-first, local-first, CLI-first, MCP-compatible, Obsidian-friendly and cloud-compatible second brain/thinking OS for business, personal life, projects, agents, tools, skills, decisions, sessions, research, and self-improvement.

## Laws

### 1. Everything durable is typed

Every durable object must have stable id, type, schema_version, status, timestamps, provenance, sensitivity, domain/scope, tags, confidence where relevant, staleness/supersession where relevant, and relationships where relevant.

### 2. Capture is not exposure

Capture can be broad. Exposure must be minimal, redacted, source-backed, sensitivity-filtered, and auditable.

### 3. Markdown is human face; DB is machine truth

Obsidian markdown is readable human interface. DB rows are normalized machine truth. Object-level source-of-truth rules must define which side is canonical/editable/generated/imported.

### 4. Protected changes require review

Identity, policies, schemas, secrets, personal/relationship/health/legal/financial memory, source-of-truth decisions, broad skills, tool grants, and external actions require review before application.

### 5. Altevra improves itself

Altevra must observe its own performance, repeated corrections, ignored suggestions, retrieval/context failures, schema gaps, stale wiki/tools/prompts, and propose self-improvement through review-gated meta-proposals.

### 6. Business and personal are first-class but bounded

Business, personal, project, client, relationship, health, legal, financial, and public/shareable domains must be modeled explicitly and enforced by context packet policy.

### 7. Every contract must be testable

Architecture is not accepted unless it can be tested with fixtures, golden snapshots, state-machine checks, or smoke scripts.

## Architecture assembly protocol

Detailed protocol lives in:

- Obsidian: `Projects/z_Other_Projects/Altevra/ALTEVRA_ARCHITECTURE_ASSEMBLY_PROTOCOL_2026-06-01.md`
- Repo working draft: `docs/architecture/ALTEVRA_ARCHITECTURE_WORKING_DRAFT.md`
- Review log: `docs/architecture/ALTEVRA_ARCHITECTURE_REVIEW_LOG.md`

## Approval

This file is synthesized after Opus deep sections and Hermes fallback breaker review. Codex breaker pass remains pending until Codex workspace credits are refilled; Pavle acceptance is still required before treating this as final product law.
