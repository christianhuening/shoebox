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
| 1 | **Catalog, sync & stack** | Spec drafted 2026-05-17, awaiting user review and implementation plan | [`docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md`](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
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

## Memory pointers

User has explicitly requested these be remembered across sessions:

- **XMP sidecar exporter is deferred to post-v1 backlog.** The catalog is
  authoritative for all edit data; the exporter is a known future feature.
  See memory: `project_xmp_sidecar_exporter_deferred.md`.
