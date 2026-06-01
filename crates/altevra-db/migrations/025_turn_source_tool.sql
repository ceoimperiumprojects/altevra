-- P0.1 (tool attribution): sign every turn with the agent/tool that produced it.
-- Sessions already carry `tool`, but for mixed-source histories (Hermes imports,
-- multi-tool sessions) each turn needs its OWN signature so "who wrote this" is
-- answerable per-row, not just per-session. `source_tool` = claude-code | codex |
-- cursor | antigravity | hermes | ... ; populated by hook-handle from --tool.
ALTER TABLE turns ADD COLUMN source_tool TEXT;
CREATE INDEX IF NOT EXISTS idx_turns_source_tool ON turns(source_tool);
