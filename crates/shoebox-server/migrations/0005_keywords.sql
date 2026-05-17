-- Hierarchical keywords (catalog-shared) and their attachment to photos.

CREATE TABLE keywords (
    id         TEXT PRIMARY KEY,
    parent_id  TEXT REFERENCES keywords(id),
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (parent_id, name)
);

CREATE INDEX keywords_parent_idx ON keywords(parent_id);

CREATE TABLE photo_keywords (
    photo_id   TEXT NOT NULL REFERENCES photos(id),
    keyword_id TEXT NOT NULL REFERENCES keywords(id),
    added_by   TEXT NOT NULL REFERENCES users(id),
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (photo_id, keyword_id)
);

CREATE INDEX photo_keywords_keyword_idx ON photo_keywords(keyword_id);
