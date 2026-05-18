-- Enforce uniqueness of root-level keyword names. Migration 0005's
-- `UNIQUE (parent_id, name)` doesn't catch duplicates when parent_id IS
-- NULL because SQLite treats every NULL as distinct in UNIQUE indexes.
--
-- Strategy: dedupe any duplicate root keywords (merging their
-- photo_keywords attachments to a canonical survivor), then add a
-- partial unique index that enforces uniqueness for the NULL-parent
-- subset going forward.

-- Pick the surviving id (alphabetically smallest) for each duplicated
-- root keyword name.
CREATE TEMP TABLE _keyword_dedup AS
SELECT name, MIN(id) AS canonical_id
FROM keywords
WHERE parent_id IS NULL
GROUP BY name
HAVING COUNT(*) > 1;

-- Repoint photo_keywords from non-canonical duplicates to the canonical
-- id. Skip pairs where the canonical attachment already exists (would
-- collide with the existing (photo_id, keyword_id) PRIMARY KEY).
UPDATE photo_keywords
SET keyword_id = (
    SELECT d.canonical_id
    FROM _keyword_dedup d
    JOIN keywords k ON k.name = d.name AND k.id = photo_keywords.keyword_id
    WHERE k.parent_id IS NULL
)
WHERE keyword_id IN (
    SELECT k.id
    FROM keywords k
    JOIN _keyword_dedup d ON k.name = d.name AND k.id != d.canonical_id
    WHERE k.parent_id IS NULL
)
AND NOT EXISTS (
    SELECT 1
    FROM photo_keywords existing
    JOIN _keyword_dedup d2 ON d2.canonical_id = existing.keyword_id
    JOIN keywords k2 ON k2.id = photo_keywords.keyword_id
    WHERE existing.photo_id = photo_keywords.photo_id
      AND d2.name = k2.name
);

-- Drop any photo_keywords rows that couldn't be repointed (canonical
-- attachment already existed).
DELETE FROM photo_keywords
WHERE keyword_id IN (
    SELECT k.id
    FROM keywords k
    JOIN _keyword_dedup d ON k.name = d.name AND k.id != d.canonical_id
    WHERE k.parent_id IS NULL
);

-- Delete the duplicate keyword rows themselves.
DELETE FROM keywords
WHERE parent_id IS NULL
  AND id IN (
    SELECT k.id
    FROM keywords k
    JOIN _keyword_dedup d ON k.name = d.name AND k.id != d.canonical_id
);

DROP TABLE _keyword_dedup;

-- Now enforce root-level uniqueness for future writes.
CREATE UNIQUE INDEX keywords_unique_root_idx
    ON keywords(name)
    WHERE parent_id IS NULL;
