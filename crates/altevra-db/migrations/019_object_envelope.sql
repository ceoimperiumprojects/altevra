-- P0.1 (BUILD_TASKS T1.1, RECONCILIATION R8): additive object-envelope columns.
-- Brings live durable tables up to the §1.3 mandatory envelope WITHOUT rewriting
-- them. SQLite ADD COLUMN with constant DEFAULT is cheap + non-breaking; legacy
-- rows backfill to safe defaults (business / internal / schema_version 1 /
-- provenance.origin=imported / revision 1). No table is dropped or recreated.
--
-- Envelope fields: schema_version, domain, scope, provenance(JSON), revision,
-- tags(JSON), categories(JSON), risk_tags(JSON), confidence, sensitivity(where
-- missing), updated_at(where missing), supersedes/superseded_by,
-- valid_until, review_after, origin_device, policy_version.
-- (id/type/status/created_at/checksum already exist where relevant.)

-- ---- decisions ----
ALTER TABLE decisions ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE decisions ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE decisions ADD COLUMN updated_at TEXT;
ALTER TABLE decisions ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE decisions ADD COLUMN scope TEXT;
ALTER TABLE decisions ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'internal';
ALTER TABLE decisions ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"imported"}';
ALTER TABLE decisions ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE decisions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN categories TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN risk_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN confidence TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE decisions ADD COLUMN supersedes TEXT;
ALTER TABLE decisions ADD COLUMN superseded_by TEXT;
ALTER TABLE decisions ADD COLUMN valid_until TEXT;
ALTER TABLE decisions ADD COLUMN review_after TEXT;
ALTER TABLE decisions ADD COLUMN origin_device TEXT;
ALTER TABLE decisions ADD COLUMN policy_version INTEGER;

-- ---- tasks ----
ALTER TABLE tasks ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tasks ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE tasks ADD COLUMN scope TEXT;
ALTER TABLE tasks ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'internal';
ALTER TABLE tasks ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"imported"}';
ALTER TABLE tasks ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tasks ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE tasks ADD COLUMN categories TEXT NOT NULL DEFAULT '[]';
ALTER TABLE tasks ADD COLUMN risk_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE tasks ADD COLUMN confidence TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE tasks ADD COLUMN supersedes TEXT;
ALTER TABLE tasks ADD COLUMN superseded_by TEXT;
ALTER TABLE tasks ADD COLUMN valid_until TEXT;
ALTER TABLE tasks ADD COLUMN review_after TEXT;
ALTER TABLE tasks ADD COLUMN origin_device TEXT;
ALTER TABLE tasks ADD COLUMN policy_version INTEGER;

-- ---- goals ----
ALTER TABLE goals ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE goals ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE goals ADD COLUMN scope TEXT;
ALTER TABLE goals ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'internal';
ALTER TABLE goals ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"imported"}';
ALTER TABLE goals ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE goals ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE goals ADD COLUMN categories TEXT NOT NULL DEFAULT '[]';
ALTER TABLE goals ADD COLUMN risk_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE goals ADD COLUMN confidence TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE goals ADD COLUMN supersedes TEXT;
ALTER TABLE goals ADD COLUMN superseded_by TEXT;
ALTER TABLE goals ADD COLUMN valid_until TEXT;
ALTER TABLE goals ADD COLUMN review_after TEXT;
ALTER TABLE goals ADD COLUMN origin_device TEXT;
ALTER TABLE goals ADD COLUMN policy_version INTEGER;

-- ---- memory_documents ----
ALTER TABLE memory_documents ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE memory_documents ADD COLUMN type TEXT NOT NULL DEFAULT 'memory_document';
ALTER TABLE memory_documents ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE memory_documents ADD COLUMN created_at TEXT;
ALTER TABLE memory_documents ADD COLUMN updated_at TEXT;
ALTER TABLE memory_documents ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE memory_documents ADD COLUMN scope TEXT;
ALTER TABLE memory_documents ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'internal';
ALTER TABLE memory_documents ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"imported"}';
ALTER TABLE memory_documents ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE memory_documents ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE memory_documents ADD COLUMN categories TEXT NOT NULL DEFAULT '[]';
ALTER TABLE memory_documents ADD COLUMN risk_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE memory_documents ADD COLUMN confidence TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE memory_documents ADD COLUMN redaction_status TEXT NOT NULL DEFAULT 'unscanned';
ALTER TABLE memory_documents ADD COLUMN supersedes TEXT;
ALTER TABLE memory_documents ADD COLUMN superseded_by TEXT;
ALTER TABLE memory_documents ADD COLUMN valid_until TEXT;
ALTER TABLE memory_documents ADD COLUMN review_after TEXT;
ALTER TABLE memory_documents ADD COLUMN origin_device TEXT;
ALTER TABLE memory_documents ADD COLUMN policy_version INTEGER;

-- ---- wiki_pages (already has status/confidence/sensitivity/checksum) ----
ALTER TABLE wiki_pages ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wiki_pages ADD COLUMN type TEXT NOT NULL DEFAULT 'wiki_page';
ALTER TABLE wiki_pages ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE wiki_pages ADD COLUMN scope TEXT;
ALTER TABLE wiki_pages ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"imported"}';
ALTER TABLE wiki_pages ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE wiki_pages ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE wiki_pages ADD COLUMN categories TEXT NOT NULL DEFAULT '[]';
ALTER TABLE wiki_pages ADD COLUMN risk_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE wiki_pages ADD COLUMN supersedes TEXT;
ALTER TABLE wiki_pages ADD COLUMN superseded_by TEXT;
ALTER TABLE wiki_pages ADD COLUMN valid_until TEXT;
ALTER TABLE wiki_pages ADD COLUMN review_after TEXT;
ALTER TABLE wiki_pages ADD COLUMN origin_device TEXT;
ALTER TABLE wiki_pages ADD COLUMN policy_version INTEGER;

-- ---- review_items ----
ALTER TABLE review_items ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE review_items ADD COLUMN type TEXT NOT NULL DEFAULT 'review_item';
ALTER TABLE review_items ADD COLUMN updated_at TEXT;
ALTER TABLE review_items ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE review_items ADD COLUMN scope TEXT;
ALTER TABLE review_items ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'internal';
ALTER TABLE review_items ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"system_derived"}';
ALTER TABLE review_items ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE review_items ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE review_items ADD COLUMN categories TEXT NOT NULL DEFAULT '["review"]';
ALTER TABLE review_items ADD COLUMN risk_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE review_items ADD COLUMN risk_tier INTEGER;
ALTER TABLE review_items ADD COLUMN proposed_diff TEXT;
ALTER TABLE review_items ADD COLUMN rationale TEXT;
ALTER TABLE review_items ADD COLUMN required_approver TEXT;
ALTER TABLE review_items ADD COLUMN decision TEXT;
ALTER TABLE review_items ADD COLUMN decided_by TEXT;
ALTER TABLE review_items ADD COLUMN decided_at TEXT;
ALTER TABLE review_items ADD COLUMN applied_at TEXT;

-- ---- events (already has sensitivity/status/created_at) ----
ALTER TABLE events ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE events ADD COLUMN domain TEXT NOT NULL DEFAULT 'business';
ALTER TABLE events ADD COLUMN scope TEXT;
ALTER TABLE events ADD COLUMN provenance TEXT NOT NULL DEFAULT '{"origin":"system_derived"}';
ALTER TABLE events ADD COLUMN categories TEXT NOT NULL DEFAULT '["event"]';

CREATE INDEX IF NOT EXISTS idx_decisions_status ON decisions(status);
CREATE INDEX IF NOT EXISTS idx_decisions_domain ON decisions(domain);
CREATE INDEX IF NOT EXISTS idx_memory_documents_domain ON memory_documents(domain);
CREATE INDEX IF NOT EXISTS idx_wiki_pages_domain ON wiki_pages(domain);
