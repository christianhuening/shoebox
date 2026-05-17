-- crates/shoebox-server/migrations/0001_identity.sql
-- Identity, config, sessions, and certificate revocation.

CREATE TABLE config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE users (
    id           TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    avatar_blob  BLOB,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER
);

CREATE TABLE sessions (
    id                TEXT PRIMARY KEY,
    user_id           TEXT NOT NULL REFERENCES users(id),
    client_machine_id TEXT NOT NULL,
    established_at    INTEGER NOT NULL,
    last_active_at    INTEGER NOT NULL
);

CREATE INDEX sessions_user_idx ON sessions(user_id);

CREATE TABLE revoked_certs (
    serial_number TEXT PRIMARY KEY,
    revoked_at    INTEGER NOT NULL,
    reason        TEXT,
    revoked_by    TEXT REFERENCES users(id)
);
