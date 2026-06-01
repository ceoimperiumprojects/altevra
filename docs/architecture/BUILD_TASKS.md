# Altevra Build Tasks — Granular Execution Plan (full vision)

Status: **ready-to-execute / pending Pavle sign-off on RECONCILIATION.md**
Date: 2026-06-01
Author: Claude (Opus 4.8, 1M)
Inputs: working draft §1–§7, `RECONCILIATION.md` (locked decisions R1–R11), live code map (15 crates, 18 migrations, v0.3.8).

> This is the complete task breakdown from "contracts done" to "full second-brain vision," mapped onto the **existing** codebase. Every task names the crate/migration/file it touches, its dependencies, and its acceptance test. Tasks are atomic enough to commit one at a time. Phase gates are hard: a phase isn't done until its baseline is green (`cargo fmt --check && cargo test && cargo build && cargo clippy --workspace -- -D warnings`).
>
> Legend: **[NEW]** new file/table · **[EXT]** extends existing code · **[FIX]** corrects a contract doc · dep = depends-on task id.

---

## Phase map (what each phase delivers + what already exists)

| Phase | Delivers | Foundation already in code |
|---|---|---|
| **P0.0** | Locked contracts, core enums, fixtures, validation harness | `Sensitivity` enum, repo pattern, roundtrip test harness |
| **P0.1** | Vertical loop: capture→gate→persist→mirror→packet→audit→review | `altevra-secrets` (9 detectors), `altevra-vault`, repo layer, MCP/CLI shells |
| **P0.2** | Capability/tool registry (read-only honesty) | `ToolAdapter` ×4, `installed_components`, `VersionCheckResult` |
| **P0.3** | Control plane: CLI/MCP verbs for new objects | 23 CLI cmds, 36 MCP tools, bootstrap packet |
| **P0.4** | Full retrieval: vector index + full golden eval | `embedder_queue`, `memory_chunk_vectors_v2`, BM25 turn search |
| **P0.5** | Resident runtime (dry-run, proposal-only) + model routing | `altevra-brain` scheduler+JobKind+brain_jobs, `altevra-llm` ChatMessage/RateLimiter |
| **P0.6** | Self-improvement loop + runaway firewall | scheduler dispatch pattern, research relevance gate |
| **P0.7** | Skill factory (propose→render→install→monitor) | `altevra-skills` registry, adapter render path |
| **P0.8** | Domains + lifecycle + project compounding + RTBF | research project budgets, identity registry |
| **P0.9** | Cloud-sync prep (policy only) | per-project state, sensitivity ladder |
| **P1+** | Embeddings-per-sensitivity, signed unlock, full sync substrate, dashboard | — |

---

## P0.0 — Contracts & core types (no behaviour, all type-level)

Gate: contracts locked, core enums compile, fixture vault exists, validation harness runs.

- **T0.1 [FIX]** Apply `RECONCILIATION.md` rulings into `contracts/P0_CONTRACTS.md`: correct §2 status families (remove `quarantined` from `ObjectStatus`, add `RedactionStatus` row — R2), state the 6-level ladder + level/domain/tag split (R1), the human-presence mechanism (R4), the packet/audit split (R5). dep: —
- **T0.2 [FIX]** Stamp `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md` with a superseded-by-SQLite header (R10). dep: —
- **T0.3 [EXT]** `altevra-core/src/security.rs`: extend `Sensitivity` → 6 levels (`Public, Shareable, Internal, Confidential, Secret, Restricted`) + `Other(String)`; impl total `Ord`, `Display`, `FromStr` (back-compat parse of old 4). Unit test: ordering + roundtrip + `Other` tolerance. dep: T0.1
- **T0.4 [NEW]** `altevra-core/src/domain.rs`: `Domain` enum (9 governed + `Other`), `RiskTag` enum (8). `Display`/`FromStr`/tests. dep: —
- **T0.5 [NEW]** `altevra-core/src/status.rs`: `ObjectStatus`, `RedactionStatus`, `ReviewStatus`, `LifecycleState`, `CapabilityState`, `ProposalStatus` enums (R2). Each with allowed-transition fn + `Other(String)` + tests. dep: T0.1
- **T0.6 [NEW]** `altevra-core/src/envelope.rs`: `Envelope` struct (all §1.3 fields) + `Provenance` struct (§1.4.6) + `trait HasEnvelope`. Serde to/from JSON + frontmatter-key mapping. Tests. dep: T0.3, T0.4, T0.5
- **T0.7 [NEW]** `fixtures/p0/` synthetic vault: `project_decision_publicish.md`, `personal_health_sensitive.md`, `secret_looking_payload.txt` (fake `sk-`/`ghp_`/`AKIA`/JWT/`postgres://`), `human_edit_generated_mirror.md`, `superseded_decision_v1|v2.md`, `prompt_injection_capture.md`. **No real secrets.** dep: —
- **T0.8 [NEW]** `altevra-core/tests/contract_validation.rs`: harness asserting enum value sets match `P0_CONTRACTS.md` (golden string lists), so contract drift is a visible test failure. dep: T0.3–T0.5
- **T0.9 [NEW]** (R13) `altevra-core/src/template.rs`: `Template` durable type + builtin templates (skill, hook, wiki_page, daily_brief, decision, learning, person, preference, insight_card) — required frontmatter keys + body sections + required tag slots. Tests. dep: T0.6
- **T0.10 [NEW]** (R13/TAG-1) `altevra-core/src/template/gate.rs`: `TemplateGate` — validates a write against its type template + enforces mandatory `domain` + ≥1 governed category. Returns conformance result or quarantine reason. dep: T0.9, T0.4

---

## P0.1 — The vertical loop (the spine)

Gate: `altevra p0-vertical-smoke --fixture fixtures/p0 --json` passes; envelope conformance meta-test green; P0.1 golden subset (R6) green; **Codex safety breaker re-run (R11) before merge of T1.4/T1.8**.

### Schema
- **T1.1 [NEW]** `migration 019_object_envelope.sql`: additive `ALTER TABLE ADD COLUMN` for envelope fields on all durable tables (R8.1) + backfill defaults. dep: T0.6
- **T1.2 [NEW]** `migration 020_relations.sql`: `relations` edge table (§1.6); migrate `wiki_page_links` data in. dep: T1.1
- **T1.3 [NEW]** `migration 021_object_index.sql`: denormalized `object_index` (R8.3) + triggers/repo-side maintenance. dep: T1.1
- **T1.4 [NEW]** `migration 022_safety.sql`: `secret_sighting` (fingerprint/metadata, **never value**), `redaction_manifest`, `exposure_decision` (append-only audit, R5), `audit_log` (append-only, §2.10) + `redaction_status` column on text-bearing tables. dep: T1.1
- **T1.5 [NEW]** `migration 023_context_packet.sql`: `context_packet` (ephemeral) + `context_packet_sources`. dep: T1.1
- **T1.6 [EXT]** `migration 024_decision_status.sql`: add `status` to `decisions`, backfill `active` (R10). dep: T1.1

### PreWriteSafetyGate (ingest_guard) — builds on `altevra-secrets`
- **T1.7 [EXT]** `altevra-secrets/src/detector.rs`: add PII detectors (email, phone, IBAN/card, SSN-like) alongside the existing 9 secret detectors. Tests against fixture corpus. dep: —
- **T1.8 [NEW]** `altevra-core/src/safety/ingest_guard.rs`: the single pre-write choke point. Pipeline pattern→entropy→classify→**TemplateGate (T0.10)**→act→audit (§2.5 + R13). Returns `Guarded{value, redaction_status, manifest_ref, sensitivity, risk_tags, template_ok, tags}`. Uses `altevra-secrets` detector+redactor+store. Emits `secret_sighting` + `audit_log`. Fail-closed: `unscanned` never survives commit (I1); untagged/non-template → quarantine (TAG-1/TEMPLATE-1). dep: T1.4, T1.7, T0.6, T0.10
- **T1.9 [NEW]** `altevra-core/src/safety/classify.rs`: sensitivity classifier (rule-based for P0: frontmatter `sensitivity` → domain default → default-up). LLM classification is P0.5. dep: T0.3, T0.4
- **T1.10 [NEW]** `altevra-core/src/presence.rs`: `require_human_presence` (TTY via `IsTerminal` + `ALTEVRA_UNLOCK` token, R4). Invariants HP-1/HP-2. Tests (non-TTY refused). dep: —

### ExposureGate
- **T1.11 [NEW]** `altevra-core/src/safety/exposure_gate.rs`: the single read/exposure path. Monotone intersection (R1: level `≤` + `domains ⊆` + `redaction ≥ min` + audience + packet_eligible). Sensitivity-aware reason codes (no existence leak, §2.13). Writes `exposure_decision`. dep: T1.4, T0.6
- **T1.12 [NEW]** repo methods for `relations`, `object_index`, `secret_sighting`, `exposure_decision`, `context_packet` in `altevra-db/src/repositories/`. dep: T1.2–T1.5

### Persist + mirror + packet
- **T1.13 [EXT]** `altevra-db` repositories: route every durable write through `ingest_guard` (T1.8); populate envelope columns; maintain `object_index`. dep: T1.8, T1.12
- **T1.14 [EXT]** `altevra-vault`: add managed-header + envelope frontmatter to written docs; honor `mirror_to_markdown=false` for `confidential+` (§2.14 D4); only "both/markdown" families get a face. dep: T1.13
- **T1.14b [NEW]** (R12/R13) `altevra-db`: FTS5 virtual tables over `title+body+tags`; tag/structured index over `object_index`. This is the **primary** retrieval substrate (no vectors). dep: T1.3
- **T1.15 [NEW]** `altevra-core/src/packet/compiler.rs`: deterministic **tag/structured + BM25(FTS5) + graph + recency** compiler — **NO vector path (R12)**. Two-layer scoring (gates ≠ weights, §3.3); relevance = `bm25 + tag_match + graph + recency`. Calls `exposure_gate` **before ranking** (§2.13). Whole-item packing, reserves, pointer-only must-includes (§3.8). Emits `context_packet` + `exposure_decision`. dep: T1.11, T1.12, T1.14b
- **T1.16 [NEW]** `WhyIncluded`/`ExclusionRecord` on every item (§3.5). dep: T1.15

### Review + smoke
- **T1.17 [EXT]** `altevra-db` `review_items`: envelope upgrade; `create_review_item` writes proposal-grade rows; approval path gated by `require_human_presence` (T1.10). dep: T1.10, T1.1
- **T1.18 [NEW]** `altevra-cli` `p0-vertical-smoke` command: runs the full loop on `fixtures/p0`, emits deterministic JSON. dep: T1.13–T1.17
- **T1.19 [NEW]** `altevra-db/tests/`: envelope conformance meta-test (R8.2); P0.1 golden subset (R6); leak suites (secret=0, scope=0, sensitivity=0); supersession/illegal-transition/import-idempotency tests (§1.14). dep: T1.13–T1.16
- **T1.20 [GATE]** Re-run Codex security + implementation breakers on the safety crate (R11) before merging T1.8/T1.11. dep: T1.8, T1.11

---

## P0.2 — Capability / tool registry (read-only honesty)

Gate: capability honesty test (T7 — `supported` requires `evidence_ref`); drift detection wired; cross-surface parity (CLI plan == MCP plan).

- **T2.1 [NEW]** `migration 025_capability.sql`: `adapter_dossier`, `capability_record`, `skill_proposal`, `capability_grant` (full envelope, R9). Fold `skill_installations` → `installed_component(component_type=skill)` (R10). dep: T1.1
- **T2.2 [NEW]** `altevra-core/src/capability.rs`: `TrustLevel` ladder (`none<read<propose<render<install<execute`), `Support` enum, `CapabilityState` machine (R2). dep: T0.5
- **T2.3 [EXT]** `altevra-skills/src/registry.rs`: map `installed_component.status` to the computed `CapabilityState` machine (§5.3) — derived by `verify`, never asserted (T8). Handle `Ahead→conflicted` (R10). dep: T2.2
- **T2.4 [EXT]** `altevra-adapters`: emit `capability_record` with `evidence_ref` from `verify()` runs (T7 honesty). Build `adapter_dossier` per tool (the V5 capability matrix as a durable object). dep: T2.1, T2.2
- **T2.5 [EXT]** `altevra-watcher`: populate `before_hash` (currently always `None`) + add self-write marker (anti-loop, §2.7 I11) so drift detection actually fires on managed-file edits. dep: T1.14
- **T2.6 [NEW]** 3-way-diff drift reconciliation → `review_item` (§5.4.3, T4 no silent overwrite). dep: T2.5, T1.17
- **T2.7 [NEW]** tests: capability honesty, component-state machine (each `VersionCheckResult` × disk condition), drift→review, no-secret-in-render (reuse fixture corpus). dep: T2.3–T2.6

---

## P0.3 — Control plane (CLI/MCP verbs for new objects)

Gate: every new object has a thin CLI verb + MCP read tool, all routed through `exposure_gate`; MCP has NO approve/apply path (HP-1).

- **T3.1 [NEW]** CLI verbs: `altevra sot show`, `redact check`, `review list|show|approve|reject`, `forget --dry-run`, `audit query`, `capabilities show|verify`, `adapter list|dossier`, `grant list|show|approve|revoke`, `component list`. dep: P0.2
- **T3.2 [EXT]** `altevra doctor` (exists): add checks for unscanned rows, orphan embeddings, drift backlog, secrets-dir perms (0600), audit-chain, `mirror_to_markdown` violations (§2.16). dep: T1.4, T2.5
- **T3.3 [EXT]** `altevra-mcp/src/server.rs`: ensure `get_context_packet`/`search_memory`/`get_source_of_truth`/`get_capabilities` enforce ceiling via `exposure_gate`; add `request_forget`, `propose_improvement` (write review_item only). **Verify no approve/apply tool exists** (HP-1 test). dep: T1.11, T1.17
- **T3.4 [NEW]** tests: MCP caller ceiling enforcement; HP-1 (no agent approve path); cross-surface parity. dep: T3.1–T3.3

---

## P0.4 — Retrieval hardening (NO vectors — R12 moved them to P1)

Gate: complete golden eval (the non-embedding cases, R12) green; determinism byte-equal; CLI/MCP packet parity (INV-14). **Vector/embedding retrieval is REMOVED from P0 (R12) — it's P1 optional.**

- **T4.1 [EXT]** `altevra-memory`: route ingest through `ingest_guard` (redacted text only into chunks, I2). Chunks feed FTS5, not embeddings. dep: T1.8
- **T4.2 [NEW]** retrieval profiles (intent→weights over `bm25 + tag_match + graph + recency`) as versioned config (§3.3). dep: T1.15
- **T4.3 [NEW]** multilingual via FTS5 analyzer + mandatory tags (SR+EN), NOT embeddings (R12). dep: T4.2
- **T4.4 [EXT]** parent-scope hierarchy (R7) activated in scoring once `project` table exists (P0.8). dep: T8.x
- **T4.5 [EXT]** `altevra doctor`: tag-coverage + template-conformance audit (TAG-1/TEMPLATE-1 health). dep: T0.10

---

## P0.5 — Resident runtime (dry-run, proposal-only) + model routing

Gate: dry-run resident emits schema-valid, review-routed proposals; **no external model calls** (noop provider); vertical loop test (§4.17) green.

- **T5.1 [EXT]** `altevra-llm`: introduce `trait ChatProvider`/`EmbeddingProvider`; **role routing** (`cheap_worker/strong_reasoner/local_private/embedding/reranker/none`); **noop/stub provider** (R10); keep `GeminiFlashChat` as one impl; reuse `RateLimiter`. dep: —
- **T5.2 [NEW]** `migration 026_resident.sql`: extend `brain_jobs`→`resident_run` columns (R10); `resident_mode` registry, `resident_budget`. dep: T1.1
- **T5.3 [EXT]** `altevra-brain`: generalize `JobKind`→`resident_mode` registry (keep existing 10 jobs working); resident dispatch reuses scheduler `tick()`/`dispatch()` pattern. dep: T5.2
- **T5.4 [NEW]** resident runtime contract (§4.4): input = context packet (T1.15), output schema-validated, every run a `resident_run` row, self-write exclusion (SI-6), role↔ceiling (SI-7). dep: T5.1, T5.3
- **T5.5 [NEW]** tests: dry-run mode, schema-invalid output → failed+zero writes (SI-14), personal_data_allowed⇒local_private (SI-7). dep: T5.4

---

## P0.6 — Self-improvement loop + runaway firewall

Gate: runaway-prevention suite green (budget/circuit-breaker/cap/cooldown/eval-gate/lock/kill); Tier-2 cannot auto-apply (SI-2).

- **T6.1 [NEW]** `migration 027_proposals.sql`: unified `proposal` (`kind` discriminator, R10), `improvement_signal`, `prompt`, `prompt_eval_result`. dep: T1.1
- **T6.2 [NEW]** `altevra-core/src/selfimprove/`: 7-stage loop (§4.5) capture→cluster→detect→gate→apply→monitor→retire. Evidence-bound (min_evidence). dep: T6.1, T5.4
- **T6.3 [NEW]** risk-tier deriver (pure fn, SI-9) + write-authority matrix (§4.6). dep: T6.2
- **T6.4 [NEW]** runaway firewall in Rust below the LLM (§4.7): budgets, circuit breaker (SI-11), Tier-0 cap (SI-12), dedup+cooldown (SI-13), shadow eval gate (SI-10), constitutional lock (SI-2), kill switches. dep: T6.3
- **T6.5 [NEW]** prompt registry + layered render + rollback (§4.8); `safety`/`altevra_rules` constitutional-locked. dep: T6.1
- **T6.6 [NEW]** tests: full runaway suite (§4.17) incl. prompt-injection-cannot-change-gate (SI-15). dep: T6.4

---

## P0.7 — Skill factory (the compounding loop)

Gate: fixture repeated workflow → exactly one deduped `skill_proposal` → approve → renders a complete skill to target dir; usage monitored.

- **T7.1 [EXT]** `skill_factory_proposer` resident mode (detects repeated workflow from hook turns). dep: P0.6
- **T7.2 [EXT]** post-approval render path (§5 owns): `skill_proposal(applied)` → `skill` object → `ToolAdapter` render → `installed_component` (T6 no-secret-in-render). dep: T7.1, P0.2
- **T7.3 [EXT]** Hermes `ToolAdapter` (R10 Q7): skills → `~/.imperium/skills/shared/`. dep: T7.2
- **T7.4 [NEW]** usage tracking + deprecate-when-stale (§4.5 stage 7). dep: T7.2
- **T7.5 [NEW]** tests: dedup, render completeness, cross-agent grant review-gate (T9). dep: T7.2

---

## P0.8 — Domains, lifecycle, project compounding, RTBF

Gate: 9-builtin domain seed golden; lifecycle dry-run; export/forget/legal-hold vertical loop; cross-domain leak=0.

- **T8.1 [NEW]** `migration 028_domains.sql`: `domain_policy` (§6.2) + seed 9 builtins (§6.4 matrix golden). dep: T1.1, T0.4
- **T8.2 [NEW]** `migration 029_projects_personal.sql`: `project` (+`parent_id`, R7), `person`, `relationship`, `preference`, `event_log_personal`, `learning`, `insight_card`. dep: T1.1
- **T8.3 [NEW]** policy resolution (D1/D2 snapshot-at-create, most-restrictive multi-domain, R3) wired into `ingest_guard`/`exposure_gate`. dep: T8.1, T1.8, T1.11
- **T8.4 [EXT]** `altevra-brain` lifecycle job (§6.6): staleness derive, soft-archive (standard), ephemeral purge (D6 fence, R-EPH horizons), review-gate destructive/policy. Purges only `context_packet` bodies, never `exposure_decision` (R5-INV). dep: T8.1, T5.3
- **T8.5 [NEW]** project lifecycle: archive-demotion (D5 scope multiplier → §3), scope-promotion (D8 additive), provenance compaction (turns→archived). dep: T8.2, T4.5
- **T8.6 [NEW]** export (`altevra export`, sovereignty) + forget/RTBF (consumes §2.8 pipeline) + legal-hold precedence (D7), all human-presence gated (R4). dep: T8.1, T1.10
- **T8.7 [NEW]** Imperium-vault generated_mirror writer (Q-VAULT, R10): Altevra writes `~/Obsidian/Imperium/` only as generated_mirror. dep: T1.14
- **T8.8 [NEW]** tests: §6.12 suite (policy seed, default application, multi-domain, lifecycle, archive-demotion, promotion, compaction, export, RTBF+legal-hold, policy-change safety, sync ceiling, cross-domain leak=0). dep: T8.3–T8.6

---

## P0.9 — Cloud-sync prep (policy only, no daemon)

Gate: per-domain cloud_sync map enforced; restricted never in sync set; tombstone = id+hash only. **No sync daemon.**

- **T9.1 [NEW]** per-domain `cloud_sync` ceiling enforcement (§6.10, D3) in an export/sync-set selector. dep: T8.3
- **T9.2 [NEW]** tombstone-as-id+hash model + `revision`/`origin_device`/`checksum` conflict markers (no last-writer-wins, §1.12). dep: T1.1
- **T9.3 [NEW]** tests: restricted excluded from sync set; reclassified-up object held back. dep: T9.1

---

## P1+ — Beyond P0 (vision completion, not P0-blocking)

- **T-P1.1** Per-sensitivity embedding models (local_private for personal, cloud for business) → separate non-comparable vector spaces (§3.20 Q2).
- **T-P1.2** Signed passphrase-derived unlock token replacing env token (R4 P1).
- **T-P1.3** Real sync substrate decision (CRDT vs revision-vector + review-on-conflict, §1 Q7) + sync daemon.
- **T-P1.4** Audit hash-chain tamper-evidence (§2.10, deferred from P0 per §2.19#5).
- **T-P1.5** Postgres/pgvector cloud adapter (R10) for multi-device.
- **T-P1.6** Reranker stage (§3.20 Q4).
- **T-P1.7** Dashboard / focus-packet UI (deferred by every breaker).
- **T-P1.8** Encryption-at-rest for quarantined/restricted (SQLCipher vs envelope encryption, §2.19#2).

---

## Critical path (shortest route to a working brain)

```
P0.0 (enums+fixtures)
   └─▶ P0.1 (vertical loop)  ◀── the proof the whole thing works
          ├─▶ P0.2 (capability registry)
          ├─▶ P0.3 (control plane)
          └─▶ P0.4 (vector retrieval)
                 └─▶ P0.5 (resident runtime)
                        └─▶ P0.6 (self-improve + firewall)
                               └─▶ P0.7 (skill factory)
P0.8 (domains/lifecycle) can start after P0.1 schema, parallel to P0.2–P0.4
P0.9 after P0.8
```

**The one task that proves the vision:** `T1.18 p0-vertical-smoke` — capture a fixture, gate it, persist it, mirror it, compile a packet that *correctly excludes* the personal/health object with a non-leaking reason, audit the exposure, raise a review item — all deterministic, no real secrets, no network. When that JSON snapshot is green, Altevra's core law works end-to-end.

---

## Counts

- **Phases:** 10 (P0.0–P0.9) + P1 backlog.
- **Tasks:** 73 atomic tasks to full P0; ~8 P1 follow-ups.
- **New migrations:** 11 (019–029).
- **New core modules:** envelope, domain, status, capability, presence, safety/{ingest_guard,exposure_gate,classify}, packet/compiler, selfimprove/*, prompt registry.
- **Reused foundations:** secret detection (9 types), scheduler+JobKind, ToolAdapter×4, managed header, repo pattern, vault zones, embedder queue, research relevance gate, rate limiter.
