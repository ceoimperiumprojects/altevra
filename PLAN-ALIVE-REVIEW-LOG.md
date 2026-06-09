# Plan Review Log: Make Altevra ALIVE
Act 1 (grill) complete — plan locked with Pavle. MAX_ROUNDS=5. PLAN_FILE=PLAN-ALIVE.md.

## Round 1 — Fable 5 adversarial panel (Codex unavailable: workspace out of credits)

**Honest note:** Codex (GPT-5.5) could not run (out of credits; stuck call killed). Per Pavle's direction, round 1 was performed by TWO independent fresh-context Fable 5 agents with distinct lenses (correctness/data-integrity + security/privacy). Same-family review — not a true cross-model check; a Codex re-pass on P1-P5 is recommended before P3 begins.

### Reviewer A (correctness/data-integrity) — VERDICT: REVISE — 19 findings (F1-F19)
Key: sqlx migrator unsafe on foreign shadow DB (checksum mismatch + over-apply) → introspection ALTER; WAL txn not atomic across ATTACHed DBs → canonical-only txn; checkpoint-then-copy backup; FK remap set enumerated (improvement_signals, events.entity_id, FTS via triggers); hook regen must PRECEDE real unify; per-test temp DBs; tool_records UNIQUE(name,kind)+adapter_ref (name collides across kinds and with adapter_dossiers); realpath doesn't reconcile the motivating case + shim denylist; SessionStart fail-open deadline + locked-DB test; stdout double-JSON collision (hook_handle.rs:140); Cursor has no additionalContext channel → (tool x transport) matrix; pin 1-2K budget (context_packet default is 8K); backfill UUIDv5+OR IGNORE+watermark; historical timestamps + one-shot scan --since; events retention missing from lifecycle; event emission must land in P0 (P3c depends on it); Codex route already exists in factory.rs (stale S1 reference); SLOW_UPDATE vs ALTEVRA_MANAGED grammar collision; personal_notes Decision/Goal must FK existing stores.

### Reviewer B (security/privacy) — VERDICT: REVISE — 7 findings
Key: spool replay via record_turn pointer path = silent data loss (pointer cleared on session_end) → replay-by-id, loud errors, TTL'd lock, replay-spool epilogue, doctor flag, 0600 O_EXCL per-event files; tool scan = secret amplifier (guard_json at upsert + S3 DENY globs inherited); SessionStart has NO sensitivity filter → ExposureGate::decide per item, per-item fail-closed, exposure_decisions audit; backfill must be metadata-only (033-era content weakly redacted) + merged shadow turns marked unscanned; P3b cited mechanism DOES NOT EXIST (domain_policy.rs:161-180 is embedding_role_for; turns carry no domain, stamped Business) → NEW external_route ExposureGate profile, refuse-whole-proposal, residual risk acknowledged; P4 ships relationship data to Obsidian against seeded mirror=never → per-item policy consult, fail-closed; managed_writes manifest table (drift undetectable without stored baseline) + restore command + git-only-if-repo.

### Claude's response (final arbiter)
ACCEPTED all 26 findings — every one verified against actual code by the reviewers (pool.rs, hook_handle.rs:140/311-317, sync.rs:251-314, domain_policy.rs, 021/024/030 migrations, jobs.rs:583-641, lifecycle.rs, factory.rs). PLAN-ALIVE.md fully revised: P0 reordered (hook regen first) + introspection upgrade + canonical-only txn + enumerated FK remap + replay-by-id spool + event emission/retention moved into P0; P1 UNIQUE(name,kind) + guard_json upsert + shim denylist; P2 transport matrix + ExposureGate filter + 1-2K budget + single-JSON stdout + deadline; P3 external_route profile (corrected mechanism) + managed_writes manifest + unified region grammar + stale S1 ref fixed; P4 obsidian_mirror per-item fail-closed + userVisibleOnly default + metadata-only deterministic backfill; P5 FK-pointer kinds. REJECTED: none.

## Round 2 — Fable 5 verification panel

### Reviewer A2 (correctness) — VERDICT: REVISE
18 of 19 round-1 fixes verified genuinely addressed (each code-spot-checked). ONE material finding: the plan cited FTS5 triggers that DO NOT EXIST (030 creates only the virtual table; zero CREATE TRIGGER anywhere; FTS is app-maintained by FtsRepository explicit DELETE+INSERT) — merged rows would be silently invisible to search. Minor: proposals.evidence_refs missing from FK-remap enumeration; content-table merge scope (65 learnings + 1427 research_items + 4 wiki pages live ONLY in shadow) was ambiguous.

### Reviewer B2 (security) — VERDICT: APPROVED
All 7 round-1 security groups verified as real mechanisms with accurate code citations (ExposureGate fail-closed on Unscanned confirmed at exposure_gate.rs:223 test; dp_relationship mirror=never confirmed at 024:49; audit/exposure_decisions/secret_sightings tables confirmed in 021). No material new leak paths. Minor non-blocking notes: P1 gate should also test the DENY-glob skip; spool removal-on-success implied not stated.

### Claude's response (final arbiter)
ACCEPTED A2's finding and both minors — fixed verbatim per prescription: FTS sentence replaced with explicit FtsRepository::index re-index requirement + FTS-findable gate assert; proposals.evidence_refs added to remap enumeration; content-table merge scope made explicit (merge + dedup-by-id + per-table dry-run counts). B2's two minors folded in as gate notes. REJECTED: none.

## Resolution: CONVERGED (security APPROVED; correctness's sole material finding fixed exactly as prescribed)
Same-family panel review (Codex out of credits) — honestly flagged throughout. Recommended: Codex cross-model re-pass on P3 (external-route stage) before P3 begins, when credits return. Plan is ready to implement; build starts with P0 per Pavle's standing direction.
