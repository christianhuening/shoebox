# shoebox

A cross-platform desktop application for managing, developing, and exporting
RAW digital photos. Lightroom-shaped, but with a defining capability: a single
shared catalog accessed concurrently from multiple machines, suitable for
families or small studios working off a NAS-hosted library.

Initial supported RAW formats: Pentax (PEF/DNG) and Fuji (RAF), extensible.
Initial supported platforms: macOS, Windows. Linux likely follows for free.

## Project shape

shoebox is a collection of interdependent subsystems, each developed via its
own spec → plan → implementation cycle. Sub-project status:

| # | Sub-project | Status | Spec |
|---|---|---|---|
| 1 | **Catalog, sync & stack** | Plans 1.1+1.2+1.3 implemented — full server data plane (libSQL proxy, indexer, thumbnailer, dev-locks, janitor, backups, metrics, cert renewal). Plans 1.4-1.5 (client + deployment) pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
| 2 | RAW pipeline (PEF/RAF/DNG decode, demosaic, color mgmt) | Not started | — |
| 3 | Library / browser UI (grid, filmstrip, search, filter) | Not started | — |
| 4 | Develop module (sliders, curves, masks, real-time preview) | Not started | — |
| 5 | Export pipeline (render → JPEG/TIFF/HEIC, presets) | Not started | — |

Sub-projects must generally be tackled in dependency order; #1 must complete
before the others have a foundation to build on.

## Locked-in technology decisions

These are settled and apply across all sub-projects unless explicitly revisited:

- **Language:** Rust everywhere (server + client). Single-language stack
  top-to-bottom.
- **Catalog DB:** libSQL with embedded client replicas. Server runs `sqld`
  embedded inside a custom `shoebox-server` wrapper; each desktop client
  maintains a local libSQL replica for snappy reads.
- **Desktop UI:** Iced (pure-Rust, wgpu-rendered). No webview. Accepts the
  cost of building more UI primitives ourselves in exchange for direct GPU
  access on render-heavy hot paths.
- **Auth:** Internal CA + mutual TLS, bootstrapped via a shared catalog
  secret. mDNS for service discovery only. LAN-default threat model.
- **Edit storage:** Catalog-authoritative. All develop settings, ratings,
  keywords, etc. live in libSQL. XMP sidecar exporter is in the backlog.
- **Collaboration model:** Hybrid — shared organization (keywords, folders,
  collections), per-user creative via virtual copies and per-(user, variant)
  ratings/flags/color labels.
- **Concurrency:** Pessimistic soft lock with takeover request for develop
  edits on a single variant; optimistic / additive everywhere else.
- **Deployment:** Docker (primary), standalone binary (fallback for
  non-Docker NASes / dedicated hosts), Helm chart for Kubernetes. HA and
  scale-out are backlog.

Full rationale and tradeoffs are in the sub-project #1 spec.

## Repository layout

```
shoebox/
├── CLAUDE.md                                ← this file
├── README.md
├── LICENSE
├── Cargo.toml                               ← workspace manifest
├── Dockerfile                               ← multi-stage server image (bundles sqld)
├── crates/
│   ├── shoebox-server/                      ← server binary (data plane)
│   └── shoebox-common/                      ← shared types
└── docs/
    └── superpowers/
        ├── specs/                           ← design specs, one per sub-project
        │   └── 2026-05-17-catalog-sync-and-stack-design.md
        └── plans/                           ← per-sub-project implementation plans
```

Client-side directories (`shoebox-client/`, `deploy/`, etc.) will be added
as Plans 1.4 and 1.5 begin.

## Working on this project

- **Always work from the active spec.** Don't propose architectural changes
  without flagging that they contradict a locked-in decision.
- **One spec per sub-project**, dated `YYYY-MM-DD-<topic>-design.md` under
  `docs/superpowers/specs/`. The superpowers `brainstorming` → `writing-plans`
  → `executing-plans` skills are the intended workflow.
- **Backlog items live in the relevant spec's "Backlog" section,** not in a
  separate file. When a backlog item is picked up, it gets its own spec.

## Implementation status

- `crates/shoebox-server` — full data plane:
  - libSQL embedded `sqld` subprocess + mTLS-protected wire proxy on `/v1/*` and `/v2/*`
  - Filesystem indexer (BLAKE3 hashing, folder mirroring) with `notify`-based live watcher
  - Thumbnailer (256 px + 2 k JPEGs to shared cache, content-addressed by hash)
  - HTTP endpoints: `/enroll`, `/renew`, `/whoami`, `/thumbs/<hash>`, `/previews/<hash>`, `/locks/:variant_id` (acquire/heartbeat/release/takeover)
  - Background tasks: janitor (lock expiry / session cleanup / orphaned-thumb GC), 6 h backups with 14-snapshot rotation, 12 h server-cert renewal check
  - Health + Prometheus `/metrics` on loopback `:9001`
- `crates/shoebox-common` — shared `Error`/`Result`, `UserId`/`MachineId`, `SCHEMA_VERSION`.
- Run locally: `cargo run -p shoebox-server` (mTLS on `0.0.0.0:9000`, health+metrics on `127.0.0.1:9001`).
- Run in Docker: `docker build -t shoebox-server:dev . && docker run --rm -p 9000:9000 -v shoebox-data:/var/lib/shoebox shoebox-server:dev`. The image bundles `sqld`.
- CI: fmt + clippy + tests + docker build on push and PR.
- **Toolchain:** `rust-toolchain.toml` pins `stable`. MSRV in workspace `Cargo.toml` is 1.85 (libsql 0.6 transitive deps require edition2024).

## Known limitations (Plan 1.3 v1)

Surfaced during Plan 1.3 implementation; tracked as memory notes for future attention:

- **rawler forces JPEG decode + re-encode.** No public access to raw embedded JPEG bytes; we decode via rawler and re-encode at quality 90. Net cost: one extra JPEG round-trip per indexed RAW. See memory: `project_rawler_api_constraints.md`.
- **Two writers to `catalog.db`.** The migration runner (`Db`) and the spawned `sqld` subprocess both hold the same SQLite file. SQLite WAL handles this badly across processes; the v1 risk is accepted. Resolution: route all server-side writes through the loopback sqld connection. See memory: `project_libsql_server_unpublished.md`.

## Memory pointers

User has explicitly requested these be remembered across sessions:

- **XMP sidecar exporter is deferred to post-v1 backlog.** The catalog is
  authoritative for all edit data; the exporter is a known future feature.
  See memory: `project_xmp_sidecar_exporter_deferred.md`.
