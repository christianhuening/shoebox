-- Virtual buckets. Members are variants (not photos) so master and
-- virtual copies can be collected separately.

CREATE TABLE collections (
    id         TEXT PRIMARY KEY,
    parent_id  TEXT REFERENCES collections(id),
    name       TEXT NOT NULL,
    created_by TEXT NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL
);

CREATE INDEX collections_parent_idx ON collections(parent_id);

CREATE TABLE collection_members (
    collection_id TEXT NOT NULL REFERENCES collections(id),
    variant_id    TEXT NOT NULL REFERENCES variants(id),
    added_by      TEXT NOT NULL REFERENCES users(id),
    added_at      INTEGER NOT NULL,
    sort_order    INTEGER NOT NULL,
    PRIMARY KEY (collection_id, variant_id)
);

CREATE INDEX collection_members_variant_idx ON collection_members(variant_id);
