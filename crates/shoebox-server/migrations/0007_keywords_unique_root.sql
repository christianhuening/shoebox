-- Enforce uniqueness of root-level keyword names. Migration 0005's
-- `UNIQUE (parent_id, name)` doesn't catch duplicates when parent_id IS
-- NULL because SQLite treats every NULL as distinct in UNIQUE indexes.
-- This partial index fills the gap for the NULL-parent subset.

CREATE UNIQUE INDEX keywords_unique_root_idx
    ON keywords(name)
    WHERE parent_id IS NULL;
