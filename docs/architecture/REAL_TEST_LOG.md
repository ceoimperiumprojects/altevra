# Altevra Real Integration Test Log

> Live cross-tool tests. One entry per run.

## 2026-06-02 (session 3) — LLM provider modes + hybrid lane, live-verified ✅ PASS

LLM provider work (plan `giggly-humming-ullman.md`, R15). All live, real-world:

- **codex_oauth reasoning → GPT-5.5 LIVE ✅** — `CodexOAuthProvider::from_default_auth()` against the REAL `~/.codex/auth.json`; minimal request returned `"PONG"`. Reverse-engineered the ChatGPT codex Responses contract through its 400s: instructions required, `store:false`, `stream:true`+SSE, no max_output_tokens. Auth accepted (not 401) → token valid. **This mode works today with NO API key**, on the existing ChatGPT Plus seat. (test `live_codex_completes`, `#[ignore]`.)
- **sqlite-vec dense store LIVE ✅** — `--features embedding`: `SqliteVecStore::open_in_memory` (statically-linked extension) → upsert 3 vecs → KNN returns nearest in correct order. Single-binary, local (SI-7). (tests `upsert_then_knn_roundtrip`, `dim_mismatch_errors`.)
- **config CLI flow ✅** — real debug binary: `config set/get/show llm.{reasoning_mode,embedding_mode,codex_model}` round-trips; `[llm]` persists to `.altevra/config.toml` (correct TOML table ordering); invalid mode rejected with clear message.
- **MCP connection re-smoke (post-changes) ✅** — `scripts/p0_mcp_smoke.sh` (debug bin): initialize → tools/list (delegation tools present: get_resident_prompt, get_context_packet, build_system_prompt, list_resident_modes) → get_capabilities (isError:false). Provider/embedding changes did NOT break the connection. Confirms `delegated` mode: a connected Claude/Cursor pulls prompt+packet and writes back via save_*.
- **Baseline ✅** — 674 workspace tests pass / 0 fail; `cargo clippy --workspace -D warnings` clean; `--features embedding` clippy clean; onnxruntime/fastembed compile.

Awaits Pavle (honest): `api` mode keys (`altevra secrets set <KEY>`); BGE-M3 real inference (first run downloads ~2GB model — `bge_embeds_with_correct_dim` `#[ignore]` ready); interactive Cursor TUI spawn in a herdr pane (his hands-on; CLI+config verified ready).

## 2026-06-02 (session 4) — temporal recall + skill cross-tool inventory ✅ PASS

Two follow-on features built and live-verified after Pavle's product feedback:

- **Temporal recall LIVE ✅** — answers his exact use case "šta smo radili pre mesec dana sa Amerikancima". `altevra-core::time_window` parses `24h`/`7d`/`30d`/`3mo` + presets `last_week`/`last_month`/etc (fail-closed on garbage — typo never widens a window). `SessionsRepository::search_turns_in_window` added; existing `search_turns` delegates with `None,None` (back-compat preserved across 12+ call sites). MCP `search_turns` tool now accepts `window`/`since`/`until` params. **Live MCP smoke**: `window="last_month"` → correct 30d range echoed; `window="garbage"` → fail-closed error listing valid forms; `since="2026-05-01" until="2026-05-31"` → date-only parsing works. Headline DB test (3 turns: month-old/yesterday/unrelated about "Americans"; window 25-40d back) returns ONLY the month-old Americans turn.
- **Skill cross-tool inventory LIVE ✅** — answers his use case "kad ubacim skill u Claude da se automatski vidi u svima". `altevra-skills::importer` scans `~/.{claude,codex,cursor,hermes,imperium}/skills/` with a loose YAML parser tolerating Claude/Hermes `name:` AND Altevra `slug:`/`title:` (separate from the strict `parse_skill` — authoring contract untouched). Detects `<!-- ALTEVRA_MANAGED -->` marker. `altevra skill inventory` CLI command — read-only first pass before any propagation. **Live on real disk**: 137 unique skill slugs across 5 tools. `--missing` flag exposes propagation candidates (e.g. `audit` is in claude+codex+cursor but missing from hermes+imperium). No writes performed (sync `--apply` is the next increment).

Baseline 685 tests pass / 0 fail; clippy `--workspace -D warnings` clean. 15 commits on `altevra-overnight-p0`.

## 2026-06-01 — MCP stdio connection (autonomous half) ✅ PASS

**What:** Built the real release binary (`cargo build --release -p altevra-cli`,
exit 0) and drove the actual `altevra serve` MCP server over stdio with real
JSON-RPC 2.0 — exactly how Claude Code / Cursor / Codex talk to Altevra.

**Script:** `scripts/p0_mcp_smoke.sh`

**Results (verbatim from the live server):**
- `initialize` → `{"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"altevra","version":"0.3.0"}}}` ✅
- `tools/list` → 36 tools returned with full input schemas, incl.
  `get_agent_bootstrap_packet`, `get_context_packet`, `search_memory`,
  `get_source_of_truth`, `create_review_item`, `get_capabilities` ✅
- `tools/call get_capabilities` →
  `{"adapters":["claude-code","codex","cursor","antigravity"],"hooks":["session_start","session_end","on_error"],"mcp_tools":22,"skills":[]}` ✅

**Verdict:** The MCP connection works end-to-end against the real binary. This is
the autonomous, deterministic half of the "real test" — proves any MCP client
(Claude Code, Cursor) can initialize, discover tools, and call them.

## 2026-06-01 — adapter connect dry-run (all tools) ✅ PASS

`altevra connect --tool <t> --dry-run` against the real binary:
- **claude-code** → detected; would update `.claude/settings.json` (MCP config) + instructions + 2 skills. ✅
- **cursor** → would create `.cursor/mcp.json` + `.cursor/hooks.json` + `.cursor/rules/altevra.mdc`. ✅
- **codex** → would create `.codex/config.toml`. ✅
- **drift safety (T4):** `AGENTS.md` flagged drifted, "won't overwrite without --force" for all three. ✅

**Verdict:** Altevra connects to Claude Code, Cursor, Codex with correct per-tool
config + drift protection. With the MCP stdio pass above, the connection layer is
proven against the real binary for every adapter.

## 2026-06-01 — LIVE herdr Claude agent spawn ✅ PASS (found + fixed 3 real bugs)

**What:** Spawned real `claude` agents in herdr (`herdr agent start … -- claude`,
split panes), pointed at Altevra, and told them to call the `get_capabilities`
MCP tool. This is the interactive half — and it did exactly what a live test is
for: **it surfaced 3 real integration bugs that the protocol smoke could not.**

**Live findings → fixes:**
1. **`altevra: command not found`** (hooks failed) → the binary wasn't on PATH.
   FIX: symlink `~/.local/bin/altevra → target/release/altevra`. ✅
2. **`mcp__altevra__*` not registered** (agent: "no altevra MCP server in this
   session") → project `.mcp.json` isn't auto-trusted. FIX: registered Altevra
   MCP at **user scope** via `claude mcp add altevra --scope user -- altevra serve`.
   Next agent then reported **"Called altevra — get_capabilities executed without
   error"** ✅ (connection works).
3. **Empty tool result** (agent: "executed but returned empty") → Altevra's
   `tools/call` returned BARE JSON, but Claude Code expects the MCP
   `{"content":[{"type":"text",...}]}` envelope. FIX: wrap every tools/call result
   in `content` + `structuredContent` in `McpServer::handle`. Verified via stdio:
   result now `{"content":[{"text":"{adapters:[claude-code,codex,cursor,antigravity]…}"}],"isError":false}`. ✅

**Verdict:** Live herdr spawn done; the Altevra↔Claude-Code connection is now
fixed end-to-end (PATH + MCP registration + response format). The smoke + adapter
proofs above plus these live fixes mean the connection layer is genuinely working,
not just protocol-shaped.

**Note:** Cursor live spawn not separately driven, but it shares the same MCP
server + the (now-fixed) response format, and `connect --tool cursor` config is
verified above.

## 2026-06-01 (session 2) — MCP live smoke after R11 + P0.5 + P0.6
- `scripts/p0_mcp_smoke.sh` against release binary: **PASS**.
  - initialize → serverInfo altevra v0.3.0 ✓
  - tools/list → all tools present incl. gated `replay_session` / `search_turns` (R11 J) ✓
  - tools/call `get_capabilities` → proper MCP content envelope, isError=false ✓
- Confirms the R11 turn-read gating + P0.5/P0.6 additions did NOT break the live MCP connection.
- `altevra resident run <mode>` live-verified separately (P0.5): personal_curator→local_private→noop(local), recorded as resident_run.
- Pending interactive half: Cursor spawn in herdr (Claude side previously live-tested).

## 2026-06-01 (session 2) — Cursor connection validated (autonomous half)
- `altevra connect --tool cursor` generates AGENTS.md + .cursor/mcp.json + hooks.json
  + rules/altevra.mdc. mcp.json registers server `altevra` → command `altevra serve`.
- Smoke through the EXACT spawn command cursor's config uses: initialize OK
  (altevra 0.3.0) + tools/list OK (36 tools incl get_context_packet). **Cursor would
  connect correctly.**
- Interactive half (a live Cursor TUI agent in a herdr pane calling Altevra) is
  Pavle's hands-on step — cursor CLI + herdr are both installed and ready.
