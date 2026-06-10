---
slug: sandbox-aware-browser-pipeline
version: 0.1.0
title: Sandbox-aware browser pipeline execution
description: Run and debug browser-backed lead pipelines by splitting Codex code work from real Hermes terminal execution, verifying VPN location, and cleaning up Playwright/imperium-crawl resources so Node exits cleanly.
---

# Sandbox-aware browser pipeline execution

## When to use

Use this when a Node/TypeScript CLI pipeline uses browser automation or `imperium-crawl`/Playwright-style resources and:

- A command finishes its logical work but the process hangs.
- Browser phases work in a normal terminal but fail or behave differently under Codex sandbox.
- The user asks to use Codex in headless mode while the task includes real browser/network scraping.
- The pipeline depends on a location-specific VPN, e.g. Miami, before running OCS/OR or official-records phases.
- Lead outputs are generated but marked `NOT_SAFE_TO_SEND` because official evidence/enrichment gates are incomplete.

## Steps

1. **Separate “code brain” from “real browser execution.”**

   Treat Codex as the coding/diagnostic worker:

   ```bash
   codex exec 'Inspect why the enrichment CLI hangs after completion and propose a cleanup fix'
   ```

   But do **not** rely on Codex sandbox for real browser pipeline runs. Browser phases should run from a normal Hermes/system terminal, not inside the Codex sandbox, because Chromium/browser sandbox and namespace operations can be blocked there.

   Working rule:

   - Codex = code, analysis, refactor, tests.
   - Normal Hermes terminal = real Playwright/browser/pipeline execution.

2. **Confirm you are in the correct repository before starting Codex.**

   Codex CLI requires a git repository. If `git status` reports:

   ```text
   fatal: not a git repository
   ```

   locate the actual project repo first, then run `codex exec` from that directory.

3. **Verify VPN/location before real scraping runs.**

   For location-sensitive official-records/case-search phases, check the public IP/location before running the pipeline.

   Example style of check:

   ```bash
   curl -s https://ipinfo.io/json
   ```

   Confirm the city/region/country matches the required target, such as Miami/Florida/US. If the machine is on the wrong endpoint, pause and connect the VPN before running browser phases.

   If NetworkManager is unavailable, do not assume `nmcli` can manage VPN state:

   ```bash
   nmcli connection show --active
   ```

   may return:

   ```text
   Error: NetworkManager is not running.
   ```

   In that case, inspect which VPN clients exist on the machine, such as `openvpn`, `nordvpn`, or other installed tools, and use the available configured client/profile rather than inventing a new setup.

4. **Run real browser pipeline commands from the normal terminal with a timeout.**

   Use timeouts to distinguish “pipeline work failed” from “pipeline work completed but Node stayed alive.”

   Example:

   ```bash
   timeout 90 node dist/cli.js enrich --case 2023-015095-CA-01 --phase ocs
   timeout 120 node dist/cli.js enrich --case 2023-015095-CA-01 --phase or
   ```

   Interpret results carefully:

   - If output says enrichment completed but `timeout` kills the process, suspect leaked browser/session handles.
   - If the process exits normally before timeout, cleanup is probably working.

5. **Add CLI cleanup for browser/session resources.**

   In a Node CLI entrypoint such as `src/cli.ts`, ensure async commands are awaited and cleanup runs in `finally`.

   The replayed fix pattern was:

   - Replace plain `program.parse()` with async parsing:

     ```ts
     await program.parseAsync(process.argv)
     ```

   - Wrap CLI execution in `try/finally`.
   - In `finally`, close/reset `imperium-crawl` browser/session resources:
     - close `imperium-crawl/stealth/browser-pool`
     - reset `imperium-crawl/sessions`

   The goal is that even when OCS/OR phases complete successfully, leftover browser/session handles do not keep Node alive.

6. **Build and test after cleanup changes.**

   Run the project’s normal verification commands:

   ```bash
   npm run build
   npm test
   ```

   Then re-run real browser phases in the normal terminal with `timeout` to confirm the process exits cleanly.

7. **Do not confuse “pipeline generated files” with “buyer-ready leads.”**

   For ReVesta-style lead outputs, validate the final gates, not just whether CSV/MD files exist.

   A generated CSV can still be `NOT_SAFE_TO_SEND` if:

   - It has `0` data rows.
   - `0/N` leads are buyer-sendable.
   - `all_active_case_verified=false`
   - `all_buyer_sendable=false`
   - `all_required_pipeline_status_done=false`
   - `all_required_evidence_links_present=false`
   - official records are not done or official-records links are missing.
   - case search, complaint, NOA, encumbrance, or evidence fields are incomplete.

   Report this as a validation/evidence issue, not as “no leads exist” or “CSV failed.”

8. **Summarize the outcome in operational terms.**

   When reporting back, separate:

   - What Codex inspected or changed.
   - Whether VPN/location was correct.
   - Which real browser commands were run from the normal terminal.
   - Whether build/tests passed.
   - Whether the process exited before timeout.
   - Whether generated leads are `SAFE_TO_SEND` or `NOT_SAFE_TO_SEND`, and why.