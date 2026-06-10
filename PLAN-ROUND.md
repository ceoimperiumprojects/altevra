# Plan: Zaokruživanje — Compounds-for-Years Round
_Locked via grill — by Claude + Pavle, 2026-06-10. Revised after live Codex round 1 (20 findings accepted)._

## Goal

Altevra's flywheel ran end-to-end once (capture → triage → gate → Codex render → staged skill). This round closes every gap between "it worked once" and "it compounds for years unattended": the brain survives reboots, all of Pavle's AI history is in the store, semantic search works over all of it, observer insights actually fire from real data, the rendered skill reaches the agents that need it, every tool's memory flows through Altevra, and the data is backed up automatically (local, rotated, restore-verified; the CODE is pushed off-machine, and an OPTIONAL Pavle-gated remote data-backup leg is specified in R1). Each stage is a verified milestone; external side effects are Pavle-gated. **Ordering corrected per review: import BEFORE embedding backfill (one backfill, not two).**

Verified current state (corrected by review): brain daemon not persistent; embeddings off — but **BGE-M3 1024d embedder already exists in altevra-memory behind the `embedding` feature** and `openai_compat` is chat-only; `analyze` already parses claude-code+codex but `import --tool` supports only hermes; tool calls/results ARE recorded (as turns) and skill_invocation/reaction events emit — observer detectors just don't consume the right shapes; lifecycle archiver does NOT prune raw `events`; the guarded writer is file-level (whole generated skill files), not block-level; doctor lacks service/backup/embedding checks; workspace version is 0.3.0.

## Approach — order: R0 → R1 → R3 → R2 → R4 → R5 → R6 → R7

### R0 — Push the branch ✅ DONE
`s0-foundation` pushed to origin (2026-06-10).

### R1 — Persistence + backup automation (systemd user units)
1. **THREE units, not one** (review): `altevra-brain.service`, `altevra-embedder.service` (embedding is a separate `altevra embed` worker today — either wire `EmbedderWorker` into `BrainScheduler` as a job and skip this unit, or generate it; decide in implementation by smaller diff, document), `altevra-backup.service` + `.timer` (daily). Each unit: explicit `ExecStart` absolute path, explicit `--db`/vault args, `Environment=HOME=…`, `WorkingDirectory`, `Restart=always`/`RestartSec=5`, journal logging; `WantedBy=default.target`; `loginctl enable-linger pavle`. **Stale-lock handling:** services check the maintenance lock with the TTL logic (never spin against a stale lock).
2. **Backup correctness** (review r2-hardened): backup acquires the writer-pausing maintenance lock and **HOLDS it for the ENTIRE snapshot+verify window** (checkpoint → `VACUUM INTO` → read-only open → `integrity_check` + count probe → release) — no writer resumes mid-snapshot; hooks spool as designed. Rotate keep-last-14. Also tar `config.toml`, `interests.yaml`, `state/`. **Optional remote leg (Pavle-gated, opt-in):** `altevra backup remote` pushing the verified snapshot via rclone/restic to a target Pavle picks (Oracle VPS / Tailscale old-laptop) — implemented as a documented hook, OFF by default; without it, off-machine durability = pushed code + local rotated data backups.
3. `altevra service install` generates + installs + enables units (idempotent, `--dry-run`, unit files mirrored to repo `deploy/systemd/`).

**Gate (hermetic):** unit-file generation tests (absolute paths, env, no CWD deps); backup lock + rotation + restore-verify logic tests. **Manual smoke (Pavle-gated):** services active; one timer-run backup verified restorable; brain survives reboot.

### R3 — Historical import arms (claude-code + codex) — BEFORE embeddings
**Graduate the existing `analyze` parsers into `import` — do NOT build a second importer** (review: `analyze/orchestrator.rs:71` already parses both formats).
1. Wire `import --tool claude-code|codex` arms onto the analyze parsers with the import pipeline's idempotency (`(tool, external_id)` non-null asserted), guard_text redaction, oldest-watermark order.
2. **`working_dir` threading** (review): add `working_dir` to `ImportedSession`; claude-code from transcript `cwd` field / decoded project-dir path; preserve through session AND turn inserts.
3. **Codex schema robustness** (review): `state_5.sqlite` query uses `id, title, cwd, created_at`; `created_at` parser accepts BOTH epoch-int and RFC3339 string variants; `history.jsonl` real fields (`session_id`/`ts`/`text`). Schema-probing fixtures for both variants.
4. **Dry-run with size projection** (review + 91%-full disk): dry-run parses and reports projected sessions/turns counts AND estimated DB bytes without writing. Pre-step: `cargo clean` stale build artifacts to free GBs; importer refuses if projected size > free space − 5GB margin.
5. Post-import: skill-candidate signals fire via the existing import path → more proposals.

**Gate (hermetic):** fixture parsers (both codex timestamp variants, claude-code cwd threading); idempotency re-run zero dups; non-null external_id; guard applied; dry-run size report. **Manual smoke:** dry-run counts on real dirs → Pavle-gated real import → `recall` finds months-old content; per-table count report sane.

### R2 — Embeddings ON + ONE full backfill (after import)
1. **Embedder choice (review-corrected):** primary = the EXISTING BGE-M3 1024d in-repo embedder (`altevra-memory/src/bge.rs`, behind `embedding` feature) — **NOTE (r2): `altevra-cli` does not enable that feature today, so the binary cannot use BGE-M3 as-is.** Deliverable: a CLI/workspace feature (e.g. `altevra-cli` feature `embedding` → `altevra-memory/embedding`) wired into the release build; the R2 gate names the EXACT commands (`cargo build --release --features embedding`, the embed-run invocation, and the service ExecStart) so activation is proven, not assumed. Benchmark on this CPU; fallback = native Ollama embeddings provider (`/api/embeddings`, nomic-embed-text 768d) ONLY if BGE-M3 is unusably slow (openai_compat is chat-only). One model, one dim, recorded in meta.
2. **Model+dim enforcement at BOTH gates** (review): `memory_chunk_vectors_v2` carries dim — add write-gate (refuse foreign dim) AND query-gate (filter `model+dim`, never cosine across mixed dims).
3. **Canonical embedding object model — synthetic document contract** (r2-hardened): `pending_indexing`/`memory_documents.source_path` are file-path oriented; DB objects get a STABLE synthetic URI — `source_path = "db://turn/<id>"`, `db://learning/<id>`, `db://note/<id>`, `db://wiki/<slug>`, `db://research/<id>` — with checksum = sha256 of the embedded text, re-embed invalidation = checksum change, uniqueness on the URI (no fake filesystem paths, no collisions with real files). Turns→turn-chunks capped per turn. Contract documented + tested BEFORE backfill starts.
4. Backfill ALL content once (post-import corpus), batched + niced + checksum/watermark resumable. Ongoing: embedder runs continuously (brain job or its own unit per R1 decision) so new turns embed within minutes.
5. Hybrid search: BM25 + vector union with rerank; lexical fallback when vectors missing.

**Gate (hermetic):** write/read-back at the chosen dim; mixed-dim write AND query refusal; hybrid returns a semantic (non-keyword) fixture hit; backfill idempotency. **Manual smoke:** Serbian semantic query over real history beats BM25.

### R4 — Observer that actually fires (consume what exists; emit only what's missing)
**Review correction: tool_call/tool_result already land as TURNS, and skill_invocation/skill_reaction events already emit.** Therefore:
1. **Detectors become DB-backed over sessions/turns** (not event-only): drift (working_dir switching), stale_projects (sessions recency — keyed by working_dir/project_name, NOT project_id which hook sessions lack), repeated tool_failure patterns (turns with error results), late-night long-session pattern (session start/end times — the "3am" vision insight), hook-failure (audit_log). Keep seeded-fixture unit tests per detector.
2. **New events are metadata-only pointers** (review): any added emission (`tool_failure`, `file_change`) carries turn-id refs + counts, NEVER payload copies (turns already hold the guarded payload — no double-write).
3. **Explicit events retention job** (review: lifecycle archiver does NOT prune events today): add a retention sweep to the lifecycle/brain job — raw/noise-class events pruned after N days, session/skill events kept.
4. Verify skill_invocation→reaction→judge loop live with one real Skill invocation.

**Gate (hermetic):** each detector fires on seeded sessions/turns fixtures; metadata-only event emission test (no payload duplication); retention prunes. **Manual smoke (the acceptance that has failed since day one):** within a day of real use, `altevra observer scan` returns ≥1 REAL insight.

### R5 — Memory sync hub (ingest + block-level ALTEVRA_MANAGED write-back)
1. Ingest with the locked allowlist (CLAUDE.md, projects/*/memory, Obsidian Decisions/Learnings; People.md LOCAL-ONLY no-write-back), DENY globs before open, guard + provenance, `--dry-run` first.
2. **Write-back requires a NEW block-level guarded writer** (review: today's writer is whole-file for generated skills): manifest keyed by `(file, marker-range)` with block hash; `<!-- ALTEVRA_MANAGED_START/END -->` markers; drift→refuse+review; backup before write; human content outside markers byte-sacred.
3. **Block-parser edge tests mandatory** (review): missing markers (append block), duplicate markers (refuse), nested markers (refuse), CRLF preservation, manual edits inside block (drift→refuse), manual edits outside block (survive byte-identically).
4. Digest content ExposureGate-filtered per destination; idempotent.

**Gate (hermetic):** allow/deny dry-run; ingest redaction; ALL block-parser edge cases; two-run idempotency with a hand-edit surviving. **Manual smoke (Pavle-gated):** one real sync into `~/.claude/CLAUDE.md` managed block, eyeballed.

### R6 — Last-meter flywheel + doctor + polish
1. **First live install with a dry-run plan FIRST** (review): `altevra skill sync --dry-run` proving target path under `~/.claude/skills/`, manifest row, backup path; Pavle reviews the staged skill; then `--apply` through the guarded writer; verify a fresh Claude session loads it (skill listed).
2. **Doctor extension is a deliverable, not an assumption** (review): add checks — brain service active, backup freshness (<48h), embedding lag (pending queue depth), unimported-history hint, installed-skill visibility, spool empty. Then doctor is the R6 acceptance gate.
3. Brief polish: local-timezone date; What-Changed/Decisions/Tasks sections wired to now-populated sources.

**Gate:** doctor 100% green on the real machine; brief renders all sections; installed skill visible to a fresh agent session.

### R7 — Version, changelog, merge + tag (Pavle-gated)
1. **Version bump 0.3.0 → 0.4.0 in EVERY crate that declares it** (r2: crates like `altevra-cli` carry explicit `version = "0.3.0"`, not workspace inheritance — bump each runtime/published crate that reports into bootstrap, grep-verified `version = "0.3.0"` → zero hits) + CHANGELOG.md entry.
2. Full `cargo test --workspace` green + doctor green + 24h stable brain service → merge `s0-foundation` → `master`, tag `v0.4.0-alive`, push.

**Gate:** Pavle's explicit merge sign-off.

## Key decisions & tradeoffs

1. **Order: import (R3) before embeddings (R2)** — one full backfill over the complete corpus instead of two (review).
2. **Embedder: existing BGE-M3 1024d first, Ollama-nomic only as measured fallback** — reuse beats rebuild; model+dim enforced at write AND query gates (the current `search_by_vector` cosine-scores blindly — that gate is part of R2).
3. **Import arms = graduated `analyze` parsers** — no second importer; `working_dir` threaded; codex timestamps accept int+string; dry-run projects disk usage and refuses near-full disk.
4. **Observer consumes turns/DB; new events are metadata-only pointers; explicit events-retention job** — no payload double-writes, detectors fire on data that actually exists.
5. **Write-back gets a block-level writer with marker-range manifest + full edge-case suite** — the file-level writer is insufficient for human-owned files.
6. **Three systemd units (or brain-job embedder) + backup with its own lock + restore verification** — backup that was never restore-tested is not a backup.
7. **Doctor extension and version bump are explicit deliverables.**
8. **Push-first done; merge only after 24h stable + sign-off.**

## Risks / open questions

- **BGE-M3 CPU speed unknown** — benchmark first; the Ollama fallback is specified, not improvised.
- **Import volume vs 91%-full disk** — dry-run size projection + free-space refusal + `cargo clean` pre-step.
- **Backfill duration** (tens of thousands of turns × CPU) — resumable watermark, background nice; acceptable.
- **Detector first-pass noise** — high-precision-or-silent; tune thresholds on real data before R4 sign-off.
- **Block-writer blast radius** — the edge-case suite + drift-refuse + backups are the containment; Pavle smokes each target once.

## Out of scope

- Old-laptop-over-Tailscale backlog; Cursor/Antigravity import arms; voice/multi-modal/wearables; paid cloud APIs; public packaging.
