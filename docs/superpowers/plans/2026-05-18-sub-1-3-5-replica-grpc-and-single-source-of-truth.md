# Sub-1-3-5 Implementation Plan — Replica gRPC routing & single-source-of-truth catalog

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the libSQL embedded-replica round-trip actually work end-to-end. After this plan, the first-run wizard reaches the Library screen, server-side writes are visible on the client's replica, the integration tests that exercise this path actually execute in CI, and the legacy `<data_dir>/catalog.db` file is renamed cleanly on upgrade.

**Architecture:** Server spawns `sqld` with both `--http-listen-addr` (Hrana) and `--grpc-listen-addr` (libSQL replication), backed by a single `--db-path`. The mTLS proxy on `:9000` branches by `Content-Type`: `application/grpc*` → HTTP/2-only hyper client → sqld's gRPC port (with `/v1`/`/v2` path prefix stripped); everything else → existing HTTP/1.1 client → sqld's HTTP port. `shoebox-server`'s `Db` switches from libsql's local backend to a libsql remote client talking to sqld's loopback HTTP port — same database backs both server-side writes and client replicas. The spike commit `691d94e` already landed the proxy + sqld_embed + ALPN + AppState wiring; this plan finishes the Db rewrite, the upgrade path, the CI sqld install, and the end-to-end verification.

**Tech Stack:** Same as Plan 1.3 (axum 0.7, hyper-util 0.1.20, tonic 0.11, libsql 0.6.0, rustls 0.23, sqld v0.24.32). No version bumps.

**Prerequisites for the implementing engineer:**

- Read the spec: `docs/superpowers/specs/2026-05-18-sub-1-3-5-replica-grpc-and-single-source-of-truth-design.md` (commit `85ec6c2`).
- The spike (commit `691d94e`) is already on `main` — proxy + sqld_embed + ALPN + AppState already do the gRPC routing. This plan picks up from there.
- A working `sqld` binary on `$PATH` (or via `SHOEBOX_SQLD_PATH`):
  - **Linux:** `wget https://github.com/tursodatabase/libsql/releases/download/libsql-server-v0.24.32/libsql-server-x86_64-unknown-linux-gnu.tar.xz && tar -xJf libsql-server-x86_64-unknown-linux-gnu.tar.xz && sudo install -m 755 libsql-server-x86_64-unknown-linux-gnu/sqld /usr/local/bin/sqld`
  - **macOS:** `cargo install --git https://github.com/tursodatabase/libsql --tag libsql-server-v0.24.32 sqld`
  - **Windows:** sqld doesn't have a Windows build; extract from the project's docker image into WSL2: `docker create --name x shoebox-server:dev; docker cp x:/usr/local/bin/sqld /tmp/sqld; docker rm x`. Then run tests inside WSL2.
- Confirm with `sqld --version` before starting.
- Docker available for end-to-end verification.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/shoebox-server/src/db.rs` | Modify | Swap `Builder::new_local` → `Builder::new_remote`; change `Db::open` signature from `(path: &Path)` to `(sqld_http_url: &str)`. |
| `crates/shoebox-server/src/lib.rs` | Modify | Re-export new `test_helpers` module behind `#[cfg(any(test, feature = "test-helpers"))]`. |
| `crates/shoebox-server/src/test_helpers.rs` | Create | `TestDb` helper that spawns sqld + opens a `Db` against it. Reduces boilerplate in 14+ test files. |
| `crates/shoebox-server/Cargo.toml` | Modify | Add `test-helpers` feature; gate optional `tempfile` re-export if not already. |
| `crates/shoebox-server/src/main.rs` | Modify | Reorder startup (sqld → Db → rest); add catalog.db.legacy rename. |
| `crates/shoebox-server/src/upgrade.rs` | Create | `rename_legacy_catalog_db(data_dir)` — pre-startup migration helper, unit-testable. |
| `crates/shoebox-server/src/lib.rs` | Modify | `pub mod upgrade;`. |
| `crates/shoebox-server/src/{secret,ca_cert,http,indexer}.rs` | Modify | Update `#[cfg(test)]` blocks to use `TestDb`. |
| `crates/shoebox-server/tests/{enroll,revoke,locks,health,renew,metrics,proxy}_e2e.rs` | Modify | Switch to `TestDb`. |
| `crates/shoebox-client/tests/{replica,library_view,library_lock,first_run,cert_renewal}_e2e.rs` | Modify | Switch their server-side Db setup to `TestDb`. |
| `.github/workflows/ci.yml` | Modify | New step in `test` job installing sqld v0.24.32. |
| `CLAUDE.md` | Modify | Update sub-project #1 status; remove "two writers to catalog.db" risk; reference the new spec. |

No new client-side files — the client doesn't change.

---

## Task 1: CI installs sqld v0.24.32

This task ships independently of the rest. It un-gates the existing sqld-dependent tests so the rest of the plan has automation feedback.

**Files:**
- Modify: `.github/workflows/ci.yml:9-24`

- [ ] **Step 1: Add the sqld install step**

Insert after the existing `Install Rust toolchain` + `rust-cache` steps, before the `fmt` step:

```yaml
      - name: Install sqld v0.24.32
        run: |
          set -eux
          cd /tmp
          wget -q https://github.com/tursodatabase/libsql/releases/download/libsql-server-v0.24.32/libsql-server-x86_64-unknown-linux-gnu.tar.xz
          echo "71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f  libsql-server-x86_64-unknown-linux-gnu.tar.xz" | sha256sum -c -
          tar -xJf libsql-server-x86_64-unknown-linux-gnu.tar.xz
          sudo install -m 755 libsql-server-x86_64-unknown-linux-gnu/sqld /usr/local/bin/sqld
          sqld --version
```

Version and sha256 mirror `Dockerfile:38-55`. ubuntu-latest already has wget + tar + sudo.

- [ ] **Step 2: Verify the workflow file parses**

Run:
```bash
yq eval '.jobs.test.steps[] | select(.name == "Install sqld v0.24.32")' .github/workflows/ci.yml
```
Expected: prints the step. If `yq` isn't installed, `grep -A6 "Install sqld" .github/workflows/ci.yml` is fine.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
GIT_AUTHOR_NAME="Christian Huening" GIT_AUTHOR_EMAIL="christianhuening@posteo.de" \
GIT_COMMITTER_NAME="Christian Huening" GIT_COMMITTER_EMAIL="christianhuening@posteo.de" \
git commit -m "ci: install sqld v0.24.32 in the test job

The replica_e2e, library_view_e2e, library_lock_e2e, proxy_e2e and
locks_e2e integration tests all gate on which::which(\"sqld\").is_ok()
and have been silently skipping in CI since they were added. Pin the
sqld build to match the Dockerfile's pinned version + sha256.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

Push and wait for CI on the next push or PR — this is the regression gate for the rest of the plan.

---

## Task 2: Add the `TestDb` test helper

Before changing `Db::open`'s signature, give the tests an ergonomic way to spawn sqld and open a Db against it. Without this, the next task's diff is ~14 files of identical 4-line boilerplate.

**Files:**
- Create: `crates/shoebox-server/src/test_helpers.rs`
- Modify: `crates/shoebox-server/src/lib.rs`
- Modify: `crates/shoebox-server/Cargo.toml`

- [ ] **Step 1: Add the `test-helpers` feature**

Edit `crates/shoebox-server/Cargo.toml` and add to the `[features]` table (create the table if absent):

```toml
[features]
default = []
test-helpers = ["dep:tempfile"]
```

If `tempfile` is already a `[dev-dependencies]`, move it to `[dependencies]` with `optional = true`:

```toml
[dependencies]
# ... existing deps ...
tempfile = { version = "3", optional = true }
```

Confirm by reading the resulting `Cargo.toml` once.

- [ ] **Step 2: Add the helper module**

Create `crates/shoebox-server/src/test_helpers.rs`:

```rust
//! Test helpers exposed to integration tests in this crate and the
//! `shoebox-client` crate. Gated behind the `test-helpers` cargo feature
//! so they never compile into the production binary.

use std::sync::Arc;
use tempfile::TempDir;

use crate::db::Db;
use crate::sqld_embed::{self, EmbeddedSqld};

/// A spawned `sqld` subprocess plus a `Db` connected to it via libsql's
/// remote backend. The temp data dir is held for the lifetime of the
/// struct; `shutdown()` consumes self and SIGKILLs the child.
pub struct TestDb {
    pub db: Arc<Db>,
    pub embedded: EmbeddedSqld,
    pub data_dir: TempDir,
}

impl TestDb {
    /// Spawn a fresh sqld in a temp directory and open a Db against it.
    /// Returns once sqld is accepting HTTP connections and all migrations
    /// have been applied.
    pub async fn start() -> Self {
        let data_dir = TempDir::new().expect("creating temp data dir");
        let embedded = sqld_embed::start(data_dir.path().to_path_buf())
            .await
            .expect("spawning sqld");
        let db = Arc::new(
            Db::open(&embedded.local_url)
                .await
                .expect("opening Db against sqld"),
        );
        Self {
            db,
            embedded,
            data_dir,
        }
    }

    /// SIGKILL the sqld child. The TempDir is dropped (deleted) by the
    /// struct's normal Drop.
    pub async fn shutdown(self) {
        self.embedded.shutdown().await;
        // data_dir Drop removes the directory.
    }
}
```

- [ ] **Step 3: Re-export from lib.rs**

Add to `crates/shoebox-server/src/lib.rs`:

```rust
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;
```

- [ ] **Step 4: Have `shoebox-client` depend on the feature for its tests**

Edit `crates/shoebox-client/Cargo.toml`. Find the `[dev-dependencies]` line for `shoebox-server` (typically `shoebox-server = { path = "../shoebox-server" }`) and add the feature:

```toml
[dev-dependencies]
shoebox-server = { path = "../shoebox-server", features = ["test-helpers"] }
```

- [ ] **Step 5: Verify the workspace still compiles**

```bash
cargo check --workspace --all-targets --features shoebox-server/test-helpers
```

Expected: clean. (`cargo check --workspace --all-targets` without the feature flag should also pass — the cfg-gated mod isn't built then.)

- [ ] **Step 6: Commit**

```bash
git add crates/shoebox-server/src/test_helpers.rs \
        crates/shoebox-server/src/lib.rs \
        crates/shoebox-server/Cargo.toml \
        crates/shoebox-client/Cargo.toml
GIT_AUTHOR_NAME="Christian Huening" GIT_AUTHOR_EMAIL="christianhuening@posteo.de" \
GIT_COMMITTER_NAME="Christian Huening" GIT_COMMITTER_EMAIL="christianhuening@posteo.de" \
git commit -m "test(server): add TestDb helper for sqld-backed integration tests

Wraps the spawn-sqld + Db::open boilerplate into one TestDb::start()
call. Gated behind the test-helpers feature so it never compiles into
the production binary. shoebox-client's dev-dependencies enable the
feature so client integration tests can use it.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Rewrite `Db::open` to use libsql's remote backend

Single atomic refactor — the signature change forces all call sites to update in the same commit.

**Files:**
- Modify: `crates/shoebox-server/src/db.rs`

- [ ] **Step 1: Update `Db::open`**

In `crates/shoebox-server/src/db.rs`, replace lines 14-34 (the `impl Db { pub async fn open ... pub fn connect ... }` block). Existing code:

```rust
impl Db {
    /// Open (creating if absent) the libSQL database at the given path
    /// and apply all pending migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        let database = Builder::new_local(path)
            .build()
            .await
            .map_err(|e| anyhow!("failed to open libSQL database at {}: {e}", path.display()))?;

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
    // ...
}
```

Replace with:

```rust
impl Db {
    /// Open a libsql `Database` connected over HTTP to the loopback
    /// `sqld` subprocess at `sqld_http_url`, and apply all pending
    /// migrations through that connection. sqld is the single backing
    /// store for both server-side writes and client-side replicas.
    pub async fn open(sqld_http_url: &str) -> Result<Self> {
        let database = Builder::new_remote(sqld_http_url.to_string(), String::new())
            .build()
            .await
            .map_err(|e| {
                anyhow!("failed to open libSQL remote database at {sqld_http_url}: {e}")
            })?;

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
    // ... rest unchanged ...
}
```

Also remove the `use std::path::Path;` line at the top of `db.rs` — no longer used.

- [ ] **Step 2: Update the `#[cfg(test)] mod tests` block in `db.rs`**

Tests in db.rs currently look like:

```rust
let tmp = TempDir::new().unwrap();
let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
```

Replace each with:

```rust
let test_db = crate::test_helpers::TestDb::start().await;
let db = test_db.db.clone();
// ... existing test body using `db` ...
test_db.shutdown().await;
```

Apply at every `Db::open(&...)` call site in `db.rs` (lines 303, 327, 349, 370, 413, 435, 452, 474, 496, 532 per earlier grep). After editing, the file should have **no** direct `Db::open(...)` calls — they all go through `TestDb::start()`.

- [ ] **Step 3: Update `secret.rs`, `ca_cert.rs`, `http.rs`, `indexer.rs` test blocks**

Same pattern in each `#[cfg(test)] mod tests { ... }`:

- `secret.rs:92,109` — replace `Db::open(&tmp.path().join("catalog.db")).await.unwrap()` with `TestDb::start().await`-then-`.db.clone()`.
- `ca_cert.rs:37` — same.
- `http.rs:76` — same.
- `indexer.rs:548` — same.

For brevity: search-and-replace each file. The diff per file is mechanical.

- [ ] **Step 4: Update `enroll_e2e.rs`, `revoke_e2e.rs`, `locks_e2e.rs`, `health_e2e.rs`, `renew_e2e.rs`, `metrics_e2e.rs`**

Each of these currently does:

```rust
let tmp = TempDir::new().unwrap();
let db = Arc::new(
    shoebox_server::db::Db::open(&tmp.path().join("catalog.db"))
        .await
        .unwrap(),
);
// ... AppState construction ...
let state = shoebox_server::http::AppState {
    db: db.clone(),
    schema_version: shoebox_common::SCHEMA_VERSION,
    ca: ca.clone(),
    sqld_url: "http://127.0.0.1:0".to_string(),
    sqld_grpc_url: "http://127.0.0.1:0".to_string(),
    cache_dir: tmp.path().to_path_buf(),
};
```

Replace with:

```rust
let test_db = shoebox_server::test_helpers::TestDb::start().await;
let db = test_db.db.clone();
let cache_dir_temp = TempDir::new().unwrap();
// ... CA construction unchanged ...
let state = shoebox_server::http::AppState {
    db: db.clone(),
    schema_version: shoebox_common::SCHEMA_VERSION,
    ca: ca.clone(),
    sqld_url: test_db.embedded.local_url.clone(),
    sqld_grpc_url: test_db.embedded.local_grpc_url.clone(),
    cache_dir: cache_dir_temp.path().to_path_buf(),
};
```

…then at the end of each test body (before the final `let _ = shutdown_tx.send(())` and `server.await`), add:

```rust
test_db.shutdown().await;
```

- [ ] **Step 5: Update `proxy_e2e.rs`**

This one already spawns sqld directly (lines 60-65). Replace the manual `Db::open` + `sqld_embed::start` pair with `TestDb::start()`. Same pattern as Step 4.

- [ ] **Step 6: Update the five `shoebox-client/tests/*.rs` files**

`replica_e2e.rs`, `library_view_e2e.rs`, `library_lock_e2e.rs`, `first_run_e2e.rs`, `cert_renewal_e2e.rs` each have the same server-side bootstrap. Replace with `TestDb::start()` calls. The seeded writes in `replica_e2e.rs:52-74` (INSERTs into `users` and `photos`) now hit sqld through `Db::connect()` — no change to the test body, since `Db::connect()`'s API is unchanged.

- [ ] **Step 7: Reorder `main.rs::serve_main` so sqld spawns first**

This commit must keep the binary build green, so the `main.rs` change lands here, in the same atomic commit as the `Db` signature change.

In `crates/shoebox-server/src/main.rs::serve_main`, find the original early-startup block (roughly lines 47–100 in the current source). It currently does (paraphrased):

```rust
std::fs::create_dir_all(&cfg.data_dir)?;
let db = Arc::new(db::Db::open(&cfg.data_dir.join("catalog.db")).await?);

let ca = Arc::new(ca::Ca::open(&cfg.data_dir)?);
// ... CA + cert + CRL + secret bootstrap ...

let embedded_sqld = sqld_embed::start(cfg.data_dir.clone()).await?;
```

Reorder to spawn sqld first, then open the Db against sqld's HTTP URL:

```rust
std::fs::create_dir_all(&cfg.data_dir)?;

// sqld must be running before we can open the Db (libsql remote backend
// connects to sqld's HTTP port). Sub-1-3-5 reordered startup to put
// sqld at the front; everything that needs the catalog comes after.
let embedded_sqld = sqld_embed::start(cfg.data_dir.clone()).await?;

let db = Arc::new(db::Db::open(&embedded_sqld.local_url).await?);

let ca = Arc::new(ca::Ca::open(&cfg.data_dir)?);
// ... CA + cert + CRL + secret bootstrap (unchanged) ...
```

Then **remove** the original `let embedded_sqld = sqld_embed::start(cfg.data_dir.clone()).await?;` line further down (around line 100) — it has moved up. The rest of `serve_main` continues unchanged; downstream code already references `embedded_sqld.local_url` (and now `embedded_sqld.local_grpc_url` from the spike) in the `AppState` construction.

The `upgrade::rename_legacy_catalog_db` call lands in Task 4 — for this commit, the legacy catalog.db handling is not yet present, so on a pre-existing volume the legacy file will sit untouched on disk. That's harmless (nothing reads it) and Task 4 cleans it up.

- [ ] **Step 8: Run all tests**

```bash
cargo test --workspace --all-targets --features shoebox-server/test-helpers
```

Expected: all tests pass **except** any that depend on `main.rs` building (none should — main.rs is a binary, not a lib target, and tests link against lib).

If `cargo test` errors out because `cargo` builds the binary first: run only the lib + integration tests:

```bash
cargo test --workspace --tests --lib --features shoebox-server/test-helpers
```

- [ ] **Step 9: Commit**

```bash
git add crates/shoebox-server/src/db.rs \
        crates/shoebox-server/src/secret.rs \
        crates/shoebox-server/src/ca_cert.rs \
        crates/shoebox-server/src/http.rs \
        crates/shoebox-server/src/indexer.rs \
        crates/shoebox-server/src/main.rs \
        crates/shoebox-server/tests/ \
        crates/shoebox-client/tests/
GIT_AUTHOR_NAME="Christian Huening" GIT_AUTHOR_EMAIL="christianhuening@posteo.de" \
GIT_COMMITTER_NAME="Christian Huening" GIT_COMMITTER_EMAIL="christianhuening@posteo.de" \
git commit -m "refactor(server): route Db through sqld (single source of truth)

Db::open now takes the sqld HTTP URL and uses libsql's remote backend
(Builder::new_remote) instead of the local file backend. This closes
the cross-database divergence flagged in CLAUDE.md as 'two writers to
catalog.db' — there is now one underlying SQLite, owned by sqld, and
server-side writes flow through the same connection that client-side
replicas sync from.

All 14+ tests that previously opened a Db against a temp .db file now
go through the new TestDb helper, which spawns sqld and opens a Db
against it in one call. main.rs still references the old signature with
a TODO marker — the next commit (Task 4) re-orders startup so sqld
spawns before Db::open.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Reorder server startup + add catalog.db.legacy rename

**Files:**
- Create: `crates/shoebox-server/src/upgrade.rs`
- Modify: `crates/shoebox-server/src/lib.rs`
- Modify: `crates/shoebox-server/src/main.rs`

- [ ] **Step 1: Write the failing test for the rename helper**

Create `crates/shoebox-server/src/upgrade.rs`:

```rust
//! Pre-startup upgrade helpers. Currently the only one is renaming the
//! legacy `catalog.db` from before sub-1-3-5 (when shoebox-server wrote
//! to that file directly instead of going through sqld).

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// If `<data_dir>/catalog.db` exists, rename it to
/// `<data_dir>/catalog.db.legacy-pre-grpc-fix-<unix_ts>` and log a
/// `WARN catalog.legacy.renamed` event. Idempotent — does nothing if
/// the file is absent.
///
/// We rename rather than delete so an operator can manually inspect or
/// recover anything they care about. The renamed file is otherwise
/// unused by the new code path; it can be deleted at the operator's
/// discretion.
pub fn rename_legacy_catalog_db(data_dir: &Path) -> Result<()> {
    let legacy = data_dir.join("catalog.db");
    if !legacy.exists() {
        return Ok(());
    }
    let unix_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("system clock before epoch: {e}"))?
        .as_secs();
    let renamed = data_dir.join(format!("catalog.db.legacy-pre-grpc-fix-{unix_ts}"));
    std::fs::rename(&legacy, &renamed)
        .with_context(|| format!("renaming {} → {}", legacy.display(), renamed.display()))?;
    tracing::warn!(
        event = "catalog.legacy.renamed",
        from = %legacy.display(),
        to = %renamed.display(),
        "found pre-sub-1-3-5 catalog.db; renamed (sqld is now the single source of truth)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn no_legacy_file_is_a_noop() {
        let dir = TempDir::new().unwrap();
        rename_legacy_catalog_db(dir.path()).unwrap();
        // Nothing created.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn existing_catalog_db_is_renamed() {
        let dir = TempDir::new().unwrap();
        let legacy = dir.path().join("catalog.db");
        std::fs::write(&legacy, b"old-data").unwrap();
        rename_legacy_catalog_db(dir.path()).unwrap();
        assert!(!legacy.exists(), "legacy file should be gone");
        let renamed_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("catalog.db.legacy-pre-grpc-fix-")
            })
            .count();
        assert_eq!(renamed_count, 1, "expected exactly one renamed file");
    }
}
```

- [ ] **Step 2: Re-export the module**

Add to `crates/shoebox-server/src/lib.rs`:

```rust
pub mod upgrade;
```

- [ ] **Step 3: Run the unit tests**

```bash
cargo test -p shoebox-server upgrade -- --nocapture
```

Expected: 2 tests pass.

- [ ] **Step 4: Wire the upgrade helper into `main.rs::serve_main`**

After Task 3, `serve_main` looks like:

```rust
std::fs::create_dir_all(&cfg.data_dir)?;

let embedded_sqld = sqld_embed::start(cfg.data_dir.clone()).await?;
let db = Arc::new(db::Db::open(&embedded_sqld.local_url).await?);
// ...
```

Insert the upgrade call **between** `create_dir_all` and `sqld_embed::start` — the rename must happen before sqld touches the data dir:

```rust
std::fs::create_dir_all(&cfg.data_dir)?;

// Pre-startup migration from pre-sub-1-3-5 layout.
upgrade::rename_legacy_catalog_db(&cfg.data_dir)?;

let embedded_sqld = sqld_embed::start(cfg.data_dir.clone()).await?;
let db = Arc::new(db::Db::open(&embedded_sqld.local_url).await?);
// ...
```

Also add the import at the top of `main.rs`:

```rust
use shoebox_server::{/* ... existing ... */, upgrade};
```

(If `main.rs` already uses the `use shoebox_server::{...};` blanket import, just add `upgrade` to the list. Match the existing style — `main.rs` may import each module on its own `use` line; if so, follow that style.)

- [ ] **Step 5: Build the binary**

```bash
cargo build --release -p shoebox-server
```

Expected: success.

- [ ] **Step 6: Rebuild the docker image and boot**

```bash
docker compose -f D:/shoebox/.local-run/docker-compose.yml --project-directory D:/shoebox/.local-run -p shoebox-local down -v
docker build -t shoebox-server:dev D:/shoebox
docker compose -f D:/shoebox/.local-run/docker-compose.yml --project-directory D:/shoebox/.local-run -p shoebox-local up -d
sleep 3
docker logs --tail 20 shoebox-server-local
```

Expected logs include (in this order):
1. `sqld.spawn` with both `local_url` and `local_grpc_url`
2. `migration.apply` events for versions 1–7 (now run through sqld)
3. `secret.generated` (since we wiped the volume)
4. `https.listen.public addr=0.0.0.0:9000`
5. `health server bound addr=0.0.0.0:9001`

If migrations fail with a libsql/Hrana error, that's the empirical answer to "does sqld v0.24's Hrana HTTP API accept the migration SQL?" — investigate, likely a pragma or trigger issue.

- [ ] **Step 7: Health check**

```bash
docker exec shoebox-server-local wget -qO- http://127.0.0.1:9001/health
```

Expected: `{"status":"ok","schema_version":7}` (or whatever the latest schema version is).

- [ ] **Step 8: Commit**

```bash
git add crates/shoebox-server/src/upgrade.rs \
        crates/shoebox-server/src/lib.rs \
        crates/shoebox-server/src/main.rs
GIT_AUTHOR_NAME="Christian Huening" GIT_AUTHOR_EMAIL="christianhuening@posteo.de" \
GIT_COMMITTER_NAME="Christian Huening" GIT_COMMITTER_EMAIL="christianhuening@posteo.de" \
git commit -m "feat(server): reorder startup for sqld-first + rename legacy catalog.db

main.rs serve_main now spawns sqld before opening the Db, so the Db
talks to libsql's remote backend (not a local file). On startup, any
existing <data_dir>/catalog.db from before sub-1-3-5 is renamed to
catalog.db.legacy-pre-grpc-fix-<unix_ts> and a WARN is logged. The
rename is preserved (not deleted) so an operator can recover manually.

Unit tests in src/upgrade.rs cover both the noop and rename paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Verify `replica_e2e.rs` round-trips end-to-end

The existing test is now the canonical regression gate. With sqld installed and the Db rewrite landed, it should pass.

**Files:** none modified (this task is verification).

- [ ] **Step 1: Run the test against a real sqld**

```bash
cargo test -p shoebox-client --test replica_e2e -- --nocapture
```

Expected: the test executes (not skipped), opens a sqld via TestDb, seeds two users + one photo via Db, enrolls a client, opens the replica, syncs, sees the seeded data (3 users including the enrolled one, 1 photo), writes a client-side user "Cara", re-syncs, and reads "Cara" back on the server side. Final line: `test replica_round_trips_writes_back_to_server ... ok`.

If the test fails:
- Diagnose by re-running with `RUST_LOG=info,libsql=debug,libsql_replication=debug,shoebox_server=debug` set.
- Likely failure modes: migration SQL incompatible with Hrana (investigate the failing migration), gRPC handshake failure (re-check the spike code in proxy.rs), seeded data not visible (investigate Db's connection — is it actually going through sqld?).

- [ ] **Step 2: Run the other sqld-gated tests too**

```bash
cargo test -p shoebox-client --test library_view_e2e -- --nocapture
cargo test -p shoebox-client --test library_lock_e2e -- --nocapture
cargo test -p shoebox-server --test proxy_e2e -- --nocapture
cargo test -p shoebox-server --test locks_e2e -- --nocapture
```

Expected: all pass.

- [ ] **Step 3: Run the full suite**

```bash
cargo test --workspace --all-targets --features shoebox-server/test-helpers
```

Expected: green.

- [ ] **Step 4: No commit — this is verification only**

If steps 1–3 pass, move on. If anything fails, **stop and investigate** before proceeding. The plan assumes the design is sound; a failure here means a design defect that needs to be re-spec'd, not a code-level fix.

---

## Task 6: End-to-end manual verification through the Iced wizard

This catches anything tests miss — the actual UI flow with a real Docker container.

**Files:** none.

- [ ] **Step 1: Wipe local state for a true first-run experience**

```bash
docker compose -f D:/shoebox/.local-run/docker-compose.yml --project-directory D:/shoebox/.local-run -p shoebox-local down -v
```

Also clear the client's stored cert + config (Windows):
```powershell
Remove-Item -Recurse -Force "$env:APPDATA\shoebox-client" -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\shoebox-client" -ErrorAction SilentlyContinue
```
And delete the keychain entry: open **Credential Manager** → Windows Credentials → look for `shoebox-client-*` entries → Remove.

- [ ] **Step 2: Boot the server**

```bash
docker compose -f D:/shoebox/.local-run/docker-compose.yml --project-directory D:/shoebox/.local-run -p shoebox-local up -d
sleep 3
docker logs --tail 20 shoebox-server-local | grep -E "secret\.|sqld\.spawn"
```

Note the `SHOEBOX_SECRET` from `.env` (it's the same one across runs since we set it explicitly).

- [ ] **Step 3: Run the client**

```bash
RUST_LOG=info,libsql=debug,libsql_replication=debug cargo run -p shoebox-client
```

A GUI window opens. Drive the wizard:
1. Discovery screen → Manual entry: `https://127.0.0.1:9000`, name `local`, Add.
2. Click the server entry.
3. EnterSecret screen → paste the secret from `.env`, display name `Christian` (or whatever), submit.
4. ProfilePicker screen should appear with one user (the display name you just entered, which was created server-side during enroll).
5. Click that user.
6. **Library screen should appear, empty.**

Expected: no hang on EnrollProgress; no hang on ProfilePicker; Library screen loads.

- [ ] **Step 4: Confirm cross-direction round-trip**

The fact that the enrolled user appears on the **ProfilePicker** screen (Step 3) is itself the proof that server-side writes (the `/enroll` handler's `INSERT INTO users`) are now visible on the client's replica — exactly the round-trip that was broken before this plan. No separate sqld query needed.

If the ProfilePicker is empty, something is wrong. Most likely cause: the Db is still writing somewhere other than sqld (re-verify Task 3 landed cleanly) or the client's replica isn't syncing (check `docker logs` for proxy errors).

- [ ] **Step 5: Close the client and re-run it**

```bash
# Close the window first.
RUST_LOG=info cargo run -p shoebox-client
```

Expected: skips the wizard (cert is in Credential Manager + client.toml has server_url), goes straight to Library.

- [ ] **Step 6: No commit — verification only**

---

## Task 7: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the sub-project table**

The current entry for sub-project #1 says "Plans 1.1–1.5 implemented. Sub-project complete." Add a sub-row or note:

```markdown
| 1 | **Catalog, sync & stack** | Plans 1.1–1.5 + Plan 1.3.5 (replica gRPC + single-source-of-truth) implemented. Sub-project complete. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md), [1.3.5 spec](docs/superpowers/specs/2026-05-18-sub-1-3-5-replica-grpc-and-single-source-of-truth-design.md) |
```

- [ ] **Step 2: Remove the inaccurate "two writers" risk note**

In the "Known limitations (Plan 1.3+1.4+1.4b v1)" section, find the bullet:

```markdown
- **Two writers to `catalog.db`.** The migration runner (`Db`) and the spawned `sqld` subprocess both hold the same SQLite file. ...
```

Delete it (and update the section title if appropriate).

- [ ] **Step 3: Update the "Implementation status" section's server bullet**

Find the `crates/shoebox-server` bullet and update the libSQL line:

```markdown
- libSQL embedded `sqld` subprocess + mTLS-protected wire proxy on `/v1/*` and `/v2/*`
  - Spawns sqld with both `--http-listen-addr` (Hrana HTTP) and `--grpc-listen-addr`
    (libSQL replication). Proxy branches by Content-Type: gRPC traffic forwards to
    sqld's grpc port via an HTTP/2-only hyper client (with `/v1`/`/v2` prefix stripped);
    Hrana traffic forwards to sqld's http port unchanged. ALPN advertises `h2 + http/1.1`.
  - All server-side writes route through sqld via libsql's remote backend
    (`Db` uses `Builder::new_remote`). Single SQLite db owned by sqld backs both
    server writes and client-side replicas.
```

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
GIT_AUTHOR_NAME="Christian Huening" GIT_AUTHOR_EMAIL="christianhuening@posteo.de" \
GIT_COMMITTER_NAME="Christian Huening" GIT_COMMITTER_EMAIL="christianhuening@posteo.de" \
git commit -m "docs(claude.md): record sub-1-3-5 completion + drop inaccurate two-writers note

The 'two writers to catalog.db' wording was wrong — the two processes
were writing to different databases (catalog.db vs sqld's own data
dir). Sub-1-3-5 consolidated all writes through sqld; both the gap and
the misnomer are now gone.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Checklist (post-implementation)

After landing all tasks, the implementer should:

- [ ] All commits pushed; CI green on the test job (with sqld now installed).
- [ ] `cargo test --workspace --all-targets --features shoebox-server/test-helpers` is green locally.
- [ ] First-run wizard reaches the Library screen end-to-end (Task 6).
- [ ] No `catalog.db` exists in `<data_dir>` after server startup — only `catalog.db.legacy-pre-grpc-fix-*` (if upgrading) and `sqld/dbs/...`.
- [ ] CLAUDE.md no longer mentions "two writers to catalog.db" as a known limitation.

---

## Out of scope (carried over from the spec)

- `deploy/compose/.env.example` `SHOEBOX_PHOTOS_DIR` overload bug — separate spec.
- OS-specific dev-setup scripts (the gap that prompted this session) — separate spec.
- `crates/shoebox-server/src/ca.rs` Windows portability (`std::os::unix::fs`) — separate spec.
- Switching the proxy to a tonic-server vs. forwarding to sqld's gRPC — current "thin proxy" is good enough.

---

## Done criteria

- The client wizard reaches the Library screen and shows the user created during enrollment.
- `cargo test --workspace --all-targets --features shoebox-server/test-helpers` is green.
- CI is green on the next PR/push.
- A pre-existing `<data_dir>/catalog.db` is renamed (not deleted) on first startup after upgrade.
- `crates/shoebox-client/tests/replica_e2e.rs` actually runs in CI and asserts seeded server-side data is visible on the client's replica.
