# Altevra P0 Acceptance Tests

Status: synthesized-by-Hermes

## Vertical loop fixture set

Use synthetic data only:

1. `project_decision_publicish.md` — project/business decision, normal sensitivity.
2. `personal_health_sensitive.md` — sensitive personal object.
3. `secret_looking_payload.txt` — fake key/token pattern, must quarantine/redact.
4. `human_edit_generated_mirror.md` — edit to generated mirror, must create review item.
5. `superseded_decision_v1/v2` — old revision excluded by default.
6. `prompt_injection_capture.md` — captured text that says “ignore instructions,” must remain inert data.

## Required tests

- Pre-write gate rejects/quarantines fake secret payload before raw persistence.
- Project context packet includes project decision with provenance.
- Sensitive personal object is excluded from project packet with non-leaking explanation.
- Deleted/forgotten object never appears in retrieval; audit contains tombstone/redacted ref only.
- Superseded object excluded unless explicitly requested.
- Generated Obsidian mirror edit creates review item, not direct DB overwrite.
- Self-improvement fixture creates proposal with `proposed` status and does not auto-apply.
- Tool output fixture is sensitivity-labeled before packet inclusion.
- Focus/daily packet from 100 events shows bounded actionable set, not raw dump.
- CLI/MCP packet output includes source refs, revision ids, and packet audit id.

## Smoke command target

Future implementation should expose a command like:

```bash
altevra p0-vertical-smoke --fixture fixtures/p0 --json
```

Expected: all gates pass, no real secrets required, deterministic JSON snapshot.
