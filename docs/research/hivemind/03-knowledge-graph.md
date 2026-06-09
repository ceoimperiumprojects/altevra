# Hivemind — Knowledge Graph Layer (deep teardown)

Source: `/home/pavle/projekti/vendor/hivemind/src/graph/` (Apache-2.0, Activeloop).
Read-only research. All file refs are absolute, line refs in `path:line` form.

> **TL;DR / headline correction.** The README markets the graph as "a live graph of
> your codebase from the same traces it captures: files, symbols, imports, and the
> **edges your agents actually traverse** during real sessions"
> (`README.md:324`). After reading the code, that last clause is **marketing, not
> mechanism**. The graph is a **pure static tree-sitter AST extraction over the git
> file list** (`git ls-files`). It is *triggered* by session lifecycle events
> (Stop / SessionEnd), and the *file selection threshold* is gated on git-committed
> changes — but **no session trace, tool call, or agent traversal feeds a node or
> edge**. There is no `calls`/`imports`/`reads` edge derived from what an agent did.
> The honest internal docs even say so: the SessionStart inject describes it as
> "**AST-based** — call/import/reference edges; NOT semantic similarity"
> (`session-context.ts:30-31`). This distinction is the single most important
> takeaway for Altevra's v0.6 design (see §6).

---

## 0. Directory map

```
src/graph/
├── types.ts                 # the whole data model (NetworkX node-link JSON)
├── extract/
│   ├── index.ts             # dispatch-by-extension → per-language extractor
│   ├── shared.ts            # tree-sitter helpers, node/id factories
│   ├── typescript.ts        # the rich one (raw_calls + import_bindings)
│   ├── javascript.ts python.ts go.ts rust.ts java.ts ruby.ts c.ts cpp.ts
│   └── grammar-shims.d.ts
├── resolve/
│   └── cross-file.ts        # turn raw_calls + import_bindings → cross-file edges
├── render/
│   ├── neighborhood.ts      # symbols in a file + cross-file neighbors
│   ├── path.ts              # shortest path A→B
│   ├── tour.ts              # Kahn topological dependency walkthrough
│   ├── layers.ts            # subsystem grouping by path heuristic
│   └── impact.ts            # transitive blast-radius (reverse BFS)
├── snapshot.ts              # aggregate → canonicalize → hash → atomic write
├── deeplake-push.ts         # local → cloud sync (SELECT-before-INSERT + drift)
├── deeplake-pull.ts         # cloud → local sync (freshest-for-HEAD)
├── vfs-handler.ts           # serves `cat ~/.deeplake/memory/graph/...` queries
├── session-context.ts       # SessionStart inject line
├── graph-command.ts (../commands/graph.ts)  # build orchestration + file walk
└── (build-lock, cache, history, last-build, node-metadata, snapshot, ...)
```

---

## 1. What the graph models, and how nodes/edges are extracted

### 1.1 Data model (`types.ts`)

The snapshot is a **directed multigraph** in NetworkX node-link JSON shape
(`types.ts:23-41`), deliberately chosen so the output is consumable by anything
that already reads NetworkX — *including graphify's visualizers* (`types.ts:5-7`).

**Nodes** (`types.ts:92-125`):
- `id` = `<source_file>:<symbol_name>:<kind>` (`types.ts:93`, builder `shared.ts:132-138`)
- `kind` ∈ `function | class | method | interface | type_alias | enum | const | variable | module` (`types.ts:127-136`)
- `language` ∈ TS / JS / Python / Go / Rust / Java / Ruby / C / C++ (`types.ts:138-147`)
- `source_file`, `source_location` (`L12` or `L12-40`), `exported` flag
- Optional AST metadata: `signature`, `doc` (intrinsic), and **derived** `fan_in`,
  `fan_out`, `is_entrypoint` (`types.ts:115-124`) computed *after* edge resolution.

**Edges** (`types.ts:149-181`):
- `relation` ∈ `imports | calls | extends | implements | method_of` (`types.ts:169-179`)
- `confidence` ∈ `EXTRACTED | INFERRED | AMBIGUOUS` (`types.ts:181`) — borrowed
  from graphify's convention "so consumers can apply the same filtering logic"
  (`types.ts:158-161`). In practice **everything is `EXTRACTED`** today;
  INFERRED/AMBIGUOUS are reserved for a future LLM extractor (`types.ts:160`,
  `cross-file.ts:23-24`).
- `ord` disambiguates parallel multigraph edges (same source/target/relation).

So the modeled universe is **code structure only**: symbols and the static
relationships the compiler/AST can prove. People, decisions, sessions — none of
it. This is a *codebase* graph, not a *knowledge* graph in Altevra's sense.

### 1.2 How files are selected (NOT from traces)

`../commands/graph.ts` drives the build:
- `discoverSourceFiles(cwd, ignoreConfig)` (`graph.ts:437`) selects files.
- Preferred path: `git ls-files --cached --others --exclude-standard -z`
  (`graph.ts:645-660`) — i.e. **tracked + untracked-not-ignored files**, honoring
  `.gitignore` exactly.
- Fallback for non-git dirs: a manual recursive `walk()` with name-based ignores
  (`graph.ts:631-686`).
- Each file is routed to a language extractor via `extractFile()` (`graph.ts:454`,
  dispatch in `extract/index.ts:28-40`).

**Nothing reads session transcripts here.** The whole input set is "the repo's
source files." The word "traces" in the README does not map to any code path that
touches captured sessions.

### 1.3 Per-file extraction (tree-sitter AST)

`extract/index.ts:28-40` routes by extension; everything ultimately produces a
uniform `FileExtraction` (`types.ts:188-212`). The shared layer (`extract/shared.ts`):
- Spins up singleton tree-sitter parsers per grammar (`shared.ts:48-59`).
- Streams source in 16 KB chunks because tree-sitter 0.21 throws on >32 KB strings
  (`shared.ts:34-46`).
- `makeModuleNode` / `makeNode` / `nodeId` build node records (`shared.ts:82-138`).
- `collectParseErrors` records ERROR/MISSING nodes without losing the file
  (`shared.ts:61-80`) — a parse failure degrades to "fewer nodes," never a crash.
- `findEnclosingDecl` walks *up* the AST to attribute a call site to its enclosing
  function/method (`shared.ts:161-182`).

The TypeScript extractor is the richest: besides nodes/edges it emits two
**Phase-1.5 inputs** consumed later by the resolver (`types.ts:199-211`):
- `raw_calls: RawCall[]` — unresolved call sites: `{caller_id, callee_name, receiver?}`
  (`types.ts:227-231`).
- `import_bindings: ImportBinding[]` — `{local_name, imported_name, kind, specifier, type_only?}`
  (`types.ts:239-252`).

Intra-file `calls` edges are emitted during extraction; cross-file calls are
*deferred* to the resolver because they need every file's export index.

### 1.4 Build trigger (session lifecycle, gated on git)

`../hooks/graph-on-stop.ts` is the auto-build hook, registered under both `Stop`
and `SessionEnd` (`graph-on-stop.ts:1-12`). The gate (`decideGate`,
`graph-on-stop.ts:105-147`) fires a rebuild only when **all** hold:
1. not disabled (`HIVEMIND_GRAPH_ON_STOP=0`)
2. cwd is a git repo (`graph-on-stop.ts:120-123`)
3. rate limit elapsed — default 10 min (`graph-on-stop.ts:74-79, 132-134`)
4. `HEAD != last_build.commit_sha` (`graph-on-stop.ts:136-138`)
5. at least one *source* file changed `git diff --name-only <last>..HEAD`
   filtered by `SOURCE_GLOBS` (`graph-on-stop.ts:85, 141-144, 159-177`)

> **Key insight:** the *trigger* is the session ("rebuild when the agent stops"),
> but the *content* is whatever is committed to git. An agent that read 40 files
> and edited 3 produces the exact same graph as `git checkout` + a manual build at
> that commit. Traversal does not shape the graph. A cross-process build lock
> (`build-lock.ts`, referenced `graph-on-stop.ts:53, 238-249`) handles the
> Stop+SessionEnd double-fire race.

---

## 2. Resolve (dedupe / cross-file) and Render

### 2.1 `resolve/cross-file.ts` — high-confidence, AST-only resolution

Run inside `buildSnapshot` *after* all files are extracted (`snapshot.ts:60-83`).
Three passes:

1. **`resolveCrossFileCalls`** (`cross-file.ts:60-99`): builds an **export index**
   `source_file → (exported symbol name → node id)` over exported top-level nodes
   (`cross-file.ts:42-51`), then matches each `RawCall` against the file's import
   bindings (`resolveOne`, `cross-file.ts:101-136`). Emits a `calls` edge **only**
   for:
   - `foo()` where `foo` is a **named** import (incl. `as` alias) resolving to a
     real export in a resolvable local file, or
   - `ns.foo()` where `ns` is `import * as ns` and the target file exports `foo`.

   It **deliberately drops** (does not guess): default imports, bare/npm specifiers,
   tsconfig path aliases, barrel re-exports, instance-method dispatch `obj.foo()`,
   dynamic `import()`, `require()` (`cross-file.ts:14-24, 113-130`). Doctrine,
   stated in the header: *"Ambiguous cases are dropped, not guessed"* —
   every emitted edge is `EXTRACTED` because binding + export are both concrete
   AST facts (`cross-file.ts:23-24`).

2. **`repointImportEdges`** (`cross-file.ts:151-164`): an `imports` edge initially
   points at a placeholder `external:<specifier>`. If the specifier is relative and
   resolves to a known repo file, repoint it at that file's `::module` node.
   Bare/npm and unresolvable specifiers **keep** `external:` — preserving the
   "our code vs a dependency" distinction.

3. **`resolveHeritageEdges`** (`cross-file.ts:178-226`): repoints
   `extends`/`implements` placeholders (`unresolved:<file>:<name>:<kind>`) to the
   real base-type node, first same-file then via named import. Note heritage
   accepts `type_only` imports (an interface base is legitimately type-only) while
   call resolution rejects them (`types.ts:245-251`, `cross-file.ts:118-128`).

**Module resolution** (`resolveModule`, `cross-file.ts:248-294`) emulates TS
resolution order (explicit ext → importer's family → other family → `/index`),
with a dedicated Python resolver (`resolvePythonModule`, `cross-file.ts:314-368`)
that handles dot-relative and dotted-absolute imports and **drops ambiguous suffix
matches** rather than guess (`matchPythonSuffix`, `cross-file.ts:350-368`).

> **"Dedup" here is two things:** (a) edge dedup via a `seen` set keyed
> `source\0target` (`cross-file.ts:70, 86-88`); (b) the deeper *snapshot-identity*
> dedup via content hashing (§4). There is **no entity-merge / alias resolution**
> the way Altevra would dedup "Elena" ≈ "my girlfriend" — node identity is the
> deterministic `file:name:kind` string, full stop. Two symbols with the same name
> in two files are simply two nodes.

After resolution, `annotateNodeDegrees` (`node-metadata.ts`, called
`snapshot.ts:83`) computes `fan_in`/`fan_out`/`is_entrypoint` over the **fully
resolved** edge set, then nodes/edges are sorted deterministically
(`snapshot.ts:85-86, 98-110`).

### 2.2 `render/` — five read views (all deterministic, capped, never throw)

All renderers take a parsed `GraphSnapshot` and return plain text for the agent:

| Renderer | File | What it produces |
|---|---|---|
| neighborhood | `render/neighborhood.ts:13` | symbols defined in a file + cross-file in/out neighbors grouped by relation, capped at 25 (`neighborhood.ts:3`). Fuzzy-resolves the file arg (`neighborhood.ts:16-47`). |
| path | `render/path.ts` | shortest path between two symbol patterns via BFS over an adjacency map (`path.ts:resolvePattern/buildAdjacency`). |
| tour | `render/tour.ts:14` | dependency-ordered walkthrough via **Kahn's algorithm on the reversed graph** (deps before dependents), line-capped at 60 (`tour.ts:1-13`). |
| layers | `render/layers.ts` | subsystem grouping by **path heuristic** — ordered first-match rules (`/hooks/`→Hooks, `/commands/`→CLI, `/graph/`→Graph, …) (`layers.ts:8-21`). Pure naming convention, not learned. |
| impact | `render/impact.ts:1-15` | transitive **dependents** (blast radius) via reverse BFS grouped by depth, capped at 80, max depth 25 (`impact.ts:16-22`). Header is explicit it's a **lower bound** because unresolved edges aren't traversed. |

---

## 3. How search/recall walks the graph instead of plain text

This is the load-bearing UX claim — and here the code *does* deliver, via a
**virtual filesystem** (`vfs-handler.ts`). The agent doesn't call a graph API; it
runs `cat ~/.deeplake/memory/graph/<endpoint>` and Hivemind intercepts the read,
parses the local snapshot, and returns a rendered view (`vfs-handler.ts:1-21`).
Zero network in the read path (`vfs-handler.ts:21`).

Dispatcher `handleGraphVfs(subpath, cwd)` (`vfs-handler.ts:56-159`) routes:

- `index.md` — overview: commit, node/edge/file counts + an honest **Limitations**
  block (`renderIndex`, `vfs-handler.ts:234-309`).
- `find/<pattern>` — substring search over node `id`+`label`, ranked (exact label >
  prefix > id-contains > label-contains), emits numbered **handles** persisted per
  worktree (`renderFind`, `vfs-handler.ts:404-427`; ranking `findMatches`
  `vfs-handler.ts:322-360`).
- `query/<pattern>` — **the headline "where do we handle auth" flow**
  (`renderQuery`, `vfs-handler.ts:440-475`): find + **1-hop expansion** of the top
  5 matches, showing callers/callees/imports/heritage grouped by relation
  (`renderHopGroup`, `vfs-handler.ts:484-506`). Supports multi-token AND
  (`query/auth+middleware`, `vfs-handler.ts:345-359`).
- `show/<handle-or-pattern>` — node detail + 1-hop neighbors; resolves a digit via
  the saved handle table or a pattern (`renderShow`, `vfs-handler.ts:508-547`).
- `impact/`, `neighborhood/`, `layers`, `tour`, `path/<from>/<to>` route to the
  renderers in §2.2.

**So how does "where do we handle auth?" land on real files instead of every file
mentioning "auth"?** Two mechanisms, *both lexical at the seed*:
1. The seed search matches the substring against **symbol names and node ids**
   (`auth` matches `authenticate`, `AuthMiddleware`), not free-text file bodies —
   so it already skips files that merely *mention* "auth" in a comment but define
   no auth symbol.
2. It then **walks the graph** 1 hop (`query/`) to surface the callers/callees, so
   the agent sees the *cluster* of real symbols and their files, then `Read`s those
   few files. The graph is positioned as a **fast index to the few files that
   matter**, explicitly *not* a substitute for reading source
   (`session-context.ts:144-147`).

A zero-dependency **Levenshtein fuzzy fallback** (`fuzzyMatches`/`editDistance`,
`vfs-handler.ts:368-402`) kicks in only when there's no substring hit
(`vfs-handler.ts:338-341`) — typo tolerance (`pushSnaphot`→`pushSnapshot`).

> Honest caveat baked into the inject (`session-context.ts:143-159`): the graph
> "omits instance-method calls (`obj.method()`), nested/inner functions, and
> dynamic dispatch — so confirm every claim against the file before stating it."
> Recall is **lexical-on-symbols + structural-1-hop**, not embeddings. There is *no
> semantic similarity edge* in this layer (the README's separate embedding daemon
> serves the *memory/trace* search, not this codebase graph). Search-walks-the-
> graph is real; "semantic recall over the graph" is not — it's a deliberate v1.2
> follow-up per `session-context.ts:30-31`.

---

## 4. `deeplake-pull.ts` — snapshot sync, commit sha, hashing

### 4.1 The hashing contract (snapshot identity)

`snapshot.ts:138-147` `computeSnapshotSha256` hashes **only the stable fields**
`{directed, multigraph, graph, nodes, links}` and **excludes `observation`**
(`types.ts:30-41, 63-90`). `observation` holds volatile build metadata: timestamp,
branch, worktree path, generator version, file counts (`types.ts:63-90`). The
contract (`types.ts:14-22`): *two builds of identical code on different worktrees,
branches, or timestamps must dedup to the same `snapshot_sha256`.* Canonicalization
is sorted-keys-at-every-level compact JSON (`canonicalJSON`, `snapshot.ts:154-165`)
with caller-sorted node/edge arrays.

So **commit_sha** is the *addressing* key (file is named `<commit-sha>.json`,
`snapshot.ts:206-210`) and **snapshot_sha256** is the *content-identity* key. When
there's no git (loose dir), the file falls back to being named by its sha256
(`snapshot.ts:207-208`).

### 4.2 Local layout (`snapshot.ts`)

```
~/.hivemind/graphs/<repo-key>/
  snapshots/<commit-sha>.json            # the canonical bytes
  worktrees/<worktree-id>/latest-commit.txt
  worktrees/<worktree-id>/.last-build.json   # ts, commit_sha, sha256, counts
  history.jsonl                          # append-only audit (shared across worktrees)
  .graph-on-stop.log
```
- `repo-key` = sha1 of normalized git remote URL (`types.ts:54-55`,
  `deriveProjectKey`).
- `worktree-id` = first 16 hex of sha256(cwd) (`session-context.ts:55-57`,
  `deeplake-pull.ts:46-48`) — so two checkouts on one machine don't clobber each
  other's metadata.
- Writes are **atomic** (temp-in-same-dir + `renameSync`, `snapshot.ts:259-264`).

### 4.3 Push identity vs pull identity (the asymmetry)

- **Push key** (`deeplake-push.ts`): `(org, workspace, repo, user, worktree_id,
  commit_sha)` — one row per checkout that ran the extractor. SELECT-before-INSERT
  with **drift detection**: same commit producing a *different* `snapshot_sha256`
  is logged as drift and **not overwritten** — *"let a human investigate before
  clobbering history"* (`deeplake-push.ts:3-13`). Known non-atomicity gap
  (no server-side UNIQUE) mitigated by the build lock + post-insert re-SELECT
  returning `inserted-with-duplicate-race` (`deeplake-push.ts:24-43`).
- **Pull key** (`deeplake-pull.ts:10-23`): `(org, workspace, repo, user,
  commit_sha)` — **drops `worktree_id`**, then `ORDER BY ts DESC LIMIT 1`. Rationale
  (`deeplake-pull.ts:16-23`): "what's the freshest snapshot of THIS commit for ME,
  anywhere?" — identical source → identical extracted bytes regardless of which
  checkout produced them.

### 4.4 The pull flow (`pullSnapshot`, `deeplake-pull.ts:91-261`)

Ordered resolution (`deeplake-pull.ts:83-90`):
1. `HIVEMIND_GRAPH_PULL=0` → `skipped-disabled`
2. no auth config → `skipped-no-auth` (never pulls without opt-in)
3. `git rev-parse HEAD` fails → `skipped-no-head`
4. SELECT 0 rows → `no-cloud-row`
5. local sha256 == cloud sha256 (and same HEAD) → `up-to-date`, no write
6. local ts > cloud ts (same HEAD) → `local-newer`, no overwrite
7. else → `pulled` (write payload + sidecars)

Safety hardening worth stealing:
- **Validate before writing** (`deeplake-pull.ts:143-184`): coerce payload
  (string or object), `JSON.parse`, assert it's an object with `nodes`+`links`
  arrays, then **recompute `computeSnapshotSha256` and compare** to the claimed
  column — *refuse* on mismatch rather than poison the local cache.
- **Commit-gated comparison** (`deeplake-pull.ts:190-222`): the timestamp/sha
  comparison is only meaningful when local and cloud refer to the *same* commit;
  otherwise fall through and pull. (`.last-build.json` records the last build for
  *any* commit, so a naive ts compare would wrongly refuse.)
- Timestamp coercion handles epoch-s / epoch-ms / ISO (`parseTs`,
  `deeplake-pull.ts:293-303`).
- Pulled bytes are byte-identical to a local build (same canonical writer), so the
  rest of the toolchain reads it transparently (`deeplake-pull.ts:224-227`).

`SessionStart` then surfaces the graph's existence cheaply by reading only the tiny
`.last-build.json` (never parsing the ~1 MB snapshot on the hot path),
with staleness escalation (>1h warn, >1d hard-warn) (`session-context.ts:69-163`).

---

## 5. Hivemind graph vs graphify

Both are **tree-sitter / static-parse codebase graphs** with the same
`EXTRACTED/INFERRED/AMBIGUOUS` confidence vocabulary — Hivemind explicitly borrows
graphify's conventions and NetworkX output so they're interoperable
(`types.ts:5-7, 158-161`). graphify's own SKILL confirms its shape:
"persistent knowledge graph with god nodes, community detection, and
query/path/explain tools" (`/home/pavle/.claude/skills/graphify/SKILL.md`).

| Dimension | Hivemind graph | graphify |
|---|---|---|
| **Inputs** | code only (TS/JS/Py/Go/Rust/Java/Ruby/C/C++) | *any* — code, docs, papers, images, video (Whisper) |
| **Extraction** | tree-sitter AST, deterministic | tree-sitter + LLM for INFERRED edges (`--mode deep`) |
| **Edge types** | imports/calls/extends/implements/method_of | structural + semantic/INFERRED + community edges |
| **Higher-order** | none (flat symbol graph) | **god nodes + Louvain community detection** |
| **Semantic recall** | no — lexical-on-symbols + 1-hop walk | yes — GraphRAG BFS/DFS `query`, `explain` |
| **Trigger / freshness** | auto-rebuild on Stop/SessionEnd, git-gated | manual `/graphify`, `--watch`, `--update` incremental |
| **Storage** | `~/.hivemind/graphs/`, content-hashed snapshots, **cloud sync (Deeplake) w/ drift detection** | `graphify-out/graph.json`, Obsidian vault, optional Neo4j/Gephi/GraphML export |
| **Access surface** | **VFS** `cat ~/.deeplake/memory/graph/...` | CLI `query`/`path`/`explain`, MCP server, HTML viz |
| **Multi-repo** | one repo per `repo-key` | cross-repo merge into one graph |

**Overlap:** both are static AST graphs, both speak NetworkX, both have
path/neighborhood/query primitives, both keep an honest confidence/audit trail,
both are deliberately deterministic at the core.

**Differences that matter for us:**
1. graphify is **corpus-general** (any file type) and does **community detection +
   god nodes** — emergent structure. Hivemind is code-only and flat.
2. graphify does **GraphRAG semantic query**; Hivemind's "search the graph" is
   lexical seed + 1-hop structural walk (no embeddings in *this* layer).
3. Hivemind's standout engineering is the **sync + identity model** (content
   hashing that excludes build noise, per-worktree state, push/pull asymmetry,
   drift detection, validate-before-write) and the **VFS access pattern** — neither
   of which graphify has. graphify's standout is **multi-modal ingestion + community
   structure**, which Hivemind lacks.

---

## 6. Adoptable for Altevra (v0.6 Knowledge Graph)

Altevra's v0.6 is a knowledge graph of **edges between people / projects /
decisions / goals** (per `CLAUDE.md` §5 and ROADMAP). Hivemind is a *codebase*
graph, so most of its extraction is irrelevant — but several design ideas transfer
cleanly, and one widely-repeated idea turns out to be a **trap to avoid**.

### 6.1 The "build from actual agent traversal" idea — what it really teaches

The README's framing ("edges your agents actually traverse") is aspirational; the
code builds from **static structure** and only *triggers* on sessions. The honest
lesson for Altevra is twofold:

- **Reusable:** *use sessions as the trigger and the threshold, not necessarily as
  the edge source.* Hivemind rebuilds when an agent stops AND a meaningful change
  landed (`graph-on-stop.ts:105-147`). Altevra's analogue: re-derive
  person/decision/goal edges when a session import or hook turn lands new content,
  gated by a cheap "did anything material change?" check — exactly the
  self-improving review-fork pattern already in `CLAUDE.md` §12. Don't recompute
  the whole life-graph on every turn; gate it.

- **The genuinely powerful version Hivemind *doesn't* implement — and Altevra
  should.** For a *personal* knowledge graph, the trace-derived edge is far more
  valuable than for code. "Which decisions did Pavle actually revisit together?",
  "which people co-occur in the same sessions?", "which goal does this research
  thread keep getting pulled toward?" — these are **co-access / co-mention edges
  weighted by real traversal**, and they have no static-AST equivalent. Hivemind
  has the *trigger* infrastructure but throws away the traversal signal. **Altevra
  should keep it:** every time the agent recalls record A and B in the same context
  window, that's a weighted edge `A —co_recalled→ B`. This is the one place where
  "build the graph from what the agent actually traverses" is a real, buildable,
  high-value mechanism — and it's the gap in Hivemind worth filling.

### 6.2 Directly reusable engineering patterns

1. **NetworkX node-link JSON as the snapshot format** (`types.ts:23-41`). Free
   interop with graphify, Gephi, Neo4j, and visualizers. Altevra should emit the
   same shape so the personal graph is inspectable with off-the-shelf tools.

2. **Confidence labels on every edge** (`types.ts:181`). Maps *perfectly* onto
   Altevra's existing provenance model (`CLAUDE.md` §4.3: Pavle's direct statement
   vs AI-inferred). Reuse `EXTRACTED` (Pavle said it) / `INFERRED` (agent derived
   it) / `AMBIGUOUS` (needs review). The "drop, don't guess" doctrine
   (`cross-file.ts:23-24`) is the right default for sensitive personal edges.

3. **Content-hash identity that excludes volatile metadata**
   (`snapshot.ts:138-147`, `types.ts:14-22`). Altevra's graph rebuilt at 3am vs 1pm
   with the same facts should dedup to the same hash. Split *stable* (the facts)
   from *observation* (when/where/by-which-model) so versioning tracks meaning, not
   noise — directly supports `CLAUDE.md` §4.3 temporal/provenance and §3.4
   compounding-over-time.

4. **The VFS access surface** (`vfs-handler.ts`). Exposing the graph as
   `cat <path>/query/<pattern>` means *every* AI tool that can run `cat` gets graph
   recall with zero per-tool integration — squarely Altevra's "universal AI tool
   integration" axiom (`CLAUDE.md` §3.5). Altevra already has an MCP surface; a
   read-only VFS/`cat` mirror is a cheap second front door for hook-only tools.

5. **`query/` = seed + 1-hop expand** (`vfs-handler.ts:440-475`). The personal-graph
   analogue of "where do we handle auth" is "what's connected to the ReVesta GTM
   decision?" → match the decision node, expand 1 hop to the goals, people, and
   research it touches. The summed-rank multi-token AND (`query/elena+health`) is
   directly applicable. **But** seed search must be **embedding-based for personal
   data**, not substring — "girlfriend" must hit "Elena." This is where Altevra
   *should diverge* from Hivemind's lexical seed and use its embedding space.

6. **Pull-validate-before-write + drift detection** (`deeplake-pull.ts:143-184`,
   `deeplake-push.ts:3-13`). If Altevra ever syncs the graph (opt-in per
   `CLAUDE.md` §4.4 sovereignty), recompute-and-verify-hash before persisting, and
   *never silently overwrite* a conflicting version — surface drift to Pavle. The
   "no auth → silent no-op, never sync without opt-in" stance
   (`deeplake-pull.ts:98-101`) matches Altevra's local-first axiom exactly.

7. **`impact/` reverse-BFS = "what does this touch?"** (`render/impact.ts`). For
   Altevra: "if this goal changes, which decisions/people/projects are downstream?"
   — the same reverse-reachability primitive, invaluable for the proactive notifier
   ("you changed this goal; 3 decisions depended on it").

### 6.3 What is NOT reusable / what to do differently

- **Tree-sitter extraction** — irrelevant; life has no AST. Altevra's node sources
  are notes, decisions, sessions, identity YAML, Obsidian docs — extract entities
  with an LLM + the personal-brain type system, not a parser.
- **`file:name:kind` deterministic node ids** — life entities need *entity
  resolution* (the thing Hivemind explicitly does **not** do, §2.1): "Elena" ≈ "my
  girlfriend" ≈ "@elena" must merge to one node. This is the hardest part of a
  personal graph and Hivemind offers **no help** here — borrow graphify's
  god-node/community ideas or build dedicated embedding-based entity linking.
- **Lexical-only recall** — too brittle for personal data; use embeddings at the
  seed.
- **Flat structure** — no community detection. For a decade-scale life graph,
  graphify's **community detection** (surfacing cross-domain connections you
  wouldn't think to ask about) is closer to Altevra's "the brain that *notices*"
  goal (`CLAUDE.md` §3.6) than Hivemind's flat symbol graph.

**Net recommendation for v0.6:** take Hivemind's *plumbing* (NetworkX format,
confidence labels, content-hash identity, VFS surface, validate-before-write,
reverse-BFS impact, session-as-trigger gating) + graphify's *intelligence*
(multi-modal ingest, community detection, semantic GraphRAG query) + a layer
**neither has**: embedding-based entity resolution and **traversal-weighted
co-recall edges** built from Altevra's own session history — which is the only
honest realization of the "graph from what the agent actually traverses" promise.

---

## Appendix — file reference index

- Data model: `src/graph/types.ts:23-252`
- Extract dispatch: `src/graph/extract/index.ts:28-40`
- Extract helpers: `src/graph/extract/shared.ts:34-182`
- Cross-file resolve: `src/graph/resolve/cross-file.ts:60-368`
- Snapshot build/hash/write: `src/graph/snapshot.ts:53-264`
- VFS query surface: `src/graph/vfs-handler.ts:56-547`
- Renderers: `src/graph/render/{neighborhood,path,tour,layers,impact}.ts`
- Cloud pull: `src/graph/deeplake-pull.ts:91-261`
- Cloud push: `src/graph/deeplake-push.ts:1-60`
- SessionStart inject: `src/graph/session-context.ts:69-163`
- Build trigger/gate: `src/hooks/graph-on-stop.ts:105-289`
- File walk (git ls-files): `src/commands/graph.ts:437,623-686`
- README graph claim: `README.md:322-330`
- graphify comparison: `/home/pavle/.claude/skills/graphify/SKILL.md`
