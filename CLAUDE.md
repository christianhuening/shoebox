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
| 1 | **Catalog, sync & stack** | Plans 1.1+1.2 implemented — workspace, schema, /health, mDNS, mTLS + enrollment + revocation, Dockerfile, CI. Plans 1.3-1.5 pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
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
└── docs/
    └── superpowers/
        └── specs/                           ← design specs, one per sub-project
            └── 2026-05-17-catalog-sync-and-stack-design.md
```

Implementation directories (`shoebox-server/`, `shoebox-client/`, `shoebox-common/`,
`deploy/`, etc.) will be added as sub-project #1 implementation begins.

## Working on this project

- **Always work from the active spec.** Don't propose architectural changes
  without flagging that they contradict a locked-in decision.
- **One spec per sub-project**, dated `YYYY-MM-DD-<topic>-design.md` under
  `docs/superpowers/specs/`. The superpowers `brainstorming` → `writing-plans`
  → `executing-plans` skills are the intended workflow.
- **Backlog items live in the relevant spec's "Backlog" section,** not in a
  separate file. When a backlog item is picked up, it gets its own spec.
- **No code exists yet.** Implementation of sub-project #1 has not started.

## Implementation status

- `crates/shoebox-server` — workspace skeleton, libSQL catalog with 6 migrations, internal Ed25519 CA + mTLS, /enroll + /renew + /whoami endpoints, CRL-aware client cert verification, clap CLI with `serve`/`revoke` subcommands, mDNS broadcaster, multi-stage Dockerfile. **Plan 1.3 in progress**: data-plane deps added (Task 1); `sqld_embed` module spawns sqld as a child subprocess (Task 2, pivoted — libsql-server isn't published on crates.io). Tasks 3-22 remaining: proxy, indexer, thumbnailer, dev-locks, janitor, backups, /metrics, cert renewal, integration tests.
- `crates/shoebox-common` — shared `Error`/`Result`, `UserId`/`MachineId` types, `SCHEMA_VERSION` constant.
- Run locally: `cargo run -p shoebox-server` (mTLS on `0.0.0.0:9000`, health on `127.0.0.1:9001`). Plan 1.3 work in progress means the proxy + indexer etc. aren't wired into main.rs yet.
- Run in Docker: `docker build -t shoebox-server:dev . && docker run --rm -p 9000:9000 -v shoebox-data:/var/lib/shoebox shoebox-server:dev`.
- CI: fmt + clippy + tests + docker build on push and PR.
- **Toolchain:** `rust-toolchain.toml` pins `stable` (currently ~1.95). MSRV in workspace `Cargo.toml` is 1.85 — that's the floor for `libsql 0.6`'s transitive deps (edition2024).

## Resuming Plan 1.3 execution

To continue Plan 1.3 in a fresh session:
- Plan path: `docs/superpowers/plans/2026-05-17-sub-1-3-server-data-plane.md`
- Latest commit on data-plane work: `e003a54`
- **Task 2 was pivoted:** the plan as written assumed `libsql-server` could be embedded as a crate dep. It can't (only published in the libsql monorepo with incompatible axum 0.6). Task 2 now spawns standalone `sqld` as a subprocess via `crates/shoebox-server/src/sqld_embed.rs`. See memory `project_libsql_server_unpublished.md` for the full reasoning and the known v1 two-writers-to-catalog.db architectural debt.
- **Tasks 3 onward proceed as written in the plan**, but the proxy (Task 3) targets the sqld subprocess URL (already returned by `sqld_embed::start`).
- **Task 4 (proxy e2e test) and Task 19/20 (indexer/locks e2e tests) need `sqld` on PATH** to actually run. Install with `cargo install --git https://github.com/tursodatabase/libsql sqld` or download a release binary; otherwise the proxy test will need to be gated like `sqld_embed::tests::starts_subprocess_if_sqld_present` (skip when absent).
- **Task 22 (Dockerfile updates) must install `sqld` in the runtime image** so the deployed container can spawn it.

## Memory pointers

User has explicitly requested these be remembered across sessions:

- **XMP sidecar exporter is deferred to post-v1 backlog.** The catalog is
  authoritative for all edit data; the exporter is a known future feature.
  See memory: `project_xmp_sidecar_exporter_deferred.md`.
