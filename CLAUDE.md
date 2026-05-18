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
| 1 | **Catalog, sync & stack** | Plans 1.1–1.5 + 1.3.5 (replica gRPC + single source of truth) implemented. Sub-project complete. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) · [1.3.5 spec](docs/superpowers/specs/2026-05-18-sub-1-3-5-replica-grpc-and-single-source-of-truth-design.md) |
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
│   ├── shoebox-client/                      ← desktop client (Iced UI, foundation)
│   └── shoebox-common/                      ← shared types
└── docs/
    └── superpowers/
        ├── specs/                           ← design specs, one per sub-project
        │   └── 2026-05-17-catalog-sync-and-stack-design.md
        └── plans/                           ← per-sub-project implementation plans
```

Deployment directories (`deploy/compose/`, `deploy/helm/shoebox/`,
`deploy/systemd/`, `deploy/openrc/`) and release tooling
(`.github/release/`, `.github/workflows/release.yml`,
`.github/workflows/helm-lint.yml`) shipped with Plan 1.5.
Operator-facing docs live in `docs/deployment/`.

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
  - libSQL embedded `sqld` subprocess (both `--http-listen-addr` for Hrana
    and `--grpc-listen-addr` for replication, against one `--db-path`).
    `shoebox-server`'s own `Db` opens a libsql remote client to sqld's HTTP
    port — all server-side writes flow through the same SQLite that backs
    client replicas, no separate `catalog.db`.
  - mTLS proxy on `:9000` that branches by `Content-Type`: gRPC traffic
    (HTTP/2) forwards to sqld's grpc port with the `/v1`/`/v2` path prefix
    stripped; Hrana traffic (HTTP/1.1) forwards to sqld's http port. ALPN
    advertises `h2 + http/1.1`.
  - Filesystem indexer (BLAKE3 hashing, folder mirroring) with `notify`-based live watcher
  - Thumbnailer (256 px + 2 k JPEGs to shared cache, content-addressed by hash)
  - HTTP endpoints: `/enroll`, `/renew`, `/whoami`, `/thumbs/<hash>`, `/previews/<hash>`, `/locks/:variant_id` (acquire/heartbeat/release/takeover)
  - Background tasks: janitor (lock expiry / session cleanup / orphaned-thumb GC), 6 h backups with 14-snapshot rotation, 12 h server-cert renewal check
  - Health + Prometheus `/metrics` on loopback `:9001`
- `crates/shoebox-client` — desktop client foundation (Plan 1.4):
  - Iced single-Application state machine: Discovery → EnterSecret → EnrollProgress (+ KeychainFailure consent) → ProfilePicker → Library
  - First-run wizard: mDNS discovery, manual entry, `/ca-cert` bootstrap, `/enroll`, profile picker, initial replica sync
  - Cert + key storage: OS keychain via `keyring` (Keychain / Credential Manager / Secret Service); explicit-consent mode-0600 file fallback
  - libSQL embedded replica through the mTLS proxy; 30s background catchup ticker
  - 12h background cert renewal task; re-issues when <30 days remain
  - Linux + macOS + Windows from one source tree (manual smoke on each)
- `crates/shoebox-client` — demo library view (Plan 1.4b):
  - Three-pane Library screen: folder tree / photo grid with thumbnails / EXIF + edit detail panel
  - `ThumbCache`: in-memory LRU (1024) + on-disk JPEG cache; mTLS fetch from `<server>/thumbs/<hash>`
  - Editing actions through local replica: rate (per-user UPSERT), keyword add/remove (race-resolved), virtual copy
  - Develop-lock UI: 5 s status poll from local replica; acquire/release/takeover via `/locks/:id`; 5 min heartbeat
  - Keyboard: arrows navigate grid; 0-5 set rating on selected variant
- `deploy/` + `.github/release/` + `.github/workflows/{release,helm-lint}.yml` — full deployment plane (Plan 1.5):
  - Multi-arch Docker image (`linux/amd64` + `linux/arm64`) published to `ghcr.io/<owner>/shoebox-server` on every `v*` tag and on `main`. Arm64 sqld pinned to `37f9eee4...`, amd64 to `71720fc8...`. `release.yml` does a QEMU `sqld --help` smoke after push to catch arm64 layer regressions.
  - GitHub Releases standalone tarballs: `linux-amd64`, `linux-arm64` (via `cross 0.2.5`), `macos-arm64` (`macos-14` runner). Each bundles `shoebox-server` + matching pinned `sqld` + Linux: systemd + OpenRC units, macOS: launchd plist + `config.example.toml` + README, with sha256 sidecar.
  - Helm chart (`deploy/helm/shoebox/`): single-replica, two PVCs (data unconditional + optional cache), photos via existingClaim or hostPath. Auto-generated bootstrap Secret with `helm.sh/resource-policy: keep` AND a `lookup` guard so `helm upgrade` reuses the in-cluster value (avoids rotating clients' cert-signing CA). `_helpers.tpl` validators (`shoebox.validateSecret`, `shoebox.validatePhotos`) `fail` fast on misconfig. `helm-lint.yml` enforces `helm lint` + `helm template` golden-file diff (SHOEBOX_SECRET redacted) on PRs touching `deploy/helm/**`.
  - Compose example (`deploy/compose/`): single-service `docker-compose.yml` (with `SHOEBOX_HEALTH_BIND_ADDR=0.0.0.0:9001` baked in so host port-mapping reaches health) + `.env.example` + README. CI smoke-tests it (`compose-smoke` job in `ci.yml`, 60 s ceiling).
  - Binary smoke (`binary-smoke` job in `ci.yml`): builds the linux-amd64 tarball, verifies the tar listing contains all expected files (bin/, share/systemd/, share/openrc/, etc.), extracts, runs the server against synthetic env, hits `/health`, kills cleanly. arm64 + macos targets get build-only validation; runtime smoke for those requires hardware (backlog).
  - Three deployment quickstarts under `docs/deployment/{quickstart-docker,quickstart-binary,quickstart-kubernetes}.md`.
- `crates/shoebox-common` — shared `Error`/`Result`, `UserId`/`MachineId`, `SCHEMA_VERSION`.
- Run locally:
  - Server: `cargo run -p shoebox-server` (mTLS on `0.0.0.0:9000`, health+metrics on `127.0.0.1:9001`).
  - Client: `cargo run -p shoebox-client` (against a running `shoebox-server`).
- Run in Docker: `docker build -t shoebox-server:dev . && docker run --rm -p 9000:9000 -v shoebox-data:/var/lib/shoebox shoebox-server:dev`. The image bundles `sqld`. (For end-user deployment, see `docs/deployment/quickstart-docker.md`.)
- CI: fmt + clippy + tests + docker build + compose-smoke + binary-smoke on push and PR (`ci.yml`); helm-lint on PRs touching `deploy/helm/**` (`helm-lint.yml`); multi-arch image push + tarball release + chart package on `v*` tag (`release.yml`).
- **Toolchain:** `rust-toolchain.toml` pins `stable`. MSRV in workspace `Cargo.toml` is 1.85 (libsql 0.6 transitive deps require edition2024).

## Known limitations (Plan 1.3+1.4+1.4b v1)

Surfaced during Plan 1.3/1.4/1.4b implementation; tracked as memory notes for future attention:

- **rawler forces JPEG decode + re-encode.** No public access to raw embedded JPEG bytes; we decode via rawler and re-encode at quality 90. Net cost: one extra JPEG round-trip per indexed RAW. See memory: `project_rawler_api_constraints.md`.
- **No grid virtualization.** Folders with thousands of photos will render slowly. Plan 1.4b grids ~30-photo test sets cleanly; full virtualization is sub-project #3.
- **Lock UI surfaces 4 states, no auto-release on app exit.** Releasing a lock requires the user clicking Release; if the app dies, the lock expires via the server janitor's 30 min TTL instead.
- **Client-side writes via the embedded replica are broken** (libsql 0.9.30 ↔ sqld 0.24.32 protocol mismatch). The libsql client sends `x-authorization` on WriteProxy gRPC calls but sqld 0.24.32 (latest released) expects `x-proxy-authorization` and rejects with `InvalidArgument`. Reads via the replica work fine, and server-side writes (via libsql Remote/Hrana, which bypasses WriteProxy) work fine — but `conn.execute()` from the client's `Replica` will fail. Resolution path: replace the client's replica-write code paths with custom HTTP endpoints handled server-side, or wait for an upstream sqld release that accepts `x-authorization` (or a libsql release that sends `x-proxy-authorization`). Tracked in sub-1-3-5 spec follow-ups.

## Memory pointers

User has explicitly requested these be remembered across sessions:

- **XMP sidecar exporter is deferred to post-v1 backlog.** The catalog is
  authoritative for all edit data; the exporter is a known future feature.
  See memory: `project_xmp_sidecar_exporter_deferred.md`.
