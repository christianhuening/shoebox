# Sub-project 2 — RAW Pipeline Design

**Status:** Draft (2026-05-19)
**Parent spec:** `2026-05-17-catalog-sync-and-stack-design.md`
**Predecessor sub-project:** #1 (Catalog, sync & stack) — complete
**Successor sub-projects:** #4 (Develop module) extends this; #5 (Export) consumes it.

---

## 1. Goal

Build the foundational image-rendering pipeline that turns a RAW file
(PEF/RAF/DNG initially) into accurate, color-managed pixel data on the
client. The pipeline is the substrate the Develop module (#4) plugs
adjustment math into and that the Export pipeline (#5) drives at full
resolution for output renders.

After this sub-project, shoebox can:

- Decode a PEF/RAF/DNG file on the client via libraw.
- Demosaic and color-correct to a documented linear working space (Rec.2020).
- Cache the result on disk as a per-photo `.linear` file (content-addressed
  by photo hash), with LRU eviction against a configurable disk budget.
- Render that cached linear image to an sRGB 8-bit display surface on the
  GPU via wgpu, sharing the Iced renderer's device for zero-copy handoff.
- Surface a typed-stage extension point (`DevelopStage` enum) that #4
  populates without touching the rendering loop's structure.

There is **no UI integration** in this sub-project. A library smoke test
and integration tests against synthetic RAW fixtures prove the path. UI
hookup is a #4 deliverable.

## 2. Non-goals (carried to backlog)

- **Develop math.** Exposure, white balance, tone curves, contrast,
  highlights/shadows, masks, virtual-copy diffing — all live in #4.
- **Real-time slider feedback.** Achievable via the architecture below,
  but the actual `<33ms` interactive loop is #4's measurement to hit.
- **Tile-based / pyramid rendering.** Whole-image upload. 1:1 zoom on a
  100 MP image and panning are #3 (browser UI) or #4 concerns.
- **Server-side rendering.** Catalog spec already established client-only
  develop renders. Server keeps the existing embedded-JPEG thumbnailer.
- **ICC display profiles.** Architecture supports it (lcms2 is in), but v1
  always outputs sRGB 8-bit. Custom display profile is a backlog item.
- **HDR output.** Same — architecture supports it (we render in linear
  Rec.2020), v1 tonemaps to sRGB.
- **Camera-compatibility list maintenance.** We support whatever libraw
  supports. Unknown cameras return `PipelineError::Decode` cleanly.
- **GPU demosaic.** CPU decode + demosaic via libraw is sufficient for v1.
- **Render-quality A/B vs Lightroom / Capture One.** Calibration job for
  later sub-projects.
- **Bit-exact rendering across GPUs.** Visually identical is the contract;
  GPU rounding differences are accepted.
- **Adobe XMP / develop-settings interop.** The catalog is authoritative
  (parent spec §4.6); XMP sidecar exporter remains backlog (memory note
  `project_xmp_sidecar_exporter_deferred.md`).

## 3. Locked-in decisions

These came out of brainstorming and are settled for this sub-project:

| Decision | Choice | Rationale |
|---|---|---|
| Where the pipeline runs | Client-only | Server's embedded-JPEG thumbs unchanged. Each client owns its linear cache. Matches the catalog-authoritative model. |
| Compute split | Hybrid: CPU bake → linear cache → GPU render | CPU decode+demosaic is expensive but one-time per RAW. GPU runs color and (in #4) develop math at interactive rates. The `.linear` file is the explicit boundary. |
| Decoder library | **libraw via FFI** | Industry-standard quality (AHD/AMaZE demosaic, mature decode coverage). Deliberate exception to the all-Rust stack premise — quality outweighs purity for the develop pipeline. rawler stays in the server for embedded-JPEG thumbs. |
| Color management | **lcms2 via FFI** (`lcms2` Rust crate) | Full ICC, supports camera and display profiles, well-maintained Rust bindings. |
| Working color space | **Linear Rec.2020 (D65), half-float** | Real-display-target standard for modern wide-gamut and HDR displays. ProPhotoRGB is wider in absolute volume but uses imaginary primaries outside the visible spectrum, which complicates downstream tone mapping. Rec.2020 stays inside the visible gamut and aligns with the HDR roadmap. |
| Output color space | sRGB 8-bit | Default for v1; lcms2 lets us add custom display profiles trivially later. |
| Stage framework | Two-phase: monolithic CPU bake + GPU Renderer driven by `#[non_exhaustive] DevelopStage` enum | Mirrors the CPU/GPU split. No over-abstraction. #4 adds enum variants without touching call sites. |
| Cache file format | Custom binary (`.linear`, ~30 LOC reader/writer) | Internal-only, never read by external tools. `exr` crate dep buys features we don't use. |
| libraw distribution | Dynamic linking, libraw shipped alongside the binary | LGPLv2.1 compliance with static linking requires shipping relink-able object files; dynamic linking is the clean path for an Apache-2.0 project. |
| GPU device | Renderer accepts `Arc<wgpu::Device>` + `Queue` from the caller | Lets the Renderer share Iced's device — zero-copy texture handoff to UI. Tests construct a private device via wgpu's headless backend. |
| Determinism | Visually identical across machines, **not** bit-exact | GPU rounding differs across vendors. Documented non-goal. |

## 4. Architecture overview

```
                 ┌──────────────────── shoebox-pipeline ────────────────────┐
                 │                                                          │
RAW file ────────│──► [ Bake (CPU, sync) ]                                  │
(.PEF/.RAF/.DNG) │      ├─ libraw: parse + demosaic (AHD by default)        │
                 │      ├─ camera-matrix → linear Rec.2020 (lcms2)          │
                 │      └─ write .linear file (half-float Rec.2020 RGB)     │
                 │                       │                                  │
                 │                       ▼                                  │
                 │      <cache>/linear/<photo_hash>.linear  ◄── LRU evicted │
                 │                       │                                  │
                 │                       ▼                                  │
                 │       [ Render (GPU, wgpu) ]                             │
                 │         ├─ load .linear → wgpu::Texture (f16)            │
                 │         ├─ apply Vec<DevelopStage> (empty in #2)         │
                 │         └─ output color transform → display profile      │
                 │                       │                                  │
                 └───────────────────────│──────────────────────────────────┘
                                         ▼
                                  Iced render target
                                  (sRGB 8-bit by default)
```

The Bake/Render boundary is the `.linear` cache file. CPU writes it
synchronously inside a `spawn_blocking` worker. GPU reads it via
`Renderer::render`. Two clients viewing the same photo each maintain
their own linear cache; the file is content-addressed by photo hash so
re-baking is deterministic.

## 5. Crate layout

```
crates/shoebox-pipeline/
├── Cargo.toml
└── src/
    ├── lib.rs              ← public API re-exports
    ├── types.rs            ← LinearImage, Rgba8Image, WorkingSpace, OutputColorSpace, BakeOptions
    ├── error.rs            ← PipelineError
    ├── bake/
    │   ├── mod.rs          ← bake_linear() entry point
    │   ├── decoder.rs      ← libraw wrapper: RAW → demosaicked camera-RGB
    │   └── color_in.rs     ← camera matrix → linear Rec.2020 (lcms2)
    ├── cache.rs            ← .linear file format + LinearCache + LRU
    └── render/
        ├── mod.rs          ← Renderer + render() entry point
        ├── gpu.rs          ← wgpu device sharing + texture upload + caching
        ├── stages.rs       ← DevelopStage enum (#[non_exhaustive], empty in #2) + dispatch
        └── color_out.rs    ← Rec.2020 → sRGB transform (LUT baked by lcms2, sampled by shader)

crates/libraw-sys/          ← our own bindgen wrapper for libraw
├── Cargo.toml
├── build.rs                ← pkg-config / vcpkg / fallback path detection
└── src/lib.rs              ← bindgen-generated, hand-edited surface (decode + demosaic only)
```

`shoebox-client` adds `shoebox-pipeline` as a workspace dep. `shoebox-server`
does not depend on it. `shoebox-common` is untouched.

`libraw-sys` is owned by us rather than a third-party crate. Existing
crates (`libraw-rs`, `libraw-r`) are unmaintained or expose more surface
than we need. Owning ~150 lines of bindgen output is cheaper than
inheriting their drift.

## 6. Public API surface

```rust
// shoebox-pipeline/src/lib.rs

pub fn bake_linear(raw_path: &Path, opts: &BakeOptions)
    -> Result<LinearImage, PipelineError>;

#[derive(Debug, Clone)]
pub struct BakeOptions {
    pub demosaic: DemosaicAlgorithm, // default: Ahd
    pub working_space: WorkingSpace, // default: Rec2020
    // Camera-profile override path; None uses libraw's internal matrix.
    pub camera_profile_icc: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    pub working_space: WorkingSpace,
    pub pixels: Vec<half::f16>,  // len = width * height * 3, interleaved RGB
                                 // Same layout as the .linear file's pixel block so the
                                 // cache reader can mmap directly into this representation.
}

#[derive(Debug, Clone, Copy)]
pub enum WorkingSpace { Rec2020 } // future: ProPhotoRGB, ACEScg

#[derive(Debug, Clone, Copy)]
pub enum DemosaicAlgorithm { Ahd, Linear } // libraw exposes more; v1 ships two

pub struct Renderer { /* owns shared wgpu Device + Queue + pipelines + texture cache */ }

impl Renderer {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>)
        -> Result<Self, PipelineError>;

    pub fn render(
        &self,
        linear: &LinearImage,
        stages: &[DevelopStage],
        output: OutputColorSpace,
    ) -> Result<Rgba8Image, PipelineError>;
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum DevelopStage {
    // Empty in #2. #4 will add variants like:
    //   Exposure(f32),
    //   WhiteBalance { temp_k: u32, tint: i32 },
    //   ToneCurve(Vec<[f32; 2]>),
    //   ...
}

pub enum OutputColorSpace {
    Srgb8,
    // future: IccProfile(Vec<u8>),
}

pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>, // width * height * 4, RGBA
}

// Cache
pub struct LinearCache { /* internal */ }

impl LinearCache {
    pub fn open(dir: &Path, budget_bytes: u64) -> Result<Self, PipelineError>;

    /// Single-flight: concurrent calls for the same `photo_hash` share one bake.
    pub async fn get_or_bake_async(
        &self,
        photo_hash: &str,
        raw_path: &Path,
        opts: &BakeOptions,
    ) -> Result<LinearImage, PipelineError>;

    /// Synchronous variant for non-async callers (e.g. #5 export).
    pub fn get_or_bake(
        &self,
        photo_hash: &str,
        raw_path: &Path,
        opts: &BakeOptions,
    ) -> Result<LinearImage, PipelineError>;

    /// Hint that a photo is about to be needed. Spawns a background bake if
    /// the cache is cold. Returns immediately.
    pub fn prefetch(&self, photo_hash: &str, raw_path: &Path, opts: &BakeOptions);

    /// Cancel a pending prefetch before it starts. No effect once libraw is
    /// underway.
    pub fn cancel_pending(&self, photo_hash: &str);

    /// Bytes freed.
    pub fn evict_to_budget(&self) -> Result<u64, PipelineError>;
}
```

## 7. `.linear` cache file format

```
offset  bytes  field
0       4      magic        b"SBLN"
4       2      version      u16 = 1               (little-endian)
6       1      working_space u8  = 0 (Rec2020)
7       1      reserved     u8  = 0
8       4      width        u32                   (little-endian)
12      4      height       u32                   (little-endian)
16      4      hash_low32   u32 — low 32 bits of photo_hash (sanity check)
20      *      pixels       width * height * 3 * f16 (little-endian)
EOF-4   4      crc32        u32 — CRC32 of all preceding bytes (incl. header)
```

Reader validates magic, version, that `len(pixels) == width * height * 6`,
and CRC32. Any failure returns `PipelineError::CacheCorrupt`, which the
cache layer responds to by deleting the file and falling through to a
fresh bake (one retry; second failure propagates).

Writer uses the atomic pattern: write to `<path>.tmp`, fsync, rename over
the final path. This matches the existing `thumbnailer.rs` convention.

Future-compat: `version` is bumped on any layout change. Reader rejects
unknown versions; cache layer treats this as "corrupt" and re-bakes.

## 8. Data flow & lifecycle

### When the pipeline runs

The pipeline runs only when a sensor-accurate render is required. The
server's embedded-JPEG thumb path is untouched and remains the source of
truth for grid views and quick-look previews.

Triggers:

1. User enters the Develop module (#4) on a photo.
2. User zooms past the 2 k embedded preview's effective resolution.
3. UI prefetch hint from the filmstrip or grid focus (optional, opt-in).
4. Export (#5) — direct `bake_linear()` call, typically bypasses the
   cache so output is deterministic.

There is **no eager bake on indexer add**. A 100 k-photo library would
consume ~14 TiB of linear caches at f16 RGB. Bake is on-demand only.

### Lifecycle of a single render

```
User opens photo X in Develop:
  │
  ├─ shoebox-client looks up photo_hash from local libSQL replica
  │
  ├─ LinearCache::get_or_bake_async(hash, raw_path, opts).await
  │     │
  │     ├─ cache hit?  mmap + parse header → LinearImage         (~50 ms)
  │     │
  │     └─ cache miss?
  │           ├─ spawn_blocking { bake_linear(raw_path, opts) }  (1–3 s)
  │           ├─ atomic write to <cache>/linear/<hash>.linear
  │           ├─ schedule LinearCache::evict_to_budget()         (background)
  │           └─ return LinearImage
  │
  ├─ Renderer::render(&linear, &develop_stages, OutputColorSpace::Srgb8)
  │     ├─ upload f16 pixels → wgpu::Texture                     (once per LinearImage)
  │     ├─ run color_in stage  (Rec2020 → working LUT in shader)
  │     ├─ run develop stages  (empty in #2; #4 fills this)
  │     ├─ run color_out stage (Rec2020 → sRGB LUT)
  │     └─ readback Rgba8Image                                   (5–30 ms)
  │
  └─ Iced renders Rgba8Image to the screen
```

### Cache location

Sits under the existing client cache dir, alongside the structures
introduced in Plan 1.4:

```
<client_cache>/
├── preview-cache/<hash>.jpg          ← LRU of server-side 2k JPEGs (existing)
├── render-cache/<variant_id>.jpg     ← develop renders, per-variant (#4 populates)
└── linear/<photo_hash>.linear        ← NEW: per-RAW linear-RGB bake (this sub-project)
```

### Cache eviction

- LRU by mtime, touched on every cache hit.
- Background eviction runs when `cache.write_bytes() > budget * 1.1`
  after a new bake.
- Eviction only deletes files; in-memory `LinearImage` copies survive.
- Files held open as `wgpu::Texture` survive deletion on POSIX (open
  fd keeps the inode alive); on Windows, eviction skips files we
  currently hold (via a small "pinned" set in the cache).

## 9. Threading model

- **`bake_linear()`** is synchronous and slow (1–3 s on a 24 MP RAW).
  All entry points wrap it in `tokio::task::spawn_blocking`.
  `LinearCache::get_or_bake_async()` is the recommended call site from
  async code; it handles `spawn_blocking` internally.
- **`Renderer::render()`** is synchronous but fast (a single wgpu
  submission + readback). Called from the UI render thread or a
  dedicated render task in the Develop module.
- **`LinearCache`** uses an internal `HashMap<photo_hash,
  Shared<Future<LinearImage>>>` to dedupe concurrent bakes. The Develop
  module opening a photo while the grid prefetched it must not run
  libraw twice.
- **Eviction** runs on a `tokio::task::spawn` background task with a
  debounce. Never blocks bake or render.

## 10. Cancellation semantics

- Bake mid-libraw call is **not** cancellable. libraw is a single
  blocking C call. Acceptable: 1–3 s worst case, and the result lands
  in cache so wasted work is bounded.
- `LinearCache::cancel_pending()` removes a queued bake before
  `spawn_blocking` runs. After libraw starts, the result is delivered
  regardless.
- Render is too fast (<30 ms) for cancellation to matter; the next
  render supersedes the prior one naturally.

## 11. Error handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("RAW file not readable: {0}")]
    Io(#[from] std::io::Error),

    #[error("libraw failed to decode {path}: {reason}")]
    Decode { path: PathBuf, reason: String },

    #[error("libraw runtime not found — install libraw or set SHOEBOX_LIBRAW_PATH")]
    LibrawMissing,

    #[error("ICC transform failed: {0}")]
    ColorTransform(String),

    #[error("cache file {path} is corrupt: {reason}")]
    CacheCorrupt { path: PathBuf, reason: String },

    #[error("cache disk budget exhausted; freed {freed} bytes, still over by {over} bytes")]
    CacheBudgetExhausted { freed: u64, over: u64 },

    #[error("wgpu device lost during render")]
    DeviceLost,

    #[error("wgpu compute pipeline build failed: {0}")]
    GpuPipelineBuild(String),

    #[error("internal: {0}")]
    Internal(String),
}
```

| Failure | Detection | Behavior |
|---|---|---|
| libraw can't decode the file | libraw error code | `Decode { path, reason }`. Caller surfaces "Cannot decode RAW" UI; photo remains in catalog, user can retry. |
| libraw runtime missing | Lazy `dlopen` at first bake | First bake call returns `LibrawMissing`. Banner UI; catalog sync / grid / ratings / keywords keep working. |
| `.linear` file corrupt | Magic, CRC32, length checks | Delete file, single retry via fresh bake. Second failure propagates `CacheCorrupt`. |
| Cache disk budget exhausted | Eviction can't free enough | `CacheBudgetExhausted`. Caller can offer to raise the budget. |
| libraw FFI panic | `catch_unwind` around the FFI call | Convert to `Decode { reason: "FFI panic" }`. Worker thread survives. |
| wgpu device lost | wgpu `DeviceLostInfo` | Mark Renderer poisoned. Caller recreates Renderer with fresh device on next call. Cached `LinearImage`s in memory are still valid; GPU textures get re-uploaded lazily. |
| Concurrent bakes of same photo | Single-flight map in `LinearCache` | Second caller awaits the first call's future. No duplicate libraw work. |
| Mid-bake process exit | Atomic `tmp` → fsync → rename | `LinearCache::open()` does a startup sweep for orphaned `.tmp` files. |
| Render produces zero output | wgpu validation in debug builds + unit tests | Caught in tests; no runtime detection (too expensive per frame). |

Explicit non-handling:

- Cameras unknown to libraw → return `Decode` cleanly. No
  compatibility-list maintenance in this sub-project.
- Corrupt embedded ICC profiles in the RAW → fall back to libraw's
  built-in camera matrix; log a warning.
- OOM during bake on hypothetical 4-billion-pixel inputs → allocator
  panics; `catch_unwind` converts to `Internal`. Accept for v1.

## 12. Build & distribution

### libraw

- License: LGPLv2.1 / CDDL / commercial (triple-licensed). shoebox is
  Apache-2.0. Dynamic linking against the LGPLv2.1 build satisfies the
  weak-copyleft obligation without us shipping relink-able object files.
- Distribution: ship the libraw shared library alongside our binary
  (Linux/macOS: `<prefix>/lib/`; Windows: same dir as `.exe`). Adjust
  rpath at packaging time:
  - Linux: `patchelf --set-rpath '$ORIGIN/../lib' shoebox-client`
  - macOS: `install_name_tool -change` + `@executable_path/../Frameworks`
  - Windows: DLL in the same dir as the `.exe`
- `libraw-sys` `build.rs`:
  - Prefer `pkg-config --libs libraw_r`, falling back to `libraw`.
  - On Windows, look up via `VCPKG_ROOT` and `vcpkg/installed/x64-windows`.
  - Require libraw ≥ 0.21 (ABI stability); produce a clear build error
    otherwise.
  - Optional vendored build (off by default) via the `vendored` Cargo
    feature for fully hermetic builds.

### lcms2

- License: MIT. No distribution concerns beyond shipping the shared
  library, identical to libraw.
- Use the existing `lcms2` crate; it handles bindings.

### CI changes

- `ci.yml`:
  - Linux runner: `apt-get install -y libraw-dev liblcms2-dev`.
  - macOS runner: `brew install libraw little-cms2`.
  - Windows job (build-only today): `vcpkg install libraw lcms`.
- `release.yml`:
  - Linux x86_64 / arm64 (via `cross`): install dev packages in the
    cross image (use a custom `Cross.toml` with a pre-build hook).
  - macOS arm64 (`macos-14`): same brew install, then `install_name_tool`
    on packaging.
  - Bundle the dynamic libraries into the tarball.
- New CI smoke: link-check job that loads the shipped binary in a clean
  Docker container with no system libraw and asserts the rpath finds the
  bundled library.

### Local development

- CLAUDE.md adds a "Pipeline dependencies" subsection documenting the
  one-time install of libraw and lcms2 dev packages for local builds.
- A `make dev-deps` (or `scripts/install-dev-deps.sh`) handles all
  three platforms.

## 13. Testing strategy

| Layer | Scope | Notes |
|---|---|---|
| Unit | `.linear` header round-trip, CRC32 verification, LRU mtime ordering, working-space matrix constants, error mapping from libraw return codes | No fixtures. Every CI build. |
| `bake/` integration | `bake_linear()` against real RAW fixtures (PEF, RAF, DNG) | Fixtures under `crates/shoebox-pipeline/tests/fixtures/`. ~2 MB each, ~640×480. Golden-hash assertion catches regressions in libraw or our color-in path. |
| `cache/` integration | Hit/miss, single-flight concurrency, eviction, corruption recovery, orphan `.tmp` cleanup | `tempdir` + synthetic `.linear` files. |
| `render/` integration | `Renderer::render()` with empty stages, asserts color-out produces expected sRGB pixels for a known linear input | Headless wgpu device via `Backends::PRIMARY` / fallback to llvmpipe on Linux CI. Gated behind `--features gpu-tests` so platforms without a usable GPU can skip. |
| Color correctness | Synthetic `LinearImage` with known Rec.2020 values (e.g., pure red `[1.0, 0.0, 0.0]`); assert sRGB output matches lcms2 CPU reference within `0.5/255` | Catches GPU LUT vs CPU LUT divergence. |
| End-to-end | Deferred to #4 (manual smoke on a real RAW through the Develop UI) | Not in #2's scope. |

### Performance budgets (asserts on a baseline machine)

| Operation | Cold | Warm cache |
|---|---|---|
| `bake_linear` 24 MP PEF | < 3 s | n/a |
| `LinearCache::get_or_bake` cache hit | < 100 ms | < 100 ms |
| `Renderer::render` 24 MP, empty stages | < 50 ms | < 30 ms |
| `evict_to_budget` over 20 GiB cache dir | < 200 ms | n/a |

Budgets gate regressions, not feature acceptance. Failing budgets in CI
warn unless `BENCH_STRICT=1`.

### RAW fixtures

Three small synthetic samples committed to the repo, sourced from RAWs
the project author owns and resampled or cropped to ~640×480:

- `tests/fixtures/pentax_k1_synthetic.pef`
- `tests/fixtures/fuji_xt4_synthetic.raf`
- `tests/fixtures/leica_q_synthetic.dng`

If lossless resampling proves impractical for a given format, fall back
to the smallest real RAW available (~3 MB ceiling). The plan settles
this; the spec just declares the artifacts.

## 14. Interaction with adjacent sub-projects

| Sub-project | Direction | Interaction |
|---|---|---|
| #1 (catalog, sync, stack) | Upstream, complete | Reads `photos.id` (BLAKE3 hash), `photo_files.path` from the local libSQL replica to locate RAWs. No schema changes. |
| #3 (Library / browser UI) | Sibling | Continues to consume server-side embedded-JPEG thumbs (unchanged). May optionally call `LinearCache::prefetch()` when filmstrip focus settles. |
| #4 (Develop module) | Downstream | Adds variants to `DevelopStage`. Owns the UI render loop that calls `Renderer::render()` per slider change. Adds wgpu compute shaders per stage. |
| #5 (Export) | Downstream | Calls `bake_linear()` directly (bypassing the cache typically). Adds its own output paths and presets. |

## 15. Open questions deferred to the plan

- Exact bindgen surface in `libraw-sys` (which libraw functions to expose;
  prefer the minimal set: open, unpack, demosaic, fetch processed
  image, free).
- Whether `Renderer` caches GPU textures keyed by `LinearImage` pointer
  identity vs photo hash. (Likely hash, with weak refs.)
- Exact wgpu backend selection in tests (Vulkan vs Metal vs DX12 vs
  llvmpipe) and how to gate CI runners that lack any.
- Whether to support `BakeOptions::demosaic = Linear` in v1 or AHD only.
- Specific tooling for resampling real RAWs into tiny test fixtures.

## 16. Backlog (post-#2)

- ICC display profile output (`OutputColorSpace::IccProfile`).
- HDR output path (PQ / HLG encode from linear Rec.2020).
- Tile-based linear cache for very large sensors / large libraries.
- GPU demosaic (replace libraw's CPU demosaic on the Bake path).
- libraw → rawler swap if rawler's decode/demosaic catches up — would
  eliminate the C dep and align with the Rust-only stack decision.
- Cache sharing across the LAN (NAS-side linear cache populated by one
  client, consumed by others). Probably more trouble than it's worth.
- Camera-profile UI: let users supply a `.icc` for their body/lens combo.
- Smarter prefetch policy informed by the Library UI's scroll position.
