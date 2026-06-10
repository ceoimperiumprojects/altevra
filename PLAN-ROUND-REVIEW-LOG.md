# Plan Review Log: Zaokruživanje — Compounds-for-Years Round
Act 1 (grill) complete — plan locked with Pavle. MAX_ROUNDS=5. PLAN_FILE=PLAN-ROUND.md. Codex lane: LIVE (personal Plus OAuth).

## Round 1 — Codex (LIVE, personal Plus OAuth)

Findings:

- **R2 Embeddings:** Plan assumes `nomic-embed-text` 768d via OpenAI-compatible embeddings, but current local embedder is BGE-M3 1024d behind the `embedding` feature, while OpenAI-compatible provider is chat-only. See [bge.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/bge.rs:1), [openai_compat.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-llm/src/openai_compat.rs:1). Fix: choose BGE-M3/1024d now, or explicitly add an Ollama embedding provider plus migration/dim tests.

- **R2 Brain Integration:** Plan says ongoing embeddings run “as part of brain,” but embedding is a separate `altevra embed` command using Gemini/NoOp today. See [embed.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-cli/src/commands/embed.rs:1). Fix: add `altevra-embed.service` or wire `EmbedderWorker` into `BrainScheduler`.

- **R2 Backfill Scope:** Existing queue only handles `memory_chunks`; turns, personal notes, research summaries, and wiki pages are not naturally chunked into `memory_chunks`. Fix: define one canonical embedding object model before “embed ALL existing content.”

- **R2 Dim Safety:** `memory_chunk_vectors_v2` stores `dim`, but `search_by_vector` loads all vectors and cosine-scores without filtering/refusing mixed dims. Fix: enforce `model+dim` at query and write gates before any 768/1024 transition.

- **R3 Import:** Plan frames Claude/Codex import as absent, but `analyze` already parses both; `import --tool` only supports Hermes. See [orchestrator.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-cli/src/commands/analyze/orchestrator.rs:71), [import.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-cli/src/commands/import.rs:94). Fix: reuse/graduate `analyze` into `import --tool claude-code|codex` instead of building a second importer.

- **R3 Working Dir:** Plan says Claude `working_dir` from encoded project path, but the current parser uses `cwd` for project hint and imports `working_dir = None`. Fix: add `working_dir` to `ImportedSession` and preserve `cwd`/decoded path through insert.

- **R3 Dry-Run:** Existing `analyze --dry-run` returns discovery only, not projected DB bytes/turn counts, which matters on a 91%-full disk. Fix: dry-run must parse/count/truncate-estimate without writing.

- **R3 Codex Schema:** Plan assumes `state_5.sqlite` `created_at` epoch ints, but current parser expects string RFC3339 and only queries `id,title,project,created_at`. Fix: support int/string timestamp variants and schema probing fixtures.

- **R4 Events:** Plan says “today only session boundaries emit,” but pre/post tool hooks already record `tool_call`/`tool_result` as turns and emit skill invocation/reaction events. See [hook_handle.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-cli/src/commands/hook_handle.rs:476). Fix: decide whether observer consumes turns or events; do not duplicate full tool payloads into both.

- **R4 Event Volume:** Emitting every `PostToolUse` as an event can double-write large tool outputs beyond the already guarded turn table. Fix: emit metadata-only event rows with pointers to turn ids, not payload copies.

- **R4 Detector Mismatch:** Current observer detectors are event-only and project-stale requires `project_id`, but hook-created session events have no project id. Fix: add DB-backed detectors over `sessions/turns`, or enrich events with project/session entity ids.

- **R4 Retention:** Plan says event retention is P0 lifecycle-bounded, but lifecycle archiver targets `object_index` and ephemeral `context_packets`, not raw `events` pruning. Fix: add explicit event retention job/policy before high-volume events.

- **R1 Systemd:** One brain service is insufficient if embeddings remain separate; backup also needs coordination with live WAL writers and stale maintenance locks. Fix: generate units for brain, embedder, and backup with explicit DB path, HOME, WorkingDirectory, lock behavior, and stale-lock handling.

- **R1 Backup:** `VACUUM INTO` while brain/embedder are active can contend; “refuses while maintenance lock held” does not stop ordinary writers. Fix: backup should acquire its own backup lock or pause writers, checkpoint safely, and verify restore.

- **R5 Write-Back:** Existing guarded writer protects whole generated skill files with `ALTEVRA_MANAGED: true`, not arbitrary managed blocks inside human-owned `CLAUDE.md`. Fix: implement block-level hash/manifest keyed by file+marker range before touching user memory files.

- **R5 Safety:** Allowlist includes `~/.claude/CLAUDE.md` write-back, but current repo has many adapter-generated managed headers, not a proven “human edits win around managed block” memory writer. Fix: build block parser tests for missing markers, duplicate markers, nested markers, CRLF, and manual edits inside/outside block.

- **R6 Skill Install:** Plan assumes first live install into `~/.claude/skills/`; current guarded skill sync is file-level and repo/tool-target oriented. Fix: run a dry-run plan proving target path, manifest row, backup path, and fresh-Claude load mechanism before `--apply`.

- **R6 Doctor:** Current doctor checks vault/init/Claude files/spool only; it does not check service active, backup freshness, embedding lag, unimported history, or live skill visibility. Fix: extend doctor before making it the R6 acceptance gate.

- **R7 Release:** Bootstrap reports Altevra `0.3.0`, while plan tags `v0.4.0-alive`; version/changelog update is not listed. Fix: add explicit workspace version and changelog step before tag.

- **Ordering:** R2 before R3 means full backfill runs, then import adds tens of thousands of new turns requiring another major backfill. Fix: do R0/R1 minimal safety, R3 import dry-run+real import, then R2 one full backfill.

VERDICT: REVISE
### Claude's response (final arbiter)
ACCEPTED all 20 findings — every one verified the actual code (BGE-M3 1024d exists behind feature flag, openai_compat chat-only, analyze parses both formats, tool calls land as turns, lifecycle doesn't prune events, writer is file-level, doctor minimal, version 0.3.0). PLAN-ROUND.md revised: order R3-before-R2 (one backfill); embedder = existing BGE-M3 first with measured Ollama fallback + model+dim write/query gates; import = graduated analyze parsers + working_dir threading + int/string codex timestamps + dry-run disk projection; observer = DB-backed detectors over sessions/turns + metadata-only pointer events + explicit retention job; R1 = three units + backup self-lock + restore verification + stale-lock handling; R5 = NEW block-level writer with marker-range manifest + full edge suite; R6 doctor extension explicit; R7 version bump + changelog. REJECTED: none.

## Round 2 — Codex

Re-review result: the revised plan is much stronger, but I still see material gaps worth fixing before approval.

- **R1 / Goal mismatch:** The goal still says backups are “pushed off-machine,” but R1 only defines local `~/.altevra/backups/auto` rotation. One-line fix: add a Pavle-gated remote backup leg, e.g. restic/rclone/Tailscale target, with restore smoke, or explicitly remove off-machine backup from the goal.

- **R1 / Backup locking:** “Take maintenance lock briefly” is unsafe/ambiguous for `VACUUM INTO`; if writers resume during snapshot/checkpoint, the guarantee is unclear. One-line fix: hold the writer-pausing lock for the entire snapshot+verify window, or use SQLite online backup semantics and document exactly which writes can proceed.

- **R2 / BGE feature activation:** `altevra-cli` depends on `altevra-memory` without enabling the `embedding` feature, and `Bge3Embedder` is `#[cfg(feature = "embedding")]`. A normal `altevra embed run` binary cannot use BGE-M3 yet. One-line fix: add a CLI/workspace feature that enables `altevra-memory/embedding`, and make R2 gates include the exact build/test/service command using it.

- **R2 / Synthetic DB embeddings:** `pending_indexing` and `memory_documents.source_path` are file-path oriented; turns/wiki/personal_notes/research rows are DB objects, not files. “Via existing ingest path” is still under-specified and risks fake paths, bad invalidation, or uniqueness collisions. One-line fix: define a stable synthetic document contract (`source_path = db://turn/<id>` or separate vector object table), checksum rules, and re-embed invalidation before backfill.

- **R7 / Version bump:** The plan says workspace version bump, but crates like `altevra-cli` have explicit `version = "0.3.0"` rather than inheriting workspace version. One-line fix: require bumping every published/runtime crate version that reports into bootstrap, not just `[workspace.package]`.

VERDICT: REVISE
### Claude's response (final arbiter)
ACCEPTED all 5: goal wording honest (local rotated + optional Pavle-gated remote leg via rclone, OFF by default); backup holds writer-pausing lock for ENTIRE snapshot+verify window; CLI embedding feature wiring is an explicit deliverable with exact build/run/service commands in the gate; synthetic document contract (db://turn/<id> URIs, sha256 checksum invalidation, URI uniqueness) defined before backfill; version bump per-crate grep-verified. REJECTED: none.

## Round 3 — Codex — VERDICT: APPROVED

Re-reviewed `PLAN-ROUND.md`. The five round-2 blockers are now materially addressed:

- Off-machine wording is honest, with local restore-verified backups as the baseline and an optional Pavle-gated remote leg.
- Backup now holds the writer-pausing maintenance lock for the full snapshot and verify window.
- BGE-M3 activation now explicitly calls out the missing CLI feature wiring and requires exact build/service commands in the gate.
- DB-object embeddings now have a concrete `db://...` synthetic document contract with checksum invalidation.
- Version bump now covers every crate declaring `0.3.0`, not just workspace metadata.

I do not see another plan-level blocker worth forcing another revision. Remaining risks are implementation risks already called out in the plan: BGE CPU speed, import volume, backfill duration, detector noise, and block-writer blast radius. Those are acceptable with the gates now listed.

VERDICT: APPROVED
## Resolution: APPROVED after 3 rounds (20 + 5 + 0 findings; all accepted, 0 rejected)
Live cross-model review (GPT-5.5, personal Plus OAuth). Remaining risks are implementation risks with gates defined. Awaiting Pavle's implement sign-off.
