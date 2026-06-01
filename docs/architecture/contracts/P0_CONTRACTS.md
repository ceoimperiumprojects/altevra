# Altevra P0 Contracts

Status: synthesized-by-Hermes / **reconciled by `../RECONCILIATION.md` (R1–R11)**
Date: 2026-06-01 (reconciled)
Source: `../ALTEVRA_ARCHITECTURE_WORKING_DRAFT.md` + `../ALTEVRA_ARCHITECTURE_REVIEW_LOG.md` + `../RECONCILIATION.md`

> **Authority note:** Where this file and `RECONCILIATION.md` differ, RECONCILIATION wins (it resolves the cross-section contradictions). Sections below are updated to match the locked decisions.

## 1. Ratified decisions

- P0 is SQLite/local-first. Postgres/pgvector/cloud are future adapters.
- Obsidian is human-readable face; DB is normalized machine truth.
- Every durable object uses the common envelope.
- Pre-write safety and exposure gates are mandatory.
- Context packets, not raw stores, are the primary human/agent interface.
- Self-improvement is proposal-only in P0.
- Tool/skill registry is read/health/proposal first; auto-install/update deferred.
- Generated Obsidian content is hidden/contained by default.

## 2. Core status families (RECONCILED — R2)

Six **separate** enums, never overloaded into one column (`quarantined` corrected out of `ObjectStatus` — it is a `RedactionStatus`):

- `ObjectStatus` (lifecycle): `draft`, `active`, `superseded`, `archived`, `forgotten`, `deleted_tombstone`.
- `RedactionStatus` (safety, separate column on text-bearing rows): `unscanned`, `clean`, `redacted`, `quarantined`, `rejected`.
- `ReviewStatus`: `not_required`, `pending_review`, `approved`, `rejected`, `needs_changes`, `expired`.
- `LifecycleState` (**derived, not stored** except `archived`): `fresh`, `due_for_review`, `expired`, `archived`, `retention_due`, `delete_due`, `legal_hold`.
- `CapabilityState` (§5 installed component, computed by `verify`): `discovered`, `installed`, `current`, `outdated`, `drifted`, `broken`, `disabled`, `needs_review`, `missing`, `conflicted`, `unsupported`.
- `ProposalStatus`: `proposed`, `triaged`, `approved`, `applied`, `rejected`, `superseded`, `withdrawn`, `deprecated`.

Rule: never overload one enum for all workflows; link families through invariants. All enums carry `Other(String)` tolerant parse (no panic on unknown).

## 2b. Sensitivity model (RECONCILED — R1)

- **One canonical 6-level total-ordered ladder** everywhere: `public < shareable < internal < confidential < secret < restricted`.
- Sensitivity = `sensitivity_level` (the ladder) **+** orthogonal `domains[]` (R3) **+** orthogonal `risk_tags[]` (`financial, health, relationship, legal, credential, identity, minor, third_party_pii`).
- **Ceiling math (`≤`) touches ONLY `sensitivity_level`.** `domains` and `risk_tags` are separate gate conditions. `combine()` = `max(level), ∪ domains, ∪ tags` (monotone).
- `secret` level is credential-class only — no durable object body resolves to `secret` (only `secret_sighting` fingerprints); else → quarantine.
- Live `altevra-core/src/security.rs` `Sensitivity` enum is extended from 4 → 6 levels additively (old strings still parse).

## 3. Mandatory gates

### PreWriteSafetyGate

Inputs: caller, purpose, raw payload, intended object type, intended domain, intended source-of-truth class.
Outputs: allow/quarantine/reject, redacted payload, sensitivity, detected risk labels, review requirement, audit ref.

Must run before durable persistence of text/payload fields.

### ExposureGate

Inputs: caller, purpose, object refs/revisions, max sensitivity, domain scope, target channel/tool, requested packet profile.
Outputs: allowed items, redacted forms, exclusion explanations that do not leak protected existence, audit ref.

Must run before retrieval ranking, tool output, MCP response, prompt packet, Obsidian export, or cloud sync.

### Human-presence gate (RECONCILED — R4)

Every protected approval (`review approve/reject`, `connect`, `grant approve`, `forget --execute`, `domain set-policy`, `legal-hold`, `export --raw`) requires a **human-presence proof**: TTY (`std::io::IsTerminal`) or a one-shot `ALTEVRA_UNLOCK` token. Non-TTY/agent callers are refused (`requires_human_presence`). **No MCP/agent caller has any approve/apply path** (HP-1). `"approved"` is never an accepted input field (HP-2).

### Context packet vs audit split (RECONCILED — R5)

`context_packet` (compiled body/items) is **ephemeral** (auto-purge 14d, regenerable). `exposure_decision` (request + included/excluded refs + reasons + ceiling) is **append-only audit, never purged**. Purging a packet body never touches its audit row (R5-INV).

## 4. Source-of-truth classes

- `db_canonical`: DB row is truth; markdown is generated mirror.
- `markdown_authored`: human markdown can be source; import/reconcile creates normalized objects.
- `generated_mirror`: never edited as truth; edits become review items.
- `imported_evidence`: immutable/raw-ish evidence, always sensitivity gated.
- `derived_summary`: regenerated from refs; must cite source refs/revisions.

## 5. Domain/sync defaults

Protected by default: relationship, health, legal, financial, credentials/secrets, identity/policy/schema.
Cloud sync starts disabled for protected domains unless explicitly policy-approved.
Public/shareable is lowest sensitivity but still provenance/audit-backed.

## 6. P0 non-negotiables

- No real secrets in fixtures.
- No silent external side effects.
- No broad auto-apply self-improvement.
- No generated top-level Obsidian clutter.
- No retrieval result without source refs and inclusion/exclusion explanation.
