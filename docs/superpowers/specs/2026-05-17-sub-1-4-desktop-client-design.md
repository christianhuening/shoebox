# shoebox-client Foundation (Plan 1.4) — Design Spec

**Status:** approved 2026-05-17, ready for plan-writing.
**Parent spec:** [`2026-05-17-catalog-sync-and-stack-design.md`](2026-05-17-catalog-sync-and-stack-design.md) (sub-project #1).
**Predecessors:** Plans 1.1–1.3 (server foundation, mTLS+enrollment, data plane) — all merged on `main`.

## 1. Overview

Plan 1.4 delivers the **foundation** for the shoebox desktop client: enough Iced app to enroll against a `shoebox-server`, open a libSQL embedded replica through the mTLS proxy, and demonstrate a working end-to-end catalog round-trip on Linux, macOS, and Windows.

It does **not** ship the polished library experience (grid, EXIF panel, rating / keywords / virtual copies, develop-lock banners). Those land in a follow-up Plan 1.4b. Plan 1.4 ends with the user landing in a debug "Library home" view that shows connection status, schema version, photo count, folder count, and the currently active user — enough to prove the data plane works.

**Definition of done for Plan 1.4:**

- On all three platforms (Linux dev box, Windows 11 host, MacBook Pro), a fresh install completes the §7.6 first-run wizard end-to-end against a running `shoebox-server`.
- Client cert + key persisted in the OS-native keychain.
- libSQL embedded replica catches up on first sync and again on a 30-second background ticker thereafter.
- Background cert-renewal task re-issues the client cert when <30 days remain.
- Server kill → offline banner → server restart → banner clears within one catchup tick.
- Server-side revocation of a client cert → next sync attempt boots the client back to the Discovery screen.

## 2. Locked-in technology decisions

These are inherited from the parent spec and CLAUDE.md; restated here so the plan author doesn't have to dig:

- **Language:** Rust.
- **UI:** `iced` (pure-Rust, wgpu-rendered). No webview.
- **Local catalog:** `libsql` 0.6 (workspace dep) in **embedded-replica mode** pointing at `https://<server>:9000/v1/...` through the mTLS proxy.
- **Auth:** mTLS with the cert issued by `shoebox-server`'s `/enroll`. Server CA is fetched once via a new unauthenticated `GET /ca-cert` endpoint (see §3) before any TLS-validated request.
- **Cert + key storage:** OS keychain via the `keyring` crate (Keychain on macOS, Credential Manager on Windows, Secret Service / kwallet on Linux). Explicit-consent fallback to mode-0600 PEM files in app-data if keychain is unavailable.
- **Discovery:** mDNS via `mdns-sd` (already a workspace dep) browsing `_shoebox._tcp.local`. Manual server-URL entry is always available.
- **Config:** `client.toml` under `directories::ProjectDirs::config_dir()`.
- **Platforms:** Linux, Windows, macOS, all in Plan 1.4. Cross-OS code signing / distribution is deferred to Plan 1.5.

## 3. Architecture

A single new crate `crates/shoebox-client` produces one binary (`shoebox-client`) and a thin lib for integration tests.

The binary is one Iced `Application` with one top-level state machine — a `Screen` enum (`Discovery`, `EnterSecret`, `EnrollProgress`, `ProfilePicker`, `Library`) drives transitions. Shared resources (replica handle, mTLS HTTP client, mDNS browser, cert / config) live in an `AppState` struct wrapped in `Arc<RwLock<…>>` for background-task access. Iced subscriptions drive the periodic tickers (replica catchup, cert renewal, mDNS rebrowse).

**Crate layout:**

```
crates/shoebox-client/
├── Cargo.toml
├── src/
│   ├── main.rs              ← Iced Application impl; routes Screen → view()/update()
│   ├── lib.rs               ← re-exports for integration tests
│   ├── app_state.rs         ← shared state (cert, mTLS client, replica, mDNS, config)
│   ├── screens/
│   │   ├── mod.rs           ← Screen + Message enums + transitions
│   │   ├── discovery.rs     ← mDNS list + "Add manually" form
│   │   ├── enter_secret.rs  ← shared-secret prompt
│   │   ├── enroll_progress.rs
│   │   ├── profile_picker.rs
│   │   └── library.rs       ← debug catalog-state view
│   ├── discovery.rs         ← mDNS browser wrapper
│   ├── enrollment.rs        ← CSR gen, /enroll call
│   ├── replica.rs           ← libSQL embedded replica open + sync
│   ├── mtls_http.rs         ← reqwest builder w/ client cert
│   ├── cert_store.rs        ← keyring wrapper + file fallback
│   ├── config.rs            ← client.toml read/write
│   └── cert_renewal.rs      ← background renewal task
└── tests/
    ├── first_run_e2e.rs     ← spawn server in-proc, drive wizard programmatically
    ├── replica_e2e.rs       ← seeded catalog round-trip
    └── cert_renewal_e2e.rs  ← short-lifetime cert → renewal fires
```

**One server-side addition required:** add an unauthenticated `GET /ca-cert` endpoint to `shoebox-server` that returns the CA cert PEM. Needed so the client can validate the TLS chain on its very first request (the existing `/enroll` is unauthenticated but the *server cert* it presents is signed by the CA, so the client has a chicken-and-egg problem without `/ca-cert`). Scope: ~10 lines on the server, mirroring the existing `enroll` route shape.

## 4. Component responsibilities

Each module has one job and a narrow interface. The screens stay dumb (UI only); business logic lives in the non-UI modules so it's `cargo test`-able without Iced.

- **`cert_store`** — `store(server_url, cert_pem, key_pem) -> Result`, `load(server_url) -> Result<Option<(cert, key)>>`, `delete(server_url) -> Result`. Wraps `keyring` keyed by server URL (so one client paired with multiple servers keeps separate entries). Falls back to mode-0600 PEM files under the OS app-data dir **only on explicit user consent** (see §6).
- **`mtls_http`** — `build_client(root_cert_pem, client_cert_pem, client_key_pem) -> Result<reqwest::Client>`. Pure builder, no caching.
- **`discovery`** — `Browser { rx: mpsc::Receiver<DiscoveredServer> }` and `start() -> Browser`. Spawns an `mdns-sd` query for `_shoebox._tcp.local`; an Iced subscription drains the receiver into `Message::ServerDiscovered`. `Browser::add_manual(url)` injects a manually-entered server.
- **`enrollment`** — `enroll(server_url, root_cert_pem, shared_secret, display_name) -> Result<EnrollResult>`. Generates an Ed25519 keypair + CSR via `rcgen`, POSTs `/enroll` over plain HTTPS pinning the server's CA, returns the parsed `EnrollResponse`. Does not touch the keychain or filesystem.
- **`replica`** — `open(data_dir, server_url, mtls_client) -> Result<Replica>` where `Replica` wraps `libsql::Database` in embedded-replica mode. `sync(&self) -> Result<u64>` runs WAL catchup. `conn(&self) -> Result<Connection>` hands out a connection for screens to query.
- **`config`** — TOML at `directories::ProjectDirs::config_dir()/client.toml`. Schema: `server_url: String`, `cert_serial_hex: String`, `last_active_user_id: Option<String>`. Default-empty on missing file (signals first-run).
- **`cert_renewal`** — `run(state: Arc<AppState>, shutdown: oneshot::Receiver<()>)`. 12-hour ticker; calls `/renew` when <30 days remain; writes the new cert to `cert_store`; updates `client.toml`'s `cert_serial_hex`. Warn-logs on failure and retries on the next tick. Mirrors `shoebox-server`'s `cert_renewal.rs` shape.
- **`screens/`** — each screen is `fn view(state: &AppState) -> Element<Message>` plus `fn handle(state: &mut AppState, msg: Message) -> Task<Message>`. No DB or HTTP calls inside screens; they emit messages that other modules handle.

## 5. Data flow

**First-run** (no `client.toml`):

1. `main.rs` constructs `AppState::default()`, sets `Screen::Discovery`.
2. mDNS subscription starts browsing `_shoebox._tcp.local`.
3. Discovered servers stream into the list; user picks one (or uses manual entry).
4. Before any TLS-validated request: client hits `GET https://<server>:9000/ca-cert` with `dangerous_accept_invalid_certs(true)` to retrieve the CA PEM. Pin this CA for all subsequent requests in this session.
5. `Screen::EnterSecret`: user pastes shared secret + display name.
6. `enrollment::enroll(...)` runs (via `Task::perform`): generates keypair + CSR, POSTs `/enroll` over plain HTTPS validating against the pinned CA, returns `EnrollResponse`.
7. `cert_store::store(server_url, cert_pem, key_pem)` → keychain; `config::write({ server_url, cert_serial_hex, last_active_user_id: None })`.
8. `mtls_http::build_client(ca_cert, cert, key)` → `AppState.client`.
9. `replica::open(data_dir, server_url, client)` → `AppState.replica`; `replica::sync()` runs the initial snapshot transfer.
10. `Screen::ProfilePicker` queries `SELECT id, display_name FROM users` from the replica. Shows the list + "Create new". "Create new" inserts a row via the proxy.
11. `config::write({ ..., last_active_user_id: Some(picked) })`.
12. `Screen::Library`: read folder count, photo count, schema version from the replica; display + log "ready".

**Steady-state** (subsequent launches):

1. `main.rs` reads `client.toml`.
2. `cert_store::load(server_url)` → `(cert_pem, key_pem)`.
3. `mtls_http::build_client(...)` → `AppState.client`.
4. `replica::open(...)` → `AppState.replica`; `replica::sync()` runs incremental WAL catchup.
5. `last_active_user_id` loaded; go straight to `Screen::Library`.
6. Background tasks spawn: replica catchup ticker (30 s), cert renewal check (12 h). mDNS rebrowse is *not* spawned (no longer needed once paired).

**Offline:** if `replica::sync()` fails (network or server down), `AppState.connection_status = Offline`; Library banner shows offline; reads continue from the local replica file; writes are disabled in the UI for v1 (write queueing is parent-spec backlog §11.3.3). Background catchup ticker keeps retrying; banner clears when sync succeeds again.

**Cert revoked between sessions:** an early `GET /whoami` ping right after `build_client` catches it. On TLS error or 401, wipe keychain entry + `client.toml` → drop back to `Screen::Discovery` as if first-run.

## 6. Error handling

Each failure mode has one well-defined recovery. Nothing silent.

| Where | Failure | Response |
|---|---|---|
| `discovery` | No mDNS hits in 5 s | Banner: "No servers found." + **Retry discovery** button (restarts mDNS browse) + manual-entry form stays visible. |
| `discovery` | mDNS subscription itself errors | Inline error: "Discovery unavailable: \<reason\>." + **Retry** button + manual-entry form. Don't crash. |
| `enrollment` | `/ca-cert` returns non-2xx or network error | Inline error on EnterSecret; **Retry** button. |
| `enrollment` | `/enroll` returns 401 (bad secret) | Inline error: "Invalid secret. Check with your admin." Field stays populated. |
| `enrollment` | `/enroll` returns 5xx or network drops | Inline error + **Retry**. No state mutation until 2xx. |
| `cert_store::store` | keychain write fails (locked Keychain, dismissed Secret Service prompt, etc.) | Inline error on EnrollProgress with **Retry** (re-attempts keychain write) and **Use file storage instead** (explicit consent → falls back to mode-0600 file in app-data + warning banner persisting in Library until user re-enrolls). |
| `replica::open` | libsql replica path corrupt / schema mismatch | Wipe local replica file + retry once. Still failing → "Local catalog corrupt; re-enroll" + button to drop back to Discovery (also wipes keychain + client.toml). |
| `replica::sync` | network failure | Set `connection_status = Offline`; banner; reads keep working; background catchup ticker keeps trying. |
| `replica::sync` | server auth error (cert revoked) | Wipe keychain + `client.toml` → `Screen::Discovery`. |
| `cert_renewal` | `/renew` returns 5xx or network failure | Warn-log + retry on next 12 h tick. |
| `cert_renewal` | new cert rejected on next request | Same as "cert revoked" path above. |
| `profile_picker` | Replica empty (fresh server, no users yet) | "Create new" is the only option; no list. |
| Any panic | unwind → log → exit nonzero | Standard Rust panic handler. |

**Three patterns** worth calling out:

1. **Error display lives in screens, never in modules.** Modules return typed `Result<_, ClientError>`; screens decide whether to show inline text, a banner, or transition.
2. **State is never half-mutated.** `cert_store::store` + `config::write` happen as a tuple after `/enroll` succeeds; if `config::write` fails after `cert_store::store` succeeds, the keychain entry is deleted. Either both land or neither does.
3. **"Drop to first-run" is the universal recovery.** When local state is unrecoverable (corrupt replica, revoked cert, missing CA), wiping keychain + `client.toml` + replica file + showing `Screen::Discovery` is always safe.

## 7. Testing strategy

**Unit tests** (`#[cfg(test)] mod tests`):

- `cert_store`: round-trip store → load → delete via keyring; file-fallback path with tempdir + mode-bit assertion on Unix; delete-after-load-failure cleanup.
- `mtls_http`: builder accepts valid PEMs; rejects malformed cert / key; the returned client actually presents the cert (hit a tiny in-test axum mTLS endpoint and assert peer-cert OU).
- `enrollment`: CSR generated by the function parses cleanly via rcgen; on stubbed 2xx, fields extract correctly; on stubbed 401, returns typed `BadSecret`.
- `config`: TOML round-trips; missing file returns `Config::default()`; partial file (e.g., missing `last_active_user_id`) doesn't panic.
- `discovery`: `add_manual` injects a `DiscoveredServer` into the channel. (mDNS itself is hard to unit-test; defer to e2e.)
- `replica`: open against an empty libsql file + sync against a stub; thin sanity test for path handling.
- `cert_renewal`: "should renew" predicate returns true for `not_after < 30 days`, false otherwise.

**Integration tests** (`crates/shoebox-client/tests/*.rs`, gated on `sqld` being on PATH — same skip pattern as `proxy_e2e.rs`):

- `first_run_e2e.rs` — spawn `shoebox-server` in-process, pre-generate the shared secret, drive the wizard programmatically by injecting `Message`s. Assert each Screen transition; assert `AppState.replica` is connected at end; assert the new `users` row is queryable; assert `schema_version` matches.
- `replica_e2e.rs` — server seeded with `users`/`photos`; client opens replica through the proxy; assert reads return the seeded data; insert a row from the client; assert it round-trips on re-read.
- `cert_renewal_e2e.rs` — exercise the renewal trigger by passing the cert renewal task a manually-constructed `not_after_unix` that is < 30 days from now, rather than mocking time. The test then runs one tick, asserts `/renew` was called (server-side log or counter), and asserts the keychain holds a new cert serial. No production-code knobs added to `shoebox-server`'s `Ca`.

**Manual / cross-platform smoke** (out of CI):

- Build + full first-run wizard runs end-to-end on Linux, Windows 11, and macOS against the same `shoebox-server` instance.
- Keychain entries visible in each OS's native UI (Keychain Access / `cmdkey /list` / `secret-tool`).
- Kill the server mid-run → offline banner → restart → banner clears within 30 s.
- Revoke a client cert server-side via `shoebox-server revoke <serial>` → client gets booted to Discovery on next sync attempt.

**Not tested in Plan 1.4** (correctly out of scope):

- Iced UI rendering — no snapshot tests. The screens are thin and rendering quality lives in Plan 1.4b's library view.
- Multi-user concurrent edits — that's Plan 1.4b.
- Cross-OS code signing — Plan 1.5.

## 8. Scope boundary

### 8.1 In scope (Plan 1.4)

- New crate `crates/shoebox-client` with the module layout in §3.
- All seven steps of parent-spec §7.6 first-run wizard, including the mDNS picker and profile picker.
- OS-keychain cert storage via `keyring`, with explicit-consent file fallback.
- libSQL embedded-replica wiring through the mTLS proxy.
- 30-second background replica catchup ticker.
- 12-hour background cert renewal task (client side).
- Linux + macOS + Windows build targets. The same source tree compiles on all three. Platform-specific code lives behind `#[cfg(target_os = ...)]` or behind `keyring`'s abstraction.
- One new server endpoint: `GET /ca-cert` on `shoebox-server` (unauthenticated, returns CA PEM).
- Debug "Library home" screen showing connection status, schema version, photo count, folder count, active user.

### 8.2 Out of scope (each gets its own follow-up plan or stays backlog)

| Item | Where it lands |
|---|---|
| Demo library view: folder tree + photo grid + EXIF panel + rate/keyword/virtual-copy actions (parent-spec §11.1's "shoebox-client" leftover) | Plan 1.4b (still part of sub-project #1) |
| Polished library experience: smooth-scroll 100k-thumbnail grid, filmstrip, faceted search, multi-select | Sub-project #3 (own spec + plan cycle) |
| EXIF panel | Plan 1.4b |
| Rate / keyword / virtual-copy actions | Plan 1.4b |
| Develop-lock acquire/heartbeat/release/takeover UI banners | Plan 1.4b |
| Local thumbnail cache (LRU) backed by HTTP fetches | Plan 1.4b (needed when the grid lands) |
| Cross-OS code signing, notarization, MSIX/DMG packaging, auto-update | Plan 1.5 (deployment) |
| Write queueing while offline | Parent-spec backlog §11.3.3 |
| Per-user password auth | Parent-spec backlog §11.3.4 |

## 9. Known limitations carried forward

- **Iced rendering tests are absent** — Iced's snapshot story is weak and the foundation screens are thin. Plan 1.4b's library view will need a richer testing approach (likely manual + perf benchmarks rather than golden images).
- **Cross-platform manual smoke** rather than CI. Setting up macOS / Windows CI runners is its own ticket (parent-spec backlog).
- **The `Arc<RwLock<AppState>>` choice** is uniform but coarse-grained. If lock contention shows up under realistic catchup load, a follow-up could split into per-resource locks. Unlikely to matter for the foundation's single-screen workload.
- **`replica::sync` is poll-based** (30 s ticker) rather than push-driven. libsql 0.6 doesn't expose change-stream subscriptions natively. Real-time UI updates (parent-spec §5.4) need either polling with shorter intervals or a server-side WebSocket fanout — to be decided in Plan 1.4b when the grid needs it.

## 10. Open questions

None at the time of writing. The two that came up during brainstorming —
mDNS-vs-`/ca-cert` for root cert bootstrap, and what to put in the "Library
home" debug view — were decided in favor of `/ca-cert` and the
connection-status / schema / counts / user display respectively.
