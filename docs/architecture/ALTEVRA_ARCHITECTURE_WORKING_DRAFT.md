# Altevra Architecture Working Draft

Status: multi-agent working draft
Rule: workers edit only their assigned section between SECTION markers.

## Editing Protocol

Runtime/skill-factory correction (2026-06-06): read `ALTEVRA_RUNTIME_MODEL_AND_SKILL_FACTORY_DOCTRINE.md` before interpreting model-routing or self-improvement sections. Altevra's intended runtime includes cheap_worker, strong_reasoner, local_private, embedding_model, and an attached Codex/GPT renderer. Skill factory is Altevra-native: cheap/local modes propose pointer-only candidates, strong reasoning triages, and Codex/GPT replays raw trace before rendering final `SKILL.md`.

1. Edit only the section where OWNER matches your worker name.
2. Do not edit another worker’s section.
3. Do not edit this protocol/header unless you are Hermes.
4. If another section needs a change, add a cross-section request under your own section.
5. End your section with acceptance criteria, failure modes, tests/fixtures, and unresolved questions.
6. Do not read/print secrets. Do not implement code.

---

<!-- SECTION: object-model -->
<!-- OWNER: opus-object-model -->
<!-- STATUS: ready-for-review -->
## 1. Object Model

> Author: `opus-object-model` (Opus 4.8 MAX). Scope: the durable-object contract
> for Altevra/VVLT. This section is **schema law**, not implementation. It is
> grounded in the existing `crates/altevra-db` SQLite schema (migrations 001–018)
> and the Constitution (`ALTEVRA_ARCHITECTURE_CONSTITUTION.md`, Laws 1–7).
> Where the live schema and this contract disagree, this section defines the
> **target**; the gap is itemized in §1.9 (failure modes) and §1.16 (unresolved).

### 1.1 Purpose

Define the single, type-safe vocabulary of "things Altevra remembers" so that:

1. **Every durable object is self-describing.** Any agent (Claude, Codex, Cursor,
   Antigravity, Hermes, future) can read an object and know *what it is, how
   current it is, how trustworthy it is, who it belongs to, how sensitive it is,
   and what it connects to* — without out-of-band knowledge. This is the
   precondition for the Constitution's Law 1 ("everything durable is typed").
2. **Capture stays broad, exposure stays bounded** (Law 2). The object envelope
   carries the `sensitivity` and `domain` fields that the context/safety layers
   read to decide what may leave the machine or enter a prompt.
3. **Knowledge compounds, never silently rots** (`VISION`/`CLAUDE.md` §3.4).
   Supersession, confidence, and provenance are first-class so a record from
   2027 can be safely related to one from 2031 without an agent treating stale
   content as current.
4. **Markdown and DB stay one truth in two faces** (Law 3). The envelope is the
   shared contract that makes Obsidian frontmatter and SQLite rows round-trip.

Non-goals of this section: storage engine choice, query/retrieval ranking
(see §3 context-retrieval), redaction mechanics (see §2 safety), per-domain
policy (see §6 domains-lifecycle). This section owns **the shape of a record**.

### 1.2 Canonical object taxonomy

Objects are grouped by **family**. Each concrete `type` carries the common
envelope (§1.3). "Face" = whether the object has a human-editable markdown form
in Obsidian, a DB-only form, or both. "Canonical key" = the natural-uniqueness
constraint beyond `id`.

| Family | `type` (discriminator) | Backing store (live) | Face | Canonical key |
|--------|------------------------|----------------------|------|---------------|
| **Knowledge** | `wiki_page` | `wiki_pages` (+`wiki_page_links`) | both | `topic` (UNIQUE) |
| | `memory_document` | `memory_documents` | markdown→DB | `source_path` (UNIQUE) |
| | `memory_chunk` | `memory_chunks` | DB-only | `(document_id,start_byte,end_byte)` |
| | `insight_card` | *gap → §1.9* | DB-only | content hash |
| **Decision/Intent** | `decision` | `decisions` | both | none (append-only log) |
| | `goal` | `goals` | both | none |
| | `task` | `tasks` | both | none |
| | `learning` | *gap → §1.9* (currently `metadata` on others) | both | none |
| | `preference` | *gap → §1.9* | both | `(domain,key)` |
| **Personal brain** | `person` | *gap → §1.9* | both | stable slug |
| | `relationship` | *gap → §1.9* | both | `(person_a,person_b)` |
| | `event_log_personal` (place/health/mood/idea/…) | *gap → §1.9* | both | none |
| **Activity/Provenance** | `session` | `sessions` | DB-only | `(tool,external_id)` |
| | `turn` | `turns` | DB-only | `(session_id,turn_idx)` UNIQUE |
| | `file_change` | `file_changes` | DB-only | none |
| | `system_event` | `events` | DB-only | none |
| | `update_feed_item` | `update_feed` | DB-only | `event_id` |
| **Self-improvement** | `review_item` | `review_items` | DB-only | none |
| | `skill_proposal` | *gap → §1.9* | DB-only | dedup hash of workflow |
| | `prompt_proposal` | *gap → §1.9* | DB-only | `(mode,target)` |
| | `resident_run` | *partial:* `brain_jobs` | DB-only | none |
| | `context_packet_source` | *gap → §1.9* | DB-only | `(packet_id,object_ref)` |
| **Capability/Config** | `skill` | `skills` | both (generated md) | `slug` |
| | `hook` | `hooks` | generated | `slug` |
| | `tool_installation` | `tool_installations` | DB-only | `(tool,project_id)` |
| | `installed_component` | `installed_components` | DB-only | `(installation_id,slug)` |
| | `secret_sighting` | *gap → §1.9* (metadata only; **never raw**) | DB-only | fingerprint |
| **Research** | `research_item` | `research_items` | DB-only | `(feed_id,guid)` UNIQUE |
| **Edges** | `relation` | *gap → §1.6* (only `wiki_page_links` exists) | DB-only | `(from,rel,to)` UNIQUE |

**Taxonomy rule:** the set of `type` values is a *closed, versioned registry*
(`object_types`), but parsing is *open* (`Other(String)`) so an older binary
reading a newer object degrades gracefully instead of crashing (mirrors the
existing `WikiStatus::Other` / `WikiConfidence::Other` pattern). Adding a new
`type` is a schema-version bump and a review-gated change (Law 4), **not** an
ad-hoc string. The *category/tag* taxonomy (§1.4) is the living, auto-grown one;
the *type* taxonomy is the governed one. Do not conflate them.

### 1.3 Common metadata envelope (mandatory on every durable object)

Constitution Law 1 enumerates the required fields. This is the binding list.
Every durable object — DB row and/or markdown frontmatter — MUST carry all
**Required** fields; **Conditional** fields are required when the predicate
holds; **Optional** fields are recommended where meaningful.

| Field | Req. | Logical type | SQLite storage | Default | Frontmatter key |
|-------|------|--------------|----------------|---------|-----------------|
| `id` | Required | typed opaque id | `TEXT` PK | generated | `id` |
| `type` | Required | enum (open) | `TEXT` | per table | `type` |
| `schema_version` | Required | int ≥ 1 | `INTEGER` | `1` | `schema_version` |
| `status` | Required | enum (open) | `TEXT` | per family | `status` |
| `created_at` | Required | UTC instant | `TEXT` ISO-8601 `…Z` | `now()` | `created` |
| `updated_at` | Required | UTC instant | `TEXT` ISO-8601 `…Z` | `now()` | `updated` |
| `provenance` | Required | struct (§1.4.6) | `TEXT` JSON | `{}` (origin req.) | flattened keys |
| `sensitivity` | Required | enum ladder | `TEXT` | `internal` | `sensitivity` |
| `domain` | Required | enum | `TEXT` | `business`/`project` | `domain` |
| `scope` | Required | `project_id`\|`global` | `TEXT` (`project_id`, null=global) | null | `project` |
| `tags` | Optional | string[] | `TEXT` JSON array | `[]` | `tags` |
| `categories` | Optional | string[] (taxonomy) | `TEXT` JSON array | `[]` | `categories` |
| `confidence` | Conditional¹ | enum + opt. numeric | `TEXT` (+`REAL` opt.) | `medium` | `confidence` |
| `supersedes` | Conditional² | object ref | via `relations` (+denorm `TEXT`) | null | `supersedes` |
| `superseded_by` | Conditional² | object ref | via `relations` (+denorm `TEXT`) | null | `superseded_by` |
| `valid_until` | Optional | UTC instant | `TEXT` | null | `valid_until` |
| `review_after` | Optional | UTC instant | `TEXT` | null | `review_after` |
| `revision` | Required³ | int ≥ 1 | `INTEGER` | `1` | `rev` |
| `origin_device` | Conditional³ | device id | `TEXT` | local id | `origin_device` |
| `checksum` | Conditional⁴ | sha256 hex | `TEXT` | `''` | `checksum` |
| `relationships` | Conditional⁵ | edge set | `relations` table | — | `related` / `[[links]]` |
| `metadata` | Optional | free JSON | `TEXT` JSON | `{}` | (frontmatter extras) |

¹ Required for any object whose content is *synthesized or inferred*
(`wiki_page`, `insight_card`, `learning`, anything with `provenance.origin =
agent_inferred`). Optional for raw-captured facts stated by Pavle directly.
² Required as soon as a supersession exists; both ends written (bidirectional).
³ `revision`/`origin_device` are required only for objects in **sync-eligible**
domains (§1.12); DB-only activity rows (`turn`, `system_event`) may omit.
⁴ Required for objects with a markdown face (drift detection, Law 3) and for any
object whose body is rendered into managed files.
⁵ Required where the object is a node in the knowledge graph (decisions, goals,
people, wiki, proposals); raw activity rows need not declare relations.

**Envelope conformance is testable** (§1.14): a meta-test enumerates every
durable table and asserts the Required columns exist with the correct affinity.

### 1.4 Exact field contracts

#### 1.4.1 `id`
- **Format:** `TEXT`, globally unique across *all* types. Live schema uses
  RFC-4122 UUIDv4 (36-char dashed, e.g. `repository_roundtrip.rs` uses
  `uuid::Uuid`). **Target:** keep UUIDv4 valid; for new sortable, high-volume
  types (`turn`, `system_event`, `file_change`) **recommend ULID** (lexicographic
  = chronological) — see §1.16 Q1. Either way the id is **opaque**: no parser may
  infer meaning from its bytes except the type registry.
- **Typing:** the `id` alone is *not* type-bearing in the live schema; `type` is
  the discriminator. A cross-type reference is therefore always the **pair**
  `(type, id)` (see relations §1.6). Optional human-facing stable handles
  (`wiki.topic`, `skill.slug`, `person.slug`) are *separate* canonical keys, never
  substitutes for `id`.
- **Invariant:** immutable for the life of the object. Re-import of the same
  logical object MUST reuse the id (idempotent import keyed on canonical key);
  never mint a second id for the same canonical key.

#### 1.4.2 `type`
- Open enum string from the §1.2 registry. Lowercase snake. Immutable.
- A row's `type` MUST match the table it lives in (e.g. no `goal` rows in
  `decisions`). For the future denormalized `object_index`, `type` is the join
  discriminator.

#### 1.4.3 `schema_version`
- `INTEGER`, starts at `1`, monotonic **per `type`**. Bumped only when the
  object's field contract changes incompatibly. A migration registry
  (`object_schema_versions(type, version, migration_ref, introduced_at)`) records
  each bump. Readers MUST tolerate `schema_version` higher than they know
  (forward-read: read known fields, preserve unknown via `metadata`, never drop).
- **Distinct** from DB migration number and from product semver. This is the
  *object's* contract version.

#### 1.4.4 `status`
- Open enum; per-family lifecycle (§1.5). Default per family. Transitions are
  governed (§1.5, §1.8 I8). The string set is in the registry; `Other(String)`
  tolerated on read, flagged for review.

#### 1.4.5 `timestamps`
- `created_at`, `updated_at`: `TEXT`, ISO-8601 **UTC**, millisecond precision,
  trailing `Z` (exactly the live `strftime('%Y-%m-%dT%H:%M:%fZ','now')` form).
- Domain-event times (`decided_at`, `due_at`, `published_at`, `started_at`,
  `ended_at`, `last_synthesized_at`) are **semantic** timestamps, distinct from
  envelope `created_at` (when the *row* was written). Never overload one for the
  other.
- Invariant: `created_at ≤ updated_at ≤ now()`; no naive/local times on disk.

#### 1.4.6 `provenance` (struct, stored as JSON)
```jsonc
{
  "origin":      "pavle_direct" | "agent_inferred" | "imported" | "system_derived",
  "source_ref":  "session:<id>" | "turn:<id>" | "file:<path>" | "url:<url>"
               | "import:<batch_id>" | "object:<type>:<id>" | null,
  "captured_by": "<actor_type>:<actor_id>",   // e.g. "agent:claude-code", "user:pavle"
  "captured_at": "<iso8601Z>",
  "tool":        "<tool>" | null,              // claude-code, codex, cursor, …
  "confidence_origin": "stated" | "observed" | "derived"  // why we trust it
}
```
- Generalizes the existing `events.actor_type/actor_id/source` triple to every
  object. `origin` is **required**; the rest is best-effort but `imported`
  objects MUST carry a `source_ref` (no anonymous imports — §1.9 FM-9).
- **Provenance can itself be sensitive** (a `source_ref` pointing at a private
  session). It inherits the object's `sensitivity` and is redaction-eligible.

#### 1.4.7 `sensitivity` (ordered ladder)
`public < shareable < internal < confidential < secret < restricted`
- `public` — safe to publish (LinkedIn-ready). `shareable` — fine for external
  agents/clients. `internal` — default; Pavle's working data. `confidential` —
  business-sensitive (deals, finances-as-business). `secret` — credentials class
  (**but raw secrets never live in object text — see §1.10/§2**). `restricted` —
  personal/relationship/health/legal/financial (the §1.11 high-water domains).
- Default `internal` (matches live schema). Stored lowercase `TEXT`. The ladder
  is **total-ordered** so a ceiling comparison (`≤`) is well-defined for context
  packets (§3) and sync (§1.12).
- **Monotonic-derivation invariant (I5):** a derived object's `sensitivity` ≥
  `max(sensitivity of all sources)`. No laundering personal data to public via
  synthesis.

#### 1.4.8 `domain` / `scope`
- `domain` enum (Law 6): `business`, `personal`, `project`, `client`,
  `relationship`, `health`, `legal`, `financial`, `public`. A `domains[]`
  multi-value MAY be used for cross-domain objects (then `domain` = primary).
- `scope`: `project_id` (FK-soft to a project) or null = `global`. Live tables
  already carry `project_id`; this formalizes null = global.
- §6 (domains-lifecycle) owns per-domain *policy*; this section owns the *field*.
  Enum values MUST be reconciled with §6 (cross-request §1.17).

#### 1.4.9 `tags` vs `categories`
- `tags`: free-form, normalized `lower-kebab`, JSON array, de-duplicated. User/
  agent ergonomics. No governance.
- `categories`: the **living taxonomy** (`CLAUDE.md` §3.2 auto-categorization).
  Auto-proposed on capture; new categories surface in the daily digest for
  rename/merge. Stored as JSON array of category slugs that reference a
  `categories` registry object (itself a durable object). Category creation is
  *auto-applied* (low risk) but logged; merges/renames are review-gated.

#### 1.4.10 `confidence`
- Enum `low | medium | high` (mirrors live `WikiConfidence`) + optional numeric
  `confidence_score` `REAL ∈ [0,1]`. Default `medium`.
- **Consistency invariant (I4):** `origin=pavle_direct` ⇒ may be `high`;
  `origin=agent_inferred` ⇒ `≤ medium` until independently verified (verification
  recorded as a `supports` relation from a confirming object).

#### 1.4.11 staleness / supersession
- **Append-only correction model.** Editing a canonical fact = create the new
  version + mark the old `status=superseded` + write bidirectional
  `supersedes`/`superseded_by` edges. The old object is **retained** (history,
  Law 8), never destroyed.
- `valid_until`: hard expiry (after which content is presumed stale).
- `review_after`: soft nudge ("re-check this decision"; powers proactive briefs).
- Derived `staleness` state (not stored; computed): `fresh` |
  `due_for_review` (`now > review_after`) | `expired` (`now > valid_until`) |
  `superseded`.
- **Invariant (I3):** an object with `status=superseded` MUST have a live
  `superseded_by` target; agents MUST NOT present superseded content as current
  unless the caller explicitly asks for history/as-of.

#### 1.4.12 `relationships`
- Not stored on the object; expressed via the `relations` edge table (§1.6).
  Frontmatter exposes a convenience mirror (`related:`, `supersedes:`,
  `[[wiki-links]]`) that the indexer materializes into edges. The edge table is
  the truth; frontmatter is the face.

### 1.5 Enum / status definitions

**Sensitivity** (ladder, §1.4.7): `public, shareable, internal, confidential,
secret, restricted`.

**Domain** (§1.4.8): `business, personal, project, client, relationship, health,
legal, financial, public`.

**Provenance.origin:** `pavle_direct, agent_inferred, imported, system_derived`.

**Confidence:** `low, medium, high`.

**Status — common superset** (each family uses a subset; `Other(String)` on read):

| Family | States (→ allowed transitions) | Initial |
|--------|-------------------------------|---------|
| Generic | `draft → active → {superseded, archived, deleted}` | `active` |
| `task` | `open → in_progress → {blocked↔in_progress} → {done, cancelled}` | `open` |
| `goal` | `active → {achieved, abandoned, superseded}` | `active` |
| `decision` | `proposed → accepted → {superseded, reversed}` | `accepted`* |
| `review_item` | `open → {approved, rejected, deferred}` ; `deferred → open` | `open` |
| `wiki_page` | `draft → living → archived` (`WikiStatus` live) | `living` |
| `skill_proposal` / `prompt_proposal` | `proposed → {approved → applied, rejected, withdrawn}` ; `applied → deprecated` | `proposed` |
| `skill` / `hook` (installed) | `current, outdated, drifted, missing, conflicted, unsupported` (V5 §12 component states) | `current` |
| `secret_sighting` | `detected → {redacted, granted, revoked}` | `detected` |
| `research_item` | `ingested → {kept, dismissed}` (relevance gate) | `ingested` |
| `resident_run` | `running → {done, failed}` (live `brain_jobs`) | `running` |

\* `decisions` is currently an append-only log with no `status` column (gap,
§1.9 FM-12). Target: add `status` defaulting `accepted`; pre-existing rows
backfill to `accepted`.

**Tombstone:** `deleted` is a *soft* terminal state (status flip + `deleted_at`
in metadata), never a hard `DELETE` for sync-eligible objects (§1.12).

### 1.6 Relation / edge model

A single canonical edge table generalizes today's `wiki_page_links`:

```
relations(
  id            TEXT PRIMARY KEY,          -- typed opaque id
  from_type     TEXT NOT NULL,
  from_id       TEXT NOT NULL,
  rel           TEXT NOT NULL,             -- predicate enum (below)
  to_type       TEXT,                      -- null when target is a dangling ref
  to_id         TEXT,                      -- null when target not yet materialized
  to_ref        TEXT,                      -- by-key target: wiki topic, url, slug
  weight        REAL,                      -- optional strength (0..1)
  sensitivity   TEXT NOT NULL DEFAULT 'internal',
  provenance    TEXT NOT NULL DEFAULT '{}',
  status        TEXT NOT NULL DEFAULT 'active',  -- active | retracted
  valid_until   TEXT,
  created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE(from_type, from_id, rel, to_type, to_id, to_ref)
)
```

**Predicate (`rel`) enum** (open on read, governed on write):
`relates_to, refines, supersedes, superseded_by, derived_from, supports,
contradicts, depends_on, blocks, blocked_by, part_of, mentions, about,
decided_in, owned_by, source_of, duplicate_of`.

- **Edges are themselves sensitivity-bearing and provenanced** (an edge can leak
  that two sensitive objects relate). An edge's effective sensitivity =
  `max(sensitivity)` of its endpoints unless explicitly higher.
- **Bidirectional pairs** (`supersedes`/`superseded_by`, `blocks`/`blocked_by`)
  are written as two rows or one row + a derived view; the writer guarantees both
  directions are queryable. Choose one and assert it in tests (§1.14).
- **Dangling targets** (`to_id=null, to_ref="topic"`) are allowed **only** for
  `mentions`/`relates_to`/`about` to a not-yet-synthesized wiki topic (preserves
  the existing `wiki_page_links.to_topic` denormalization and feeds the Wiki
  Curator candidate queue). All other predicates require a resolved `(to_type,
  to_id)` or are rejected (I11).
- `duplicate_of` is the **only** sanctioned dedup mechanism — no silent merge
  (I7). Two objects sharing a canonical key are reconciled by promoting one
  canonical and pointing the other via `duplicate_of` (+ `status=superseded`).

### 1.7 Object lifecycle rules

1. **Create** → envelope fully populated; `revision=1`; emit a `system_event`
   (`<type>_created`) and (if it clears the relevance/importance gate) an
   `update_feed_item`. Markdown-faced objects also write the file with
   frontmatter `id` == DB `id` and a fresh `checksum`.
2. **Update (non-canonical fields)** → mutate in place, bump `updated_at`,
   `revision += 1`, recompute `checksum`. Allowed only for non-fact fields
   (status within the legal machine, tags, metadata). Emit `<type>_updated`.
3. **Correct (canonical fact change)** → never overwrite. Mint new version, set
   old `status=superseded`, write `supersedes`/`superseded_by` (§1.4.11). The new
   object inherits/raises `sensitivity` per I5.
4. **Supersede / archive / deprecate** → terminal-ish; object retained, excluded
   from default agent reads (§1.8 I12).
5. **Delete** → soft tombstone for sync-eligible objects; hard delete permitted
   only for DB-only ephemeral rows explicitly marked non-sync (e.g. transient
   `pending_indexing`, `embedder_queue`).
6. **Promotion across faces** → a DB-only object MAY gain a markdown face (e.g. an
   `insight_card` promoted to a `wiki_page`); this is a *new object* + a
   `derived_from` edge, not a type mutation.
7. **Every state transition is an event.** No silent lifecycle changes (Law 8 /
   V5 §6). Protected transitions (§1.11) route through a `review_item` first.

### 1.8 Invariants that prevent agent confusion

These are hard constraints; each maps to a test (§1.14). Naming `I#` so reviewers
and other sections can cite them.

- **I1 (envelope completeness):** every durable row has non-null `id, type,
  schema_version, status, created_at, updated_at, sensitivity, domain` and a
  `provenance.origin`.
- **I2 (id uniqueness):** `id` is unique across the entire store; a cross-type
  reference is always `(type, id)`.
- **I3 (no stale-as-current):** default reads exclude `superseded`/`archived`;
  a superseded object always has a live `superseded_by`.
- **I4 (confidence↔origin):** inferred objects cap at `medium` until verified.
- **I5 (sensitivity monotonicity):** derived `sensitivity` ≥ max(source
  sensitivity). Edges too.
- **I6 (domain union):** a derived/aggregate object's `domain` covers the union
  of its sources; cross-domain objects are flagged (not silently relabeled).
- **I7 (one canonical per key):** canonical-key uniqueness enforced; duplicates
  resolved via `duplicate_of`, never silent merge.
- **I8 (legal transitions only):** status changes follow §1.5; an illegal
  transition is rejected and opens a `review_item`.
- **I9 (face identity):** markdown frontmatter `id` == DB `id`; `checksum` matches
  body; divergence (manual Obsidian edit) is detected and routed to review, never
  silently overwritten (Law 3 / V5 §10).
- **I10 (temporal sanity):** UTC only; `created_at ≤ updated_at ≤ now`.
- **I11 (relation integrity):** edges reference existing types; dangling `to_ref`
  allowed only for the whitelisted predicates (§1.6).
- **I12 (gated default reads):** agent-facing reads default to
  `status ∈ {live set}` AND `sensitivity ≤ caller ceiling` AND `domain ∈ caller
  allowed`; widening requires an explicit flag and is auditable.
- **I13 (immutable identity/type/schema lineage):** `id`, `type` never change;
  `schema_version` only increases.

### 1.9 Failure modes

| # | Failure | Trigger | Mitigation |
|---|---------|---------|------------|
| FM-1 | id collision on import | two imports mint different ids for same canonical key, or external id reused | idempotent upsert keyed on canonical key; import dedup test (§1.14) |
| FM-2 | schema_version skew | newer object read by older binary | forward-read rule (§1.4.3): keep known fields, preserve unknown in `metadata`, never drop |
| FM-3 | enum drift | unknown `status`/`sensitivity`/`rel` string | `Other(String)` tolerant parse + flag to review; never panic |
| FM-4 | face↔DB divergence | manual Obsidian edit changes frontmatter/body | checksum compare on index; I9 review, no silent overwrite |
| FM-5 | supersession cycle | A supersedes B supersedes A | cycle detection on edge write; reject + review |
| FM-6 | orphaned relations | endpoint deleted/superseded | edges carry status; cascade to `retracted` on hard-delete; nightly integrity job |
| FM-7 | sensitivity downgrade | derived object labeled below source | I5 enforced at write; reject downgrade |
| FM-8 | naive/local timestamp | importer writes non-UTC | normalize to UTC `…Z` at boundary; I10 test |
| FM-9 | provenance loss | `origin=imported` without `source_ref` | reject import lacking source_ref; require batch id |
| FM-10 | taxonomy explosion | every capture invents a category | auto-create allowed but rate-surfaced in digest; merge tooling; tags vs categories split |
| FM-11 | gap objects undefined | `insight_card`, `learning`, `preference`, `person`, `skill_proposal`, `prompt_proposal`, `context_packet_source`, `secret_sighting`, `relation` have **no migration yet** | P0.1 lands these tables with the full envelope before resident/self-improve writes (build-plan dependency) |
| FM-12 | legacy objects lack envelope | live `tasks/goals/decisions` miss `schema_version, sensitivity, provenance, tags, confidence, supersession` | additive migration + backfill defaults (`internal`, `provenance.origin=imported|pavle_direct`, `schema_version=1`); no breaking change |
| FM-13 | sync conflict | two devices edit same canonical object | revision + checksum compare; conflict → `review_item`, not last-writer-wins (§1.12) |

### 1.10 Security / privacy risks

- **Sensitivity laundering** via synthesis/derivation → I5 + edge sensitivity.
- **Secrets in body/metadata/tags/provenance** → object text is **never** a place
  for raw secrets; only `secret_sighting` *fingerprints/metadata* persist. Raw
  material lives in keyring/encrypted store only. Redaction MUST happen *before*
  persist/index/embed. **(Cross-ref §2 safety; this section only forbids raw
  secrets in the envelope.)**
- **High-water domain leakage** (`relationship/health/legal/financial`,
  `sensitivity ≥ confidential`) into context packets, Obsidian, embeddings, or
  sync → default exclusion (I12) + per-domain ceiling.
- **Provenance as a side channel** → `source_ref` can reveal a sensitive origin;
  provenance inherits object sensitivity and is redaction-eligible in exposure.
- **Id enumeration** → opaque, non-sequential ids (UUID/ULID) so existence of a
  sensitive object can't be probed by counting; MCP never confirms an id outside
  the caller's sensitivity/domain ceiling.
- **Embedding leakage** → embeddings of sensitive bodies are themselves sensitive;
  the vector store row inherits `sensitivity` and is excluded from default
  semantic search above ceiling (cross-ref §3 context-retrieval).

### 1.11 Obsidian implications

- **Faces:** only families marked "both"/"markdown" in §1.2 get an Obsidian file.
  Activity rows (`turn`, `system_event`, `file_change`, `update_feed_item`),
  vectors, queues, and **all `sensitivity ≥ confidential` / high-water-domain
  objects are DB-only by default** and MUST NOT be written to the vault unless
  Pavle opts that domain in (Obsidian is human-visible *and* potentially synced).
- **Frontmatter contract = envelope subset:** `id, type, schema_version, status,
  sensitivity, domain, project, tags, categories, confidence, created, updated,
  supersedes, superseded_by, related`. Round-trip identity is law (I9).
- **Managed vs human-owned:** generated faces (skills, generated wiki) carry the
  V5 managed header (`ALTEVRA_MANAGED`, `checksum`, `generated_by`); human-owned
  notes are imported as `memory_document` with `provenance.origin=pavle_direct`.
  Source-of-truth per object (which side is canonical/editable) is **defined in §2
  safety-source-truth** — this section provides the fields it switches on
  (cross-request §1.17).
- **`[[wiki-links]]`** in bodies materialize into `relations` (`mentions`/
  `relates_to`), preserving the existing `wiki_page_links` behavior (dangling
  targets allowed).

### 1.12 Cloud / local sync implications

- **Local-first by axiom** (Law 2 / `CLAUDE.md` §4.4). Sync is **opt-in per
  domain and bounded by a sensitivity ceiling** per target. `restricted` and
  high-water domains never sync by default.
- **Conflict resolution is review-gated, not last-writer-wins** (FM-13). Each
  sync-eligible object carries `revision` (monotonic int) + `origin_device` +
  `checksum`. On divergent `revision`/`checksum`, the supersession model applies:
  produce a merged candidate → `review_item`; never silently clobber a canonical
  fact.
- **Append-only correction + tombstones** make sync safe: edits are new versions,
  deletes are soft tombstones; a peer that missed a delete sees a `deleted`
  status, not a vanished row.
- **Sync substrate undecided** (CRDT vs review-on-conflict vs simple
  revision-vector) — §1.16 Q7; flagged to Hermes since no sync section exists.
- The envelope is **transport-agnostic**: the same JSON envelope serializes for
  DB, CLI, MCP, and any future sync wire format.

### 1.13 CLI / MCP implications

- **One serializer, two surfaces.** The envelope serializes identically in CLI
  `--json` and MCP responses (V5 rule: CLI primary, MCP adapter, no duplicate
  logic). Every agent-facing read returns the full envelope so agents can relate
  objects by `(type, id)`.
- **Per-type verbs:** `create | get | list | update | supersede | relate` with
  `--json`. Common flags across all object commands:
  `--domain`, `--sensitivity-ceiling`, `--status` (default = live set),
  `--include-archived`, `--include-superseded`, `--as-of <iso8601>` (temporal
  read), `--project`.
- **Default-safe reads (I12):** MCP read tools (`search_memory`,
  `get_context_packet`, `get_active_tasks`, `get_source_of_truth`, …) apply the
  status + sensitivity + domain gate by default; widening requires an explicit
  parameter and is logged as an event.
- **Relate is first-class:** an `altevra relate <from> <rel> <to> [--json]` (and
  MCP equivalent) so agents can record `supports`/`contradicts`/`derived_from`
  without hand-editing tables. Edge writes obey I5/I11.
- New object types in §1.2 each need a thin CLI verb set + MCP read tool before
  they're considered "shipped" (P0.3 control-plane pass).

### 1.14 Required tests / fixtures / golden snapshots

Extends the existing `crates/altevra-db/tests/repository_roundtrip.rs` pattern
(fresh in-memory SQLite + `run_migrations`).

1. **Migrations-from-empty** apply cleanly (exists — keep green).
2. **Envelope conformance meta-test:** enumerate every durable table; assert the
   Required envelope columns (§1.3) exist with correct affinity (`TEXT`/`INTEGER`).
   This is the executable form of Law 1.
3. **Roundtrip per type:** insert → list/get → assert every envelope field
   survives, including new gap-objects (FM-11) once landed.
4. **Face identity:** write a `wiki_page`/`decision` to markdown, re-parse,
   assert frontmatter `id` == DB `id` and `checksum` matches (I9); mutate body →
   assert drift detected, not silently overwritten.
5. **Supersession chain:** create A, supersede with B; assert default list
   excludes A, `--include-superseded` includes A, `superseded_by(A)=B`, no cycle
   (FM-5), `--as-of` before B still returns A.
6. **Sensitivity monotonicity (I5):** derive C from a `restricted` source with
   declared `public` → write rejected.
7. **Illegal transition (I8):** `task: done → in_progress` rejected + review_item
   created.
8. **Open-enum tolerance (FM-3):** parse object with unknown `status`/`rel` →
   `Other(String)`, no panic, flagged.
9. **Import idempotency (FM-1/FM-9):** import same canonical key twice → one row,
   same id; import with `origin=imported` and no `source_ref` → rejected.
10. **Relation integrity (I11):** edge to non-existent type rejected; dangling
    wiki `mentions` allowed.
11. **No-raw-secret-in-object-text:** fake OpenAI/GitHub/AWS/JWT/DB-URL strings
    absent from any object body/metadata/tags after capture (shared with §2/P0.2).
12. **Golden snapshots:** one canonical serialized envelope JSON per type checked
    in under `crates/altevra-db/tests/golden/` so any envelope drift is a visible
    diff in review.

### 1.15 Acceptance criteria

- [ ] Constitution Law 1 is **operationalized**: a documented mandatory envelope
      (§1.3) with exact field contracts (§1.4), enforced by the conformance
      meta-test (§1.14.2).
- [ ] Every object family in §1.2 has: backing store (or named gap), face
      decision, canonical key, status machine (§1.5), and lifecycle rules (§1.7).
- [ ] A **single** relation/edge model (§1.6) subsumes `wiki_page_links` and is
      the only sanctioned cross-object link + dedup mechanism.
- [ ] Sensitivity / domain / provenance / confidence / supersession contracts are
      unambiguous (type, default, validation) and total-ordered where needed.
- [ ] All invariants I1–I13 are stated *and* each has a corresponding test in
      §1.14.
- [ ] The contract is consistent with the **live SQLite conventions** (UUID/ULID
      TEXT, ISO-8601-Z TEXT, JSON TEXT, BOOL-as-INTEGER, `Other(String)` open
      enums, `metadata` escape hatch) — **no Postgres/pgvector assumption** (the
      V5 doc's Postgres text is superseded by the shipped SQLite schema; flagged
      to consistency reviewer).
- [ ] Default agent reads can **never** surface superseded or over-ceiling
      content without an explicit, audited flag (I3 + I12).
- [ ] Gap objects (FM-11) and legacy-envelope backfill (FM-12) have a stated,
      additive (non-breaking) migration path landing in P0.1.

### 1.16 Unresolved questions

- **Q1 — id scheme:** keep UUIDv4 everywhere (current) vs adopt **ULID** for
  high-volume sortable types (`turn`, `system_event`, `file_change`) vs
  typed-prefixed ids (`task_<ulid>`). Tradeoff: sortability/index locality vs a
  second format to support. *Owner: implementation reviewer + Hermes.*
- **Q2 — cross-type index:** materialize a denormalized `object_index`
  (type, id, status, sensitivity, domain, updated_at, title) now (cheap cross-type
  list/search, single gate point) vs defer and query per-table? Leaning: build a
  lightweight index in P0.1 because §1.13 default-safe reads want one gate.
- **Q3 — schema_version granularity:** per-type integer (this spec) vs a global
  object-model semver. Per-type chosen; confirm the migration-registry location.
- **Q4 — confidence:** enum-only vs enum + numeric `confidence_score`. Spec allows
  both; do we *require* numeric for inferred objects to enable ranking in §3?
- **Q5 — taxonomy governance:** category auto-create is low-risk auto-applied;
  who/what approves *merges/renames* — daily digest review, or a `review_item`?
- **Q6 — retention/TTL:** noisy activity objects (`turn`, low-importance
  `system_event`, dismissed `research_item`) — retention policy & owner? (Compounds
  vs bloats.) Likely §6 domains-lifecycle.
- **Q7 — sync substrate:** CRDT vs revision-vector + review-on-conflict vs
  per-field merge. No sync section exists → escalate to Hermes (§1.17).
- **Q8 — `decisions` status:** confirm additive `status` column + `accepted`
  backfill is acceptable vs keeping decisions a pure append-only log.

### 1.17 Cross-section requests

- **→ §2 safety-source-truth:** (a) define the **per-object source-of-truth**
  rule (markdown-canonical vs DB-canonical vs generated vs imported) keyed on the
  fields this section provides (`type`, `provenance.origin`, face). (b) Own the
  **redaction-before-persist** guarantee referenced in §1.10/FM-11 (raw secrets
  never enter the envelope). (c) Define the **review-gate predicate** in terms of
  envelope fields (`sensitivity ≥ confidential` OR `domain ∈ {relationship,
  health, legal, financial}` OR protected `type` OR illegal transition) so I8/I12
  and §1.11 use one shared predicate.
- **→ §3 context-retrieval:** packets/embeddings MUST read envelope fields
  (`status`, `sensitivity`, `domain`, `confidence`, supersession) for filtering;
  exclude superseded/over-ceiling by default (I3/I12); embeddings inherit object
  `sensitivity` and are keyed by `(object_id, revision)` so a corrected object
  re-embeds rather than shadowing the old vector.
- **→ §4 agents-self-improve:** `skill_proposal`, `prompt_proposal`,
  `resident_run`, `insight_card` are durable objects and MUST carry the full
  envelope + their proposal status machine (§1.5); observer-authored changes write
  `supersedes`/`derived_from` edges, never destructive edits.
- **→ §5 tools-skills-interfaces:** `skill`, `hook`, `tool_installation`,
  `installed_component` are durable objects; map the V5 managed-file `checksum`
  to the object `checksum`/`revision` so drift detection and supersession share
  one mechanism (I9). Align component states (`current/outdated/drifted/…`) with
  §1.5.
- **→ §6 domains-lifecycle:** **co-own the `domain` enum and per-family status
  machines.** This section fixes the *field set and value list* (§1.4.8, §1.5);
  §6 owns per-domain *policy* (retention, exposure defaults, lifecycle nuances).
  Reconcile any enum divergence before Hermes synthesis.
- **→ §7 hermes-synthesis:** ratify (a) the mandatory envelope as cross-cutting
  schema law, (b) the id scheme decision (Q1), (c) the SQLite-not-Postgres
  reconciliation against the V5 doc, and (d) ownership of a future **sync section**
  (Q7) since none exists today.

<!-- END_SECTION: object-model -->

---

<!-- SECTION: safety-source-truth -->
<!-- OWNER: opus-safety-source-truth -->
<!-- STATUS: drafted -->
## 2. Safety + Source of Truth

> Highest-risk section. Design stance: **default-deny, fail-closed, single
> choke point, append-only audit, no silent anything.** Capture may be broad;
> exposure is minimal, redacted, source-backed, sensitivity-filtered, audited
> (Constitution Laws 2–4). When any decision is ambiguous, the system raises
> sensitivity, blocks exposure, and routes to review — never the reverse.

### 2.1 Purpose

Define the safety and source-of-truth (SoT) substrate that every other
section sits on:

- **Data safety** — no raw secret, no over-ceiling PII, no cross-domain bleed
  ever reaches DB text, embeddings, markdown, journals, event payloads, logs,
  packets, or cloud.
- **Source-of-truth** — for every durable field, exactly one authority owns
  the canonical value (DB row, human-authored markdown, generated mirror, or
  immutable import). No two stores silently disagree.
- **Lifecycle integrity** — correction, supersession, soft-forget, and hard
  delete are four distinct, audited, reversible-where-required operations that
  never leave orphans.
- **Review-gated mutation** — protected changes (identity, policy, schema,
  secrets, sensitive memory, SoT reassignment, exposure widening, hard delete,
  external actions) cannot apply without an approved review item.

Two enforcement primitives serve the whole system and live in **core** (CLI,
MCP, hooks, resident agent all call them — no duplicate logic, per V5 §5):

- `ingest_guard(text, ctx) -> Guarded { value, redaction_status, manifest_ref, sensitivity }`
  — the only sanctioned write path for free text/payloads.
- `exposure_gate(item, request{audience, sensitivity_ceiling, domain_scope}) -> Decision { allow|deny, reason_code }`
  — the only sanctioned read/exposure path.

If a code path touches durable text without one of these, it is a bug by
definition (see invariants §2.11, doctor checks §2.16).

### 2.2 Data safety contract for every text/payload field

Every durable field that can hold free text, a JSON payload, a title, a
summary, or a file body carries a **field safety descriptor** (declared in the
schema, owned jointly with object-model §1):

| Descriptor | Values | Meaning |
|---|---|---|
| `safety_class` | `opaque_secret_ref` · `redactable_text` · `structured_payload` · `derived_summary` · `public_text` | How the field is treated by guard/gate. `opaque_secret_ref` holds only `{{secret:handle}}`, never a value. |
| `secret_scan` | bool | Field must pass secret detection before persist. |
| `pii_scan` | bool | Field must pass PII detection before persist. |
| `max_sensitivity` | sensitivity level | Ceiling this field is permitted to carry; a higher classification quarantines the row. |
| `redaction_status` | see §2.3 | Result of the guard. |
| `redaction_manifest_ref` | id | Pointer to what was redacted/where (metadata only, no values). |
| `provenance` | source, actor, confidence, captured_at | Who/where/when (object-model §1). |

Contract rules:

1. **No field is exempt.** `public_text` still gets `secret_scan` (accidental
   key paste into a "public" note is the classic leak).
2. `structured_payload` (JSONB) is scanned **recursively** — secrets hide in
   nested keys; the guard walks all string leaves.
3. `derived_summary` (LLM/agent output) is scanned **on output**, not trusted
   because "we generated it" — a summary can echo a secret from its input.
4. A field whose resolved sensitivity exceeds `max_sensitivity` is **rejected
   to quarantine**, never truncated/auto-downgraded.

### 2.3 `redaction_status` + `exposure_policy` rules

**`redaction_status`** (enum, fail-closed):

| Value | Exposable? | Meaning |
|---|---|---|
| `unscanned` | **never** | Transient pre-commit only. Invariant I1: must not survive a transaction. |
| `clean` | yes (subject to gate) | Scanned, nothing sensitive found. |
| `redacted` | yes (subject to gate) | Secrets/PII replaced by placeholders + handle refs; manifest attached. |
| `quarantined` | no (review only) | Detection ambiguous or over-ceiling; stored encrypted, not exposed until review resolves it. |
| `rejected` | n/a (not stored) | Hard-secret (e.g. private key) blocked from durable storage entirely; only an audit fingerprint persists. |

**`exposure_policy`** (per object, defaulted per domain by domains-lifecycle §6):

| Field | Values | Effect |
|---|---|---|
| `audience_ceiling` | `self_only` · `pavle_only` · `trusted_agents` · `project_agents` · `any_agent` · `shareable_public` | Max audience that may ever see it. |
| `packet_eligible` | bool | May it enter a context packet at all. |
| `min_redaction_required` | `redaction_status` | Minimum status before exposure (default `clean`/`redacted`). |
| `domain_scope` | set of domains | Only requests within these domains may surface it. |
| `requires_explicit_unlock` | bool | Needs a per-call authorization token (interactive Pavle), not just an agent request. |
| `mirror_to_markdown` | bool | May this be written to plaintext Obsidian (see §2.14). Default **false** for `confidential+`. |

**Exposure decision** = monotone intersection: an item is exposed iff
`request.audience ≤ item.audience_ceiling` **and** `request.sensitivity_ceiling ≥ item.sensitivity.level`
**and** `request.domain_scope ⊇ item.domain_scope` **and**
`item.redaction_status ≥ item.min_redaction_required` **and** `packet_eligible`.
Any failure → deny. The reason code is **sensitivity-aware**: it never reveals
the existence of a higher-classified item (see §2.13 side-channel).

### 2.4 Sensitivity / domain model interface

Sensitivity is a **lattice**, not a scalar:

```
Sensitivity {
  level:   public < internal < confidential < secret < restricted   // ordered
  tags:    set<{ financial, health, relationship, legal, credential,
                 identity, minor, third_party_pii }>                 // orthogonal
  domains: set<{ business, personal, project, client, relationship,
                 health, legal, financial, public }>                 // Constitution Law 6
}
```

**Composition rule (monotonic, Invariant I8):** when objects are combined
(packet build, summary, wiki page, journal entry), the result's sensitivity is
`level = max(levels)`, `tags = ∪ tags`, `domains = ∪ domains`. Sensitivity
**only ever rises** under composition. Any operation that would lower it is a
review-gated reclassification (§2.9), never automatic.

**Default-up classification:** when the classifier (local model per
Constitution / Hermes routing) is uncertain, it picks the **higher** level and
attaches the candidate tag — uncertainty resolves toward protection.

Interface used by other sections:

```
classify(text, ctx) -> Sensitivity          // local_private model only for personal domains
ceiling_ok(item.sensitivity, request) -> bool
combine(a, b) -> Sensitivity                 // monotone join
```

### 2.5 Secret detection / redaction flow

Single function `ingest_guard` runs **before** every storage/index/exposure
boundary enumerated by P0.2 (turn record, hook handle, session import, file
history, memory ingest, context packet, embeddings queue):

```
text ─▶ [1 pattern detect] ─▶ [2 entropy heuristic] ─▶ [3 classify] ─▶ [4 act] ─▶ [5 audit] ─▶ Guarded
```

1. **Pattern detectors** (regex, by type — not value): provider API keys
   (e.g. `sk-`, `ghp_`/`gho_`, `AKIA…`, Google `AIza…`), JWTs (`eyJ…` 3-part),
   PEM private-key blocks, bearer tokens, `postgres://user:pass@…` / other
   credentialed URLs, `.env`-style `KEY=value` assignments, SSH keys.
2. **Entropy heuristic** — flags high-entropy tokens that match no known
   pattern (novel formats); flagged tokens go to `quarantined`, not `clean`.
3. **Classify** → `hard_secret` (private key, cloud root cred, DB URL with
   password) vs `soft_secret` (ambiguous high-entropy) vs `pii` vs `none`.
4. **Act:**
   - `hard_secret` that Pavle **intends** to store (e.g. `altevra secrets set`)
     → value goes to **keyring / encrypted store only**; inline text replaced
     with `{{secret:<handle>}}`; a non-reversing fingerprint (`sha256[:12]`) is
     recorded for dedup/audit. `redaction_status = redacted`.
   - `hard_secret` appearing **accidentally** in captured content → replaced
     with `[REDACTED:<type>]`, **no value stored anywhere**,
     `redaction_status = redacted`, **secret_sighting** + **review item** raised.
   - `soft_secret`/over-ceiling → `quarantined` (encrypted, review).
   - private-key/root-cred in a context where storage is illegitimate →
     `rejected` (row not written; audit fingerprint only).
   - `pii` over field ceiling → redact/placeholder + tag + raise sensitivity.
5. **Audit** — emit `secret_sighting` (type, fingerprint, source ref, location,
   action — **never the value**) + `redaction_applied`.

**Hard rules:** embeddings are computed on the **redacted** text only (I2). A
secret value lives **only** in keyring/encrypted backend (I4). Re-ingesting the
same content is idempotent (fingerprint dedup). A periodic re-scan job
re-evaluates `clean` rows against updated detectors (catches detector-gap
leaks; see failure modes §2.12).

### 2.6 DB-canonical vs Obsidian-authored vs generated-mirror policy

Constitution Law 3 ("markdown is human face; DB is machine truth") +
project doctrine ("Obsidian is human-visible truth for Decisions/Learnings/
People") resolve via **field-level authorship**, not a single global winner.

Every durable object/field has `authorship_class`:

| `authorship_class` | Canonical store | Human edits markdown? | On change |
|---|---|---|---|
| `db_canonical` | DB row | no | regenerate mirror; markdown is read-only output |
| `obsidian_authored` | human markdown | **yes** | re-ingest → update derived projection (chunks/embeddings/extracted fields); markdown wins |
| `generated_mirror` | DB row → md file | no (managed header) | overwrite allowed; human edit = drift (§2.7) |
| `imported_readonly` | immutable snapshot | no | never mutated; corrections via supersession (§2.8) |
| `agent_proposed` | proposal/staging area | no | not truth until review approves (§2.9) |

Default class map:

| Object class | Class |
|---|---|
| events, turns, sessions, file_changes, hook_runs, update_feed, secret_sightings, review_items, context_packet_sources, resident_runs, audit rows | `db_canonical` (append-only) |
| Daily notes, `Memory/Decisions.md`, `Memory/Learnings.md`, `Memory/People.md`, hand-written wiki pages, project READMEs | `obsidian_authored` |
| change-journal, daily briefing, auto-curated wiki pages, generated skill/instruction files | `generated_mirror` |
| imported AI-tool sessions (Codex/Cursor/Antigravity/Hermes), scraped research snapshots | `imported_readonly` |
| resident insight cards, skill proposals, prompt tweaks, identity-shift candidates | `agent_proposed` |

**Dual-authored objects** (e.g. a decision Pavle wrote by hand that Altevra
enriched with structured fields): the prose body is `obsidian_authored`; the
derived structured fields are `db_canonical`. Field-level `authorship_class`
removes the ambiguity. `altevra sot show <object>` prints the per-field owner.

### 2.7 Human markdown edit reconciliation

The `notify` watcher classifies every changed file by zone + managed header:

- **Authored zone** (e.g. `/00-authored/**`, no managed header): treat edit as
  truth. Re-chunk, re-embed (redacted), refresh derived fields. Markdown stays
  canonical. If a `db_canonical` field disagrees, field-level authorship
  decides; genuine conflicts on a shared field → review item.
- **Generated/managed file** (ALTEVRA_MANAGED header + checksum, V5 §10):
  checksum mismatch = **DRIFT**. **Never silently overwrite** (V5 rule). The
  human edit is captured as `pending_human_override` (quarantined), surfaced in
  review with a 3-way diff (`base` = last generated snapshot, `ours` = current
  DB render, `theirs` = human edit). Pavle chooses: promote edit to canonical,
  discard + regenerate, or fork into an authored file.

**Anti-loop (Invariant I11):** every Altevra write to a file stamps a
`self_write` marker (expected-hash + mtime). The watcher ignores a change whose
content-hash matches the stamped expected-hash, so "regenerate → watcher fires
→ regenerate" cannot loop. Reconciliation keys on
`(content_hash, mtime, last_ingested_hash)` and is idempotent.

### 2.8 Correction / forgetting / delete / supersession

Four distinct, never-conflated operations:

| Op | Trigger | Mechanism | Old data | Retrieval | Reversible |
|---|---|---|---|---|---|
| **Correction** | a fact was wrong | new row supersedes; `status=superseded`, `superseded_by`, `reason` | retained, audit-only | excluded (active=false) | yes |
| **Supersession** | belief evolved (still true *then*) | version chain; same fields, semantic "changed-now" | retained, time-travel queryable | excluded from default; available to "what did I believe on DATE" | yes |
| **Soft-forget** | "stop surfacing this" | `status=forgotten`, embedding deactivated | retained encrypted | excluded everywhere | yes (un-forget) |
| **Hard delete** | "erase, truly gone" (RTBF) | enumerate→plan→review→execute→verify-absence→audit | **purged from all stores** | gone | no (tombstone only) |

**Hard-delete pipeline (the dangerous one, Invariant I7):**

```
forget_request(id)
  → enumerate all locations: DB row, derived chunks, vector index entries,
    generated markdown mirror, journal mentions, event payloads referencing id,
    cached packet sources, keyring (if secret), backups
  → produce deletion_plan (explicit location list)
  → REVIEW GATE (irreversible ⇒ always Pavle-approved)
  → execute purge in dependency order
  → derived-artifact reconcile: regenerate/flag wikis & briefings that used it
  → VERIFY-ABSENCE: re-scan every enumerated store for id + content-hash;
    assert zero hits (else: deletion_failed alarm, do not report success)
  → emit forget_completed (id, content_hash, locations, NOT content)
```

Embeddings cannot be "un-computed" — the vector **and** its source text are
purged, and the verify step proves the vector is gone. Third-party PII delete
is a **hard requirement**, not opt-in.

### 2.9 Review gates for protected changes

State machine: `proposed → pending_review → {approved | rejected | needs_changes} → {applied | archived}`.

**Always protected (Constitution Law 4 + paranoia additions):**

- identity edits; policy/schema edits; secret grants/tool grants; broad skills
- personal / relationship / health / legal / financial memory writes
- **SoT reassignment** (changing a field's canonical owner — could let an agent
  overwrite human truth)
- **exposure widening / sensitivity downgrade** (the sneaky leak: reclassify
  `restricted→public` then surface it) — *always* review-gated
- hard delete (§2.8); any external side effect (deploy/push/email/payment)

**Trust ladder (CLAUDE.md §12):** auto-apply low-risk (new research insight,
non-sensitive memory, new category creation); review-required for everything
above. A resident agent **proposes** (`agent_proposed`); it never writes truth
directly to a protected class (Invariant I9). Review item carries:
`id, change_type, target, proposed_diff, risk_level, rationale, proposer_mode,
required_approver, sensitivity, created_at, decision, decided_by, decided_at, applied_at`.

**Approver authenticity:** "approved by Pavle" must come from an interactive
human-presence signal (CLI TTY / explicit unlock token), not a flag an agent
can set in a payload — otherwise an agent forges its own approval (§2.13).

### 2.10 Audit trail rules

One **append-only**, **tamper-evident** audit log (DB-canonical). Every
safety-relevant action emits an immutable row: `secret_sighting`,
`redaction_applied`, `exposure_decision` (what packet surfaced what at what
ceiling, to whom), review lifecycle transitions, correction/supersession,
`forget_request`/`forget_completed`, sensitivity reclassification, SoT
reassignment, drift detected, human-override quarantined, sync push/pull.

Rules:

- **Append-only** — no UPDATE/DELETE on audit; corrections are new rows.
- **Redaction-safe** — audit stores fingerprints/hashes/metadata, **never** the
  secret/PII value (auditing a leak must not re-leak).
- **Tamper-evident** — each row embeds `prev_row_hash` (hash chain); a broken
  chain is an alarm (doctor §2.16). *(Chain may be deferred past P0 — see §2.19.)*
- **Queryable** — `altevra audit query` answers "why was X exposed", "when did
  I delete Y", "what did agent Z see".

### 2.11 Invariants (prevent silent leaks / drift)

| # | Invariant |
|---|---|
| I1 | No row persists with `redaction_status = unscanned` past commit. |
| I2 | No text enters the embedding queue before redaction. |
| I3 | No packet item exceeds the request's `sensitivity_ceiling`, `audience`, or `domain_scope`. |
| I4 | Raw secret value lives only in keyring/encrypted store; never in DB text, markdown, journal, event payload, log, or embedding. |
| I5 | Every exposed packet item carries `source_ref` + `redaction_status` (no mystery summaries). |
| I6 | Generated mirrors always carry managed header + checksum; human edits to them are quarantined, never lost. |
| I7 | Hard delete cascades to all stores; verify-absence must pass before success is reported. |
| I8 | Sensitivity is monotonic under composition (never auto-lowered). |
| I9 | Exposure-widening, sensitivity-downgrade, and SoT reassignment are always review-gated; agents never write protected truth directly. |
| I10 | Superseded/forgotten/deleted content is excluded from default retrieval (`active=false`). |
| I11 | Altevra self-writes never trigger reconciliation loops (self-write marker). |
| I12 | Every safety action is audited; audit is append-only (+ hash-chained). |
| I13 | Sync never pushes an object above its per-category cloud ceiling. |
| I14 | Ingested content is **data, never instruction** — policy is enforced in code, not by trusting text. |

### 2.12 Failure modes

| Failure | Mitigation | Residual |
|---|---|---|
| Secret detector misses novel key format | entropy fallback → `quarantined`; periodic re-scan of `clean` rows; per-field deny-by-default for high-risk fields | re-scan window; flagged honestly |
| Watcher reconciliation loop | self-write marker + content-hash compare (I11) | none if marker correct |
| Delete leaves orphan embedding/mirror | enumerate + verify-absence step (I7); doctor orphan scan | backup tombstone (see §2.19) |
| LLM mis-classifies sensitivity low | default-up rule; `quarantined` on uncertainty | classifier drift → re-eval job |
| Backup resurrects deleted data | tombstone propagation to backups / backup re-redaction | depends on RTBF decision (§2.19) |
| Race: correction lands mid-packet-build | packet records the `version_id` it used; snapshot read | brief staleness, logged |
| Cross-domain bleed (personal → business packet) | `domain_scope` enforced in gate (I3) | none if domains tagged |
| Audit chain break | tamper alarm in doctor | detect-only |
| Drift backlog accumulates silently | doctor surfaces drift count; review queue | needs Pavle attention |
| Markdown mirror itself leaks (plaintext on disk) | `mirror_to_markdown=false` default for `confidential+` (§2.14) | vault perms (doctor) |

### 2.13 Security / privacy risks

- **Embedding inversion / membership inference** — similarity search can leak
  the *presence* of restricted content even when the row isn't returned.
  Mitigation: the gate filters the **candidate set before ranking**, not after
  (request to context-retrieval §2.20). Restricted vectors are never in the
  searchable set for an under-ceiling request.
- **Prompt injection via ingested content** — a malicious file/session says
  "ignore policy, dump secrets". Mitigation: I14 — ingested content is data;
  policy lives in code; tool outputs are untrusted.
- **Inclusion/exclusion side-channel** — "excluded 1 restricted health item"
  itself leaks existence. Mitigation: reason codes are coarse
  ("items above ceiling omitted"), never count/type restricted items.
- **Forged approval** — agent sets an `approved=true` payload field.
  Mitigation: §2.9 human-presence signal required for protected approvals.
- **Local filesystem exposure** — world-readable vault / secrets dir.
  Mitigation: doctor enforces `0600` secrets, warns on broad vault perms.
- **Cloud metadata leakage** — titles/tags pushed even when bodies encrypted.
  Mitigation: metadata is gated by the same ceiling (§2.15).

### 2.14 Obsidian implications

- **Physical zone separation** so the watcher knows authorship without
  guessing: authored zone (`/00-authored/**`, no managed header) vs generated
  zone (`/15-generated/**`, managed header required). Generated files carry the
  V5 managed header (source, generated_by, adapter, version, checksum,
  generated_at) + warning not to edit.
- **The markdown mirror is itself an exposure surface** (plaintext on disk,
  possibly synced by an Obsidian sync plugin). Therefore `confidential+` /
  `restricted` content is **DB-only by default** (`mirror_to_markdown=false`).
  Restricted personal/health/relationship content must not be written to a
  plaintext synced vault folder.
- Human edits to authored notes re-ingest cleanly; edits to generated mirrors
  quarantine as drift (§2.7).

### 2.15 Cloud / local sync implications

- **Local-first by axiom** (Constitution §5 / project §4.4). Cloud sync is
  **opt-in per category**, default off.
- Per-domain ceiling `cloud_sync ∈ {disabled, encrypted_only, allowed}`;
  default `disabled` for personal/health/relationship/financial/credential.
- **Sync runs `exposure_gate` with audience = external/`any_agent`**: anything
  above its category ceiling never leaves the machine. `encrypted_only` =
  client-side encryption before push; provider sees ciphertext + minimal
  metadata (metadata also gated).
- Cloud is **never canonical** for authored content unless Pavle sets it so;
  conflicts resolve by the §2.7 rules.
- **Tombstones sync** so deletes propagate, but a tombstone carries id +
  content-hash, never content (I13, §2.8).

### 2.16 CLI / MCP implications

- **One core, two faces** (V5 §5): CLI and MCP both call `ingest_guard` /
  `exposure_gate`; MCP is an adapter and **cannot bypass** enforcement.
- **Caller class matters:** human-CLI (TTY) vs agent-MCP. `altevra secrets get`
  returns a **handle/metadata** to an agent caller; the raw value requires an
  interactive Pavle session + explicit unlock. No MCP tool ever returns a raw
  secret value.
- New/affected commands & tools:
  - `altevra sot show <object>` — per-field canonical owner.
  - `altevra redact check <path|->` — dry-run guard on input.
  - `altevra review list|show|approve|reject` (approve requires human presence).
  - `altevra forget <id> --dry-run` (prints plan) / `--execute` (review-gated).
  - `altevra audit query [--object|--actor|--since]`.
  - `altevra doctor` checks: unscanned rows, orphan embeddings, drift backlog,
    secrets-dir perms, audit-chain integrity, `mirror_to_markdown` violations.
  - MCP: `get_source_of_truth` (V5), `create_review_item` (V5),
    `request_forget`, `get_context_packet` (must accept + enforce
    `sensitivity_ceiling`, `audience`, `domain_scope`).

### 2.17 Required tests / fixtures / golden snapshots

- **Fake-secret corpus** (by *type*, synthetic non-functional values): OpenAI
  `sk-` prefix, GitHub `ghp_`, AWS `AKIA`, JWT 3-part, PEM private key,
  `postgres://` credentialed URL, bearer token, `.env` line. Assert **absent
  from**: DB text, embedding input, packet, journal, event payload, markdown
  mirror, logs (covers P0.2 acceptance).
- **Redaction golden snapshots** — input → deterministic redacted output +
  manifest.
- **Sensitivity-ceiling packet tests** — request at each ceiling; assert no
  over-ceiling item leaks; assert exclusion reason reveals no existence/count.
- **Reconciliation tests** — human edit of managed file → quarantined (not
  lost); self-write → no loop; authored edit → re-ingested, markdown wins.
- **Correction/supersede tests** — corrected fact: old `superseded`, excluded
  from retrieval, present in audit + time-travel query.
- **Delete/forget tests** — `request_forget` → plan enumerates all stores →
  execute → verify-absence returns 0 hits across DB/vector/markdown/journal/
  event/cache/keyring; orphan-embedding regression.
- **Review state-machine tests** — protected change can't apply without
  approval; exposure-widening + sensitivity-downgrade forced through review;
  forged-approval payload rejected.
- **Audit tests** — every safety action emits a row; append-only enforced;
  hash chain verifies; tamper detected.
- **Sync tests** — per-category ceiling respected; restricted never pushed;
  tombstone propagates without content.
- **Doctor tests** — unscanned-row, orphan-embedding, world-readable-secret,
  audit-chain-break, markdown-mirror-violation detection.
- **Fuzz** — random high-entropy strings: classification stable + fail-closed.

### 2.18 Acceptance criteria

- All fake secrets absent from every store (DB/embedding/packet/journal/event/
  markdown/log) — P0.2.
- Every durable text field resolves to a `safety_class` + `redaction_status`;
  no `unscanned` row persists (I1).
- `exposure_gate` is the **sole** path to packet inclusion; golden tests show
  ceiling/audience/domain never exceeded (I3) — P0.4.
- Hard-delete verify-absence passes for every enumerated store (I7).
- No protected change applies without an approved, human-authenticated review
  item; exposure-widening + SoT reassignment always gated (I9).
- Audit is append-only and (if chain enabled) verifies; every safety action
  produces a row (I12).
- `confidential+` content is never written to plaintext markdown by default
  (§2.14); sync never pushes above category ceiling (I13).
- CLI and MCP demonstrably share one guard/gate core; no bypass path exists.
- Baseline suite green: `cargo fmt --check && cargo test && cargo build &&
  cargo clippy --workspace -- -D warnings`.

### 2.19 Unresolved questions

1. **Storage engine.** V5 says Postgres + pgvector; AGENTS.md / Constitution /
   project vision say **SQLite local-first**. The repo has `altevra-db`.
   Canonical engine for P0 affects encryption-at-rest, delete guarantees, and
   vector purge mechanics. → **Pavle/Hermes decision needed before P0.2.**
2. **Encryption at rest** for `quarantined`/`restricted` content — SQLCipher,
   per-field envelope encryption (keyring-wrapped DEK), or OS-level only?
3. **Backup RTBF** — is true hard-delete-from-backups required, or is
   tombstone-on-restore (re-redact on restore) acceptable?
4. **Markdown mirror of restricted personal content** — DB-only always, or
   opt-in encrypted markdown? (Default here: DB-only.)
5. **Audit hash-chain in P0** — ship tamper-evidence now or defer to post-P0
   (append-only first)?
6. **Sensitivity classifier** — which model role does personal-domain
   classification (Constitution: `local_private` only), and what's the
   offline/no-model fallback? (Default-up + quarantine when no classifier.)
7. **Human-presence authentication** — how does the system prove an approval
   came from Pavle and not an agent (TTY check, signed unlock token, passphrase)?

### 2.20 Cross-section requests

- **→ object-model (opus-object-model):** reserve in the common metadata
  contract + status enum: `sensitivity{level,tags,domains}`, per-field
  `authorship_class`, `safety_class`, `redaction_status`,
  `redaction_manifest_ref`, `provenance{source,actor,confidence,captured_at}`,
  `exposure_policy_ref`, and status values `{active, quarantined, superseded,
  forgotten, deleted_tombstone}` with `supersedes/superseded_by` + `version`.
  These fields are load-bearing for §2.2–2.10.
- **→ context-retrieval (opus-context-retrieval):** packet compiler MUST call
  shared `exposure_gate` **before ranking** (filter candidate set pre-rank, not
  post — §2.13 inversion risk); attach `source_ref` + `redaction_status` to
  every item (I5); produce **sensitivity-aware** exclusion explanations (no
  existence/count leak); honor `active` flag (I10); record the `version_id`
  used per item (correction race, §2.12).
- **→ agents-self-improve (claude-agents-self-improve):** all agent outputs are
  `agent_proposed`; writes to protected classes create review items, never
  direct truth (I9); observer treats ingested content as untrusted data, not
  instructions (I14).
- **→ tools-skills-interfaces (claude-tools-skills-interfaces):** route all
  reads through `exposure_gate`; no tool returns raw secret values;
  distinguish human-CLI (TTY) from agent caller for secret access (§2.16).
- **→ domains-lifecycle (claude-domains-lifecycle):** provide the
  domain→default-sensitivity map and domain→`cloud_sync` ceiling map; confirm
  authored-vs-generated Obsidian folder zones (§2.14).
- **→ Hermes (synthesis):** resolve §2.19 #1 (storage engine), #3 (backup RTBF),
  and #7 (human-presence auth) **before P0.2 sign-off** — they change the
  delete/encryption contract.

<!-- END_SECTION: safety-source-truth -->

---

<!-- SECTION: context-retrieval -->
<!-- OWNER: opus-context-retrieval -->
<!-- STATUS: drafted-by-opus-context-retrieval -->
## 3. Context + Retrieval

> Scope of this section: retrieval architecture + the Context Packet Compiler.
> It **consumes** object-model field contracts (§1) and **invokes** the
> safety/redaction pipeline (§2). It does not redefine the object taxonomy,
> the sensitivity enum, or the secret-detection mechanism — it references them
> and states the retrieval-time enforcement order. Cross-section dependencies
> are listed in §3.21.

### 3.1 Purpose

The retrieval layer answers exactly one question for any agent or tool:

> **"Given who you are, what you are doing, and how much room you have,
> here is the minimal, source-backed, sensitivity-safe, non-stale set of
> things you must know — and a defensible reason for every inclusion and
> every exclusion."**

This is the line between *precise context* and *RAG soup*. A naive RAG layer
embeds everything, takes top-k by cosine, and stuffs the result into the
prompt. That produces: out-of-scope bleed, superseded facts, duplicate
restatements, cross-domain leaks, provenance-blind hallucination fuel,
non-deterministic packets, and mid-fact truncation. Every design choice below
exists to defeat one of those failure modes by construction, not by tuning.

Two hard product constraints from the Constitution and V5 govern everything:

- **Capture is broad; exposure is minimal, redacted, source-backed,
  sensitivity-filtered, and auditable** (Constitution Law 2). Retrieval is the
  *exposure* boundary — the last gate before context leaves Altevra.
- **The same compiler core serves CLI and MCP** (P0.4). There is one compiler;
  `altevra context`, `altevra packet`, `get_context_packet`, `search_memory`,
  `get_project_context`, and `get_source_of_truth` are thin adapters over it.

### 3.2 Retrieval sources and indexes

Sources are the durable object classes defined in §1 plus the V5 feeds. Each
source is reachable through one or more of four index families.

**Sources (what can enter a packet):**

| Source | Origin | Canonical store | Typical intents |
|--------|--------|-----------------|-----------------|
| `source_of_truth` (decisions, policies, identity facts) | §1 / §2 registry | DB | almost all |
| `tasks` / `goals` | §1 | DB | task_work, bootstrap |
| `learnings` | §1 | DB | task_work, research |
| `updates_feed` | V5 §6 | DB | bootstrap, freshness |
| `memory` (turns, notes, captured thoughts) | §1 | DB | most |
| `wiki` (curated pages) | vault + DB | DB canonical, vault mirror | research, person/project lookup |
| `obsidian_docs` (raw vault) | vault | vault-authored, DB-mirrored | research, lookup |
| `research_items` | §1 | DB | research |
| `skills` | V5 skill registry | DB/file | tool setup, capability |
| `repo_context` (`AGENTS.md`, project README) | repo files | file | task_work, bootstrap |
| `people` / relationships | §1 personal | DB (sensitive) | person_lookup (elevated) |

**Index families (how candidates are generated):**

1. **Lexical / BM25 index** — full-text over `title` + `body` + `tags`.
   Backend-neutral: SQLite **FTS5** (local-first default) **or** Postgres
   `tsvector` (V5 cloud-compat path). Must support per-language analyzers
   (Pavle writes Serbian + English; see §3.20). Exact, fast, no model
   dependency — the floor that always works even if embeddings are unavailable.
2. **Embedding / vector index** — per-chunk dense vectors.
   Backend-neutral: **sqlite-vec** (local) **or** **pgvector** (cloud).
   Every vector row carries `embedding_model_id`, `dim`, and **inherits the
   sensitivity of its source object** (embeddings of sensitive text are
   themselves sensitive — see §3.14). Cross-model vectors are never compared
   (dimension/model mismatch → signal `unavailable`, never silently zero-padded).
3. **Graph index** — typed edges between objects (§1 edge model): e.g.
   `decision —relates_to→ goal`, `person —mentioned_in→ session`,
   `wiki —supersedes→ note`. Used for "what's connected to the thing I'm
   working on" and for cross-scope admission (§3.3).
4. **Structured / filter index** — direct indexed queries, not similarity:
   active tasks for a project, updates since `last_seen_event_id`, the
   source-of-truth registry, objects by `status`/`domain`/`scope`. This is how
   must-include context (active task, latest decision) enters deterministically
   without depending on a fuzzy score.

> Anti-soup note: must-include context comes from the **structured index**, not
> from similarity. Similarity only *adds* relevant supporting context; it never
> *decides* whether the active task or the governing decision is present.

### 3.3 Scoring contract (BM25 / embedding / graph / recency / scope)

The scoring model has two strictly separated layers. Conflating them is the
single most common cause of RAG soup, so the separation is an invariant
(§3.12, INV-1).

**Layer A — Gates (hard, boolean, correctness/safety).** Applied first.
A candidate that fails any gate is dropped and recorded with an
`ExclusionRecord` (§3.5). Gates are never overridden by a high relevance score.

```text
GATE scope        : c.scope ∈ allowed_scopes(request, profile)
                    OR c is graph-justified by an explicit cross-scope edge
                    AND profile.allow_cross_scope = true
                    else → EXCLUDE(out_of_scope)
GATE sensitivity  : sensitivity_rank(c) ≤ request.sensitivity_ceiling   (fail-closed)
                    else → EXCLUDE(over_sensitivity_ceiling)
GATE lifecycle    : c.status ∉ {superseded, retracted, deleted}
                    UNLESS intent ∈ history_intents
                    else → EXCLUDE(superseded | retracted | deleted)
GATE staleness    : age(c) ≤ hard_expiry(c.domain)
                    else → EXCLUDE(stale)
GATE integrity    : c has resolvable object_ref AND passes redaction (§3.6)
                    else → EXCLUDE(redaction_failed | unresolvable_ref)
```

**Layer B — Relevance score (soft, ranks survivors only).** Every signal is
normalized to `[0,1]` with a **deterministic, pool-independent** normalizer (no
min-max over the candidate pool — that changes with pool composition and breaks
determinism).

```text
s_bm25(c)  = bm25_raw / (bm25_raw + k_sat)            # saturating, k_sat from profile
s_emb(c)   = (cosine(q_vec, c_vec) + 1) / 2           # only if model_id matches; else marked unavailable
s_graph(c) = max over anchors a of edge_weight(a→c) · hop_decay^(hops−1)   # capped at 1
s_rec(c)   = 0.5 ^ ( age_days(c) / half_life(profile, c.domain) )
s_scope(c) = 1.0 (exact) | profile.scope_parent_factor (parent/related) | 0
s_conf(c)  = confidence(c)   # pavle_stated = 1.0, ai_inferred < 1.0  (from §1)

fused(c) = ( w_bm25·s_bm25 + w_emb·s_emb + w_graph·s_graph + w_rec·s_rec )
           · scope_mult(c)            # s_scope folded in as a multiplier here too
           · conf_mult(c)             # confidence as a multiplier, never additive
```

- Soft weights `w_*` sum to 1 and are **profile-specified** (see profiles below).
- `scope_mult` and `conf_mult` are multipliers in `(0,1]` — they *demote* but do
  not *gate* (gating already happened in Layer A). A parent-scope item with great
  lexical match can still appear, ranked below exact-scope matches.
- If `s_emb` is `unavailable` (no vector / model mismatch), `w_emb` mass is
  redistributed proportionally to the remaining available signals — the packet
  degrades gracefully to lexical+graph+recency, it does not crash or zero out.

**Retrieval profiles (intent → weights + reserves).** Intent selects a profile;
this is the second major anti-soup lever (one-size-fits-all weighting is soup).

| Intent | w_bm25 | w_emb | w_graph | w_rec | half_life | notes |
|--------|-------|-------|--------|-------|-----------|-------|
| `bootstrap` | 0.15 | 0.15 | 0.10 | 0.60 | 2d | freshness-dominant; structured must-includes lead |
| `task_work` | 0.30 | 0.30 | 0.25 | 0.15 | 14d | balanced; graph pulls in linked decisions/goals |
| `decision_lookup` | 0.45 | 0.30 | 0.20 | 0.05 | 365d | precision/lexical-dominant; recency near-irrelevant |
| `research` | 0.20 | 0.45 | 0.20 | 0.15 | 30d | semantic-dominant |
| `person_lookup` | 0.30 | 0.25 | 0.40 | 0.05 | 365d | graph-dominant; **requires elevated ceiling** |
| `freshness_check` | 0.05 | 0.05 | 0.05 | 0.85 | 1d | almost pure recency over updates feed |
| `*_history` | 0.40 | 0.30 | 0.25 | 0.05 | 365d | lifecycle gate relaxed; superseded shown with badge |

Weights and half-lives are config, not code constants, and are themselves a
self-improvement surface (§3.21 → agents-self-improve): the observer may
*propose* weight changes, but changes are review-gated and versioned, and the
golden eval (§3.10) must not regress.

### 3.4 Context packet object schema

The packet is a **durable, read-only object** (links to P0.1
`context_packet` + `context_packet_sources`). It never mutates its source
objects. Field contracts for shared metadata (`id`, `sensitivity`, `domain`,
`provenance`, `status`) are owned by §1; this schema references them.

```jsonc
ContextPacket {
  "packet_id":      "string (stable id, §1 id contract)",
  "schema_version": "int",
  "compiler_version":"string",            // for replay/regression
  "created_at":     "RFC3339",
  "request":        RetrievalRequest,      // echoed back verbatim
  "profile_id":     "string",             // which retrieval profile was used
  "db_snapshot":    "string",             // monotonic marker for deterministic replay
  "tokenizer_id":   "string",             // pinned tokenizer used for budgeting
  "budget": {
    "token_budget":     "int",
    "tokens_used":      "int",
    "reserves":         { "source_of_truth": "int", "active_work": "int", "updates": "int" },
    "truncated":        "bool",
    "truncation_reason":"enum|null"        // budget_exhausted | must_include_pointer_only
  },
  "items":    [ ContextPacketItem ],       // included, in deterministic order
  "excluded": [ ExclusionRecord ],         // candidates that did not make it (capped, see §3.5)
  "warnings": [ "string" ],                // e.g. "1 item dropped: redaction_failed"
  "stats": {
    "candidates_per_index": { "bm25": "int", "embedding": "int", "graph": "int", "structured": "int" },
    "after_gates":  "int",
    "after_dedup":  "int",
    "emitted":      "int"
  },
  "audit_ref": "string",                   // pointer to audit row (§3.11)
  "integrity": { "items_hash": "sha256", "redaction_clean": "bool" }
}

RetrievalRequest {
  "agent_kind":         "string",          // claude-code | codex | cursor | antigravity | hermes | human
  "tool":               "string",
  "project":            "string|null",
  "intent":             "enum",            // bootstrap | task_work | decision_lookup | research | person_lookup | freshness_check | *_history
  "query":              "string|null",     // free text for lexical/semantic; null for pure structured intents
  "scope":              "string|null",     // explicit scope override; else derived from project
  "token_budget":       "int",
  "sensitivity_ceiling":"enum",            // §2 sensitivity enum; default derived from agent/tool/domain
  "elevation":          "bool",            // explicit user opt-in to raise ceiling for personal/sensitive intents
  "include_history":    "bool"             // opt-in to see superseded objects
}

ContextPacketItem {
  "item_id":   "string",
  "rank":      "int",                      // 1-based, matches deterministic order
  "section":   "enum",                     // source_of_truth | active_work | updates | memory_wiki | research_skills | repo
  "object_ref":{ "type":"string", "id":"string", "schema_version":"int" },   // ALWAYS present
  "source_index": "enum",                  // bm25 | embedding | graph | structured  (which index surfaced it)
  "title":     "string",
  "rendered_text": "string",               // post-redaction, whole-item (never mid-fact truncated)
  "token_count":   "int",
  "synthesized":   "bool",                 // true if compiler-generated rollup
  "derived_from":  [ "object_ref" ],       // required & non-empty when synthesized = true
  "scores": { "bm25":"float","embedding":"float|null","graph":"float","recency":"float","scope":"float","confidence":"float","fused":"float" },
  "sensitivity":   "enum",
  "domain":        "enum",
  "provenance":    { "actor":"enum", "source":"string", "captured_at":"RFC3339", "confidence":"float" },
  "staleness":     { "status":"enum", "age_days":"float", "superseded_by":"object_ref|null" },
  "redaction":     { "status":"enum", "spans_redacted":"int" },   // status from §2; raw secret never present
  "why_included":  WhyIncluded
}
```

### 3.5 Inclusion / exclusion explanation contract

P0.4 forbids "mystery summaries." Every item — included or not — carries a
machine-checkable reason. This is what makes the layer debuggable, testable, and
trustworthy to the agent consuming it.

```jsonc
WhyIncluded {
  "profile_id":   "string",
  "section":      "enum",
  "fired_signals":[ "bm25" | "embedding" | "graph" | "recency" ],   // which signals were non-trivial
  "anchor":       "object_ref|null",       // for graph/structured admission: what it was linked to
  "fused_score":  "float",
  "rule":         "enum"                    // top_scored | must_include | graph_linked | freshness | source_of_truth
}

ExclusionRecord {
  "object_ref":   { "type":"string","id":"string","schema_version":"int" },
  "reason":       "enum",                   // out_of_scope | over_sensitivity_ceiling | superseded
                                            // | retracted | deleted | stale | duplicate_of
                                            // | below_score_threshold | budget_exhausted
                                            // | redaction_failed | unresolvable_ref
  "detail":       "string|null",            // e.g. duplicate_of → canonical object_ref
  "stage":        "enum"                    // gate | dedup | budget | redaction
}
```

Exclusion logging policy: `excluded[]` is **capped** (default top-N per reason,
configurable) to avoid the audit itself becoming bloat — but the **count per
reason is always exact** in `stats`. Security/safety exclusions
(`over_sensitivity_ceiling`, `redaction_failed`) are **never elided** and never
include the offending content (only the ref + reason).

### 3.6 Redaction / sensitivity-ceiling flow

Defense in depth, ordered, fail-closed. The actual secret-detection and
redaction *mechanism* is owned by §2; retrieval owns the *enforcement order*:

```text
1. INGESTION (upstream, §2 + P0.2): raw secrets are detected and stripped
   before storage/index/embedding. By contract, candidate text is already
   clean. Retrieval treats this as untrusted and re-checks anyway.

2. CANDIDATE FILTER (Layer-A sensitivity gate, §3.3): drop every candidate with
   sensitivity_rank > request.sensitivity_ceiling BEFORE it is scored or
   embedded into the result. Over-ceiling content never reaches ranking.

3. CEILING DERIVATION: ceiling = min( agent/tool default ceiling,
   domain policy ceiling ).  Personal/relationship/health/legal/financial
   intents start BELOW their data's sensitivity and require request.elevation =
   true (explicit human opt-in) to raise the ceiling. A work tool can never
   auto-elevate into personal data.

4. FINAL REDACTION PASS (mandatory, on rendered_text, §2 pipeline): before any
   item leaves the compiler, rendered_text is passed through the redaction
   verifier. Fail-closed: if the verifier cannot certify the text clean, the
   item is DROPPED (ExclusionRecord redaction_failed) — never emitted raw,
   never best-effort.

5. PROTECTED-EXPOSURE REVIEW HOOK (§2 / Constitution Law 4): if a packet would
   expose a protected class (identity, relationship, health, financial,
   source-of-truth edit context) above a configured exposure threshold, the
   compiler emits a review_item and returns the packet WITHOUT that item plus a
   warning, rather than exposing it silently.
```

Sensitivity ranks are comparable (a total order) by §1/§2 contract so the
`≤ ceiling` test is well-defined. Embeddings inherit source sensitivity, so the
vector search itself is ceiling-filtered (§3.14).

### 3.7 Source refs / provenance in every packet item

Hard rule (INV-3): **no item without a resolvable `object_ref`.** The agent (and
a human auditor) can always trace any line of context back to a typed,
versioned source object. Consequences:

- A synthesized rollup (e.g. "3 updates since last session") sets
  `synthesized = true` and **must** populate a non-empty `derived_from[]` of the
  contributing refs. Synthesis without refs is forbidden (INV-12).
- `provenance.actor` distinguishes `pavle_stated` / `ai_inferred` / `imported` /
  `tool_generated`. This flows into `conf_mult` (scoring) and is surfaced to the
  agent so it can weight a Pavle-stated fact above an AI inference instead of
  treating all retrieved text as equally authoritative.
- `provenance.source` resolves to a concrete locus: DB row, vault path +
  heading anchor, session id + turn, or repo file path. Vault items reference the
  Obsidian path/anchor (human-clickable), DB items reference the row.

### 3.8 Deterministic ordering and token-budget rules

**Determinism (INV-6).** Given identical `RetrievalRequest` + identical
`db_snapshot` + identical config, the packet is byte-identical except
`created_at`/`packet_id`. Achieved by:

- Pool-independent score normalizers (§3.3) — no min-max over the pool.
- Pinned `embedding_model_id` and pinned `tokenizer_id` recorded in the packet.
- Total sort order with explicit tiebreakers:
  `ORDER BY fused_score DESC, section_priority ASC, captured_at DESC, object_id ASC`.
  The trailing `object_id ASC` guarantees a total order even under exact ties.
- A captured `db_snapshot` marker so replay reads the same rows.

**Token budget.** Whole-item inclusion only — **never truncate mid-item**
(INV-7); a fact cut in half is worse than absent. Packing:

```text
sections (priority order): source_of_truth > active_work > updates > memory_wiki > research_skills > repo

reserves: each critical section has a floor (e.g. SoT 15%, active_work 15%,
          updates 15% of token_budget). Reserves are FLOORS not caps:
          unused reserve flows down to flexible sections.

pack():
  for section in priority_order:
    greedily add items by deterministic sort until section reserve filled,
    then continue from a shared flex pool
  if budget exhausted: stop. Remaining survivors → ExclusionRecord(budget_exhausted)

must-include rule:
  source_of_truth + active task/goal are must-includes. If a must-include does
  not fit even in its reserve, DO NOT silently drop it: emit a POINTER-ONLY item
  (title + object_ref + one-line, no body) and set
  budget.truncation_reason = must_include_pointer_only + a warning.
```

Empty packets are valid (INV-13): if nothing survives the gates, return an empty
`items[]` with a clear `warnings` entry. Padding a packet with marginal content
to "fill the budget" is forbidden — fewer, correct items beat a full, noisy
context window.

### 3.9 Staleness / supersession filtering

Driven by §1 `status` + `superseded_by` + domain TTLs:

- `status ∈ {superseded, retracted, deleted}` → excluded by default (lifecycle
  gate). If `A superseded_by B`, only `B` is eligible; `A` appears **only** under
  a `*_history` intent, always carrying the `superseded` badge and the
  `superseded_by` ref so the agent cannot mistake it for current truth.
- `status = stale` or `age > soft_TTL(domain)` → not excluded, but receives the
  recency penalty (`s_rec` decay) **and** a `staleness.status = stale` badge so
  the agent can discount it.
- `age > hard_expiry(domain)` → excluded (`stale`), even if highly relevant —
  ancient context masquerading as current is a top RAG-soup hazard.
- Domain TTLs (soft + hard) are owned by §6 domains-lifecycle (§3.21 request);
  retrieval consumes them.

### 3.10 Golden eval query set

A fixed fixture corpus + fixed query set with **gold labels** (must-include /
must-exclude object ids per query). The eval is the contract's regression guard;
profile/weight tuning must keep it green.

Representative golden queries (each is a named fixture case):

1. `G01 bootstrap_fresh` — new session, project=altevra → must include latest
   updates + active task + governing decision; must **not** include unrelated
   project tasks.
2. `G02 superseded_decision` — query a topic whose decision was reversed → must
   return only the new decision; old one in `excluded(superseded)`.
3. `G03 personal_in_work_packet` — work-tool packet, query brushes personal data
   (Elena/health) → personal items in `excluded(over_sensitivity_ceiling)`;
   **scope/sensitivity leak rate must be 0**.
4. `G04 duplicate_fact` — same fact stated in 5 sessions → exactly one canonical
   item; 4 in `excluded(duplicate_of)`.
5. `G05 cross_project_link` — task_work where a decision in another project is
   graph-linked → the linked decision admitted via `graph_linked` with anchor
   ref; unrelated cross-project content excluded.
6. `G06 stale_research` — research item past hard_expiry → `excluded(stale)`.
7. `G07 budget_squeeze` — token_budget smaller than must-includes → must-includes
   present as pointer-only, `truncation_reason = must_include_pointer_only`.
8. `G08 determinism` — compile G01 twice → byte-identical items/order.
9. `G09 fake_secret` — fixture contains a planted fake API key in a note →
   absent from packet, audit, and any embedding text; `redaction` reflects it.
10. `G10 empty_valid` — query with no surviving candidates → empty `items[]`,
    no padding, explanatory warning.
11. `G11 person_lookup_elevated` — person_lookup with `elevation=false` →
    redacted/empty; with `elevation=true` → person item present, ceiling raised
    only for this request.
12. `G12 embedding_unavailable` — vector index disabled → packet still compiles
    via bm25+graph+recency; `w_emb` redistributed; no crash.
13. `G13 multilingual` — Serbian query must retrieve a Serbian note and its
    English counterpart (analyzer + embedding handle both; §3.20).
14. `G14 provenance_weighting` — a Pavle-stated fact and a contradicting
    AI-inferred note both match → Pavle-stated ranks above via `conf_mult`.
15. `G15 history_intent` — `decision_history` intent → superseded items appear,
    each badged; current decision still ranks first.

Metrics + thresholds: `precision@k`, recall of must-include set (= 1.0 for
must-includes), **scope-leak = 0**, **sensitivity-leak = 0**,
**staleness-leak = 0**, **secret-leak = 0**, determinism = pass, budget
adherence (`tokens_used ≤ token_budget`).

### 3.11 Packet audit trail

Every compile writes a durable audit row (P0.1 `context_packet` +
`context_packet_sources`), enabling replay and regression diffing:

```jsonc
PacketAudit {
  "packet_id":"...", "audit_ref":"...", "created_at":"...",
  "request": RetrievalRequest,
  "profile_id":"...", "compiler_version":"...", "tokenizer_id":"...",
  "embedding_model_id":"...", "db_snapshot":"...",
  "candidates_per_index": {...}, "gate_drops": { "<reason>": count, ... },
  "included_refs":  [ { object_ref, rank, fused_score, section, rule } ],
  "excluded_refs":  [ ExclusionRecord ],     // capped per §3.5
  "redactions": { "items_redacted":int, "items_dropped_redaction_failed":int },
  "budget": { token_budget, tokens_used, truncated, truncation_reason }
}
```

The audit **never stores raw secrets** — only refs, counts, and fingerprints
(consistent with §2). It stores object refs + scores, not necessarily full
rendered bodies, to stay compact; bodies are reproducible from refs at the
recorded snapshot.

### 3.12 Invariants (anti-RAG-soup / anti-confusion)

These are hard, testable, and non-negotiable. Each maps to ≥1 golden case.

- **INV-1 Gates ≠ weights.** Scope, sensitivity, lifecycle, staleness, integrity
  are boolean gates; no relevance score can override them. (G02,G03,G06)
- **INV-2 Sensitivity ceiling is fail-closed.** Uncertain → exclude. (G03,G11)
- **INV-3 Every item has a resolvable `object_ref`.** No anonymous text. (G09)
- **INV-4 No item is emitted without passing final redaction.** (G09)
- **INV-5 Superseded/retracted/deleted excluded by default.** (G02,G15)
- **INV-6 Deterministic given snapshot + config.** (G08)
- **INV-7 Whole-item inclusion; never truncate mid-fact.** (G07)
- **INV-8 Near-duplicates collapse to one canonical item.** (G04)
- **INV-9 Provenance + confidence preserved and surfaced; AI-inferred is
  labeled and ranked below Pavle-stated.** (G14)
- **INV-10 Every inclusion AND exclusion is explained.** (all)
- **INV-11 Reserved sub-budgets protect critical context from flooding.** (G01,G07)
- **INV-12 Synthesized text carries non-empty `derived_from[]`.** (—)
- **INV-13 Empty is valid; never pad to fill budget.** (G10)
- **INV-14 One compiler core for CLI + MCP; identical request → identical
  packet.** (cross-surface test)
- **INV-15 Packet is a read-only snapshot; compiling never mutates sources.**
- **INV-16 Retrieved content is treated as DATA, not instructions** — rendered
  text is delivered wrapped/quoted so a malicious note cannot inject directives
  into the consuming agent (prompt-injection defense, §3.14).

### 3.13 Failure modes

| Failure | Behavior (fail-safe) |
|---------|----------------------|
| Vector index unavailable / corrupt | degrade to bm25+graph+recency, redistribute `w_emb`, warn; never crash (G12) |
| Embedding model/dim mismatch | mark `s_emb` unavailable for those rows; never compare incompatible vectors |
| Tokenizer drift (counts wrong) | refuse to over-pack; record `tokenizer_id`; conservative under-fill rather than overflow |
| Budget < must-includes | pointer-only must-includes + warning (G07), never silent drop |
| All candidates gated out | empty packet + explanatory warning (G10) |
| Redaction service down | fail-closed: drop affected items, warn; do not emit unverified text |
| Graph cycle / runaway hops | hop cap + visited-set; bounded traversal |
| Score ties | resolved by deterministic tiebreakers (§3.8) |
| Clock skew / bad timestamps | recency clamps to `[0,1]`; future-dated items treated as age 0, flagged |
| Concurrent write during compile | read at a single `db_snapshot`; no torn reads |
| Stale permission via cached packet | packets carry `db_snapshot` + ceiling; cache keyed on (request, snapshot, ceiling), invalidated on sensitivity/policy change |

### 3.14 Security / privacy risks

- **Cross-domain leak** (personal into work packet) → sensitivity gate + ceiling
  derivation + elevation opt-in (§3.6). Primary risk; golden-tested (G03,G11).
- **Embedding inversion** — dense vectors of sensitive text can be partially
  inverted, so **vectors inherit source sensitivity** and the vector search is
  ceiling-filtered *before* nearest-neighbor results are used; sensitive vectors
  are never returned to a sub-ceiling request.
- **Secret in candidate text** — defense in depth: ingestion strip (P0.2) +
  candidate gate + final redaction pass; fail-closed (G09).
- **Audit-trail leakage** — audit stores refs/counts/fingerprints, never raw
  secrets or necessarily full bodies (§3.11).
- **Untrusted MCP caller** — the calling agent's identity sets the ceiling;
  an agent cannot request a ceiling above its grant. MCP cannot bypass the core
  compiler's gates (INV-14).
- **Prompt injection via retrieved content** — INV-16: retrieved text is framed
  as quoted data with provenance, not as instructions to the consumer.
- **Re-identification via graph** — person/relationship edges are themselves
  sensitivity-labeled; graph expansion respects the ceiling (a graph hop cannot
  smuggle in over-ceiling neighbors).

### 3.15 Obsidian implications

- **DB is canonical; vault is the human face** (Constitution Law 3). Vault docs
  and wiki pages are indexed (chunked + embedded), but on conflict the
  source-of-truth policy (§2) decides which side wins; retrieval reads the
  resolved canonical version.
- Packet items from the vault use **path + heading-anchor** as `provenance.source`
  so a human can click straight to the note.
- **Wiki (curated) outranks raw notes** for the same fact (a `wiki —supersedes→
  note` edge or a higher confidence) — curated knowledge beats raw capture.
- **No write-back by default.** The compiler does **not** dump packets into the
  vault (that would re-clutter the brain Altevra is meant to keep clean). An
  optional `altevra packet show <id> --markdown` can render a human-readable
  packet to a scratch location on explicit request only.
- Frontmatter `sensitivity` / `domain` / `status` are honored as object metadata.

### 3.16 Cloud / local sync implications

- **Local-first by axiom** (vision §4.4): indexes and embeddings live locally.
- **Cloud sync is opt-in per domain.** A packet must never pull cloud-only
  sensitive content unless that domain is sync-authorized; otherwise the item is
  treated as out-of-ceiling/unavailable, not silently fetched.
- **Embedding consistency across devices**: synced devices must share
  `embedding_model_id`/`dim`, or re-embed on import — mismatched vectors are
  never compared (§3.13).
- **Conflict resolution** defers to §1/§2 versioning; retrieval always reads the
  resolved canonical version at the snapshot.

### 3.17 CLI / MCP implications

One compiler core; these are adapters (INV-14).

**CLI:**
```bash
altevra context  --project <p> --intent <i> --json            # convenience packet
altevra packet compile --agent <a> --tool <t> --project <p> \
        --intent <i> --budget <n> --sensitivity-ceiling <s> [--elevate] --json
altevra packet show <packet_id> [--markdown]
altevra packet explain <packet_id> --item <item_id>           # prints WhyIncluded
altevra packet excluded <packet_id> [--reason <r>]            # prints ExclusionRecords
altevra search  --query <q> --scope <s> [--intent <i>] --json # ranked candidates, no packing
altevra eval run [--case <Gxx>]                               # golden eval
```

**MCP tools (all → same core):**
`get_context_packet(request)`, `search_memory(query, scope, intent)`,
`get_project_context(project)`, `get_source_of_truth(topic, scope)`.
MCP enforces caller identity → ceiling, returns `ContextPacket` + `warnings`.
`get_source_of_truth` is a structured-index path (no fuzzy ranking) so governing
facts are exact and deterministic.

### 3.18 Required tests / fixtures / golden snapshots

- **Fixture vault + fixture DB**: deterministic seed, no real secrets, includes
  planted **fake** secrets (OpenAI/GitHub/AWS/JWT/DB-URL shapes) for leak tests.
- **Golden packet snapshots** per `(intent, profile)` — committed; diffed on CI.
- **Property tests**: (a) ordering stable under candidate-input permutation;
  (b) `tokens_used ≤ token_budget` always; (c) every emitted item has a
  resolvable `object_ref`; (d) no item with `sensitivity_rank > ceiling`.
- **Leak suites** (must be 0): scope-leak, sensitivity-leak, staleness-leak,
  secret-leak — mapped to G03/G06/G09.
- **Determinism test**: double-compile byte-equality (G08).
- **Degradation tests**: embedding index off (G12), redaction service down,
  budget < must-includes (G07).
- **Cross-surface test**: CLI `packet compile` and MCP `get_context_packet`
  produce identical packets for identical requests (INV-14).
- **Dedup test** (G04), **graph-admission test** (G05), **multilingual test**
  (G13), **provenance-weighting test** (G14), **history-intent test** (G15).

### 3.19 Acceptance criteria

(aligns with and tightens P0.4)

1. **No raw secret** ever appears in any packet item, audit row, or embedding
   text (G09; leak suite = 0).
2. **Token budget respected**: `tokens_used ≤ token_budget`; must-includes never
   silently dropped (pointer-only fallback + warning) (G07).
3. **Every item has a source ref + a `why_included`**; no mystery summaries;
   synthesized items carry `derived_from[]` (INV-3, INV-10, INV-12).
4. **Scope-, sensitivity-, staleness-leak = 0** on the golden eval (G02,G03,G06).
5. **Determinism**: identical request + snapshot → byte-identical packet (G08).
6. **CLI and MCP share the same compiler core** and return identical packets for
   identical requests (INV-14).
7. **Graceful degradation**: vector/redaction/budget failures fail safe, never
   crash, never emit unverified content (G10,G12).
8. **Every exclusion is explained** with a typed reason; security exclusions are
   never elided and never echo the offending content (§3.5).

### 3.20 Unresolved questions

1. **Storage backend split**: V5 §4/§25 specify Postgres + pgvector, but
   AGENTS.md / CLAUDE.md / "local-first by axiom" point to SQLite (FTS5 +
   sqlite-vec) for the local default, with Postgres as the cloud-sync path.
   The scoring/packet contracts are written backend-neutral, but the index
   implementation needs a decision. **Recommendation: SQLite (FTS5 +
   sqlite-vec) as local-first canonical; Postgres as opt-in cloud mirror.**
   → owner: object-model + Hermes.
2. **Embedding model + dimension + provider**: personal-data embeddings must run
   on a **local** model (vision: don't send Pavle's life to US clouds), while
   business content may use a stronger cloud embedder. Does this mean a
   **per-sensitivity embedding model** (and therefore separate, non-comparable
   vector spaces)? If so, cross-sensitivity semantic search is bounded by design.
3. **Tokenizer pinning**: the packet serves many target models (Claude, Codex,
   local). Pin one canonical tokenizer for budgeting (conservative), or pass the
   target model so budgeting matches the consumer exactly?
4. **Reranker role**: V5 model-routing lists a `reranker`. Is a cross-encoder
   rerank stage in P0.4, or a v2 enhancement after fusion? (Current design works
   without it; it would slot between dedup and ordering.)
5. **Packet caching**: cache compiled packets keyed on
   `(request, db_snapshot, ceiling, config_version)`? Needed for hook-driven
   bootstrap latency, but adds a stale-permission risk surface (§3.13).
6. **Domain TTL defaults** (soft/hard) per domain — depends on §6.
7. **Multilingual analyzer**: confirm FTS5/tsvector analyzer + embedding model
   both handle Serbian (latin + možda ćirilica) and English without separate
   pipelines.

### 3.21 Cross-section requests

- **→ object-model (§1):** Please guarantee these fields/contracts that
  retrieval depends on: stable `object_ref` (`type` + `id` + `schema_version`);
  a **totally ordered** `sensitivity` enum (so `≤ ceiling` is well-defined);
  a `domain`/`scope` model with hierarchy (project → parent → global) for the
  scope gate/multiplier; a `status` enum including `current|stale|superseded|
  retracted|deleted`; a `superseded_by` ref; a `confidence` scale with
  `pavle_stated` as the max; and a typed **edge model** (edge `type` + `weight` +
  `directionality`) for the graph index. Please also confirm the durable
  `context_packet` + `context_packet_sources` objects (P0.1) and add an
  `embedding` association object (`object_id`, `chunk_id`, `embedding_model_id`,
  `dim`, `vector_ref`, inherited `sensitivity`).
- **→ safety-source-truth (§2):** Retrieval calls your redaction pipeline at the
  final pass (§3.6 step 4) and your exposure-review hook (step 5). Please expose:
  a `redact(text, ceiling) → {clean_text, status, spans}` contract with a
  fail-closed status; the **ceiling derivation rules** per `(agent, tool,
  domain)`; the guarantee that secret detection happens at ingestion (so
  candidates are pre-clean and retrieval is defense-in-depth); and the
  canonical-vs-mirror decision so retrieval can rank canonical above mirror.
- **→ domains-lifecycle (§6):** Please provide per-domain **soft TTL** and
  **hard-expiry** values; these feed the recency decay half-life defaults and the
  staleness gate (§3.9).
- **→ agents-self-improve (§4):** The observer should consume the **packet audit
  trail** (§3.11) — ignored items, repeated knowledge-gap reports, redaction
  drops, empty packets — to propose retrieval-profile/weight changes. All such
  proposals must be **review-gated**, versioned, and must not regress the golden
  eval (§3.10).
- **→ tools-skills-interfaces (§5):** Please align on the CLI/MCP command surface
  in §3.17 so the `packet`/`context`/`search` verbs and MCP tool names do not
  collide with the tool/skill registry surface.

### 3.22 Summary of this section's changes

This section specifies the **retrieval layer + Context Packet Compiler** as the
exposure boundary that turns broad capture into precise context. It defines:
four index families (BM25 lexical, embedding, graph, structured); a two-layer
scoring model where **scope/sensitivity/lifecycle/staleness/integrity are hard
gates and only relevance is a soft weighted score** (the core anti-RAG-soup
principle); intent-driven retrieval profiles; the full `ContextPacket` /
`ContextPacketItem` / `RetrievalRequest` schema; a machine-checkable
inclusion/exclusion explanation contract (no mystery summaries); a fail-closed,
defense-in-depth redaction + sensitivity-ceiling flow; mandatory resolvable
provenance on every item; deterministic ordering with reserved-sub-budget,
whole-item token packing; staleness/supersession filtering; a 15-case golden
eval set with zero-leak thresholds; a durable packet audit trail; 16 hard
invariants; failure modes; security/privacy risks (incl. embedding inversion and
prompt injection); Obsidian, cloud/local-sync, and CLI/MCP implications; the
required test/fixture/snapshot suite; acceptance criteria; open questions
(backend split, embedding model per sensitivity, tokenizer pinning, reranker,
caching); and cross-section requests to object-model, safety-source-truth,
domains-lifecycle, agents-self-improve, and tools-skills-interfaces.

<!-- END_SECTION: context-retrieval -->

---

<!-- SECTION: agents-self-improve -->
<!-- OWNER: opus-agents-self-improve -->
<!-- STATUS: drafted-by-opus-agents-self-improve -->
## 4. Agent Prompts + Self-Improvement

> Author: `opus-agents-self-improve` (Opus 4.8 MAX). Scope: the **agent prompt
> registry + resident-agent runtime + self-improvement loop** for Altevra/VVLT.
> This section is **behavioral law** for how Altevra observes itself, proposes
> changes, and applies them **without ever rewriting its own guard rails
> autonomously**. It consumes the durable-object envelope (§1), the safety /
> review / source-of-truth substrate (§2), and the context-packet + packet-audit
> contracts (§3). It is grounded in the live `altevra-brain` crate
> (`scheduler.rs` + `jobs.rs`, `JobKind` enum) and migration `011_brain_jobs.sql`.
> Where the live schema and this contract disagree, this section defines the
> **target**; gaps are itemized in §4.12 (failure modes) and §4.19 (unresolved).

### 4.1 Purpose and non-goals

**Purpose.** Turn Altevra from a static context store into the *living, compounding*
system the vision demands (`CLAUDE.md` §3.6, P0 build plan "compounding loop"):

1. **Resident agents** run as bounded, scheduled, dry-run-first reasoning jobs
   that read context packets (§3) and emit **typed proposals**, never raw truth.
2. **A prompt registry** holds every system/mode prompt as a durable, versioned,
   checksum'd, source-of-truth object (§2.6) with layered composition and
   rollback.
3. **A self-improvement loop** observes usage/events/packet-audit/corrections,
   detects repeated workflow / drift / pain, proposes improvements (skills,
   prompts, hooks, schema gaps, retrieval-profile tweaks, categories), routes
   risk through review gates (§2.9), applies the safe ones, monitors usage, and
   patches/deprecates when stale.
4. **A runaway-prevention firewall** guarantees the loop *cannot* silently
   modify its own behavior, prompts-of-record, or safety gates — the
   distinguishing property versus a naive "agent that edits its own prompt."

**Non-goals (explicitly out of scope here):**

- **No model/provider integration in P0** (AGENTS.md + P0.5: model client is
  `altevra-llm`; use role routing only if already safe, else a noop/stub provider
  for tests). This section specifies the *contracts the runtime obeys*, not the
  model plumbing.
- **No autonomous external side effects** (deploy/push/email/customer/payment) —
  always §2.9-gated; the resident never crosses the machine boundary.
- **Not the object envelope** (§1), **not the safety primitives** (`ingest_guard`
  / `exposure_gate`, §2), **not the packet compiler** (§3), **not the skill/tool
  *registry* surface** (§5) — this section *consumes* those and owns only the
  resident/prompt/proposal/loop contracts. The skill *renderer/adapter targets*
  are §5's; the proposal *that produces a skill* is ours.
- **No general "AI agent framework."** Exactly the modes the brain crate already
  schedules, generalized — no fantasy plugin platform.

### 4.2 Object / schema contracts (consume the §1 envelope)

Every object below **carries the full §1.3 common metadata envelope** (`id`,
`type`, `schema_version`, `status`, `created_at`, `updated_at`, `provenance`,
`sensitivity`, `domain`, `scope`, `confidence` where inferred, relations via
§1.6). Only the **type-specific** fields are listed; envelope fields are implied
and MUST NOT be redefined here (single source of truth = §1).

#### 4.2.1 `resident_mode` (registry / config object)

One durable row per resident behavior. Generalizes the hard-coded `JobKind`
enum in `altevra-brain/jobs.rs` into a governed registry.

```jsonc
ResidentMode {                          // envelope: type="resident_mode", domain="business", scope=global
  "slug":            "observer | insight_synthesizer | memory_curator | synthesis"
                   | "wiki_curator | daily_briefing | skill_factory_proposer | event_classifier | task_grooming",
  "trigger":         { "kind": "schedule|event|hook|manual", "period_secs": "int|null",
                       "event_types": ["string"]|null, "cron_local": "string|null" },
  "model_role":      "cheap_worker | strong_reasoner | local_private | embedding | reranker | none",
  "input_profile":   "string",          // §3 retrieval profile id the mode requests
  "input_intent":    "string",          // §3 RetrievalRequest.intent
  "output_types":    ["insight_card","proposal","wiki_page","briefing","resident_run_note"],
  "write_authority": "propose_only | auto_apply_tier0",   // NEVER higher (SI-1)
  "prompt_ref":      { "type":"prompt", "id":"string" },  // active prompt-of-record (§4.8)
  "budget_ref":      { "type":"resident_budget", "id":"string" },
  "enabled":         "bool",             // per-mode kill switch
  "personal_data_allowed": "bool",       // if true ⇒ model_role MUST be local_private (SI-7)
  "min_evidence":    "int",              // ≥ this many signals required to emit a proposal (SI-5)
  "self_authored_excluded": "bool (always true)"          // SI-6 anti-feedback
}
```

#### 4.2.2 `resident_run` (execution record) — generalizes `brain_jobs`

Target schema = live `brain_jobs` (`id, kind, status, started_at, finished_at,
duration_ms, error, result_summary`) **plus** the columns below. Migration gap is
FM-1.

```jsonc
ResidentRun {                           // envelope: type="resident_run"; status family §1.5: running→{done,failed}
  "mode_slug":        "string",          // = brain_jobs.kind
  "trigger_ref":      "string|null",     // event id / hook run id / "manual:<actor>"
  "model_role":       "string",
  "model_id":         "string|null",     // resolved provider/model, null in noop/test
  "input_packet_ref": { "type":"context_packet", "id":"string" }|null,   // §3 packet consumed
  "inputs_digest":    "sha256",          // hash of the signal set + packet (replay/determinism)
  "outputs":          [ { "type":"string", "id":"string" } ],  // proposals/insights produced
  "tokens":           { "in":"int","out":"int" },
  "cost_estimate":    "float|null",
  "decision":         "completed | failed | aborted_budget | aborted_circuit_breaker | skipped_disabled",
  "schema_valid":     "bool",            // P0.5: outputs validated against output schema
  "result_summary":   "string"          // redacted (§2) human line
}
```

#### 4.2.3 `prompt` (source-of-truth class, versioned)

A prompt is **DB-canonical** (§2.6) and **review-gated to change** (§2.9). It is
a node in the §1 supersession chain (append-only correction, §1.4.11): a new
version is a *new* `prompt` row with `supersedes`/`superseded_by` edges; the live
one has `status=active`.

```jsonc
Prompt {                                // envelope: type="prompt", sensitivity≥internal
  "slug":          "string",            // e.g. "resident.observer", "tool.claude-code.system", "mode.daily_briefing"
  "target":        { "kind":"resident_mode|external_tool|external_agent", "ref":"string" },
  "layer":         "safety | altevra_rules | tool_behavior | project | task_goal | updates | skills | output_protocol",
  "body":          "string (template)", // {{var}} placeholders; rendered by §4.8
  "variables":     [ "string" ],        // declared interpolation slots
  "checksum":      "sha256",            // body hash (§1.4 checksum; drift detection)
  "version":       "int ≥ 1",           // monotonic per slug
  "active":        "bool",              // exactly one active per slug (SI-8)
  "constitutional_lock": "bool",        // true ⇒ Tier-2, never auto-applied (SI-2); safety/altevra_rules core
  "eval_baseline_ref": "string|null",   // golden-eval snapshot this version is pinned to (§3.10)
  "applied_via":   { "type":"proposal","id":"string" }|null   // the prompt_proposal that applied it
}
```

#### 4.2.4 `proposal` (super-family; the self-improvement output)

One unified family with a `kind` discriminator. `skill_proposal` and
`prompt_proposal` are the §1.5-reserved named kinds; this generalizes them (see
cross-request §4.20). Lifecycle = §1.5 proposal family:
`proposed → {approved → applied, rejected, withdrawn}; applied → deprecated`.

```jsonc
Proposal {                              // envelope: type="proposal"; provenance.origin=agent_inferred (SI-3)
  "kind":          "skill | prompt | hook | schema_gap | retrieval_profile | category_merge"
                 | "capability_gap | wiki_update | mode_change | policy_change | insight_promotion",
  "risk_tier":     "0 | 1 | 2",          // §4.6 matrix; derived, not agent-chosen (SI-9)
  "title":         "string",
  "rationale":     "string",            // redacted human explanation
  "proposed_diff": "structured-payload", // kind-specific; what would change (never applied yet)
  "evidence":      [ { "type":"improvement_signal","id":"string" } ],  // ≥ mode.min_evidence (SI-5)
  "dedup_key":     "string",            // stable hash of (kind, target, normalized intent) (SI-4)
  "success_metric":"string",            // how we'll know it worked (powers monitor/patch, §4.5 stage 6)
  "proposer_mode": "string",            // resident_mode.slug
  "review_item_ref": { "type":"review_item","id":"string" }|null,      // set when Tier≥1 (§2.9)
  "applied_ref":   { "type":"string","id":"string" }|null,             // the object created/changed on apply
  "shadow_eval_ref": "string|null"      // golden-eval result for prompt/profile/policy kinds (SI-10)
}
```

#### 4.2.5 `improvement_signal` (evidence unit — the anti-thrash backbone)

A clustered, de-duplicated observation. Evidence — never a fact about Pavle.
Lets proposals cite *why* and lets the loop suppress nagging (§4.7).

```jsonc
ImprovementSignal {                     // envelope: type="improvement_signal", confidence required
  "signal_kind":   "repeated_workflow | repeated_correction | ignored_suggestion | knowledge_gap"
                 | "capability_gap | retrieval_miss | redaction_drop | empty_packet | stale_artifact | schema_gap",
  "subject_ref":   { "type":"string","id":"string" }|null,   // what it's about (skill, prompt, wiki, profile)
  "occurrences":   "int",               // count feeding min_evidence
  "evidence_refs": [ "event:<id> | audit:<id> | turn:<id> | packet:<id> | correction:<id>" ],
  "dedup_key":     "string",            // collapses repeats into one growing signal (SI-4)
  "first_seen_at": "RFC3339",
  "last_seen_at":  "RFC3339"
}
```

#### 4.2.6 `resident_budget` + `prompt_eval_result` (control objects)

```jsonc
ResidentBudget {                        // envelope: type="resident_budget"
  "scope":              "global | mode:<slug>",
  "max_runs_per_window":"int", "window_secs":"int", "min_run_interval_secs":"int",
  "max_tokens_per_day": "int", "max_proposals_per_run":"int",
  "max_open_proposals": "int",          // global circuit breaker (SI-11)
  "max_auto_applies_per_day":"int",     // Tier-0 cap (SI-12)
  "rejection_cooldown_days":"int"       // suppress re-proposing a rejected dedup_key (SI-13)
}

PromptEvalResult {                      // envelope: type="prompt_eval_result"; immutable
  "prompt_ref": {"type":"prompt","id":"string"}, "proposal_ref": {"type":"proposal","id":"string"},
  "eval_set_version":"string",          // §3.10 golden set id
  "pass":"bool", "regressions":[ "string" ], "scores":{ "string":"float" }, "ran_at":"RFC3339"
}
```

### 4.3 Enums / statuses / state machines

**`resident_run.status`** (= §1.5 `resident_run` family, = live `brain_jobs`):
`running → done | failed`. Extended `decision` (terminal reason) in §4.2.2.

**`proposal.status`** (= §1.5 proposal family):
```
proposed ──(Tier 0, gates pass)──────────────→ applied ──→ deprecated
   │                                              ↑
   ├─(Tier ≥1)→ review_item.open ─approved→ approved ─(apply, eval gate)→ applied
   │                              ├─rejected→ rejected ─(→ cooldown, SI-13)
   │                              └─deferred→ proposed (re-queued)
   └─ withdrawn   (proposer/budget retracts before review)
```
- A `proposal` may only reach `applied` from `approved` (Tier ≥1) or directly
  (Tier 0, gates pass). `applied → deprecated` when its `success_metric` decays
  or a superseding proposal lands.

**`prompt.status`** (§1.5 generic, specialized): `draft → active → {superseded,
deprecated}`. Exactly one `active` per `slug` (SI-8). Rollback = mint a new
version `derived_from` the older one, set it `active`, old `active → superseded`.

**`review_item`** state machine, approver authenticity, and the `review_item`
row schema are **owned by §2.9** — this section *creates* review items and reads
their terminal state; it never defines its own approval mechanics.

**`improvement_signal`**: `open` (accumulating) → `consumed` (a proposal cited
it) → `expired` (`now > review_after`, no proposal). De-dup growth never changes
`id`.

### 4.4 Resident-agent runtime contract

1. **Dry-run / proposal-first by construction** (P0.5). A resident mode's *only*
   write powers are: emit `insight_card`/`wiki_page` candidates and emit
   `proposal` rows. It **cannot** write to any §1 canonical fact, prompt,
   policy, schema, or protected-domain memory directly (SI-1, enforces §2 I9).
2. **Input is a context packet, not a vault dump** (§3, "no full vault dumps").
   The mode declares `input_profile` + `input_intent`; the runtime compiles a
   packet via the shared §3 compiler with the mode's *own* sensitivity ceiling
   (a `cheap_worker` mode never gets a `restricted` ceiling — SI-7).
3. **Output is schema-validated** (P0.5): every mode output must validate against
   the §4.2 schema for its declared `output_types`, else `resident_run.decision =
   failed`, `schema_valid=false`, and **no** partial writes land.
4. **Model role routing.** Each mode declares `model_role`
   (`cheap_worker|strong_reasoner|local_private|embedding|reranker|none`). In P0
   the role resolves through `altevra-llm`; if no provider is configured the
   role resolves to a **noop provider** so the contract and tests run without
   network (AGENTS.md: "local agent attachment/execution is enough for MVP").
5. **Every run is a `resident_run` row** (Law 8 / §1.7 rule 7): inputs digest,
   model, outputs, tokens, decision, redacted summary. No silent reasoning.
6. **Self-write exclusion (SI-6).** Signal collection and packet input **exclude
   objects whose `provenance.captured_by` is any resident mode** (the §2 I11
   self-write marker). The observer reasons about Pavle/external-agent activity,
   never about its own prior outputs — this is the primary anti-feedback rule.

### 4.5 The self-improvement loop (concrete 7-stage pipeline)

```
(1) CAPTURE      events (V5 §6) + hook turns (V5 §8) + packet audit (§3.11)
                 + corrections (§2.8) + knowledge/capability-gap reports
                      │  exclude self-authored (SI-6); redacted at ingest (§2 I2)
                      ▼
(2) CLUSTER      → improvement_signal rows (dedup_key, growing occurrences)
                      │  no proposal until occurrences ≥ mode.min_evidence (SI-5)
                      ▼
(3) DETECT       observer/skill_factory modes read signals (via packet) →
                 emit Proposal{kind, proposed_diff, evidence[], success_metric}
                      │  risk_tier DERIVED from kind+target (§4.6), not agent-set (SI-9)
                      ▼
(4) GATE         Tier 0 → §4.7 budget/dedup/eval checks → auto-apply (logged)
                 Tier 1 → create review_item (§2.9), human-presence approval
                 Tier 2 → create review_item, constitutional-lock, NEVER auto (SI-2)
                      ▼
(5) APPLY        render via target's owner:
                   skill  → §5 renderer/adapter dir   prompt → §4.8 registry (new version)
                   hook   → §5 hook registry          schema_gap → cross-request only (no auto-DDL)
                   profile→ §3 profile (versioned)    category_merge → §1 categories registry
                      │  prompt/profile/policy MUST pass shadow golden-eval first (SI-10)
                      ▼
(6) MONITOR      track usage of the applied artifact vs success_metric
                 (skill invocations, prompt eval scores, packet-audit deltas)
                      ▼
(7) PATCH/RETIRE stale/regressing artifact → new proposal to patch, or
                 status→deprecated; stale signals expire. Loop closes.
```

The loop is **append-only and evidence-bound**: nothing is applied without (a)
a cited `improvement_signal` set ≥ `min_evidence`, (b) a `success_metric`, and
(c) for behavior-changing kinds, a passing shadow eval.

### 4.6 Risk tiers + write-authority matrix (specialized trust ladder)

`proposal.risk_tier` is **derived deterministically** from `(kind, target,
sensitivity, domain)` by a pure function in core (SI-9 — an agent can never lower
its own tier). Mapping:

| Proposal kind / target | Tier | Gate |
|---|---|---|
| `insight_promotion` (non-sensitive), new `category` proposal, `wiki_update` draft on non-person/non-restricted topic | **0** | auto-apply, logged, reversible, Tier-0 daily cap (SI-12) |
| `skill`, `hook`, `prompt` (non-locked layer), `retrieval_profile`, `capability_gap` grant, `wiki_update` on a **person/relationship** or restricted-domain topic, any proposal touching `restricted` data | **1** | `review_item`, human-presence approval (§2.9) |
| `mode_change` to a mode's `write_authority`/`enabled`/`budget`; `policy_change` (review-gate policy, risk-tier map, exposure policy); `prompt` on a **`constitutional_lock` layer** (`safety`, core `altevra_rules`); any change to the runaway-prevention budgets, the eval gate, or §2 safety primitives; `schema_gap` that implies a migration | **2** | `review_item` + **constitutional lock**: NEVER auto-applicable under any trust setting; requires interactive Pavle + explicit diff; cannot be self-proposed-and-applied in one motion (SI-2) |

**Write-authority invariant (SI-1):** `resident_mode.write_authority ∈
{propose_only, auto_apply_tier0}`. There is **no** mode that can auto-apply
Tier 1 or Tier 2. Tier 0 auto-apply is gated by §4.7 (budget + dedup + cap +
reversibility). This single constraint is what makes the loop *safe* rather than
*self-rewriting*.

### 4.7 Runaway-prevention contract (the firewall)

Hard limits enforced in core, independent of any prompt (so a poisoned prompt
cannot disable them — they live below the LLM, in Rust, tested):

1. **Budgets / rate limits (`resident_budget`).** Per-mode `max_runs_per_window`,
   `min_run_interval_secs`, `max_tokens_per_day`, `max_proposals_per_run`.
   Exceeding → `resident_run.decision = aborted_budget`; no outputs land.
2. **Global circuit breaker (SI-11).** If `count(open proposals) >
   max_open_proposals`, all `propose_only` modes pause proposing (runs still log,
   but emit zero proposals) and a Tier-0 `insight_card` alerts Pavle. Prevents a
   proposal flood from a degraded model.
3. **Tier-0 auto-apply cap (SI-12).** ≤ `max_auto_applies_per_day` auto-applies;
   excess defers to Tier-1 review. No "drift by a thousand auto-applies."
4. **Dedup + rejection cooldown (SI-13).** A proposal's `dedup_key` collides with
   an existing open/applied proposal → merged, not duplicated. A **rejected**
   `dedup_key` is suppressed for `rejection_cooldown_days` — re-proposing the
   same change during cooldown is blocked (anti-nag); only *new evidence beyond a
   threshold* lifts suppression early, and that lift is itself logged.
5. **Self-write exclusion (SI-6).** Per §4.4.6 — the observer never consumes its
   own outputs as signals; reconciliation loops impossible by construction
   (aligns §2 I11).
6. **Shadow eval gate (SI-10).** Any `prompt`/`retrieval_profile`/`policy`
   proposal MUST run the §3.10 golden eval in shadow and produce a passing
   `prompt_eval_result` **before** it can move `approved → applied`. A regression
   = auto-`rejected` with reason `eval_regression`; it can never be applied "by
   approval" over a failing eval.
7. **Constitutional lock (SI-2).** Tier-2 targets (safety prompt layer, the
   risk-tier map, the budgets above, the review-gate policy, §2 safety
   primitives, mode write-authority) are **never** auto-applicable and **never**
   applicable in the same operation that proposed them. They sit in `review_item`
   until an interactive Pavle session applies the diff. The loop **cannot modify
   the rules that constrain the loop.**
8. **Kill switches.** `resident_mode.enabled=false` (per mode);
   `altevra resident disable` / a `RESIDENT_DISABLED` flag (global pause, mirrors
   `SYMBIOSIS_DISABLED`); both checked at the top of every run → `decision =
   skipped_disabled`. A disabled resident still serves reads/packets.

### 4.8 Prompt registry + version lifecycle + layered render

- **Layered composition (V5 §14 priority):** rendered system prompt =
  ordered concat of active `prompt` rows by `layer`:
  `safety ▸ altevra_rules ▸ tool_behavior ▸ project ▸ task_goal ▸ updates ▸
  skills ▸ output_protocol`. Priority for *conflict resolution* is the same
  order (safety wins). `safety` + core `altevra_rules` carry
  `constitutional_lock=true`.
- **Inferred preferences and research suggestions are NOT prompt layers** — they
  enter as *content* in the packet (§3), never silently mutate a prompt-of-record.
- **Deterministic render:** `render(slug_set, variables, db_snapshot)` is pure;
  same inputs → byte-identical output (testable, mirrors §3 determinism). Output
  carries a `prompt_render_manifest` (which versions, which checksums) for replay.
- **Lifecycle:** change a prompt only via a `prompt` proposal (Tier 1, or Tier 2
  if a locked layer). On apply: mint version `n+1` (`status=active`), old → `superseded`,
  write `applied_via` + `supersedes`. **Rollback** = `altevra prompt rollback
  <slug> --to <version>`: re-activate a prior version as a *new* version
  (`derived_from`), itself a Tier-1 review.
- **Drift:** prompt `checksum` mismatch (a generated prompt file edited by hand)
  is quarantined as drift (§2 I6/§2.7), never silently overwritten.

### 4.9 Meta-proposals + observer consumption of the packet audit (§3.11)

The observer's signal sources for *self*-improvement (distinct from improving
*Pavle's* knowledge):

| Signal | Source | Proposal it can raise |
|---|---|---|
| Items repeatedly retrieved but **ignored** | §3.11 packet audit | `retrieval_profile` (down-weight), `wiki_update` (consolidate) |
| Repeated **knowledge-gap** reports (MCP `report_knowledge_gap`) | events | `wiki_update`, `schema_gap` |
| Repeated **capability-gap** reports (`report_capability_gap`) | events | `capability_gap`, `skill` |
| **Empty packets** / `redaction_drop`s | §3.11 audit | `retrieval_profile`, `policy_change` (Tier 2) |
| Repeated **manual workflow** across sessions | hook turns (V5 §8) | `skill` (skill-factory, P0.7) |
| Repeated **corrections** to one fact/area | §2.8 corrections | `wiki_update`, `prompt` (tool behavior) |
| Stale skills/prompts/wiki (unused N days) | usage tracking | `deprecate` (patch/retire, §4.5 stage 7) |

All such proposals are **review-gated, versioned, and must not regress the golden
eval** (this section's binding answer to §3.21's request to agents-self-improve).

### 4.10 Main flows

**Happy path — Tier 0 (insight):** event flood after a work session → CAPTURE
(self-authored excluded) → CLUSTER into a `repeated_workflow` signal (occurrences
4 ≥ min_evidence 3) → observer run emits `insight_card` (Tier 0) → budget+cap OK →
auto-applied to `/10-insights/` + `change-journal` → `resident_run.decision=completed`,
audit row written. No human needed; fully reversible.

**Happy path — Tier 1 (skill factory, P0.7):** repeated manual workflow detected
across 3 sessions → `skill` proposal (Tier 1, dedup_key set) → `review_item`
created (§2.9) → Pavle approves via interactive CLI (human-presence signal) →
§5 renderer writes the skill to the adapter dir with managed header → usage
tracker arms `success_metric` (skill invoked ≥1×/week) → applied.

**Review / rejection path:** Tier-1 `prompt` proposal → shadow golden eval runs →
**regression** → auto-`rejected` (`reason=eval_regression`), `dedup_key` enters
cooldown (SI-13); OR Pavle rejects → `rejected`, cooldown armed, re-proposal
suppressed for `rejection_cooldown_days`. Either way: no change to the active
prompt, full audit row, Obsidian review card flips to `rejected`.

**Constitutional-lock path (Tier 2):** observer notices the observer's own
budget is too tight and proposes raising `max_proposals_per_run` → `policy_change`
(Tier 2) → `review_item` with constitutional lock → **cannot auto-apply**; sits
until interactive Pavle applies the diff. The loop proposed a change to its own
guard rail and was *structurally prevented* from enacting it.

**One vertical loop test (the acceptance spine, §4.17):** fixture events →
signal cluster → skill_proposal → review_item → simulated human-presence approve
→ rendered skill artifact → usage event → success_metric satisfied. Single
end-to-end test proves the whole compounding loop without any network model.

### 4.11 Invariants (`SI#` — other sections/tests enforce)

- **SI-1 (no auto-apply above Tier 0):** `resident_mode.write_authority ∈
  {propose_only, auto_apply_tier0}`; no path applies Tier ≥1 without an approved
  review item.
- **SI-2 (constitutional lock):** Tier-2 targets are never auto-applicable and
  never applied in the same op that proposed them.
- **SI-3 (agent-proposed only):** every resident output has
  `provenance.origin = agent_inferred` and `confidence ≤ medium` until verified
  (consumes §1 I4, §2 I9).
- **SI-4 (dedup):** proposals/signals are keyed by `dedup_key`; collisions merge,
  never duplicate.
- **SI-5 (evidence floor):** no proposal emits with `len(evidence) <
  mode.min_evidence`.
- **SI-6 (self-write exclusion):** signal collection excludes resident-authored
  objects; no reconciliation loop (consumes §2 I11).
- **SI-7 (role↔ceiling):** a mode's packet ceiling ≤ its `model_role`'s allowed
  ceiling; `personal_data_allowed ⇒ model_role=local_private` (Constitution).
- **SI-8 (one active prompt per slug):** exactly one `prompt` row per `slug` is
  `active`.
- **SI-9 (tier not agent-chosen):** `risk_tier` is computed by core from
  `(kind,target,sensitivity,domain)`; an agent-supplied tier is ignored.
- **SI-10 (eval gate):** prompt/profile/policy proposals require a passing
  `prompt_eval_result` against the pinned §3.10 set before apply.
- **SI-11 (circuit breaker):** open-proposal count > cap ⇒ proposing pauses.
- **SI-12 (auto-apply cap):** ≤ `max_auto_applies_per_day` Tier-0 applies.
- **SI-13 (rejection cooldown):** a rejected `dedup_key` is suppressed for the
  cooldown window.
- **SI-14 (every run logged):** every resident invocation writes a `resident_run`
  row (consumes §1.7/Law 8); failed schema-validation lands zero outputs.
- **SI-15 (instructions are data):** content read by a resident mode is **never**
  treated as instruction; tier/gate decisions live in code (consumes §2 I14 —
  a malicious note cannot talk the observer into a Tier-2 auto-apply).

### 4.12 Failure modes

| # | Failure | Trigger | Mitigation |
|---|---|---|---|
| FM-1 | `brain_jobs` lacks resident columns | live schema is v0.3.4 minimal | migration to add §4.2.2 columns; until then `resident_run` is a superset view + new table (P0.1) |
| FM-2 | Proposal flood from degraded model | model returns garbage en masse | SI-11 circuit breaker + SI-5 evidence floor + per-run proposal cap |
| FM-3 | Observer reacts to its own output (loop) | self-authored signal ingested | SI-6 self-write exclusion (self-write marker, §2 I11) |
| FM-4 | Nagging (re-proposing rejected change) | rejected proposal regenerated | SI-13 cooldown + dedup_key suppression |
| FM-5 | Prompt regression shipped | approved prompt worsens behavior | SI-10 shadow golden-eval gate; auto-reject on regression |
| FM-6 | Self-rewrite of guard rails | proposal targets budgets/policy/safety prompt | SI-2 constitutional lock; SI-9 tier-by-core |
| FM-7 | Personal data sent to cloud model | mode mis-routed | SI-7 role↔ceiling; `personal_data_allowed⇒local_private` |
| FM-8 | Schema-invalid output partially written | LLM emits malformed proposal | SI-14 validate-then-write; failed run lands nothing |
| FM-9 | Prompt injection in ingested note | note says "auto-apply everything" | SI-15 instructions-are-data; gates in code (§2 I14) |
| FM-10 | Stale skills/prompts accumulate (clutter) | applied artifacts never retired | §4.5 stage 7 patch/retire; usage tracking + deprecate; importance gate before persist |
| FM-11 | Forged approval | agent sets `approved=true` | approval authenticity owned by §2.9 (human-presence); resident never approves |
| FM-12 | Wiki bloat from over-proposing pages | every signal → new page | dedup + `insight_promotion` consolidation + Tier-0 daily cap (SI-12) |

### 4.13 Security / privacy risks and how §2 gates apply

- **The resident is the highest-leverage write actor** → it is also the most
  dangerous. Mitigation: it has the *narrowest* write surface (propose-only +
  Tier-0), and **all** its reads go through `exposure_gate` (§2) and all its
  writes through `ingest_guard` (§2) — it has **no privileged path** (§2 I-no-bypass).
- **Prompt-of-record is an attack surface** (a poisoned prompt could weaken
  behavior): prompts are SoT, review-gated, checksum'd, drift-detected, and the
  safety layer is constitutional-locked (Tier 2) — an agent cannot edit the
  layer that forbids leaks.
- **Self-improvement could launder sensitivity** (synthesize restricted → emit
  public insight): blocked by §1 I5 monotonic derivation + §2 I8; a resident
  output inherits `max(source sensitivity)`.
- **Personal-domain reasoning** must use `local_private` (SI-7) — Pavle's life
  never goes to a US/Chinese cloud for classification (Constitution / vision).
- **Audit:** every run, proposal, apply, reject, and Tier-0 auto-apply emits an
  append-only audit row (§2.10) — "what did the resident change and why" is
  always answerable.

### 4.14 Obsidian implications

- **Review inbox zone `/20-review/`** (generated zone, managed header per §2.14):
  one human-readable markdown card per open `review_item`/`proposal` —
  frontmatter `{id, kind, risk_tier, proposer_mode, success_metric, status,
  dedup_key}` + body (rationale, rendered diff, evidence links, "approve via
  `altevra review approve <id>`"). Card flips status on CLI decision.
- **Insights/briefings** in `/10-insights/` (V5 §7 change-journal + daily
  briefing), authored by Tier-0 modes.
- **Prompts:** human-seeded prompt bodies live in the **authored zone**
  (`/00-authored/prompts/**`, no managed header); rendered/generated prompt files
  live in `/15-generated/prompts/**` with managed header + checksum.
- **Wiki hygiene (anti-clutter, vision: "remember important work … without
  becoming cluttered"):** `wiki_curator` MUST dedup (`duplicate_of`, §1.6),
  supersede stale pages (§1.4.11), add links rather than fork pages, and apply an
  **importance gate before persisting** a new page; low-value insights stay as
  DB-only `insight_card`s and are *promoted* to a wiki page only via
  `insight_promotion` (Tier 0) when they clear the gate.
- **`confidential+` resident outputs are DB-only** (`mirror_to_markdown=false`,
  §2.14) — a sensitive insight is never written to plaintext vault by default.

### 4.15 Cloud / local sync implications

- **Prompts and skills (business/tooling domain)** are sync-eligible (they're not
  personal data); they carry `revision`/`origin_device` (§1.3) and resolve
  conflicts by the append-only version chain (local canonical unless Pavle sets
  otherwise; §2.7).
- **Proposals touching `restricted`/personal domains never sync** (§2.15 per-domain
  ceiling); **`improvement_signal`s and `resident_run` rows over personal data
  stay local** (they reference sensitive evidence).
- **`prompt_eval_result`** and golden-eval snapshots are sync-eligible (no
  personal data) and double as cross-device regression baselines.
- **Conflict:** two devices both bump a prompt → two versions in the chain; the
  reconciler keeps both as history and routes the divergence to a Tier-1 review
  (no silent last-writer-wins on a SoT prompt).

### 4.16 CLI / MCP implications + caller boundaries

**CLI (one core, MCP is adapter — V5 §5):**
```bash
altevra resident modes [--json]                       # registry
altevra resident run <mode> --dry-run|--once [--explain] [--json]
altevra resident status [--failed] [--json]           # generalizes `brain status/jobs`
altevra resident disable|enable [--mode <slug>]       # kill switch (SI-8 kill / global)
altevra propose list|show <id> [--kind <k>] [--json]  # meta-proposals
altevra prompt list|show <slug>|render <slug>|diff <slug>|history <slug>|rollback <slug> --to <v>
altevra review list|show|approve|reject <id>          # SHARED with §2.9 (approve = human-presence)
```

**MCP tools (all → same core, all enforce caller→ceiling via §2):**
`get_resident_status`, `report_knowledge_gap`, `report_capability_gap` (V5),
`propose_improvement(kind, target, rationale, evidence)` → creates an
`agent_proposed` proposal (Tier auto-derived), `create_review_item` (V5),
`request_skill_refresh` (V5).

**Caller boundaries (hard):**
- **External agents** (Claude Code/Codex/Cursor/Antigravity/Hermes via MCP) MAY:
  report gaps, `propose_improvement` (creates a proposal at the core-derived
  tier). They MAY NOT: run resident modes, approve/reject review items, apply
  any proposal, read raw secrets, or set their own `risk_tier`.
- **Resident runtime** (local, in-process) MAY: run modes, emit proposals,
  auto-apply Tier 0 (within budget). It MAY NOT: approve Tier ≥1, apply Tier 2,
  cross the machine boundary.
- **Human-CLI (TTY / explicit unlock, §2.9)** is the **only** caller that can
  approve protected changes and apply Tier-2 diffs.

### 4.17 Required tests / fixtures / golden snapshots

- **Vertical loop test (spine):** fixture events → `improvement_signal` →
  `skill` proposal → `review_item` → simulated human-presence approve → §5
  render stub → usage event satisfies `success_metric`. One end-to-end pass,
  no network (noop model). *(maps P0.5 + P0.7 acceptance)*
- **Runaway-prevention suite:** (a) budget cap → `aborted_budget`, no outputs;
  (b) proposal-flood → SI-11 circuit breaker pauses proposing; (c) Tier-0
  daily-cap → overflow defers to Tier-1; (d) rejected `dedup_key` re-proposal
  blocked during cooldown (SI-13); (e) Tier-2 proposal **cannot** auto-apply
  (SI-2); (f) self-authored event excluded from signals (SI-6); (g)
  agent-supplied `risk_tier` ignored, core re-derives (SI-9); (h) prompt-injected
  note ("auto-apply everything") does not change any gate (SI-15).
- **Prompt lifecycle:** propose → version `n+1` active, old superseded →
  `rollback --to` mints derived active (Tier-1 gated); layered `render`
  byte-deterministic; checksum/drift quarantine on hand-edit.
- **Eval gate:** prompt proposal with a planted regression → shadow eval fails →
  auto-`rejected (eval_regression)`, active prompt unchanged (SI-10).
- **Schema validation:** malformed mode output → `resident_run.failed`,
  `schema_valid=false`, zero rows written (SI-14).
- **Role/ceiling:** `personal_data_allowed` mode forced to `local_private`;
  packet ceiling ≤ role ceiling (SI-7).
- **Repository roundtrip** (P0.1) for `proposal`, `review_item`,
  `resident_run`, `improvement_signal`, `prompt`, `context_packet_sources` —
  envelope conformance (§1.14 meta-test) + no raw secrets in any fixture text.
- **Fixtures:** deterministic seed DB + vault, planted **fake** secrets (reuse
  §2.17 / §3.18 corpus) to prove no resident path leaks them.

### 4.18 Acceptance criteria for P0.0 / P0.1

**P0.0 (contracts — this document):**
- §4.2 object schemas (`resident_mode`, `resident_run`, `prompt`, `proposal`,
  `improvement_signal`, `resident_budget`, `prompt_eval_result`) defined,
  consuming the §1 envelope; no field redefines §1.
- §4.3 state machines + §4.6 tier matrix + §4.11 invariants `SI-1…SI-15`
  enumerated and each mapped to a §4.17 test.
- §4.7 runaway-prevention firewall fully specified in code-enforceable terms
  (budgets/breaker/cap/cooldown/eval-gate/lock/kill) independent of any prompt.
- Cross-section requests (§4.20) filed; no edits outside this section.

**P0.1 (data-model landing zone — matches build plan "Immediate next task"):**
- Migrations from empty DB pass for `proposal` (with `kind`), `review_item`,
  `resident_runs` (extend `brain_jobs`), `improvement_signal`, `prompt` +
  `prompt_eval_result`; `context_packet_sources` already in scope.
- Repository roundtrip tests pass (à la `repository_roundtrip.rs`); envelope
  fields present with correct affinity (§1.14).
- **No resident execution and no external model calls yet** (P0.1 explicitly):
  the schemas + repositories are the *landing zone* for proposals so that the
  later runtime has structured places to write — autonomy comes after.
- No raw secrets in any durable fixture text; baseline green
  (`cargo fmt --check && cargo test && cargo build && cargo clippy --workspace -- -D warnings`).

**P0.5/P0.7 readiness (gated on §4.19 #1 model decision):** dry-run resident with
`insight_synthesizer` + `skill_factory_proposer` modes emitting schema-valid,
review-routed proposals; the §4.17 vertical loop test green.

### 4.19 Unresolved questions (owner / recommended default)

1. **Model role plumbing for resident modes.** AGENTS.md/P0.5 say no external
   API in P0; the modes need *some* executor for dry-run tests.
   → **Default: noop/stub provider** behind `altevra-llm` role routing; real
   providers post-P0. Owner: Pavle/Hermes (P0.5 gate).
2. **`resident_run` vs `brain_jobs`.** Extend the existing table or add a new
   `resident_runs` table and view `brain_jobs` as a subset? → **Default: extend
   `brain_jobs` with §4.2.2 columns** (one history table). Owner: object-model + Hermes.
3. **Proposal super-family vs separate tables.** §1.5 named `skill_proposal` /
   `prompt_proposal` separately. → **Default: one `proposal` table with `kind`**
   (less schema churn, uniform review/dedup). Owner: object-model (see §4.20).
4. **Golden-eval coupling.** SI-10 requires the §3.10 set to exist before
   prompt/profile proposals can apply. Is the eval set a P0.4 deliverable or a
   P0.5 prerequisite? → **Default: a minimal eval set ships with P0.4**; prompt
   self-improvement is gated until it exists. Owner: context-retrieval + Hermes.
5. **Human-presence authentication** (shared with §2.19 #7) — TTY vs signed
   unlock token vs passphrase for approvals. → defer to §2's resolution; this
   section only *requires* that "approved" cannot be an agent-set payload flag.
6. **Hermes' `background_review.py` reconciliation.** Hermes already runs a
   per-turn self-review fork; do Altevra resident modes and Hermes' fork share a
   proposal queue or stay separate? → **Default: separate stores, both write
   proposals into Altevra's review queue via MCP `propose_improvement`** (Altevra
   = proposal system-of-record). Owner: Hermes.
7. **Skill-factory render target** (where an approved `skill` proposal is
   written) — owned by §5; this section needs only the renderer interface.
   → cross-request §4.20.

### 4.20 Cross-section requests

- **→ object-model (§1):** (a) confirm a unified **`proposal`** durable type with
  a `kind` discriminator covering `{skill, prompt, hook, schema_gap,
  retrieval_profile, category_merge, capability_gap, wiki_update, mode_change,
  policy_change, insight_promotion}` — generalizing the §1.5-reserved
  `skill_proposal`/`prompt_proposal`; (b) add types `resident_mode`,
  `resident_budget`, `improvement_signal`, `prompt`, `prompt_eval_result` to the
  §1.2 taxonomy with the §1.5 status families used here; (c) add `prompt`'s
  `slug`/`version`/`active`/`constitutional_lock` as canonical keys (one active
  per slug); (d) confirm `resident_run` extends `brain_jobs` with the §4.2.2
  columns.
- **→ safety-source-truth (§2):** (a) confirm `prompt` (esp. `safety`/core
  `altevra_rules` layers) is a **review-gated SoT class** and that exposure-widening
  / sensitivity-downgrade proposals are forced to Tier-2; (b) provide the
  human-presence approval signal `review approve` consumes; (c) confirm resident
  reads/writes have **no bypass** of `exposure_gate`/`ingest_guard`; (d) ratify
  SI-15 (ingested content is data, not instruction) as the shared §2 I14.
- **→ context-retrieval (§3):** (a) expose the **packet audit trail** (§3.11)
  fields the observer needs: per-item `ignored`/`used` signal, `redaction_drop`
  count, `empty_packet` marker, repeated knowledge-gap linkage; (b) confirm the
  **golden-eval set + runner** (§3.10) is callable in *shadow* mode so SI-10 can
  gate prompt/profile proposals; (c) accept `retrieval_profile` as a versioned,
  proposal-changeable object that must not regress the eval.
- **→ tools-skills-interfaces (§5):** (a) provide the **skill/hook renderer
  interface** an approved `skill`/`hook` proposal calls (target adapter dir +
  managed header + checksum); (b) provide a **usage-tracking** read so §4.5 stage
  6 can evaluate a skill's `success_metric` and stage 7 can deprecate; (c) align
  the `altevra resident|propose|prompt|review` CLI verbs and the `propose_improvement`
  / `get_resident_status` MCP names so they do not collide with the tool/skill
  registry surface (§3.21 also flagged this).
- **→ domains-lifecycle (§6):** provide per-domain defaults for which domains a
  resident mode may read (`personal_data_allowed` gating) and per-domain
  `cloud_sync` ceiling for proposals/signals (so SI-7 and §4.15 have concrete maps).
- **→ Hermes (synthesis):** resolve §4.19 #1 (resident executor for P0.5),
  #2 (`brain_jobs` extension), #6 (Hermes fork ↔ Altevra proposal queue) **before
  P0.5 sign-off** — they change the resident-runtime contract.

### 4.21 Summary of this section's changes

This section specifies the **Agent-Prompt + Self-Improvement layer** as a
bounded, evidence-driven, review-gated loop that *cannot rewrite its own guard
rails*. It defines: a governed **`resident_mode` registry** (generalizing the
live `JobKind`/`brain_jobs`), **`resident_run`** execution records, a versioned
**prompt registry** with layered render + rollback + a constitutional-locked
safety layer, a unified **`proposal`** super-family, **`improvement_signal`**
evidence units, and **`resident_budget`/`prompt_eval_result`** control objects —
all consuming the §1 envelope. It pins state machines, a deterministic
**risk-tier matrix** (Tier 0 auto / Tier 1 human-review / Tier 2
constitutional-lock), and **fifteen invariants `SI-1…SI-15`**, each mapped to a
test. The core contribution is the **runaway-prevention firewall** (budgets,
global circuit breaker, Tier-0 cap, dedup + rejection cooldown, self-write
exclusion, shadow golden-eval gate, constitutional lock, kill switches) enforced
in code *below* the LLM so a poisoned prompt cannot disable it. It specifies the
7-stage compounding loop, happy/review/rejection/constitutional-lock flows, a
single **vertical loop test** as the acceptance spine, Obsidian zones (review
inbox `/20-review/`, wiki hygiene/anti-clutter), local-first sync rules
(personal signals stay local, prompts/skills sync via version chain), the
CLI/MCP surface with hard caller boundaries (external agents may *propose*, only
the local resident may *run*, only human-CLI may *approve*), P0.0/P0.1 acceptance
criteria, seven unresolved questions with defaults, and cross-section requests to
object-model, safety-source-truth, context-retrieval, tools-skills-interfaces,
domains-lifecycle, and Hermes.

<!-- END_SECTION: agents-self-improve -->

---

<!-- SECTION: tools-skills-interfaces -->
<!-- OWNER: opus-tools-skills-interfaces -->
<!-- STATUS: drafted-by-opus-tools-skills-interfaces -->
## 5. Tools + Skills + Interfaces

> Author: `opus-tools-skills-interfaces` (Opus 4.8 MAX). Scope: the **capability /
> tool / skill / interface** contract — how Altevra knows *what it and every
> external agent can do*, *what is installed where*, how it **manufactures and
> renders** skills/hooks into native tool files, and how capability is **shared
> across agents** without leaking or silently over-granting. This is the layer
> that makes Altevra "the skill manufacturing layer for Pavle's entire AI tool
> ecosystem" (`CLAUDE.md` §12) instead of a Claude-only integration.
>
> Grounding: live crates `altevra-skills` (parser/registry/version/checksum/
> renderer), `altevra-hooks` (universal/registry/runner/actions), `altevra-adapters`
> (`ToolAdapter` trait, `claude_code`/`codex`/`cursor`/`antigravity`), migrations
> `003_skills`, `004_hooks`, `005_tool_installations`; V5 §§8–22; build plan P0.7.
> This section is **contract law**, not implementation. Where the live schema and
> this contract disagree, this section defines the **target**; gaps are itemized
> in §5.6 and §5.13.
>
> Consumes §1 (envelope, edges, status superset, gap-object program), invokes §2
> (`ingest_guard`/`exposure_gate`, review gates, authorship classes, secret
> handling), and composes with §3 (bootstrap packet ⊕ context packet; no verb
> collision). It does **not** redefine the envelope, the sensitivity ladder, the
> redaction mechanism, or the retrieval compiler — it references them.

### 5.1 Purpose and non-goals

**Purpose.** Give every agent (Claude Code, Codex, Cursor, Antigravity, Hermes,
future) a single, honest, testable answer to four questions:

1. **"What can Altevra do, and what can *I* (this tool) do natively vs. via
   fallback?"** — the **capability registry** + **adapter dossier** (tool
   registry). Honesty is law: `supported` requires *evidence*, never a guess
   (Constitution Law 6).
2. **"What is installed in this repo/tool right now, is it current, and has a
   human edited a managed file?"** — the **installed-component state** layer
   (drift = supersession, one checksum mechanism per §1's request).
3. **"Here is a workflow I keep repeating — turn it into a reusable skill."** —
   the **skill factory** (propose → review → render → install → monitor →
   deprecate). The compounding loop (P0.7, `CLAUDE.md` §12).
4. **"Give this capability/skill to that other agent, safely."** — **cross-agent
   skill/tool sharing**, gated by a trust ladder + review (Constitution §10
   "shared context ≠ shared permission").

**Non-goals (owned elsewhere):**

- Resident-agent runtime, modes, and the *generation* of proposals — §4
  agents-self-improve. §5 owns the **proposal contract**, the **render target**,
  and **post-approval materialization**, not the model that authors a proposal.
- Object envelope, edge model, gap-object migration program — §1.
- Secret detection mechanism, redaction, review-gate predicate, human-presence
  auth — §2 (referenced, not reimplemented).
- Context packet compilation / ranking — §3 (the bootstrap packet *embeds* a §3
  packet; §5 contributes only the freshness/setup/capability slice).
- Per-domain policy (which domains a skill may be shared into, retention of dead
  proposals) — §6.
- The Last-Updates feed and event classifier (V5 §6/§19) — emitted *to*, not
  *owned by*, this section.

### 5.2 Object / schema contracts (consume the §1 envelope)

Every object below carries the **full §1.3 mandatory envelope** (`id`, `type`,
`schema_version`, `status`, `created_at`, `updated_at`, `provenance`,
`sensitivity`, `domain`, `scope`, `tags`, `categories`, `confidence` where
inferred, `supersedes`/`superseded_by`, `revision`, `origin_device`, `checksum`
where it has a face, `metadata`). Only **section-specific** fields are listed.
Storage is backend-neutral (SQLite local-first canonical; Postgres = opt-in
mirror — same as §1/§2/§3, see §5.13 Q6).

**5.2.1 `skill`** — universal skill definition (canonical key `slug`; live
`skills` table). Face: `both` (authored in `/06-skills/*.md`, or generated).

```jsonc
skill {
  // envelope …
  "slug":        "string UNIQUE",          // canonical key (live)
  "version":     "semver string",          // SkillVersion (live)
  "title":       "string",
  "body":        SkillBody,                 // structured (5.2.2); markdown is the face
  "source_path": "string",                 // vault path of the authored/generated md
  "authorship_class": "obsidian_authored | generated_mirror | agent_proposed",  // §2.6
  "target_agents":    [ "claude-code|codex|cursor|antigravity|hermes|*" ],      // who may receive it
  "capability_grade": "read | propose | render | install | execute",            // breadth (drives review, 5.3)
  "sharing": { "trust_required": TrustLevel, "requires_approval": "bool" }       // §5.4.5
  // envelope.checksum == hash(SkillBody render) — the ONE mechanism (T2)
}
```

**5.2.2 `SkillBody`** — the structured payload every generated skill MUST fill
(P0.7 acceptance: "trigger, steps, commands, pitfalls, verification"):

```jsonc
SkillBody {
  "trigger":      "string",                 // WHEN to use this skill
  "steps":        [ "string" ],             // ordered actions
  "commands":     [ "string" ],             // exact shell/CLI invocations (secret-free, {{secret:h}} only)
  "pitfalls":     [ "string" ],             // known failure modes / gotchas
  "verification": [ "string" ]              // how to confirm it worked
}
```

**5.2.3 `hook`** — universal hook definition (canonical key `slug`; live `hooks`
table). Face: `generated`. Section fields: `hook_type` (one of
`UniversalHookType`: `session_start | session_end | before_tool_call |
after_tool_call | before_file_edit | after_file_edit | before_command |
after_command | on_error | on_skill_check | on_context_request | on_task_complete
| on_project_switch`), `actions` (`HookAction[]` — e.g. `check_skill_version`,
`get_last_updates`, `get_project_context`, `start/end_session_log`, `emit_event`,
`detect_secret_leak`, `create_review_item`), `source_file`.

**5.2.4 `adapter_dossier`** — tool-registry entry / V5 "Adapter Capability
Matrix" formalized as a durable object (canonical key `tool_name`; **gap → P0.1**;
vault face `/07-capabilities/agent-tools.yaml`, `authorship_class =
generated_mirror`).

```jsonc
adapter_dossier {
  // envelope …
  "tool_name":      "string UNIQUE",       // claude-code | codex | cursor | antigravity | hermes | aider
  "adapter_version":"string",              // ToolAdapter::adapter_version()
  "support_tier":   "native | partial | fallback_only | unsupported",
  "surfaces": {                            // per native surface: a capability verdict
    "hooks":        SurfaceSupport,
    "skills":       SurfaceSupport,
    "mcp":          SurfaceSupport,
    "instructions": SurfaceSupport,
    "slash_commands": SurfaceSupport,
    "prompt":       SurfaceSupport
  },
  "hook_events_supported": [ UniversalHookType ],   // subset honored natively
  "skill_format":   "md | mdc | yaml | none",
  "install_targets":[ "relative/path" ],   // files this adapter writes (e.g. .claude/, AGENTS.md)
  "fallback_strategy": "cli_wrapper | instructions_only | none",
  "detection":      "string"               // how the adapter detects the tool in a repo
}
SurfaceSupport { "support": Support, "evidence_ref": "object_ref|null", "degraded_to": "string|null" }
```

**5.2.5 `capability_record`** — the **honest** can/cannot/unverified ledger
(Constitution Law 6; canonical key `(actor, capability_key)`; **gap → P0.1**;
generalizes the live `.altevra/state/capabilities.json`).

```jsonc
capability_record {
  // envelope …
  "actor":          "altevra | claude-code | codex | cursor | antigravity | hermes",
  "capability_key": "string",              // e.g. hook.session_start, mcp.tools, skill.render, secrets.resolve
  "support":        "supported | unsupported | unverified | fallback",
  "evidence_ref":   "object_ref|null",     // verify-run / test / install that PROVED it (required when supported)
  "verification_method": "tested | declared | observed",
  "verified_at":    "iso8601Z|null",
  "degraded_to":    "capability_key|null"  // fallback target when unsupported
}
```

**5.2.6 `tool_installation`** (live `005`) — `(tool_name, project_id)` install
record. Section fields already present: `adapter_version`, `installed_at`,
`last_verified_at`, `status`, `metadata`. Add envelope fields via P0.1 backfill
(FM-12). **Local-per-device** (not sync-eligible — describes *this* machine).

**5.2.7 `installed_component`** (live `005`) — per-component install row;
canonical key `(installation_id, component_slug)`. Section fields:
`component_type` (enum, 5.3), `component_slug`, `installed_version`,
`installed_path`, `checksum` (= expected on-disk body hash, T-CHK), `status`
(component state machine, 5.3), `last_checked_at`. **Local-per-device.**
`skill_installation` (live `003`) is the skill-specific specialization with the
same shape; it MAY be folded into `installed_component` with
`component_type=skill` (see §5.13 Q3).

**5.2.8 `skill_proposal`** — skill-factory output (canonical key = `dedup_hash`
of the workflow; **gap → P0.1**; `authorship_class = agent_proposed`).
**Co-owned with §4** (§4 owns generation + status lifecycle; §5 owns the render
contract below — see §5.14).

```jsonc
skill_proposal {
  // envelope (status machine: proposed → {approved → applied, rejected, withdrawn}; applied → deprecated) …
  "dedup_hash":      "string",             // canonical key — same workflow proposes once
  "proposed_slug":   "string",
  "proposed_body":   SkillBody,            // must be fillable to render (5.2.2)
  "workflow_evidence": [ "object_ref" ],   // sessions/turns/tool-calls that show the repeated pattern
  "occurrences":     "int",                // how many times the pattern was seen
  "target_agents":   [ "string" ],
  "capability_grade":"read|propose|render|install|execute",
  "render_target":   "string|null"         // set on approval: dir the renderer writes to
}
```

**5.2.9 `capability_grant`** — records that a capability/skill is granted to an
agent (canonical key `(grantee, capability_key|skill_slug)`; **gap → P0.1**;
implements Constitution §10 "skill execution between agents requires a contract
with `trust_level` and `requires_approval`" + Law 4 "tool grants require
review").

```jsonc
capability_grant {
  // envelope …
  "grantee":        "string",              // agent/tool receiving the grant
  "subject":        { "kind": "skill|capability", "ref": "slug|capability_key" },
  "trust_level":    TrustLevel,            // 5.3
  "requires_approval": "bool",
  "approval_ref":   "object_ref|null",     // review_item that approved it (required when requires_approval)
  "scope":          "project_id | global",
  "status":         "pending | granted | revoked",
  "granted_at":     "iso8601Z|null",
  "expires_at":     "iso8601Z|null"
}
```

> **Answer to §1's request (skill/hook/installation/component are durable
> objects; map managed-file checksum to object checksum/revision):** the
> envelope `checksum` of a faced object (`skill`, `hook`, generated
> `adapter_dossier`) is **the body hash that the managed-file header records**
> (T-CHK, 5.5). `installed_component.checksum` is the *expected* on-disk body
> hash captured at install. Drift detection (a *face* problem) and supersession
> (a *version* problem) therefore read the **same** `(checksum, revision)` pair —
> one mechanism, satisfying I9. Component states (5.3) are aligned to the §1.5
> superset.

### 5.3 Enums / statuses / state machines

**`TrustLevel`** (ordered ladder for cross-agent grants, 5.2.9):
`none < read < propose < render < install < execute`.
- `read` — may see the skill/capability metadata. `propose` — may suggest it.
- `render` — may produce native files (dry-run plan). `install` — may write
  managed files. `execute` — the skill grants an action capability (shell, file
  write, external side effect) ⇒ **always protected** (§2.9, T9).

**`Support`** (capability honesty, 5.2.5):
`supported | unsupported | unverified | fallback` — initial `unverified`;
`→ supported` ONLY with an `evidence_ref`; `→ fallback` when native unsupported
but a wrapper exists; `→ unsupported` when nothing works.

**`component_type`** (5.2.7): `instruction | skill | hook | mcp_config |
fallback_script | slash_command | prompt`.

**Installed-component status** — aligned to §1.5 row "`skill`/`hook` (installed)"
`{current, outdated, drifted, missing, conflicted, unsupported}`, initial
`current`. **Status is computed by `verify`, never asserted by a payload (T8).**
Derivation (let `disk = hash(body_after_stripping_managed_header)`,
`expect = component.checksum`, `vc = VersionCheckResult` from
`altevra-skills::registry::check_version`):

```text
status(component):
  if file_absent(installed_path):                 missing
  elif not adapter.supports(component_type):      unsupported
  elif disk != expect:
        if vc == Outdated:                         conflicted   # drifted AND behind
        else:                                      drifted      # human edited managed file
  elif vc == Outdated:                             outdated
  elif vc == Ahead:                                conflicted   # installed newer than registry — Q1
  else:                                            current
```

Transitions: `current ↔ outdated` (registry/version change), `current → drifted`
(human edit), `* → missing` (file removed out-of-band), `{drifted,outdated} →
conflicted` (both), `* → unsupported` (adapter loses native surface). `drifted`
and `conflicted` are **review states** — never auto-resolved (T4).
`VersionCheckResult::{NotFound, ParseError}` are *errors*, surfaced as warnings
(not component statuses).

**`skill_proposal` status** (5.2.8, co-owned §4): `proposed → {approved, rejected,
withdrawn}`; `approved → applied`; `applied → deprecated` (usage dropped /
superseded). Mirrors §1.5 proposal row.

**`adapter_dossier.support_tier`**: `native > partial > fallback_only >
unsupported` — a *derived rollup* of its `surfaces[].support`.

### 5.4 Main flows

**5.4.1 Connect / setup (happy path)** — V5 §§10/16, alias `connect = setup`:

```text
altevra connect --tool claude-code --project altevra [--dry-run]
 1. adapter.detect(repo)                         → AdapterDetectionResult
 2. load adapter_dossier + capability_records    → know native vs fallback surfaces
 3. load skills (/06-skills) + universal hooks (/07-capabilities/hooks.yaml)
 4. adapter.render_{instructions,skills,hooks}() → Vec<GeneratedFile>  (deterministic, T3)
 5. FOR EACH GeneratedFile.body: §2 ingest_guard(body)   → reject/redact if secret (T6)
 6. adapter.build_install_plan()                 → InstallPlan (create/update/DRIFTED)
 7. IF --dry-run: print plan, mutate nothing (T5); STOP
 8. IF plan has drifted files: route to review (5.4.3), DO NOT overwrite (T4)
 9. IF any component is capability_grade=execute OR sensitivity≥confidential:
       create review_item (T9); apply only the approved subset
10. adapter.install(plan)                        → write managed files w/ header (T-CHK)
11. upsert tool_installation + installed_component rows (status=current)
12. adapter.verify()                             → confirm checksums; recompute statuses (T8)
13. write/verify capability_records w/ evidence_ref = this verify run (T7)
14. emit events: tool_connected, component_installed (V5 §6 feed)
```

**5.4.2 Skill factory (propose → render → monitor → deprecate)** — P0.7,
`CLAUDE.md` §12 compounding loop:

```text
[§4 resident skill_factory_proposer] detects a repeated workflow across sessions
  → emits skill_proposal{dedup_hash, proposed_body:SkillBody, occurrences, target_agents}
  → DEDUP: if dedup_hash exists, increment occurrences, do NOT create a 2nd (T12 / duplicate_of)
  → review_item (skill creation is a broad change ⇒ protected, §2.9, T9)
  → [Pavle approves, human-presence-authenticated §2.9]
  → §5 materialize: skill_proposal(applied) ⇒ new `skill` object (slug, body, checksum)
  → render to each target agent via its ToolAdapter (T11) → installed_component rows
  → monitor usage (hook_runs / event feed) → if unused for N: propose deprecated
```

**5.4.3 Drift reconciliation (review / rejection path)** — a human edited a
managed file (§2.7 / §2 I6):

```text
verify (or watcher) finds disk_hash != installed_component.checksum
  → status = drifted (or conflicted)
  → capture human edit as pending_human_override (quarantined, §2.7)
  → create review_item with a 3-WAY DIFF:
        base   = last generated snapshot (component.checksum)
        ours   = current DB render (re-render at latest revision)
        theirs = human edit on disk
  → NEVER silently overwrite (T4)
  → Pavle chooses:  promote-edit-to-authored | discard+regenerate | fork
  → on resolve: emit component_reconciled; recompute status → current
Rejection path: if Pavle rejects a skill_proposal/grant, status → rejected/withdrawn;
no skill/component is created; dedup_hash retained so the same pattern does not
re-propose immediately (cool-down owned by §4).
```

**5.4.4 Capability query (bootstrap composition)** — `agent bootstrap` and
`get_agent_bootstrap_packet` compose two slices:

```text
bootstrap_packet =
  §5 slice:  skill_freshness(check_version_opt per skill)   // current/outdated/not_installed
           ⊕ setup_status(installed_component statuses)
           ⊕ capability_matrix(adapter_dossier + capability_records, gated by §2 exposure_gate)
  ⊕ §3 slice: context_packet(intent=bootstrap)              // owned by §3, T10/§3 INV-14
All reads pass §2 exposure_gate; no raw secret ever returned (T10).
```

**5.4.5 Cross-agent skill/tool sharing** — Constitution §10:

```text
altevra grant --skill <slug> --to hermes [--trust render] [--scope global]
  → build capability_grant{grantee, subject, trust_level, requires_approval}
  → requires_approval = (capability_grade ∈ {install,execute}) OR (sensitivity ≥ confidential)
                        OR (target domain ∉ skill.domain allowance §6)
  → if requires_approval: review_item (human-presence approval, §2.9); status=pending
  → on approve: status=granted; render via grantee adapter (5.4.1 steps 4–13)
  → revoke: status=revoked; uninstall component; emit grant_revoked
A grant NEVER auto-elevates trust; an agent cannot grant itself (T9, §2 I9).
```

### 5.5 Invariants (other sections / tests enforce)

Named `T#` so reviewers can cite them; each maps to a test (§5.11).

- **T1 (envelope conformance):** `skill, hook, adapter_dossier, capability_record,
  tool_installation, installed_component, skill_proposal, capability_grant` carry
  the full §1.3 envelope (conformance meta-test §1.14.2).
- **T2 (one checksum/revision mechanism):** a faced object's envelope `checksum`
  is the body hash recorded in its managed-file header; drift detection and
  supersession read the same `(checksum, revision)` — satisfies §1's request +
  I9. No second hashing scheme.
- **T3 (deterministic render):** `render_*` for the same `(object, revision,
  adapter)` is byte-identical across runs. The managed header carries **no
  timestamp / no nonce** (live `test_managed_header_no_timestamp` — keep green).
  Non-determinism would make every render look like drift.
- **T-CHK (checksum locus):** the header's `checksum` is computed over the
  **body before the header is prepended** (live `GeneratedFile::new` → then
  `with_managed_header`). Drift check = strip header → hash remaining body →
  compare to header `checksum`. (Prevents the self-referential header paradox.)
- **T4 (no silent overwrite):** a `drifted`/`conflicted` managed file is never
  overwritten by `install`; it routes to review with a 3-way diff (§2 I6, V5 §10).
- **T5 (dry-run first):** every `install` is preceded by `build_install_plan`;
  `--dry-run` and any agent-initiated render mutate nothing on disk.
- **T6 (no raw secret in rendered artifact):** every `GeneratedFile.body` and
  every `SkillBody.commands[]` passes §2 `ingest_guard` before write; secrets are
  `{{secret:<handle>}}` placeholders resolved by the CLI at runtime (V5 §17, §2 I4).
- **T7 (capability honesty):** `capability_record.support = supported` requires a
  non-null `evidence_ref` from a passing verify/test; absent that it is
  `unverified` (Constitution Law 6). No adapter advertises an unproven native
  surface.
- **T8 (component state is computed):** `installed_component.status` is derived by
  `verify` from (version-compare ⊕ checksum-compare ⊕ adapter-support), never set
  by an agent-supplied field.
- **T9 (broad capability is review-gated):** creating/installing/sharing a skill
  or grant with `capability_grade ∈ {install, execute}`, `TrustLevel ≥ install`,
  or `sensitivity ≥ confidential` creates a `review_item`; agents **propose**,
  never self-grant (§2.9 I9, Law 4).
- **T10 (gated reads, no raw secret over the wire):** all skill/capability/
  registry reads pass §2 `exposure_gate`; no MCP tool returns a raw secret value;
  an agent caller gets a handle/metadata only — raw resolve requires human-CLI
  TTY + unlock (§2.16).
- **T11 (single native-write boundary):** core/CLI/MCP never write a tool's
  native config directly; **only** `ToolAdapter::install` does. One renderer, no
  duplicate logic (V5 §5). CLI `connect` and MCP `get_setup_status`-driven plan
  derive from the same adapter.
- **T12 (canonical uniqueness):** `skill.slug`, `hook.slug`,
  `adapter_dossier.tool_name` unique; `(tool,project)` and
  `(installation,component_slug)` unique (live constraints); `skill_proposal`
  deduped by `dedup_hash`; duplicates resolved via §1 `duplicate_of`, never silent
  merge.
- **T13 (managed/authored zone integrity):** generated artifacts live only in the
  generated zone (`/15-generated/**`, `.claude/`, `AGENTS.md`, …) with the managed
  header; authored skills (`/06-skills` human-written) carry no managed header and
  are `obsidian_authored` (§2.14).

### 5.6 Failure modes and mitigations

| # | Failure | Trigger | Mitigation |
|---|---------|---------|------------|
| FM-1 | Drift storm (every render flagged drifted) | non-deterministic render (timestamp/nonce in header or body) | T3 + live no-timestamp test; render is pure fn of `(object,rev,adapter)` |
| FM-2 | Adapter claims native hook the tool ignores | `support=supported` by declaration | T7 evidence requirement; `verify` must prove; else `fallback`/`unsupported` |
| FM-3 | Secret baked into a skill/instruction file | factory or human pastes a key into `SkillBody.commands` | T6 `ingest_guard` pre-write; reject/redact; `secret_sighting` + review (§2.5) |
| FM-4 | Stale component status (file deleted out of band) | manual `rm` of a managed file | `verify` recomputes → `missing`; `doctor` orphan/missing scan |
| FM-5 | Silent over-grant across agents | broad skill shared without review | T9 review gate; `requires_approval` forced for `install/execute` grade |
| FM-6 | `Ahead` version (installed newer than registry) | manual install / registry rollback | mapped to `conflicted` → review (Q1); never auto-clobbered |
| FM-7 | Two adapters target the same path | install-target overlap | path-collision check in `build_install_plan`; reject with explicit conflict |
| FM-8 | Duplicate skill proposals for one workflow | detector fires repeatedly | `dedup_hash` canonical key; increment `occurrences`; `duplicate_of` (T12) |
| FM-9 | CLI vs MCP install divergence | duplicated render logic | T11 single adapter core; cross-surface test (§5.11) |
| FM-10 | Header/body checksum paradox | hashing the header-with-checksum | T-CHK: hash body *before* header prepend; strip-then-hash on verify |
| FM-11 | Gap objects undefined | `adapter_dossier, capability_record, skill_proposal, capability_grant` have **no migration yet** | P0.1 lands these tables with full envelope before factory/sharing writes (build-plan dependency, mirrors §1 FM-11) |
| FM-12 | Legacy rows lack envelope | live `skills/hooks/tool_installations/installed_components` miss `schema_version, sensitivity, provenance, domain, revision` | additive migration + backfill (`internal`, `provenance.origin=imported`, `schema_version=1`); non-breaking (mirrors §1 FM-12) |
| FM-13 | Capability claimed for an unsupported tool, then relied on at runtime | optimistic dossier | hooks degrade to CLI fallback (V5 §9 "be honest"); `degraded_to` recorded; runtime checks `capability_record` |

### 5.7 Security / privacy risks and how §2 gates apply

- **Secret leakage into generated files** (the classic): every rendered
  `GeneratedFile.body` and `SkillBody.commands` is a §2 **capture boundary** and
  MUST pass `ingest_guard` *before* write (T6). Hook files never contain a secret
  value — they call `altevra …` which resolves `{{secret:<handle>}}` internally
  (V5 §17). **Request to §2/P0.2:** add "rendered artifact / generated file" to
  the enumerated ingestion boundaries (§5.14).
- **Raw secret over MCP** — no tool/skill/capability MCP verb returns a raw
  secret; `secrets get` to an agent caller returns handle + metadata only; raw
  reveal requires TTY + unlock (§2.16, T10).
- **Capability existence side-channel** — a `capability_record`/skill above the
  caller's ceiling must not be *confirmed* to exist (mirror §1 id-enumeration /
  §2.13 inclusion side-channel): `exposure_gate` filters the registry listing;
  reason codes are coarse ("items above ceiling omitted").
- **Over-broad cross-agent grants** — `execute`-grade skills and `≥ confidential`
  sensitivity are always review-gated (T9); approval needs human presence (§2.9),
  so an agent cannot forge `approved=true` in a payload.
- **Cross-domain skill bleed** — a `restricted`/personal-domain skill must not
  render to a work agent; sharing checks the skill's `domain`/`sensitivity`
  against the grantee's audience ceiling (§2.3 `exposure_policy`).
- **Prompt injection via skill body** — a skill body is **data**, rendered as
  instructions to the *target* tool only after review; ingested workflow evidence
  is data, never policy (§2 I14). The factory cannot be steered by content it
  ingested into auto-granting itself a capability.
- **Managed-file tamper** — drift detection (T2/T-CHK) is the integrity check; a
  human edit is surfaced (review), an unexpected change is an audit signal (§2.10).

### 5.8 Obsidian implications

- **Zones (§2.14):** authored skills live in `/06-skills/*.md` (no managed
  header, `obsidian_authored`); generated skills, instruction files, setup packs
  live in the **generated zone** (`/15-generated/setup-packs/{tool}/`, managed
  header required) and in tool repos (`.claude/`, `AGENTS.md`, `.cursor/rules/`,
  …). Universal hook + adapter matrix are config faces:
  `/07-capabilities/hooks.yaml`, `/07-capabilities/agent-tools.yaml`.
- **Frontmatter contract** for a skill `.md` = envelope subset + skill keys
  (`slug, version, title, tools, tags, trigger`). Round-trip identity is law (I9):
  frontmatter `id` == DB `id`, `checksum` matches body (T-CHK). The live skill
  parser frontmatter (`slug, version, title, description, author, tools, tags`)
  is the seed; P0.1 adds the envelope keys.
- **Managed header** = `ALTEVRA_MANAGED, source, generated_by, adapter, version,
  checksum` (live `with_managed_header`) — **no `generated_at`** (T3).
- **Wiki hygiene:** `installed_component`, `tool_installation`, `capability_record`
  are **DB-only** — they are machine install-state, never dumped into notes (keeps
  the brain clean, the whole point of Altevra). An optional human-readable
  capability page MAY be a `generated_mirror`, but is not canonical and is not
  written by default. `confidential+` skills are DB-only (`mirror_to_markdown=false`).

### 5.9 Cloud / local sync implications

- **Definitions vs install-state split.** Universal `skill` and `hook`
  *definitions* are shareable knowledge → **sync-eligible** subject to the
  sensitivity ceiling (§1.12, §2.15). `tool_installation`, `installed_component`,
  and device-scoped `capability_record` describe **this machine** (contain
  `installed_path`) → **never sync** (local-per-device, `origin_device` set).
- **No content conflict on definitions:** deterministic render (T3) means two
  devices produce byte-identical files from the same `(skill, revision)`; install
  state is per-device so there is nothing to merge. A definition edit follows §1
  supersession (new revision), not last-writer-wins.
- **`skill_proposal`** is knowledge → may sync; its **approval/grant** is a
  protected decision → recommend `global` scope with review (Q3, §5.13).
- **Capability honesty across devices:** a `supported` verdict is device-specific
  evidence; on a new device the verdict resets to `unverified` until that device's
  `verify` runs (no inheriting another machine's proof).

### 5.10 CLI / MCP implications (verbs + caller boundaries)

**One core, two faces** (V5 §5; T11). Verbs below are **disjoint from §3's**
`context | packet | search` surface (confirmed with §3, §5.14):

```bash
# capability / tool registry
altevra capabilities show [--actor <a>] [--json]
altevra capabilities verify --tool <t>              # runs adapter.verify → evidence
altevra adapter list [--json]
altevra adapter dossier --tool <t> [--json]
# install / setup (alias: connect == setup)
altevra connect|setup --tool <t> --project <p> [--dry-run] [--json]
altevra setup verify|repair|status --tool <t> --project <p> [--json]
altevra component list --tool <t> [--project <p>] [--json]   # statuses (5.3)
# skills + hooks
altevra skill list|show|check [--all] [--json]
altevra skill propose <slug> | render <slug> --tool <t> [--dry-run] | refresh <slug>
altevra hook list|status|verify|install [--json]
altevra hook run <event> --tool <t> --project <p> --json "$PAYLOAD"
# cross-agent sharing (grants)
altevra grant list|show [--json]
altevra grant --skill <slug> --to <agent> [--trust <lvl>] [--scope <s>]   # review-gated if broad
altevra grant approve|revoke <id>                  # approve requires human-CLI TTY (T10)
```

**MCP tools (all → same core, adapter cannot bypass):**
`get_agent_bootstrap_packet`, `check_altevra_skill_version`, `get_altevra_skill`,
`list_skills`, `get_skill`, `get_capabilities`, `get_setup_status`,
`report_capability_gap`, `request_skill_refresh`, `run_hook`,
`create_review_item` (shared w/ §2). **Caller boundary:** read/list/check/
dry-run-render are agent-callable; `install`/`connect`/`grant approve`/secret
reveal of a **broad/protected** component require human-CLI TTY presence (§2.16,
T9/T10). MCP `get_capabilities` is gated by `exposure_gate` and never confirms
over-ceiling capabilities.

### 5.11 Required tests / fixtures / golden snapshots

Extends `repository_roundtrip.rs` + the live adapter/skill tests.

1. **Envelope conformance** for the eight §5.2 objects (shared §1.14.2).
2. **Roundtrip per new gap object** (`adapter_dossier, capability_record,
   skill_proposal, capability_grant`): insert → get/list → every envelope field
   survives (FM-11).
3. **Deterministic render golden snapshot** per `(skill, adapter)`: committed
   under `crates/altevra-adapters/tests/golden/`; double-render byte-equal (T3);
   keep `test_managed_header_no_timestamp` green.
4. **Drift detection:** write managed file → mutate body → `verify` reports
   `drifted`; `install` refuses overwrite; `review_item` with 3-way diff created
   (T4, §2 I6).
5. **Checksum locus (T-CHK):** header `checksum` == hash(body-before-header);
   verify strips header, re-hashes, matches.
6. **Component state machine:** each `VersionCheckResult` (`Current/Outdated/
   Ahead/NotInstalled/NotFound/ParseError`) + each disk condition (`absent`,
   `mismatch`, `match`) → expected status (5.3); `Ahead → conflicted`.
7. **No-secret-in-render:** skill body / instruction file with planted fake
   `sk-…`/`ghp_…`/`AKIA…`/JWT/`postgres://` → render rejected or `{{secret:h}}`;
   absent from any written file (shared §2.17 / P0.2 corpus).
8. **Capability honesty (T7):** `support=supported` without `evidence_ref`
   rejected; `adapter.verify` produces evidence; unsupported tool → `fallback`.
9. **Cross-surface parity (T11):** CLI `connect --dry-run` plan == MCP
   `get_setup_status`-derived plan for the same `(tool, project)`.
10. **Skill factory (P0.7):** fixture repeated workflow → exactly one deduped
    `skill_proposal`; approve → renders a `skill` with non-empty `trigger, steps,
    commands, pitfalls, verification` to the target dir.
11. **Cross-agent grant (T9):** sharing an `execute`-grade skill → `review_item`,
    `status=pending`, no component written until approved; forged `approved=true`
    payload rejected.
12. **Path-collision (FM-7):** two adapters with overlapping `install_targets` →
    plan rejects with explicit conflict.

### 5.12 Acceptance criteria for P0.0 / P0.1

**P0.0 (contracts — this section):**
- [ ] The eight §5.2 objects each have: section schema, canonical key, face
      decision, status machine (5.3), and consume the §1 envelope.
- [ ] Drift ⇄ supersession unified on one `(checksum, revision)` mechanism (T2/
      T-CHK) — explicitly answers §1's §5 cross-request.
- [ ] Component status enum reconciled with §1.5; `Ahead`/`NotFound`/`ParseError`
      handling defined (5.3).
- [ ] CLI/MCP verb surface is disjoint from §3 and routed through one adapter
      core (T11) and §2 gates (T6/T9/T10).
- [ ] All invariants T1–T13 stated with a matching test in §5.11.

**P0.1 (data-model upgrade — build-plan unit, "skill_proposals … only" + this
section's siblings):**
- [ ] Migrations land `adapter_dossier, capability_record, skill_proposal,
      capability_grant` with full envelope (FM-11); migrations-from-empty pass.
- [ ] Additive envelope backfill for live `skills/hooks/tool_installations/
      installed_components` (FM-12), non-breaking.
- [ ] Repository roundtrip tests (§5.11.2) pass; component-state mapping test
      (§5.11.6) passes.
- [ ] No raw secret in any durable fixture or rendered artifact (§5.11.7).
- [ ] `cargo fmt --check && cargo test && cargo build && cargo clippy --workspace
      -- -D warnings` green (P0 build rule).
- **Scope guard:** no resident *execution*, no external model dependency, no new
  connectors — proposals land in tables; rendering/grant *application* stays
  review-gated and behind human presence (build-plan "landing zone, not
  autonomous" rule).

### 5.13 Unresolved questions (owner / recommended default)

- **Q1 — `Ahead` component state.** Installed version > registry latest (manual
  install or registry rollback). *Recommend:* `conflicted` → review, never
  auto-clobber. *Owner:* implementation reviewer (Codex) + Hermes.
- **Q2 — `skill_proposal` ownership split.** §1 routes it to §4; §5 needs the
  render contract. *Recommend:* co-own — §4 owns generation + status lifecycle,
  §5 owns post-approval render/install/usage-tracking/deprecation (as drafted).
  *Owner:* Hermes (reconcile §4↔§5 before synthesis).
- **Q3 — Fold `skill_installation` into `installed_component`?** Live schema has
  both (`003` vs `005`). *Recommend:* keep `installed_component` canonical with
  `component_type=skill`; treat `skill_installations` as a legacy view/migration
  target. *Owner:* object-model + implementation reviewer.
- **Q4 — Capability verification cadence.** *Recommend:* verify on `connect` +
  periodic `doctor`; `bootstrap` reads cached `capability_record` (no live
  re-verify on the hot path). *Owner:* §4 observer.
- **Q5 — `prompt_proposal` rendering to *other* agents.** Should system-prompt
  tweaks to Claude/Codex/etc. route through the adapter layer like skills?
  *Recommend:* yes for tools with a managed prompt file (treat as
  `component_type=prompt`); proposal side stays in §4. *Owner:* §4 + §5.
- **Q6 — Storage backend.** Same escalation as §1/§2/§3 (V5 says Postgres; repo +
  doctrine say SQLite). My contracts are backend-neutral. *Recommend:* SQLite
  local-first canonical, Postgres opt-in mirror. *Owner:* Hermes.
- **Q7 — Is Hermes a render target (adapter)?** `CLAUDE.md` §12 says Altevra
  manufactures skills *for Hermes*. *Recommend:* yes — Hermes gets a
  `ToolAdapter` (skills → `~/.imperium/skills/shared/`), making cross-agent
  sharing symmetric. *Owner:* Hermes.

### 5.14 Cross-section requests

- **→ §1 object-model:** register `adapter_dossier, capability_record,
  skill_proposal, capability_grant` as durable `type`s in §1.2 with canonical
  keys (`tool_name`; `(actor, capability_key)`; `dedup_hash`;
  `(grantee, subject.ref)`); add the component status values
  `{current, outdated, drifted, missing, conflicted, unsupported}` to the §1.5
  status superset (already partially present for `skill/hook` installed); confirm
  the §5.2 envelope-`checksum` == managed-header-`checksum` mapping (this section
  answers your §5 request — please ratify). Confirm `skill_installation` →
  `installed_component(component_type=skill)` consolidation (Q3).
- **→ §2 safety-source-truth:** add **"rendered artifact / generated file"** to
  the enumerated `ingest_guard` ingestion boundaries (P0.2 list) — a generated
  skill/instruction/hook file is a write surface that can leak a secret (T6).
  Confirm the protected-change predicate includes **broad skill creation** and
  **`capability_grant` with `TrustLevel ≥ install` or `capability_grade =
  execute`** (T9). Confirm human-presence auth applies to `connect`/`grant
  approve`/`secrets reveal` (T10). Expose the `exposure_gate` signature for
  capability/skill *registry listing* (existence side-channel, §5.7).
- **→ §3 context-retrieval:** confirm the **bootstrap-packet composition
  boundary** (§5.4.4): §5 supplies skill-freshness + setup-status + capability
  matrix; §3 supplies the `intent=bootstrap` context packet; `get_agent_bootstrap_
  packet` composes both without re-implementing the compiler. Verb surfaces are
  disjoint (`connect/skill/hook/capabilities/adapter/grant/component` vs
  `context/packet/search`) — please keep them so.
- **→ §4 agents-self-improve:** **co-own `skill_proposal`** — you own detection
  (`skill_factory_proposer`), generation, and the proposal status machine; §5
  owns the post-approval render/install, usage tracking, and deprecation. Route
  `prompt_proposal` rendering to *other* agents' managed prompt files through the
  §5 adapter layer (Q5). Your observer should consume **`installed_component`
  drift**, **`capability_gap` reports**, and **skill usage stats** as signals to
  propose patches/deprecations.
- **→ §6 domains-lifecycle:** provide per-domain **default sensitivity for
  skills** (does a "personal-life" skill default `restricted`?), the
  **domain allowance** controlling which domains a skill may be shared into
  (gates §5.4.5), and **retention/TTL** for dismissed `skill_proposal`s and stale
  `capability_record`s.
- **→ §7 Hermes synthesis:** ratify (a) the `ToolAdapter` render path as the
  **single tool-native write boundary** (T11); (b) the `Ahead`-state policy (Q1)
  and `skill_proposal` ownership split (Q2); (c) Hermes-as-render-target (Q7); and
  (d) the SQLite-not-Postgres reconciliation (Q6) consistent with §1/§2/§3.

### 5.15 Summary of this section's changes

Replaced the TODO with the **Tools + Skills + Interfaces** contract. It defines:
eight durable objects consuming the §1 envelope (`skill` + structured
`SkillBody`, `hook`, `adapter_dossier`, `capability_record`, `tool_installation`,
`installed_component`, `skill_proposal`, `capability_grant`); a `TrustLevel`
ladder, `Support`/`component_type` enums, and a **computed** installed-component
state machine aligned to §1.5; five main flows (connect, skill factory, drift
reconciliation, bootstrap composition, cross-agent sharing) with explicit
happy-path and review/rejection paths; thirteen invariants (T1–T13) — notably
**one `(checksum, revision)` mechanism for drift + supersession** (answering §1's
request), **deterministic render** (no header timestamp), **no silent overwrite**,
**no raw secret in any rendered artifact**, **capability honesty with evidence**,
and the **single native-write boundary**; failure modes; security/privacy risks
mapped to §2 gates; Obsidian zone/frontmatter/wiki-hygiene rules; a
definitions-sync-but-install-state-local split; a CLI/MCP verb surface disjoint
from §3 and routed through one adapter core; required tests/golden snapshots;
P0.0/P0.1 acceptance criteria; seven unresolved questions with recommended
defaults; and cross-section requests to §1, §2, §3, §4, §6, and §7.

<!-- END_SECTION: tools-skills-interfaces -->

---

<!-- SECTION: domains-lifecycle -->
<!-- OWNER: opus-domains-lifecycle -->
<!-- STATUS: drafted-by-opus-domains-lifecycle -->
## 6. Domains + Lifecycle

> Author: `opus-domains-lifecycle` (Opus 4.8 MAX). Scope: the **per-domain policy
> layer** and the **object lifecycle/retention engine** for Altevra/VVLT.
> §1 (object-model) fixes the `domain` *field* and the *value list* and the
> abstract status machines; §2 (safety) fixes the *gate mechanism*; §3
> (context-retrieval) *consumes* TTLs. **This section owns the policy each domain
> resolves to** (default sensitivity, audience, cloud-sync ceiling, embedding
> role, Obsidian mirror, retention class, soft/hard TTLs, RTBF/legal-hold,
> export class) **and the lifecycle rules that keep the brain compounding without
> becoming cluttered** (project archival demotion, cross-project scope promotion,
> provenance compaction, retention/forget/export). It is **policy law**, grounded
> in the Constitution (Law 6 "business and personal first-class but bounded",
> Law 2 capture≠exposure, Law 5 self-improvement), `CLAUDE.md` §3.1/§4.4, and the
> live `crates/altevra-db` envelope (§1). Where this section and another disagree
> on a value, the conflict is itemized in §6.13 (unresolved) / §6.14 (cross-req).

### 6.1 Purpose and non-goals

**Purpose.** Make "business and personal are first-class but bounded" (Law 6)
*operational* by giving every durable object a **domain** whose **policy** is the
single source of its safety/retention/sync/exposure defaults, and by defining the
**lifecycle** that lets a decades-long brain (i) compound knowledge across many
projects/agents/tools, while (ii) never cluttering a default read with stale,
archived, or out-of-scope material, and (iii) keeping Pavle sovereign over his
own data (export, forget, local-first).

Concretely §6 answers, with contracts:

1. *What does each domain default to?* — the per-domain policy matrix (§6.4).
2. *How long does a thing live, and what happens when it ages?* — retention
   classes + the lifecycle engine (§6.5–§6.6).
3. *How does work compound across projects without clutter?* — project lifecycle
   + scope promotion + provenance compaction (§6.7).
4. *How does Pavle take his data out / make Altevra forget?* — export + forget
   per domain, RTBF, legal hold (§6.8).
5. *What may leave the machine?* — per-domain cloud-sync map (§6.10).
6. *Where does it live as human-readable markdown?* — vault zone map (§6.9).

**Non-goals (owned elsewhere — do not re-specify here):**
- The `domain` field/enum *definition* and envelope shape → §1.4.8/§1.5.
- The secret-detection / redaction *mechanism* and `exposure_gate` / `ingest_guard`
  → §2. §6 supplies the *defaults* those primitives fall back to; it does not
  re-implement them.
- Retrieval ranking, recency half-lives, packet packing → §3. §6 supplies the
  *TTL values* §3.9 consumes; it does not rank.
- Resident-agent mode logic and review-queue UX → §4. §6 declares *which*
  lifecycle transitions are review-gated; §4 runs the proposals.
- Skill/hook/tool registry surfaces → §5; §6 only states that those objects are
  `global`/`project`-scoped and obey the same retention (deprecated≠deleted).

### 6.2 Domain registry as a durable object (consumes §1 envelope)

A domain is **not** a bare string. Each domain is backed by a `domain_policy`
durable object carrying the full §1 envelope, so the policy itself is typed,
versioned, provenanced, and review-gated to change (it *is* policy → Law 4).

```jsonc
domain_policy {                       // type = "domain_policy"; one row per domain
  // ── §1 mandatory envelope (verbatim from §1.3) ──
  "id": "...", "type": "domain_policy", "schema_version": 1,
  "status": "active",                  // active | superseded (policy edits supersede, never overwrite)
  "created_at": "...Z", "updated_at": "...Z",
  "provenance": { "origin": "pavle_direct" | "system_derived", ... },
  "sensitivity": "internal",           // the policy ROW's own sensitivity (the matrix is internal, not secret)
  "domain": "business",                // self-descriptor of the row's own classification
  "scope": null,                       // policies are global
  "revision": 1,

  // ── policy payload (the value list §1.4.8 switches on, now with behavior) ──
  "domain_key": "business",            // governed enum (§1.5) — the key other objects carry
  "display_name": "Business",
  "description": "...",
  "is_builtin": true,                  // 9 builtins seeded; new domains are review-gated additions
  "policy_version": 1,                 // bumped on any policy edit; snapshotted onto objects at write (D2)

  "default_sensitivity":   "internal", // §1.4.7 ladder value applied at create if object doesn't override
  "max_sensitivity":       "confidential", // ceiling a member may carry without an extra review (else quarantine)
  "default_audience_ceiling": "project_agents", // §2.3 exposure audience enum
  "cloud_sync":            "encrypted_only",    // §2.15 ceiling: disabled | encrypted_only | allowed
  "embedding_model_role":  "cloud_ok",          // local_private | cloud_ok  (personal → local_private)
  "obsidian_mirror":       "opt_in",            // never | opt_in | default_on  (markdown face default)
  "obsidian_zone":         "30-business",       // vault zone key (§6.9); null when obsidian_mirror=never
  "retention_class":       "long",              // permanent | long | standard | ephemeral (§6.5)
  "soft_ttl_days":         180,                 // → default review_after offset; null = no nudge
  "hard_expiry_days":      null,                // → default valid_until offset; null = never expires
  "review_on_write":       false,               // does a write to this domain open a review_item (§2.9)?
  "rtbf_required":         false,               // must support hard delete on request (§6.8)?
  "legal_hold_capable":    false,               // may carry a delete-blocking legal_hold flag?
  "export_class":          "on_request"         // always | on_request | restricted (§6.8)
}
```

**Per-object snapshot (D2).** At create time, the object records
`policy_version` (which policy generation seeded its defaults) and the *resolved*
`default_sensitivity` / `cloud_sync` / `valid_until` / `review_after`. A later
policy edit does **not** retro-mutate existing objects (FM in §6.6); re-evaluation
is an explicit, review-gated job. This makes policy changes safe on a brain with
years of rows.

### 6.3 Domain taxonomy: governed set vs living categories

Mirrors §1.2's split between the **governed `type` registry** and the **living
`category` taxonomy** (§1.4.9), placing `domain` precisely:

- **`domain` is governed** — the 9 builtins (§6.4) are closed by default. Adding a
  domain (e.g. `fitness`, `music`, `content`) is a **review-gated** change (it
  mints a `domain_policy` object = policy edit, Law 4). This prevents domain
  sprawl that would dilute the safety contract.
- **`category` is living** — inside a domain, auto-categorization (`CLAUDE.md`
  §3.2) freely creates categories (auto-applied, surfaced in the daily digest,
  merges/renames review-gated per §1 Q5). Categories are the cheap, fast taxonomy;
  domains are the load-bearing safety boundary.
- **Distinction is hard:** a new *category* never changes a record's
  sensitivity/sync defaults; only its **domain** does. An agent that wants to
  "create a new domain" emits an `agent_proposed` review item (§2.9), never a
  direct write (D10).

### 6.4 The per-domain policy matrix (canonical defaults — fulfils §2.20 + §3.21)

This is the map §2.20 ("domain→default-sensitivity" and "domain→cloud_sync
ceiling") and §3.21 ("per-domain soft TTL + hard-expiry") explicitly requested.
Sensitivity values use the **§1 six-level ladder**
(`public < shareable < internal < confidential < secret < restricted`); see
§6.13 Q-LADDER for the §1/§2 reconciliation. `secret` is credential-class only
(raw secrets never live in object text — §1.10/§2.5), so **no domain defaults to
`secret`**.

| domain | default_sens | max_sens | audience_ceiling | cloud_sync | embed_role | obsidian_mirror | retention | soft_ttl | hard_expiry | review_on_write | rtbf | legal_hold |
|--------|------|------|------|------|------|------|------|------|------|------|------|------|
| `business` | internal | confidential | project_agents | encrypted_only | cloud_ok | opt_in | long | 180d | none | no | no | no |
| `project` | internal | confidential | project_agents | encrypted_only | cloud_ok | default_on | standard | 90d | none | no | no | no |
| `client` | confidential | restricted | trusted_agents | **disabled** | **local_private** | **never** | long | 365d | per-contract¹ | **yes** | **yes** | yes |
| `personal` | confidential | restricted | pavle_only | **disabled** | **local_private** | opt_in | permanent | none | none | **yes** | yes(req) | no |
| `relationship` | restricted | restricted | pavle_only | **disabled** | **local_private** | **never** | permanent | none | none | **yes** | yes(req) | no |
| `health` | restricted | restricted | pavle_only | **disabled** | **local_private** | **never** | permanent | none | none | **yes** | yes(req) | no |
| `legal` | confidential | restricted | pavle_only | **disabled** | **local_private** | **never** | permanent | none | none | **yes** | **conditional²** | **yes** |
| `financial` | confidential | restricted | pavle_only | **disabled** | **local_private** | **never** | long | none | 7y→review³ | **yes** | **conditional²** | yes |
| `public` | public | shareable | shareable_public | allowed | cloud_ok | default_on | standard | 365d | none | no | no | no |

¹ `client.hard_expiry = per-contract` — bound to a client engagement record; on
contract close, client PII enters the RTBF path (§6.8). ² `legal`/`financial`
RTBF is **conditional**: a `legal_hold` (tax, dispute, regulatory) *blocks* delete
until released (D7). ³ `financial.hard_expiry = 7y→review`: after the statutory
retention window (default 7y — jurisdiction TBD, §6.13 Q-FIN) the object is
flagged for review, **not** auto-purged.

**Resolution rule (D1/D2).** On create, an object's effective defaults =
its explicit fields, else its `domain_policy` snapshot, else system defaults
(`internal` / `disabled` / `cloud_ok` / no-TTL). A **multi-domain** object
(`domains[]`, §1.4.8) resolves to the **most restrictive** policy across its
domains for every field (max sensitivity, min audience, min cloud_sync,
local_private if any member is, never-mirror if any member is, shortest TTL of
the retention-bearing members). This is the lifecycle-side mirror of §2's
monotone `combine()` and §1's I6 domain-union.

### 6.5 Retention classes + lifecycle state machine

Retention class (from the matrix) decides what aging *does*. It is orthogonal to
the §1 status machine; it drives the **derived** staleness state §1.4.11 defines
(`fresh | due_for_review | expired | superseded`) and the actions on it.

| retention_class | soft_ttl effect | hard_expiry effect | auto-archive? | auto-purge? | examples |
|---|---|---|---|---|---|
| `permanent` | nudge only (review_after) | n/a (no expiry) | **never** | **never** | decisions, learnings, identity, personal, relationship, health, legal |
| `long` | nudge | flag for review at expiry | only on project/engagement archive | **never** | business, financial, client |
| `standard` | nudge | **auto-archive** (soft) at expiry | yes (retrievable via history) | **never** | project ops, kept research, public |
| `ephemeral` | — | — | — | **hard-purge** at TTL (only path that hard-deletes) | low-importance `system_event`, dismissed `research_item`, expired `context_packet`, `embedder_queue` |

**Lifecycle state machine (per object, retention-driven):**

```text
                 now>review_after                refresh/supersede
   fresh ───────────────────────────▶ due_for_review ───────────────▶ fresh
     │                                      │
     │ now>valid_until                      │ now>valid_until
     ▼                                      ▼
   expired ───[standard]──▶ archived (status=archived, retained, history-only read)
     │
     ├──[ephemeral, non-canonical, non-sync]──▶ purged (HARD delete, §2.8 fast-path)
     └──[permanent|long]──▶ (stays expired+flagged; NEVER auto-archived/purged)
```

- `due_for_review` and `expired` are **derived** (computed by the lifecycle job),
  not stored statuses — except the terminal `archived` (a real §1 status flip) and
  `purged` (a §2 tombstone). This keeps the engine idempotent and re-runnable.
- **Auto-archive is soft** (`status=archived`, object retained, excluded from
  default reads per §1 I3 / §3.9, available with `--include-archived` /
  `*_history` intent). It is the primary anti-clutter lever and is **reversible**.
- **Auto-purge is the only auto-hard-delete** and is fenced by D6 (ephemeral +
  non-canonical + non-sync + DB-only). It can never touch a canonical or
  sync-eligible object.

### 6.6 Lifecycle engine (the periodic job) — happy path + review/rejection path

A periodic `lifecycle` brain job (idempotent, dry-runnable) walks objects by
domain and applies §6.5. It is a `resident_run` (§1) for audit/observability.

**Happy path (no protected transition):**
```text
lifecycle.run(now, [--dry-run])
  for each live object o (batched by domain, cheap structured index — §1 Q2):
    p   = domain_policy(o.domain)            # snapshot-aware
    st  = derive_staleness(o, p, now)        # fresh|due_for_review|expired|superseded
    case st:
      due_for_review → enqueue digest nudge (no status change);    emit lifecycle_nudge
      expired & standard         → o.status = archived (soft);     emit object_archived
      expired & ephemeral & D6   → schedule purge (→ §2.8 fast);   emit object_purged (tombstone, no content)
      expired & permanent|long   → flag + digest only;             emit lifecycle_flagged
    record action in resident_run output
  return LifecycleReport { scanned, nudged, archived, purged, flagged, skipped_protected }
```

**Review / rejection path (protected or destructive transition):**
```text
  if action would: hard-delete a canonical/sync object,
                   archive a `review_on_write` domain object Pavle pinned,
                   or purge under a legal_hold (D7):
    → DO NOT act. Emit an `agent_proposed` review_item (§2.9):
        { change_type: "lifecycle_<action>", target: o.id, rationale, risk_level, sensitivity }
    → object stays in current state until Pavle approves/rejects.
    → reject  → record decision, set review_after = now + grace (re-nudge later), never re-propose silently
    → approve → execute, audit object_archived/object_purged with decided_by/decided_at
```

- The job **never** auto-applies a destructive or policy-class transition; those
  always route to review (Law 4 / §2.9). Soft archive of `standard`-retention,
  non-pinned objects is auto (low-risk, reversible).
- A **policy change** (`domain_policy` edit) does **not** trigger retro-mutation.
  A separate, explicit `altevra retention reeval --domain <d>` job (review-gated)
  re-derives defaults for existing objects; default behavior is "old objects keep
  their creation-time snapshot" (FM-DL-4).

### 6.7 Project lifecycle + cross-project compounding (anti-clutter, anti-loss)

Projects are where Pavle's work actually lives (ReVesta, Cockpit, PhoneAgent,
Tunia, …). `project` is **both** a domain value **and** a scope container
(`scope = project_id`, §1.4.8). §6 owns the project *lifecycle* and the
*compounding* mechanics that satisfy the mandate: *remember important work across
projects without becoming cluttered*.

**Project object + state machine** (mirrors the identity registry
`~/.imperium/identity/projects.yaml`; canonicity is §6.13 Q-PROJ):
```text
project status:  active → paused → archived → {completed | abandoned}
                   ▲         │
                   └─────────┘  (reopen)
```

- **Archive demotion (D5).** When a project is `archived`/`completed`, its
  scoped objects are **not** deleted and **not** individually re-statused; instead
  retrieval applies a **scope demotion** (a multiplier, request to §3 in §6.14)
  so an archived project's content stops cluttering default packets but remains
  fully retrievable with `--project <p>` or a `*_history` intent. Reopening
  restores normal ranking. *Clean by default, lossless by guarantee.*
- **Scope promotion (D8) — the compounding engine.** A `learning` / `decision` /
  `insight_card` that generalizes beyond its project is **promoted** to
  `scope=global` by minting a **new** global object + a `derived_from` edge (§1.6)
  back to the project-scoped original (which is retained). Never relabel in place.
  This is how "the ReVesta GTM lesson from Mar 2026 applies to Tunia GTM in Sept
  2027" (CLAUDE.md §3.4) physically works: the global lesson outlives and outranks
  its archived source (request to §3, §6.14).
- **Provenance compaction.** High-volume provenance (`turn`, `file_change`,
  low-importance `system_event`) is **not** purged (it feeds compounding) but is
  **compacted**: after a `session` ends and a summary/`insight_card` is created
  with `derived_from` edges to its turns, the raw turns flip to `status=archived`
  (history-queryable, excluded from default reads). Compaction = anti-clutter
  *without* provenance loss. (Resolves §1 Q6 for `turn`/`system_event`.)
- **Cross-project links** survive project archival: `relations` edges between a
  paused/archived project and a live one are retained (status `active`), only
  demoted, never retracted (D5). A "what connects to X across all projects" query
  (graph index, §3.2) still traverses them.

### 6.8 Retention / export / delete (sovereignty + RTBF + legal hold)

**Export (data sovereignty — Constitution §4.4 / `CLAUDE.md` §4.4).** Pavle must
be able to take his brain and leave.
```text
altevra export --domain <d>|--all [--format jsonl|md] [--include-superseded]
               [--include-archived] [--out <dir>] [--raw]   # --raw requires human-presence (§2.9)
  → emits one portable record per object = full §1 envelope (+ body)
  → runs exposure_gate (§2.3) per object:
       agent caller            → redacted bodies, secret HANDLES only, ceiling-clamped
       interactive Pavle (TTY) + --raw → decrypted bodies; secret VALUES still require
                                         per-secret unlock (§2.16); never bulk-dumped
  → markdown export only for obsidian_mirror ≠ never domains (D4)
  → emits audited `export_completed` event (domain, count, format, audience — never content) (§2.10)
```
`export_class`: `always` (public/business — exportable freely), `on_request`
(default — needs the command), `restricted` (relationship/health/legal — export
allowed but always `--raw`-gated + audited, never agent-initiated).

**Forget / delete (consumes §2.8 hard-delete pipeline; §6 owns the *when/which*):**
- `rtbf_required` domains (`client`, `personal`, `relationship`, `health`,
  conditionally `legal`/`financial`) **must** support true hard delete via the
  §2.8 enumerate→plan→review→purge→verify-absence pipeline.
- **Legal hold precedence (D7).** Before §2.8's review gate, the forget pipeline
  checks `legal_hold`. If set, the request is **rejected** (not queued) with a
  legal-hold reason and an audited `forget_blocked_legal_hold`; it cannot proceed
  until `altevra legal-hold release <id>` (human-presence, review-gated) clears it.
  This is the one case where RTBF yields to a stronger obligation, and it is
  explicit and audited — never silent.
- **Cascade.** Deleting a `person` (personal) cascades to its `relationship`
  edges and `mentions` edges (§1.6) → edges `retracted`, not orphaned (composition
  with §1 FM-6 / §2 I7 verify-absence). Deleting a client cascades to that
  client's PII objects under its `client_id` sub-scope.
- **Client engagement close.** When a client engagement ends, its PII does not
  silently persist forever: `client.hard_expiry=per-contract` schedules a
  RTBF-review (not auto-purge — client data may have its own legal hold).

### 6.9 Obsidian implications (zones, frontmatter, wiki hygiene)

**Two markdown worlds, one contract.** Altevra owns a machine-managed vault
(numbered zones, the V5 `/NN-*` layout); the human-canonical **Imperium Obsidian
vault** (`~/Obsidian/Imperium/`, Constitution §2/§5) is a *separate* surface that
Altevra reads (Daily/Memory) and writes *only* as `generated_mirror`. Two-vault
canonicity is §6.13 Q-VAULT. Both obey §2's authored-vs-generated zone rule.

**Altevra vault zone map** (extends §2.14 `/00-authored/**` + `/15-generated/**`;
each zone has a default authorship class (§2.6) and a domain mapping):

| Zone | Authorship | Domains mapped here | Mirror default |
|---|---|---|---|
| `/00-authored/**` | `obsidian_authored` | human notes (any domain, but `confidential+` → DB-only, D4) | n/a |
| `/06-skills/**` | `generated_mirror` | global/project (skills) | default_on |
| `/07-capabilities/**` | `generated_mirror` | global (hooks, tool matrix) | default_on |
| `/10-insights/**` | `generated_mirror` | business/project (journals, briefings) | default_on |
| `/20-wiki/**` | mixed (curated) | business/project/public | default_on |
| `/30-business/**`, `/31-projects/**` | mixed | business, project | opt_in/default_on |
| `/15-generated/**` | `generated_mirror` | any (managed header required) | n/a |
| `/90-archive/**` | retained, read-only | archived/superseded faces | — |
| **(no zone)** | **DB-only** | `personal`,`relationship`,`health`,`legal`,`financial`,`client`,`secret≥` | **never (D4)** |

- **D4 is the hard rule:** domains with `obsidian_mirror=never` get **no plaintext
  markdown face ever** (composition with §2.14 `mirror_to_markdown=false` for
  `confidential+`). Personal/health/relationship/legal/financial/client content is
  DB-only; it cannot leak via a synced vault folder.
- **Frontmatter contract** = §1.11 subset **plus** the lifecycle keys §6 adds:
  `domain`, `sensitivity`, `status`, `retention` (class), `review_after`,
  `valid_until`, `policy_version`, `legal_hold`. Round-trip identity is §1 I9.
- **Wiki hygiene (anti-clutter for curated knowledge).** Wiki pages
  (`WikiStatus: draft→living→archived`, §1.5) carry `review_after` from their
  domain policy; a stale `living` page surfaces in the digest (`due_for_review`),
  is **not** auto-edited. Superseded/archived wiki pages move to `/90-archive/**`
  (a file move, not a delete), retaining their `id`/edges. Auto-curated wiki is
  `generated_mirror`; hand-written wiki is `obsidian_authored` — a human edit to a
  generated page quarantines as drift (§2.7), never silently overwritten.

### 6.10 Cloud / local sync implications (per-domain sync map)

Local-first by axiom (Constitution §5 / `CLAUDE.md` §4.4). §6 owns the **canonical
per-domain `cloud_sync` map** that §1.12 and §2.15 defer to (the `cloud_sync`
column in §6.4):

| cloud_sync | domains | meaning |
|---|---|---|
| `disabled` | client, personal, relationship, health, legal, financial | **never leaves the machine**; no plaintext, no ciphertext, no metadata |
| `encrypted_only` | business, project | client-side encrypted before push; provider sees ciphertext + ceiling-gated metadata only (§2.15) |
| `allowed` | public | may sync (still gated per-object; a `public` object that got reclassified up is held back) |

- **Conflict handling assumption (delegated, not redefined).** Sync conflict
  resolution is §1.12 (revision + origin_device + checksum → review-on-conflict,
  not last-writer-wins) and §2.15 (tombstones sync as id+hash, never content). §6
  adds one *simplifying invariant*: **`disabled` domains are single-device-of-record
  by construction → they have no sync conflicts at all** (D3). The entire
  conflict-resolution surface is therefore confined to `encrypted_only`/`allowed`
  (business/project/public) data — the least sensitive tier — which materially
  shrinks the risk surface for the (still-undecided, §1 Q7) sync substrate.
- **Sync is opt-in per domain and changing it is review-gated** (D10): turning
  `personal.cloud_sync = encrypted_only` is a policy edit (Law 4), never an agent
  flag.
- **No object's effective `cloud_sync` may exceed its domain ceiling** (D3,
  composes with §2 I13): a `restricted`-reclassified business object is held back
  even though `business` allows `encrypted_only`.

### 6.11 CLI / MCP implications (verbs, tool surface, caller boundaries)

One core, two faces (V5 §5). Mutations to policy/holds/raw-export require an
interactive human-presence signal (§2.9/§2.16); agents get read + propose only.

**CLI:**
```bash
altevra domain list [--json]                         # all domain_policy rows
altevra domain show <domain> [--json]                # one policy + member counts
altevra domain set-policy <domain> --<field> <v>     # REVIEW-GATED (human-presence)
altevra domain create <domain> ...                   # REVIEW-GATED (governed add, §6.3)
altevra retention status [--domain <d>] [--json]     # counts by staleness state
altevra retention run [--dry-run] [--domain <d>]     # the §6.6 engine
altevra retention reeval --domain <d>                # REVIEW-GATED policy re-application
altevra project list [--status <s>] [--json]
altevra project archive <p> | reopen <p> | complete <p>
altevra promote <object_id> --to-global              # scope promotion (§6.7, mints new + edge)
altevra export --domain <d>|--all [--format jsonl|md] [--raw]   # --raw → human-presence
altevra forget <id> --dry-run | --execute            # defers to §2.8; checks legal_hold first (D7)
altevra legal-hold set <id> --reason <r> | release <id>   # REVIEW-GATED (human-presence)
```

**MCP tools (→ same core; agent-facing, read-default-safe per §1 I12 / §2 I3):**
- `get_domain_policy(domain)` — read; returns policy minus any field above caller
  ceiling.
- `list_projects(status?)` — read; scope-gated.
- `propose_domain(name, rationale)` / `propose_lifecycle_action(...)` — write a
  `review_item` only (`agent_proposed`), never apply.
- **Caller boundary (hard):** no MCP tool may `set-policy`, `create domain`,
  `legal-hold`, `export --raw`, or `forget --execute`. Those are
  human-presence-only (§2.9). An agent that needs one of them files a review item.

### 6.12 Required tests / fixtures / golden snapshots

Extends the `repository_roundtrip.rs` + golden-snapshot pattern (§1.14 / §2.17 /
§3.18). No real secrets; planted **fake** secrets reused from the §2/§3 corpus.

1. **Policy seed golden (P0.1):** migrate-from-empty seeds exactly the 9 builtin
   `domain_policy` rows; golden-snapshot the §6.4 matrix → any default drift is a
   visible review diff.
2. **Default application (D1/D2):** create an object per domain with no explicit
   safety fields → assert resolved `sensitivity`/`cloud_sync`/`obsidian_mirror`/
   `valid_until`/`review_after`/`policy_version` exactly match the domain policy.
3. **Multi-domain resolution (§6.4):** object with `domains=[business,
   relationship]` → resolves to most-restrictive (sensitivity `restricted`,
   cloud_sync `disabled`, mirror `never`).
4. **Lifecycle engine (D5/D6/D11):** fixtures aged past soft/hard TTL →
   `standard` auto-archives (still retrievable via `--include-archived`);
   `ephemeral`+non-canonical purges (verify-absence); `permanent`/`long` only
   flagged, **never** archived/purged; dry-run mutates nothing.
5. **Project archive demotion (D5):** archived-project object absent from a
   default packet, present with `--project`/`*_history`; reopen restores.
6. **Scope promotion (D8):** project `learning` → global mints a new object +
   `derived_from` edge; original retained; promoted object resolvable globally.
7. **Provenance compaction (§6.7):** session-end summary → raw turns flip
   `archived`, excluded from default read, reachable via `derived_from`.
8. **Export golden (P0.1):** `export --domain business --format jsonl` round-trips
   full envelopes; planted fake secret **absent**; `--raw` without human-presence
   → refused; agent-audience export is redacted.
9. **RTBF + legal hold (D7):** `forget` on a `client` PII object → §2.8 verify-
   absence = 0 hits; same object under `legal_hold` → `forget` **rejected** +
   `forget_blocked_legal_hold` audited; after `legal-hold release` → forget
   succeeds.
10. **Policy change safety (D10/FM-DL-4):** `set-policy` is review-gated; applying
    it bumps `policy_version` and supersedes the old policy row; **existing objects
    are unchanged** until an explicit `retention reeval`.
11. **Sync ceiling (D3):** a `restricted`-reclassified business object is excluded
    from a sync push even though `business=encrypted_only`; `disabled`-domain
    objects never enter the sync set.
12. **Cross-domain leak = 0 (composition with §3 G03):** a personal/relationship
    object never surfaces in a `business`/`project` packet via domain default.

### 6.13 Acceptance criteria (P0.0 contract / P0.1 schema)

**P0.0 (this contract) accepted when:**
- [ ] The 9 builtin domains each have a complete §6.4 policy row (every column
      specified), using the §1 six-level ladder, with `secret` reserved (no domain
      defaults to it).
- [ ] Retention classes (§6.5) + lifecycle state machine (§6.6) are stated with
      explicit auto-archive (soft, reversible) vs auto-purge (ephemeral-only, D6)
      vs review-gated (destructive/policy) paths.
- [ ] Project lifecycle, archive-demotion (D5), scope-promotion (D8), and
      provenance compaction are defined as the anti-clutter/anti-loss mechanics.
- [ ] Export, RTBF, and legal-hold contracts (§6.8) consume §2.8 without
      redefining it, and legal-hold precedence (D7) is explicit + audited.
- [ ] The per-domain `cloud_sync` map (§6.10) and Obsidian zone map (§6.9, D4) are
      the canonical values §1.12/§2.15/§2.14 defer to.
- [ ] Invariants D1–D11 are each stated with a corresponding test in §6.12.
- [ ] CLI/MCP caller boundaries (§6.11) keep policy/hold/raw-export human-presence-only.

**P0.1 (data-model landing, per build-plan P0.1) accepted when:**
- [ ] `domain_policy` table + repository land; migrate-from-empty seeds the 9
      builtins (test §6.12.1) — additive, no breaking change to live tables.
- [ ] Every durable object carries `domain` + creation-time `policy_version` +
      resolved TTL fields (`review_after`/`valid_until`); legacy rows backfilled
      (`business`/`internal` defaults, composes with §1 FM-12).
- [ ] A dry-run `lifecycle` job computes staleness without mutating (test §6.12.4).
- [ ] One **vertical loop test** passes end-to-end: *create object in domain X →
      defaults applied from policy snapshot → lifecycle dry-run flags it
      `due_for_review` after soft_ttl → `export --domain X` includes it (redacted) →
      `forget` removes it with verify-absence = 0* (covers §6.12 #2,#4,#8,#9).
- [ ] Baseline green: `cargo fmt --check && cargo test && cargo build &&
      cargo clippy --workspace -- -D warnings`.

### 6.14 Failure modes + mitigations

| # | Failure | Trigger | Mitigation |
|---|---------|---------|------------|
| FM-DL-1 | **Domain misclassification** (work tagged personal or vice-versa) | classifier/agent guesses wrong domain | default-**up** to most-restrictive on uncertainty (§2.4 rule); misclassification surfaces in digest; correction = supersession (§1.4.11), audited |
| FM-DL-2 | **Over-aggressive archiving hides active context** | a `standard` object Pavle still needs auto-archives at hard_expiry | archive is **soft + reversible** (D5); pinned objects route to review (§6.6); undo grace window; digest lists archived-today |
| FM-DL-3 | **Legal hold forgotten → object un-deletable forever** | hold set, never released | `doctor` surfaces stale holds; release is human-presence; hold carries `reason` + `set_at` for review |
| FM-DL-4 | **Policy edit retro-mutates years of rows** | someone tightens a domain default | **no retro-mutation** (D10): objects keep creation-time `policy_version`; re-application is explicit, review-gated `retention reeval` |
| FM-DL-5 | **Export exfiltration** (raw secrets / over-ceiling bulk dump) | agent or careless `--raw` | export runs `exposure_gate` (§2.3); `--raw` needs human-presence; secrets stay handles; every export audited (§2.10) |
| FM-DL-6 | **Ephemeral purge eats canonical data** | purge rule misapplied to a real object | D6 fence: purge only `ephemeral` ∧ non-canonical ∧ non-sync ∧ DB-only; everything else soft-archives |
| FM-DL-7 | **Project archive orphans cross-project edges** | archive deletes/retracts edges | edges retained + demoted, never retracted on archive (D5); nightly integrity job (composes §1 FM-6) |
| FM-DL-8 | **Multi-domain object leaks via the laxer domain** | `domains=[business,health]` exposed at business ceiling | most-restrictive resolution (§6.4) + §1 I6 + §3 domain_scope gate (G03); leak test §6.12.12 |
| FM-DL-9 | **Domain sprawl** dilutes safety boundary | agents invent domains freely | domain add is review-gated (§6.3); categories absorb fine-grained taxonomy instead |
| FM-DL-10 | **Compaction loses provenance** | turns purged instead of compacted | compaction = `archived` (retained, history-queryable) + `derived_from` edges, never delete (§6.7) |

### 6.15 Security / privacy risks (how §2 gates apply)

- **Cross-domain bleed** (personal→business packet) — *the* primary §6 risk. §6
  supplies the per-domain `domain_scope` defaults; §2's `exposure_gate` (I3) and
  §3's pre-rank sensitivity gate (G03/G11) enforce them. §6 does not enforce, it
  *defines the defaults the enforcers read*.
- **Personal data to cloud** — `cloud_sync=disabled` default for the six
  high-water domains (§6.10, D3); changing it is review-gated; §2 I13 backstops.
- **Plaintext on disk** — `obsidian_mirror=never` (D4) for high-water domains;
  §2.14 `mirror_to_markdown=false` for `confidential+` is the mechanism.
- **Export as a side channel** — audited, ceiling-gated, `--raw` human-presence
  (§6.8, FM-DL-5); composes with §2.10 audit + §2.16 secret-handle rule.
- **Legal/financial retention vs RTBF** — explicit `legal_hold` precedence (D7),
  audited both ways (`forget_blocked_legal_hold`, `legal_hold_released`); never a
  silent permanent retention.
- **Provenance compaction as a hiding place** — compacted turns are `archived`,
  not deleted, and remain subject to `forget`/RTBF (a delete cascades through the
  `derived_from` chain per §2.8 enumerate step). Compaction never escapes RTBF.
- **Policy object itself** — `domain_policy` is `internal` and edits are
  review-gated (Law 4); an agent cannot quietly loosen a ceiling to then exfiltrate
  (composes §2 I9 exposure-widening always-gated).

### 6.16 Invariants (D1–D11; enforceable by other sections/tests)

- **D1 (one primary domain):** every durable object resolves to exactly one
  primary `domain` ∈ governed set; unknown → flagged for review, defaulted up.
- **D2 (policy snapshot):** at create, an object's defaults equal its
  `domain_policy` snapshot and it records `policy_version`; policy edits do not
  retro-mutate (FM-DL-4).
- **D3 (sync ceiling):** an object's effective `cloud_sync` never exceeds its
  domain ceiling; `disabled` domains never enter any sync set (composes §2 I13).
- **D4 (no plaintext for high-water):** `obsidian_mirror=never` domains
  (personal/relationship/health/legal/financial/client + `secret≥`) get no
  markdown face (composes §2.14).
- **D5 (archive ≠ delete):** archived projects/objects are excluded from default
  reads but retained and fully retrievable; edges demoted, never retracted.
- **D6 (purge fence):** auto-hard-purge applies only to `ephemeral` ∧ non-canonical
  ∧ non-sync ∧ DB-only rows; never a canonical/sync-eligible object.
- **D7 (legal-hold precedence):** a `legal_hold` blocks hard delete even on an
  RTBF domain until released; the block and release are audited.
- **D8 (promotion is additive):** project→global compounding mints a new object +
  `derived_from` edge; never relabels scope in place (composes §1 supersession).
- **D9 (export completeness + safety):** export of a domain returns the full
  envelope of every live object; raw secret values are never exported absent
  per-secret interactive unlock.
- **D10 (policy/domain change is review-gated):** creating/altering a domain or
  its policy, and changing per-domain sync, are review-gated, versioned, audited
  (composes §2 I9).
- **D11 (TTL never silently destroys):** retention TTLs only *flag* or *soft-archive*
  canonical objects; the sole auto-hard-delete path is D6 ephemeral purge.

### 6.17 Unresolved questions (owner / recommended default)

- **Q-LADDER — sensitivity ladder reconciliation.** §1.4.7 defines a **6-level**
  ladder (`public < shareable < internal < confidential < secret < restricted`);
  §2.4 uses a **5-level** lattice (drops `shareable`). §6.4 uses the §1 6-level
  ladder. *Owner: object-model + safety + Hermes. Recommended default: adopt §1's
  6-level ladder everywhere; `shareable` = "fine for external agents/clients",
  distinct from `public` (publishable). Safety §2 should add `shareable` between
  `public` and `internal`.*
- **Q-VAULT — two-vault canonicity.** Does Altevra keep its own machine vault
  (`~/.altevra/vault`, numbered zones) separate from the human Imperium vault
  (`~/Obsidian/Imperium/`), mirroring curated/human content into Imperium's
  Daily/Memory as `generated_mirror`? *Owner: Pavle/Hermes. Recommended default:
  yes — Altevra vault is machine-managed; Imperium stays human-canonical; Altevra
  writes Imperium only as generated_mirror, never authoritative.*
- **Q-PROJ — project object canonicity.** Is the `project` durable object sourced
  from the identity registry (`~/.imperium/identity/projects.yaml`) or native to
  the brain? *Owner: object-model + Hermes. Recommended default: identity registry
  is canonical; brain mirrors projects as `imported_readonly` `project` objects
  carrying live `status`.*
- **Q-FIN — financial/legal statutory retention.** The 7y `financial.hard_expiry`
  and any `legal` retention depend on jurisdiction (Serbia personal vs Wyoming
  LLC). *Owner: Pavle. Recommended default: 7y→review (no auto-purge), confirm
  per-jurisdiction later.*
- **Q-EPH — ephemeral purge horizons.** Concrete TTLs for compaction/purge.
  *Owner: §6 + Hermes. Recommended default: session turns compacted on summary then
  `archived`; low-importance `system_event` purge at 90d; dismissed `research_item`
  purge at 30d; expired `context_packet` purge at 14d.*
- **Q-HOLD — legal-hold authority.** Who may set/release a hold? *Owner: §2/Hermes.
  Recommended default: human-presence (Pavle) only; agents may only propose.*

### 6.18 Cross-section requests

- **→ §1 object-model (opus-object-model):** (a) add `domain_policy` to the
  governed `type` registry (§1.2) with the §6.2 envelope; (b) add a per-object
  `policy_version` (int) and `legal_hold` (bool) field to the envelope (or a
  governed metadata convention) so D2/D7 are schema-enforced; (c) confirm the
  `domains[]` multi-value (§1.4.8) is official and that I6 domain-union uses
  **most-restrictive** resolution for *policy* fields (sensitivity/sync/mirror/TTL),
  matching §6.4; (d) confirm a `project` object type + scope hierarchy
  (project→global) for D8 promotion; (e) **ratify the §1 six-level sensitivity
  ladder as canonical** (Q-LADDER).
- **→ §2 safety-source-truth (opus-safety-source-truth):** (a) here is the
  **domain→default-sensitivity** map and **domain→cloud_sync** ceiling map you
  requested in §2.20 — please consume §6.4 as the authoritative defaults that
  `exposure_policy` falls back to; (b) please integrate `legal_hold` as a
  delete-blocker checked **before** the §2.8 hard-delete review gate (D7); (c)
  `export --raw` must require the **same human-presence signal** as a protected
  approval (§2.9); (d) confirm the §6.9 vault zone map extends §2.14 cleanly; (e)
  add `shareable` to the §2.4 lattice (Q-LADDER).
- **→ §3 context-retrieval (opus-context-retrieval):** (a) here are the per-domain
  **soft_ttl** and **hard_expiry** values (§6.4) feeding your §3.9 staleness gate
  and recency half-lives; (b) please add an **archived-project scope-demotion
  multiplier** (D5) so archived-project content is demoted, not gated out; (c) a
  **scope-promoted global object should outrank its archived project original**
  (D8) — model via the `derived_from` edge + scope multiplier.
- **→ §4 agents-self-improve (claude-agents-self-improve):** (a) the `lifecycle`
  job's `due_for_review`/archive/flag outputs and domain-misclassification
  corrections are observer signals; (b) new-**category** proposals auto-apply
  (low-risk, §1.4.9), new-**domain** proposals are review-gated (§6.3, D10); (c)
  all lifecycle destructive/policy actions route through review (§6.6), never
  direct writes.
- **→ §5 tools-skills-interfaces (claude-tools-skills-interfaces):** (a) align the
  `domain`/`project`/`retention`/`export`/`forget`/`legal-hold` CLI verbs + MCP
  tool names (§6.11) so they don't collide with the tool/skill registry surface;
  (b) confirm `skill`/`hook`/`tool_installation` objects are `global`/`project`
  scoped and obey retention: a **deprecated skill is archived, not deleted** (D5),
  and its usage history is retained for the skill-factory loop.
- **→ §7 hermes-synthesis (hermes):** ratify (a) the §6.4 per-domain policy matrix
  as the canonical default map cross-cutting §1/§2/§3; (b) resolve Q-VAULT
  (two-vault canonicity) and Q-PROJ (project object source of truth); (c) own the
  Q-FIN jurisdictional retention default; (d) confirm `legal_hold` authority
  (Q-HOLD) and the human-presence requirement for policy/hold/raw-export.

### 6.19 Summary of this section's changes

This section specifies **Domains + Lifecycle** as the policy-and-retention law
that makes Law 6 ("business and personal first-class but bounded") operational
and keeps a decades-long brain compounding without clutter. It defines: the
`domain_policy` **durable object** (consuming the §1 envelope, review-gated to
edit) and the governed-domains-vs-living-categories split; the **canonical
per-domain policy matrix** (default sensitivity, audience, cloud-sync ceiling,
embedding role, Obsidian mirror, retention class, soft/hard TTLs, RTBF,
legal-hold, export class) that fulfils §2.20's and §3.21's explicit requests;
**retention classes** + an idempotent, dry-runnable **lifecycle engine** with
distinct happy-path (soft auto-archive, ephemeral-only auto-purge) and
review/rejection paths (all destructive/policy transitions gated); **project
lifecycle** with archive-demotion (D5), **cross-project scope promotion** (D8),
and **provenance compaction** as the anti-clutter/anti-loss compounding mechanics;
**export** (sovereignty), **forget/RTBF** (consuming §2.8), and **legal-hold
precedence** (D7); a per-domain **cloud-sync map** that confines all sync-conflict
surface to the least-sensitive tier; an **Obsidian zone map** with the hard "no
plaintext for high-water domains" rule (D4) and wiki-hygiene rules; CLI/MCP verbs
with human-presence caller boundaries; 11 invariants (D1–D11) each with a test; a
test/fixture/golden suite including a single **vertical-loop** P0.1 test;
P0.0/P0.1 acceptance criteria; 10 failure modes; security/privacy risks mapped to
§2 gates; six unresolved questions with owners and recommended defaults; and
cross-section requests to object-model, safety, context-retrieval,
agents-self-improve, tools-skills-interfaces, and Hermes.

<!-- END_SECTION: domains-lifecycle -->

---

<!-- SECTION: hermes-synthesis -->
<!-- OWNER: hermes -->
<!-- STATUS: synthesized-by-hermes -->
## 7. Hermes Synthesis

### 7.1 Final architecture decisions

1. **P0 storage truth:** Altevra P0 is SQLite/local-first. Obsidian is the human face; generated markdown mirrors must be reconciled through DB-backed contracts. Postgres/pgvector/cloud backends are future adapters.
2. **Durable object law:** every persisted thing uses the §1 envelope: stable id, type, schema version, status family, timestamps, provenance, sensitivity, domain/scope, tags, confidence/staleness where relevant, and relations.
3. **Capture != exposure:** broad capture is allowed only if `PreWriteSafetyGate` runs before persistence and `ExposureGate` runs before every retrieval/tool/MCP/context output.
4. **Context packets are the product primitive:** agents and humans consume scoped packets with provenance/explanations, not raw database dumps.
5. **Self-improvement is proposal-first:** Altevra may detect schema/prompt/tool/wiki/context weaknesses and create meta-proposals; protected changes require review.
6. **Tools/skills are registry objects:** discovery, health, permissions, versions, drift, contracts, and provenance are durable but auto-execution/auto-install is review-gated.
7. **Domains are explicit and enforced:** business, personal, project, client, relationship, health, legal, financial, public/shareable, system, and agent domains carry default sensitivity, retention, sync ceilings, and review rules.
8. **Anti-clutter law:** generated/captured noise is hidden/summarized by default; dashboards show focus/daily/actionable packets only.

### 7.2 Cross-section contract map

- §1 owns the common envelope and relation model.
- §2 owns safety/source-of-truth gates, redaction, protected review, deletion/forgetting.
- §3 owns retrieval/indexing/context packet compilation and packet audits.
- §4 owns resident agent prompts, prompt registry, meta-proposals, self-improvement review loop.
- §5 owns capability/tool/skill registry, CLI/MCP interface contracts, renderer/tool output safety.
- §6 owns domain registry, retention/sync ceilings, Obsidian zones, lifecycle engine.
- §7 owns conflict resolution and P0 cut line.

### 7.3 P0.0 required contract artifacts

1. `contracts/P0_CONTRACTS.md` — canonical decisions, enums, gates, state machines.
2. `contracts/P0_ACCEPTANCE_TESTS.md` — vertical-loop fixtures/evals.
3. `contracts/P0_IMPLEMENTATION_PLAN.md` — minimal implementation order.
4. `ALTEVRA_ARCHITECTURE_REVIEW_LOG.md` — breaker feedback and Hermes resolution.

### 7.4 P0.1 vertical loop

Implement only this first:

1. Create one synthetic project/business object and one synthetic personal/sensitive object.
2. Run `PreWriteSafetyGate` before insert.
3. Persist normalized objects in SQLite.
4. Render allowed Obsidian markdown mirrors with frontmatter.
5. Compile a scoped context packet for a project task.
6. Verify sensitive object is excluded with non-leaking explanation.
7. Write packet audit record.
8. Create one self-improvement proposal from a fixture failure, but do not auto-apply.

### 7.5 Hard deferrals

- No dashboard build.
- No external connectors/research ingestion.
- No production cloud sync.
- No auto-modifying broad skills/prompts/policies.
- No vector backend dependency before deterministic packet evals pass.

### 7.6 Acceptance gate for architecture phase

Architecture is considered locally ready for P0.0 implementation when:

- all six deep sections are drafted;
- review log contains breaker feedback or an explicit blocker/fallback note;
- Hermes resolution exists;
- contract files exist;
- marker/status validation passes;
- P0.1 vertical loop is testable without real secrets/customer data.

<!-- END_SECTION: hermes-synthesis -->
