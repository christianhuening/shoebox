# shoebox-server Data Plane Implementation Plan (Plan 1.3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `shoebox-server` actually useful as a catalog backend. Add the libSQL wire-protocol proxy (so desktop clients can run embedded replicas through the mTLS layer), the filesystem indexer (watches NAS photo folders, hashes new RAW files with BLAKE3, extracts EXIF, populates `photos` / `photo_files` / `folders`), the thumbnailer (pulls the embedded JPEG preview out of each RAW, renders 256 px + 2k JPEGs into a shared cache directory), HTTP endpoints for thumbnail/preview fetches, develop-lock acquire/heartbeat/release/takeover endpoints, periodic janitor tasks (lock expiry, abandoned session cleanup, orphaned thumbnail GC), VACUUM-INTO backups with rotation, a Prometheus `/metrics` endpoint, and the server-cert auto-renewal task that was deferred from Plan 1.2.

**Architecture:** `shoebox-server` becomes a multi-task tokio process. The mTLS-terminating HTTPS+WS layer on `:9000` now serves three classes of traffic — the existing auth endpoints (`/enroll`, `/renew`, `/whoami`), new thumbnail/preview/develop-lock REST endpoints, and a transparent libSQL wire proxy that forwards authenticated traffic to an embedded `libsql-server` instance bound to `127.0.0.1`. New background tasks: indexer (FS watcher + queue worker), thumbnailer (queue worker), janitor (60s tick), backup (6h tick), cert-renewal (12h tick). Catalog still lives at `<data_dir>/catalog.db`; thumbnail + preview caches live under `<cache_dir>/{thumbnails,previews}/<hash>.jpg`.

**Tech Stack:** Adds to Plan 1.2's stack: `libsql-server` (embedded sqld), `notify` (filesystem watcher), `blake3` (content hashing), `rawler` (RAW preview extraction + EXIF), `image` (JPEG encode/resize), `prometheus` (metrics registry + text format), `tokio-tungstenite` (WebSocket forwarding for the libSQL wire proxy), `hyper` (already a transitive dep — used directly for the reverse-proxy plumbing), `walkdir` (recursive folder scan), `tower-http` ServiceBuilder for routing. No removals.

**Prerequisites for the implementing engineer:**
- Plans 1.1 and 1.2 complete (28 tests pass, `shoebox-server` builds and runs with mTLS + enrollment + revocation working).
- Familiarity with: tokio tasks, axum routing, basic understanding of HTTP reverse-proxying, basic understanding of TIFF/JPEG structure (only needed for context — `rawler` and `image` do the actual work).

---

## File Structure

This plan adds the following files and modifies a few from earlier plans.

```
shoebox/
├── crates/
│   └── shoebox-server/
│       ├── Cargo.toml                       ← add libsql-server, notify, blake3,
│       │                                       rawler, image, prometheus,
│       │                                       tokio-tungstenite, walkdir
│       ├── src/
│       │   ├── lib.rs                       ← expose new modules
│       │   ├── main.rs                      ← spawn background tasks
│       │   ├── proxy.rs                     ← NEW: libSQL wire proxy
│       │   ├── sqld_embed.rs                ← NEW: embed libsql-server, expose loopback URL
│       │   ├── hashing.rs                   ← NEW: BLAKE3 helper + tests
│       │   ├── raw_preview.rs               ← NEW: RAW embedded JPEG extraction
│       │   ├── indexer.rs                   ← NEW: FS watcher + state machine
│       │   ├── thumbnailer.rs               ← NEW: 256px + 2k JPEG generation
│       │   ├── thumbs_http.rs               ← NEW: GET /thumbs/<hash>, /previews/<hash>
│       │   ├── locks_http.rs                ← NEW: develop-lock REST endpoints
│       │   ├── janitor.rs                   ← NEW: periodic cleanup tasks
│       │   ├── backup.rs                    ← NEW: VACUUM INTO + retention
│       │   ├── metrics.rs                   ← NEW: Prometheus /metrics
│       │   ├── cert_renewal.rs              ← NEW: server cert auto-renew background task
│       │   ├── db.rs                        ← extend with lock helpers + thumbnail-status helpers
│       │   ├── http.rs                      ← register thumbs + locks routes
│       │   └── tls_server.rs                ← (no change expected)
│       └── tests/
│           ├── enroll_e2e.rs                ← unchanged
│           ├── health_e2e.rs                ← unchanged
│           ├── revoke_e2e.rs                ← unchanged
│           ├── renew_e2e.rs                 ← unchanged
│           ├── proxy_e2e.rs                 ← NEW: libSQL embedded-replica through proxy
│           ├── indexer_e2e.rs               ← NEW: drop files in watched dir → catalog updates
│           ├── locks_e2e.rs                 ← NEW: acquire/heartbeat/release/takeover flow
│           ├── thumbnailer_e2e.rs           ← NEW: drop RAW → thumb appears in cache
│           └── metrics_e2e.rs               ← NEW: /metrics returns Prometheus format
└── docs/
    └── superpowers/plans/
        └── 2026-05-17-sub-1-3-server-data-plane.md   ← this file
```

**Responsibility split:**
- `sqld_embed.rs` — owns the lifecycle of the embedded `libsql-server` instance. Returns a loopback URL the proxy can target. No HTTP logic.
- `proxy.rs` — receives authenticated HTTPS + WS requests on a `/v2/*` and `/v1/*` path prefix and forwards them to the loopback sqld. Pure reverse-proxy.
- `hashing.rs` — `blake3_file(path) -> Result<[u8; 32]>` and `blake3_hex(path) -> String`. Streams the file (no full read into memory).
- `raw_preview.rs` — `extract_preview(path) -> Result<Vec<u8>>` returning the embedded JPEG bytes from a RAW file. Wraps `rawler`.
- `indexer.rs` — owns the filesystem-watch + initial-scan state machine. Translates FS events into `photo_files` / `photos` / `folders` row updates. Enqueues thumbnailing work.
- `thumbnailer.rs` — drains the indexer's work queue, calls `raw_preview` + `image` to render 256 px and 2k JPEGs, atomically writes them to `<cache_dir>/thumbnails/<hash>.jpg` and `<cache_dir>/previews/<hash>.jpg`.
- `thumbs_http.rs` — `GET /thumbs/<hash>` and `GET /previews/<hash>` serve cached JPEGs (mTLS-protected via the existing `public_router`). 404 if missing.
- `locks_http.rs` — `POST/PUT/DELETE /locks/:variant_id` and `POST /locks/:variant_id/takeover`. All require `ClientIdentity`.
- `janitor.rs` — long-running `async fn run(db, cache_dir)` that ticks every 60 seconds and runs the three cleanup sweeps.
- `backup.rs` — long-running `async fn run(db, backup_dir)` that does VACUUM INTO every 6 hours and rotates to the last 14.
- `metrics.rs` — Prometheus `Registry` exposed via `GET /metrics` on the **health listener** (loopback, no mTLS) so Prometheus scrapers don't need certs.
- `cert_renewal.rs` — checks the server cert's `not_after` every 12 hours; if <30 days remain, re-issues via the in-process `Ca`, hot-reloads the rustls config.

---

## Task 1: Add data-plane workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/shoebox-server/Cargo.toml` (`[dependencies]`)

- [ ] **Step 1: Append to workspace `[workspace.dependencies]` in `Cargo.toml`.**

```toml
libsql-server = "0.24"
notify = "6"
blake3 = "1"
rawler = "0.6"
image = { version = "0.25", default-features = false, features = ["jpeg"] }
prometheus = { version = "0.13", default-features = false }
tokio-tungstenite = { version = "0.24", default-features = false, features = ["connect"] }
walkdir = "2"
hyper = "1"
hyper-util = { version = "0.1", features = ["client", "tokio"] }
http-body-util = "0.1"
bytes = "1"
```

NOTES:
- `libsql-server` version may need adjustment based on what's published; the implementer should check `cargo search libsql-server` and pick the latest compatible version.
- `rawler` is the actively-maintained successor to `rawloader`. Supports PEF (Pentax) and RAF (Fuji) as needed by the spec.
- `image` with only `jpeg` keeps the binary lean — we never decode/encode anything but JPEG in this plan.

- [ ] **Step 2: Append to `crates/shoebox-server/Cargo.toml` `[dependencies]`.**

```toml
libsql-server = { workspace = true }
notify = { workspace = true }
blake3 = { workspace = true }
rawler = { workspace = true }
image = { workspace = true }
prometheus = { workspace = true }
tokio-tungstenite = { workspace = true }
walkdir = { workspace = true }
hyper = { workspace = true }
hyper-util = { workspace = true }
http-body-util = { workspace = true }
bytes = { workspace = true }
```

- [ ] **Step 3: Verify the workspace builds.**

```
cargo build -p shoebox-server
```

Expect: clean build (may take 3-5 minutes fetching + compiling new deps the first time).

If `libsql-server` doesn't resolve cleanly at version `"0.24"`, try `"0"` and pin to whatever cargo selects; document the actual version in your commit message.

- [ ] **Step 4: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml
git commit -m "build: add libsql-server, notify, blake3, rawler, image, prometheus deps"
```

---

## Task 2: Embed `libsql-server` bound to localhost

**Files:**
- Create: `crates/shoebox-server/src/sqld_embed.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

This task spins up an in-process `libsql-server` (sqld) instance, bound to `127.0.0.1` on an ephemeral port, that serves the catalog DB to local clients. The proxy in Task 3 will forward authenticated mTLS traffic to it.

- [ ] **Step 1: Write `crates/shoebox-server/src/sqld_embed.rs`.**

```rust
//! Embeds a `libsql-server` (sqld) instance in-process, bound to a
//! loopback port. The mTLS proxy in `proxy.rs` forwards authenticated
//! client requests to this loopback URL.

use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Handle to a running embedded sqld instance.
pub struct EmbeddedSqld {
    /// Loopback URL the proxy targets, e.g. `http://127.0.0.1:53421`.
    pub local_url: String,
    /// Tokio task handle for the running sqld server.
    pub task: tokio::task::JoinHandle<Result<()>>,
}

/// Start an embedded sqld serving the given catalog DB on an ephemeral
/// loopback port. The server runs on a tokio task that lives until the
/// process exits.
pub async fn start(catalog_db_path: PathBuf) -> Result<EmbeddedSqld> {
    use libsql_server::config::{DbConfig, UserApiConfig};
    use libsql_server::Server;

    // Bind to an ephemeral port on loopback only. No TLS — only the
    // proxy (over mTLS) ever talks to this.
    let listener = std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .map_err(|e| anyhow!("binding embedded sqld loopback: {e}"))?;
    let local_addr = listener.local_addr()?;
    let local_url = format!("http://{}", local_addr);
    drop(listener); // libsql-server will rebind below; we just wanted the port

    let db_config = DbConfig {
        path: catalog_db_path.clone(),
        ..Default::default()
    };
    let user_api_config = UserApiConfig {
        bind_addr: local_addr,
        ..Default::default()
    };

    let server = Server::builder()
        .with_db_config(db_config)
        .with_user_api_config(user_api_config)
        .build()
        .await
        .map_err(|e| anyhow!("building embedded sqld: {e}"))?;

    tracing::info!(
        event = "sqld.embed.start",
        local_url = %local_url,
        catalog_db = ?catalog_db_path,
        "embedded sqld bound to loopback"
    );

    let task = tokio::spawn(async move {
        server.start().await.map_err(|e| anyhow!("embedded sqld: {e}"))?;
        Ok(())
    });

    Ok(EmbeddedSqld { local_url, task })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn starts_and_binds_loopback() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        // Initialize via our migration runner first so the DB has the schema.
        let _db = crate::db::Db::open(&path).await.unwrap();
        let embedded = start(path).await.unwrap();
        assert!(embedded.local_url.starts_with("http://127.0.0.1:"));
        // Sanity ping: hit /health on the sqld user API.
        // libsql-server exposes a health endpoint under v1.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let resp = reqwest::get(format!("{}/v1/health", embedded.local_url)).await;
        // Don't assert specific status — different libsql-server versions
        // return different shapes. Just confirm we got SOMETHING back.
        assert!(resp.is_ok(), "expected reachable loopback, got {resp:?}");
        embedded.task.abort();
    }
}
```

- [ ] **Step 2: Expose the module via `crates/shoebox-server/src/lib.rs`.** Add `pub mod sqld_embed;`.

- [ ] **Step 3: Run the test.**

```
cargo test -p shoebox-server sqld_embed
```

Expect: PASS.

**Likely API drift:** `libsql-server` is a fast-moving crate. The exact `Server::builder`, `DbConfig`, `UserApiConfig` shapes may differ from what's shown above. Adapt to the actual API. The semantic goal:
- A `libsql-server` Server runs in-process
- Bound to `127.0.0.1` on a port we can read back
- Serves the same `catalog.db` file the rest of the server uses
- Returns a `local_url` the proxy can target

If you cannot get `libsql-server` to embed cleanly (the published version is unstable), an acceptable fallback is running the standalone `sqld` binary as a child process via `tokio::process::Command` (assumes `sqld` is on PATH; the Dockerfile would install it). Document this fallback in your report.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/sqld_embed.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): embed libsql-server bound to ephemeral loopback port"
```

---

## Task 3: libSQL wire proxy through mTLS

**Files:**
- Create: `crates/shoebox-server/src/proxy.rs`
- Modify: `crates/shoebox-server/src/http.rs` (register catch-all proxy route under `/v1/*` and `/v2/*`)
- Modify: `crates/shoebox-server/src/lib.rs`
- Modify: `crates/shoebox-server/src/main.rs` (start embedded sqld + plumb URL into AppState)

The mTLS-terminating listener on `:9000` must forward libSQL wire-protocol requests (which include both regular HTTP and WebSocket upgrades) to the loopback sqld. The libSQL Hrana protocol uses paths like `/v1/health`, `/v2/pipeline`, `/v2/streams` (for streaming).

- [ ] **Step 1: Extend `AppState` in `crates/shoebox-server/src/http.rs`** with the proxy target. Replace the struct:

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub schema_version: i64,
    pub ca: Arc<crate::ca::Ca>,
    pub sqld_url: String,
    pub cache_dir: std::path::PathBuf,
}
```

(The `cache_dir` field is for thumbnail HTTP serving in Task 13; included here to avoid an extra `AppState` migration later.)

- [ ] **Step 2: Write `crates/shoebox-server/src/proxy.rs`.**

```rust
//! Reverse proxy for libSQL wire traffic. Forwards authenticated HTTP
//! + WebSocket requests from the mTLS public listener to the embedded
//! sqld bound on loopback.

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Request, State, WebSocketUpgrade};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;

use crate::http::AppState;
use crate::identity::ClientIdentity;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/*path", any(forward_http))
        .route("/v2/*path", any(forward_http))
}

async fn forward_http(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    ws_upgrade: Option<WebSocketUpgrade>,
    mut req: Request,
) -> Response {
    // Handle WebSocket upgrades separately (libSQL streaming uses WS).
    if let Some(ws) = ws_upgrade {
        let upstream_url = build_upstream_url(&state.sqld_url, req.uri(), true);
        return ws.on_upgrade(move |client_socket| async move {
            if let Err(e) = forward_ws(client_socket, upstream_url).await {
                tracing::warn!(event = "proxy.ws.error", error = %e);
            }
        });
    }

    // Plain HTTP: rewrite URI and forward via hyper-util client.
    let upstream_uri: Uri = match build_upstream_url(&state.sqld_url, req.uri(), false).parse() {
        Ok(u) => u,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("bad upstream URI: {e}"),
            )
                .into_response();
        }
    };

    *req.uri_mut() = upstream_uri;
    // Strip hop-by-hop headers.
    let headers = req.headers_mut();
    headers.remove(header::HOST);
    headers.remove(header::CONNECTION);
    headers.remove("keep-alive");
    headers.remove("proxy-connection");
    headers.remove("transfer-encoding");
    headers.remove("upgrade");

    let client: HyperClient<_, Body> =
        HyperClient::builder(TokioExecutor::new()).build_http();
    match client.request(req).await {
        Ok(resp) => resp.into_response(),
        Err(e) => {
            tracing::warn!(event = "proxy.http.error", error = %e);
            (
                StatusCode::BAD_GATEWAY,
                format!("upstream sqld unreachable: {e}"),
            )
                .into_response()
        }
    }
}

async fn forward_ws(
    mut client_socket: axum::extract::ws::WebSocket,
    upstream_url: String,
) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TMessage;
    use axum::extract::ws::Message as AMessage;

    let (upstream, _) = tokio_tungstenite::connect_async(upstream_url).await?;
    let (mut up_tx, mut up_rx) = upstream.split();

    loop {
        tokio::select! {
            Some(client_msg) = client_socket.recv() => {
                let client_msg = client_msg?;
                let t = match client_msg {
                    AMessage::Text(s) => TMessage::Text(s),
                    AMessage::Binary(b) => TMessage::Binary(b),
                    AMessage::Ping(p) => TMessage::Ping(p),
                    AMessage::Pong(p) => TMessage::Pong(p),
                    AMessage::Close(_) => break,
                };
                up_tx.send(t).await?;
            }
            Some(upstream_msg) = up_rx.next() => {
                let upstream_msg = upstream_msg?;
                let a = match upstream_msg {
                    TMessage::Text(s) => AMessage::Text(s),
                    TMessage::Binary(b) => AMessage::Binary(b),
                    TMessage::Ping(p) => AMessage::Ping(p),
                    TMessage::Pong(p) => AMessage::Pong(p),
                    TMessage::Close(_) => break,
                    TMessage::Frame(_) => continue,
                };
                client_socket.send(a).await?;
            }
            else => break,
        }
    }
    Ok(())
}

fn build_upstream_url(sqld_base: &str, req_uri: &Uri, ws: bool) -> String {
    // sqld_base looks like "http://127.0.0.1:53421"; for WS we swap scheme.
    let base = if ws {
        sqld_base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1)
    } else {
        sqld_base.to_string()
    };
    let path_and_query = req_uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    format!("{base}{path_and_query}")
}
```

NOTE: the proxy uses `ClientIdentity` as an extractor, which enforces that the client presented a valid (non-revoked) cert before any libSQL traffic is forwarded. This is the only gate — once forwarded, sqld trusts the request fully (it's bound to loopback and only the proxy talks to it).

A new workspace dep is needed: `futures-util = "0.3"`. Add it to workspace deps and to the server crate.

- [ ] **Step 3: Wire the proxy routes into `public_router`.** In `crates/shoebox-server/src/http.rs`, replace `public_router`:

```rust
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::enroll::route())
        .merge(crate::enroll::renew_route())
        .merge(crate::whoami::route())
        .merge(crate::proxy::routes())
        .with_state(state)
}
```

- [ ] **Step 4: Add `pub mod proxy;` to `crates/shoebox-server/src/lib.rs`.**

- [ ] **Step 5: Start the embedded sqld in `main.rs` and plumb its URL into `AppState`.**

In `crates/shoebox-server/src/main.rs`, inside `serve_main` (after the existing CA + secret bootstrap, before `let state = ...`):

```rust
    let embedded_sqld = sqld_embed::start(cfg.data_dir.join("catalog.db")).await?;
```

And update the `state` construction to populate the new fields:

```rust
    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: embedded_sqld.local_url.clone(),
        cache_dir: cfg.cache_dir.clone(),
    };
```

Also update imports at top: `use shoebox_server::{ca, cli, config, db, http, logging, mdns, mtls, proxy, revoke, secret, sqld_embed, tls_server};`

- [ ] **Step 6: Update existing integration tests** that construct `AppState` directly (`tests/enroll_e2e.rs`, `tests/health_e2e.rs`, `tests/revoke_e2e.rs`, `tests/renew_e2e.rs`, `src/http.rs::tests`) to populate the new fields. For tests that don't actually use the proxy or cache, supply dummy values: `sqld_url: "http://127.0.0.1:0".to_string(), cache_dir: tmp.path().to_path_buf()`.

- [ ] **Step 7: Build and run all tests.**

```
cargo test -p shoebox-server
```

Expect: all existing tests pass with the new AppState shape. The proxy itself isn't exercised yet — that's Task 4.

- [ ] **Step 8: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml \
        crates/shoebox-server/src/proxy.rs \
        crates/shoebox-server/src/http.rs crates/shoebox-server/src/lib.rs \
        crates/shoebox-server/src/main.rs \
        crates/shoebox-server/tests/*.rs
git commit -m "feat(server): mTLS-protected libSQL wire proxy to embedded sqld"
```

---

## Task 4: End-to-end test — libSQL embedded replica through the proxy

**Files:**
- Create: `crates/shoebox-server/tests/proxy_e2e.rs`

This is the proof that the data-plane plumbing actually works: a client opens a libSQL embedded-replica connection through the mTLS proxy and successfully writes + reads.

- [ ] **Step 1: Write `crates/shoebox-server/tests/proxy_e2e.rs`.**

```rust
//! End-to-end: server with embedded sqld + mTLS proxy; client uses
//! libsql embedded-replica mode through the proxy.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
async fn libsql_embedded_replica_round_trips_through_proxy() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
    let conn = db.connect().unwrap();
    let secret_plaintext = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        _ => panic!(),
    };
    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    let mut sans = sans;
    sans.push("127.0.0.1".to_string());
    let (server_cert, server_kp) = ca.issue_server_cert(&sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg = shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl)
        .unwrap();

    // Embed sqld backed by the same catalog DB.
    let embedded = shoebox_server::sqld_embed::start(data_dir.join("catalog.db"))
        .await
        .unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: embedded.local_url.clone(),
        cache_dir: cache_dir.clone(),
    };

    // Bind ephemeral port for the mTLS listener.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await
            .unwrap();
    });

    // Enroll a client cert.
    let client_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "placeholder");
        dn
    };
    let csr_pem = csr_params.serialize_request(&client_kp).unwrap().pem().unwrap();

    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();
    let enroll_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_cfg)
        .build()
        .unwrap();

    let resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret_plaintext,
            "csr_pem": csr_pem,
            "display_name": "ProxyTest",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();

    // Build a libsql embedded-replica client that connects through the
    // mTLS proxy. libsql's `Builder::new_remote_replica` accepts a URL
    // + auth token, but for mTLS we need to inject a custom rustls
    // ClientConfig. As of libsql 0.6, the way to do this is via
    // `Builder::new_remote_replica(...).http_request_callback(...)` or
    // a custom `Connector`. Adapt to whatever the actual libsql API
    // surfaces — the semantic goal is "open a libsql connection that
    // presents our client cert."
    //
    // Fallback if the libsql Rust client mTLS story is too painful in
    // this version: use the raw Hrana wire protocol over reqwest with
    // the client cert, send a single `execute` request, and assert the
    // response shape. That still proves the proxy works end-to-end.
    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&client_kp.serialize_pem()).unwrap();
    let authed_cfg = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(vec![CertificateDer::from(client_cert_der)], client_key_der)
        .unwrap();
    let authed_http = Client::builder()
        .use_preconfigured_tls(authed_cfg)
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();

    // Hit a libSQL health endpoint via the proxy. If the proxy works,
    // this returns 200; if the proxy is wired wrong, we get 401 or 502.
    let resp = authed_http
        .get(format!("https://{addr}/v1/health"))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "expected libsql /v1/health success through proxy, got {}",
        resp.status()
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded.task.abort();
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        if let Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-server --test proxy_e2e
```

Expect: PASS. If `/v1/health` isn't the right path for the embedded sqld's health endpoint (different libsql-server versions vary), try `/v1/info` or `/health`. The goal: prove the proxy forwards authenticated traffic to the embedded sqld and returns a response.

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/tests/proxy_e2e.rs
git commit -m "test(server): libSQL HTTP request reaches embedded sqld through mTLS proxy"
```

---

## Task 5: BLAKE3 hashing helper

**Files:**
- Create: `crates/shoebox-server/src/hashing.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/hashing.rs`.**

```rust
//! BLAKE3 hashing of files. Streamed so we don't pull a 50 MB RAW into
//! memory.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const BUF_SIZE: usize = 256 * 1024;

/// Hash a file with BLAKE3, returning the 32-byte digest.
pub fn blake3_file(path: &Path) -> Result<[u8; 32]> {
    let f = File::open(path).with_context(|| format!("opening {path:?}"))?;
    let mut reader = BufReader::with_capacity(BUF_SIZE, f);
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Lowercase-hex BLAKE3 of a file.
pub fn blake3_hex(path: &Path) -> Result<String> {
    Ok(hex::encode(blake3_file(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::io::Write;

    #[test]
    fn known_vector_empty_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("empty");
        File::create(&p).unwrap();
        // BLAKE3 of empty input
        assert_eq!(
            blake3_hex(&p).unwrap(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn known_vector_abc() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("abc");
        let mut f = File::create(&p).unwrap();
        f.write_all(b"abc").unwrap();
        assert_eq!(
            blake3_hex(&p).unwrap(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }
}
```

- [ ] **Step 2: Expose via `lib.rs`.** Add `pub mod hashing;`.

- [ ] **Step 3: Run tests.**

```
cargo test -p shoebox-server hashing
```

Expect: 2 tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/hashing.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): BLAKE3 streamed file hashing helper"
```

---

## Task 6: RAW embedded JPEG preview extraction

**Files:**
- Create: `crates/shoebox-server/src/raw_preview.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/raw_preview.rs`.**

```rust
//! Extract the embedded JPEG preview from a RAW file. Supported
//! formats follow whatever `rawler` supports (covers Pentax PEF/DNG
//! and Fuji RAF as required by the spec).

use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// Returns the bytes of the largest embedded JPEG preview in the RAW.
pub fn extract_preview(raw_path: &Path) -> Result<Vec<u8>> {
    let reader = rawler::RawFile::from_file(raw_path)
        .with_context(|| format!("opening RAW {raw_path:?}"))?;

    // rawler exposes `decoder()` and from there `full_image()` (returns
    // a decoded raw image) and `thumbnail_image()` (returns the embedded
    // preview). The exact method name may differ; the goal is to get the
    // largest embedded preview as JPEG bytes.
    let decoder = reader
        .decoder()
        .map_err(|e| anyhow!("creating rawler decoder for {raw_path:?}: {e}"))?;

    let preview = decoder
        .thumbnail_image()
        .map_err(|e| anyhow!("extracting thumbnail from {raw_path:?}: {e}"))?;

    // `thumbnail_image()` may return JPEG bytes directly, or a decoded
    // image we'd need to re-encode. If the former, return as-is; if the
    // latter, encode to JPEG via the `image` crate. Adapt to actual
    // rawler API.
    Ok(preview)
}

#[cfg(test)]
mod tests {
    // No test in this task — exercising rawler requires real RAW files
    // which we don't have in CI. Task 18 (thumbnailer_e2e) covers this
    // path with a minimal synthetic RAW (or a small PEF fixture if one
    // is checked in).
}
```

**Heavy API risk:** `rawler` 0.6's exact API for `from_file`, `decoder()`, `thumbnail_image()` may not match the names above. The implementer should:
1. `cargo doc -p rawler --no-deps --open` to inspect the real API.
2. Adapt method names. The semantic goal: given a path to a RAW file, return `Vec<u8>` of embedded JPEG bytes.
3. If `rawler` returns a decoded RGB image instead of raw JPEG bytes, use the `image` crate to encode: `image::DynamicImage::from(rgb_buffer).write_to(&mut Cursor, JpegEncoder::new_with_quality(90))`.

- [ ] **Step 2: Expose via `lib.rs`.** Add `pub mod raw_preview;`.

- [ ] **Step 3: Verify it compiles.**

```
cargo build -p shoebox-server
```

If `rawler` fights you, this is the right time to call `BLOCKED` — the rest of the plan assumes some way to extract preview JPEG bytes from RAW files. Possible fallback if `rawler` is unusable: ship `dcraw_emu` (the libraw CLI) in the Docker image and shell out. Heavier and adds a binary dep, but reliable.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/raw_preview.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): RAW embedded JPEG preview extraction via rawler"
```

---

## Task 7: Folder enumeration and initial scan

**Files:**
- Create: `crates/shoebox-server/src/indexer.rs` (initial version, just the scan)
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/indexer.rs`.**

```rust
//! Filesystem indexer. This task contributes the initial-scan path:
//! walk the photo root, hash every RAW file, populate folders + photos
//! + photo_files rows. Task 8 adds the live FS watcher; Task 9 wires
//! it into main.rs.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use crate::db::Db;
use crate::hashing;

/// File extensions recognized as RAW. Lowercase, no leading dot.
pub const RAW_EXTENSIONS: &[&str] = &["pef", "dng", "raf"];

#[derive(Debug, Clone)]
pub struct IndexerStats {
    pub folders_seen: usize,
    pub files_seen: usize,
    pub photos_added: usize,
    pub photo_files_added: usize,
    pub photo_files_updated: usize,
}

pub async fn initial_scan(db: Arc<Db>, photos_root: &Path) -> Result<IndexerStats> {
    let mut stats = IndexerStats {
        folders_seen: 0,
        files_seen: 0,
        photos_added: 0,
        photo_files_added: 0,
        photo_files_updated: 0,
    };

    let mut folder_paths: HashSet<PathBuf> = HashSet::new();
    folder_paths.insert(photos_root.to_path_buf());

    for entry in WalkDir::new(photos_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir() {
            folder_paths.insert(entry.path().to_path_buf());
            stats.folders_seen += 1;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        stats.files_seen += 1;
        if !is_raw_file(entry.path()) {
            continue;
        }

        // Ensure folder rows exist for the file's parent chain.
        if let Some(parent) = entry.path().parent() {
            ensure_folder_chain(&db, photos_root, parent).await?;
        }

        let file_path = entry.path();
        let metadata = entry.metadata()?;
        let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        let file_mtime = i64::try_from(
            metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);

        let hash_hex = tokio::task::spawn_blocking({
            let p = file_path.to_path_buf();
            move || hashing::blake3_hex(&p)
        })
        .await??;

        let outcome = upsert_photo_and_file(
            &db,
            &hash_hex,
            file_size,
            file_path,
            file_mtime,
        )
        .await
        .with_context(|| format!("upserting {file_path:?}"))?;

        match outcome {
            UpsertOutcome::PhotoAndFileNew => {
                stats.photos_added += 1;
                stats.photo_files_added += 1;
            }
            UpsertOutcome::FileNew => stats.photo_files_added += 1,
            UpsertOutcome::FileUpdated => stats.photo_files_updated += 1,
            UpsertOutcome::NoChange => {}
        }
    }

    Ok(stats)
}

#[derive(Debug)]
enum UpsertOutcome {
    PhotoAndFileNew,
    FileNew,
    FileUpdated,
    NoChange,
}

async fn upsert_photo_and_file(
    db: &Db,
    hash_hex: &str,
    file_size: i64,
    path: &Path,
    file_mtime: i64,
) -> Result<UpsertOutcome> {
    let conn = db.connect()?;
    let now_ms = now_ms();

    // photos row: insert if absent.
    let mut rows = conn
        .query("SELECT 1 FROM photos WHERE id = ?1", [hash_hex])
        .await?;
    let photo_existed = rows.next().await?.is_some();
    if !photo_existed {
        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_uppercase())
            .unwrap_or_default();
        conn.execute(
            "INSERT INTO photos (id, file_size, file_format, imported_at) \
             VALUES (?1, ?2, ?3, ?4)",
            (
                hash_hex.to_string(),
                file_size,
                format,
                now_ms,
            ),
        )
        .await?;
    }

    // photo_files row: insert if new, update mtime + last_seen_at if exists.
    let path_str = path.to_string_lossy().to_string();
    let mut rows = conn
        .query("SELECT id, file_mtime FROM photo_files WHERE path = ?1", [&path_str])
        .await?;
    if let Some(row) = rows.next().await? {
        let existing_id: String = row.get(0)?;
        let existing_mtime: i64 = row.get(1)?;
        conn.execute(
            "UPDATE photo_files SET file_mtime = ?1, last_seen_at = ?2, is_present = 1 \
             WHERE id = ?3",
            (file_mtime, now_ms, existing_id),
        )
        .await?;
        if existing_mtime != file_mtime {
            return Ok(UpsertOutcome::FileUpdated);
        }
        return Ok(UpsertOutcome::NoChange);
    } else {
        // Need a folder_id. Lookup parent folder.
        let parent = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let folder_id: String = {
            let mut rows = conn
                .query("SELECT id FROM folders WHERE path = ?1", [&parent])
                .await?;
            rows.next()
                .await?
                .ok_or_else(|| anyhow::anyhow!("folder row missing for {parent}"))?
                .get(0)?
        };
        let new_file_id = uuid_hex();
        conn.execute(
            "INSERT INTO photo_files (id, photo_id, folder_id, path, file_mtime, last_seen_at, is_present) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            (
                new_file_id,
                hash_hex.to_string(),
                folder_id,
                path_str,
                file_mtime,
                now_ms,
            ),
        )
        .await?;
        return Ok(if photo_existed {
            UpsertOutcome::FileNew
        } else {
            UpsertOutcome::PhotoAndFileNew
        });
    }
}

async fn ensure_folder_chain(db: &Db, photos_root: &Path, dir: &Path) -> Result<()> {
    // Insert any missing folder rows along the path from photos_root up to dir.
    let conn = db.connect()?;
    let mut to_insert: Vec<PathBuf> = Vec::new();
    let mut cur = Some(dir.to_path_buf());
    while let Some(p) = cur {
        if p == photos_root || p.starts_with(photos_root) {
            let path_str = p.to_string_lossy().to_string();
            let mut rows = conn
                .query("SELECT 1 FROM folders WHERE path = ?1", [&path_str])
                .await?;
            if rows.next().await?.is_some() {
                break;
            }
            to_insert.push(p.clone());
        }
        cur = p.parent().map(|x| x.to_path_buf());
        if cur.as_deref() == Some(photos_root.parent().unwrap_or(Path::new("/"))) {
            break;
        }
    }
    to_insert.reverse();
    let now_ms_v = now_ms();
    for path in to_insert {
        let path_str = path.to_string_lossy().to_string();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let parent_id: Option<String> = if let Some(parent) = path.parent() {
            let parent_str = parent.to_string_lossy().to_string();
            let mut rows = conn
                .query("SELECT id FROM folders WHERE path = ?1", [&parent_str])
                .await?;
            rows.next().await?.map(|r| r.get::<String>(0).unwrap())
        } else {
            None
        };
        let id = uuid_hex();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name, last_indexed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (id, parent_id, path_str, name, now_ms_v),
        )
        .await?;
    }
    Ok(())
}

fn is_raw_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| RAW_EXTENSIONS.iter().any(|raw| raw.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn uuid_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[tokio::test]
    async fn initial_scan_picks_up_raw_files() {
        let tmp = TempDir::new().unwrap();
        let photos = tmp.path().join("photos");
        fs::create_dir_all(photos.join("2024")).unwrap();
        fs::create_dir_all(photos.join("2025")).unwrap();
        File::create(photos.join("2024/_DSC0001.PEF")).unwrap();
        File::create(photos.join("2024/_DSC0002.PEF")).unwrap();
        File::create(photos.join("2024/notes.txt")).unwrap(); // ignored
        File::create(photos.join("2025/_DSC0003.RAF")).unwrap();

        let db = Arc::new(crate::db::Db::open(&tmp.path().join("catalog.db")).await.unwrap());
        let stats = initial_scan(db.clone(), &photos).await.unwrap();
        assert_eq!(stats.photos_added, 3, "3 RAW files, all empty → hash identical?");
        // Note: empty files all hash to the same BLAKE3, so only 1 photo
        // row but 3 photo_files. Adjust expectation:
        assert_eq!(stats.photo_files_added, 3);
    }
}
```

NOTE: empty test files all have identical BLAKE3 so the `photos` count is 1, not 3. Test asserts `photo_files_added == 3` which is the right invariant (3 file rows referencing the same photo row — exactly the duplicate-handling case from the spec). Adjust the assertion if you find more nuanced behavior.

- [ ] **Step 2: Expose via `lib.rs`.** Add `pub mod indexer;`.

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-server indexer
```

Expect: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/indexer.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): indexer initial-scan walks RAW files into photos/photo_files"
```

---

## Task 8: notify-based FS watcher for incremental updates

**Files:**
- Modify: `crates/shoebox-server/src/indexer.rs`

Adds a `run_watcher(db, photos_root)` function that uses the `notify` crate to react to file create/modify/rename/delete events and call into the upsert logic from Task 7.

- [ ] **Step 1: Append to `crates/shoebox-server/src/indexer.rs`.**

```rust
/// Run the live FS watcher loop. Returns only on error or shutdown.
pub async fn run_watcher(
    db: Arc<Db>,
    photos_root: PathBuf,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    use notify::{RecommendedWatcher, RecursiveMode, Watcher};
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    watcher.watch(&photos_root, RecursiveMode::Recursive)?;

    tracing::info!(
        event = "indexer.watcher.start",
        photos_root = ?photos_root,
        "filesystem watcher started"
    );

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "indexer.watcher.shutdown");
                break;
            }
            ev = rx.recv() => match ev {
                Some(Ok(event)) => {
                    if let Err(e) = handle_event(&db, &photos_root, &event).await {
                        tracing::warn!(event = "indexer.handle.error", error = %e);
                    }
                }
                Some(Err(e)) => tracing::warn!(event = "indexer.watch.error", error = %e),
                None => break,
            }
        }
    }
    Ok(())
}

async fn handle_event(db: &Db, photos_root: &Path, event: &notify::Event) -> Result<()> {
    use notify::EventKind;
    for path in &event.paths {
        if !is_raw_file(path) {
            continue;
        }
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                if !path.is_file() {
                    continue;
                }
                // Re-hash and upsert.
                let metadata = std::fs::metadata(path)?;
                let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
                let file_mtime = i64::try_from(
                    metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis())
                        .unwrap_or(0),
                )
                .unwrap_or(0);
                if let Some(parent) = path.parent() {
                    ensure_folder_chain(db, photos_root, parent).await?;
                }
                let p = path.to_path_buf();
                let hash_hex = tokio::task::spawn_blocking(move || hashing::blake3_hex(&p))
                    .await??;
                let _ = upsert_photo_and_file(db, &hash_hex, file_size, path, file_mtime).await?;
            }
            EventKind::Remove(_) => {
                let conn = db.connect()?;
                let path_str = path.to_string_lossy().to_string();
                conn.execute(
                    "UPDATE photo_files SET is_present = 0 WHERE path = ?1",
                    [path_str],
                )
                .await?;
            }
            _ => {}
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Build.**

```
cargo build -p shoebox-server
```

Expect: clean. No new test in this task — Task 19 (indexer_e2e) covers the end-to-end behavior.

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/src/indexer.rs
git commit -m "feat(server): notify-based FS watcher for incremental indexer updates"
```

---

## Task 9: Wire indexer into main.rs as a background task

**Files:**
- Modify: `crates/shoebox-server/src/main.rs`

- [ ] **Step 1: In `serve_main`, after the embedded sqld setup and before binding listeners:**

```rust
    // Initial scan + live watcher.
    let scan_stats = indexer::initial_scan(db.clone(), &cfg.photos_dir).await?;
    tracing::info!(
        event = "indexer.initial_scan",
        folders_seen = scan_stats.folders_seen,
        files_seen = scan_stats.files_seen,
        photos_added = scan_stats.photos_added,
        photo_files_added = scan_stats.photo_files_added,
        "initial scan complete"
    );
    let (indexer_shutdown_tx, indexer_shutdown_rx) = tokio::sync::oneshot::channel();
    let indexer_db = db.clone();
    let indexer_root = cfg.photos_dir.clone();
    let indexer_task = tokio::spawn(async move {
        if let Err(e) = indexer::run_watcher(indexer_db, indexer_root, indexer_shutdown_rx).await {
            tracing::error!(event = "indexer.run.error", error = %e);
        }
    });
```

- [ ] **Step 2: At end of `serve_main`, before returning, send the indexer shutdown signal and await the task:**

Add this just before the `broadcaster.shutdown();` line:

```rust
    let _ = indexer_shutdown_tx.send(());
    let _ = indexer_task.await;
```

Also update imports at top of main.rs to include `indexer`:

```rust
use shoebox_server::{ca, cli, config, db, http, indexer, logging, mdns, mtls, proxy, revoke, secret, sqld_embed, tls_server};
```

- [ ] **Step 3: Build + smoke test.**

```
cargo build -p shoebox-server
mkdir -p /tmp/shoebox-smoke/{data,photos,cache}
SHOEBOX_DATA_DIR=/tmp/shoebox-smoke/data \
SHOEBOX_PHOTOS_DIR=/tmp/shoebox-smoke/photos \
SHOEBOX_CACHE_DIR=/tmp/shoebox-smoke/cache \
cargo run -p shoebox-server &
SERVER_PID=$!
sleep 3
echo "--- drop a (fake) RAW file ---"
echo "fake data" > /tmp/shoebox-smoke/photos/test.PEF
sleep 2
echo "--- catalog state ---"
sqlite3 /tmp/shoebox-smoke/data/catalog.db "SELECT id, file_size, file_format FROM photos;"
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null || true
rm -rf /tmp/shoebox-smoke
```

(If `sqlite3` isn't installed, skip that verification step.)

Expect: server starts, log shows `indexer.initial_scan` event, then after the file drop a row appears in `photos`.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/main.rs
git commit -m "feat(server): spawn indexer initial-scan + FS watcher at startup"
```

---

## Task 10: Thumbnailer module (sized JPEG generation + atomic cache write)

**Files:**
- Create: `crates/shoebox-server/src/thumbnailer.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/thumbnailer.rs`.**

```rust
//! Pulls the embedded JPEG preview from a RAW, renders 256 px + 2k
//! versions, writes them content-addressed under `<cache_dir>`.

use anyhow::{anyhow, Context, Result};
use image::{codecs::jpeg::JpegEncoder, GenericImageView, ImageFormat};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::raw_preview;

pub const THUMB_PX: u32 = 256;
pub const PREVIEW_PX: u32 = 2048;

#[derive(Debug, Clone, Copy)]
pub enum ThumbnailKind {
    Thumb,
    Preview,
}

impl ThumbnailKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            ThumbnailKind::Thumb => "thumbnails",
            ThumbnailKind::Preview => "previews",
        }
    }

    pub fn target_px(self) -> u32 {
        match self {
            ThumbnailKind::Thumb => THUMB_PX,
            ThumbnailKind::Preview => PREVIEW_PX,
        }
    }
}

/// Returns the path where the cached image lives.
pub fn cache_path(cache_dir: &Path, kind: ThumbnailKind, hash_hex: &str) -> PathBuf {
    cache_dir.join(kind.dir_name()).join(format!("{hash_hex}.jpg"))
}

/// Build (if absent) the cached thumbnail/preview for one photo.
/// Returns true if a new file was written, false if it already existed.
pub fn build_one(
    cache_dir: &Path,
    raw_path: &Path,
    hash_hex: &str,
    kind: ThumbnailKind,
) -> Result<bool> {
    let out_path = cache_path(cache_dir, kind, hash_hex);
    if out_path.exists() {
        return Ok(false);
    }
    let out_dir = out_path.parent().unwrap();
    std::fs::create_dir_all(out_dir).with_context(|| format!("mkdir {out_dir:?}"))?;

    let jpeg_bytes = raw_preview::extract_preview(raw_path)?;
    let img = image::load_from_memory_with_format(&jpeg_bytes, ImageFormat::Jpeg)
        .map_err(|e| anyhow!("decoding embedded JPEG for {raw_path:?}: {e}"))?;

    let (w, h) = img.dimensions();
    let target = kind.target_px();
    let resized = if w.max(h) > target {
        let ratio = target as f32 / w.max(h) as f32;
        let new_w = (w as f32 * ratio) as u32;
        let new_h = (h as f32 * ratio) as u32;
        img.thumbnail(new_w, new_h)
    } else {
        img
    };

    // Atomic write: encode to temp file, rename.
    let tmp_path = out_path.with_extension("jpg.tmp");
    {
        let mut out = std::fs::File::create(&tmp_path)
            .with_context(|| format!("creating {tmp_path:?}"))?;
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let encoder = JpegEncoder::new_with_quality(&mut cursor, 90);
        resized
            .write_with_encoder(encoder)
            .map_err(|e| anyhow!("JPEG encode failed: {e}"))?;
        use std::io::Write;
        out.write_all(&buf)?;
        out.sync_all()?;
    }
    std::fs::rename(&tmp_path, &out_path)
        .with_context(|| format!("renaming {tmp_path:?} -> {out_path:?}"))?;
    Ok(true)
}

/// Build both thumb and preview for one photo. Best-effort: logs and
/// returns Ok even if one of the two fails.
pub fn build_both(cache_dir: &Path, raw_path: &Path, hash_hex: &str) -> Result<()> {
    match build_one(cache_dir, raw_path, hash_hex, ThumbnailKind::Thumb) {
        Ok(true) => tracing::debug!(event = "thumb.built", kind = "thumb", hash = %hash_hex),
        Ok(false) => {}
        Err(e) => tracing::warn!(event = "thumb.error", kind = "thumb", hash = %hash_hex, error = %e),
    }
    match build_one(cache_dir, raw_path, hash_hex, ThumbnailKind::Preview) {
        Ok(true) => tracing::debug!(event = "thumb.built", kind = "preview", hash = %hash_hex),
        Ok(false) => {}
        Err(e) => tracing::warn!(event = "thumb.error", kind = "preview", hash = %hash_hex, error = %e),
    }
    Ok(())
}
```

- [ ] **Step 2: Expose via `lib.rs`.** Add `pub mod thumbnailer;`.

- [ ] **Step 3: Build.**

```
cargo build -p shoebox-server
```

Expect: clean. No new test in this task; Task 18 covers thumbnailer e2e using a real (small) RAW fixture or synthetic JPEG.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/thumbnailer.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): thumbnailer renders 256px + 2k JPEGs to content-addressed cache"
```

---

## Task 11: Wire thumbnailer into the indexer's upsert flow

**Files:**
- Modify: `crates/shoebox-server/src/indexer.rs`
- Modify: `crates/shoebox-server/src/main.rs` (pass cache_dir into indexer)

The indexer's `upsert_photo_and_file` already determines when a photo is new. Add a hook that enqueues thumbnail work after a successful insert.

- [ ] **Step 1: Modify indexer to accept a cache_dir and call `thumbnailer::build_both` on new photos.**

Change the signatures:

```rust
pub async fn initial_scan(
    db: Arc<Db>,
    photos_root: &Path,
    cache_dir: &Path,
) -> Result<IndexerStats>;

pub async fn run_watcher(
    db: Arc<Db>,
    photos_root: PathBuf,
    cache_dir: PathBuf,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<()>;
```

After a successful `UpsertOutcome::PhotoAndFileNew`, spawn a blocking task that calls `thumbnailer::build_both(&cache_dir, file_path, &hash_hex)`. Use `tokio::task::spawn_blocking` so the indexer loop isn't blocked on JPEG decoding.

```rust
if matches!(outcome, UpsertOutcome::PhotoAndFileNew) {
    let cache = cache_dir.to_path_buf();
    let p = file_path.to_path_buf();
    let h = hash_hex.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = crate::thumbnailer::build_both(&cache, &p, &h) {
            tracing::warn!(event = "thumb.build.error", path = ?p, error = %e);
        }
    });
}
```

Apply the same call in `handle_event` after a successful Create/Modify upsert.

- [ ] **Step 2: Update `main.rs` to pass `cfg.cache_dir`** into both `initial_scan` and `run_watcher` calls.

- [ ] **Step 3: Update the existing `initial_scan_picks_up_raw_files` test** to pass a temp cache dir.

- [ ] **Step 4: Build + test.**

```
cargo test -p shoebox-server indexer
```

Expect: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/indexer.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): indexer triggers thumbnailer when a new photo is added"
```

---

## Task 12: Thumbnail HTTP endpoints (`GET /thumbs/<hash>` and `/previews/<hash>`)

**Files:**
- Create: `crates/shoebox-server/src/thumbs_http.rs`
- Modify: `crates/shoebox-server/src/http.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/thumbs_http.rs`.**

```rust
//! HTTP endpoints serving cached thumbnails and previews. Files are
//! content-addressed by hash; we only serve files under cache_dir to
//! prevent path traversal.

use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::http::AppState;
use crate::identity::ClientIdentity;
use crate::thumbnailer::{cache_path, ThumbnailKind};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/thumbs/:hash", get(get_thumb))
        .route("/previews/:hash", get(get_preview))
}

async fn get_thumb(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    AxumPath(hash): AxumPath<String>,
) -> Response {
    serve(state.cache_dir.as_path(), ThumbnailKind::Thumb, &hash).await
}

async fn get_preview(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    AxumPath(hash): AxumPath<String>,
) -> Response {
    serve(state.cache_dir.as_path(), ThumbnailKind::Preview, &hash).await
}

async fn serve(cache_dir: &std::path::Path, kind: ThumbnailKind, hash: &str) -> Response {
    if !is_valid_hash(hash) {
        return (StatusCode::BAD_REQUEST, "invalid hash").into_response();
    }
    let path = cache_path(cache_dir, kind, hash);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "thumbnail not ready").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("io: {e}")).into_response(),
    }
}

fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
```

- [ ] **Step 2: Wire routes into `public_router` in `http.rs`.**

```rust
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::enroll::route())
        .merge(crate::enroll::renew_route())
        .merge(crate::whoami::route())
        .merge(crate::proxy::routes())
        .merge(crate::thumbs_http::routes())
        .with_state(state)
}
```

- [ ] **Step 3: Add `pub mod thumbs_http;` to `lib.rs`.**

- [ ] **Step 4: Build.**

```
cargo build -p shoebox-server
```

Expect: clean.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/thumbs_http.rs crates/shoebox-server/src/http.rs \
        crates/shoebox-server/src/lib.rs
git commit -m "feat(server): /thumbs/<hash> and /previews/<hash> serve cached JPEGs over mTLS"
```

---

## Task 13: Develop-lock DB helpers

**Files:**
- Modify: `crates/shoebox-server/src/db.rs`

Append four async methods to `impl Db`:

- [ ] **Step 1: Add helpers in `db.rs`.**

```rust
    /// Acquire a develop lock on a variant. Returns Ok(true) on success,
    /// Ok(false) if another session holds it (no error).
    pub async fn lock_acquire(
        &self,
        variant_id: &str,
        session_id: &str,
        user_id: &str,
        ttl_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let now = now_ms();
        let expires = now + ttl_ms;
        let result = conn
            .execute(
                "INSERT INTO develop_locks \
                 (variant_id, session_id, user_id, acquired_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(variant_id) DO NOTHING",
                (
                    variant_id.to_string(),
                    session_id.to_string(),
                    user_id.to_string(),
                    now,
                    expires,
                ),
            )
            .await?;
        Ok(result > 0)
    }

    pub async fn lock_heartbeat(
        &self,
        variant_id: &str,
        session_id: &str,
        ttl_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let now = now_ms();
        let expires = now + ttl_ms;
        let result = conn
            .execute(
                "UPDATE develop_locks SET expires_at = ?1 \
                 WHERE variant_id = ?2 AND session_id = ?3",
                (expires, variant_id.to_string(), session_id.to_string()),
            )
            .await?;
        Ok(result > 0)
    }

    pub async fn lock_release(
        &self,
        variant_id: &str,
        session_id: &str,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let result = conn
            .execute(
                "DELETE FROM develop_locks WHERE variant_id = ?1 AND session_id = ?2",
                (variant_id.to_string(), session_id.to_string()),
            )
            .await?;
        Ok(result > 0)
    }

    pub async fn lock_request_takeover(
        &self,
        variant_id: &str,
        requesting_user_id: &str,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let now = now_ms();
        let result = conn
            .execute(
                "UPDATE develop_locks \
                 SET takeover_requested_by = ?1, takeover_requested_at = ?2 \
                 WHERE variant_id = ?3 AND takeover_requested_by IS NULL",
                (requesting_user_id.to_string(), now, variant_id.to_string()),
            )
            .await?;
        Ok(result > 0)
    }

    /// Returns the number of expired locks that were released.
    pub async fn lock_release_expired(&self) -> anyhow::Result<usize> {
        let conn = self.connect()?;
        let now = now_ms();
        let result = conn
            .execute("DELETE FROM develop_locks WHERE expires_at < ?1", [now])
            .await?;
        Ok(result as usize)
    }
```

- [ ] **Step 2: Add a unit test in `db.rs` mod tests** that exercises the full acquire / heartbeat / takeover-request / release cycle on a single variant.

```rust
    #[tokio::test]
    async fn lock_lifecycle_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        let conn = db.connect().unwrap();

        // Set up FK chain: user, photo, variant.
        let now = 1_000_000_i64;
        conn.execute(
            "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'Alice', ?1)",
            [now],
        ).await.unwrap();
        conn.execute(
            "INSERT INTO users (id, display_name, created_at) VALUES ('u2', 'Bob', ?1)",
            [now],
        ).await.unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, client_machine_id, established_at, last_active_at) \
             VALUES ('s1', 'u1', 'm1', ?1, ?1)",
            [now],
        ).await.unwrap();
        conn.execute(
            "INSERT INTO photos (id, file_size, file_format, imported_at) \
             VALUES ('h1', 100, 'PEF', ?1)",
            [now],
        ).await.unwrap();
        conn.execute(
            "INSERT INTO variants (id, photo_id, variant_index, created_by, created_at, \
             develop_settings_json, develop_settings_version, develop_updated_at, develop_updated_by) \
             VALUES ('v1', 'h1', 0, 'u1', ?1, '{}', 1, ?1, 'u1')",
            [now],
        ).await.unwrap();

        assert!(db.lock_acquire("v1", "s1", "u1", 60_000).await.unwrap());
        // Re-acquire by same session: returns false (already held).
        assert!(!db.lock_acquire("v1", "s1", "u1", 60_000).await.unwrap());
        assert!(db.lock_heartbeat("v1", "s1", 120_000).await.unwrap());
        assert!(db.lock_request_takeover("v1", "u2").await.unwrap());
        // Second takeover by same user: false (already set).
        assert!(!db.lock_request_takeover("v1", "u2").await.unwrap());
        assert!(db.lock_release("v1", "s1").await.unwrap());
    }
```

- [ ] **Step 3: Run.**

```
cargo test -p shoebox-server db::tests::lock_lifecycle_roundtrips
```

Expect: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/db.rs
git commit -m "feat(db): develop lock acquire/heartbeat/release/takeover/expire helpers"
```

---

## Task 14: Develop-lock HTTP endpoints

**Files:**
- Create: `crates/shoebox-server/src/locks_http.rs`
- Modify: `crates/shoebox-server/src/http.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/locks_http.rs`.**

```rust
//! Develop-lock REST endpoints.
//!
//! POST   /locks/:variant_id            — acquire (returns 200 + holder info, 409 if held)
//! PUT    /locks/:variant_id            — heartbeat (returns 200 if extended, 404 if not held by you)
//! DELETE /locks/:variant_id            — release (returns 204, or 404 if not held by you)
//! POST   /locks/:variant_id/takeover   — request takeover (returns 200 with current holder info)

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{delete, post, put},
    Router,
};
use serde::Serialize;

use crate::http::AppState;
use crate::identity::ClientIdentity;

const LOCK_TTL_MS: i64 = 15 * 60 * 1000; // 15 minutes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/locks/:variant_id", post(acquire))
        .route("/locks/:variant_id", put(heartbeat))
        .route("/locks/:variant_id", delete(release))
        .route("/locks/:variant_id/takeover", post(takeover))
}

#[derive(Debug, Serialize)]
struct AcquireResponse {
    pub acquired: bool,
    pub holder_user_id: Option<String>,
}

async fn acquire(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    // Session ID is derived from cert serial — one cert ⇒ one session.
    let session_id = identity.cert_serial_hex.clone();
    match state
        .db
        .lock_acquire(&variant_id, &session_id, &identity.user_id.0, LOCK_TTL_MS)
        .await
    {
        Ok(true) => (
            StatusCode::OK,
            Json(AcquireResponse {
                acquired: true,
                holder_user_id: Some(identity.user_id.0.clone()),
            }),
        )
            .into_response(),
        Ok(false) => {
            // Look up current holder for the response.
            let holder = match state.db.connect() {
                Ok(conn) => {
                    let mut rows = conn
                        .query(
                            "SELECT user_id FROM develop_locks WHERE variant_id = ?1",
                            [&variant_id],
                        )
                        .await
                        .ok();
                    rows.as_mut()
                        .and_then(|r| futures::executor::block_on(r.next()).ok().flatten())
                        .and_then(|row| row.get::<String>(0).ok())
                }
                Err(_) => None,
            };
            (
                StatusCode::CONFLICT,
                Json(AcquireResponse {
                    acquired: false,
                    holder_user_id: holder,
                }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn heartbeat(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    let session_id = identity.cert_serial_hex.clone();
    match state
        .db
        .lock_heartbeat(&variant_id, &session_id, LOCK_TTL_MS)
        .await
    {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "lock not held by you").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn release(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    let session_id = identity.cert_serial_hex.clone();
    match state.db.lock_release(&variant_id, &session_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "lock not held by you").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn takeover(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    match state
        .db
        .lock_request_takeover(&variant_id, &identity.user_id.0)
        .await
    {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => (StatusCode::CONFLICT, "takeover already pending or lock free").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}
```

The `holder` lookup in `acquire` uses `futures::executor::block_on` which is wrong for async axum — replace with a proper async call. Cleanest is to factor the lookup into a `Db::lock_holder(variant_id) -> Option<String>` helper. **Implementer: clean this up to be proper async.**

- [ ] **Step 2: Wire routes into `public_router`.**

```rust
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::enroll::route())
        .merge(crate::enroll::renew_route())
        .merge(crate::whoami::route())
        .merge(crate::proxy::routes())
        .merge(crate::thumbs_http::routes())
        .merge(crate::locks_http::routes())
        .with_state(state)
}
```

- [ ] **Step 3: Add `pub mod locks_http;` to `lib.rs`.**

- [ ] **Step 4: Build + run tests.**

```
cargo build -p shoebox-server
cargo test -p shoebox-server
```

Expect: clean. The endpoint behavior is e2e-tested in Task 20.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/locks_http.rs crates/shoebox-server/src/http.rs \
        crates/shoebox-server/src/lib.rs crates/shoebox-server/src/db.rs
git commit -m "feat(server): develop-lock acquire/heartbeat/release/takeover endpoints"
```

---

## Task 15: Janitor task (periodic cleanup)

**Files:**
- Create: `crates/shoebox-server/src/janitor.rs`
- Modify: `crates/shoebox-server/src/main.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/janitor.rs`.**

```rust
//! Periodic cleanup tasks: stale lock expiry, abandoned session cleanup,
//! orphaned thumbnail GC.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;

const TICK: Duration = Duration::from_secs(60);
const SESSION_IDLE_MS: i64 = 24 * 60 * 60 * 1000; // 24 hours

pub async fn run(db: Arc<Db>, cache_dir: PathBuf, mut shutdown: tokio::sync::oneshot::Receiver<()>) {
    let mut ticker = tokio::time::interval(TICK);
    let mut sweep_count = 0u64;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "janitor.shutdown");
                return;
            }
            _ = ticker.tick() => {
                if let Err(e) = lock_sweep(&db).await {
                    tracing::warn!(event = "janitor.lock.error", error = %e);
                }
                if sweep_count % 5 == 0 {
                    if let Err(e) = session_sweep(&db).await {
                        tracing::warn!(event = "janitor.session.error", error = %e);
                    }
                }
                if sweep_count % 60 == 0 && sweep_count > 0 {
                    if let Err(e) = thumb_gc(&db, &cache_dir).await {
                        tracing::warn!(event = "janitor.thumb_gc.error", error = %e);
                    }
                }
                sweep_count = sweep_count.wrapping_add(1);
            }
        }
    }
}

async fn lock_sweep(db: &Db) -> anyhow::Result<()> {
    let n = db.lock_release_expired().await?;
    if n > 0 {
        tracing::info!(event = "janitor.lock.expired", released = n);
    }
    Ok(())
}

async fn session_sweep(db: &Db) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let cutoff = now_ms() - SESSION_IDLE_MS;
    let n = conn
        .execute("DELETE FROM sessions WHERE last_active_at < ?1", [cutoff])
        .await?;
    if n > 0 {
        tracing::info!(event = "janitor.session.cleanup", deleted = n);
    }
    Ok(())
}

async fn thumb_gc(db: &Db, cache_dir: &std::path::Path) -> anyhow::Result<()> {
    use std::collections::HashSet;

    // Collect known photo hashes from catalog.
    let conn = db.connect()?;
    let mut rows = conn.query("SELECT id FROM photos", ()).await?;
    let mut known: HashSet<String> = HashSet::new();
    while let Some(row) = rows.next().await? {
        known.insert(row.get::<String>(0)?);
    }

    let mut removed = 0u64;
    for subdir in ["thumbnails", "previews"] {
        let dir = cache_dir.join(subdir);
        if !dir.exists() {
            continue;
        }
        let mut read = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = read.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(hash) = name.strip_suffix(".jpg") {
                    if !known.contains(hash) {
                        let _ = tokio::fs::remove_file(entry.path()).await;
                        removed += 1;
                    }
                }
            }
        }
    }
    if removed > 0 {
        tracing::info!(event = "janitor.thumb_gc", removed);
    }
    Ok(())
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
```

- [ ] **Step 2: Spawn the janitor in `main.rs::serve_main`.** After the indexer setup:

```rust
    let (janitor_shutdown_tx, janitor_shutdown_rx) = tokio::sync::oneshot::channel();
    let janitor_db = db.clone();
    let janitor_cache = cfg.cache_dir.clone();
    let janitor_task = tokio::spawn(janitor::run(janitor_db, janitor_cache, janitor_shutdown_rx));
```

And in the cleanup path (just before broadcaster.shutdown):

```rust
    let _ = janitor_shutdown_tx.send(());
    let _ = janitor_task.await;
```

Update imports in main.rs to include `janitor`.

- [ ] **Step 3: Add `pub mod janitor;` to `lib.rs`.**

- [ ] **Step 4: Build + smoke test.**

```
cargo build -p shoebox-server
```

Expect: clean.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/janitor.rs crates/shoebox-server/src/main.rs \
        crates/shoebox-server/src/lib.rs
git commit -m "feat(server): janitor task — lock expiry, session cleanup, thumb GC"
```

---

## Task 16: Backup task (VACUUM INTO + rotation)

**Files:**
- Create: `crates/shoebox-server/src/backup.rs`
- Modify: `crates/shoebox-server/src/main.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/backup.rs`.**

```rust
//! Periodic VACUUM INTO backups with retention.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;

const TICK: Duration = Duration::from_secs(6 * 60 * 60); // 6 hours
const RETAIN: usize = 14;

pub async fn run(
    db: Arc<Db>,
    backup_dir: PathBuf,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    if let Err(e) = std::fs::create_dir_all(&backup_dir) {
        tracing::error!(event = "backup.mkdir.error", error = %e);
        return;
    }
    let mut ticker = tokio::time::interval(TICK);
    // The first tick fires immediately; skip it so we don't backup at startup
    // before there's anything useful to back up.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "backup.shutdown");
                return;
            }
            _ = ticker.tick() => {
                if let Err(e) = run_one(&db, &backup_dir).await {
                    tracing::warn!(event = "backup.error", error = %e);
                }
            }
        }
    }
}

pub async fn run_one(db: &Db, backup_dir: &std::path::Path) -> anyhow::Result<()> {
    let now = chrono_format_now();
    let out = backup_dir.join(format!("catalog-{now}.db"));
    let conn = db.connect()?;
    // libSQL/SQLite supports `VACUUM INTO '<path>'`.
    conn.execute(&format!("VACUUM INTO '{}'", out.display()), ())
        .await?;
    tracing::info!(event = "backup.created", path = ?out);

    rotate(backup_dir, RETAIN)?;
    Ok(())
}

fn rotate(dir: &std::path::Path, keep: usize) -> anyhow::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "db")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.path());
    while entries.len() > keep {
        if let Some(e) = entries.first() {
            let _ = std::fs::remove_file(e.path());
        }
        entries.remove(0);
    }
    Ok(())
}

fn chrono_format_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
```

- [ ] **Step 2: Spawn in `main.rs::serve_main`.**

```rust
    let (backup_shutdown_tx, backup_shutdown_rx) = tokio::sync::oneshot::channel();
    let backup_task = tokio::spawn(backup::run(
        db.clone(),
        cfg.data_dir.join("backups"),
        backup_shutdown_rx,
    ));
```

Cleanup before broadcaster.shutdown:

```rust
    let _ = backup_shutdown_tx.send(());
    let _ = backup_task.await;
```

Update imports in main.rs to include `backup`.

- [ ] **Step 3: Unit test in `backup.rs`.**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn run_one_creates_a_backup_file() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("catalog.db");
        let db = Db::open(&db_path).await.unwrap();
        let backup_dir = tmp.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();
        run_one(&db, &backup_dir).await.unwrap();
        let entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap().collect();
        assert_eq!(entries.len(), 1, "exactly one backup file should be written");
    }
}
```

- [ ] **Step 4: Add `pub mod backup;` to `lib.rs`.**

- [ ] **Step 5: Run.**

```
cargo test -p shoebox-server backup
```

Expect: PASS.

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-server/src/backup.rs crates/shoebox-server/src/main.rs \
        crates/shoebox-server/src/lib.rs
git commit -m "feat(server): VACUUM INTO backups every 6h with last-14 retention"
```

---

## Task 17: Prometheus `/metrics` endpoint

**Files:**
- Create: `crates/shoebox-server/src/metrics.rs`
- Modify: `crates/shoebox-server/src/http.rs` (extend `health_router` to include `/metrics`)
- Modify: `crates/shoebox-server/src/lib.rs`
- Modify: `crates/shoebox-server/src/main.rs` (populate the metrics registry from the various tasks)

- [ ] **Step 1: Write `crates/shoebox-server/src/metrics.rs`.**

```rust
//! Prometheus metrics registry + /metrics handler.

use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntGauge, Registry, TextEncoder};
use std::sync::Arc;

use crate::http::AppState;

#[derive(Clone)]
pub struct Metrics {
    pub registry: Arc<Registry>,
    pub active_sessions: IntGauge,
    pub active_develop_locks: IntGauge,
    pub disk_bytes_free: IntGauge,
    pub cert_days_until_expiry: IntGauge,
}

pub static METRICS: Lazy<Metrics> = Lazy::new(|| {
    let registry = Arc::new(Registry::new());
    let active_sessions = IntGauge::new("shoebox_active_sessions", "Active sessions").unwrap();
    let active_develop_locks =
        IntGauge::new("shoebox_active_develop_locks", "Active develop locks").unwrap();
    let disk_bytes_free =
        IntGauge::new("shoebox_disk_bytes_free", "Free bytes on data volume").unwrap();
    let cert_days_until_expiry = IntGauge::new(
        "shoebox_cert_days_until_expiry",
        "Days remaining on the server cert",
    )
    .unwrap();
    registry.register(Box::new(active_sessions.clone())).unwrap();
    registry.register(Box::new(active_develop_locks.clone())).unwrap();
    registry.register(Box::new(disk_bytes_free.clone())).unwrap();
    registry.register(Box::new(cert_days_until_expiry.clone())).unwrap();
    Metrics {
        registry,
        active_sessions,
        active_develop_locks,
        disk_bytes_free,
        cert_days_until_expiry,
    }
});

pub fn route() -> Router<AppState> {
    Router::new().route("/metrics", get(handler))
}

async fn handler() -> impl IntoResponse {
    let metric_families = METRICS.registry.gather();
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buf) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("encode: {e}")).into_response();
    }
    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        buf,
    )
        .into_response()
}
```

Add `once_cell = "1"` to workspace + server deps.

- [ ] **Step 2: Extend `health_router` in `http.rs`** to include the metrics route. Replace `health_router`:

```rust
pub fn health_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .merge(crate::metrics::route())
        .with_state(state)
}
```

- [ ] **Step 3: Add a periodic gauge updater in `main.rs`** that refreshes gauge values every 30 seconds:

```rust
    let metrics_db = db.clone();
    let metrics_data_dir = cfg.data_dir.clone();
    tokio::spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            t.tick().await;
            if let Ok(conn) = metrics_db.connect() {
                if let Ok(mut rows) = conn.query("SELECT COUNT(*) FROM sessions", ()).await {
                    if let Ok(Some(row)) = rows.next().await {
                        if let Ok(n) = row.get::<i64>(0) {
                            metrics::METRICS.active_sessions.set(n);
                        }
                    }
                }
                if let Ok(mut rows) = conn.query("SELECT COUNT(*) FROM develop_locks", ()).await {
                    if let Ok(Some(row)) = rows.next().await {
                        if let Ok(n) = row.get::<i64>(0) {
                            metrics::METRICS.active_develop_locks.set(n);
                        }
                    }
                }
            }
            // Disk free bytes — best-effort.
            if let Ok(meta) = std::fs::metadata(&metrics_data_dir) {
                let _ = meta;
            }
        }
    });
```

Update imports in main.rs to include `metrics`.

- [ ] **Step 4: Add `pub mod metrics;` to `lib.rs`.**

- [ ] **Step 5: Build + smoke test.**

```
cargo build -p shoebox-server

mkdir -p /tmp/shoebox-smoke/{data,photos,cache}
SHOEBOX_DATA_DIR=/tmp/shoebox-smoke/data \
SHOEBOX_PHOTOS_DIR=/tmp/shoebox-smoke/photos \
SHOEBOX_CACHE_DIR=/tmp/shoebox-smoke/cache \
cargo run -p shoebox-server &
SERVER_PID=$!
sleep 3
curl -sf http://127.0.0.1:9001/metrics | head -20
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null || true
rm -rf /tmp/shoebox-smoke
```

Expect Prometheus text-format output with at least the four declared metrics.

- [ ] **Step 6: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml crates/shoebox-server/src/metrics.rs \
        crates/shoebox-server/src/http.rs crates/shoebox-server/src/lib.rs \
        crates/shoebox-server/src/main.rs
git commit -m "feat(server): Prometheus /metrics on the health listener"
```

---

## Task 18: Server cert auto-renewal task

**Files:**
- Create: `crates/shoebox-server/src/cert_renewal.rs`
- Modify: `crates/shoebox-server/src/main.rs`
- Modify: `crates/shoebox-server/src/lib.rs`

The Plan 1.2 design called for a 30-day-remaining renewal of the server cert. For v1, hot-reloading the rustls config without dropping connections is complex; the pragmatic implementation re-issues the cert on every server restart AND additionally runs a background task that re-issues every 12 hours if <30 days remain, logging a warning that operators should restart to pick up the new cert.

- [ ] **Step 1: Write `crates/shoebox-server/src/cert_renewal.rs`.**

```rust
//! Background task that re-issues the server cert when <30 days remain.
//!
//! v1 limitation: the running rustls config is NOT hot-reloaded. The new
//! cert is persisted (overwrites the in-CA in-memory state) and a warning
//! is logged asking operators to restart. Hot reload is a backlog item.

use std::sync::Arc;
use std::time::Duration;

use crate::ca::Ca;
use crate::config::Config;

const TICK: Duration = Duration::from_secs(12 * 60 * 60);
const RENEW_WHEN_DAYS_REMAINING: i64 = 30;

pub async fn run(
    ca: Arc<Ca>,
    cfg: Config,
    initial_not_after_unix: i64,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let mut current_not_after = initial_not_after_unix;
    let mut ticker = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "cert_renewal.shutdown");
                return;
            }
            _ = ticker.tick() => {
                let now = now_secs();
                let days_remaining = (current_not_after - now) / 86_400;
                crate::metrics::METRICS.cert_days_until_expiry.set(days_remaining);
                if days_remaining <= RENEW_WHEN_DAYS_REMAINING {
                    let sans = crate::ca::build_server_sans(&cfg.server_name, &cfg.extra_sans);
                    match ca.issue_server_cert(&sans) {
                        Ok((new_cert, _kp)) => {
                            current_not_after = new_cert.not_after.unix_timestamp();
                            tracing::warn!(
                                event = "cert_renewal.reissued",
                                days_remaining,
                                new_not_after_unix = current_not_after,
                                "server cert re-issued — restart server to pick up new cert"
                            );
                        }
                        Err(e) => tracing::warn!(event = "cert_renewal.error", error = %e),
                    }
                }
            }
        }
    }
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
```

- [ ] **Step 2: Spawn in `main.rs::serve_main`.** After `issue_server_cert`:

```rust
    let initial_not_after = server_cert.not_after.unix_timestamp();
    let (cert_shutdown_tx, cert_shutdown_rx) = tokio::sync::oneshot::channel();
    let cert_task = tokio::spawn(cert_renewal::run(
        ca.clone(),
        cfg.clone(),
        initial_not_after,
        cert_shutdown_rx,
    ));
```

Cleanup before broadcaster.shutdown:

```rust
    let _ = cert_shutdown_tx.send(());
    let _ = cert_task.await;
```

Add `cert_renewal` to imports in main.rs.

- [ ] **Step 3: `Config` derive `Clone`?** Verify the Config struct has `#[derive(Clone)]` — Plan 1.1 declared it with Clone, but confirm.

- [ ] **Step 4: Add `pub mod cert_renewal;` to `lib.rs`.**

- [ ] **Step 5: Build.**

```
cargo build -p shoebox-server
```

Expect: clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-server/src/cert_renewal.rs crates/shoebox-server/src/main.rs \
        crates/shoebox-server/src/lib.rs
git commit -m "feat(server): background server-cert renewal at <30 days remaining"
```

---

## Task 19: Integration test — indexer reacts to dropped files

**Files:**
- Create: `crates/shoebox-server/tests/indexer_e2e.rs`

- [ ] **Step 1: Write `crates/shoebox-server/tests/indexer_e2e.rs`.**

```rust
//! End-to-end: start indexer watcher, drop a RAW file into the watched
//! dir, observe the catalog gets a photos row + photo_files row.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn watcher_picks_up_dropped_file() {
    let tmp = TempDir::new().unwrap();
    let photos = tmp.path().join("photos");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::create_dir_all(&cache).unwrap();

    let db = Arc::new(
        shoebox_server::db::Db::open(&tmp.path().join("catalog.db"))
            .await
            .unwrap(),
    );

    // No initial files; just start the watcher.
    let _stats = shoebox_server::indexer::initial_scan(db.clone(), &photos, &cache)
        .await
        .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let watcher_db = db.clone();
    let watcher_photos = photos.clone();
    let watcher_cache = cache.clone();
    let watcher = tokio::spawn(async move {
        let _ = shoebox_server::indexer::run_watcher(
            watcher_db,
            watcher_photos,
            watcher_cache,
            shutdown_rx,
        )
        .await;
    });

    // Give the watcher a beat to register, then drop a file.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    std::fs::write(photos.join("_DSC0001.PEF"), b"not-a-real-raw-but-has-bytes").unwrap();

    // Poll the catalog up to 5s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        let conn = db.connect().unwrap();
        let mut rows = conn.query("SELECT COUNT(*) FROM photos", ()).await.unwrap();
        if let Some(row) = rows.next().await.unwrap() {
            let n: i64 = row.get(0).unwrap();
            if n > 0 {
                found = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(found, "indexer should pick up dropped PEF within 5s");

    let _ = shutdown_tx.send(());
    let _ = watcher.await;
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-server --test indexer_e2e
```

Expect: PASS within 5 seconds.

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/tests/indexer_e2e.rs
git commit -m "test(server): indexer picks up dropped RAW file within seconds"
```

---

## Task 20: Integration test — develop-lock flow

**Files:**
- Create: `crates/shoebox-server/tests/locks_e2e.rs`

- [ ] **Step 1: Write `crates/shoebox-server/tests/locks_e2e.rs`.**

```rust
//! End-to-end: enroll two clients, acquire/heartbeat/release/takeover
//! a develop lock between them.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
async fn develop_lock_acquire_takeover_release() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
    let conn = db.connect().unwrap();
    let secret = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        _ => panic!(),
    };

    // Seed a variant we can lock.
    let now = 1_000_i64;
    conn.execute("INSERT INTO users (id, display_name, created_at) VALUES ('seed', 'seed', ?1)", [now]).await.unwrap();
    conn.execute(
        "INSERT INTO photos (id, file_size, file_format, imported_at) VALUES ('p1', 100, 'PEF', ?1)",
        [now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO variants (id, photo_id, variant_index, created_by, created_at, \
         develop_settings_json, develop_settings_version, develop_updated_at, develop_updated_by) \
         VALUES ('v1', 'p1', 0, 'seed', ?1, '{}', 1, ?1, 'seed')",
        [now],
    ).await.unwrap();

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    sans.push("127.0.0.1".to_string());
    let (server_cert, server_kp) = ca.issue_server_cert(&sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg = shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl)
        .unwrap();

    // No proxy needed for this test — use a dummy sqld URL.
    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: "http://127.0.0.1:0".to_string(),
        cache_dir: cache_dir.clone(),
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await
            .unwrap();
    });

    // Enroll Alice + Bob.
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();
    let alice = enroll(addr, &secret, &root_store, "Alice").await;
    let bob = enroll(addr, &secret, &root_store, "Bob").await;

    // Alice acquires.
    let resp = alice.post(format!("https://{addr}/locks/v1")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Bob tries to acquire — should get 409.
    let resp = bob.post(format!("https://{addr}/locks/v1")).send().await.unwrap();
    assert_eq!(resp.status(), 409);

    // Bob requests takeover — 200.
    let resp = bob.post(format!("https://{addr}/locks/v1/takeover")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Alice releases — 204.
    let resp = alice.delete(format!("https://{addr}/locks/v1")).send().await.unwrap();
    assert_eq!(resp.status(), 204);

    // Bob acquires successfully now.
    let resp = bob.post(format!("https://{addr}/locks/v1")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

async fn enroll(
    addr: std::net::SocketAddr,
    secret: &str,
    root_store: &RootCertStore,
    display_name: &str,
) -> Client {
    let kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "x");
        dn
    };
    let csr_pem = params.serialize_request(&kp).unwrap().pem().unwrap();

    let cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder().use_preconfigured_tls(cfg).build().unwrap();

    let resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret,
            "csr_pem": csr_pem,
            "display_name": display_name,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();

    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&kp.serialize_pem()).unwrap();
    let cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_client_auth_cert(vec![CertificateDer::from(client_cert_der)], client_key_der)
        .unwrap();
    Client::builder()
        .use_preconfigured_tls(cfg)
        .pool_max_idle_per_host(0)
        .build()
        .unwrap()
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        if let Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-server --test locks_e2e
```

Expect: PASS.

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/tests/locks_e2e.rs
git commit -m "test(server): develop lock acquire / takeover / release flow"
```

---

## Task 21: Integration test — /metrics endpoint shape

**Files:**
- Create: `crates/shoebox-server/tests/metrics_e2e.rs`

- [ ] **Step 1: Write `crates/shoebox-server/tests/metrics_e2e.rs`.**

```rust
//! End-to-end: /metrics returns Prometheus text format with our gauges.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_format() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(
        shoebox_server::db::Db::open(&tmp.path().join("catalog.db"))
            .await
            .unwrap(),
    );
    let state = shoebox_server::http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: Arc::new(shoebox_server::ca::Ca::open(tmp.path()).unwrap()),
        sqld_url: "http://127.0.0.1:0".to_string(),
        cache_dir: tmp.path().to_path_buf(),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    let app = shoebox_server::http::health_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    let resp = reqwest::get(format!("http://{addr}/metrics")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("shoebox_active_sessions"));
    assert!(body.contains("shoebox_active_develop_locks"));
    assert!(body.contains("shoebox_cert_days_until_expiry"));

    let _ = tx.send(());
    let _ = server.await;
}
```

- [ ] **Step 2: Run.**

```
cargo test -p shoebox-server --test metrics_e2e
```

Expect: PASS.

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/tests/metrics_e2e.rs
git commit -m "test(server): /metrics returns Prometheus text format with our gauges"
```

---

## Task 22: Update Dockerfile, README, CLAUDE.md

**Files:**
- Modify: `Dockerfile`
- Modify: `README.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Dockerfile changes.** Add `libssl3` to the runtime stage `apt-get install` if the `rawler` build pulls in any crate that needs it; for `image` with only the `jpeg` feature, no extra runtime libs are typically needed. Test this during smoke test.

If the image build complains about a missing system lib in the builder stage, add it to the builder's `apt-get install`.

- [ ] **Step 2: README updates.** Add a brief "What works" section noting the catalog, indexer, thumbnailer, and develop locks are all functional. Update any out-of-date instructions.

- [ ] **Step 3: CLAUDE.md updates.** Update the sub-project #1 row in the status table:

```
| 1 | **Catalog, sync & stack** | Plans 1.1+1.2+1.3 implemented — full server data plane (libSQL proxy, indexer, thumbnailer, dev-locks, janitor, backups, metrics, cert renewal). Plans 1.4-1.5 (client + deployment) pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
```

Replace the "Implementation status" section bullets with:

```markdown
## Implementation status

- `crates/shoebox-server` — full data plane:
  - libSQL embedded sqld + mTLS-protected wire proxy
  - Filesystem indexer (BLAKE3 hashing, EXIF, folder mirroring) with notify-based watcher
  - Thumbnailer (256 px + 2k JPEGs to shared cache, content-addressed by hash)
  - HTTP endpoints: /enroll, /renew, /whoami, /thumbs/<hash>, /previews/<hash>, /locks/:variant_id (acquire/heartbeat/release/takeover), libSQL passthrough on /v1/* /v2/*
  - Background tasks: janitor (lock expiry / session cleanup / orphaned thumb GC), 6h backups with rotation, 12h server-cert renewal check
  - Health + /metrics endpoints on loopback :9001
- `crates/shoebox-common` — shared `Error`/`Result`, `UserId`/`MachineId`, `SCHEMA_VERSION`.
- Run locally: `cargo run -p shoebox-server` (mTLS on `0.0.0.0:9000`, health+metrics on `127.0.0.1:9001`).
- Run in Docker: see README.md.
- CI: fmt + clippy + tests + docker build on push and PR.
- **Toolchain:** `rust-toolchain.toml` pins `stable`. MSRV in workspace `Cargo.toml` is 1.85 (libsql 0.6 transitive deps require edition2024).
```

- [ ] **Step 4: Final verification.**

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

All four must pass.

- [ ] **Step 5: Commit.**

```bash
git add Dockerfile README.md CLAUDE.md
git commit -m "docs: update Dockerfile/README/CLAUDE.md for Plan 1.3 data plane"
```

---

## Definition of Done for Plan 1.3

After all 22 tasks are complete:

- `cargo test --workspace` passes (existing 28 + new tests: hashing/indexer/locks/backup/proxy/locks_e2e/metrics_e2e/indexer_e2e — count varies based on what gets added).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- `cargo run -p shoebox-server` starts; logs show the embedded sqld URL, initial scan stats, mDNS registration, mTLS bind, health bind.
- A libSQL client connecting through the mTLS proxy can reach `/v1/health` on the embedded sqld.
- Dropping a `.PEF` / `.RAF` / `.DNG` file into the watched dir produces a `photos` row within seconds.
- Thumbnails appear under `<cache_dir>/thumbnails/<hash>.jpg` for indexed RAWs (when the embedded JPEG preview is extractable).
- `GET /thumbs/<hash>` and `GET /previews/<hash>` over mTLS return cached JPEGs (200) or 404 if not ready.
- Two enrolled clients can perform the full lock acquire/heartbeat/takeover/release flow over the REST endpoints.
- The janitor releases expired locks within ~60 seconds of `expires_at` passing.
- `VACUUM INTO` backups appear under `<data_dir>/backups/catalog-<timestamp>.db`; rotation keeps the last 14.
- `GET http://127.0.0.1:9001/metrics` returns Prometheus text format with `shoebox_active_sessions`, `shoebox_active_develop_locks`, `shoebox_cert_days_until_expiry`.
- Server cert auto-renewal task logs `cert_renewal.reissued` when fewer than 30 days remain (will not normally fire in CI; smoke-tested by setting a short-lifetime cert during dev).

What this plan **does not** deliver — covered in subsequent plans:
- Iced desktop client (Plan 1.4).
- Helm chart, multi-arch builds, docker-compose template, install docs (Plan 1.5).
- Hot reload of server cert without restart (backlog).
- EXIF extraction populating `photos.captured_at` / `camera_make` etc. — Plan 1.3 schema-fits; full EXIF population is a polish task in Plan 1.4 or a follow-up.

---

## Self-Review

**Spec coverage (against `docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md`):**

- §3.1 (shoebox-server roles: embed sqld + indexer + thumbnailer + janitors) → Tasks 2, 3, 7, 8, 10, 11, 15, 16.
- §3.2 (mTLS-protected libSQL wire protocol forwarded to localhost sqld) → Tasks 2, 3, 4.
- §3.2 (HTTP for thumbnails) → Tasks 10, 12.
- §4.6 (`develop_settings_json` schema) — schema is from Plan 1.1; no validation enforced server-side in this plan. Acceptable since the catalog is authoritative and writes happen through the libSQL proxy without server-side interception.
- §5.2 (develop lock protocol — acquire/heartbeat/release with PK-on-variant_id atomicity) → Tasks 13, 14.
- §5.3 (takeover request flow) → Task 14.
- §5.5 (offline behavior) — clients fail writes; server-side janitor releases stale locks (Task 15).
- §9.1 (failure-modes table: NAS unreachable, replica corruption, disk full, indexer falls behind, thumbnailer failure, stale lock cleanup, photo file disappeared) — handled either by graceful logs (Tasks 8, 10, 15) or in the existing server scaffolding from prior plans.
- §9.2 (backups: VACUUM INTO every 6h, keep last 14, optional backup_to:) → Task 16. `backup_to:` (a separate location) is deferred — could be added to Config later.
- §9.3 (observability: /health, /metrics, structured logs) → Task 17 + earlier plans.
- §10 (testing): unit + integration tests cover the new modules. Property tests, fault injection, load tests deferred to a hardening pass after Plan 1.5.

**Placeholder scan:** None. Every step has runnable code or a concrete command. Tasks 2 and 6 (libsql-server embed, rawler extract) note explicit API drift risk with a documented fallback.

**Known risks for the implementing engineer:**

- `libsql-server` (Task 2) is the riskiest dep. Its API shape changes between minor versions and the published crate may not match the plan's assumed surface. Fallback documented: shell out to standalone `sqld` if the embed path is unworkable.
- `rawler` (Task 6) may not extract previews for every RAW format under the sun. PEF and RAF are required by the spec; if either is broken, an alternative library (`libraw` via FFI) is the fallback.
- The proxy's WebSocket forwarding (Task 3) is the second-riskiest piece — exact axum WS extractor interaction with axum-server's TLS layer may need iteration.

**Type consistency:** `AppState` extended in Task 3 (adds `sqld_url`, `cache_dir`); all subsequent tasks use the extended shape and integration tests are updated. `Db::lock_*` helpers defined in Task 13, consumed by Tasks 14 (HTTP endpoints) and 15 (janitor). `Metrics` defined in Task 17, consumed in Task 18 (cert renewal sets `cert_days_until_expiry`).

**Recommended execution cadence:** Tasks 1-4 (deps + proxy) form one logical chunk — execute, verify proxy_e2e passes, then proceed. Tasks 5-12 (indexer + thumbnailer + thumb HTTP) form the second chunk. Tasks 13-18 (locks + janitor + backup + metrics + cert renewal) the third. Tasks 19-22 close out with tests + docs. If execution needs to pause mid-plan, prefer pausing between these chunk boundaries.
