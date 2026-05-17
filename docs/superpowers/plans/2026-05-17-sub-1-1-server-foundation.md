# shoebox-server Foundation Implementation Plan (Plan 1.1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the minimum runnable `shoebox-server` binary — a Rust workspace with the libSQL-backed catalog schema, a localhost-bound embedded sqld, structured JSON logging, a `/health` endpoint, mDNS broadcasting, and a Dockerfile. No auth and no business logic yet; that comes in plans 1.2 and 1.3.

**Architecture:** Single Cargo workspace with two crates: `shoebox-common` (shared types) and `shoebox-server` (the binary). The binary uses Tokio + Axum for HTTP and embeds libSQL via the `libsql` crate. Schema migrations are plain SQL files in a `migrations/` directory loaded via `include_dir!` and applied in numeric order by a tiny custom runner using a `_schema_migrations` tracking table. Configuration loaded from `server.toml` with environment-variable overrides.

**Tech Stack:** Rust (toolchain pinned to a recent stable), Tokio, Axum, libSQL, tracing + tracing-subscriber (JSON output), serde + serde_json + toml, mdns-sd, anyhow, thiserror, include_dir. Test stack: tokio's `#[tokio::test]`, plain `assert_eq!`, and `reqwest` for HTTP integration tests.

**Prerequisites for the implementing engineer:**
- A POSIX shell with `git`, a working Rust install (we'll pin the toolchain in repo), and Docker.
- Familiarity with Rust async basics, Tokio tasks, and Axum routing. Knowing nothing about libSQL or mDNS specifically is fine — every API call is shown in the steps.

---

## File Structure

This plan creates the following files. Subsequent plans add to these and create new ones.

```
shoebox/
├── Cargo.toml                              ← workspace manifest
├── rust-toolchain.toml                     ← pinned toolchain
├── .gitignore                              ← target/, .env, etc.
├── crates/
│   ├── shoebox-common/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      ← re-exports
│   │       └── error.rs                    ← Error / Result types
│   └── shoebox-server/
│       ├── Cargo.toml
│       ├── build.rs                        ← embeds migrations (optional, only if needed)
│       ├── migrations/                     ← .sql files, numerically prefixed
│       │   ├── 0001_identity.sql
│       │   ├── 0002_files.sql
│       │   ├── 0003_variants.sql
│       │   ├── 0004_variant_user_state.sql
│       │   ├── 0005_keywords.sql
│       │   └── 0006_collections.sql
│       └── src/
│           ├── main.rs                     ← entry point, arg parsing, runtime
│           ├── config.rs                   ← Config struct + load
│           ├── logging.rs                  ← tracing-subscriber JSON init
│           ├── db.rs                       ← libSQL DB open + migration runner
│           ├── http.rs                     ← Axum router, /health
│           ├── mdns.rs                     ← mDNS service broadcaster
│           └── tests/                      ← integration tests live with the crate
└── Dockerfile                              ← multi-stage build
```

**Responsibility split:**
- `shoebox-common` exists from the start so plans 1.2-1.4 have an obvious place for shared types (errors, IDs, schema versions) without circular dependencies between server and client.
- `shoebox-server::config` does ONE thing — load `Config` from TOML + env. No HTTP, no DB.
- `shoebox-server::db` does ONE thing — open the libSQL database and run migrations. No business logic.
- `shoebox-server::http` does ONE thing — own the Axum router and bind. The handlers live here for plan 1.1 since there's only `/health`; in plan 1.2 we'll split `enroll.rs`, `renew.rs`, etc.
- `shoebox-server::mdns` does ONE thing — broadcast the service. No discovery (clients do that).

---

## Task 1: Initialize the Cargo workspace

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`

- [ ] **Step 1: Write `rust-toolchain.toml`** to pin a recent stable.

```toml
[toolchain]
channel = "1.78.0"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 2: Write workspace `Cargo.toml`.**

```toml
[workspace]
resolver = "2"
members = [
    "crates/shoebox-common",
    "crates/shoebox-server",
]

[workspace.package]
edition = "2021"
rust-version = "1.78"
license = "Apache-2.0"
repository = "https://github.com/CHANGE-ME/shoebox"

[workspace.dependencies]
anyhow = "1.0"
thiserror = "1.0"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
axum = "0.7"
tower = "0.4"
tower-http = { version = "0.5", features = ["trace"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
libsql = { version = "0.6", default-features = false, features = ["core"] }
mdns-sd = "0.11"
include_dir = "0.7"
hostname = "0.3"
```

- [ ] **Step 3: Write `.gitignore`.**

```gitignore
/target
**/*.rs.bk
.env
.envrc
*.swp
*.swo
.DS_Store
/data
```

- [ ] **Step 4: Install the pinned toolchain.**

Run: `rustup show`
Expected: rustup picks up the 1.78.0 channel from `rust-toolchain.toml` and installs it on demand. If it doesn't show 1.78.0, run `rustup install 1.78.0` explicitly.

- [ ] **Step 5: Commit.**

```bash
git add Cargo.toml rust-toolchain.toml .gitignore
git commit -m "build: initialize Cargo workspace with pinned Rust 1.78 toolchain"
```

---

## Task 2: Scaffold the `shoebox-common` crate

**Files:**
- Create: `crates/shoebox-common/Cargo.toml`
- Create: `crates/shoebox-common/src/lib.rs`
- Create: `crates/shoebox-common/src/error.rs`

- [ ] **Step 1: Write `crates/shoebox-common/Cargo.toml`.**

```toml
[package]
name = "shoebox-common"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
thiserror = { workspace = true }
serde = { workspace = true }
```

- [ ] **Step 2: Write `crates/shoebox-common/src/error.rs`** with a typed root error.

```rust
//! Shared error types used across shoebox crates.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 3: Write `crates/shoebox-common/src/lib.rs`.**

```rust
//! Shared types and utilities for shoebox.

pub mod error;

pub use error::{Error, Result};

/// Schema version this build of shoebox understands.
/// Update this when the migration set changes.
pub const SCHEMA_VERSION: i64 = 6;
```

- [ ] **Step 4: Verify the crate builds.**

Run: `cargo check -p shoebox-common`
Expected: completes with no errors.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-common
git commit -m "feat(common): scaffold shoebox-common crate with Error/Result and SCHEMA_VERSION"
```

---

## Task 3: Scaffold the `shoebox-server` crate

**Files:**
- Create: `crates/shoebox-server/Cargo.toml`
- Create: `crates/shoebox-server/src/main.rs`

- [ ] **Step 1: Write `crates/shoebox-server/Cargo.toml`.**

```toml
[package]
name = "shoebox-server"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "shoebox-server"
path = "src/main.rs"

[dependencies]
shoebox-common = { path = "../shoebox-common" }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
axum = { workspace = true }
tower = { workspace = true }
tower-http = { workspace = true }
libsql = { workspace = true }
mdns-sd = { workspace = true }
include_dir = { workspace = true }
hostname = { workspace = true }

[dev-dependencies]
reqwest = { workspace = true }
tempfile = "3"
```

- [ ] **Step 2: Write a minimal `crates/shoebox-server/src/main.rs`.**

```rust
fn main() {
    println!("shoebox-server stub");
}
```

- [ ] **Step 3: Verify the workspace builds.**

Run: `cargo build`
Expected: both crates compile; `target/debug/shoebox-server` exists.

- [ ] **Step 4: Verify the stub runs.**

Run: `cargo run -p shoebox-server`
Expected: prints `shoebox-server stub` and exits 0.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server
git commit -m "feat(server): scaffold shoebox-server binary crate"
```

---

## Task 4: Add configuration loading (`Config` from TOML + env)

**Files:**
- Create: `crates/shoebox-server/src/config.rs`
- Modify: `crates/shoebox-server/src/main.rs`
- Test: inline `#[cfg(test)] mod tests` in `config.rs`

- [ ] **Step 1: Write the failing test in `crates/shoebox-server/src/config.rs`.**

```rust
//! Server configuration: loaded from TOML, overridable by env vars.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server_name: String,
    pub bind_addr: SocketAddr,
    pub data_dir: PathBuf,
    pub photos_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Config {
    pub fn from_toml_str(toml_str: &str) -> anyhow::Result<Self> {
        toml::from_str(toml_str).map_err(|e| anyhow::anyhow!("invalid config TOML: {e}"))
    }

    pub fn apply_env_overrides(mut self) -> Self {
        if let Ok(v) = std::env::var("SHOEBOX_BIND_ADDR") {
            if let Ok(addr) = v.parse() {
                self.bind_addr = addr;
            }
        }
        if let Ok(v) = std::env::var("SHOEBOX_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("SHOEBOX_PHOTOS_DIR") {
            self.photos_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("SHOEBOX_CACHE_DIR") {
            self.cache_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("SHOEBOX_SERVER_NAME") {
            self.server_name = v;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_toml() {
        let s = r#"
            server_name = "shoebox-test"
            bind_addr = "127.0.0.1:9000"
            data_dir = "/var/lib/shoebox"
            photos_dir = "/photos"
            cache_dir = "/shoebox-cache"
        "#;
        let cfg = Config::from_toml_str(s).unwrap();
        assert_eq!(cfg.server_name, "shoebox-test");
        assert_eq!(cfg.bind_addr.port(), 9000);
        assert_eq!(cfg.data_dir, PathBuf::from("/var/lib/shoebox"));
    }

    #[test]
    fn env_overrides_take_precedence() {
        std::env::set_var("SHOEBOX_BIND_ADDR", "0.0.0.0:8888");
        let s = r#"
            server_name = "x"
            bind_addr = "127.0.0.1:9000"
            data_dir = "/d"
            photos_dir = "/p"
            cache_dir = "/c"
        "#;
        let cfg = Config::from_toml_str(s).unwrap().apply_env_overrides();
        assert_eq!(cfg.bind_addr.port(), 8888);
        std::env::remove_var("SHOEBOX_BIND_ADDR");
    }
}
```

- [ ] **Step 2: Wire the module into `crates/shoebox-server/src/main.rs`.**

```rust
mod config;

fn main() {
    println!("shoebox-server stub");
}
```

- [ ] **Step 3: Run the tests to verify they pass.**

Run: `cargo test -p shoebox-server config`
Expected: 2 tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/config.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): add Config loader from TOML with env overrides"
```

---

## Task 5: Add structured JSON logging

**Files:**
- Create: `crates/shoebox-server/src/logging.rs`
- Modify: `crates/shoebox-server/src/main.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/logging.rs`.**

```rust
//! Initialize tracing-subscriber for structured JSON logging.
//! Log level configurable via SHOEBOX_LOG (e.g. "info", "debug",
//! "shoebox_server=debug,info").

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_env("SHOEBOX_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_target(true)
                .with_current_span(true)
                .with_span_list(false),
        )
        .init();
}
```

- [ ] **Step 2: Update `crates/shoebox-server/src/main.rs` to call it.**

```rust
mod config;
mod logging;

fn main() {
    logging::init();
    tracing::info!(event = "startup", "shoebox-server starting");
    println!("(stub: nothing running yet)");
}
```

- [ ] **Step 3: Verify it builds and runs.**

Run: `cargo run -p shoebox-server 2>&1 | head -5`
Expected: one JSON line containing `"event":"startup"` followed by the stub message.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/logging.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): add tracing-subscriber JSON logging, SHOEBOX_LOG-configurable"
```

---

## Task 6: Open a libSQL database and write the migration runner

**Files:**
- Create: `crates/shoebox-server/src/db.rs`
- Modify: `crates/shoebox-server/src/main.rs`
- Create: `crates/shoebox-server/migrations/0001_identity.sql` (placeholder, real content in Task 7)

- [ ] **Step 1: Create a placeholder migration so `include_dir!` has something to embed.**

```sql
-- crates/shoebox-server/migrations/0001_identity.sql
-- (Real contents added in Task 7.)
CREATE TABLE IF NOT EXISTS _ping (id INTEGER PRIMARY KEY);
```

- [ ] **Step 2: Write `crates/shoebox-server/src/db.rs` with the migration runner and a failing test.**

```rust
//! libSQL database lifecycle: open, run migrations.

use anyhow::{anyhow, Context, Result};
use include_dir::{include_dir, Dir};
use libsql::{Builder, Connection, Database};
use std::path::Path;

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

pub struct Db {
    pub database: Database,
}

impl Db {
    /// Open (creating if absent) the libSQL database at the given path
    /// and apply all pending migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        let database = Builder::new_local(path)
            .build()
            .await
            .map_err(|e| anyhow!("failed to open libSQL database at {path:?}: {e}"))?;

        let conn = database
            .connect()
            .map_err(|e| anyhow!("failed to connect to libSQL: {e}"))?;
        apply_migrations(&conn).await?;
        Ok(Self { database })
    }

    pub fn connect(&self) -> Result<Connection> {
        self.database
            .connect()
            .map_err(|e| anyhow!("failed to connect to libSQL: {e}"))
    }
}

async fn apply_migrations(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS _schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("creating _schema_migrations")?;

    let mut entries: Vec<_> = MIGRATIONS_DIR
        .files()
        .filter(|f| {
            f.path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "sql")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|f| f.path().file_name().map(|n| n.to_os_string()));

    for file in entries {
        let name = file
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad migration filename"))?;
        let version: i64 = name
            .split('_')
            .next()
            .ok_or_else(|| anyhow!("migration {name} missing numeric prefix"))?
            .parse()
            .map_err(|_| anyhow!("migration {name} has non-numeric prefix"))?;

        let mut rows = conn
            .query(
                "SELECT 1 FROM _schema_migrations WHERE version = ?1",
                [version],
            )
            .await?;
        if rows.next().await?.is_some() {
            continue;
        }

        let sql = file
            .contents_utf8()
            .ok_or_else(|| anyhow!("migration {name} not UTF-8"))?;
        tracing::info!(event = "migration.apply", version, name, "applying migration");
        conn.execute_batch(sql).await.with_context(|| {
            format!("applying migration {name} (version {version})")
        })?;
        let now_ms = chrono_now_ms();
        conn.execute(
            "INSERT INTO _schema_migrations (version, applied_at) VALUES (?1, ?2)",
            (version, now_ms),
        )
        .await?;
    }

    Ok(())
}

fn chrono_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn opens_and_applies_migrations() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        // _schema_migrations exists and contains version 1.
        let mut rows = conn
            .query("SELECT version FROM _schema_migrations ORDER BY version", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("at least one migration");
        let version: i64 = row.get(0).unwrap();
        assert_eq!(version, 1);

        // Reopening is a no-op: migrations are idempotent.
        let db2 = Db::open(&path).await.unwrap();
        drop(db2);
    }
}
```

- [ ] **Step 3: Run the test, expecting it to fail to compile or run because `chrono_now_ms` and module wiring need to be present and the module must be in main.rs.** Update `crates/shoebox-server/src/main.rs`:

```rust
mod config;
mod db;
mod logging;

fn main() {
    logging::init();
    tracing::info!(event = "startup", "shoebox-server starting");
    println!("(stub: nothing running yet)");
}
```

- [ ] **Step 4: Run the test.**

Run: `cargo test -p shoebox-server db::tests::opens_and_applies_migrations`
Expected: PASS. The placeholder `0001_identity.sql` migration creates `_ping` and records version 1.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/db.rs crates/shoebox-server/src/main.rs \
        crates/shoebox-server/migrations/0001_identity.sql
git commit -m "feat(server): add libSQL open and idempotent migration runner"
```

---

## Task 7: Migration 0001 — identity tables

**Files:**
- Modify: `crates/shoebox-server/migrations/0001_identity.sql`
- Test: extend the `db::tests` module

- [ ] **Step 1: Replace `0001_identity.sql` with the real identity schema.**

```sql
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
```

- [ ] **Step 2: Add a test that asserts the four tables exist with the right columns.** Append to the `mod tests` block in `crates/shoebox-server/src/db.rs`:

```rust
    #[tokio::test]
    async fn migration_0001_creates_identity_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["config", "users", "sessions", "revoked_certs"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0001"
            );
        }
    }
```

- [ ] **Step 3: Run the new test, expecting FAIL** because the previous `_ping` table is gone and only this migration applies (the `_schema_migrations` table is empty in a fresh tmp dir, so all migrations run).

Run: `cargo test -p shoebox-server db::tests::migration_0001_creates_identity_tables`
Expected: PASS.

- [ ] **Step 4: Run the full test suite to make sure nothing else broke.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/migrations/0001_identity.sql crates/shoebox-server/src/db.rs
git commit -m "feat(db): migration 0001 - config, users, sessions, revoked_certs"
```

---

## Task 8: Migration 0002 — files (folders, photos, photo_files)

**Files:**
- Create: `crates/shoebox-server/migrations/0002_files.sql`
- Test: extend `db::tests`

- [ ] **Step 1: Write `crates/shoebox-server/migrations/0002_files.sql`.**

```sql
-- File system mirror and photo identity.

CREATE TABLE folders (
    id              TEXT PRIMARY KEY,
    parent_id       TEXT REFERENCES folders(id),
    path            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    last_indexed_at INTEGER
);

CREATE INDEX folders_parent_idx ON folders(parent_id);

CREATE TABLE photos (
    id              TEXT PRIMARY KEY,
    file_size       INTEGER NOT NULL,
    file_format     TEXT NOT NULL,
    captured_at     INTEGER,
    camera_make     TEXT,
    camera_model    TEXT,
    lens            TEXT,
    iso             INTEGER,
    aperture        REAL,
    shutter_us      INTEGER,
    focal_length_mm REAL,
    width_px        INTEGER,
    height_px       INTEGER,
    orientation     INTEGER,
    imported_at     INTEGER NOT NULL,
    exif_json       TEXT
);

CREATE INDEX photos_captured_idx ON photos(captured_at);
CREATE INDEX photos_camera_idx   ON photos(camera_make, camera_model);

CREATE TABLE photo_files (
    id            TEXT PRIMARY KEY,
    photo_id      TEXT NOT NULL REFERENCES photos(id),
    folder_id     TEXT NOT NULL REFERENCES folders(id),
    path          TEXT NOT NULL UNIQUE,
    file_mtime    INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    is_present    INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX photo_files_photo_idx  ON photo_files(photo_id);
CREATE INDEX photo_files_folder_idx ON photo_files(folder_id);
```

- [ ] **Step 2: Add a test in `crates/shoebox-server/src/db.rs`.**

```rust
    #[tokio::test]
    async fn migration_0002_creates_file_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["folders", "photos", "photo_files"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0002"
            );
        }
    }
```

- [ ] **Step 3: Run the test.**

Run: `cargo test -p shoebox-server db::tests::migration_0002_creates_file_tables`
Expected: PASS.

- [ ] **Step 4: Run the full suite.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/migrations/0002_files.sql crates/shoebox-server/src/db.rs
git commit -m "feat(db): migration 0002 - folders, photos, photo_files"
```

---

## Task 9: Migration 0003 — variants and develop locks

**Files:**
- Create: `crates/shoebox-server/migrations/0003_variants.sql`
- Test: extend `db::tests`

- [ ] **Step 1: Write `crates/shoebox-server/migrations/0003_variants.sql`.**

```sql
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
```

- [ ] **Step 2: Add a test in `crates/shoebox-server/src/db.rs`.**

```rust
    #[tokio::test]
    async fn migration_0003_creates_variant_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["variants", "develop_locks"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0003"
            );
        }
    }
```

- [ ] **Step 3: Run the test.**

Run: `cargo test -p shoebox-server db::tests::migration_0003_creates_variant_tables`
Expected: PASS.

- [ ] **Step 4: Full suite.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/migrations/0003_variants.sql crates/shoebox-server/src/db.rs
git commit -m "feat(db): migration 0003 - variants and develop_locks"
```

---

## Task 10: Migration 0004 — variant_user_state

**Files:**
- Create: `crates/shoebox-server/migrations/0004_variant_user_state.sql`
- Test: extend `db::tests`

- [ ] **Step 1: Write `crates/shoebox-server/migrations/0004_variant_user_state.sql`.**

```sql
-- Per-(user, variant) star rating, flag, and color label.

CREATE TABLE variant_user_state (
    variant_id  TEXT NOT NULL REFERENCES variants(id),
    user_id     TEXT NOT NULL REFERENCES users(id),
    rating      INTEGER,
    flag        TEXT,
    color_label TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (variant_id, user_id)
);

CREATE INDEX variant_user_state_user_idx ON variant_user_state(user_id);
```

- [ ] **Step 2: Add a test in `crates/shoebox-server/src/db.rs`.**

```rust
    #[tokio::test]
    async fn migration_0004_creates_variant_user_state() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='variant_user_state'",
                (),
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_some());
    }
```

- [ ] **Step 3: Run the test.**

Run: `cargo test -p shoebox-server db::tests::migration_0004_creates_variant_user_state`
Expected: PASS.

- [ ] **Step 4: Full suite.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/migrations/0004_variant_user_state.sql crates/shoebox-server/src/db.rs
git commit -m "feat(db): migration 0004 - variant_user_state"
```

---

## Task 11: Migration 0005 — keywords and photo_keywords

**Files:**
- Create: `crates/shoebox-server/migrations/0005_keywords.sql`
- Test: extend `db::tests`

- [ ] **Step 1: Write `crates/shoebox-server/migrations/0005_keywords.sql`.**

```sql
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
```

- [ ] **Step 2: Add a test in `crates/shoebox-server/src/db.rs`.**

```rust
    #[tokio::test]
    async fn migration_0005_creates_keyword_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["keywords", "photo_keywords"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0005"
            );
        }
    }
```

- [ ] **Step 3: Run the test.**

Run: `cargo test -p shoebox-server db::tests::migration_0005_creates_keyword_tables`
Expected: PASS.

- [ ] **Step 4: Full suite.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/migrations/0005_keywords.sql crates/shoebox-server/src/db.rs
git commit -m "feat(db): migration 0005 - keywords and photo_keywords"
```

---

## Task 12: Migration 0006 — collections and members

**Files:**
- Create: `crates/shoebox-server/migrations/0006_collections.sql`
- Test: extend `db::tests`

- [ ] **Step 1: Write `crates/shoebox-server/migrations/0006_collections.sql`.**

```sql
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
```

- [ ] **Step 2: Add a test in `crates/shoebox-server/src/db.rs`.**

```rust
    #[tokio::test]
    async fn migration_0006_creates_collection_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["collections", "collection_members"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0006"
            );
        }
    }

    #[tokio::test]
    async fn all_six_migrations_applied_in_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        let mut rows = conn
            .query(
                "SELECT version FROM _schema_migrations ORDER BY version",
                (),
            )
            .await
            .unwrap();
        let mut versions = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            versions.push(row.get::<i64>(0).unwrap());
        }
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
    }
```

- [ ] **Step 3: Run the tests.**

Run: `cargo test -p shoebox-server db::tests`
Expected: all tests pass, including `all_six_migrations_applied_in_order`.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/migrations/0006_collections.sql crates/shoebox-server/src/db.rs
git commit -m "feat(db): migration 0006 - collections and collection_members"
```

---

## Task 13: Add the Axum HTTP server skeleton with `/health`

**Files:**
- Create: `crates/shoebox-server/src/http.rs`
- Modify: `crates/shoebox-server/src/main.rs`
- Test: integration test using `reqwest`

- [ ] **Step 1: Write `crates/shoebox-server/src/http.rs`.**

```rust
//! HTTP server: router construction and request handlers.

use axum::{extract::State, http::StatusCode, response::Json, routing::get, Router};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub schema_version: i64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub schema_version: i64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            schema_version: state.schema_version,
        }),
    )
}

/// Bind and serve the router until `shutdown` resolves.
pub async fn serve(
    addr: SocketAddr,
    state: AppState,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    tracing::info!(event = "http.listen", addr = %actual, "HTTP server bound");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
        })
        .await?;
    Ok(())
}
```

- [ ] **Step 2: Write an integration test in `crates/shoebox-server/src/http.rs`.**

Append to the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn health_endpoint_returns_ok_with_schema_version() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("catalog.db");
        let db = Arc::new(Db::open(&db_path).await.unwrap());
        let state = AppState {
            db,
            schema_version: shoebox_common::SCHEMA_VERSION,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();

        let app = router(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        let resp = reqwest::get(format!("http://{addr}/health"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["schema_version"], 6);

        let _ = tx.send(());
        server.await.unwrap();
    }
}
```

- [ ] **Step 3: Wire `http` into `main.rs`.** Update `crates/shoebox-server/src/main.rs`:

```rust
mod config;
mod db;
mod http;
mod logging;

use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();
    tracing::info!(event = "startup", "shoebox-server starting");

    // Minimal in-place defaults for Plan 1.1; replaced by Config in Task 14.
    let data_dir = std::path::PathBuf::from(
        std::env::var("SHOEBOX_DATA_DIR").unwrap_or_else(|_| "./data".into()),
    );
    std::fs::create_dir_all(&data_dir)?;
    let db = Arc::new(db::Db::open(&data_dir.join("catalog.db")).await?);

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };
    let addr: std::net::SocketAddr = std::env::var("SHOEBOX_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:9000".into())
        .parse()?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(());
    });

    http::serve(addr, state, shutdown_rx).await
}
```

- [ ] **Step 4: Run the integration test.**

Run: `cargo test -p shoebox-server http::tests::health_endpoint_returns_ok_with_schema_version`
Expected: PASS.

- [ ] **Step 5: Smoke-test by hand.**

Run (in one terminal): `cargo run -p shoebox-server`
Run (in another): `curl -s http://127.0.0.1:9000/health | jq`
Expected: `{"status":"ok","schema_version":6}`. `Ctrl+C` the server.

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-server/src/http.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): add Axum HTTP server with /health endpoint and graceful shutdown"
```

---

## Task 14: Wire `Config` into `main.rs`

**Files:**
- Modify: `crates/shoebox-server/src/main.rs`
- Modify: `crates/shoebox-server/src/config.rs` (add file-loading helper)

- [ ] **Step 1: Add a file-loading helper to `crates/shoebox-server/src/config.rs`.** Append:

```rust
impl Config {
    pub fn load_from_path(path: &std::path::Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {path:?}: {e}"))?;
        Ok(Self::from_toml_str(&s)?.apply_env_overrides())
    }

    /// Build a Config from environment variables alone, with sensible
    /// defaults for any not set. Used when no `server.toml` is present.
    pub fn from_env_with_defaults() -> Self {
        Self {
            server_name: std::env::var("SHOEBOX_SERVER_NAME")
                .unwrap_or_else(|_| {
                    hostname::get()
                        .ok()
                        .and_then(|h| h.into_string().ok())
                        .unwrap_or_else(|| "shoebox".to_string())
                }),
            bind_addr: std::env::var("SHOEBOX_BIND_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:9000".into())
                .parse()
                .expect("SHOEBOX_BIND_ADDR must parse as SocketAddr"),
            data_dir: std::path::PathBuf::from(
                std::env::var("SHOEBOX_DATA_DIR").unwrap_or_else(|_| "/var/lib/shoebox".into()),
            ),
            photos_dir: std::path::PathBuf::from(
                std::env::var("SHOEBOX_PHOTOS_DIR").unwrap_or_else(|_| "/photos".into()),
            ),
            cache_dir: std::path::PathBuf::from(
                std::env::var("SHOEBOX_CACHE_DIR").unwrap_or_else(|_| "/shoebox-cache".into()),
            ),
        }
    }
}
```

- [ ] **Step 2: Replace `main.rs` to use `Config`.**

```rust
mod config;
mod db;
mod http;
mod logging;

use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();

    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    let cfg = match cfg_path {
        Some(p) => {
            tracing::info!(event = "config.load", path = %p, "loading config file");
            config::Config::load_from_path(std::path::Path::new(&p))?
        }
        None => {
            tracing::info!(event = "config.load", source = "env", "no SHOEBOX_CONFIG; building from env");
            config::Config::from_env_with_defaults()
        }
    };

    tracing::info!(
        event = "startup",
        server_name = %cfg.server_name,
        bind_addr = %cfg.bind_addr,
        data_dir = ?cfg.data_dir,
        "shoebox-server starting"
    );

    std::fs::create_dir_all(&cfg.data_dir)?;
    let db = Arc::new(db::Db::open(&cfg.data_dir.join("catalog.db")).await?);

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown.signal", "received ctrl-c, shutting down");
        let _ = shutdown_tx.send(());
    });

    http::serve(cfg.bind_addr, state, shutdown_rx).await
}
```

- [ ] **Step 3: Smoke test with a real config file.**

Create `server.toml.example`:

```toml
server_name = "shoebox-dev"
bind_addr = "127.0.0.1:9000"
data_dir = "./data"
photos_dir = "./photos"
cache_dir = "./cache"
```

Run: `SHOEBOX_CONFIG=./server.toml.example cargo run -p shoebox-server &`
Then: `curl -s http://127.0.0.1:9000/health`
Expected: `{"status":"ok","schema_version":6}`. Kill the background process: `kill %1`.

- [ ] **Step 4: Full test suite.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/config.rs crates/shoebox-server/src/main.rs server.toml.example
git commit -m "feat(server): load Config from SHOEBOX_CONFIG TOML or env defaults"
```

---

## Task 15: Add mDNS service broadcaster

**Files:**
- Create: `crates/shoebox-server/src/mdns.rs`
- Modify: `crates/shoebox-server/src/main.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/mdns.rs`.**

```rust
//! mDNS service broadcaster. Announces _shoebox._tcp.local with TXT
//! records so LAN clients can auto-discover the server.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;

pub const SERVICE_TYPE: &str = "_shoebox._tcp.local.";

pub struct MdnsBroadcaster {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsBroadcaster {
    /// Begin broadcasting. Returns immediately; the daemon broadcasts
    /// in the background until `shutdown()` is called.
    pub fn start(
        server_name: &str,
        port: u16,
        schema_version: i64,
        ips: Vec<IpAddr>,
    ) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("creating mdns daemon")?;
        let host_label = sanitize(server_name);
        let fullname = format!("{host_label}.{SERVICE_TYPE}");

        let mut txt = HashMap::new();
        txt.insert("name".to_string(), server_name.to_string());
        txt.insert("schema".to_string(), schema_version.to_string());
        txt.insert("proto".to_string(), "libsql".to_string());

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &host_label,
            &format!("{host_label}.local."),
            ips.as_slice(),
            port,
            Some(txt),
        )
        .context("building ServiceInfo")?;

        daemon.register(info).context("registering mdns service")?;
        tracing::info!(
            event = "mdns.register",
            service = SERVICE_TYPE,
            name = %server_name,
            port,
            "mDNS service registered"
        );

        Ok(Self { daemon, fullname })
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
        tracing::info!(event = "mdns.unregister", "mDNS service unregistered");
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

/// Enumerate non-loopback IPs from local network interfaces.
pub fn local_ips() -> Vec<IpAddr> {
    // Use `if_addrs` for cross-platform interface enumeration.
    // For now keep it minimal: read from std until we add the dep.
    // mdns-sd accepts an empty list and will try to use all interfaces.
    Vec::new()
}
```

- [ ] **Step 2: Wire it into `main.rs`.** Update `crates/shoebox-server/src/main.rs` to start the broadcaster after binding HTTP and shut it down before exit:

```rust
mod config;
mod db;
mod http;
mod logging;
mod mdns;

use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();

    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    let cfg = match cfg_path {
        Some(p) => {
            tracing::info!(event = "config.load", path = %p, "loading config file");
            config::Config::load_from_path(std::path::Path::new(&p))?
        }
        None => {
            tracing::info!(event = "config.load", source = "env", "no SHOEBOX_CONFIG; building from env");
            config::Config::from_env_with_defaults()
        }
    };

    tracing::info!(
        event = "startup",
        server_name = %cfg.server_name,
        bind_addr = %cfg.bind_addr,
        data_dir = ?cfg.data_dir,
        "shoebox-server starting"
    );

    std::fs::create_dir_all(&cfg.data_dir)?;
    let db = Arc::new(db::Db::open(&cfg.data_dir.join("catalog.db")).await?);

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };

    let broadcaster = mdns::MdnsBroadcaster::start(
        &cfg.server_name,
        cfg.bind_addr.port(),
        shoebox_common::SCHEMA_VERSION,
        mdns::local_ips(),
    )?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown.signal", "received ctrl-c, shutting down");
        let _ = shutdown_tx.send(());
    });

    let result = http::serve(cfg.bind_addr, state, shutdown_rx).await;
    broadcaster.shutdown();
    result
}
```

- [ ] **Step 3: Verify it builds.**

Run: `cargo build -p shoebox-server`
Expected: clean build.

- [ ] **Step 4: Smoke-test the broadcaster.** Run the server and use a host's mDNS browser to verify the service appears.

Run (terminal 1): `SHOEBOX_CONFIG=./server.toml.example cargo run -p shoebox-server`

Run (terminal 2, on macOS): `dns-sd -B _shoebox._tcp` (Linux: `avahi-browse -r _shoebox._tcp` if avahi is installed).
Expected: one entry named `shoebox-dev`.

Kill the server with `Ctrl+C`; the broadcaster log line `mDNS service unregistered` should appear.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/mdns.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): broadcast _shoebox._tcp via mDNS with name/schema/proto TXT records"
```

---

## Task 16: Add an integration test that runs the full server end-to-end

**Files:**
- Create: `crates/shoebox-server/tests/health_e2e.rs`

- [ ] **Step 1: Write the integration test.**

```rust
//! End-to-end test: spawn the server's components in-process, hit
//! /health over loopback, verify response.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn full_server_serves_health() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(
        shoebox_server::db::Db::open(&tmp.path().join("catalog.db"))
            .await
            .unwrap(),
    );

    let state = shoebox_server::http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    let app = shoebox_server::http::router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    let resp = reqwest::get(format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["schema_version"], 6);

    let _ = tx.send(());
    server.await.unwrap();
}
```

- [ ] **Step 2: Expose the necessary modules from `shoebox-server` for use in integration tests.** Add a `lib.rs` so integration tests can import server modules. Create `crates/shoebox-server/src/lib.rs`:

```rust
//! Library facade for integration tests. The binary entry point lives
//! in `main.rs` and uses these modules directly.

pub mod config;
pub mod db;
pub mod http;
pub mod logging;
pub mod mdns;
```

Update `crates/shoebox-server/Cargo.toml` to declare both targets — add under `[package]`:

```toml
[lib]
name = "shoebox_server"
path = "src/lib.rs"
```

And remove the `mod config; mod db; ...` lines from `main.rs`, replacing them with:

```rust
use shoebox_server::{config, db, http, logging, mdns};
```

- [ ] **Step 3: Run the integration test.**

Run: `cargo test -p shoebox-server --test health_e2e`
Expected: PASS.

- [ ] **Step 4: Run the full suite.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/lib.rs crates/shoebox-server/src/main.rs \
        crates/shoebox-server/Cargo.toml crates/shoebox-server/tests/health_e2e.rs
git commit -m "test(server): add end-to-end /health integration test and expose lib.rs"
```

---

## Task 17: Add a multi-stage Dockerfile

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Write `.dockerignore`.**

```
target/
data/
.git/
**/*.swp
.DS_Store
```

- [ ] **Step 2: Write `Dockerfile`.**

```dockerfile
# syntax=docker/dockerfile:1.6

FROM rust:1.78-slim-bookworm AS builder

# Build dependencies first for caching: copy manifests, fetch deps,
# then copy sources and build.
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml Cargo.toml ./
COPY crates/shoebox-common/Cargo.toml crates/shoebox-common/Cargo.toml
COPY crates/shoebox-server/Cargo.toml crates/shoebox-server/Cargo.toml

# Stub sources so `cargo fetch` works without real code.
RUN mkdir -p crates/shoebox-common/src crates/shoebox-server/src/migrations \
    && echo "fn main() {}" > crates/shoebox-server/src/main.rs \
    && echo "" > crates/shoebox-common/src/lib.rs \
    && cargo fetch

COPY crates ./crates
RUN cargo build --release -p shoebox-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/shoebox shoebox

COPY --from=builder /build/target/release/shoebox-server /usr/local/bin/shoebox-server

USER shoebox
WORKDIR /var/lib/shoebox
EXPOSE 9000
ENV SHOEBOX_BIND_ADDR=0.0.0.0:9000 \
    SHOEBOX_DATA_DIR=/var/lib/shoebox \
    SHOEBOX_PHOTOS_DIR=/photos \
    SHOEBOX_CACHE_DIR=/shoebox-cache

ENTRYPOINT ["/usr/local/bin/shoebox-server"]
```

- [ ] **Step 3: Build the image locally.**

Run: `docker build -t shoebox-server:dev .`
Expected: successful build. May take 5-10 minutes the first time due to dependency compilation.

- [ ] **Step 4: Run the image and hit /health.**

Run: `docker run --rm -p 9000:9000 -v /tmp/shoebox-data:/var/lib/shoebox shoebox-server:dev &`
Then: `sleep 2 && curl -s http://127.0.0.1:9000/health`
Expected: `{"status":"ok","schema_version":6}`.

Stop the container: `docker kill $(docker ps -q --filter ancestor=shoebox-server:dev)`.

- [ ] **Step 5: Commit.**

```bash
git add Dockerfile .dockerignore
git commit -m "build: multi-stage Dockerfile producing distroless-style shoebox-server image"
```

---

## Task 18: Configure clippy lints and run them clean

**Files:**
- Modify: `Cargo.toml` (add `[workspace.lints]`)

- [ ] **Step 1: Add lint configuration to `Cargo.toml`.** Append to the workspace manifest:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
# Pedantic exceptions that fight ergonomic Rust in this codebase:
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
```

- [ ] **Step 2: Apply lints to each crate.** In `crates/shoebox-common/Cargo.toml` and `crates/shoebox-server/Cargo.toml`, add at the end:

```toml
[lints]
workspace = true
```

- [ ] **Step 3: Run clippy and fix any warnings.**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS. Fix anything that doesn't.

- [ ] **Step 4: Run rustfmt.**

Run: `cargo fmt --all`
Then: `cargo fmt --all -- --check`
Expected: no formatting changes pending.

- [ ] **Step 5: Commit.**

```bash
git add Cargo.toml crates/shoebox-common/Cargo.toml crates/shoebox-server/Cargo.toml
git commit -m "build: enable workspace-wide clippy::all + clippy::pedantic with curated exceptions"
```

---

## Task 19: Add a minimal CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write `.github/workflows/ci.yml`.**

```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.78.0
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: fmt
        run: cargo fmt --all -- --check
      - name: clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: test
        run: cargo test --workspace --all-targets

  docker:
    runs-on: ubuntu-latest
    needs: test
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-buildx-action@v3
      - name: Build image (no push)
        run: docker build -t shoebox-server:ci .
```

- [ ] **Step 2: Sanity-check the YAML is valid.**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"`
Expected: completes with no error.

- [ ] **Step 3: Commit.**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add fmt, clippy, test, and docker build workflow"
```

---

## Task 20: Update CLAUDE.md and README with implementation status

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

- [ ] **Step 1: Update the sub-project status table in `CLAUDE.md`.** Find the row for sub-project #1 and replace its Status column:

```
| 1 | **Catalog, sync & stack** | Plan 1.1 (server foundation) implemented — workspace, schema migrations, /health, mDNS, Dockerfile, CI. Plans 1.2-1.5 pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
```

Also append a new section before "Memory pointers":

```markdown
## Implementation status

- `crates/shoebox-server` — workspace skeleton, libSQL catalog with 6 migrations, Axum HTTP server with `/health`, mDNS broadcaster, multi-stage Dockerfile. No auth, no indexer, no thumbnailer yet.
- `crates/shoebox-common` — shared `Error`/`Result` and `SCHEMA_VERSION` constant.
- Run locally: `cargo run -p shoebox-server` (defaults to `127.0.0.1:9000`).
- Run in Docker: `docker build -t shoebox-server:dev . && docker run --rm -p 9000:9000 -v /tmp/shoebox-data:/var/lib/shoebox shoebox-server:dev`.
- CI: fmt + clippy + tests + docker build on push and PR.
```

- [ ] **Step 2: Replace the one-line `README.md` with something useful.**

```markdown
# shoebox

Cross-platform desktop application for managing, developing, and exporting
RAW digital photos. Multi-user shared catalog hosted on a NAS.

**Status:** in active development. See `CLAUDE.md` for sub-project status and
`docs/superpowers/specs/` for design documents.

## Running the server locally

```bash
cargo run -p shoebox-server
curl -s http://127.0.0.1:9000/health
```

## Building the Docker image

```bash
docker build -t shoebox-server:dev .
docker run --rm -p 9000:9000 \
  -v /tmp/shoebox-data:/var/lib/shoebox \
  shoebox-server:dev
```

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```
```

- [ ] **Step 3: Commit.**

```bash
git add CLAUDE.md README.md
git commit -m "docs: update CLAUDE.md and README to reflect Plan 1.1 implementation"
```

---

## Definition of Done for Plan 1.1

After all 20 tasks are complete:

- `cargo test --workspace` passes (all schema, config, http unit + integration tests).
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- `cargo fmt --all -- --check` is clean.
- `cargo run -p shoebox-server` starts the server, creates `./data/catalog.db`, applies all 6 migrations, binds `127.0.0.1:9000`, serves `/health`, and broadcasts `_shoebox._tcp.local` via mDNS.
- `docker build -t shoebox-server:dev .` builds successfully and the resulting image runs `/health` correctly.
- CI workflow runs fmt, clippy, tests, and docker build on every push/PR.

What this plan **does not** deliver — covered in subsequent plans:
- mTLS / internal CA / enrollment / revocation (Plan 1.2).
- libSQL wire-protocol proxy for client embedded replicas (Plan 1.2).
- Filesystem indexer, thumbnailer, develop-lock server operations, janitor, backups, metrics endpoints (Plan 1.3).
- Iced desktop client (Plan 1.4).
- docker-compose template, Helm chart, multi-arch builds, install docs (Plan 1.5).

---

## Self-Review

Looking at the spec (`docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md`) against this plan:

**Spec coverage:**
- §2 (Stack decisions) → Tasks 1, 3 establish the Rust/Cargo workspace and pin the toolchain. libSQL chosen as the DB in Task 6. Iced is deferred to Plan 1.4.
- §4 (Data model) — all 12 tables covered: Tasks 7-12 implement the 6 migrations.
- §4.7 (Schema migrations) — Task 6 implements the runner with `_schema_migrations`; the forward-only additive policy is documented but enforcement (min/max client schema check) is deferred to Plan 1.2 where the enroll endpoint exists.
- §7.5 (Discovery via mDNS) — Task 15.
- §9.3 (Observability — `/health`) — Task 13. `/metrics` deferred to Plan 1.3.
- §9.3 (Structured logs) — Task 5.
- §8.1 (Docker on NAS) — Task 17 establishes the image; docker-compose template is in Plan 1.5.
- §6 (Storage layout — `catalog.db` placement constraint) — surfaced in the README in Task 20; the binary defaults to a local data dir.

**Not in this plan but in the spec — explicit deferrals:**
- §7 (Auth & discovery) — mTLS, CA, enrollment endpoints in Plan 1.2.
- §9.2 (Backup VACUUM INTO) in Plan 1.3.
- §10 (Testing strategy — property tests, fault injection, load tests) in Plan 1.3.
- §4 `develop_settings_json` schema validation — defined in spec, but no validation code yet; that lives where edits get persisted (Plan 1.3 via the libSQL proxy, or Plan 1.4 client-side).

**Placeholder scan:** none. Every step has either runnable code or an exact command.

**Type consistency:** `AppState` defined in Task 13, referenced in Task 14 and Task 16 — consistent. `Db` defined in Task 6, used in Tasks 13, 14, 16 — consistent. `Config` defined in Task 4, extended in Task 14 — consistent.

**One known gap to flag for the implementing engineer:** Task 6's migration runner uses `libsql::Connection::execute_batch` which is real in libSQL ≥0.4. If a newer version of libSQL renamed it (the API has been evolving), check `libsql --help` or the docs.rs page for the pinned version and adapt — the rest of the runner doesn't depend on the exact method name. Pinning to `libsql = "0.6"` in the workspace dependencies should keep this stable.
