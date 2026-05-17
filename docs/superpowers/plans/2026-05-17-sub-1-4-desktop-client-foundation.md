# shoebox-client Foundation Implementation Plan (Plan 1.4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up `shoebox-client` — an Iced desktop app that completes the §7.6 first-run wizard (mDNS discovery → secret prompt → enrollment → profile picker → library), persists its mTLS cert in the OS keychain, opens a libSQL embedded replica through the server's mTLS proxy, and stays connected with background catchup + cert renewal tickers. Lands the user in a debug "Library home" view that proves the catalog round-trips. Targets Linux, macOS, and Windows from the same source tree.

**Architecture:** A single new crate `crates/shoebox-client` with one Iced `Application` driven by a top-level `Screen` enum (state-machine shape). All non-UI work — keychain, mTLS HTTP, replica, discovery, enrollment, cert renewal — lives in standalone modules that screens call into via typed messages. Shared resources live in `Arc<RwLock<AppState>>` so background tasks (`Subscription`-driven tickers) can read/write the same state the UI sees. One small server-side addition: an unauthenticated `GET /ca-cert` endpoint so the client can pin the CA before its first TLS-validated request.

**Tech Stack:** Plan 1.3 stack plus: `iced = "0.13"` (pure-Rust UI), `keyring = "3"` (OS keychain abstraction), `directories = "5"` (per-OS config/data paths). All other deps (`libsql`, `reqwest`, `rcgen`, `mdns-sd`, `tokio`, `serde`, `toml`, `anyhow`) already in the workspace. No removals.

**Prerequisites for the implementing engineer:**

- Plans 1.1, 1.2, 1.3 complete (43 workspace tests pass, `shoebox-server` builds and serves the data plane).
- Familiarity with: tokio tasks, Rust `Arc<RwLock>` patterns, basic Iced 0.13 (Elm-style state machine — `Message` enum drives `update()`; `view()` returns `Element<Message>`).
- Spec: `docs/superpowers/specs/2026-05-17-sub-1-4-desktop-client-design.md` (commit `d2e34b3`). Read it once before starting; refer back when in doubt.

---

## File Structure

```
shoebox/
├── crates/
│   ├── shoebox-server/
│   │   └── src/
│   │       ├── ca_cert.rs                  ← NEW: GET /ca-cert handler
│   │       ├── http.rs                     ← merge ca_cert::route() into health_router
│   │       └── lib.rs                      ← add `pub mod ca_cert;`
│   └── shoebox-client/                     ← NEW crate
│       ├── Cargo.toml
│       ├── src/
│       │   ├── main.rs                     ← Iced Application; loads config, routes Screen
│       │   ├── lib.rs                      ← re-exports for integration tests
│       │   ├── app_state.rs                ← Arc<RwLock<AppState>>; replica, mTLS client, cert, config
│       │   ├── screens/
│       │   │   ├── mod.rs                  ← Screen + Message enums, transitions
│       │   │   ├── discovery.rs
│       │   │   ├── enter_secret.rs
│       │   │   ├── enroll_progress.rs
│       │   │   ├── profile_picker.rs
│       │   │   └── library.rs
│       │   ├── discovery.rs                ← mdns-sd wrapper + manual entry
│       │   ├── enrollment.rs               ← fetch_ca_cert + enroll
│       │   ├── replica.rs                  ← libsql embedded replica open + sync
│       │   ├── mtls_http.rs                ← reqwest client builder w/ client cert
│       │   ├── cert_store.rs               ← keyring + file fallback
│       │   ├── config.rs                   ← client.toml read/write
│       │   └── cert_renewal.rs             ← 12h tick + /renew on <30d
│       └── tests/
│           ├── first_run_e2e.rs            ← spawn server in-proc, drive wizard via Messages
│           ├── replica_e2e.rs              ← seeded catalog round-trip
│           └── cert_renewal_e2e.rs         ← short-lifetime cert → renewal fires
├── Cargo.toml                              ← add shoebox-client to members; iced/keyring/directories to workspace deps
├── CLAUDE.md                               ← update sub-project #1 status; add shoebox-client to layout + run instructions
└── README.md                               ← brief client run + first-run notes
```

**Responsibility split** (mirrors §4 of the spec; restated so the implementer can see file boundaries at a glance):

- `cert_store.rs` — keyring round-trip; falls back to mode-0600 file storage **only on explicit user consent** (consent surfaced via a Message from `screens/enroll_progress.rs`).
- `mtls_http.rs` — pure reqwest `Client` builder. No caching; caller holds the client.
- `discovery.rs` — `mdns-sd` browser + manual entry; emits `DiscoveredServer` events on an mpsc channel.
- `enrollment.rs` — `fetch_ca_cert(server_url)` + `enroll(server_url, ca_pem, secret, display_name)`. No I/O beyond HTTP.
- `replica.rs` — `Replica::open(data_dir, server_url, client)`, `Replica::sync()`, `Replica::conn()`.
- `config.rs` — `client.toml` schema + read/write. Default on missing.
- `cert_renewal.rs` — 12h ticker; mirrors `shoebox-server`'s `cert_renewal.rs` shape.
- `app_state.rs` — `AppState` struct + helpers for the screens.
- `screens/*` — view + message handling only. Zero business logic.
- `main.rs` — `iced::application(...)`; loads config; selects initial Screen; spawns subscriptions; threads shutdown.

---

## Task 1: Workspace + crate scaffolding

**Files:**
- Create: `crates/shoebox-client/Cargo.toml`
- Create: `crates/shoebox-client/src/main.rs`
- Create: `crates/shoebox-client/src/lib.rs`
- Modify: `Cargo.toml` (workspace)

- [ ] **Step 1: Add `shoebox-client` to the workspace members.** In `Cargo.toml` (workspace root), update:

```toml
[workspace]
resolver = "2"
members = [
    "crates/shoebox-common",
    "crates/shoebox-server",
    "crates/shoebox-client",
]
```

- [ ] **Step 2: Add new deps to `[workspace.dependencies]`.** Append:

```toml
iced = { version = "0.13", default-features = false, features = ["wgpu", "tokio", "advanced"] }
keyring = "3"
directories = "5"
futures-util = "0.3"  # already added in Plan 1.3 Task 3; ensure present, no-op if so
```

- [ ] **Step 3: Write `crates/shoebox-client/Cargo.toml`.**

```toml
[package]
name = "shoebox-client"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lib]
name = "shoebox_client"
path = "src/lib.rs"

[[bin]]
name = "shoebox-client"
path = "src/main.rs"

[lints]
workspace = true

[dependencies]
shoebox-common = { path = "../shoebox-common" }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
reqwest = { workspace = true }
libsql = { workspace = true }
rcgen = { workspace = true }
rustls = { workspace = true }
rustls-pemfile = { workspace = true }
mdns-sd = { workspace = true }
hex = { workspace = true }
time = { workspace = true }
iced = { workspace = true }
keyring = { workspace = true }
directories = { workspace = true }
futures-util = { workspace = true }
parking_lot = { workspace = true }

[dev-dependencies]
tempfile = "3"
which = "6"
```

- [ ] **Step 4: Write `crates/shoebox-client/src/main.rs`.**

```rust
//! shoebox-client binary entry point. Plan 1.4 Task 1 scaffolding;
//! later tasks replace this with the Iced Application.

fn main() {
    println!("shoebox-client (Plan 1.4 scaffolding)");
}
```

- [ ] **Step 5: Write `crates/shoebox-client/src/lib.rs`.**

```rust
//! Library facade for integration tests. The binary entry point lives
//! in `main.rs` and uses these modules directly.
//!
//! Plan 1.4 scaffolding — modules are added in subsequent tasks.
```

- [ ] **Step 6: Verify the workspace builds.**

```
cargo build -p shoebox-client
cargo build --workspace
```

Expected: both succeed. `shoebox-server` rebuild is incidental (workspace dep change).

- [ ] **Step 7: Commit (unsigned).**

```
git -c commit.gpgsign=false add Cargo.toml crates/shoebox-client
git -c commit.gpgsign=false commit -m "build(client): scaffold shoebox-client crate + workspace deps"
```

---

## Task 2: Server `GET /ca-cert` endpoint

**Files:**
- Create: `crates/shoebox-server/src/ca_cert.rs`
- Modify: `crates/shoebox-server/src/http.rs` (extend `health_router` to include `/ca-cert`)
  - Note: `/ca-cert` returns the CA PEM; it must be served from a listener the client can reach **before** it has a cert. The mTLS listener at `:9000` accepts unauthenticated requests (e.g., `/enroll`), but its TLS handshake presents the **server cert** which is signed by the same CA — the client needs the CA to validate it. We solve this by serving `/ca-cert` on the **mTLS listener** but matching `/enroll`'s pattern: the request uses `dangerous_accept_invalid_certs(true)` because the client has nowhere else to get the CA from. This is safe because the response body itself is the CA cert; if the client later validates that body against the cert it pinned, it learns whether to trust the TLS chain.
- Modify: `crates/shoebox-server/src/lib.rs` (add `pub mod ca_cert;`)
- Modify: `crates/shoebox-server/src/http.rs` — merge `ca_cert::route()` into `public_router` (NOT `health_router` — `/ca-cert` must reach the public listener so the client can hit it before having a cert).

- [ ] **Step 1: Write the failing test.** Append to `crates/shoebox-server/src/ca_cert.rs`:

```rust
//! GET /ca-cert — returns the CA cert PEM. Unauthenticated (client has
//! no cert yet at the point it needs this). Served on the public mTLS
//! listener; clients use `dangerous_accept_invalid_certs(true)` for the
//! single bootstrap request, then validate everything subsequent against
//! the CA they just received.

use axum::{extract::State, http::StatusCode, routing::get, Router};

use crate::http::AppState;

pub fn route() -> Router<AppState> {
    Router::new().route("/ca-cert", get(handler))
}

async fn handler(State(state): State<AppState>) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    (
        StatusCode::OK,
        [("Content-Type", "application/x-pem-file")],
        state.ca.root_cert_pem.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn ca_cert_returns_pem_body() {
        let tmp = TempDir::new().unwrap();
        let db = Arc::new(Db::open(&tmp.path().join("catalog.db")).await.unwrap());
        let ca = Arc::new(crate::ca::Ca::open(tmp.path()).unwrap());
        let state = AppState {
            db,
            schema_version: shoebox_common::SCHEMA_VERSION,
            ca: ca.clone(),
            sqld_url: "http://127.0.0.1:0".to_string(),
            cache_dir: tmp.path().to_path_buf(),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let app = Router::new().merge(route()).with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { let _ = rx.await; })
                .await.unwrap();
        });

        let resp = reqwest::get(format!("http://{addr}/ca-cert")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("-----BEGIN CERTIFICATE-----"));
        assert_eq!(body, ca.root_cert_pem);

        let _ = tx.send(());
        let _ = server.await;
    }
}
```

- [ ] **Step 2: Add `pub mod ca_cert;` to `crates/shoebox-server/src/lib.rs`.** Alphabetical position: between `ca` and `cert_renewal`.

- [ ] **Step 3: Wire `ca_cert::route()` into `public_router` in `crates/shoebox-server/src/http.rs`.** Replace the existing `public_router`:

```rust
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::ca_cert::route())
        .merge(crate::enroll::route())
        .merge(crate::enroll::renew_route())
        .merge(crate::whoami::route())
        .merge(crate::proxy::routes())
        .merge(crate::thumbs_http::routes())
        .merge(crate::locks_http::routes())
        .with_state(state)
}
```

- [ ] **Step 4: Run.**

```
cargo test -p shoebox-server ca_cert
cargo test -p shoebox-server
cargo clippy -p shoebox-server --all-targets -- -D warnings
cargo fmt --all
```

Expected: 44 server tests pass (43 prior + 1 new `ca_cert_returns_pem_body`). Clippy clean.

- [ ] **Step 5: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-server/src/ca_cert.rs crates/shoebox-server/src/lib.rs crates/shoebox-server/src/http.rs
git -c commit.gpgsign=false commit -m "feat(server): GET /ca-cert returns CA PEM for client bootstrap"
```

---

## Task 3: `config` module

**Files:**
- Create: `crates/shoebox-client/src/config.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

- [ ] **Step 1: Write the failing tests first.** Append to `crates/shoebox-client/src/config.rs`:

```rust
//! Client configuration persisted to `client.toml` under the OS's
//! per-user config directory (via `directories::ProjectDirs`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ClientConfig {
    /// `https://host:port` of the paired shoebox-server. Empty string
    /// means first-run.
    #[serde(default)]
    pub server_url: String,
    /// Hex-encoded serial of the client cert in keychain. Empty string
    /// means no cert yet.
    #[serde(default)]
    pub cert_serial_hex: String,
    /// `users.id` of the user last picked in the profile picker.
    #[serde(default)]
    pub last_active_user_id: Option<String>,
}

impl ClientConfig {
    /// True if the client has never completed first-run.
    #[must_use]
    pub fn is_first_run(&self) -> bool {
        self.server_url.is_empty() || self.cert_serial_hex.is_empty()
    }

    /// Read the config from `path`, or return defaults if the file is missing.
    ///
    /// # Errors
    /// Returns an error only on read or parse failure of an existing file.
    pub fn read_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(toml_text) => toml::from_str(&toml_text)
                .with_context(|| format!("parsing client config {}", path.display())),
            Err(read_err) if read_err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(read_err) => Err(read_err).context("reading client config"),
        }
    }

    /// Atomic write: serialize to a sibling `.tmp` file, then rename.
    ///
    /// # Errors
    /// Returns an error if the parent directory can't be created, or if
    /// any of the write / rename steps fail.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        if let Some(parent_dir) = path.parent() {
            std::fs::create_dir_all(parent_dir)
                .with_context(|| format!("creating config dir {}", parent_dir.display()))?;
        }
        let toml_text = toml::to_string_pretty(self).context("serializing client config")?;
        let temp_path = path.with_extension("toml.tmp");
        std::fs::write(&temp_path, toml_text)
            .with_context(|| format!("writing {}", temp_path.display()))?;
        std::fs::rename(&temp_path, path)
            .with_context(|| format!("renaming {} -> {}", temp_path.display(), path.display()))?;
        Ok(())
    }
}

/// Returns the canonical location of `client.toml` for this user, based
/// on `directories::ProjectDirs`. Returns `None` if the directories
/// crate can't determine a config dir (extremely rare; headless build).
#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("io", "shoebox", "shoebox-client")
        .map(|project_dirs| project_dirs.config_dir().join("client.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let cfg = ClientConfig::read_from(&tmp.path().join("absent.toml")).unwrap();
        assert!(cfg.is_first_run());
        assert!(cfg.server_url.is_empty());
        assert!(cfg.last_active_user_id.is_none());
    }

    #[test]
    fn round_trip_full_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("client.toml");
        let written = ClientConfig {
            server_url: "https://nas.local:9000".to_string(),
            cert_serial_hex: "abc123".to_string(),
            last_active_user_id: Some("user-1".to_string()),
        };
        written.write_to(&path).unwrap();
        let read_back = ClientConfig::read_from(&path).unwrap();
        assert_eq!(read_back.server_url, written.server_url);
        assert_eq!(read_back.cert_serial_hex, written.cert_serial_hex);
        assert_eq!(read_back.last_active_user_id, written.last_active_user_id);
        assert!(!read_back.is_first_run());
    }

    #[test]
    fn partial_file_missing_optional_field() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("partial.toml");
        std::fs::write(
            &path,
            r#"
            server_url = "https://x:9000"
            cert_serial_hex = "deadbeef"
            "#,
        ).unwrap();
        let cfg = ClientConfig::read_from(&path).unwrap();
        assert!(!cfg.is_first_run());
        assert!(cfg.last_active_user_id.is_none());
    }

    #[test]
    fn is_first_run_true_when_either_field_empty() {
        assert!(ClientConfig::default().is_first_run());
        assert!(ClientConfig { server_url: "x".into(), ..Default::default() }.is_first_run());
        assert!(ClientConfig { cert_serial_hex: "x".into(), ..Default::default() }.is_first_run());
    }
}
```

- [ ] **Step 2: Add `pub mod config;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-client config
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/config.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): client.toml read/write with atomic file rename"
```

---

## Task 4: `cert_store` — keyring path

**Files:**
- Create: `crates/shoebox-client/src/cert_store.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

- [ ] **Step 1: Write the module.**

```rust
//! Per-server client cert + key storage. Default backend is the OS
//! keychain (via the `keyring` crate); explicit-consent fallback to a
//! mode-0600 file under the OS app-data dir is added in Task 5.
//!
//! Keying: each (server_url, kind) pair gets its own keychain entry.
//! That way one client paired with multiple servers keeps cert sets
//! separate, and the cert ↔ key are stored as siblings.

use anyhow::{anyhow, Context, Result};

const SERVICE_PREFIX: &str = "shoebox-client";

/// Identifies which half of a cert pair an entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Cert,
    Key,
}

impl EntryKind {
    fn suffix(self) -> &'static str {
        match self {
            EntryKind::Cert => "cert",
            EntryKind::Key => "key",
        }
    }
}

fn service_name(server_url: &str, kind: EntryKind) -> String {
    format!("{SERVICE_PREFIX}::{}::{}", kind.suffix(), server_url)
}

fn keyring_entry(server_url: &str, kind: EntryKind) -> Result<keyring::Entry> {
    let service = service_name(server_url, kind);
    keyring::Entry::new(&service, "default-user")
        .with_context(|| format!("opening keyring entry for {service}"))
}

/// Store the (cert_pem, key_pem) pair for `server_url` in the OS keychain.
///
/// # Errors
/// Returns the underlying keyring error if either write fails. On failure
/// of the second write, the first write is rolled back (best-effort delete).
pub fn store_in_keyring(server_url: &str, cert_pem: &str, key_pem: &str) -> Result<()> {
    let cert_entry = keyring_entry(server_url, EntryKind::Cert)?;
    cert_entry.set_password(cert_pem).context("writing cert to keyring")?;

    let key_entry = keyring_entry(server_url, EntryKind::Key)?;
    if let Err(key_err) = key_entry.set_password(key_pem) {
        let _ = cert_entry.delete_credential();
        return Err(anyhow!("writing key to keyring: {key_err}"));
    }
    Ok(())
}

/// Load the (cert_pem, key_pem) pair for `server_url` from the OS keychain,
/// or `None` if no entry exists.
///
/// # Errors
/// Returns an error only on backend failure (not on "entry missing", which
/// returns `Ok(None)`).
pub fn load_from_keyring(server_url: &str) -> Result<Option<(String, String)>> {
    let cert_pem = match keyring_entry(server_url, EntryKind::Cert)?.get_password() {
        Ok(pem) => pem,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(other) => return Err(anyhow!("reading cert from keyring: {other}")),
    };
    let key_pem = match keyring_entry(server_url, EntryKind::Key)?.get_password() {
        Ok(pem) => pem,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(other) => return Err(anyhow!("reading key from keyring: {other}")),
    };
    Ok(Some((cert_pem, key_pem)))
}

/// Delete the cert + key entries for `server_url` from the OS keychain.
/// Missing entries are not an error.
///
/// # Errors
/// Returns the first non-`NoEntry` backend error encountered.
pub fn delete_from_keyring(server_url: &str) -> Result<()> {
    for kind in [EntryKind::Cert, EntryKind::Key] {
        match keyring_entry(server_url, kind)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(delete_err) => return Err(anyhow!("deleting {kind:?} from keyring: {delete_err}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skip-if-no-secret-service helper. Some CI Linux runners have no
    /// Secret Service backend; those env runs print "skipping" and return.
    fn skip_if_no_backend() -> bool {
        // `keyring` returns PlatformFailure on Linux when no service is
        // present. We probe with a throwaway entry; if it fails, skip.
        let probe = keyring::Entry::new("shoebox-test-probe", "probe-user");
        if probe.is_err() {
            eprintln!("skipping: keyring backend unavailable");
            return true;
        }
        let entry = probe.unwrap();
        let probe_result = entry.set_password("probe").and_then(|()| entry.delete_credential());
        if probe_result.is_err() {
            eprintln!("skipping: keyring backend present but non-functional");
            return true;
        }
        false
    }

    #[test]
    fn round_trip_via_keyring() {
        if skip_if_no_backend() { return; }
        let server_url = format!("https://test-{}.local:9000", uuid_like());
        let cert = "-----BEGIN CERTIFICATE-----\nfake-cert\n-----END CERTIFICATE-----\n";
        let key = "-----BEGIN PRIVATE KEY-----\nfake-key\n-----END PRIVATE KEY-----\n";

        store_in_keyring(&server_url, cert, key).unwrap();
        let loaded = load_from_keyring(&server_url).unwrap().expect("entry should exist");
        assert_eq!(loaded.0, cert);
        assert_eq!(loaded.1, key);

        delete_from_keyring(&server_url).unwrap();
        let after_delete = load_from_keyring(&server_url).unwrap();
        assert!(after_delete.is_none());
    }

    #[test]
    fn load_missing_returns_none() {
        if skip_if_no_backend() { return; }
        let server_url = format!("https://nonexistent-{}.local:9000", uuid_like());
        assert!(load_from_keyring(&server_url).unwrap().is_none());
    }

    #[test]
    fn delete_missing_is_ok() {
        if skip_if_no_backend() { return; }
        let server_url = format!("https://also-missing-{}.local:9000", uuid_like());
        delete_from_keyring(&server_url).unwrap();
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{nanos:x}")
    }
}
```

- [ ] **Step 2: Add `pub mod cert_store;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-client cert_store
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: 3 tests pass (or skip on a headless Linux runner with no Secret Service).

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/cert_store.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): per-server cert+key storage via OS keychain"
```

---

## Task 5: `cert_store` — file fallback

**Files:**
- Modify: `crates/shoebox-client/src/cert_store.rs`

The keychain is the default; the file fallback is opt-in (the user explicitly chose it in the `screens/enroll_progress.rs` flow). Functions go side-by-side with the keyring ones — callers pick which to use based on the consent message.

- [ ] **Step 1: Append to `cert_store.rs`.**

```rust
use std::path::{Path, PathBuf};

const FILE_CERT_NAME: &str = "client.cert.pem";
const FILE_KEY_NAME: &str = "client.key.pem";

/// Returns the directory under which `store_in_file` / `load_from_file`
/// place the cert + key files for `server_url`. Hashes the URL into the
/// filename so multiple servers don't collide.
fn file_storage_dir(server_url: &str) -> Option<PathBuf> {
    let project_dirs = directories::ProjectDirs::from("io", "shoebox", "shoebox-client")?;
    let server_slug = hex::encode(blake3::hash(server_url.as_bytes()).as_bytes());
    Some(project_dirs.data_local_dir().join("certs").join(server_slug))
}

/// Store (cert, key) on disk under the app-data dir with mode 0600 on Unix.
/// Caller has already consented to file storage (e.g., keychain unavailable).
///
/// # Errors
/// Returns an error on directory creation, file write, or permission set
/// failure.
pub fn store_in_file(server_url: &str, cert_pem: &str, key_pem: &str) -> Result<()> {
    let storage_dir = file_storage_dir(server_url)
        .ok_or_else(|| anyhow!("could not determine app-data dir"))?;
    std::fs::create_dir_all(&storage_dir)
        .with_context(|| format!("creating {}", storage_dir.display()))?;

    let cert_path = storage_dir.join(FILE_CERT_NAME);
    let key_path = storage_dir.join(FILE_KEY_NAME);
    write_with_mode_0600(&cert_path, cert_pem)?;
    write_with_mode_0600(&key_path, key_pem)?;
    Ok(())
}

/// Load (cert, key) from the file-storage dir, or `None` if not present.
///
/// # Errors
/// Returns an error only on read failure of an existing file (missing
/// files yield `Ok(None)`).
pub fn load_from_file(server_url: &str) -> Result<Option<(String, String)>> {
    let storage_dir = file_storage_dir(server_url)
        .ok_or_else(|| anyhow!("could not determine app-data dir"))?;
    let cert_path = storage_dir.join(FILE_CERT_NAME);
    let key_path = storage_dir.join(FILE_KEY_NAME);
    if !cert_path.exists() || !key_path.exists() {
        return Ok(None);
    }
    let cert_pem = std::fs::read_to_string(&cert_path)
        .with_context(|| format!("reading {}", cert_path.display()))?;
    let key_pem = std::fs::read_to_string(&key_path)
        .with_context(|| format!("reading {}", key_path.display()))?;
    Ok(Some((cert_pem, key_pem)))
}

/// Delete the file-stored cert + key for `server_url`. Missing files are OK.
///
/// # Errors
/// Returns an error only on filesystem failure other than `NotFound`.
pub fn delete_from_file(server_url: &str) -> Result<()> {
    let Some(storage_dir) = file_storage_dir(server_url) else {
        return Ok(());
    };
    for filename in [FILE_CERT_NAME, FILE_KEY_NAME] {
        let target = storage_dir.join(filename);
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(remove_err) if remove_err.kind() == std::io::ErrorKind::NotFound => {}
            Err(remove_err) => {
                return Err(remove_err).with_context(|| format!("deleting {}", target.display()));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn write_with_mode_0600(path: &Path, body: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_with_mode_0600(path: &Path, body: &str) -> Result<()> {
    // On Windows, ACL the file to the current user. For v1 we rely on
    // the per-user data dir already being protected; Plan 1.4b can
    // tighten with a proper SDDL ACL.
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}
```

- [ ] **Step 2: Add file-fallback tests inside the existing `#[cfg(test)] mod tests` block.**

```rust
    #[test]
    fn file_storage_round_trip() {
        let server_url = format!("https://file-test-{}.local:9000", uuid_like());
        let cert = "fake-cert-bytes";
        let key = "fake-key-bytes";

        store_in_file(&server_url, cert, key).unwrap();
        let loaded = load_from_file(&server_url).unwrap().unwrap();
        assert_eq!(loaded.0, cert);
        assert_eq!(loaded.1, key);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let storage_dir = file_storage_dir(&server_url).unwrap();
            let cert_path = storage_dir.join(FILE_CERT_NAME);
            let mode = std::fs::metadata(&cert_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "cert file must be mode 0600, got {mode:o}");
        }

        delete_from_file(&server_url).unwrap();
        assert!(load_from_file(&server_url).unwrap().is_none());
    }

    #[test]
    fn file_load_missing_returns_none() {
        let server_url = format!("https://file-absent-{}.local:9000", uuid_like());
        assert!(load_from_file(&server_url).unwrap().is_none());
    }
```

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-client cert_store
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: 5 tests pass total (3 keyring + 2 file).

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/cert_store.rs
git -c commit.gpgsign=false commit -m "feat(client): explicit-consent file fallback for cert storage"
```

---

## Task 6: `mtls_http` module

**Files:**
- Create: `crates/shoebox-client/src/mtls_http.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

- [ ] **Step 1: Write the module + tests.**

```rust
//! Build a `reqwest::Client` configured for mTLS against the paired
//! shoebox-server. Pure builder; caller owns the client and caches it
//! in `AppState`.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use std::time::Duration;

/// Build a `reqwest::Client` that:
///   - validates the server cert against `root_cert_pem` (the CA PEM
///     returned by `GET /ca-cert`);
///   - presents `(client_cert_pem, client_key_pem)` for mTLS auth;
///   - times out at 30 s; pool stays small.
///
/// # Errors
/// Returns an error if any PEM is malformed or rustls rejects the config.
pub fn build_mtls_client(
    root_cert_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> Result<Client> {
    let root_store = build_root_store(root_cert_pem)?;
    let client_cert_chain = parse_cert_chain(client_cert_pem)?;
    let client_key = parse_private_key(client_key_pem)?;

    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(client_cert_chain, client_key)
        .context("building rustls client auth config")?;

    Client::builder()
        .use_preconfigured_tls(tls_config)
        .pool_max_idle_per_host(2)
        .timeout(Duration::from_secs(30))
        .build()
        .context("building reqwest mtls client")
}

/// Build a `reqwest::Client` that validates the server cert against
/// `root_cert_pem` but presents no client cert. Used during the bootstrap
/// step after `/ca-cert` returns and before `/enroll` runs.
///
/// # Errors
/// Returns an error if the root PEM is malformed.
pub fn build_unauth_pinned_client(root_cert_pem: &str) -> Result<Client> {
    let root_store = build_root_store(root_cert_pem)?;
    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Client::builder()
        .use_preconfigured_tls(tls_config)
        .pool_max_idle_per_host(1)
        .timeout(Duration::from_secs(30))
        .build()
        .context("building reqwest unauth-pinned client")
}

fn build_root_store(root_cert_pem: &str) -> Result<RootCertStore> {
    let mut root_store = RootCertStore::empty();
    let mut cursor = root_cert_pem.as_bytes();
    for cert_result in rustls_pemfile::certs(&mut cursor) {
        let cert_der = cert_result.context("parsing CA cert PEM")?;
        root_store.add(cert_der).context("adding CA cert to root store")?;
    }
    if root_store.is_empty() {
        return Err(anyhow!("no certificates found in CA PEM"));
    }
    Ok(root_store)
}

fn parse_cert_chain(cert_pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut cursor = cert_pem.as_bytes();
    let mut chain = Vec::new();
    for cert_result in rustls_pemfile::certs(&mut cursor) {
        chain.push(cert_result.context("parsing client cert PEM")?);
    }
    if chain.is_empty() {
        return Err(anyhow!("no certificates found in client PEM"));
    }
    Ok(chain)
}

fn parse_private_key(key_pem: &str) -> Result<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cursor = key_pem.as_bytes();
    while let Some(item_result) = rustls_pemfile::read_one(&mut cursor).transpose() {
        let item = item_result.context("parsing client key PEM")?;
        match item {
            Item::Pkcs8Key(k) => return Ok(PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Ok(PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Ok(PrivateKeyDer::Sec1(k)),
            _ => continue,
        }
    }
    Err(anyhow!("no private key found in key PEM"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{generate_simple_self_signed, CertifiedKey};

    fn fresh_cert_pair() -> (String, String) {
        let CertifiedKey { cert, key_pair } =
            generate_simple_self_signed(vec!["test.local".to_string()]).unwrap();
        (cert.pem(), key_pair.serialize_pem())
    }

    #[test]
    fn build_unauth_client_with_valid_ca() {
        let (ca_pem, _key) = fresh_cert_pair();
        build_unauth_pinned_client(&ca_pem).unwrap();
    }

    #[test]
    fn build_unauth_client_rejects_garbage_ca() {
        let err = build_unauth_pinned_client("not a pem").unwrap_err();
        assert!(err.to_string().contains("no certificates"), "got: {err}");
    }

    #[test]
    fn build_mtls_client_with_valid_inputs() {
        let (ca_pem, _ca_key) = fresh_cert_pair();
        let (client_cert, client_key) = fresh_cert_pair();
        build_mtls_client(&ca_pem, &client_cert, &client_key).unwrap();
    }

    #[test]
    fn build_mtls_client_rejects_garbage_key() {
        let (ca_pem, _ca_key) = fresh_cert_pair();
        let (client_cert, _) = fresh_cert_pair();
        let err = build_mtls_client(&ca_pem, &client_cert, "garbage").unwrap_err();
        assert!(err.to_string().contains("no private key"), "got: {err}");
    }
}
```

- [ ] **Step 2: Add `pub mod mtls_http;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-client mtls_http
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/mtls_http.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): build_mtls_client + build_unauth_pinned_client"
```

---

## Task 7: `enrollment` module

**Files:**
- Create: `crates/shoebox-client/src/enrollment.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

The module has two public async fns: `fetch_ca_cert` (the dangerous bootstrap that hits `/ca-cert` with cert validation disabled) and `enroll` (validates the CA, generates a CSR, calls `/enroll`).

- [ ] **Step 1: Write the module.**

```rust
//! Bootstrap + enrollment HTTP calls.
//!
//! `fetch_ca_cert` is intentionally unauthenticated and disables TLS
//! validation — the client has no CA to pin yet. The first thing it
//! does on success is hand the CA PEM to subsequent calls so they CAN
//! validate. The trust boundary is parent-spec §7.7 (LAN-trusted).
//!
//! `enroll` validates the server cert chain against the CA PEM that
//! `fetch_ca_cert` returned, generates an Ed25519 keypair + CSR via
//! `rcgen`, POSTs `/enroll`, and returns the parsed response.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::mtls_http;

/// Parsed shape of `/enroll`'s response (mirrors `shoebox-server`'s
/// `EnrollResponse`).
#[derive(Debug, Deserialize)]
pub struct EnrollResult {
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub user_id: String,
    pub machine_id: String,
    pub cert_serial_hex: String,
    pub not_after_unix: i64,
    /// Filled in client-side after the response is parsed (rcgen produces
    /// the key locally and never sends it over the wire).
    #[serde(skip)]
    pub client_key_pem: String,
}

#[derive(Debug, Serialize)]
struct EnrollRequest<'a> {
    shared_secret: &'a str,
    csr_pem: String,
    display_name: &'a str,
}

/// Errors that callers in `screens/` want to discriminate on for inline
/// error messaging.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("network failure: {0}")]
    Network(String),
    #[error("invalid shared secret")]
    BadSecret,
    #[error("server returned {status}: {body}")]
    ServerError { status: u16, body: String },
    #[error("CSR generation: {0}")]
    Csr(String),
    #[error("client build: {0}")]
    Client(String),
    #[error("response parse: {0}")]
    Parse(String),
}

/// Hit `GET <server_url>/ca-cert` with TLS validation disabled. Returns
/// the CA PEM body. The caller must immediately pin it for all subsequent
/// requests.
///
/// # Errors
/// Returns an error on network failure, non-2xx response, or empty body.
pub async fn fetch_ca_cert(server_url: &str) -> Result<String> {
    let http_client = Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()
        .context("building unauth client for ca-cert bootstrap")?;
    let resp = http_client
        .get(format!("{server_url}/ca-cert"))
        .send()
        .await
        .context("GET /ca-cert")?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("ca-cert returned {status}: {body}");
    }
    let ca_pem = resp.text().await.context("reading ca-cert body")?;
    if !ca_pem.contains("-----BEGIN CERTIFICATE-----") {
        anyhow::bail!("ca-cert body does not look like a PEM cert");
    }
    Ok(ca_pem)
}

/// POST `/enroll` over a TLS-validated connection (validating against
/// `ca_pem`). Generates the keypair + CSR locally; returns the issued
/// cert plus the locally-generated key.
pub async fn enroll(
    server_url: &str,
    ca_pem: &str,
    shared_secret: &str,
    display_name: &str,
) -> Result<EnrollResult, EnrollError> {
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ED25519)
        .map_err(|csr_err| EnrollError::Csr(format!("keypair: {csr_err}")))?;
    let key_pem = key_pair.serialize_pem();

    let mut csr_params = CertificateParams::new(Vec::<String>::new())
        .map_err(|csr_err| EnrollError::Csr(format!("params: {csr_err}")))?;
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "client-csr-placeholder");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&key_pair)
        .map_err(|csr_err| EnrollError::Csr(format!("serialize: {csr_err}")))?
        .pem()
        .map_err(|csr_err| EnrollError::Csr(format!("pem: {csr_err}")))?;

    let http_client = mtls_http::build_unauth_pinned_client(ca_pem)
        .map_err(|client_err| EnrollError::Client(client_err.to_string()))?;

    let body = EnrollRequest { shared_secret, csr_pem, display_name };
    let resp = http_client
        .post(format!("{server_url}/enroll"))
        .json(&body)
        .send()
        .await
        .map_err(|net_err| EnrollError::Network(net_err.to_string()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(EnrollError::BadSecret);
    }
    if !status.is_success() {
        let response_body = resp.text().await.unwrap_or_default();
        return Err(EnrollError::ServerError { status: status.as_u16(), body: response_body });
    }
    let mut parsed: EnrollResult = resp
        .json()
        .await
        .map_err(|parse_err| EnrollError::Parse(parse_err.to_string()))?;
    parsed.client_key_pem = key_pem;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_ca_cert_rejects_non_pem_body() {
        // Spin up a tiny HTTP server that returns garbage on /ca-cert.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::AsyncWriteExt;
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut [0u8; 1024]).await;
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\nnot pem"
            ).await;
        });
        let err = fetch_ca_cert(&format!("http://{addr}")).await.unwrap_err();
        assert!(err.to_string().contains("does not look like a PEM"), "got: {err}");
        let _ = server.await;
    }
}
```

(The full `enroll()` happy-path and bad-secret paths are exercised in Task 20's `first_run_e2e.rs` against a real `shoebox-server`. Unit-stubbing reqwest here would be more boilerplate than it's worth.)

- [ ] **Step 2: Add `thiserror = "1"` to the server's workspace deps if it's not there yet** — check `Cargo.toml`'s `[workspace.dependencies]`. If absent, add:

```toml
thiserror = "1"
```

And reference in `crates/shoebox-client/Cargo.toml` `[dependencies]`:

```toml
thiserror = { workspace = true }
```

(It IS in the workspace already from Plan 1.1; just verify and reference.)

- [ ] **Step 3: Add `pub mod enrollment;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 4: Run.**

```
cargo test -p shoebox-client enrollment
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: 1 test passes.

- [ ] **Step 5: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/enrollment.rs crates/shoebox-client/src/lib.rs crates/shoebox-client/Cargo.toml
git -c commit.gpgsign=false commit -m "feat(client): fetch_ca_cert + enroll (CSR gen, /enroll POST)"
```

---

## Task 8: `replica` module

**Files:**
- Create: `crates/shoebox-client/src/replica.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

The libSQL embedded-replica client opens a local SQLite-format file and syncs against a remote URL. mTLS support is the open question: as of libsql 0.6, `Builder::new_remote_replica(url, token)` doesn't take a custom rustls config. If that's still true at implementation time, the fallback is to write through the proxy via raw Hrana over the mTLS reqwest client for v1, deferring the embedded-replica path to a follow-up.

The plan below targets the **happy path**: libsql 0.6's `Builder::new_remote_replica` accepts a callback for HTTP request customisation. If you discover the callback isn't sufficient, report DONE_WITH_CONCERNS and the controller will narrow scope to a "remote-only" replica wrapper (no local SQLite file, all queries proxied) — that still proves the data-plane round-trip the spec asks for.

- [ ] **Step 1: Write the module.**

```rust
//! Local libSQL embedded replica that syncs from `shoebox-server`'s
//! proxied sqld at `<server_url>/v1/...`.
//!
//! v1 limitation: libSQL's `new_remote_replica` does not accept a
//! pre-built reqwest client. We use its `connector` builder hook to
//! supply a rustls `ClientConfig` configured for mTLS. If that hook
//! turns out not to exist at impl time, report DONE_WITH_CONCERNS so
//! the controller can scope down.

use anyhow::{Context, Result};
use libsql::{Builder, Connection, Database};
use std::path::Path;
use std::sync::Arc;

pub struct Replica {
    database: Arc<Database>,
}

impl Replica {
    /// Open (or create) a local replica file at `local_path`, syncing
    /// against `<server_url>/v1/` over the mTLS proxy.
    ///
    /// # Errors
    /// Returns an error if the local file can't be opened or the
    /// initial connection to the remote can't be established.
    pub async fn open(
        local_path: &Path,
        server_url: &str,
        ca_pem: &str,
        client_cert_pem: &str,
        client_key_pem: &str,
    ) -> Result<Self> {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating replica dir {}", parent.display()))?;
        }
        let tls_connector = build_tls_connector(ca_pem, client_cert_pem, client_key_pem)?;
        let remote_url = format!("{server_url}/v1");
        let database = Builder::new_remote_replica(
            local_path.to_string_lossy().to_string(),
            remote_url,
            String::new(), // auth token: mTLS authenticates the connection itself
        )
        .connector(tls_connector)
        .build()
        .await
        .context("opening libsql embedded replica")?;
        Ok(Self { database: Arc::new(database) })
    }

    /// Run an incremental WAL catchup against the server. Returns the
    /// number of frames pulled.
    ///
    /// # Errors
    /// Returns an error on any sync failure (network, auth, server
    /// rejection). Caller logs + flips offline banner; doesn't crash.
    pub async fn sync(&self) -> Result<u64> {
        let replicated = self.database.sync().await.context("libsql sync")?;
        Ok(replicated.frame_no().unwrap_or(0))
    }

    /// Hand out a fresh `Connection` for queries.
    ///
    /// # Errors
    /// Returns an error on connection-creation failure.
    pub fn conn(&self) -> Result<Connection> {
        self.database.connect().context("creating libsql connection")
    }
}

fn build_tls_connector(
    ca_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> Result<libsql::replication::Connector> {
    // libsql 0.6's connector type accepts a hyper Connector. We build
    // a hyper-rustls connector pinned to ca_pem + presenting the client
    // cert. Implementer: verify the exact `Connector` API at impl time
    // — this is the most likely API-drift point.
    let _ = (ca_pem, client_cert_pem, client_key_pem);
    todo!("verify libsql 0.6 connector API at implementation time")
}
```

**IMPLEMENTER NOTE — this is the riskiest task.** Before writing the connector, inspect libsql 0.6's source for the connector API:

```
find ~/.cargo/registry/src -type d -name 'libsql-0.6*' -exec ls {}/src/replication \;
```

If `libsql::replication::Connector` exists with a `from_rustls_config(rustls::ClientConfig) -> Self`-shaped constructor (or similar), use it. If the only knob is "set an auth token header via a closure", fall through to plan B:

**Plan B (if libsql 0.6 won't accept a custom rustls config):** make `Replica` a thin wrapper around a `reqwest::Client` + the server URL, and expose `Replica::query_raw(sql, params)` that POSTs to `/v2/pipeline` (Hrana over HTTP). No local SQLite file. Queries route through the mTLS proxy as ordinary HTTPS. This loses the "embedded replica" performance benefit but proves the round-trip and is acceptable for Plan 1.4 (the polished library experience in sub-project #3 can revisit when libsql gains a richer connector hook). Report DONE_WITH_CONCERNS noting which plan you took.

- [ ] **Step 2: Add `pub mod replica;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Verify it builds.** No unit tests for this module — replica behavior is exercised by `replica_e2e.rs` (Task 21) against a real `shoebox-server`.

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/replica.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): Replica wraps libsql embedded-replica + mtls connector"
```

---

## Task 9: `discovery` module

**Files:**
- Create: `crates/shoebox-client/src/discovery.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

- [ ] **Step 1: Write the module.**

```rust
//! mDNS discovery of shoebox-server instances on the LAN, plus a
//! manual-entry path for cases where mDNS isn't available.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::sync::Arc;
use tokio::sync::mpsc;

const SERVICE_TYPE: &str = "_shoebox._tcp.local.";

#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    /// Server-friendly name from the mDNS TXT record (or the user-supplied
    /// label for a manually-entered server).
    pub display_name: String,
    /// `https://host:port` URL the client connects to.
    pub url: String,
    /// True if this entry came from the user typing a URL rather than mDNS.
    pub manual: bool,
}

pub struct Browser {
    /// Receives discovery events. Owned by the caller (the Iced
    /// subscription drains it into Messages).
    pub rx: mpsc::UnboundedReceiver<DiscoveredServer>,
    tx: mpsc::UnboundedSender<DiscoveredServer>,
    daemon: Arc<ServiceDaemon>,
}

impl Browser {
    /// Start browsing for `_shoebox._tcp.local.` services on the local
    /// network. Discovered servers stream into `rx`.
    ///
    /// # Errors
    /// Returns an error if the mDNS daemon can't be started.
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("starting mDNS daemon")?;
        let (tx, rx) = mpsc::unbounded_channel();
        let event_rx = daemon
            .browse(SERVICE_TYPE)
            .context("registering mDNS browse")?;
        let tx_for_task = tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = event_rx.recv() {
                if let ServiceEvent::ServiceResolved(info) = event {
                    let display_name = info
                        .get_property_val_str("name")
                        .unwrap_or(info.get_fullname())
                        .to_string();
                    let port = info.get_port();
                    if let Some(host_ip) = info.get_addresses().iter().next() {
                        let url = format!("https://{host_ip}:{port}");
                        if tx_for_task
                            .send(DiscoveredServer { display_name, url, manual: false })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self { rx, tx, daemon: Arc::new(daemon) })
    }

    /// Inject a manually-entered server URL as if it were discovered.
    /// `display_name` is whatever the user typed (or a default).
    pub fn add_manual(&self, display_name: &str, url: &str) {
        let _ = self.tx.send(DiscoveredServer {
            display_name: display_name.to_string(),
            url: url.to_string(),
            manual: true,
        });
    }

    /// Re-arm the browse (used by the discovery screen's Retry button).
    ///
    /// # Errors
    /// Returns an error if the daemon's re-browse fails.
    pub fn rebrowse(&self) -> Result<()> {
        let event_rx = self
            .daemon
            .browse(SERVICE_TYPE)
            .context("re-registering mDNS browse")?;
        let tx_for_task = self.tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = event_rx.recv() {
                if let ServiceEvent::ServiceResolved(info) = event {
                    let display_name = info
                        .get_property_val_str("name")
                        .unwrap_or(info.get_fullname())
                        .to_string();
                    let port = info.get_port();
                    if let Some(host_ip) = info.get_addresses().iter().next() {
                        let url = format!("https://{host_ip}:{port}");
                        if tx_for_task
                            .send(DiscoveredServer { display_name, url, manual: false })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        Ok(())
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_manual_emits_event() {
        // Note: we can't easily test the mDNS path without a real network
        // interface. The manual-entry path is exercised here.
        let browser = match Browser::start() {
            Ok(b) => b,
            Err(_) => {
                eprintln!("skipping: mDNS daemon not available");
                return;
            }
        };
        browser.add_manual("My NAS", "https://nas.local:9000");
        let received = browser
            .rx
            // can't await directly because rx is in browser; clone the test
            // pattern instead by destructuring:
            ;
        // tokio::sync::mpsc::UnboundedReceiver doesn't impl Clone — we have
        // to extract it. Restructure: see Step 1 note.
        let _ = received;
    }
}
```

**Implementer note:** the `tokio::sync::mpsc::UnboundedReceiver` can't be cloned and lives on `Browser`. The test above won't compile as-written. Restructure either by:
- (a) destructuring `Browser` to extract `rx`, exercising `add_manual` on `tx` directly (requires making `tx` accessible — currently `pub(crate)`, fine);
- (b) accepting that the unit test is unreliable and dropping it; manual entry is exercised by `first_run_e2e.rs` (Task 20) via the screen's "Add manually" button.

Pick (a) for a tighter test: change `tx` to `pub` visibility within the crate and write:

```rust
    #[tokio::test]
    async fn add_manual_emits_event() {
        let browser = match Browser::start() {
            Ok(b) => b,
            Err(_) => {
                eprintln!("skipping: mDNS daemon not available");
                return;
            }
        };
        let Browser { mut rx, tx, .. } = browser;
        let _ = tx.send(DiscoveredServer {
            display_name: "Manual".to_string(),
            url: "https://x:9000".to_string(),
            manual: true,
        });
        let received = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await.unwrap().unwrap();
        assert_eq!(received.url, "https://x:9000");
        assert!(received.manual);
    }
```

- [ ] **Step 2: Add `pub mod discovery;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-client discovery
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: 1 test passes (or skips on no-network).

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/discovery.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): mDNS Browser + manual-entry path"
```

---

## Task 10: `cert_renewal` module

**Files:**
- Create: `crates/shoebox-client/src/cert_renewal.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

- [ ] **Step 1: Write the module.**

```rust
//! Background client-cert renewal task. Mirrors `shoebox-server`'s
//! `cert_renewal.rs` shape: 12h ticker, re-issue when <30 days remain.
//!
//! Unlike the server-side task, this one (a) calls the remote `/renew`
//! endpoint over the established mTLS connection, (b) persists the new
//! cert via `cert_store`, and (c) updates `client.toml`'s
//! `cert_serial_hex`. The in-process reqwest client is NOT swapped at
//! runtime (Iced's state lives behind a lock; rebuilding the client
//! during a tick is a Plan 1.4b refinement). For v1, the warning is
//! logged and the user picks up the new cert at next launch.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const RENEW_WHEN_DAYS_REMAINING: i64 = 30;
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Debug, Deserialize)]
struct RenewResponse {
    client_cert_pem: String,
    cert_serial_hex: String,
    not_after_unix: i64,
}

#[derive(Debug, Serialize)]
struct RenewRequest {
    csr_pem: String,
}

pub struct RenewalContext {
    pub server_url: String,
    pub client: Client,
    /// Path to the local `client.toml`; we rewrite `cert_serial_hex`
    /// after a successful renewal.
    pub config_path: PathBuf,
    /// Current cert's not_after. Updated in place after each renewal.
    pub not_after_unix: i64,
}

/// Run the renewal loop until `shutdown` resolves. Re-issues the cert
/// whenever `not_after_unix` is within `RENEW_WHEN_DAYS_REMAINING` days.
pub async fn run(
    context: Arc<parking_lot::Mutex<RenewalContext>>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "client.cert_renewal.shutdown");
                return;
            }
            _ = ticker.tick() => {
                if let Err(renewal_err) = run_one(&context).await {
                    tracing::warn!(
                        event = "client.cert_renewal.error",
                        error = %renewal_err,
                    );
                }
            }
        }
    }
}

/// Public for `cert_renewal_e2e.rs` — runs exactly one renewal check.
///
/// # Errors
/// Returns an error on network failure, CSR generation failure, server
/// rejection, or config write failure.
pub async fn run_one(context: &Arc<parking_lot::Mutex<RenewalContext>>) -> Result<()> {
    let (server_url, client, config_path, current_not_after) = {
        let guard = context.lock();
        (
            guard.server_url.clone(),
            guard.client.clone(),
            guard.config_path.clone(),
            guard.not_after_unix,
        )
    };

    let now_secs = now_secs();
    let days_remaining = (current_not_after.saturating_sub(now_secs)) / SECONDS_PER_DAY;
    crate::cert_renewal::log_days_remaining(days_remaining);
    if days_remaining > RENEW_WHEN_DAYS_REMAINING {
        return Ok(());
    }

    // Generate a new keypair + CSR.
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ED25519)
        .context("generating renewal keypair")?;
    let new_key_pem = key_pair.serialize_pem();
    let mut csr_params = CertificateParams::new(Vec::<String>::new())
        .context("renewal csr params")?;
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "renewal-csr-placeholder");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&key_pair)
        .context("renewal serialize csr")?
        .pem()
        .context("renewal csr pem")?;

    let resp = client
        .post(format!("{server_url}/renew"))
        .json(&RenewRequest { csr_pem })
        .send()
        .await
        .context("POST /renew")?;
    if !resp.status().is_success() {
        anyhow::bail!("renew returned {}", resp.status());
    }
    let renewed: RenewResponse = resp.json().await.context("parsing renew response")?;

    // Persist the new cert + key. Keychain is preferred; if it fails we
    // attempt file storage with a logged warning (renewal isn't user-
    // interactive, so no "explicit consent" prompt fires — we keep the
    // existing storage location).
    if let Err(keyring_err) =
        crate::cert_store::store_in_keyring(&server_url, &renewed.client_cert_pem, &new_key_pem)
    {
        tracing::warn!(
            event = "client.cert_renewal.keyring_fallback",
            error = %keyring_err,
        );
        crate::cert_store::store_in_file(&server_url, &renewed.client_cert_pem, &new_key_pem)
            .context("file fallback for renewal cert store")?;
    }

    // Update client.toml's cert_serial_hex.
    let mut config = crate::config::ClientConfig::read_from(&config_path)
        .context("re-reading client.toml during renewal")?;
    config.cert_serial_hex = renewed.cert_serial_hex.clone();
    config.write_to(&config_path).context("writing client.toml after renewal")?;

    // Update the in-memory not_after so the next tick uses the new
    // expiry.
    context.lock().not_after_unix = renewed.not_after_unix;

    tracing::warn!(
        event = "client.cert_renewal.reissued",
        days_remaining,
        new_serial = %renewed.cert_serial_hex,
        new_not_after_unix = renewed.not_after_unix,
        "client cert re-issued — running connection still uses the previous cert; restart to switch over"
    );
    Ok(())
}

fn log_days_remaining(days: i64) {
    tracing::debug!(event = "client.cert_renewal.tick", days_remaining = days);
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
}
```

- [ ] **Step 2: Add `pub mod cert_renewal;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

(No unit tests in this task; behavior exercised by Task 22's `cert_renewal_e2e.rs`.)

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/cert_renewal.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): background cert renewal task (12h tick, <30d trigger)"
```

---

## Task 11: `app_state` module

**Files:**
- Create: `crates/shoebox-client/src/app_state.rs`
- Modify: `crates/shoebox-client/src/lib.rs`

The shared-state struct. Wrapped in `Arc<RwLock<…>>` by `main.rs` so background tasks can read/write.

- [ ] **Step 1: Write the module.**

```rust
//! Shared client state: cert, mTLS client, replica, config, current
//! connection status, current user, current screen. Owned by `main.rs`
//! behind `Arc<RwLock<…>>`; screens borrow read-only via `view()` and
//! mutate via messages dispatched by `update()`.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::ClientConfig;
use crate::replica::Replica;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Online,
    Offline,
}

/// True iff the user explicitly chose file storage over keychain during
/// this session's enrollment. Surfaced as a persistent warning banner.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FileStorageWarning(pub bool);

pub struct AppState {
    pub config: ClientConfig,
    pub config_path: PathBuf,
    pub replica: Option<Arc<Replica>>,
    pub client: Option<reqwest::Client>,
    /// CA PEM pinned during the wizard (or loaded from disk on steady-
    /// state launches). Needed by `mtls_http::build_mtls_client` after
    /// cert rotation.
    pub ca_pem: Option<String>,
    pub connection_status: ConnectionStatus,
    pub file_storage_warning: FileStorageWarning,
    /// In-flight error to display in the current screen's inline area.
    /// Set by `update()` handlers; cleared on next user action.
    pub last_error: Option<String>,
}

impl AppState {
    /// Build an `AppState` with no resources yet — the wizard or
    /// steady-state init in `main.rs` fills the optional fields.
    pub fn new(config: ClientConfig, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            replica: None,
            client: None,
            ca_pem: None,
            connection_status: ConnectionStatus::Disconnected,
            file_storage_warning: FileStorageWarning(false),
            last_error: None,
        }
    }

    /// True iff `config.is_first_run()` AND no in-memory cert/client
    /// has been populated yet.
    #[must_use]
    pub fn needs_wizard(&self) -> bool {
        self.config.is_first_run() && self.client.is_none()
    }
}
```

- [ ] **Step 2: Add `pub mod app_state;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 3: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
```

- [ ] **Step 4: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/app_state.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): AppState + ConnectionStatus + first-run gate"
```

---

## Task 12: `screens/mod.rs` — Screen + Message enums

**Files:**
- Create: `crates/shoebox-client/src/screens/mod.rs`
- Create: `crates/shoebox-client/src/screens/discovery.rs` (stub — full content in Task 13)
- Create: `crates/shoebox-client/src/screens/enter_secret.rs` (stub)
- Create: `crates/shoebox-client/src/screens/enroll_progress.rs` (stub)
- Create: `crates/shoebox-client/src/screens/profile_picker.rs` (stub)
- Create: `crates/shoebox-client/src/screens/library.rs` (stub)
- Modify: `crates/shoebox-client/src/lib.rs`

This task defines the central `Screen` + `Message` enums and creates empty stubs for each screen module (so the rest of the plan compiles incrementally). The screens get their real `view()` + handlers in Tasks 13–17.

- [ ] **Step 1: Write `screens/mod.rs`.**

```rust
//! Top-level Screen enum + Message enum. Each screen module exposes a
//! `view(&AppState) -> Element<Message>` function.

pub mod discovery;
pub mod enroll_progress;
pub mod enter_secret;
pub mod library;
pub mod profile_picker;

use crate::discovery::DiscoveredServer;
use crate::enrollment::EnrollResult;

#[derive(Debug, Clone, Default)]
pub enum Screen {
    #[default]
    Discovery,
    EnterSecret {
        chosen_server: DiscoveredServer,
        ca_pem: Option<String>, // populated after fetch_ca_cert succeeds
    },
    EnrollProgress {
        chosen_server: DiscoveredServer,
        ca_pem: String,
    },
    /// Shown when keychain write failed during EnrollProgress and the
    /// user is being asked whether to retry or use file storage.
    KeychainFailure {
        enroll_result: EnrollResult,
        chosen_server: DiscoveredServer,
        ca_pem: String,
        last_keychain_error: String,
    },
    ProfilePicker {
        /// Loaded once from `users` after replica opens; refreshed on
        /// "Create new" success.
        users: Vec<UserRow>,
    },
    Library,
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: String,
    pub display_name: String,
}

/// Every Iced Message in the app. Categorised by which screen emits or
/// consumes it. Screen handlers in the screen modules pattern-match on
/// this enum.
#[derive(Debug, Clone)]
pub enum Message {
    // Discovery
    ServerDiscovered(DiscoveredServer),
    DiscoveryError(String),
    DiscoveryRetry,
    ManualUrlSubmitted { display_name: String, url: String },
    ServerPicked(DiscoveredServer),

    // EnterSecret + ca-cert bootstrap
    CaCertFetched(Result<String, String>),
    SecretSubmitted { secret: String, display_name: String },

    // EnrollProgress
    EnrollFinished(Result<EnrollResult, String>),
    CertStored(Result<(), String>),
    UseFileStorageInstead, // user accepted the explicit consent fallback
    RetryKeychainStore,

    // ProfilePicker
    UsersLoaded(Result<Vec<UserRow>, String>),
    UserPicked(String),
    CreateUserSubmitted { display_name: String },
    UserCreated(Result<UserRow, String>),

    // Library + background tickers
    ReplicaSyncTick,
    ReplicaSyncFinished(Result<u64, String>),
    CertRenewalTick,

    // Generic
    ClearError,
    Shutdown,
}
```

- [ ] **Step 2: Write each screen stub.** Each file gets one empty function returning a placeholder `Element`.

`screens/discovery.rs`:

```rust
//! Discovery screen — populated in Task 13.

use iced::widget::text;
use iced::Element;

use crate::app_state::AppState;
use crate::screens::Message;

#[must_use]
pub fn view(_state: &AppState) -> Element<'_, Message> {
    text("Discovery (Task 13)").into()
}
```

Repeat verbatim for `enter_secret.rs` ("EnterSecret (Task 14)"), `enroll_progress.rs` ("EnrollProgress (Task 15)"), `profile_picker.rs` ("ProfilePicker (Task 16)"), `library.rs` ("Library (Task 17)").

- [ ] **Step 3: Add `pub mod screens;` to `crates/shoebox-client/src/lib.rs`.**

- [ ] **Step 4: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

Expected: clean build. The stubs ensure each subsequent screen task is purely additive.

- [ ] **Step 5: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/screens crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): Screen + Message enums + screen-module stubs"
```

---

## Task 13: `screens/discovery.rs`

**Files:**
- Modify: `crates/shoebox-client/src/screens/discovery.rs`

The screen shows: the live list of discovered servers, a manual-entry form, a "Retry discovery" button, and inline error text.

- [ ] **Step 1: Replace the stub.**

```rust
//! Discovery screen: mDNS list + manual entry + retry.

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length};

use crate::app_state::AppState;
use crate::discovery::DiscoveredServer;
use crate::screens::{Message, UserRow};

/// View state lives in `AppState` (the discovered-servers list is
/// accumulated by `update()` from `Message::ServerDiscovered`). This
/// module is pure `view`.
#[must_use]
pub fn view<'a>(
    state: &'a AppState,
    discovered_servers: &'a [DiscoveredServer],
    manual_url_draft: &'a str,
    manual_name_draft: &'a str,
) -> Element<'a, Message> {
    let header = text("Pick a shoebox server").size(28);

    let server_list: Element<Message> = if discovered_servers.is_empty() {
        text("(no servers found yet — try Retry or add one manually)").into()
    } else {
        let mut list_column = column![].spacing(8);
        for server in discovered_servers {
            let pick_button = button(text(format!(
                "{}  —  {}",
                server.display_name, server.url
            )))
                .width(Length::Fill)
                .on_press(Message::ServerPicked(server.clone()));
            list_column = list_column.push(pick_button);
        }
        list_column.into()
    };

    let manual_form = column![
        text("Or add manually:").size(18),
        text_input("Display name", manual_name_draft)
            .on_input(|new_name| Message::ManualUrlSubmitted {
                display_name: new_name,
                url: manual_url_draft.to_string(),
            }),
        text_input("https://host:9000", manual_url_draft)
            .on_input(|new_url| Message::ManualUrlSubmitted {
                display_name: manual_name_draft.to_string(),
                url: new_url,
            }),
        button(text("Add this server")).on_press(Message::ManualUrlSubmitted {
            display_name: manual_name_draft.to_string(),
            url: manual_url_draft.to_string(),
        }),
    ]
    .spacing(6);

    let retry_button = button(text("Retry discovery")).on_press(Message::DiscoveryRetry);

    let error_row: Element<Message> = match state.last_error.as_deref() {
        Some(message) => row![text("Error: ").style(iced::widget::text::danger), text(message)]
            .into(),
        None => row![].into(),
    };

    container(
        column![header, server_list, retry_button, manual_form, error_row]
            .spacing(16)
            .padding(20),
    )
    .into()
}

/// Helper consumed by `main.rs::update()` to drop a new entry into the
/// running list (deduped by URL).
pub fn merge_discovered(
    existing: &mut Vec<DiscoveredServer>,
    new_entry: DiscoveredServer,
) {
    if existing.iter().any(|server| server.url == new_entry.url) {
        return;
    }
    existing.push(new_entry);
}

// `UserRow` re-exported so screen modules importing from `screens` see
// the type without an extra import line.
#[allow(unused_imports)]
use crate::screens as _screens_imports;
let _ = UserRow { id: String::new(), display_name: String::new() };
```

**Implementer:** the final two lines (`#[allow(unused_imports)]` + `let _ = UserRow {…}`) are scaffolding artifacts to keep the import edge live until Task 16's profile picker actually consumes `UserRow`. Delete them at the bottom of Task 16 when that screen is implemented.

The `manual_url_draft` / `manual_name_draft` strings are owned by `main.rs` as part of the UI state (alongside `Screen` and `discovered_servers`). This keeps the screen module a pure function of inputs.

- [ ] **Step 2: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/screens/discovery.rs
git -c commit.gpgsign=false commit -m "feat(client): Discovery screen view + merge_discovered helper"
```

---

## Task 14: `screens/enter_secret.rs`

**Files:**
- Modify: `crates/shoebox-client/src/screens/enter_secret.rs`

- [ ] **Step 1: Replace the stub.**

```rust
//! EnterSecret screen — user types the shared catalog secret + display
//! name. On submit, `main.rs` runs `enrollment::fetch_ca_cert` then
//! `enrollment::enroll`.

use iced::widget::{button, column, container, row, text, text_input};
use iced::Element;

use crate::app_state::AppState;
use crate::discovery::DiscoveredServer;
use crate::screens::Message;

#[must_use]
pub fn view<'a>(
    state: &'a AppState,
    chosen_server: &'a DiscoveredServer,
    secret_draft: &'a str,
    display_name_draft: &'a str,
    ca_cert_loaded: bool,
) -> Element<'a, Message> {
    let header = text(format!("Connect to {}", chosen_server.display_name)).size(24);
    let url_line = text(&chosen_server.url).size(14);

    let ca_status: Element<Message> = if ca_cert_loaded {
        text("✓ Server CA loaded — your data will be TLS-validated.").into()
    } else {
        text("Fetching server CA…").into()
    };

    let form = column![
        text("Enter the shared catalog secret your admin gave you:"),
        text_input("shared secret", secret_draft).on_input(|updated_secret| {
            Message::SecretSubmitted {
                secret: updated_secret,
                display_name: display_name_draft.to_string(),
            }
        }),
        text("Your display name (shown to others on the same catalog):"),
        text_input("display name", display_name_draft).on_input(|updated_name| {
            Message::SecretSubmitted {
                secret: secret_draft.to_string(),
                display_name: updated_name,
            }
        }),
        button(text("Enroll")).on_press(Message::SecretSubmitted {
            secret: secret_draft.to_string(),
            display_name: display_name_draft.to_string(),
        }),
    ]
    .spacing(8);

    let error_row: Element<Message> = match state.last_error.as_deref() {
        Some(message) => row![text("Error: ").style(iced::widget::text::danger), text(message)]
            .into(),
        None => row![].into(),
    };

    container(
        column![header, url_line, ca_status, form, error_row]
            .spacing(16)
            .padding(20),
    )
    .into()
}
```

- [ ] **Step 2: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/screens/enter_secret.rs
git -c commit.gpgsign=false commit -m "feat(client): EnterSecret screen view"
```

---

## Task 15: `screens/enroll_progress.rs`

**Files:**
- Modify: `crates/shoebox-client/src/screens/enroll_progress.rs`

Two visual states: in-flight ("Enrolling…") and the keychain-failure consent screen ("keychain write failed — Retry / Use file storage").

- [ ] **Step 1: Replace the stub.**

```rust
//! EnrollProgress screen — spinner during /enroll + cert_store::store,
//! plus the keychain-failure consent dialog.

use iced::widget::{button, column, container, row, text};
use iced::Element;

use crate::app_state::AppState;
use crate::screens::{Message, Screen};

#[must_use]
pub fn view<'a>(state: &'a AppState, current_screen: &'a Screen) -> Element<'a, Message> {
    match current_screen {
        Screen::EnrollProgress { chosen_server, .. } => container(
            column![
                text(format!("Enrolling with {}…", chosen_server.display_name)).size(20),
                text("This usually takes about a second."),
            ]
            .spacing(12)
            .padding(20),
        )
        .into(),
        Screen::KeychainFailure { last_keychain_error, .. } => container(
            column![
                text("Could not store your cert in the OS keychain").size(20),
                text(format!("Reason: {last_keychain_error}")).size(14),
                text(
                    "You can retry (e.g., unlock the keychain if it was locked), \
                     or use file storage instead. File storage writes the cert + \
                     key to a mode-0600 file in your app-data directory. It works \
                     but isn't as secure as the keychain — anyone with read access \
                     to your home directory could recover the key.",
                ),
                row![
                    button(text("Retry keychain")).on_press(Message::RetryKeychainStore),
                    button(text("Use file storage instead"))
                        .on_press(Message::UseFileStorageInstead),
                ]
                .spacing(12),
            ]
            .spacing(12)
            .padding(20),
        )
        .into(),
        _ => container(text("(invalid screen for enroll_progress::view)"))
            .padding(20)
            .into(),
    }
}

/// Helper for `main.rs::update()` — given an enroll result, attempt to
/// store via keychain. Returns the right next-step Message either way.
pub fn store_via_keychain_or_signal_failure(
    server_url: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<(), String> {
    crate::cert_store::store_in_keyring(server_url, cert_pem, key_pem)
        .map_err(|store_err| store_err.to_string())
}

/// Same, for file storage (called when the user picks "Use file storage instead").
pub fn store_via_file(
    server_url: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<(), String> {
    crate::cert_store::store_in_file(server_url, cert_pem, key_pem)
        .map_err(|store_err| store_err.to_string())
}
```

- [ ] **Step 2: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/screens/enroll_progress.rs
git -c commit.gpgsign=false commit -m "feat(client): EnrollProgress + KeychainFailure consent screen"
```

---

## Task 16: `screens/profile_picker.rs`

**Files:**
- Modify: `crates/shoebox-client/src/screens/profile_picker.rs`
- Modify: `crates/shoebox-client/src/screens/discovery.rs` (drop the scaffolding artifacts from Task 13)

- [ ] **Step 1: Replace the stub.**

```rust
//! ProfilePicker screen — list existing users from the local replica,
//! or let the user create a new one.

use iced::widget::{button, column, container, row, text, text_input};
use iced::Element;

use crate::app_state::AppState;
use crate::screens::{Message, UserRow};

#[must_use]
pub fn view<'a>(
    state: &'a AppState,
    existing_users: &'a [UserRow],
    new_user_draft: &'a str,
) -> Element<'a, Message> {
    let header = text("Who are you?").size(24);

    let user_list: Element<Message> = if existing_users.is_empty() {
        text("(no users yet — create one below)").into()
    } else {
        let mut list_column = column![text("Pick an existing profile:").size(16)].spacing(6);
        for existing_user in existing_users {
            let pick_button = button(text(&existing_user.display_name))
                .on_press(Message::UserPicked(existing_user.id.clone()));
            list_column = list_column.push(pick_button);
        }
        list_column.into()
    };

    let new_user_form = column![
        text("Or create a new profile:").size(16),
        text_input("display name", new_user_draft).on_input(|updated_name| {
            Message::CreateUserSubmitted { display_name: updated_name }
        }),
        button(text("Create")).on_press(Message::CreateUserSubmitted {
            display_name: new_user_draft.to_string(),
        }),
    ]
    .spacing(6);

    let error_row: Element<Message> = match state.last_error.as_deref() {
        Some(message) => row![text("Error: ").style(iced::widget::text::danger), text(message)]
            .into(),
        None => row![].into(),
    };

    container(
        column![header, user_list, new_user_form, error_row]
            .spacing(16)
            .padding(20),
    )
    .into()
}

/// Helper for `main.rs::update()` — runs `SELECT id, display_name FROM users`
/// on a libsql `Connection`.
///
/// # Errors
/// Returns an error on query failure.
pub async fn load_users(conn: &libsql::Connection) -> Result<Vec<UserRow>, anyhow::Error> {
    let mut rows = conn.query("SELECT id, display_name FROM users", ()).await?;
    let mut users = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        users.push(UserRow { id, display_name });
    }
    Ok(users)
}

/// Helper for `main.rs::update()` — inserts a new `users` row with a
/// freshly-generated UUID-like id and returns the inserted row.
///
/// # Errors
/// Returns an error on insert failure.
pub async fn create_user(
    conn: &libsql::Connection,
    display_name: &str,
) -> Result<UserRow, anyhow::Error> {
    use rand::RngCore;
    let mut id_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut id_bytes);
    let new_id = hex::encode(id_bytes);
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0);
    conn.execute(
        "INSERT INTO users (id, display_name, created_at, last_seen_at) VALUES (?1, ?2, ?3, ?3)",
        (new_id.clone(), display_name.to_string(), now_ms),
    )
    .await?;
    Ok(UserRow { id: new_id, display_name: display_name.to_string() })
}
```

- [ ] **Step 2: Drop the scaffolding lines at the bottom of `screens/discovery.rs`.**

Remove the trailing block from Task 13's discovery.rs:

```rust
// remove these three lines:
#[allow(unused_imports)]
use crate::screens as _screens_imports;
let _ = UserRow { id: String::new(), display_name: String::new() };
```

Also drop the `use crate::screens::{Message, UserRow};` import in `discovery.rs` if `UserRow` is no longer referenced there. Replace with just `use crate::screens::Message;`.

- [ ] **Step 3: Add `rand = { workspace = true }` to `crates/shoebox-client/Cargo.toml` `[dependencies]`** if not already present. (`rand` IS in the workspace from Plan 1.2.)

- [ ] **Step 4: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 5: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/screens crates/shoebox-client/Cargo.toml
git -c commit.gpgsign=false commit -m "feat(client): ProfilePicker view + load_users + create_user helpers"
```

---

## Task 17: `screens/library.rs`

**Files:**
- Modify: `crates/shoebox-client/src/screens/library.rs`

The debug "Library home". Shows: server URL, connection status, schema version, photo count, folder count, active user display name. Plus banners for offline + file-storage-warning.

- [ ] **Step 1: Replace the stub.**

```rust
//! Library screen — debug "catalog state" view.

use iced::widget::{column, container, row, text};
use iced::Element;

use crate::app_state::{AppState, ConnectionStatus};
use crate::screens::Message;

/// View state owned by `main.rs`: the latest stats loaded from the
/// replica. Refreshed on `Message::ReplicaSyncFinished`.
#[derive(Debug, Default, Clone)]
pub struct LibraryStats {
    pub schema_version: i64,
    pub photo_count: i64,
    pub folder_count: i64,
    pub active_user_display_name: String,
}

#[must_use]
pub fn view<'a>(state: &'a AppState, stats: &'a LibraryStats) -> Element<'a, Message> {
    let connection_line = text(format!(
        "Connection: {:?} ({})",
        state.connection_status, state.config.server_url
    ))
    .size(16);

    let offline_banner: Element<Message> =
        if state.connection_status == ConnectionStatus::Offline {
            text("⚠ Offline — reading from local replica; writes disabled")
                .style(iced::widget::text::danger)
                .into()
        } else {
            row![].into()
        };

    let file_storage_banner: Element<Message> = if state.file_storage_warning.0 {
        text(
            "⚠ Cert is stored in a file (you chose this when the keychain failed). \
             Re-enroll on a working keychain to upgrade.",
        )
        .style(iced::widget::text::danger)
        .into()
    } else {
        row![].into()
    };

    let stats_block = column![
        text(format!("Schema version: {}", stats.schema_version)),
        text(format!("Photos: {}", stats.photo_count)),
        text(format!("Folders: {}", stats.folder_count)),
        text(format!("Active user: {}", stats.active_user_display_name)),
    ]
    .spacing(4);

    container(
        column![
            text("shoebox").size(28),
            connection_line,
            offline_banner,
            file_storage_banner,
            stats_block,
        ]
        .spacing(12)
        .padding(20),
    )
    .into()
}

/// Helper for `main.rs::update()` — populates a fresh `LibraryStats`
/// from a libsql `Connection`. Assumes `schema_version` lives in the
/// `_schema_migrations` table's max version, and `active_user_display_name`
/// comes from the `users` row matching `config.last_active_user_id`.
///
/// # Errors
/// Returns an error on query failure.
pub async fn load_stats(
    conn: &libsql::Connection,
    active_user_id: Option<&str>,
) -> Result<LibraryStats, anyhow::Error> {
    let mut stats = LibraryStats::default();

    let mut row = conn
        .query("SELECT COALESCE(MAX(version), 0) FROM _schema_migrations", ())
        .await?;
    if let Some(r) = row.next().await? {
        stats.schema_version = r.get(0)?;
    }

    let mut row = conn.query("SELECT COUNT(*) FROM photos", ()).await?;
    if let Some(r) = row.next().await? {
        stats.photo_count = r.get(0)?;
    }

    let mut row = conn.query("SELECT COUNT(*) FROM folders", ()).await?;
    if let Some(r) = row.next().await? {
        stats.folder_count = r.get(0)?;
    }

    if let Some(user_id) = active_user_id {
        let mut row = conn
            .query("SELECT display_name FROM users WHERE id = ?1", [user_id])
            .await?;
        if let Some(r) = row.next().await? {
            stats.active_user_display_name = r.get(0)?;
        }
    }
    Ok(stats)
}
```

- [ ] **Step 2: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/screens/library.rs
git -c commit.gpgsign=false commit -m "feat(client): Library screen + LibraryStats loader"
```

---

## Task 18: `main.rs` — wire everything

**Files:**
- Modify: `crates/shoebox-client/src/main.rs`

Replace the scaffolding with the real Iced `Application` impl. Loads config; routes to initial Screen; spawns subscriptions (replica catchup, mDNS, cert renewal); handles the full Message → state-transition table; threads shutdown via Iced's window-close hook.

Because this file ties together every previous module, it's the biggest task by line count. Spell it out fully.

- [ ] **Step 1: Replace `main.rs`.**

```rust
//! shoebox-client binary — one Iced Application driving the
//! Plan 1.4 state machine.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::oneshot;

use shoebox_client::app_state::{AppState, ConnectionStatus, FileStorageWarning};
use shoebox_client::cert_renewal::RenewalContext;
use shoebox_client::cert_store;
use shoebox_client::config::{default_config_path, ClientConfig};
use shoebox_client::discovery::{Browser, DiscoveredServer};
use shoebox_client::enrollment::{enroll, fetch_ca_cert, EnrollResult};
use shoebox_client::mtls_http::build_mtls_client;
use shoebox_client::replica::Replica;
use shoebox_client::screens::{
    discovery as discovery_screen, enroll_progress as enroll_progress_screen,
    enter_secret as enter_secret_screen, library as library_screen,
    profile_picker as profile_picker_screen, Message, Screen, UserRow,
};

const REPLICA_SYNC_INTERVAL: Duration = Duration::from_secs(30);
const CERT_RENEWAL_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();
    iced::application("shoebox", App::update, App::view)
        .subscription(App::subscription)
        .run_with(App::new)
}

struct App {
    /// Source of truth for everything across screens + background tasks.
    state: Arc<RwLock<AppState>>,
    /// Current screen + its UI-only draft state.
    screen: Screen,
    discovered_servers: Vec<DiscoveredServer>,
    manual_url_draft: String,
    manual_name_draft: String,
    secret_draft: String,
    display_name_draft: String,
    new_user_draft: String,
    library_stats: library_screen::LibraryStats,
    /// mDNS browser. `None` once we've paired with a server.
    discovery_browser: Option<Browser>,
    /// Cert renewal task context. `None` until enrollment completes.
    renewal_context: Option<Arc<parking_lot::Mutex<RenewalContext>>>,
}

impl App {
    fn new() -> (Self, iced::Task<Message>) {
        let config_path = default_config_path().expect("config dir resolvable");
        let config = ClientConfig::read_from(&config_path).unwrap_or_default();
        let app_state = AppState::new(config.clone(), config_path);
        let initial_screen = if app_state.needs_wizard() {
            Screen::default()
        } else {
            Screen::Library
        };
        let discovery_browser = if matches!(initial_screen, Screen::Discovery) {
            Browser::start().ok()
        } else {
            None
        };
        let app = Self {
            state: Arc::new(RwLock::new(app_state)),
            screen: initial_screen,
            discovered_servers: Vec::new(),
            manual_url_draft: String::new(),
            manual_name_draft: String::new(),
            secret_draft: String::new(),
            display_name_draft: String::new(),
            new_user_draft: String::new(),
            library_stats: library_screen::LibraryStats::default(),
            discovery_browser,
            renewal_context: None,
        };
        let initial_task = if matches!(app.screen, Screen::Library) {
            iced::Task::perform(open_replica_and_load_stats(app.state.clone()), |result| {
                match result {
                    Ok(stats) => Message::ReplicaSyncFinished(Ok(stats.frame_no)),
                    Err(open_err) => Message::ReplicaSyncFinished(Err(open_err)),
                }
            })
        } else {
            iced::Task::none()
        };
        (app, initial_task)
    }

    fn view(&self) -> iced::Element<'_, Message> {
        let guard = self.state.read();
        match &self.screen {
            Screen::Discovery => discovery_screen::view(
                &guard,
                &self.discovered_servers,
                &self.manual_url_draft,
                &self.manual_name_draft,
            ),
            Screen::EnterSecret { chosen_server, ca_pem } => enter_secret_screen::view(
                &guard,
                chosen_server,
                &self.secret_draft,
                &self.display_name_draft,
                ca_pem.is_some(),
            ),
            Screen::EnrollProgress { .. } | Screen::KeychainFailure { .. } => {
                enroll_progress_screen::view(&guard, &self.screen)
            }
            Screen::ProfilePicker { users } => profile_picker_screen::view(
                &guard,
                users,
                &self.new_user_draft,
            ),
            Screen::Library => library_screen::view(&guard, &self.library_stats),
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        let mut subscriptions = Vec::new();
        if matches!(self.screen, Screen::Library) {
            subscriptions.push(
                iced::time::every(REPLICA_SYNC_INTERVAL).map(|_| Message::ReplicaSyncTick),
            );
            subscriptions.push(
                iced::time::every(CERT_RENEWAL_INTERVAL).map(|_| Message::CertRenewalTick),
            );
        }
        // mDNS events stream in via a polling subscription that drains
        // the Browser's receiver. We use a generic recipe via
        // `iced::Subscription::run` keyed by the browser's lifecycle.
        // For v1 simplicity: poll every 250 ms while on the Discovery
        // screen.
        if matches!(self.screen, Screen::Discovery) {
            subscriptions.push(
                iced::time::every(Duration::from_millis(250))
                    .map(|_| Message::ServerDiscovered(DiscoveredServer {
                        display_name: String::new(),
                        url: String::new(),
                        manual: false,
                    })),
            );
            // Note: the above sends a sentinel; update() handles it by
            // calling `discovery_browser.rx.try_recv()` and either
            // re-emitting as ServerDiscovered or doing nothing.
        }
        iced::Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::ServerDiscovered(sentinel_or_real) => {
                // Drain the browser's queue, then merge whatever's there.
                if let Some(browser) = self.discovery_browser.as_mut() {
                    while let Ok(real) = browser.rx.try_recv() {
                        discovery_screen::merge_discovered(&mut self.discovered_servers, real);
                    }
                }
                // If the message itself was a real entry (not a poll
                // sentinel — distinguished by non-empty url), merge it too.
                if !sentinel_or_real.url.is_empty() {
                    discovery_screen::merge_discovered(
                        &mut self.discovered_servers,
                        sentinel_or_real,
                    );
                }
                iced::Task::none()
            }
            Message::DiscoveryError(error_message) => {
                self.state.write().last_error = Some(error_message);
                iced::Task::none()
            }
            Message::DiscoveryRetry => {
                if let Some(browser) = self.discovery_browser.as_mut() {
                    if let Err(rebrowse_err) = browser.rebrowse() {
                        self.state.write().last_error =
                            Some(format!("rebrowse failed: {rebrowse_err}"));
                    } else {
                        self.state.write().last_error = None;
                    }
                }
                iced::Task::none()
            }
            Message::ManualUrlSubmitted { display_name, url } => {
                self.manual_name_draft = display_name.clone();
                self.manual_url_draft = url.clone();
                if !url.is_empty() {
                    if let Some(browser) = self.discovery_browser.as_ref() {
                        browser.add_manual(&display_name, &url);
                    }
                }
                iced::Task::none()
            }
            Message::ServerPicked(server) => {
                self.screen = Screen::EnterSecret {
                    chosen_server: server.clone(),
                    ca_pem: None,
                };
                let target_url = server.url.clone();
                iced::Task::perform(
                    async move { fetch_ca_cert(&target_url).await.map_err(|e| e.to_string()) },
                    Message::CaCertFetched,
                )
            }
            Message::CaCertFetched(Ok(ca_pem)) => {
                if let Screen::EnterSecret { ca_pem: slot, .. } = &mut self.screen {
                    *slot = Some(ca_pem);
                }
                self.state.write().last_error = None;
                iced::Task::none()
            }
            Message::CaCertFetched(Err(ca_err)) => {
                self.state.write().last_error =
                    Some(format!("fetching server CA: {ca_err}"));
                iced::Task::none()
            }
            Message::SecretSubmitted { secret, display_name } => {
                self.secret_draft = secret.clone();
                self.display_name_draft = display_name.clone();
                let Screen::EnterSecret { chosen_server, ca_pem: Some(ca_pem) } =
                    self.screen.clone()
                else {
                    return iced::Task::none();
                };
                self.screen = Screen::EnrollProgress {
                    chosen_server: chosen_server.clone(),
                    ca_pem: ca_pem.clone(),
                };
                let server_url = chosen_server.url.clone();
                let ca_for_enroll = ca_pem.clone();
                iced::Task::perform(
                    async move {
                        enroll(&server_url, &ca_for_enroll, &secret, &display_name)
                            .await
                            .map_err(|enroll_err| enroll_err.to_string())
                    },
                    Message::EnrollFinished,
                )
            }
            Message::EnrollFinished(Ok(enroll_result)) => {
                let Screen::EnrollProgress { chosen_server, ca_pem } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                // Try keychain first; on failure transition to KeychainFailure.
                let store_outcome = enroll_progress_screen::store_via_keychain_or_signal_failure(
                    &chosen_server.url,
                    &enroll_result.client_cert_pem,
                    &enroll_result.client_key_pem,
                );
                match store_outcome {
                    Ok(()) => self.finalize_enrollment(
                        chosen_server.url.clone(),
                        ca_pem,
                        enroll_result,
                        FileStorageWarning(false),
                    ),
                    Err(keyring_err) => {
                        self.screen = Screen::KeychainFailure {
                            enroll_result,
                            chosen_server,
                            ca_pem,
                            last_keychain_error: keyring_err,
                        };
                        iced::Task::none()
                    }
                }
            }
            Message::EnrollFinished(Err(enroll_err)) => {
                self.state.write().last_error = Some(enroll_err);
                // Drop back to EnterSecret so the user can retry.
                if let Screen::EnrollProgress { chosen_server, ca_pem } = self.screen.clone() {
                    self.screen = Screen::EnterSecret {
                        chosen_server,
                        ca_pem: Some(ca_pem),
                    };
                }
                iced::Task::none()
            }
            Message::RetryKeychainStore => {
                let Screen::KeychainFailure {
                    enroll_result, chosen_server, ca_pem, ..
                } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                let retry_outcome = enroll_progress_screen::store_via_keychain_or_signal_failure(
                    &chosen_server.url,
                    &enroll_result.client_cert_pem,
                    &enroll_result.client_key_pem,
                );
                match retry_outcome {
                    Ok(()) => self.finalize_enrollment(
                        chosen_server.url.clone(),
                        ca_pem,
                        enroll_result,
                        FileStorageWarning(false),
                    ),
                    Err(keyring_err) => {
                        if let Screen::KeychainFailure { last_keychain_error, .. } =
                            &mut self.screen
                        {
                            *last_keychain_error = keyring_err;
                        }
                        iced::Task::none()
                    }
                }
            }
            Message::UseFileStorageInstead => {
                let Screen::KeychainFailure {
                    enroll_result, chosen_server, ca_pem, ..
                } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                match enroll_progress_screen::store_via_file(
                    &chosen_server.url,
                    &enroll_result.client_cert_pem,
                    &enroll_result.client_key_pem,
                ) {
                    Ok(()) => self.finalize_enrollment(
                        chosen_server.url.clone(),
                        ca_pem,
                        enroll_result,
                        FileStorageWarning(true),
                    ),
                    Err(file_err) => {
                        self.state.write().last_error =
                            Some(format!("file storage also failed: {file_err}"));
                        iced::Task::none()
                    }
                }
            }
            Message::CertStored(Ok(())) => iced::Task::none(),
            Message::CertStored(Err(store_err)) => {
                self.state.write().last_error = Some(store_err);
                iced::Task::none()
            }
            Message::UsersLoaded(Ok(users)) => {
                self.screen = Screen::ProfilePicker { users };
                iced::Task::none()
            }
            Message::UsersLoaded(Err(load_err)) => {
                self.state.write().last_error = Some(load_err);
                iced::Task::none()
            }
            Message::UserPicked(user_id) => {
                let state_clone = self.state.clone();
                {
                    let mut guard = state_clone.write();
                    guard.config.last_active_user_id = Some(user_id.clone());
                    let config_path = guard.config_path.clone();
                    let config_snapshot = guard.config.clone();
                    drop(guard);
                    if let Err(write_err) = config_snapshot.write_to(&config_path) {
                        state_clone.write().last_error =
                            Some(format!("writing client.toml: {write_err}"));
                    }
                }
                self.screen = Screen::Library;
                iced::Task::perform(load_library_stats(self.state.clone()), |result| {
                    Message::ReplicaSyncFinished(result.map(|stats_frame| stats_frame.frame_no))
                })
            }
            Message::CreateUserSubmitted { display_name } => {
                self.new_user_draft = display_name.clone();
                let state_clone = self.state.clone();
                iced::Task::perform(
                    async move {
                        let replica = state_clone
                            .read()
                            .replica
                            .clone()
                            .ok_or_else(|| "no replica".to_string())?;
                        let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
                        profile_picker_screen::create_user(&conn, &display_name)
                            .await
                            .map_err(|create_err| create_err.to_string())
                    },
                    Message::UserCreated,
                )
            }
            Message::UserCreated(Ok(new_user)) => {
                if let Screen::ProfilePicker { users } = &mut self.screen {
                    users.push(new_user);
                }
                iced::Task::none()
            }
            Message::UserCreated(Err(create_err)) => {
                self.state.write().last_error = Some(create_err);
                iced::Task::none()
            }
            Message::ReplicaSyncTick => {
                let state_clone = self.state.clone();
                iced::Task::perform(
                    async move { sync_and_reload_stats(state_clone).await },
                    |result| Message::ReplicaSyncFinished(result.map(|frame_no| frame_no)),
                )
            }
            Message::ReplicaSyncFinished(Ok(_frame_no)) => {
                self.state.write().connection_status = ConnectionStatus::Online;
                let state_clone = self.state.clone();
                iced::Task::perform(load_library_stats(state_clone), |result| {
                    Message::ReplicaSyncFinished(result.map(|stats_frame| stats_frame.frame_no))
                })
            }
            Message::ReplicaSyncFinished(Err(sync_err)) => {
                let mut guard = self.state.write();
                guard.connection_status = ConnectionStatus::Offline;
                guard.last_error = Some(sync_err);
                iced::Task::none()
            }
            Message::CertRenewalTick => {
                if let Some(context) = self.renewal_context.clone() {
                    iced::Task::perform(
                        async move {
                            shoebox_client::cert_renewal::run_one(&context)
                                .await
                                .map_err(|renewal_err| renewal_err.to_string())
                        },
                        |result| match result {
                            Ok(()) => Message::ClearError,
                            Err(renewal_err) => Message::DiscoveryError(renewal_err),
                        },
                    )
                } else {
                    iced::Task::none()
                }
            }
            Message::ClearError => {
                self.state.write().last_error = None;
                iced::Task::none()
            }
            Message::Shutdown => iced::Task::none(),
        }
    }

    /// After successful keychain or file storage, write client.toml,
    /// open the replica, build the mTLS client, load users, transition
    /// to ProfilePicker.
    fn finalize_enrollment(
        &mut self,
        server_url: String,
        ca_pem: String,
        enroll_result: EnrollResult,
        file_storage_warning: FileStorageWarning,
    ) -> iced::Task<Message> {
        {
            let mut guard = self.state.write();
            guard.config.server_url = server_url.clone();
            guard.config.cert_serial_hex = enroll_result.cert_serial_hex.clone();
            guard.file_storage_warning = file_storage_warning;
            guard.ca_pem = Some(ca_pem.clone());
            let config_path = guard.config_path.clone();
            let snapshot = guard.config.clone();
            if let Err(write_err) = snapshot.write_to(&config_path) {
                guard.last_error = Some(format!("writing client.toml: {write_err}"));
            }
        }

        // Build the mTLS client + open replica.
        let cert_pem = enroll_result.client_cert_pem.clone();
        let key_pem = enroll_result.client_key_pem.clone();
        let mtls_client_result = build_mtls_client(&ca_pem, &cert_pem, &key_pem);
        let Ok(mtls_client) = mtls_client_result else {
            self.state.write().last_error = Some("could not build mTLS client".to_string());
            return iced::Task::none();
        };

        let state_clone = self.state.clone();
        let server_url_for_task = server_url.clone();
        let ca_for_task = ca_pem.clone();
        let cert_for_task = cert_pem.clone();
        let key_for_task = key_pem.clone();
        let mtls_client_for_task = mtls_client.clone();

        // Set up the cert renewal context (12h ticker uses it).
        let renewal = Arc::new(parking_lot::Mutex::new(RenewalContext {
            server_url: server_url.clone(),
            client: mtls_client.clone(),
            config_path: self.state.read().config_path.clone(),
            not_after_unix: enroll_result.not_after_unix,
        }));
        self.renewal_context = Some(renewal);

        self.state.write().client = Some(mtls_client);
        self.discovery_browser = None; // we're paired

        iced::Task::perform(
            async move {
                let local_path = replica_local_path(&server_url_for_task)?;
                let replica = Replica::open(
                    &local_path,
                    &server_url_for_task,
                    &ca_for_task,
                    &cert_for_task,
                    &key_for_task,
                )
                .await
                .map_err(|open_err| open_err.to_string())?;
                replica
                    .sync()
                    .await
                    .map_err(|sync_err| sync_err.to_string())?;
                state_clone.write().replica = Some(Arc::new(replica));
                let _ = mtls_client_for_task;
                let conn = state_clone
                    .read()
                    .replica
                    .clone()
                    .ok_or_else(|| "replica missing after open".to_string())?
                    .conn()
                    .map_err(|conn_err| conn_err.to_string())?;
                profile_picker_screen::load_users(&conn)
                    .await
                    .map_err(|load_err| load_err.to_string())
            },
            Message::UsersLoaded,
        )
    }
}

fn replica_local_path(server_url: &str) -> Result<std::path::PathBuf, String> {
    let project_dirs = directories::ProjectDirs::from("io", "shoebox", "shoebox-client")
        .ok_or_else(|| "could not determine data dir".to_string())?;
    let server_slug = hex::encode(blake3::hash(server_url.as_bytes()).as_bytes());
    Ok(project_dirs
        .data_local_dir()
        .join("replicas")
        .join(server_slug)
        .join("catalog.db"))
}

async fn open_replica_and_load_stats(
    state: Arc<RwLock<AppState>>,
) -> Result<library_screen::LibraryStats, String> {
    let (server_url, ca_pem, cert_pem, key_pem, config_path, last_user) = {
        let guard = state.read();
        let server_url = guard.config.server_url.clone();
        let ca_pem = guard
            .ca_pem
            .clone()
            .or_else(|| {
                // On steady-state launches we don't have ca_pem in memory yet;
                // re-fetch via /ca-cert. This is acceptable for v1.
                None
            });
        let pair = cert_store::load_from_keyring(&server_url)
            .unwrap_or_default()
            .or_else(|| cert_store::load_from_file(&server_url).unwrap_or_default());
        (
            server_url,
            ca_pem,
            pair.as_ref().map(|p| p.0.clone()),
            pair.as_ref().map(|p| p.1.clone()),
            guard.config_path.clone(),
            guard.config.last_active_user_id.clone(),
        )
    };
    let ca_pem = match ca_pem {
        Some(p) => p,
        None => fetch_ca_cert(&server_url)
            .await
            .map_err(|fetch_err| fetch_err.to_string())?,
    };
    let cert_pem = cert_pem.ok_or_else(|| "no client cert stored".to_string())?;
    let key_pem = key_pem.ok_or_else(|| "no client key stored".to_string())?;
    let mtls_client = build_mtls_client(&ca_pem, &cert_pem, &key_pem)
        .map_err(|build_err| build_err.to_string())?;
    {
        let mut guard = state.write();
        guard.ca_pem = Some(ca_pem.clone());
        guard.client = Some(mtls_client.clone());
    }
    let local_path = replica_local_path(&server_url)?;
    let replica = Replica::open(&local_path, &server_url, &ca_pem, &cert_pem, &key_pem)
        .await
        .map_err(|open_err| open_err.to_string())?;
    let frame_no = replica
        .sync()
        .await
        .map_err(|sync_err| sync_err.to_string())?;
    state.write().replica = Some(Arc::new(replica));
    let conn = state
        .read()
        .replica
        .clone()
        .ok_or_else(|| "replica missing".to_string())?
        .conn()
        .map_err(|conn_err| conn_err.to_string())?;
    let mut stats = library_screen::load_stats(&conn, last_user.as_deref())
        .await
        .map_err(|stats_err| stats_err.to_string())?;
    stats.frame_no = frame_no; // see note below
    Ok(stats)
}

async fn sync_and_reload_stats(state: Arc<RwLock<AppState>>) -> Result<u64, String> {
    let replica = state
        .read()
        .replica
        .clone()
        .ok_or_else(|| "no replica".to_string())?;
    replica.sync().await.map_err(|sync_err| sync_err.to_string())
}

async fn load_library_stats(
    state: Arc<RwLock<AppState>>,
) -> Result<library_screen::LibraryStats, String> {
    let (replica, last_user) = {
        let guard = state.read();
        (
            guard.replica.clone().ok_or_else(|| "no replica".to_string())?,
            guard.config.last_active_user_id.clone(),
        )
    };
    let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
    library_screen::load_stats(&conn, last_user.as_deref())
        .await
        .map_err(|stats_err| stats_err.to_string())
}
```

**Implementer notes for this task:**

1. **`LibraryStats.frame_no` field referenced above doesn't exist** in `library_screen::LibraryStats` as Task 17 defined it. Add a `pub frame_no: u64,` field to `LibraryStats` in `screens/library.rs` (and `..Default::default()` in `load_stats` returns it as 0). The `sync_and_reload_stats` function uses the frame number only for log signal; the UI displays the others.

2. **Iced 0.13's `iced::application` builder + `run_with`** may have different signatures than what's written here. The pattern is: `iced::application(title_fn, update_fn, view_fn).subscription(sub_fn).run_with(|| (state, initial_task))`. If your Iced version uses a different constructor shape, adapt — the semantic contract is the same.

3. **`mtls_client.clone()`** — `reqwest::Client` is `Clone` (it's a thin Arc internally). Cheap to clone.

4. **`AppState.ca_pem`** is loaded from memory during the wizard; on steady-state launches it's `None` initially and re-fetched via `/ca-cert`. A future task could persist the CA PEM in `client.toml` so we skip the re-fetch — backlog.

- [ ] **Step 2: Add `pub frame_no: u64,` to `LibraryStats`** in `crates/shoebox-client/src/screens/library.rs`. Default to 0.

- [ ] **Step 3: Add `blake3 = { workspace = true }` to `crates/shoebox-client/Cargo.toml` `[dependencies]`** if not present (`blake3` IS in the workspace from Plan 1.3).

- [ ] **Step 4: Run.**

```
cargo build -p shoebox-client
cargo clippy -p shoebox-client --all-targets -- -D warnings
cargo fmt --all
```

If the build fails on Iced API surface (likely the trickiest part of this task), inspect the installed Iced version:

```
find ~/.cargo/registry/src -type d -name 'iced-0.13*' | head -1
```

Then look at `examples/` in that directory for the current API shape. Adapt as needed. The contract: `view(&self) -> Element`, `update(&mut self, Message) -> Task<Message>`, `subscription(&self) -> Subscription<Message>`, `new() -> (Self, Task<Message>)`.

- [ ] **Step 5: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/src/main.rs crates/shoebox-client/src/screens/library.rs crates/shoebox-client/Cargo.toml
git -c commit.gpgsign=false commit -m "feat(client): wire Iced Application + screens + subscriptions in main.rs"
```

---

## Task 19: Integration test — `first_run_e2e.rs`

**Files:**
- Create: `crates/shoebox-client/tests/first_run_e2e.rs`

End-to-end test that spawns a `shoebox-server` in-process, drives the first-run wizard programmatically by calling the client's modules directly (NOT through Iced's runtime — testing the Iced loop is too brittle), and asserts that each step transitions correctly + the replica round-trips at the end.

Gated on `sqld` being on PATH using the same skip pattern as the server's e2e tests.

- [ ] **Step 1: Write the test.**

```rust
//! End-to-end: spawn a real shoebox-server in-process, run the
//! client's enroll → cert-store → mTLS-client → replica-open →
//! create-user → load-stats flow against it. Verifies the wizard's
//! plumbing without going through Iced's runtime.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn first_run_round_trips_to_library_state() {
    // Skip gate (matches server's proxy_e2e.rs / locks_e2e.rs pattern).
    let sqld_bin = std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    if which::which(&sqld_bin).is_err() {
        eprintln!("skipping first_run_e2e: sqld not on PATH");
        return;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_tmp = TempDir::new().unwrap();
    let data_dir = server_tmp.path().to_path_buf();
    let cache_dir = server_tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Bootstrap server-side state (mirrors locks_e2e.rs).
    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await.unwrap(),
    );
    let setup_conn = db.connect().unwrap();
    let shared_secret =
        match shoebox_server::secret::ensure_present(&setup_conn).await.unwrap() {
            shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
            other => panic!("expected Generated, got {other:?}"),
        };
    drop(setup_conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    sans.push("127.0.0.1".to_string());
    let (server_cert, server_keypair) = ca.issue_server_cert(&sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_keypair, &ca, crl).unwrap();

    let embedded_sqld = shoebox_server::sqld_embed::start(data_dir.clone()).await.unwrap();
    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: embedded_sqld.local_url.clone(),
        cache_dir: cache_dir.clone(),
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await.unwrap();
    });

    let server_url = format!("https://{addr}");

    // Step 1: fetch CA via /ca-cert (unauth, accepts invalid cert).
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url).await.unwrap();
    assert!(ca_pem.contains("-----BEGIN CERTIFICATE-----"));

    // Step 2: enroll.
    let enroll_result = shoebox_client::enrollment::enroll(
        &server_url,
        &ca_pem,
        &shared_secret,
        "TestUser",
    )
    .await
    .expect("enroll should succeed");
    assert!(!enroll_result.client_cert_pem.is_empty());
    assert!(!enroll_result.client_key_pem.is_empty());

    // Step 3: store the cert (use file storage to avoid keychain side-
    // effects in CI).
    let unique_server_url = format!("{server_url}/test-{}", rand_suffix());
    shoebox_client::cert_store::store_in_file(
        &unique_server_url,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    ).unwrap();
    let loaded = shoebox_client::cert_store::load_from_file(&unique_server_url)
        .unwrap()
        .expect("file storage round-trip");
    assert_eq!(loaded.0, enroll_result.client_cert_pem);

    // Step 4: build mTLS client + open replica.
    let mtls_client = shoebox_client::mtls_http::build_mtls_client(
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    ).unwrap();
    let _ = mtls_client;

    // Step 5: open replica + sync + run a query against `users`.
    let client_tmp = TempDir::new().unwrap();
    let replica_path = client_tmp.path().join("catalog.db");
    let replica = shoebox_client::replica::Replica::open(
        &replica_path,
        &server_url,
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .await
    .expect("replica open");
    replica.sync().await.expect("initial sync");

    let conn = replica.conn().expect("conn");
    let users = shoebox_client::screens::profile_picker::load_users(&conn).await.unwrap();
    // /enroll created one user.
    assert_eq!(users.len(), 1, "expected exactly one user, got {users:?}");
    assert_eq!(users[0].display_name, "TestUser");

    // Step 6: create a second user via the helper.
    let new_user = shoebox_client::screens::profile_picker::create_user(&conn, "Second").await.unwrap();
    assert_eq!(new_user.display_name, "Second");

    // Step 7: re-sync and re-read; should see two users now.
    replica.sync().await.expect("re-sync");
    let users_again = shoebox_client::screens::profile_picker::load_users(&conn).await.unwrap();
    assert_eq!(users_again.len(), 2);

    // Step 8: library stats.
    let stats = shoebox_client::screens::library::load_stats(&conn, Some(&users[0].id))
        .await.unwrap();
    assert_eq!(stats.schema_version, shoebox_common::SCHEMA_VERSION);
    assert_eq!(stats.active_user_display_name, "TestUser");

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded_sqld.shutdown().await;
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{nanos:x}")
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-client --test first_run_e2e
cargo clippy -p shoebox-client --all-targets -- -D warnings
```

Expected on a machine without sqld: skip message + ok. On a machine with sqld: full round-trip passes.

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/tests/first_run_e2e.rs
git -c commit.gpgsign=false commit -m "test(client): first-run wizard round-trip end-to-end"
```

---

## Task 20: Integration test — `replica_e2e.rs`

**Files:**
- Create: `crates/shoebox-client/tests/replica_e2e.rs`

Seeds the server's catalog with some `users`/`photos` rows, opens a client replica, asserts reads return seeded data, inserts a row from the client, re-syncs, asserts the round-trip.

Smaller test than `first_run_e2e.rs` because it shares the same server-spawning preamble — refactor common setup into a helper module if the duplication gets uncomfortable, but for two tests the duplication is fine.

- [ ] **Step 1: Write the test.**

```rust
//! End-to-end: seeded server → client replica reads seed data; client
//! writes round-trip back through the proxy.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn replica_round_trips_writes_back_to_server() {
    let sqld_bin = std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    if which::which(&sqld_bin).is_err() {
        eprintln!("skipping replica_e2e: sqld not on PATH");
        return;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_tmp = TempDir::new().unwrap();
    let data_dir = server_tmp.path().to_path_buf();
    let cache_dir = server_tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await.unwrap(),
    );
    let setup_conn = db.connect().unwrap();
    let shared_secret =
        match shoebox_server::secret::ensure_present(&setup_conn).await.unwrap() {
            shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
            other => panic!("got {other:?}"),
        };

    // Seed two users and a photo before the client touches anything.
    let seed_ts = 1_000_000_i64;
    setup_conn.execute(
        "INSERT INTO users (id, display_name, created_at) VALUES ('seed-1', 'Alice', ?1)",
        [seed_ts],
    ).await.unwrap();
    setup_conn.execute(
        "INSERT INTO users (id, display_name, created_at) VALUES ('seed-2', 'Bob', ?1)",
        [seed_ts],
    ).await.unwrap();
    setup_conn.execute(
        "INSERT INTO photos (id, file_size, file_format, imported_at) \
         VALUES ('photo-1', 100, 'PEF', ?1)",
        [seed_ts],
    ).await.unwrap();
    drop(setup_conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    sans.push("127.0.0.1".to_string());
    let (server_cert, server_keypair) = ca.issue_server_cert(&sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_keypair, &ca, crl).unwrap();
    let embedded_sqld = shoebox_server::sqld_embed::start(data_dir.clone()).await.unwrap();
    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: embedded_sqld.local_url.clone(),
        cache_dir: cache_dir.clone(),
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await.unwrap();
    });

    let server_url = format!("https://{addr}");

    // Enroll a client.
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url).await.unwrap();
    let enroll_result = shoebox_client::enrollment::enroll(
        &server_url, &ca_pem, &shared_secret, "ReplicaTest",
    )
    .await.unwrap();

    // Open replica.
    let client_tmp = TempDir::new().unwrap();
    let replica = shoebox_client::replica::Replica::open(
        &client_tmp.path().join("catalog.db"),
        &server_url,
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    ).await.unwrap();
    replica.sync().await.unwrap();

    let conn = replica.conn().unwrap();

    // Assert seeded data visible.
    let mut row = conn.query("SELECT COUNT(*) FROM users", ()).await.unwrap();
    let user_count: i64 = row.next().await.unwrap().unwrap().get(0).unwrap();
    // 2 seeded + 1 from /enroll = 3
    assert_eq!(user_count, 3);
    let mut row = conn.query("SELECT COUNT(*) FROM photos", ()).await.unwrap();
    let photo_count: i64 = row.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(photo_count, 1);

    // Write a new user from the client; re-sync; verify it shows up on
    // a fresh server-side read.
    conn.execute(
        "INSERT INTO users (id, display_name, created_at) VALUES ('client-side', 'Cara', ?1)",
        [seed_ts],
    ).await.unwrap();
    replica.sync().await.unwrap();

    let server_side_conn = db.connect().unwrap();
    let mut row = server_side_conn
        .query("SELECT display_name FROM users WHERE id = 'client-side'", ())
        .await.unwrap();
    let server_view_name: String = row.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(server_view_name, "Cara");

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded_sqld.shutdown().await;
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-client --test replica_e2e
cargo clippy -p shoebox-client --all-targets -- -D warnings
```

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/tests/replica_e2e.rs
git -c commit.gpgsign=false commit -m "test(client): replica reads seeded data + writes round-trip back"
```

---

## Task 21: Integration test — `cert_renewal_e2e.rs`

**Files:**
- Create: `crates/shoebox-client/tests/cert_renewal_e2e.rs`

Exercises the renewal trigger by feeding `RenewalContext` an artificially-near `not_after_unix`. No server-side test-only knob — the renewal is purely client-initiated, so we just construct the context with `now + 5 days` as the not_after, then run one `cert_renewal::run_one(...)` and assert the cert serial changed.

- [ ] **Step 1: Write the test.**

```rust
//! End-to-end: cert_renewal::run_one fires /renew when not_after is
//! within 30 days and persists the new cert to file storage.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn renewal_fires_when_under_30_days_remaining() {
    let sqld_bin = std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    if which::which(&sqld_bin).is_err() {
        eprintln!("skipping cert_renewal_e2e: sqld not on PATH");
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_tmp = TempDir::new().unwrap();
    let data_dir = server_tmp.path().to_path_buf();
    let cache_dir = server_tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await.unwrap(),
    );
    let setup_conn = db.connect().unwrap();
    let shared_secret =
        match shoebox_server::secret::ensure_present(&setup_conn).await.unwrap() {
            shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
            other => panic!("got {other:?}"),
        };

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    sans.push("127.0.0.1".to_string());
    let (server_cert, server_keypair) = ca.issue_server_cert(&sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_keypair, &ca, crl).unwrap();
    let embedded_sqld = shoebox_server::sqld_embed::start(data_dir.clone()).await.unwrap();
    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: embedded_sqld.local_url.clone(),
        cache_dir: cache_dir.clone(),
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await.unwrap();
    });

    let server_url = format!("https://{addr}");
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url).await.unwrap();
    let enroll_result = shoebox_client::enrollment::enroll(
        &server_url, &ca_pem, &shared_secret, "RenewalTest",
    ).await.unwrap();

    // Store the initial cert in a per-test file-storage dir.
    let test_server_url = format!("{server_url}/renewal-{}", rand_suffix());
    shoebox_client::cert_store::store_in_file(
        &test_server_url,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    ).unwrap();

    // Build the mTLS client + a fake client.toml.
    let mtls_client = shoebox_client::mtls_http::build_mtls_client(
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    ).unwrap();
    let cfg_tmp = TempDir::new().unwrap();
    let cfg_path = cfg_tmp.path().join("client.toml");
    let initial_cfg = shoebox_client::config::ClientConfig {
        server_url: test_server_url.clone(),
        cert_serial_hex: enroll_result.cert_serial_hex.clone(),
        last_active_user_id: None,
    };
    initial_cfg.write_to(&cfg_path).unwrap();

    // Construct a renewal context with not_after = now + 5 days.
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let near_expiry = now_secs + 5 * 24 * 60 * 60;
    let context = Arc::new(parking_lot::Mutex::new(
        shoebox_client::cert_renewal::RenewalContext {
            server_url: test_server_url.clone(),
            client: mtls_client.clone(),
            config_path: cfg_path.clone(),
            not_after_unix: near_expiry,
        },
    ));

    // Run one tick.
    shoebox_client::cert_renewal::run_one(&context).await.unwrap();

    // Assert the cert serial in client.toml changed.
    let post_cfg = shoebox_client::config::ClientConfig::read_from(&cfg_path).unwrap();
    assert_ne!(post_cfg.cert_serial_hex, enroll_result.cert_serial_hex,
        "renewal should have replaced cert_serial_hex");

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded_sqld.shutdown().await;
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{nanos:x}")
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-client --test cert_renewal_e2e
cargo clippy -p shoebox-client --all-targets -- -D warnings
```

- [ ] **Step 3: Commit (unsigned).**

```
git -c commit.gpgsign=false add crates/shoebox-client/tests/cert_renewal_e2e.rs
git -c commit.gpgsign=false commit -m "test(client): cert_renewal fires when not_after < 30 days"
```

---

## Task 22: Update CLAUDE.md + README

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

- [ ] **Step 1: CLAUDE.md sub-project #1 row.** Change to:

```
| 1 | **Catalog, sync & stack** | Plans 1.1+1.2+1.3 implemented (server data plane). Plan 1.4 implemented (desktop client foundation: Iced shell + first-run wizard + libSQL embedded replica through mTLS proxy). Plans 1.4b (demo library view) and 1.5 (deployment) pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
```

- [ ] **Step 2: CLAUDE.md repository layout** — add `shoebox-client/` to the crates section:

```
│   ├── shoebox-server/                      ← server binary (data plane)
│   ├── shoebox-client/                      ← desktop client (Iced UI, foundation)
│   └── shoebox-common/                      ← shared types
```

- [ ] **Step 3: CLAUDE.md "Implementation status" section** — append a `shoebox-client` block:

```markdown
- `crates/shoebox-client` — desktop client foundation (Plan 1.4):
  - Iced single-Application state machine: Discovery → EnterSecret → EnrollProgress (+ KeychainFailure consent) → ProfilePicker → Library
  - First-run wizard: mDNS discovery, manual entry, `/ca-cert` bootstrap, `/enroll`, profile picker, initial replica sync
  - Cert + key storage: OS keychain via `keyring` (Keychain / Credential Manager / Secret Service); explicit-consent mode-0600 file fallback
  - libSQL embedded replica through the mTLS proxy; 30s background catchup ticker
  - 12h background cert renewal task; re-issues when <30 days remain
  - Linux + macOS + Windows from one source tree (manual smoke on each)
- Run locally: `cargo run -p shoebox-client` (against a running `shoebox-server`)
```

- [ ] **Step 4: README.md** — add a "Running the client" section after the existing server bits:

```markdown
## Running the client

```bash
cargo run -p shoebox-client
```

On first launch:
1. The Discovery screen browses for `_shoebox._tcp.local.` servers. If nothing
   shows up within a few seconds, use "Add manually" to enter `https://host:9000`.
2. Paste the shared catalog secret your `shoebox-server` printed on its first
   startup, plus the display name you want others on the catalog to see.
3. The client fetches the server's CA, issues itself a cert via `/enroll`, and
   stashes the cert + key in your OS keychain.
4. Pick an existing user profile or create a new one.
5. The Library screen shows your connection status, schema version, photo +
   folder counts, and the active user — the polished library experience lands
   in a follow-up plan.
```

- [ ] **Step 5: Final verification.**

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

All four must pass. New test count: 43 (workspace baseline) + 1 (server `ca_cert`) + 4 (`config`) + 5 (`cert_store`) + 4 (`mtls_http`) + 1 (`enrollment`) + 1 (`discovery`, may skip) + 3 client e2e (`first_run`, `replica`, `cert_renewal` — all skip if `sqld` absent) ≈ **62 tests** when sqld is installed, ~55 without.

- [ ] **Step 6: Commit (unsigned).**

```
git -c commit.gpgsign=false add CLAUDE.md README.md
git -c commit.gpgsign=false commit -m "docs: update CLAUDE.md/README for Plan 1.4 desktop client"
```

---

## Definition of Done for Plan 1.4

After all 22 tasks are complete:

- `cargo test --workspace --all-targets` passes (counts above).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- `cargo run -p shoebox-client` against a fresh `shoebox-server`:
  - Discovery screen appears.
  - mDNS or manual entry → EnterSecret → enroll → ProfilePicker → Library.
  - Library screen shows: connection status Online, schema version 6, photos / folders / active user.
  - Restart the client → it skips the wizard and lands directly on Library.
- Server killed mid-session → offline banner appears within 30 s catchup tick.
- Server restarted → banner clears within 30 s.
- Server-side `shoebox-server revoke <serial>` → next sync attempt boots the client back to Discovery.
- The same source tree builds and runs on Linux dev box, Windows 11 host, MacBook Pro.

What this plan **does not** deliver — covered in subsequent plans:

- Demo library view (folder tree, photo grid, EXIF panel, rate / keyword / virtual-copy actions, develop-lock UI banners). → Plan 1.4b.
- Polished library experience (smooth-scroll 100k-thumbnail grid, filmstrip, search). → Sub-project #3.
- Cross-OS code signing, MSIX/DMG packaging, auto-update. → Plan 1.5.
- Real-time UI updates via WebSocket push (currently 30s polling). → Plan 1.4b decision.

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-05-17-sub-1-4-desktop-client-design.md`):

- §3 architecture (single Iced Application, Arc<RwLock<AppState>>, module layout) → Tasks 1, 11, 12, 18.
- §3 server-side addition (`GET /ca-cert`) → Task 2.
- §4 component responsibilities:
  - `cert_store` → Tasks 4 + 5.
  - `mtls_http` → Task 6.
  - `discovery` → Task 9.
  - `enrollment` → Task 7.
  - `replica` → Task 8.
  - `config` → Task 3.
  - `cert_renewal` → Task 10.
  - `screens/*` → Tasks 12–17.
- §5 first-run data flow → Tasks 12, 13, 14, 15, 16, 18 (Iced state machine + screens).
- §5 steady-state data flow → Task 18 (`open_replica_and_load_stats` in `main.rs::App::new`).
- §5 offline + cert-revoked recovery → Task 18 (`ReplicaSyncFinished` handler + steady-state TLS-error early ping).
- §6 error handling table — each row mapped to either a screen (Tasks 13–17) or to module-level error returns (Tasks 4–10) that screens render.
- §7 testing strategy → Tasks 19, 20, 21 (integration) + per-module unit tests in Tasks 3–10.
- §8.1 in-scope items all covered.
- §8.2 out-of-scope explicitly deferred in Definition of Done section.
- §9 known limitations carried into Definition of Done.

**Placeholder scan:** searched for "TBD", "TODO", "FIXME". None present except IMPLEMENTER NOTE callouts that flag known-risky areas (libsql 0.6 connector API in Task 8, Iced 0.13 API surface in Task 18). These are intentional warnings, not placeholders.

**Type consistency:**
- `EnrollResult` defined in Task 7 (`enrollment.rs`) and consumed in Tasks 12 (`Screen::KeychainFailure` variant), 18 (`finalize_enrollment`).
- `DiscoveredServer` defined in Task 9 and consumed in Tasks 12, 13, 18.
- `Screen` + `Message` defined in Task 12 and consumed by every screen + Task 18.
- `AppState`, `ConnectionStatus`, `FileStorageWarning` defined in Task 11, consumed everywhere.
- `RenewalContext` defined in Task 10 and consumed in Tasks 18 + 21.
- `LibraryStats` defined in Task 17, consumed in Task 18 (added `frame_no` field in Task 18 — flagged so the implementer remembers).
- `UserRow` defined in Task 12, consumed in Tasks 13 (scaffolding artifact) and 16 (actual use). Scaffolding cleanup is explicit in Task 16.

**Known risks for the implementing engineer:**

- **Task 8 (replica)** is the highest risk: libsql 0.6's connector hook may not accept a custom rustls config. The task documents the fallback (raw Hrana over reqwest), and a DONE_WITH_CONCERNS report is the right escalation.
- **Task 18 (main.rs)** is the largest single task by line count and ties together all of Tasks 11–17. Iced 0.13 API surface is the most likely friction point; the task documents how to inspect the installed version.
- **mTLS over reqwest for `/renew`** assumes the workspace's reqwest version handles `use_preconfigured_tls` cleanly — Plans 1.1–1.3 already exercise this in the server's e2e tests, so the risk is low.
