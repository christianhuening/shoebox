-- Master + virtual copies, and pessimistic develop locks.

CREATE TABLE variants (
    id                       TEXT PRIMARY KEY,
    photo_id                 TEXT NOT NULL REFERENCES photos(id),
    variant_index            INTEGER NOT NULL,
    name                     TEXT,
    created_by               TEXT NOT NULL REFERENCES users(id),
    created_at               INTEGER NOT NULL,
    develop_settings_json    TEXT NOT NULL,
    develop_settings_version INTEGER NOT NULL,
    develop_updated_at       INTEGER NOT NULL,
    develop_updated_by       TEXT NOT NULL REFERENCES users(id),
    UNIQUE (photo_id, variant_index)
);

CREATE INDEX variants_photo_idx   ON variants(photo_id);
CREATE INDEX variants_created_idx ON variants(created_by);

CREATE TABLE develop_locks (
    variant_id            TEXT PRIMARY KEY REFERENCES variants(id),
    session_id            TEXT NOT NULL REFERENCES sessions(id),
    user_id               TEXT NOT NULL REFERENCES users(id),
    acquired_at           INTEGER NOT NULL,
    expires_at            INTEGER NOT NULL,
    takeover_requested_by TEXT REFERENCES users(id),
    takeover_requested_at INTEGER
);

CREATE INDEX develop_locks_expires_idx ON develop_locks(expires_at);
