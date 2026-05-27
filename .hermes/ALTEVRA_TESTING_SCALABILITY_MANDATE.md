# Altevra Testing + Scalability Mandate

Pavle explicitly said: **MORA SVE DA SE TESTIRA i sve mora da radi i da se pravi skalabilno bas onako kako sam hteo.**

This overrides any temptation to ship a half-working skeleton.

## Non-negotiable quality gate

Do not claim done unless:

1. `cargo fmt --check` passes.
2. `cargo check --workspace` passes.
3. `cargo test --workspace` passes, or failing tests are listed with exact blockers and next fix.
4. Every public module has at least basic unit or integration coverage for its core behavior.
5. CLI commands have smoke/integration tests where practical:
   - `altevra init`
   - `altevra updates --json`
   - `altevra skill list`
   - `altevra skill check`
   - `altevra hook list`
   - `altevra hook run session_start`
   - `altevra connect --tool claude-code --project altevra --dry-run`
   - `altevra agent bootstrap --tool claude-code --project altevra --json`
6. Generated outputs are deterministic enough for tests.
7. Error paths are tested, not only happy paths.
8. README examples match actual commands.

## Architecture/scalability requirements

Design for scale from the start:

- clear crate boundaries; no god modules
- shared core types used by CLI + MCP + hooks
- MCP calls same core logic as CLI; no duplicated business logic
- adapter trait keeps Claude-specific logic out of core
- repositories/interfaces separated from command handlers
- event/update system append-only and auditable
- no hardcoded absolute Pavle paths inside libraries; config/project root passed in
- no external model/API plumbing yet
- secrets are redacted/never printed
- managed file headers and drift detection are structured, not string soup everywhere
- JSON output is stable and documented
- tests should be fast and local

## Testing strategy to implement now

Add test infrastructure early:

- unit tests per crate for pure logic
- CLI smoke tests using `assert_cmd` or equivalent
- tempdir-based tests for generated files / dry-run plans
- migration SQL should be syntactically organized and documented
- bootstrap packet JSON snapshot-ish test (stable fields only)
- skill parser tests for valid frontmatter, missing version, checksum
- hook runner tests for session_start/session_end event emission
- adapter dry-run tests for generated Claude Code files and managed headers

## End-of-run required report

Final output must include:

- exact test commands run
- pass/fail status for each
- changed files
- architecture/scalability decisions
- known incomplete pieces
- next exact morning task

Do not hide uncertainty. If something is not tested, say NOT TESTED and why.
