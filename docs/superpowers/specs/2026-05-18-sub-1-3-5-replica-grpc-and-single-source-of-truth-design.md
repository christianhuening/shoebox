# Sub-project 1.3.5 — Replica gRPC routing & single-source-of-truth catalog

**Author:** generated 2026-05-18 from an interactive brainstorming session.
**Sub-project:** 1.3.5 — a follow-up to sub-projects 1.3 (server data plane) and 1.4
(desktop client foundation). Closes two related gaps surfaced by running the
end-to-end wizard for the first time after Plan 1.5 was merged.
**Status:** spec, ready for plan + implementation.

## 1. Goal

Make the libSQL embedded-replica round-trip actually work end-to-end. After
this change, the first-run wizard reaches the Library screen, server-side
writes (enrollment, revocation, locks) are visible on the client's replica,
and the integration test that exercises this path actually runs in CI.

## 2. Background

While verifying the deployment we discovered two coupled defects that, taken
together, mean the catalog has never round-tripped between server and client:

### 2.1 The gRPC replication channel is unreachable

`crates/shoebox-server/src/sqld_embed.rs` spawns `sqld` with only
`--http-listen-addr`. `crates/shoebox-server/src/proxy.rs` forwards `/v1/*`
and `/v2/*` over an HTTP/1.1 hyper client to that port. But
`libsql 0.6.0` + `libsql_replication 0.6.0`'s
`Builder::new_remote_replica` opens its replication channel over **gRPC on
HTTP/2** (tonic 0.11), which sqld serves on a *separate* port behind
`--grpc-listen-addr` (see sqld v0.24.32 source — `make_user_api_config` and
`make_rpc_server_config` are both wired against the same db instance).

Concrete symptom observed live:

```
libsql::replication::remote_client: Attempting to perform handshake with primary.
Failed handshake: Status { code: Unimplemented,
  message: "grpc-status header missing, mapped from HTTP status code 404" }
```

The client retries this every second forever; the wizard stalls on the
"Enrolling..." → "Loading library..." transition.

### 2.2 The server writes to a different database than the one sqld serves

`crates/shoebox-server/src/db.rs` opens
`<data_dir>/catalog.db` via `libsql::Builder::new_local(path)`. `sqld_embed`
points sqld at `<data_dir>/sqld/` (a directory, sqld's own data store).
**These are two unrelated SQLite databases.** Even with §2.1 fixed, the
client's replica would sync sqld's database — which contains *none* of the
server-side writes (enrollment, sessions, revoked certs, locks, indexer
output, thumbnails).

Verified on disk on 2026-05-18:

```
/var/lib/shoebox/catalog.db          ← shoebox-server's writes go here
/var/lib/shoebox/sqld/dbs/...        ← what clients sync from
```

CLAUDE.md historically described this as the "two writers to catalog.db"
risk; that wording is inaccurate. The two processes write to **different**
databases, which is the more dangerous failure mode — silent data
divergence instead of WAL corruption.

### 2.3 Why CI never caught either

`crates/shoebox-client/tests/replica_e2e.rs`,
`crates/shoebox-client/tests/library_view_e2e.rs`,
`crates/shoebox-client/tests/library_lock_e2e.rs`,
`crates/shoebox-server/tests/proxy_e2e.rs`, and
`crates/shoebox-server/tests/locks_e2e.rs` all gate on `which::which("sqld").is_ok()`.
The CI `test` job in `.github/workflows/ci.yml` never installs sqld, so
every one of these tests silently skips. The `binary-smoke` job only checks
`/health` returns 200. Nothing in CI ever actually executes a replica
handshake against a real sqld.

## 3. Design

### 3.1 sqld spawned with both listeners

`sqld_embed.rs` adds a second ephemeral loopback port and an additional
arg, so sqld serves the same `--db-path` over both protocols:

```
sqld --http-listen-addr 127.0.0.1:<H>
     --grpc-listen-addr 127.0.0.1:<G>
     --db-path /var/lib/shoebox/sqld/
```

`EmbeddedSqld` gains a `local_grpc_url: String` field; the existing
`local_url` is unchanged. Source confirmation: the sqld v0.24.32 server
holds one `Server` struct that wires both `make_user_api_config()` (HTTP)
and `make_rpc_server_config()` (gRPC) against the same database instance,
so the two listeners share state.

### 3.2 ALPN advertises h2 + http/1.1

`mtls.rs` sets `config.alpn_protocols = vec![b"h2".to_vec(),
b"http/1.1".to_vec()]` on the rustls `ServerConfig`. h2 is listed first so
gRPC clients prefer it; Hrana/REST clients negotiate http/1.1.

### 3.3 Proxy branches by Content-Type and rewrites the path

`proxy.rs` keeps its `/v1/*path` and `/v2/*path` axum routes. Inside the
`forward_http` handler:

1. **Detect gRPC**: `Content-Type: application/grpc*` ⇒ gRPC path.
2. **Pick upstream**:
   - gRPC → a process-wide `HyperClient` built with
     `.http2_only(true).build_http()` (h2 prior-knowledge, h2c against
     loopback sqld), forwarding to `state.sqld_grpc_url`.
   - Hrana/REST → the existing HTTP/1.1 client, forwarding to
     `state.sqld_url` (unchanged).
3. **Strip the `/v1` or `/v2` prefix** when building the upstream URI for
   gRPC. tonic uses `ReplicationLogClient::with_origin(uri)` which
   preserves the path of the sync URL the client gave; libsql 0.6 clients
   pass `https://<server>/v1` as `sync_url`, so gRPC requests land here as
   `/v1/wal_log.ReplicationLog/Hello`. sqld's gRPC service registers the
   methods at `/wal_log.ReplicationLog/Hello`. Without the strip, sqld
   404s every gRPC request. (Hrana traffic keeps its `/v1`/`/v2` prefix.)
4. **Preserve `TE: trailers`** on gRPC requests. The current hop-by-hop
   stripping unconditionally removes `TE`, which signals to sqld that the
   peer accepts trailers. Drop `TE` only on non-gRPC requests.

The response is forwarded back with `upstream_response.into_response()`.
hyper 1.9 `Incoming` delivers HTTP/2 trailers as `Frame::trailers(...)`
through `poll_frame`; axum's `Body::new` wraps the underlying body and
re-emits frames including trailers; axum_server's h2 writer serializes
trailers as HEADERS frames at the end of the stream.

This was empirically validated by a spike on 2026-05-18: the libsql
replication client successfully completed its `Hello` handshake against a
modified sqld, indicating the trailer-carried `grpc-status` reached it
through the proxy.

### 3.4 Server uses sqld as the single source of truth

`db.rs` changes one constructor:

| before                                 | after                                                  |
|----------------------------------------|--------------------------------------------------------|
| `Builder::new_local(path).build()`     | `Builder::new_remote(sqld_http_url.into(), "".into()).build()` |
| `Db::open(path: &Path) -> Db`          | `Db::open(sqld_http_url: &str) -> Db`                  |

Every other use of `Db` (`Db::connect()`, all the convenience methods,
`indexer`, `janitor`, `backup`, all HTTP handlers) stays identical because
the `libsql::Connection` API is the same for local and remote backends.

In `main.rs`, the startup sequence reorders to:

```
1. Spawn sqld (gives us local_url + local_grpc_url)
2. Db::open(&embedded_sqld.local_url)   ← was first; now after sqld
3. Run migrations through Db's libsql connection
4. CA bootstrap, secret bootstrap, cert renewal, indexer, etc.
```

### 3.5 No client changes

`crates/shoebox-client/src/replica.rs`'s `build_sync_url(server_url)`
keeps returning `"<server_url>/v1"`. tonic preserves the path; the proxy
strips it. The client's reqwest mTLS client (used for /enroll, /thumbs,
/locks) continues to talk HTTP/1.1 and is unaffected by ALPN.

### 3.6 CI installs sqld

The `test` job in `.github/workflows/ci.yml` gains one step (after Rust
toolchain setup, before `cargo test`):

```yaml
- name: Install sqld v0.24.32
  run: |
    cd /tmp
    wget -q https://github.com/tursodatabase/libsql/releases/download/libsql-server-v0.24.32/libsql-server-x86_64-unknown-linux-gnu.tar.xz
    echo "71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f  libsql-server-x86_64-unknown-linux-gnu.tar.xz" | sha256sum -c -
    tar -xJf libsql-server-x86_64-unknown-linux-gnu.tar.xz
    sudo install -m 755 libsql-server-x86_64-unknown-linux-gnu/sqld /usr/local/bin/sqld
    sqld --version
```

Version + sha256 mirror `Dockerfile` lines 38–55. With sqld on PATH, all
five `which::which("sqld")`-gated tests (replica_e2e, library_view_e2e,
library_lock_e2e, proxy_e2e, locks_e2e) execute.

### 3.7 New regression test: server writes visible on client replica

Add `crates/shoebox-client/tests/server_write_visible_to_client_replica.rs`.
This test is the canonical "would this regression have been caught"
backstop: the server-side `Db` (now writing through sqld) inserts a row;
the client's replica `.sync()`s; the row is visible on the replica. This
test must exist or §2.2 regresses silently.

### 3.8 Upgrade path

On startup, if `<data_dir>/catalog.db` exists and is non-empty:
1. Rename to `<data_dir>/catalog.db.legacy-pre-grpc-fix-<unix_ts>`.
2. Log a `WARN` event `catalog.legacy.renamed`.
3. Continue startup. sqld's database is now the sole source of truth.

This codebase has no production deployments; any existing `catalog.db`
contains only an internal CA private key, an enrollment secret hash, and a
small amount of server-side bookkeeping — all of which will regenerate
cleanly. The rename (rather than delete) preserves a manual recovery
option for the rare case where someone has actually enrolled real clients
against a dev install.

## 4. Out of scope

These are real follow-ups, not part of this spec:

- The `deploy/compose/.env.example` `SHOEBOX_PHOTOS_DIR` overload bug —
  the env var is read as both a host bind-mount path (`${VAR}:/photos`)
  and a container env var, so the in-container photos path defaults to
  the wrong directory and the indexer silently skips. Separate small spec.
- OS-specific dev-setup scripts (the gap the user flagged when starting
  this session). Separate spec.
- `crates/shoebox-server/src/ca.rs` is Linux-only (`use std::os::unix::fs`);
  `cargo check` on Windows fails on this file alone. Doesn't affect
  Docker builds but blocks native Windows dev. Separate small spec.
- Switching the proxy to a tonic-server (vs. forwarding to sqld's gRPC).
  Current "thin proxy" is closer to the spec's stated intent and matches
  what the existing /v1/* /v2/* proxy already does.

## 5. Test strategy

| Layer | Test | Catches |
|---|---|---|
| Unit | `proxy::strip_v1_or_v2_prefix` tests | Path-rewrite logic for gRPC |
| Unit | `proxy::build_upstream_url` tests for `strip_proxy_prefix=true/false` | Same |
| Integration (existing) | `replica_e2e.rs` | gRPC handshake + replica sync (now actually runs in CI) |
| Integration (existing) | `library_view_e2e.rs`, `library_lock_e2e.rs` | Reads + locks via replica (now runs in CI) |
| Integration (existing) | `proxy_e2e.rs` | Hrana HTTP path unaffected by ALPN/h2 changes |
| Integration (new) | `server_write_visible_to_client_replica.rs` | The cross-database divergence in §2.2 |
| Smoke (CI) | `binary-smoke` job | Still only `/health`; that's fine — the integration tests now carry the load |

## 6. Risks (post-spike)

| Risk | Pre-spike | Post-spike | Why |
|---|---|---|---|
| hyper-util preserves HTTP/2 trailers through `into_response()` | MEDIUM-HIGH | LOW | Spike client completed `Hello` handshake; `grpc-status` trailer reached the client |
| sqld serves both ports against the same db | MEDIUM | LOW | Doc'd in sqld user guide; spike showed gRPC writes/reads succeed |
| tonic 0.11 ↔ sqld 0.24.32 wire compat | MEDIUM | LOW | Both ship from the same monorepo at compatible tags |
| `TE: trailers` stripping breaks gRPC | (latent) | MITIGATED | Fix lands in the same change; conditional strip on non-gRPC only |
| `/v1` path prefix breaks gRPC method routing | (latent) | MITIGATED | Proxy strips prefix when forwarding to sqld grpc port |

## 7. Files touched (high-level)

- `crates/shoebox-server/src/sqld_embed.rs` — second listener arg, new field
- `crates/shoebox-server/src/mtls.rs` — ALPN config
- `crates/shoebox-server/src/http.rs` — new `AppState.sqld_grpc_url` field
- `crates/shoebox-server/src/main.rs` — wire through, reorder startup
- `crates/shoebox-server/src/proxy.rs` — content-type branching, gRPC client,
  prefix stripping, TE preservation
- `crates/shoebox-server/src/db.rs` — `Builder::new_local` → `Builder::new_remote`
- ~14 test files that instantiate `AppState{...}` — add the new field
- `.github/workflows/ci.yml` — install sqld step in `test` job
- `crates/shoebox-client/tests/server_write_visible_to_client_replica.rs` — new
- `CLAUDE.md` — update sub-project #1 status, drop the "two writers" risk
  note, replace with this fix
- `docs/superpowers/specs/2026-05-18-sub-1-3-5-replica-grpc-and-single-source-of-truth-design.md` — this file

## 8. Architecture sketch

```
              ┌────────────────────────────────────────────────────────┐
              │            shoebox-server (one process)                │
              │                                                        │
              │   ┌──────────────────────┐                             │
   mTLS :9000 │   │ axum + axum_server   │                             │
  (h2 + h1.1) │   │ + PeerCertAcceptor   │                             │
  ─────────►  │   │                      │                             │
              │   │  /v1/*  /v2/* ───┐   │                             │
              │   │  (forward_http)  │   │                             │
              │   └──────────────────┼───┘                             │
              │                      │                                 │
              │      Content-Type: application/grpc?                   │
              │                      │                                 │
              │            ┌─────────┴─────────┐                       │
              │            │                   │                       │
              │            ▼ NO                ▼ YES                   │
              │   ┌─────────────────┐   ┌──────────────────┐           │
              │   │ HTTP/1.1 client │   │ HTTP/2 client    │           │
              │   │ (Hrana)         │   │ (gRPC, h2c)      │           │
              │   │ keep /v1 prefix │   │ strip /v1 prefix │           │
              │   └────────┬────────┘   └────────┬─────────┘           │
              │            │                     │                     │
              │            ▼                     ▼                     │
              │     127.0.0.1:H              127.0.0.1:G               │
              │     (sqld --http-listen-addr)(sqld --grpc-listen-addr) │
              │            │                     │                     │
              │            └────────┬────────────┘                     │
              │                     ▼                                  │
              │              sqld --db-path                            │
              │              <data_dir>/sqld/                          │
              │              (single source of truth)                  │
              │                     ▲                                  │
              │                     │  libsql::new_remote (Hrana HTTP) │
              │                     │                                  │
              │              ┌──────┴──────┐                           │
              │              │ Db (server) │ — migrations, sessions,   │
              │              │             │   revoked_certs, locks    │
              │              └─────────────┘                           │
              └────────────────────────────────────────────────────────┘
```
