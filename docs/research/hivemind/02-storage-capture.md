# Hivemind — Storage + Capture Layer (deep dive)

Vendor: Activeloop **Hivemind** (Apache-2.0), at `/home/pavle/projekti/vendor/hivemind/`.
This document covers ONLY the storage + capture layer. All file:line refs are to that tree.

**One-sentence model:** Hivemind is a **cloud-first** shared-memory plugin. It captures
agent events as one row per event into **Activeloop Deep Lake tables over HTTP**, and exposes
those rows back to agents as a **virtual filesystem** under `~/.deeplake/memory/`. The only
thing that runs locally is an **optional** CPU embedding daemon (nomic-embed). There is **no
local database** — without cloud credentials the storage layer is inert.

---

## 1. The Deep Lake storage model

### 1.1 There is no on-disk VFS — `~/.deeplake/memory/` is virtual

`memoryPath` defaults to `~/.deeplake/memory` (`src/config.ts:74`), but it is **never created
or written on disk**. Its only consumer is `extractMemoryOp()` in `src/path-match.ts:16-52`,
which uses it as a **prefix-match string** to decide whether a Claude `Read`/`Write`/`Edit`/
`Grep`/`Bash` tool call is "touching memory" (so the pre-tool-use hook can reroute it). No
`mkdirSync`/`writeFileSync` against `memoryPath` exists anywhere in `src/`. (Verified by grep:
the only hits are the config definition and the path-match prefix.)

The "filesystem" you see (`ls /memory`, `cat /memory/...`) is synthesized at runtime by
`DeeplakeFs` (`src/shell/deeplake-fs.ts`), an in-memory VFS implementing the `just-bash` FS
interface. `DeeplakeFs.create()` bootstraps by running `SELECT`s against cloud tables and
fabricating virtual paths from the returned rows (`deeplake-fs.ts:196-300`). Reads lazily
`SELECT` the row body on demand (e.g. `deeplake-fs.ts:685-690, 744-746`); writes are buffered
and pushed to the cloud on `fs.flush()` (`src/shell/deeplake-shell.ts:91, 116-118, 122-124`).

So the **canonical store is the set of Deep Lake tables**, and the VFS is a read/write façade
over them.

### 1.2 The tables ("tensors")

Table names are config-driven (`src/config.ts:57-73`), defaulting to:

| Logical store | Default name | Env override | Schema const (`src/deeplake-schema.ts`) |
|---|---|---|---|
| Memory (wiki summaries) | `memory` | `HIVEMIND_TABLE` | `MEMORY_COLUMNS` (:40) |
| Sessions (raw turns) | `sessions` | `HIVEMIND_SESSIONS_TABLE` | `SESSIONS_COLUMNS` (:58) |
| Skills | `skills` | `HIVEMIND_SKILLS_TABLE` | `SKILLS_COLUMNS` (:76) |
| Rules | `hivemind_rules` | `HIVEMIND_RULES_TABLE` | `RULES_COLUMNS` (:104) |
| Goals | `hivemind_goals` | `HIVEMIND_GOALS_TABLE` | `GOALS_COLUMNS` (:136) |
| KPIs | `hivemind_kpis` | `HIVEMIND_KPIS_TABLE` | `KPIS_COLUMNS` (:165) |
| Codebase (graph) | `codebase` | `HIVEMIND_CODEBASE_TABLE` | `CODEBASE_COLUMNS` (:219) |

Each table is `CREATE TABLE ... USING deeplake` (`buildCreateTableSql`, `deeplake-schema.ts:257`).
Despite Deep Lake being a tensor DB, these are addressed as **SQL relational tables** — columns,
`INSERT`, `SELECT`, `ALTER TABLE ADD COLUMN`, `information_schema.columns`. The "tensor" nature
only surfaces in two columns: the embedding vectors typed `FLOAT4[]`.

Schema design is centralized: column lists are the single source of truth, and both CREATE and
the lazy `healMissingColumns()` path (`deeplake-schema.ts:311`) iterate the same arrays. A
module-load `validateSchema()` (`:187`) enforces every `NOT NULL` column has a `DEFAULT` (so
`ALTER ... ADD COLUMN` on a populated table can backfill). There are no foreign keys —
goal↔KPI is a "logical join on `goal_id`" only (`deeplake-schema.ts:158-160`).

### 1.3 How sessions / turns / tool-calls are stored

**Sessions = `sessions` table, one row per agent event** (`SESSIONS_COLUMNS`,
`deeplake-schema.ts:58-73`). Key columns:

- `id` (UUID), `path`, `filename` — VFS coordinates (see below)
- `message` `JSONB` — the actual event payload
- `message_embedding` `FLOAT4[]` — 768-dim nomic vector (NULL when embeddings off)
- `author` (= `userName`), `agent` (`claude_code` / `codex` / `cursor` / `hermes`),
  `project`, `description` (= `hook_event_name`), `size_bytes`, `plugin_version`,
  `creation_date`, `last_update_date`.

There is **no separate "turns" or "tool_calls" table**. A turn, a user prompt, a tool call,
and an assistant message are **all rows in `sessions`**, distinguished by a `type` field
*inside* the JSONB `message` blob (`capture.ts:113-138`):

- `type: "user_message"` → `{ content: prompt }`
- `type: "tool_call"` → `{ tool_name, tool_use_id, tool_input(JSON str), tool_response(JSON str) }`
- `type: "assistant_message"` → `{ content: last_assistant_message }`

Each event row also embeds `meta`: `session_id`, `transcript_path`, `cwd`, `permission_mode`,
`hook_event_name`, `agent_id`, `agent_type`, `timestamp` (`capture.ts:98-107`). So the schema is
**append-only, event-sourced, schema-light** — the relational columns are mostly indexing/
provenance metadata, and the semantic content lives in opaque JSONB.

**VFS path convention for a session** (`src/utils/session-path.ts`):
```
/sessions/<userName>/<userName>_<orgName>_<workspaceId>_<sessionId>.jsonl
```
All events for one session share that `path`; the VFS reconstructs the `.jsonl` "file" by
`SELECT message ... WHERE path = ? ORDER BY creation_date ASC` (`deeplake-fs.ts:685-690`). So a
session reads like a JSONL transcript even though it's N cloud rows.

**Goals / KPIs use a path-encoded convention** (`src/shell/goal-paths.ts`):
- `/memory/goal/<owner>/<status>/<goal_id>.md` → `hivemind_goals` row
- `/memory/kpi/<goal_id>/<kpi_id>.md` → `hivemind_kpis` row

`classifyPath()` (`goal-paths.ts:86`) routes a VFS write to the right table; the **path is the
source of truth** for owner/status/ids, and the row `content` column holds only the markdown
body — deliberately avoiding "path vs content drift" (`deeplake-schema.ts:121-126`).

**Immutability / versioning.** Skills, rules, goals, KPIs are **append-only with a `version`
bump** — edits INSERT `version = N+1`; reads pick `ORDER BY version DESC LIMIT 1`; deletes are
soft (e.g. goal `rm` → new version with `status='closed'`, full audit trail kept). This
sidesteps a Deep Lake UPDATE-coalescing quirk (`deeplake-schema.ts:96-103, 127-135, 161-164`).

### 1.4 Memory table (wiki summaries)

`memory` (`MEMORY_COLUMNS`, `:40`) stores **AI-written session summaries**, not raw turns:
`summary` TEXT + `summary_embedding` FLOAT4[], plus `mime_type`, `project`, `agent`, dates.
A background wiki worker generates these post-session and on mid-session checkpoints; the VFS
surfaces them at `/summaries/<user>/<session>.md` (`virtual-table-query.ts:62-66`,
`deeplake-fs.ts:577+`).

---

## 2. Capture flow (hook → row), vs a SQLite sessions/turns model

### 2.1 How a hook fires

Hivemind registers hooks per host agent. For Claude Code the capture hook
(`src/hooks/capture.ts`) is wired to **UserPromptSubmit, PostToolUse (async), Stop,
SubagentStop** (`capture.ts:5`). Each event runs the bundled hook script as a short-lived
Node process; the agent feeds the event as **JSON on stdin**.

### 2.2 The JSON a hook receives

`HookInput` (`capture.ts:55-74`) — the union of fields Claude Code passes:
- always: `session_id`, `transcript_path?`, `cwd?`, `permission_mode?`, `hook_event_name?`,
  `agent_id?`, `agent_type?`
- UserPromptSubmit: `prompt`
- PostToolUse: `tool_name`, `tool_input`, `tool_response`, `tool_use_id`
- Stop/SubagentStop: `last_assistant_message`, `stop_hook_active?`, `agent_transcript_path?`

Discrimination is by presence: `input.prompt !== undefined` → user_message; else
`input.tool_name !== undefined` → tool_call; else `last_assistant_message !== undefined` →
assistant_message; else skip (`capture.ts:111-142`). Codex's variant
(`src/hooks/codex/capture.ts:97-119`) discriminates on `hook_event_name` instead and adds
`model` / `turn_id` to meta — same target table, `agent='codex'`.

### 2.3 Gates before anything is written

`capture.ts:78-84`: `HIVEMIND_CAPTURE !== "false"` (:76) → plugin-enabled flag
(`isHivemindPluginEnabled`) → `entrypointPassesOnlyCliGate()` (`hooks/shared/capture-gate.ts`,
which can restrict capture to interactive CLI sessions and exclude SDK-spawned ones) →
`loadConfig()` must return non-null (i.e. **credentials present**, else silent no-op).

### 2.4 Trace → stored record

1. Build `entry` (UUID `id` + `meta` + `type` + payload), `JSON.stringify` it (`capture.ts:145`).
2. Compute VFS `path` via `buildSessionPath()`, derive `filename`, `project` from `cwd`.
3. **Embed** the JSON line (best-effort, async) unless embeddings disabled
   (`capture.ts:158-161`) — returns `null` on any failure → column lands NULL.
4. Build a **raw string-concatenated `INSERT`** (`capture.ts:163-166`) with `sqlStr()`
   escaping for text columns and a JSONB literal where only single quotes are doubled
   (comment at `:152-154` explains why full escaping would corrupt the JSON; `embeddingSqlLiteral`
   renders `ARRAY[...]::float4[]` or `NULL`, `src/embeddings/sql.ts`).
5. `api.query(insertSql)` → HTTP POST to Deep Lake (`capture.ts:169`).
6. **Self-healing fallback:** on `permission denied` / `does not exist`, call
   `ensureSessionsTable()` and retry once (`capture.ts:171-180`).
7. Side effects: maybe spawn a background wiki worker on a message-count/time threshold
   (`maybeTriggerPeriodicSummary`, `capture.ts:211-245`); skill-opt reaction; Stop-counter
   trigger.

On fatal error the hook **exits 0 and surfaces nothing** to the model/user — deliberately, to
avoid prompt-injection via the hook's only mid-session channel (`capture.ts:247-262`).

### 2.5 Conceptual comparison to a SQLite sessions/turns model

| Dimension | Hivemind | Typical local SQLite sessions/turns (e.g. Altevra) |
|---|---|---|
| Backing store | Cloud Deep Lake tables over HTTPS | Local SQLite file |
| Schema shape | 1 `sessions` table, `type` discriminator inside JSONB | Normalized: `sessions` + `turns` + `tool_calls`/`file_changes` |
| Tool calls / msgs | All rows in one table, payload in `message` JSONB | Separate typed tables/columns |
| Write path | String-concatenated SQL → POST per event | Parameterized prepared statement, local txn |
| Atomicity | Per-row, no transactions; "ALTER then INSERT may race" worries | ACID transactions |
| Failure mode | Network error / table-missing → retry once, else lost | Local write essentially always succeeds |
| Versioning | Append-only + `version` bump (no UPDATE) | `UPDATE`/normalized history as desired |
| Provenance | `author`, `agent`, `project`, `plugin_version`, dates per row | Same idea, typically FK to a session row |

Hivemind's model is **event-sourced + denormalized + cloud-native**. Altevra's is
**relational + normalized + local**. Hivemind pushes semantics into JSONB to stay schema-light
across many agents; the cost is no joins, no transactions, and SQL built by string concat.

---

## 3. Local vs cloud — the sovereignty boundary

This is the most important finding for Altevra.

### 3.1 Storage is cloud-only. There is no local DB.

- `DeeplakeApi.query()` is a `fetch()` POST to `https://api.deeplake.ai/workspaces/<ws>/tables/query`
  with `Authorization: Bearer <token>` (`src/deeplake-api.ts:246-249`). All reads/writes/heals
  go through this. (`apiUrl` default `https://api.deeplake.ai`, `config.ts:56`.)
- `loadConfig()` returns **`null` without `token` AND `orgId`** (`config.ts:46-48`). Capture,
  the VFS shell, every hook short-circuit to no-op when config is null
  (`capture.ts:84`, `deeplake-shell.ts:44-51`).
- Credentials come from `~/.deeplake/credentials.json` (`config.ts:33-38`) or env
  (`HIVEMIND_TOKEN`, `HIVEMIND_ORG_ID`, …). The README confirms you **get a token from a
  deeplake.ai account** (README:74, :66-71) — it's a hosted service (Activeloop, YC-backed,
  README:417).
- No `sqlite`/`better-sqlite3`/local `.db` dependency exists in `src/` or `package.json`
  (verified by grep — only a comment mentioning a hypothetical fallback in `skillify/pull.ts`).
- `~/.deeplake/memory/` is virtual (see §1.1) — nothing durable is stored there.

**Conclusion: a solo user CANNOT run Hivemind's memory fully offline.** Without Deep Lake cloud
credentials, the entire storage + capture layer is inert (silent no-op). "Bring your own cloud"
(README:404-410: AWS/GCS) still routes through Deep Lake's orchestration — it relocates the
*bytes*, not the dependency on the hosted control plane. Security posture is "TLS in transit,
AES-256 at rest, your cloud creds live in Deep Lake's vault" (README:393) — i.e. trust-the-vendor,
not local-sovereign.

### 3.2 What actually runs locally

Exactly one component: the **optional** embedding daemon (§4). It is **off by default**
(~600 MB dep footprint, README:299), and even when on it only computes vectors locally — the
vectors are still INSERTed into the cloud table. `~/.deeplake/config.json` (`src/user-config.ts`)
holds local opt-in/out (`embeddings.enabled`) and is the only durable local state besides the
credentials file.

### 3.3 Sovereignty verdict

Hivemind is a **team/org cloud product**, not local-first. Data leaves the machine by design;
the whole value prop ("shared brain across teammates' agents", README:25) depends on it. For a
single sovereign user this is the opposite of Altevra's `local-first by axiom` doctrine.

---

## 4. Embeddings

- **Model:** `nomic-ai/nomic-embed-text-v1.5` via `@huggingface/transformers`
  (`src/embeddings/protocol.ts:60`, `src/embeddings/nomic.ts:115`). Runs on **CPU, locally**.
- **Dims:** 768 (`protocol.ts:62`, `columns.ts:15`), `dtype` `q8` quantized (`protocol.ts:61`).
  Supports **Matryoshka truncation + renormalize** to a smaller dim if configured
  (`nomic.ts:152-162`).
- **Prefixes:** asymmetric — `search_document: ` for stored content, `search_query: ` for
  queries (`protocol.ts:69-70`, applied `nomic.ts:124`). Pooling = mean, normalized
  (`nomic.ts:131`).
- **Architecture:** a long-lived **Unix-socket daemon** (`src/embeddings/daemon.ts`) holds the
  model in RAM (~200 MB), serves newline-delimited JSON requests (`embed`/`ping`/`hello`),
  and **idle-exits after 10 min** (`protocol.ts:63`, `daemon.ts:95-102`). Socket/pidfile per
  uid in `/tmp` (`protocol.ts:72-78`), created 0o600. Hooks talk to it via `EmbedClient`
  (`src/embeddings/client.ts`), which self-heals: spawns the daemon on miss under an O_EXCL
  pidfile lock, recycles a stale daemon after marketplace upgrades via a `hello` handshake
  (`client.ts:182-246`). Client timeout 2000 ms; on timeout/failure `embed()` returns `null`
  and the row stores NULL (`protocol.ts:68`, `client.ts:93-111`).
- **Local vs cloud:** vector **computation is local**; vector **storage is cloud** (FLOAT4[]
  column). The model weights are fetched from HF on first load (`nomic.ts:113-115`,
  `useFSCache=true`), then cached.
- **Disabled path:** if the user opted out OR transformers isn't resolvable,
  `embeddingsDisabled()` is true (`src/embeddings/disable.ts`), the daemon round-trip is
  skipped, columns stay NULL, and recall **degrades to BM25/ILIKE lexical** (disable.ts header,
  README:299).
- **How semantic recall works:** at query time the query text is embedded with the `query`
  prefix, then a vector search over `message_embedding` / `summary_embedding` is combined with
  lexical matching ("hybrid lexical + semantic", README:46). Retrieval is invoked through the
  Grep interceptor / virtual-table-query paths over the cloud tables, not shown in capture.

---

## 5. The "tensor format / trajectory export for fine-tuning" claim

What the README actually says (README:384): *"Because traces are stored in Deep Lake's tensor
format, they're export-ready as PyTorch datasets. Teams ... can fine-tune on their org's
accumulated trajectories."*

**What it actually is, mechanically:** The `sessions` table is a Deep Lake dataset; every agent
event is one row with a `message` JSONB payload (`type: user_message | tool_call |
assistant_message` — §1.3) plus its `message_embedding` FLOAT4[] vector and provenance columns.
A full agent **trajectory** is just `SELECT * FROM sessions WHERE path = <session> ORDER BY
creation_date` — i.e. the ordered sequence of prompts, tool calls (with inputs + responses),
and assistant messages.

Because Deep Lake natively exposes its datasets as tensors/PyTorch dataloaders (Activeloop's
core product), that ordered event stream can be loaded as a training dataset **without an ETL
step** — the "tensor format" is Deep Lake's storage engine, not anything Hivemind adds. There is
**no fine-tuning code, no trajectory-export command, and no dataset-builder in this repo** —
grep finds the claim only in `README.md`. It is a **property inherited from the cloud backend**,
realized outside the plugin (by the customer, against Deep Lake's Python SDK). For a SQLite-based
system the equivalent is trivially `SELECT ... ORDER BY` + your own JSONL/dataset dump; the only
thing "tensor format" buys is skipping the export because the store *is* already a tensor DB.

So: real but oversold-by-association — it's a downstream affordance of choosing Deep Lake, not a
feature of the capture layer.

---

## 6. Adoptable for Altevra

Altevra already has a normalized local SQLite `sessions`/`turns` model (per CLAUDE.md and the
`crates/altevra-db` migrations). Against that baseline:

### 6.1 Where Altevra is already simpler / more sovereign — keep it

- **Local-first wins outright.** Hivemind cannot run offline; its storage is a hosted service
  and `loadConfig()` no-ops without cloud creds (§3). Altevra's SQLite-on-disk store is more
  sovereign by construction and matches the `local-first by axiom` doctrine. Do **not** adopt the
  Deep Lake dependency.
- **Normalized > JSONB-discriminator for a single owner.** Hivemind crams user/tool/assistant
  into one table keyed by a `type` string in JSONB because it must stay schema-light across many
  external agents and avoid Deep Lake's weak DDL/UPDATE story. A solo SQLite system gets real
  tables, foreign keys, transactions, and joins for free — keep them.
- **Parameterized SQL > string concatenation.** Hivemind hand-rolls `INSERT` strings with
  `sqlStr()`/manual quote-doubling and even has special-case JSONB escaping comments
  (`capture.ts:152-166`) because its HTTP SQL API forces it. Altevra should keep prepared
  statements — safer and simpler.
- **Embeddings stored locally.** Hivemind computes vectors locally but ships them to the cloud.
  Altevra can do both locally (sqlite-vec / local vector store) — strictly more sovereign.

### 6.2 Capture-design ideas genuinely worth borrowing

1. **One-row-per-event, append-only event sourcing.** Even with normalized tables, treat the
   raw capture stream as immutable events (no in-place edits of a captured turn). Hivemind's
   `version`-bump-instead-of-UPDATE pattern (`deeplake-schema.ts:96-103`) is a clean audit-trail
   discipline worth mirroring for mutable entities (decisions, goals, preferences) — store
   versions, read latest, soft-delete via a closed/tombstone version.
2. **Best-effort, non-blocking embedding on the write path.** `embed()` returns `null` on any
   failure and the row is written anyway with a NULL vector, to be backfilled later
   (`capture.ts:158-161`, `client.ts:93`). Never let the embedder block or fail capture — a
   strong rule for Altevra's recorder.
3. **Embedding daemon pattern.** A long-lived, idle-timeout, per-user Unix-socket daemon holding
   the model in RAM (`daemon.ts`) with a self-healing spawn-on-miss client (`client.ts`) is a
   tidy way to amortize model load cost across many short-lived hook processes. Altevra's
   embedder worker could adopt the idle-exit + O_EXCL-lock spawn + `hello`-handshake-recycle
   design almost verbatim (it's provider-agnostic).
4. **Centralized schema with auto-heal.** Single source-of-truth column arrays + a
   `healMissingColumns` ALTER pass + module-load schema validation (`deeplake-schema.ts`) keep
   the schema definition and the migration path from drifting. Altevra's migrations already do
   this via numbered SQL files; the **module-load invariant check** ("every NOT NULL has a
   DEFAULT", `:187`) is a cheap guard worth replicating.
5. **Gates before capture.** The layered gate (`capture.ts:78-84`: capture-flag → plugin-enabled
   → entrypoint allowlist → config present) and the **CLI-only entrypoint gate**
   (`capture-gate.ts`, to exclude SDK-spawned subprocess sessions) are useful patterns for
   avoiding double/garbage capture across Altevra's many tool adapters.
6. **Hook must surface nothing on failure.** The explicit "exit 0, write nothing to the model
   channel on error" rule (`capture.ts:247-262`) — because the only mid-session channel is the
   model's prompt and writing to it is prompt injection — is a sharp safety lesson for any
   Altevra hook that runs inside a live agent turn.
7. **Path-as-source-of-truth for structured entities.** The goal/KPI convention (path encodes
   owner/status/ids, body is free markdown — `goal-paths.ts`) is an elegant way to avoid
   "path vs content drift". Altevra's wiki/personal-brain layer could use a similar convention
   where filesystem-ish keys are authoritative and the file body stays purely descriptive.

### 6.3 What to explicitly reject

- The cloud control plane (no offline mode, vendor-held creds).
- Single-table JSONB-discriminator schema for the primary store.
- String-concatenated SQL.
- Treating "tensor format" as a feature — for Altevra it's just `SELECT ... ORDER BY` + a JSONL
  dumper whenever fine-tuning export is actually needed.
