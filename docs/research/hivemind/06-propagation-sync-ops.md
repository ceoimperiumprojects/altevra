# Hivemind — Propagation, Sync & Ops Layer (Deep Dive)

**Subject:** `@deeplake/hivemind` v0.7.84 (Apache-2.0), by Activeloop (YC-backed)
**Repo audited:** `/home/pavle/projekti/vendor/hivemind/` (single-commit vendored snapshot)
**Scope of this doc:** propagation/pull/sync mechanics, Deep Lake cloud sync, CLI surface, security model, maturity, licensing, and an Altevra-adoption verdict.
**Date:** 2026-06-09
**Method:** read-only source inspection with file:line refs.

---

## 0. One-paragraph orientation

Hivemind is a **cloud-backed shared-memory + skill-propagation layer** for coding agents (Claude Code, Codex, Cursor, Hermes, OpenClaw, pi). Every agent's sessions, mined skills, codebase graph snapshots, rules and goals are stored as rows in **Deep Lake cloud tables** and queried back via a single REST endpoint (`POST /workspaces/<ws>/tables/query`, `src/deeplake-api.ts:246`). The headline feature — "one engineer's agent figures out a migration Monday, every agent on the team can run it Tuesday" — is implemented as: mine a skill → INSERT row into the cloud `skills` table → every teammate's SessionStart hook auto-pulls newer rows to their local `~/.claude/skills/`. **The entire propagation story is cloud-mediated.** There is no peer-to-peer and no local DB; "the team" is a Deep Lake org/workspace.

---

## 1. Propagation: skill & graph updates → "every agent on the team in real time"

### 1.1 The write path (publish)

A skill is born locally and pushed to the cloud `skills` table:

```
worker: gate → skill-writer (local SKILL.md) → insertSkillRow (Deeplake INSERT)
```

(documented at `src/skillify/pull.ts:6-8`). The org-wide source of truth is the cloud `skills` table; it is **append-only and versioned** — readers always take `ORDER BY version DESC`. See `src/skillify/skill-org-publish.ts:1-13`: an improved skill lands as a *new version* (`version = current.version + 1`), `scope` is promoted to `team`, and the editor + `skillopt` marker are appended to `contributors`; the original `name--author` identity is never overwritten (`skill-org-publish.ts:108-142`).

There is **no human review gate** on republish by design — "detect → improve → publish, directly" (`skill-org-publish.ts:5`). Pre-release human review is listed only as a *roadmap* item (`README.md:386`).

### 1.2 The read path (pull) — `src/skillify/pull.ts`

`runPull()` (`pull.ts:456`) is the opposite of the write path: `query Deeplake → write local SKILL.md`.

- **SQL** built by `buildPullSql()` (`pull.ts:115`): selects `name, project, project_key, body, version, source_agent, scope, author, contributors, description, trigger_text, source_sessions, install, created_at, updated_at` from the skills table, `ORDER BY project_key ASC, name ASC, version DESC`.
- **Latest-wins dedup**: `selectLatestPerName()` (`pull.ts:319`) keys by `(project_key, name)` and keeps the first row seen (= highest version). Keying by name alone would drop a same-named skill from a different project (`pull.ts:313-318`).
- **On-disk layout**: pulled skills land at `<root>/<name>--<author>/SKILL.md` (`pull.ts:521-559`) so Claude Code's single-depth skill loader sees them, cross-author name collisions stay disjoint, and the dir name self-documents authorship. Locally-mined skills stay at the flat `<root>/<name>/`. Empty/invalid author or invalid skill-name rows are **skipped, not guessed** (`pull.ts:510-559`) — a path-traversal guard.
- **Fan-out to other agents**: for *global* pulls only, `fanOutSymlinks()` (`pull.ts:216`) symlinks the canonical `~/.claude/skills/<name>--<author>/` dir into every detected non-Claude agent skills root (codex/hermes/pi/cursor). It refuses to clobber real files and is idempotent. `backfillSymlinks()` (`pull.ts:282`) repairs links for skills that were already up-to-date when a *new* agent is installed after a prior pull (`pull.ts:627-638`).

### 1.3 Conflict handling — `decideAction()` (`pull.ts:432`)

Per remote row, pure decision:

| Local state | Action |
|---|---|
| local SKILL.md missing | **wrote** |
| `remoteVersion > localVersion` | **wrote** (backs up existing to `.bak`, `pull.ts:575-577`) |
| `localVersion >= remoteVersion` | **skipped** (unless `--force`) |
| dry-run | **dryrun** |

So conflict resolution is **last-writer-wins by version number**, with a `.bak` safety net. Same-author/same-name across two projects is the one regression vs the legacy layout — the more recently pulled row clobbers the earlier (recoverable via re-pull; documented `pull.ts:528-531`).

### 1.4 The auto-pull worker — "real time" mechanics

`autoPullSkills()` (`src/skillify/auto-pull.ts:75`) is the propagation engine. Wiring & cadence (`auto-pull.ts:1-26`):

- **Runs on EVERY SessionStart** of every agent. **No throttling** — `runPull` writes are idempotent, so the only cost is one SQL round-trip + `existsSync` syscalls. This replaced an old 30-min window: "a teammate who mines a skill at 10:01 is visible to anyone who opens a session at 10:02" (`auto-pull.ts:13-17`). That is the literal meaning of "real time" — **it is pull-on-session-start, not push.** An already-open session does not receive updates mid-session.
- **Bounded by a 5s timeout** (`DEFAULT_TIMEOUT_MS`, `auto-pull.ts:35`, `withTimeout` `auto-pull.ts:57`) so a slow Deep Lake never freezes startup.
- **All failures swallowed** (`auto-pull.ts:141-144`) — SessionStart must always succeed.
- **Hard opt-out**: `HIVEMIND_AUTOPULL_DISABLED=1` (`auto-pull.ts:77`).
- **Not-logged-in = silent skip** (`auto-pull.ts:85-88`): `loadConfig()` returning null short-circuits the whole thing. **This is the load-bearing fact for solo/local use — see §6.**
- Equivalent to `hivemind skillify pull --all-users --to global` (`auto-pull.ts:25-26`): `install=global`, `users=[]`, `force=false`.
- `autopull-worker.ts` is just a standalone bundled entrypoint that calls `autoPullSkills()` once and exits 0; used by `pi` (which spawns it synchronously from session_start because it can't link the TS at extension-load time, `autopull-worker.ts:1-19`).

### 1.5 Scope: me / team / org

Persisted in `~/.deeplake/state/skillify/config.json` (`src/skillify/scope-config.ts:1-16`):

- `scope: "me"` (default) → SQL filter `author = <current user>` — mine **only my own** traces.
- `scope: "team"` → SQL filter `author IN (<team list>)` (`src/commands/skillify.ts:17-18`); team is a manually-maintained username list (`skillify team add/remove`).
- `scope: "org"` was **retired** — the CLI no longer accepts it; legacy config values are silently coerced to `team` on read (`scope-config.ts:18-67`).

Cross-author edits auto-promote `me → team` so co-owned skills become visible (`src/skillify/scope-promotion.ts:34-41`). Note: **`scope` governs what you MINE, not what you PULL** — auto-pull always pulls `--all-users` regardless of scope (`auto-pull.ts:25`). Scope is a write-side filter.

### 1.6 Unpull — `src/skillify/unpull.ts`

Reverse operation, manifest-driven. Source of truth: `~/.deeplake/state/skillify/pulled.json` (`unpull.ts:1-7`). Entries not in the manifest are never touched by default — protects user skills that happen to use `--` (e.g. `deploy--blue-green`, `unpull.ts:7-9`). Two-pass: manifest removal + reversing fan-out symlinks (`unpull.ts:160-178`), then optional disk-walk for `--all` (locally-mined flat dirs) / `--legacy-cleanup` (old 16-hex `project_key` dirs). Refuses to combine `--all`/`--legacy-cleanup` with author filters (over-removal footgun, `unpull.ts:96-107`).

---

## 2. Cloud sync via Deep Lake

### 2.1 Codebase graph pull — `src/graph/deeplake-pull.ts`

`pullSnapshot()` (`deeplake-pull.ts:91`) syncs per-repo AST graph snapshots across machines/worktrees via the cloud `codebase` table. Use case: "I built HEAD on machine B; machine A pulls the row" (`deeplake-pull.ts:5-9`).

**Identity model (asymmetric, `deeplake-pull.ts:10-23`):**
- PUSH key: `(org, ws, repo, user, worktree_id, commit_sha)` — one row per checkout.
- PULL key: `(org, ws, repo, user, commit_sha)` — **no worktree_id**; "freshest snapshot of this commit for me, anywhere", `ORDER BY ts DESC LIMIT 1` (`deeplake-pull.ts:121-129`).

**Sync fields:**
- `commitSha` — from `git rev-parse HEAD` (`deeplake-pull.ts:265-275`); the join key.
- `snapshot_sha256` (the `cloudSha256`) — a **stable-field hash** that excludes build-time `observation` metadata, so identical code on different worktrees/branches/timestamps dedups (`deeplake-pull.ts:153-184`). On pull, the payload is re-parsed and the hash **recomputed and verified** before writing to disk; mismatch → refuse (`deeplake-pull.ts:179-184`). Empty sha (legacy rows) is permitted but treated as "unknown", not "current".
- `cloudTs` — `ts` column, coerced from ISO-string-or-epoch to epoch ms by `parseTs()` (`deeplake-pull.ts:293-303`).

**Resolution order** (`deeplake-pull.ts:78-90`): `HIVEMIND_GRAPH_PULL=0` → skipped-disabled; no auth → skipped-no-auth; no HEAD → skipped-no-head; 0 rows → no-cloud-row; local sha == cloud sha → up-to-date; local ts > cloud ts (same commit) → local-newer; else → **pulled** (atomic write via tmp+rename, `deeplake-pull.ts:325-330`). Best-effort: any failure logs and falls back to local disk (`deeplake-pull.ts:24-27`). This is the same conflict philosophy as skills: **content-hash + timestamp, last-writer-wins, never poison local cache.**

### 2.2 Auth & credentials

- **Credential file:** `~/.deeplake/credentials.json` (`src/cli/auth.ts:12`, `src/commands/auth-creds.ts:27`). Shape (`auth-creds.ts:31-40`): `{ token, orgId, orgName?, userName?, workspaceId?, apiUrl?, savedAt }`. Written with **mode 0600**, dir with **mode 0700** (`auth-creds.ts:62-63`).
- **Login flow:** OAuth-style **Device Authorization Flow (RFC 8628)** against `https://api.deeplake.ai` (`src/commands/auth.ts:1-4, 92-166`). Opens a browser, polls `/auth/device/token` until the user authorizes, then mints a **long-lived (365-day) org-bound API token** via `/users/me/tokens` (`auth.ts:394-403`).
- **Non-interactive:** `--token <value>` or `HIVEMIND_TOKEN` env (`src/cli/auth.ts:39-57`) → `saveCredentialsFromToken(..., { skipTokenMint: true })`. The token's `org_id` JWT claim is honored so a multi-org user binds to the right org (`auth.ts:354-392`).
- **Token drift self-heal:** `healDriftedOrgToken()` (`auth.ts:217-288`) detects when `jwt.org_id !== creds.orgId` (a legacy `org switch` bug) and re-mints, realigning `orgName` + `workspaceId` — best-effort, never blocks SessionStart.
- **Org/workspace are cloud constructs:** `listOrgs`, `switchOrg`, `listWorkspaces`, `inviteMember`, `listMembers`, `removeMember` (`auth.ts:168-332`) — "team" = Deep Lake org membership, managed server-side.

### 2.3 Security claims (README `§Security & storage`, README.md:389-413)

Verbatim claims and what the code corroborates:

| Claim | Code corroboration |
|---|---|
| "TLS between every agent and Deep Lake" (`README.md:393`) | All transport is `fetch()` to `https://api.deeplake.ai` (`deeplake-api.ts:246`, `auth.ts:20`). TLS is implicit in HTTPS; **not independently verifiable in this repo** — it's the server's posture. |
| "AES-256 on the bytes once they land" (`README.md:393`) | **Server-side claim. Zero code in this repo** — encryption-at-rest happens in Deep Lake cloud, opaque to the client. |
| "Your cloud credentials live in Deep Lake's vault, Hivemind never sees the raw keys" (`README.md:393`) | Refers to **BYOC** cloud creds (S3/GCS/Azure), not the Deep Lake token. The Deep Lake API token itself *is* stored locally in plaintext JSON at `~/.deeplake/credentials.json` (`auth-creds.ts:57-64`) — 0600, but not encrypted. |
| "Org/workspace boundaries enforced at storage layer" (`README.md:394`) | Client always sends `X-Activeloop-Org-Id` header + org-bound JWT (`deeplake-api.ts:251`); **enforcement is server-side**, unverifiable here. |
| "SQL values escaped with sqlStr/sqlLike/sqlIdent" (`README.md:399`) | **Verified in repo:** `src/utils/sql.ts` helpers used throughout (`deeplake-pull.ts:52,119-128`, `skill-org-publish.ts:15`); `pull.ts:104-109` has its own `esc()`. Path-traversal guards `assertValidSkillName`/`assertValidAuthor` (`pull.ts:37-43`). |
| "Credentials 0600, config dir 0700" (`README.md:401`) | **Verified** (`auth-creds.ts:62-63`, `update.ts:199`). |
| "Device flow login: no tokens in env or code" (`README.md:402`) | **Verified** (`auth.ts:92-166`) — though `HIVEMIND_TOKEN` env *is* a supported alternate path (`cli/auth.ts:40`). |

**Blunt read:** the verifiable, in-repo security is solid hygiene (SQL escaping, file modes, path guards, device flow). The headline encryption/isolation claims (TLS, AES-256, storage-layer tenancy) are **server-side properties of Deep Lake cloud that this open-source client cannot prove** — you trust Activeloop's infra, not the source you can read. The local token is plaintext-on-disk (standard, but worth noting for a sovereignty-obsessed system).

---

## 3. CLI surface — what a user actually runs

From `src/cli/index.ts` (`USAGE` block `index.ts:41-162`) and `src/commands/skillify.ts`:

**Lifecycle**
- `hivemind install [--only <platforms>] [--skip-auth] [--token <v>] [--with-embeddings]` — auto-detect agents, wire each in (`index.ts:339-371`). Per-agent: `hivemind {claude|codex|claw|cursor|hermes|pi} install|uninstall`.
- `hivemind uninstall [--only ...]`, `hivemind login`, `hivemind status`, `hivemind update [--dry-run]`, `hivemind --version|--help`.

**Install internals:** Claude install delegates to the `claude` CLI marketplace flow (`src/cli/install-claude.ts:7-21`); others drop a bundle into `~/.codex/hivemind`, `~/.cursor/hivemind`, etc. (`embeddings.ts:39-43`).

**`hivemind update`** (`src/cli/update.ts:258`): checks npm registry, and if newer runs `npm install -g @deeplake/hivemind@<pinned>` then re-execs `hivemind install --skip-auth` to refresh bundles (`update.ts:280-329`). Guarded by an **O_EXCL pidfile lock** at `~/.deeplake/hivemind-update.lock` (`update.ts:198-244`) because SessionStart can dispatch N concurrent updaters that otherwise corrupt the npm reify step (real incident documented, `update.ts:169-196`). Refuses to auto-update local-dev checkouts (`update.ts:347-356`).

**Skill management (the day-to-day propagation surface)** — `src/commands/skillify.ts:7-17`:
- `hivemind skillify` — show scope/team/install/status.
- `hivemind skillify scope <me|team>` — set mining scope.
- `hivemind skillify team add|remove|list <username>`.
- `hivemind skillify pull [skill-name] [--user/--users/--all-users] [--to project|global] [--force] [--dry-run]` — manual pull (auto-pull runs this for you on SessionStart).
- `hivemind skillify unpull [...]`, `hivemind skillify mine-local` (offline mining — see §6).

**Embeddings (optional semantic search)** — `hivemind embeddings install|enable|disable|uninstall [--prune]|status` (`src/cli/embeddings.ts`). `install` downloads `@huggingface/transformers` (~600 MB) **once** into `~/.hivemind/embed-deps` and symlinks every agent's `node_modules` to it (`embeddings.ts:18-21,105-183`). A local CPU embedding daemon over a Unix socket; BM25 lexical fallback when off (`README.md:38`).

**Graph** — `hivemind graph build|diff|history|init|pull|uninstall` (`index.ts:111-126`); `graph init` installs a `.git/hooks/post-commit` that rebuilds + pushes on each commit.

**Rules / goals / context / dashboard** — `hivemind rules add|list|edit|done` (org-wide rules auto-injected into SessionStart, `index.ts:130-137`); `hivemind goal/kpi`; `hivemind context` (prints rules+goals on demand for agents without SessionStart hooks); `hivemind dashboard [--serve --port N]` (self-contained HTML, `index.ts:71-82`).

**Account** — `whoami, logout, org list/switch, workspaces, workspace switch, members, invite, remove, autoupdate, sessions prune` (`index.ts:144-156`, `AUTH_SUBCOMMANDS` `index.ts:28-39`).

---

## 4. Maturity signals — real vs vaporware

**Real, and unusually disciplined.** Concrete evidence:

- **Test coverage:** **223 test files**, **~4,086 `it/test` blocks**, **856 `describe` blocks** against **205 source files / ~40,938 LOC** in `src/`. Tests directly cover the propagation core: `tests/shared/graph/deeplake-pull.test.ts`, `deeplake-push.test.ts`, `skill-org-publish.test.ts`, `skill-publisher.test.ts`, `skillify-skills-table.test.ts`, plus the whole `skillopt-*`, `success-judge`, `capture-gate` suites. The pull/unpull/decideAction logic is written as pure functions specifically for unit testing (`pull.ts:429-431`, `scope-promotion.ts:18`).
- **Engineering rigor in the code itself:** the comments cite specific GitHub issues (#118, #125, #190, #198), CodeRabbit P1/P2/P3 review findings, and dated production incidents (e.g. concurrent-install corruption 2026-05-19, `update.ts:182`). This is a codebase under active multi-contributor review, not a demo.
- **CI gates:** `npm run ci` = `typecheck && jscpd (dup detection) && test` (`package.json:46`); husky pre-commit + lint-staged (`package.json:48,51-56`); jscpd config present.
- **Versioning:** 0.7.84 on npm as `@deeplake/hivemind`, published public (`package.json:1-11`), repo `github.com/activeloopai/hivemind`. Single-commit vendored snapshot here (`git rev-list --count = 1`) — so *this checkout's* history is squashed, but the npm version string (0.7.84 = ~84 patch releases on the 0.7 line) and in-code issue refs indicate a long real history upstream.
- **Backed by Activeloop (YC)** (`README.md:417`), the Deep Lake team; they claim to dogfood it daily across 4 agents and ran the LoCoMo benchmark themselves (`README.md:419`).

**Caveats:** still 0.x (pre-1.0, API not frozen); the "real-time team propagation" and benchmark numbers are vendor-reported; human-review-before-propagation is roadmap, not shipped (`README.md:386`).

**Verdict: genuinely active, well-tested, production-grade-leaning OSS — not vaporware.**

---

## 5. Licensing reality

- **License: Apache-2.0 — confirmed.** Full 11 KB Apache 2.0 text in `LICENSE`; `package.json` ships it in `files`; README badge confirms (`README.md:16`). The npm package itself is `publishConfig.access: public`.
- **The source code is genuinely open.** You can read, fork, modify, self-build (`npm run build`).
- **But the value is cloud-gated.** The product is a *client* for Deep Lake cloud. Real propagation requires:
  - a Deep Lake **account** (device-flow login or API token, `auth.ts`),
  - an **org + workspace** (team = org membership, server-managed, `auth.ts:306-332`),
  - and Deep Lake cloud storage for every table (`deeplake-api.ts:246`).
- **No code/license restriction**, but a **service dependency + likely metered billing**: the code reads an `X-Activeloop-Balance-Cents` header and builds a "top up" / billing URL (`deeplake-api.ts:59,109-124`) and has balance-exhausted handling (`tests/shared/deeplake-api-balance-exhausted.test.ts`). So there is a paid/credit dimension to the cloud backend even though the client is Apache-2.0.
- **BYOC** (bring your own GCS/Azure/S3/on-prem bucket) is offered but several tiers are "contact us / on request" (`README.md:408-413`) — i.e. commercial/enterprise-gated for the non-default storage backends.

**Net:** Apache-2.0 client, commercial SaaS backend. Open like the AWS CLI is open — the binary is free, the cloud it talks to is not.

---

## 6. Adoptable for Altevra? + Decision inputs

Altevra's axioms (from project `CLAUDE.md` §4.4): **local-first, single-user, data sovereign, never leaves the machine without explicit auth.** Measure Hivemind against that.

### 6.1 The architectural collision (blunt)

**Hivemind has no local datastore.** Every memory/skill/graph/rule/goal operation is a `fetch()` to Deep Lake cloud (`deeplake-api.ts:246`). `loadConfig()` returns `null` whenever there's no `token + orgId` (`config.ts:48`), and *every* propagation entrypoint hard-skips on null config:
- auto-pull: `not-logged-in → silent skip` (`auto-pull.ts:85-88`)
- graph pull: `skipped-no-auth` (`deeplake-pull.ts:99-101`)
- session capture, recall, skill push: same gate.

So **logged-out Hivemind ≈ inert.** There is no SQLite, no `:memory:`, no offline store (confirmed: only `pull.ts` and `local-mined-banner.ts` even mention sqlite, and not as a store). The "single brain" *is* the cloud workspace.

### 6.2 What survives local-only / scope=me

Exactly **one** feature works with zero cloud: **`hivemind skillify mine-local`** (`src/commands/mine-local.ts:1-6`, `src/skillify/local-source.ts:1-13`). It reads your local Claude Code/Codex/Cursor/Hermes JSONL transcripts, uses the **Claude Code CLI as a local LLM gate** (`mine-local.ts:519`), and writes `SKILL.md` files to `~/.claude/skills/` plus a manifest at `~/.claude/hivemind/local-mined.json`. No Deep Lake auth required. SessionStart shows a "local mined" banner (`src/skillify/local-mined-banner.ts`).

That's it. **Everything else Pavle would actually want from a second brain — persistent searchable memory across sessions, semantic recall, cross-project connections, the "compounds over time" store — lives in the cloud and is dead without an account.**

### 6.3 The blunt verdict

**Do NOT adopt Hivemind as-is for a local-first single-user second brain. It is the wrong shape for Altevra's first axiom.**

Reasons, concretely:
1. **It violates sovereignty by construction.** The product *is* shipping your traces to Deep Lake cloud (`README.md:35` — "structured traces in Deeplake"). Local-only mode is a degenerate near-no-op, not a supported configuration.
2. **The compounding-knowledge value (Altevra §3.4) is exactly the cloud-gated part.** Embeddings, recall, semantic search, graph sync — all require the account. A solo user running local-only gets *only* one-shot offline skill mining, which is a tiny slice and is itself just "run Claude over my old transcripts and write SKILL.md."
3. **"Team in real time" is irrelevant to a single user** — there is no second agent on the team. `scope=me` + solo means auto-pull pulls back only your own rows (still requires the cloud round-trip). The entire propagation apparatus (fan-out symlinks, conflict versioning, contributors) is dead weight for n=1.
4. **It's billed SaaS underneath** (`deeplake-api.ts:59` balance header) — adds a recurring external dependency and cost for a "use it for decades" personal tool, the opposite of sovereign.

**What to do instead — borrow the ideas, not the runtime.** The genuinely good, *transferable* design patterns worth lifting into Altevra (which already has its own local SQLite + embeddings + resident agent):
- **Append-only versioned skill rows, latest-wins-by-version** (`pull.ts:432`, `skill-org-publish.ts:108`) — clean conflict model Altevra can reuse for its skill-factory.
- **Content-hash dedup with verify-before-write** for graph/snapshot sync (`deeplake-pull.ts:153-184`) — the right way to do any future Altevra cross-machine sync without corruption.
- **`<name>--<author>` on-disk layout + manifest-tracked pull/unpull** (`pull.ts:521`, `unpull.ts:1`) — directly applicable to Altevra writing skills into Claude/Codex/Cursor dirs (Altevra's "skill manufacturing layer", project CLAUDE.md §12).
- **SessionStart auto-pull as the propagation trigger** (`auto-pull.ts:1-26`) with the 5s-timeout / all-failures-swallowed / hard-opt-out discipline — exactly how Altevra should inject context at session start without ever blocking startup.
- **mine-local's offline transcript→skill pipeline** (`mine-local.ts`) — the one piece that *is* local-first and overlaps Altevra's "import + observe" self-improvement loop.

If Pavle ever genuinely wants *multi-machine sync* for his own agents (the "import backlog: old laptop over Tailscale" note in memory), Hivemind's `deeplake-pull.ts` identity/asymmetry model is the reference design — but he'd implement it over his *own* sync transport (Tailscale + his SQLite), not by signing his life into Deep Lake cloud.

**Bottom line for the GTM-disciplined view:** using Hivemind as-is buys Pavle a cloud-dependent, account-gated, n=1-useless tool that breaks the local-first promise. Borrowing its 4-5 concrete mechanics (versioned skills, hash-verified sync, fan-out layout, session-start auto-inject) into Altevra's existing local stack is high-value and low-risk. **Borrow, don't adopt.**
