-- P0.5 resident runtime (R10: extend brain_jobs — one history table; R14/MOD-2:
-- small single-purpose resident modes, never a monolith). No LLM keys needed:
-- every role resolves to the noop provider until keys are added.

-- resident_run columns on the existing brain_jobs history (additive, R10).
ALTER TABLE brain_jobs ADD COLUMN resident_mode TEXT;
ALTER TABLE brain_jobs ADD COLUMN model_role TEXT;
ALTER TABLE brain_jobs ADD COLUMN provider TEXT;
ALTER TABLE brain_jobs ADD COLUMN input_packet_id TEXT;
ALTER TABLE brain_jobs ADD COLUMN output_json TEXT;
ALTER TABLE brain_jobs ADD COLUMN proposals_emitted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE brain_jobs ADD COLUMN dry_run INTEGER NOT NULL DEFAULT 1;

-- resident_mode registry: each mode is a small agent with ONE job, a role (never
-- a concrete model), a sensitivity ceiling, and a personal-data flag (SI-7).
CREATE TABLE IF NOT EXISTS resident_modes (
    name                  TEXT PRIMARY KEY,
    description           TEXT NOT NULL,
    model_role            TEXT NOT NULL,           -- cheap_worker|strong_reasoner|local_private|embedding|reranker|none
    sensitivity_ceiling   TEXT NOT NULL DEFAULT 'internal',
    personal_data_allowed INTEGER NOT NULL DEFAULT 0,
    prompt_ref            TEXT,
    enabled               INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- resident_budget: runaway caps per mode (the P0.6 firewall reads these).
CREATE TABLE IF NOT EXISTS resident_budgets (
    mode                  TEXT PRIMARY KEY,
    max_runs_per_day      INTEGER NOT NULL DEFAULT 24,
    max_tokens_per_run    INTEGER NOT NULL DEFAULT 8000,
    max_proposals_per_run INTEGER NOT NULL DEFAULT 10,
    cooldown_secs         INTEGER NOT NULL DEFAULT 300
);

-- Seed the 8 builtin small modes (MOD-2). personal_curator is local-only (SI-7).
INSERT OR IGNORE INTO resident_modes (name, description, model_role, sensitivity_ceiling, personal_data_allowed) VALUES
  ('memory_curator',          'Decide what to save or update from a captured session',     'cheap_worker',    'internal',   0),
  ('synthesis',               'Distill raw notes into a concise, sourced summary',         'strong_reasoner', 'internal',   0),
  ('wiki_curator',            'Maintain living wiki pages from new evidence',              'cheap_worker',    'internal',   0),
  ('daily_briefing',          'Compose the daily brief from the day''s activity',          'cheap_worker',    'internal',   0),
  ('insight',                 'Detect patterns/correlations across recent activity',       'strong_reasoner', 'internal',   0),
  ('observer',                'Self-improvement: notice low-quality resident outputs',     'cheap_worker',    'internal',   0),
  ('personal_curator',        'Curate personal/relationship/health memory (LOCAL only)',   'local_private',   'restricted', 1),
  ('skill_factory_proposer',  'Propose new skills from repeated tool-call workflows',      'strong_reasoner', 'internal',   0);

INSERT OR IGNORE INTO resident_budgets (mode) SELECT name FROM resident_modes;
