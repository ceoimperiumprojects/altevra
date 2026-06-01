# R11 Codex Adversarial Safety Review — verdict: FAIL

Date: 2026-06-01. Engine: Codex (independent, read-only). Cross-engine companion to `SAFETY_REVIEW_CLAUDE.json`.

## CRITICAL
1. **Raw secret stored before scan** — `hook_handle.rs:189` `auto_capture()` runs before `guard_text()`; `capture.rs` `store.set()` writes `m.matched` raw. (By-design vault feature → encrypted store; confirm store is not world-readable. Lower priority.)
2. **Raw `tool_input` persisted** — `hook_handle.rs:225-231` writes `tool_input` raw into `turns.tool_calls` even though `content` is redacted. → FIX (scan tool_calls/file_changes).
3. **PEM private key not fully redacted** — `detector.rs` private_key regex matches only the `-----BEGIN...-----` header; base64 body + footer leak. → FIX (full-block regex).
4. **Sensitivity default-DOWN on health/relationship** — `ingest_guard.rs:185-195` starts at caller `Internal`, only bumps on hard-secret/email. → FIX (high-water → Restricted, fail-closed).
5. **PII = email only** — phone/IBAN/card/quoted-email slip through as `Clean`. → FIX (pii.rs).

## HIGH
6. Detector misses `sk-proj-` + `postgresql://`. → DONE (commit 888c360).
7. Rejected-class sighting (PEM/db-url) doesn't force quarantine; `is_safe_to_persist()` can still be true. → FIX.
8. **exposure_gate FAIL-OPEN on missing metadata** — `packet/mod.rs:105` calls `decide(.., None, ..)`; gate only checks redaction when `Some`. `unscanned` object passes. → FIX (Some + object_index redaction_status + None=fail-closed).
9. **Existence leak** — denied restricted object still emits `ExclusionRecord` with `object_id`/`object_type`. → FIX (aggregate, content-free).
10. `altevra turn record --no-redact` raw-persist bypass. → FIX (gate it).

## Closed well (both engines agree)
- `within_ceiling` operator correct (`<=` rank); unknown sensitivity ranks max; anthropic/ghp_/AKIA/JWT/xoxb- regexes catch expected formats.

## Cross-engine consensus must-fix
A detector regex gaps ✅ · B db_url @-leak ✅ · C PEM full-block · D PII phone/IBAN/card · E high-water→Restricted fail-closed · F scan tool_calls/file_changes · G exposure_gate None fail-closed + object_index redaction · H ExclusionRecord existence leak · I turns sensitivity/redaction_status columns+persist · J turn-read ExposureGate · K import guard_text + --no-redact gate · L rejected→quarantine.
