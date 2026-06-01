-- P0.4 / R12 (T1.14b): FTS5 full-text substrate over title+body+tags. This is the
-- PRIMARY lexical retrieval (BM25), NO vectors — the tag-first + FTS5 + graph stack
-- R12 mandates. `unicode61` tokenizer handles SR + EN (diacritic-folding) without a
-- model. Maintained alongside object_index on every durable write.
CREATE VIRTUAL TABLE IF NOT EXISTS object_fts USING fts5(
    object_type,
    object_id,
    title,
    body,
    tags,
    tokenize = 'unicode61 remove_diacritics 2'
);
