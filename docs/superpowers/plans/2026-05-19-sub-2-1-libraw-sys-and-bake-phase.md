# Plan 2.1 — libraw-sys + Bake Phase

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the two new crates (`libraw-sys`, `shoebox-pipeline`) and the `bake_linear()` entry point. After this plan, calling `bake_linear(raw_path, &BakeOptions::default())` against a real RAW fixture returns a `LinearImage` in linear Rec.2020 half-float — proven by an integration test. No cache, no GPU, no UI integration yet.

**Architecture:** Two new workspace crates. `libraw-sys` is a thin bindgen wrapper around the system libraw shared library (dynamically linked, LGPL-compliant). `shoebox-pipeline` is the pure-Rust crate that owns the public pipeline API; its `bake/` module wraps libraw to produce a `LinearImage`. Color management for the bake phase uses libraw's built-in linear Rec.2020 output (`output_color = 8`, `output_bps = 16`, linear gamma) — lcms2 is added as a workspace dep for later plans (2.3 render-time color transforms) but is not invoked from `bake_linear` in v1. This is a documented spec deviation against `2026-05-19-raw-pipeline-design.md` §6 (the `BakeOptions::camera_profile_icc` field is omitted from the v1 struct and slated for a future non-breaking addition).

**Tech Stack:** Rust (workspace MSRV 1.85), `bindgen` 0.71, libraw ≥ 0.21 (system shared library), `lcms2` 6.x Rust crate (workspace dep, unused in this plan), `half` 2.x for f16, `thiserror` 2, plus the existing workspace deps. Tests use the stdlib + `tempfile` + a committed ~3 MB DNG fixture.

**Prerequisites for the implementing engineer:**
- Working Rust install + the workspace's pinned 1.85 toolchain.
- `libraw-dev` and `liblcms2-dev` installed (Linux), or `brew install libraw little-cms2` (macOS). Windows is not exercised by this plan's tests; vcpkg integration arrives with Plan 2.4.
- Familiarity with Rust's FFI rules (raw pointers, `extern "C"`, `CStr`) is required for Task 6. No prior libraw experience is needed — the exact call sequence is shown.

---

## File Structure

This plan adds two crates and one fixture file.

```
shoebox/
├── Cargo.toml                                   ← MODIFIED: workspace members + deps
├── CLAUDE.md                                    ← MODIFIED: pipeline-deps note
├── crates/
│   ├── libraw-sys/                              ← NEW
│   │   ├── Cargo.toml
│   │   ├── build.rs                             ← bindgen + pkg-config link
│   │   ├── wrapper.h                            ← #include <libraw/libraw.h>
│   │   ├── src/lib.rs                           ← include!(bindings.rs)
│   │   └── tests/smoke.rs                       ← libraw_version() smoke test
│   ├── shoebox-pipeline/                        ← NEW
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs                           ← public re-exports
│   │   │   ├── types.rs                         ← LinearImage, BakeOptions, ...
│   │   │   ├── error.rs                         ← PipelineError
│   │   │   └── bake/
│   │   │       ├── mod.rs                       ← bake_linear() entry point
│   │   │       ├── decoder.rs                   ← libraw FFI wrapper
│   │   │       └── color_in.rs                  ← u16 Rec.2020 → f16 Rec.2020
│   │   └── tests/
│   │       ├── fixtures/
│   │       │   └── sample.dng                   ← ~3 MB CC0 RAW fixture
│   │       └── bake_e2e.rs                      ← bake_linear() integration test
│   └── shoebox-client/Cargo.toml                ← MODIFIED: depend on shoebox-pipeline
└── scripts/
    └── install-pipeline-deps.sh                 ← NEW: one-shot dev-deps installer
```

**Responsibility split:**
- `libraw-sys/src/lib.rs` does ONE thing — expose generated bindings. No safe wrappers.
- `shoebox-pipeline/src/bake/decoder.rs` does ONE thing — drive libraw and return `(width, height, Vec<u16>)`. No type juggling beyond what libraw forces.
- `shoebox-pipeline/src/bake/color_in.rs` does ONE thing — convert `Vec<u16>` linear-Rec.2020 to `Vec<f16>`. Trivial in v1; becomes the lcms2 hook in a future plan.
- `shoebox-pipeline/src/bake/mod.rs` does ONE thing — orchestrate the two above and assemble a `LinearImage`.

---

## Task 1: Add `libraw-sys` to the workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/libraw-sys/Cargo.toml`
- Create: `crates/libraw-sys/src/lib.rs` (stub)

- [ ] **Step 1: Add `libraw-sys` to workspace members.**

Edit `Cargo.toml` and extend the `members` array:

```toml
[workspace]
resolver = "2"
members = [
    "crates/shoebox-common",
    "crates/shoebox-server",
    "crates/shoebox-client",
    "crates/libraw-sys",
]
```

- [ ] **Step 2: Add `bindgen` to workspace dependencies.**

Edit the `[workspace.dependencies]` block in the root `Cargo.toml` — add this line (alphabetical placement near other build-related deps is fine):

```toml
bindgen = "0.71"
```

- [ ] **Step 3: Create `crates/libraw-sys/Cargo.toml`.**

```toml
[package]
name = "libraw-sys"
version = "0.1.0"
edition.workspace = true
license.workspace = true
publish = false
links = "raw_r"

[lib]
doctest = false

[build-dependencies]
bindgen = { workspace = true }
pkg-config = "0.3"

[lints]
workspace = true
```

The `links = "raw_r"` line tells Cargo that this crate links against libraw's reentrant build (libraw_r). It prevents two crates in the same build graph from linking the same C library twice.

- [ ] **Step 4: Create stub `crates/libraw-sys/src/lib.rs`.**

```rust
//! Raw bindings to `libraw`. Generated by bindgen at build time.
//!
//! Safety: every function in this module is `unsafe extern "C"`. Higher
//! layers (`shoebox-pipeline::bake::decoder`) own the safe wrappers.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
```

- [ ] **Step 5: Verify workspace still compiles before bindgen kicks in.**

The crate has no `build.rs` yet, so `include!` will fail. Comment out the `include!` line for this step:

```rust
// include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
```

Run: `cargo check -p libraw-sys`
Expected: PASS (empty crate).

- [ ] **Step 6: Commit.**

```bash
git add Cargo.toml crates/libraw-sys/
git commit -m "feat(libraw-sys): bootstrap crate skeleton"
```

---

## Task 2: Bindgen + pkg-config in `build.rs`

**Files:**
- Create: `crates/libraw-sys/build.rs`
- Create: `crates/libraw-sys/wrapper.h`
- Modify: `crates/libraw-sys/src/lib.rs` (uncomment the `include!`)

- [ ] **Step 1: Create `crates/libraw-sys/wrapper.h`.**

```c
#include <libraw/libraw.h>
```

- [ ] **Step 2: Create `crates/libraw-sys/build.rs`.**

```rust
use std::env;
use std::path::PathBuf;

fn main() {
    // Locate libraw via pkg-config and link against the reentrant build.
    let libraw = pkg_config::Config::new()
        .atleast_version("0.21")
        .probe("libraw_r")
        .or_else(|_| {
            pkg_config::Config::new()
                .atleast_version("0.21")
                .probe("libraw")
        })
        .expect(
            "libraw >= 0.21 not found via pkg-config. Install with:\n  \
             Debian/Ubuntu: sudo apt install libraw-dev\n  \
             macOS:         brew install libraw\n  \
             Windows:       vcpkg install libraw",
        );

    // bindgen
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_function("libraw_.*")
        .allowlist_type("libraw_.*")
        .allowlist_type("LibRaw_.*")
        .allowlist_var("LIBRAW_.*")
        .derive_default(true)
        .derive_debug(false)
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

    for path in libraw.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    let bindings = builder.generate().expect("bindgen failed for libraw");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings.write_to_file(&out).expect("write bindings.rs");

    // pkg_config::Config::probe already emitted the cargo:rustc-link-lib
    // and cargo:rustc-link-search lines; nothing else to do.

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
}
```

- [ ] **Step 3: Uncomment the `include!` line in `crates/libraw-sys/src/lib.rs`.**

```rust
//! Raw bindings to `libraw`. Generated by bindgen at build time.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
```

- [ ] **Step 4: Verify bindgen succeeds.**

Run: `cargo build -p libraw-sys`
Expected: PASS. If pkg-config can't find libraw, the build prints the install hint from `build.rs` and exits non-zero — install libraw-dev and retry.

- [ ] **Step 5: Spot-check the generated bindings.**

Run: `nm -D $(find target -name "libraw_sys-*.rlib" 2>/dev/null | head -1) 2>/dev/null | head -20 || true`

This isn't an assertion — just a sanity check. The more reliable check is:

```bash
grep -c "fn libraw_init" target/debug/build/libraw-sys-*/out/bindings.rs
```

Expected: at least `1`.

- [ ] **Step 6: Commit.**

```bash
git add crates/libraw-sys/build.rs crates/libraw-sys/wrapper.h crates/libraw-sys/src/lib.rs
git commit -m "feat(libraw-sys): bindgen + pkg-config wiring"
```

---

## Task 3: libraw-sys smoke test

**Files:**
- Create: `crates/libraw-sys/tests/smoke.rs`

- [ ] **Step 1: Write the failing test.**

```rust
//! Smoke test: link against libraw, call libraw_version(), check it
//! returns a non-empty string. Proves that the bindings + link line
//! actually reach the system library.

use std::ffi::CStr;

#[test]
fn libraw_version_returns_non_empty_string() {
    // SAFETY: libraw_version returns a pointer to a static C string
    // owned by the library. Always non-null per libraw's contract.
    let version_ptr = unsafe { libraw_sys::libraw_version() };
    assert!(!version_ptr.is_null(), "libraw_version returned null");

    let version_str = unsafe { CStr::from_ptr(version_ptr) }
        .to_str()
        .expect("libraw version is not valid UTF-8");

    assert!(!version_str.is_empty(), "libraw version string is empty");
    assert!(
        version_str.chars().next().unwrap().is_ascii_digit(),
        "libraw version {version_str:?} does not start with a digit"
    );
}
```

- [ ] **Step 2: Run the test, expect PASS (bindings are already in place from Task 2).**

Run: `cargo test -p libraw-sys`
Expected: 1 passed.

- [ ] **Step 3: Commit.**

```bash
git add crates/libraw-sys/tests/smoke.rs
git commit -m "test(libraw-sys): smoke test against linked libraw"
```

---

## Task 4: Add `shoebox-pipeline` to the workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/shoebox-pipeline/Cargo.toml`
- Create: `crates/shoebox-pipeline/src/lib.rs` (stub)

- [ ] **Step 1: Add workspace deps used in this plan.**

Add to `[workspace.dependencies]` in the root `Cargo.toml`:

```toml
half = "2"
lcms2 = "6"
tempfile = "3"
```

- [ ] **Step 2: Add `shoebox-pipeline` to workspace members.**

```toml
[workspace]
resolver = "2"
members = [
    "crates/shoebox-common",
    "crates/shoebox-server",
    "crates/shoebox-client",
    "crates/libraw-sys",
    "crates/shoebox-pipeline",
]
```

- [ ] **Step 3: Create `crates/shoebox-pipeline/Cargo.toml`.**

```toml
[package]
name = "shoebox-pipeline"
version = "0.1.0"
edition.workspace = true
license.workspace = true
publish = false

[lib]
doctest = false

[dependencies]
libraw-sys = { path = "../libraw-sys" }
half = { workspace = true }
lcms2 = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 4: Create stub `crates/shoebox-pipeline/src/lib.rs`.**

```rust
//! shoebox RAW pipeline — decode, demosaic, color management.
//!
//! See `docs/superpowers/specs/2026-05-19-raw-pipeline-design.md`.

pub mod error;
pub mod types;

mod bake;

pub use bake::bake_linear;
pub use error::PipelineError;
pub use types::{
    BakeOptions, DemosaicAlgorithm, DevelopStage, LinearImage, OutputColorSpace, Rgba8Image,
    WorkingSpace,
};
```

The `mod bake` is referenced before it exists; that's intentional and resolved in Task 7. For now, comment those two lines:

```rust
// mod bake;
// pub use bake::bake_linear;
```

- [ ] **Step 5: Verify the crate compiles.**

Run: `cargo check -p shoebox-pipeline`
Expected: unresolved-module errors for `error` and `types` — those land in Tasks 5–6. For Task 4's verification only, comment both `pub mod` lines too:

```rust
// pub mod error;
// pub mod types;
// mod bake;
// pub use bake::bake_linear;
// pub use error::PipelineError;
// pub use types::{ ... };
```

Run again: `cargo check -p shoebox-pipeline`
Expected: PASS (empty crate).

- [ ] **Step 6: Commit.**

```bash
git add Cargo.toml crates/shoebox-pipeline/
git commit -m "feat(shoebox-pipeline): bootstrap crate skeleton"
```

---

## Task 5: Define `PipelineError`

**Files:**
- Create: `crates/shoebox-pipeline/src/error.rs`
- Modify: `crates/shoebox-pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing test.**

Append a test module at the bottom of `crates/shoebox-pipeline/src/error.rs` (you'll create the file in Step 2):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_error_display_includes_path_and_reason() {
        let err = PipelineError::Decode {
            path: std::path::PathBuf::from("/photos/IMG.PEF"),
            reason: "bad magic".into(),
        };
        let message = err.to_string();
        assert!(message.contains("/photos/IMG.PEF"), "missing path: {message}");
        assert!(message.contains("bad magic"), "missing reason: {message}");
    }

    #[test]
    fn from_io_error_preserves_kind() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let err: PipelineError = io.into();
        assert!(matches!(err, PipelineError::Io(_)));
    }
}
```

(The test will live alongside the type in Step 2.)

- [ ] **Step 2: Create `crates/shoebox-pipeline/src/error.rs`.**

```rust
//! Pipeline error type. One enum covers every failure mode the pipeline
//! can surface; see the spec §11 table for the contract.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("I/O error: {0}")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_error_display_includes_path_and_reason() {
        let err = PipelineError::Decode {
            path: PathBuf::from("/photos/IMG.PEF"),
            reason: "bad magic".into(),
        };
        let message = err.to_string();
        assert!(message.contains("/photos/IMG.PEF"), "missing path: {message}");
        assert!(message.contains("bad magic"), "missing reason: {message}");
    }

    #[test]
    fn from_io_error_preserves_kind() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let err: PipelineError = io.into();
        assert!(matches!(err, PipelineError::Io(_)));
    }
}
```

- [ ] **Step 3: Uncomment `pub mod error;` and `pub use error::PipelineError;` in `lib.rs`.**

```rust
pub mod error;
// pub mod types;
// mod bake;
// pub use bake::bake_linear;
pub use error::PipelineError;
// pub use types::{ ... };
```

- [ ] **Step 4: Run the tests.**

Run: `cargo test -p shoebox-pipeline error::`
Expected: 2 passed.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-pipeline/src/error.rs crates/shoebox-pipeline/src/lib.rs
git commit -m "feat(shoebox-pipeline): PipelineError"
```

---

## Task 6: Define core types

**Files:**
- Create: `crates/shoebox-pipeline/src/types.rs`
- Modify: `crates/shoebox-pipeline/src/lib.rs`

- [ ] **Step 1: Create `crates/shoebox-pipeline/src/types.rs`.**

```rust
//! Public pipeline value types. Kept POD (Plain Old Data) so they can
//! travel between threads and crate boundaries without surprise.

use half::f16;

/// One photo's demosaicked, color-managed image in a known linear
/// working space. Width × height × 3 half-floats, interleaved RGB.
///
/// The pixel layout matches the on-disk `.linear` cache file's data
/// block exactly (see Plan 2.2), so the cache reader can mmap directly
/// into this representation.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    pub working_space: WorkingSpace,
    pub pixels: Vec<f16>,
}

impl LinearImage {
    /// Number of pixels per channel.
    #[must_use]
    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Expected `pixels.len()` for these dimensions (`width * height * 3`).
    #[must_use]
    pub fn expected_buffer_len(&self) -> usize {
        self.pixel_count() * 3
    }
}

/// Options that govern a single bake.
#[derive(Debug, Clone)]
pub struct BakeOptions {
    pub demosaic: DemosaicAlgorithm,
    pub working_space: WorkingSpace,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            demosaic: DemosaicAlgorithm::Ahd,
            working_space: WorkingSpace::Rec2020,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingSpace {
    Rec2020,
}

impl WorkingSpace {
    /// libraw `output_color` value (see `libraw.h`).
    #[must_use]
    pub(crate) fn libraw_output_color(self) -> i32 {
        match self {
            WorkingSpace::Rec2020 => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemosaicAlgorithm {
    /// AHD — Adaptive Homogeneity-Directed. Default. libraw `user_qual = 3`.
    Ahd,
    /// Linear interpolation. Fastest, lowest quality. libraw `user_qual = 0`.
    Linear,
}

impl DemosaicAlgorithm {
    /// libraw `user_qual` value (see `libraw.h`).
    #[must_use]
    pub(crate) fn libraw_user_qual(self) -> i32 {
        match self {
            DemosaicAlgorithm::Ahd => 3,
            DemosaicAlgorithm::Linear => 0,
        }
    }
}

/// Output color space for the (future) Renderer. Only sRGB 8-bit in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputColorSpace {
    Srgb8,
}

/// 8-bit RGBA framebuffer produced by the Renderer (Plan 2.3).
#[derive(Debug, Clone)]
pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Develop-time stages applied by the Renderer between input and output
/// color transforms. Empty in this sub-project; Plan #4 adds variants.
///
/// `#[non_exhaustive]` so callers must use a wildcard arm and so we can
/// add variants in `#4` without breaking semver.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum DevelopStage {
    // No variants in #2; #4 fills this in.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bake_options_are_ahd_rec2020() {
        let opts = BakeOptions::default();
        assert_eq!(opts.demosaic, DemosaicAlgorithm::Ahd);
        assert_eq!(opts.working_space, WorkingSpace::Rec2020);
    }

    #[test]
    fn libraw_output_color_for_rec2020_is_8() {
        assert_eq!(WorkingSpace::Rec2020.libraw_output_color(), 8);
    }

    #[test]
    fn libraw_user_qual_mapping() {
        assert_eq!(DemosaicAlgorithm::Ahd.libraw_user_qual(), 3);
        assert_eq!(DemosaicAlgorithm::Linear.libraw_user_qual(), 0);
    }

    #[test]
    fn linear_image_expected_buffer_len() {
        let img = LinearImage {
            width: 10,
            height: 20,
            working_space: WorkingSpace::Rec2020,
            pixels: Vec::new(),
        };
        assert_eq!(img.pixel_count(), 200);
        assert_eq!(img.expected_buffer_len(), 600);
    }
}
```

- [ ] **Step 2: Uncomment `pub mod types;` and the type re-export in `lib.rs`.**

The full `lib.rs` now reads:

```rust
//! shoebox RAW pipeline — decode, demosaic, color management.
//!
//! See `docs/superpowers/specs/2026-05-19-raw-pipeline-design.md`.

pub mod error;
pub mod types;

// mod bake;
// pub use bake::bake_linear;

pub use error::PipelineError;
pub use types::{
    BakeOptions, DemosaicAlgorithm, DevelopStage, LinearImage, OutputColorSpace, Rgba8Image,
    WorkingSpace,
};
```

- [ ] **Step 3: Run the tests.**

Run: `cargo test -p shoebox-pipeline types::`
Expected: 4 passed.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-pipeline/src/types.rs crates/shoebox-pipeline/src/lib.rs
git commit -m "feat(shoebox-pipeline): core value types"
```

---

## Task 7: Implement `bake/decoder.rs` (libraw FFI wrapper)

**Files:**
- Create: `crates/shoebox-pipeline/src/bake/mod.rs` (stub)
- Create: `crates/shoebox-pipeline/src/bake/decoder.rs`
- Modify: `crates/shoebox-pipeline/src/lib.rs`

The decoder owns the libraw lifecycle. Its sole job: take a path + bake options, return `(width, height, Vec<u16>)` in linear Rec.2020 16-bit/channel. All `unsafe` lives here.

- [ ] **Step 1: Write the failing test.**

Append to `crates/shoebox-pipeline/src/bake/decoder.rs` (you'll create it in Step 2). For now, sketch it on paper — the test asserts `Decode` is returned when given a non-RAW file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BakeOptions;
    use std::io::Write;

    #[test]
    fn decode_non_raw_file_returns_decode_error() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"this is definitely not a RAW").expect("write");
        let path = tmp.path().to_path_buf();

        let result = decode(&path, &BakeOptions::default());

        match result {
            Err(crate::PipelineError::Decode { path: p, reason: _ }) => {
                assert_eq!(p, path);
            }
            other => panic!("expected Decode error, got: {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Create `crates/shoebox-pipeline/src/bake/mod.rs` as a placeholder.**

```rust
//! Bake phase — RAW file → in-memory `LinearImage`.

mod color_in;
mod decoder;

// `bake_linear` itself lands in Task 9.
```

The `color_in` module will be created in Task 8; until then, this won't compile. Comment that line for this task:

```rust
//! Bake phase — RAW file → in-memory `LinearImage`.

// mod color_in;
mod decoder;
```

- [ ] **Step 3: Create `crates/shoebox-pipeline/src/bake/decoder.rs`.**

```rust
//! libraw FFI wrapper. The single place in the crate where raw pointers
//! and `unsafe` live. Everything above this module sees a pure-Rust API.

use crate::types::BakeOptions;
use crate::PipelineError;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr::NonNull;

/// Result of a successful decode: dimensions + interleaved RGB samples
/// in **linear Rec.2020, 16 bits per channel** as emitted by libraw.
pub(crate) struct RawDecoded {
    pub width: u32,
    pub height: u32,
    pub samples_u16: Vec<u16>, // width * height * 3
}

/// Decode + demosaic `raw_path` according to `opts`. Synchronous and
/// blocking; libraw is a single non-thread-safe C call. Callers from
/// async contexts must wrap in `tokio::task::spawn_blocking`.
pub(crate) fn decode(raw_path: &Path, opts: &BakeOptions) -> Result<RawDecoded, PipelineError> {
    // `RawHandle` owns the libraw_data_t * and frees it on drop.
    let handle = RawHandle::new()?;
    let path_cstring = path_to_cstring(raw_path)?;

    // SAFETY: handle.ptr is valid; path_cstring outlives the call.
    let open_rc = unsafe { libraw_sys::libraw_open_file(handle.ptr.as_ptr(), path_cstring.as_ptr()) };
    map_libraw_rc(open_rc, raw_path, "open_file")?;

    // Configure params BEFORE unpack. Field access through raw pointer
    // is required because libraw_data_t::params is a nested C struct.
    unsafe {
        let data = handle.ptr.as_mut();
        data.params.output_color = opts.working_space.libraw_output_color();
        data.params.output_bps = 16;
        data.params.user_qual = opts.demosaic.libraw_user_qual();
        data.params.no_auto_bright = 1;
        data.params.use_camera_wb = 1;
        data.params.gamm[0] = 1.0;
        data.params.gamm[1] = 1.0;
    }

    // SAFETY: handle.ptr is valid; libraw owns the unpacked buffer.
    let unpack_rc = unsafe { libraw_sys::libraw_unpack(handle.ptr.as_ptr()) };
    map_libraw_rc(unpack_rc, raw_path, "unpack")?;

    // SAFETY: handle.ptr is valid; demosaic operates on the unpacked data.
    let process_rc = unsafe { libraw_sys::libraw_dcraw_process(handle.ptr.as_ptr()) };
    map_libraw_rc(process_rc, raw_path, "dcraw_process")?;

    let mut errcode: i32 = 0;
    // SAFETY: handle.ptr is valid; libraw allocates a fresh image
    // structure that we own via `ProcessedImage`.
    let mem_ptr = unsafe { libraw_sys::libraw_dcraw_make_mem_image(handle.ptr.as_ptr(), &mut errcode) };
    map_libraw_rc(errcode, raw_path, "dcraw_make_mem_image")?;
    let image = ProcessedImage::new(mem_ptr).ok_or_else(|| PipelineError::Decode {
        path: raw_path.to_path_buf(),
        reason: "dcraw_make_mem_image returned null".into(),
    })?;

    // Validate shape: 16-bit per channel, 3 colors (RGB).
    // SAFETY: image.ptr is non-null by construction; libraw guarantees
    // the header fields are populated after a successful call.
    let (width, height, colors, bits, data_size) = unsafe {
        let img = image.ptr.as_ref();
        (
            u32::from(img.width),
            u32::from(img.height),
            i32::from(img.colors),
            i32::from(img.bits),
            img.data_size as usize,
        )
    };
    if bits != 16 || colors != 3 {
        return Err(PipelineError::Decode {
            path: raw_path.to_path_buf(),
            reason: format!("unexpected libraw output: {bits}bit, {colors} colors"),
        });
    }
    let expected_data_size = (width as usize) * (height as usize) * 3 * 2;
    if data_size != expected_data_size {
        return Err(PipelineError::Decode {
            path: raw_path.to_path_buf(),
            reason: format!(
                "libraw data_size {data_size} != expected {expected_data_size} \
                 for {width}x{height}x3x16bit"
            ),
        });
    }

    // Copy the libraw output into an owned Vec<u16>. We can't keep the
    // libraw buffer alive past `image`'s Drop, so a copy is required.
    let mut samples_u16 = vec![0u16; expected_data_size / 2];
    // SAFETY: image.data[] is data_size bytes long; we copy that many
    // bytes into a u16 buffer of the matching size.
    unsafe {
        let src = image.ptr.as_ref().data.as_ptr() as *const u8;
        let dst = samples_u16.as_mut_ptr() as *mut u8;
        std::ptr::copy_nonoverlapping(src, dst, expected_data_size);
    }

    Ok(RawDecoded {
        width,
        height,
        samples_u16,
    })
}

/// RAII wrapper around `libraw_data_t *`.
struct RawHandle {
    ptr: NonNull<libraw_sys::libraw_data_t>,
}

impl RawHandle {
    fn new() -> Result<Self, PipelineError> {
        // SAFETY: libraw_init returns NULL on alloc failure only.
        let ptr = unsafe { libraw_sys::libraw_init(0) };
        let ptr = NonNull::new(ptr).ok_or_else(|| PipelineError::Internal(
            "libraw_init returned null".into(),
        ))?;
        Ok(Self { ptr })
    }
}

impl Drop for RawHandle {
    fn drop(&mut self) {
        // SAFETY: ptr was returned by libraw_init and not yet closed.
        unsafe { libraw_sys::libraw_close(self.ptr.as_ptr()) };
    }
}

/// RAII wrapper around `libraw_processed_image_t *`.
struct ProcessedImage {
    ptr: NonNull<libraw_sys::libraw_processed_image_t>,
}

impl ProcessedImage {
    fn new(raw: *mut libraw_sys::libraw_processed_image_t) -> Option<Self> {
        NonNull::new(raw).map(|ptr| Self { ptr })
    }
}

impl Drop for ProcessedImage {
    fn drop(&mut self) {
        // SAFETY: ptr was returned by libraw_dcraw_make_mem_image.
        unsafe { libraw_sys::libraw_dcraw_clear_mem(self.ptr.as_ptr()) };
    }
}

/// Convert a libraw integer return code to a `PipelineError::Decode`
/// (zero is success per libraw's convention).
fn map_libraw_rc(rc: i32, path: &Path, stage: &str) -> Result<(), PipelineError> {
    if rc == 0 {
        return Ok(());
    }
    // SAFETY: libraw_strerror returns a pointer to a static C string.
    let msg_ptr = unsafe { libraw_sys::libraw_strerror(rc) };
    let msg = if msg_ptr.is_null() {
        format!("libraw rc {rc}")
    } else {
        // SAFETY: msg_ptr is non-null and points at a NUL-terminated
        // string owned by libraw.
        unsafe { CStr::from_ptr(msg_ptr) }
            .to_string_lossy()
            .into_owned()
    };
    Err(PipelineError::Decode {
        path: path.to_path_buf(),
        reason: format!("{stage}: {msg} (rc={rc})"),
    })
}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> Result<CString, PipelineError> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes()).map_err(|nul_error| {
        PipelineError::Internal(format!("path contains NUL byte: {nul_error}"))
    })
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> Result<CString, PipelineError> {
    let utf8 = path.to_str().ok_or_else(|| {
        PipelineError::Internal(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    CString::new(utf8).map_err(|nul_error| {
        PipelineError::Internal(format!("path contains NUL byte: {nul_error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn decode_non_raw_file_returns_decode_error() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"this is definitely not a RAW").expect("write");
        let path = tmp.path().to_path_buf();

        let result = decode(&path, &crate::types::BakeOptions::default());

        match result {
            Err(crate::PipelineError::Decode { path: p, reason: _ }) => {
                assert_eq!(p, path);
            }
            other => panic!("expected Decode error, got: {other:?}"),
        }
    }
}
```

- [ ] **Step 4: Uncomment `mod bake;` in `lib.rs`.**

```rust
pub mod error;
pub mod types;

mod bake;

// pub use bake::bake_linear;   // still commented; Task 9
```

- [ ] **Step 5: Run the failing test (it should now PASS).**

Run: `cargo test -p shoebox-pipeline bake::decoder::`
Expected: 1 passed.

If you see a link-time error mentioning `unresolved external symbol` for libraw, the `links = "raw_r"` in `libraw-sys/Cargo.toml` isn't taking effect — recheck Task 1 Step 3.

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-pipeline/src/bake/
git add crates/shoebox-pipeline/src/lib.rs
git commit -m "feat(shoebox-pipeline): bake/decoder.rs libraw FFI wrapper"
```

---

## Task 8: Implement `bake/color_in.rs` (u16 → f16)

**Files:**
- Create: `crates/shoebox-pipeline/src/bake/color_in.rs`
- Modify: `crates/shoebox-pipeline/src/bake/mod.rs`

In v1, libraw already emits linear Rec.2020 (`output_color = 8`) so `color_in` is a pure type/numeric conversion. This module exists so that a future plan can swap in an lcms2-driven camera-ICC transform without rewiring callers.

- [ ] **Step 1: Write the failing test.**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_max_maps_to_f16_one() {
        let pixels = u16_rec2020_to_f16_rec2020(&[u16::MAX, u16::MAX, u16::MAX]);
        assert_eq!(pixels.len(), 3);
        for (i, sample) in pixels.iter().enumerate() {
            let value = sample.to_f32();
            assert!(
                (value - 1.0).abs() < 1.0 / 65535.0,
                "channel {i}: expected ~1.0, got {value}"
            );
        }
    }

    #[test]
    fn u16_zero_maps_to_f16_zero() {
        let pixels = u16_rec2020_to_f16_rec2020(&[0, 0, 0]);
        for sample in &pixels {
            assert_eq!(sample.to_f32(), 0.0);
        }
    }

    #[test]
    fn buffer_length_preserved() {
        let pixels = u16_rec2020_to_f16_rec2020(&[100, 200, 300, 400, 500, 600]);
        assert_eq!(pixels.len(), 6);
    }
}
```

- [ ] **Step 2: Create `crates/shoebox-pipeline/src/bake/color_in.rs`.**

```rust
//! Color input transform. Today: pure u16 → f16 numeric conversion
//! (libraw already produces linear Rec.2020 16-bit/channel).
//!
//! Future (post-v1): when `BakeOptions::camera_profile_icc` returns,
//! this module routes through `lcms2` to apply a camera-ICC transform.

use half::f16;

/// Convert libraw's 16-bit linear Rec.2020 output to half-float linear
/// Rec.2020. Maps `0 → 0.0` and `u16::MAX → 1.0` linearly.
pub(crate) fn u16_rec2020_to_f16_rec2020(samples_u16: &[u16]) -> Vec<f16> {
    const INV_U16_MAX: f32 = 1.0 / 65535.0;
    samples_u16
        .iter()
        .map(|&v| f16::from_f32(f32::from(v) * INV_U16_MAX))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_max_maps_to_f16_one() {
        let pixels = u16_rec2020_to_f16_rec2020(&[u16::MAX, u16::MAX, u16::MAX]);
        assert_eq!(pixels.len(), 3);
        for (i, sample) in pixels.iter().enumerate() {
            let value = sample.to_f32();
            assert!(
                (value - 1.0).abs() < 1.0 / 65535.0,
                "channel {i}: expected ~1.0, got {value}"
            );
        }
    }

    #[test]
    fn u16_zero_maps_to_f16_zero() {
        let pixels = u16_rec2020_to_f16_rec2020(&[0, 0, 0]);
        for sample in &pixels {
            assert_eq!(sample.to_f32(), 0.0);
        }
    }

    #[test]
    fn buffer_length_preserved() {
        let pixels = u16_rec2020_to_f16_rec2020(&[100, 200, 300, 400, 500, 600]);
        assert_eq!(pixels.len(), 6);
    }
}
```

- [ ] **Step 3: Uncomment `mod color_in;` in `crates/shoebox-pipeline/src/bake/mod.rs`.**

```rust
//! Bake phase — RAW file → in-memory `LinearImage`.

mod color_in;
mod decoder;
```

- [ ] **Step 4: Run the tests.**

Run: `cargo test -p shoebox-pipeline bake::color_in::`
Expected: 3 passed.

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-pipeline/src/bake/color_in.rs crates/shoebox-pipeline/src/bake/mod.rs
git commit -m "feat(shoebox-pipeline): bake/color_in.rs u16→f16 transform"
```

---

## Task 9: Wire `bake_linear()` entry point

**Files:**
- Modify: `crates/shoebox-pipeline/src/bake/mod.rs`
- Modify: `crates/shoebox-pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing test.**

Add a unit-test module at the bottom of `bake/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn bake_linear_on_non_raw_returns_decode_error() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"not a RAW").expect("write");
        let result = bake_linear(tmp.path(), &BakeOptions::default());
        assert!(matches!(result, Err(PipelineError::Decode { .. })));
    }
}
```

- [ ] **Step 2: Replace `crates/shoebox-pipeline/src/bake/mod.rs` with:**

```rust
//! Bake phase — RAW file → in-memory `LinearImage`.

mod color_in;
mod decoder;

use std::panic::AssertUnwindSafe;
use std::path::Path;

use crate::types::{BakeOptions, LinearImage};
use crate::PipelineError;

/// Decode, demosaic, and color-correct `raw_path` into a `LinearImage`
/// in the working space declared by `opts`. Synchronous and blocking;
/// libraw is a non-thread-safe C library, so async callers must wrap
/// in `tokio::task::spawn_blocking`.
///
/// # Errors
///
/// Returns `PipelineError::Decode` if libraw rejects the file.
/// Returns `PipelineError::Internal` if a libraw call panics inside FFI.
pub fn bake_linear(raw_path: &Path, opts: &BakeOptions) -> Result<LinearImage, PipelineError> {
    let raw_path_owned = raw_path.to_path_buf();
    let opts_owned = opts.clone();

    // catch_unwind around the FFI boundary: libraw is C and can panic
    // on malformed inputs despite our best validation. The worker
    // thread must survive a panic so the cache layer (Plan 2.2) can
    // serve subsequent requests.
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let decoded = decoder::decode(&raw_path_owned, &opts_owned)?;
        let pixels = color_in::u16_rec2020_to_f16_rec2020(&decoded.samples_u16);
        Ok::<LinearImage, PipelineError>(LinearImage {
            width: decoded.width,
            height: decoded.height,
            working_space: opts_owned.working_space,
            pixels,
        })
    }));

    match result {
        Ok(inner) => inner,
        Err(_) => Err(PipelineError::Decode {
            path: raw_path.to_path_buf(),
            reason: "FFI panic in libraw".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn bake_linear_on_non_raw_returns_decode_error() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"not a RAW").expect("write");
        let result = bake_linear(tmp.path(), &BakeOptions::default());
        assert!(matches!(result, Err(PipelineError::Decode { .. })));
    }
}
```

- [ ] **Step 3: Uncomment the `bake_linear` re-export in `lib.rs`.**

Final `lib.rs`:

```rust
//! shoebox RAW pipeline — decode, demosaic, color management.
//!
//! See `docs/superpowers/specs/2026-05-19-raw-pipeline-design.md`.

pub mod error;
pub mod types;

mod bake;

pub use bake::bake_linear;
pub use error::PipelineError;
pub use types::{
    BakeOptions, DemosaicAlgorithm, DevelopStage, LinearImage, OutputColorSpace, Rgba8Image,
    WorkingSpace,
};
```

- [ ] **Step 4: Run the tests.**

Run: `cargo test -p shoebox-pipeline`
Expected: all tests pass (4 from types, 2 from error, 1 from decoder, 3 from color_in, 1 from bake = 11 total).

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-pipeline/src/bake/mod.rs crates/shoebox-pipeline/src/lib.rs
git commit -m "feat(shoebox-pipeline): bake_linear() entry point"
```

---

## Task 10: Add a real RAW fixture

**Files:**
- Create: `crates/shoebox-pipeline/tests/fixtures/sample.dng`
- Create: `crates/shoebox-pipeline/tests/fixtures/README.md`

The integration test in Task 11 needs an actual RAW file. We use a small, freely-licensed DNG sample. DNG is chosen because: (a) it's Adobe's open format and libraw supports it without camera-specific quirks, (b) small samples are widely available, and (c) sub-project #1's photo formats include DNG.

- [ ] **Step 1: Acquire a fixture file.**

Source: <https://raw.pixls.us/> hosts a curated set of CC-0 RAW samples. Pick a **small** DNG (≤ 5 MB). Suggested choice: a Leica Q (Typ 116) DNG, typically ~3.5 MB. If `raw.pixls.us` is unreachable, alternative sources are `rawsamples.ch` (CC-BY) and `imaging.tldr.org`.

Save the file as `crates/shoebox-pipeline/tests/fixtures/sample.dng`.

If the fixture exceeds 5 MB after download, do not commit it — instead, document a known small alternative in `fixtures/README.md` and skip the e2e test on developer machines without it (the test gates on `sample.dng` existence; see Task 11).

- [ ] **Step 2: Create `crates/shoebox-pipeline/tests/fixtures/README.md`.**

```markdown
# Test fixtures

## `sample.dng`

A small (~3 MB) CC-0 / public-domain DNG used by `bake_e2e.rs` to
exercise the full `bake_linear()` path against real libraw output.

Origin: <https://raw.pixls.us/> (CC-0).

If you replace this fixture, update the expected `width` and `height`
in `bake_e2e.rs` accordingly.
```

- [ ] **Step 3: Verify the file is committable in size.**

Run: `ls -lh crates/shoebox-pipeline/tests/fixtures/sample.dng`
Expected: under 5 MB. If larger, source a smaller one.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-pipeline/tests/fixtures/
git commit -m "test(shoebox-pipeline): add CC-0 DNG fixture for bake e2e"
```

---

## Task 11: End-to-end `bake_linear()` integration test

**Files:**
- Create: `crates/shoebox-pipeline/tests/bake_e2e.rs`

- [ ] **Step 1: Write the test.**

```rust
//! End-to-end: run bake_linear() against a real DNG, check that the
//! result has plausible dimensions, a fully-populated buffer, and
//! non-degenerate pixel content (not all zero, not all clipped).

use shoebox_pipeline::{bake_linear, BakeOptions, WorkingSpace};

const FIXTURE: &str = "tests/fixtures/sample.dng";

fn fixture_path() -> std::path::PathBuf {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(crate_dir).join(FIXTURE)
}

#[test]
fn bake_linear_real_dng_produces_plausible_linear_image() {
    let path = fixture_path();
    if !path.exists() {
        eprintln!(
            "SKIPPED: fixture {} not present (see tests/fixtures/README.md)",
            path.display()
        );
        return;
    }

    let img = bake_linear(&path, &BakeOptions::default()).expect("bake_linear");

    assert!(img.width >= 64, "width too small: {}", img.width);
    assert!(img.height >= 64, "height too small: {}", img.height);
    assert_eq!(img.working_space, WorkingSpace::Rec2020);
    assert_eq!(img.pixels.len(), img.expected_buffer_len());

    // Sanity-check: at least 1% of samples are non-zero, at least 1%
    // are not at the max value. Catches "all black" / "all white".
    let total = img.pixels.len();
    let zero_count = img.pixels.iter().filter(|p| p.to_f32() == 0.0).count();
    let max_count = img.pixels.iter().filter(|p| p.to_f32() >= 0.99).count();
    assert!(
        zero_count < total - total / 100,
        "image is {}% zero — fixture or decoder broken",
        100 * zero_count / total
    );
    assert!(
        max_count < total - total / 100,
        "image is {}% clipped — fixture or decoder broken",
        100 * max_count / total
    );
}
```

- [ ] **Step 2: Run the test.**

Run: `cargo test -p shoebox-pipeline --test bake_e2e`
Expected: 1 passed (or 1 passed with a SKIPPED message if `sample.dng` is absent — both outcomes are green).

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-pipeline/tests/bake_e2e.rs
git commit -m "test(shoebox-pipeline): bake_linear() e2e against DNG fixture"
```

---

## Task 12: Wire `shoebox-pipeline` into `shoebox-client`

**Files:**
- Modify: `crates/shoebox-client/Cargo.toml`

The client doesn't *use* the pipeline yet (UI integration is Plan 2.3 + sub-project #4). But adding the dep now catches workspace-resolution breakage early and proves the dep graph is sound.

- [ ] **Step 1: Add the dependency.**

Edit `crates/shoebox-client/Cargo.toml`. In the `[dependencies]` table, append:

```toml
shoebox-pipeline = { path = "../shoebox-pipeline" }
```

- [ ] **Step 2: Verify the workspace still builds.**

Run: `cargo build --workspace`
Expected: PASS, with one new warning (`unused_imports` or `unused_dependency`) is acceptable — we'll silence it when the client first uses the pipeline. If the warning is a build-blocker per the workspace lint config, gate the dep with an `_ = shoebox_pipeline::WorkingSpace::Rec2020;` in `crates/shoebox-client/src/main.rs` at the start of `main()` to anchor it. Prefer keeping the dep purely as a Cargo entry if the lint allows it.

- [ ] **Step 3: Run the full test suite.**

Run: `cargo test --workspace`
Expected: all existing tests + new pipeline tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-client/Cargo.toml
git commit -m "build(client): depend on shoebox-pipeline (unused in this plan)"
```

---

## Task 13: Document pipeline dev-deps

**Files:**
- Create: `scripts/install-pipeline-deps.sh`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Create `scripts/install-pipeline-deps.sh`.**

```bash
#!/usr/bin/env bash
# Install libraw and lcms2 development headers for shoebox-pipeline.
# Idempotent: re-runs are safe.

set -euo pipefail

case "$(uname -s)" in
    Linux)
        if command -v apt-get >/dev/null 2>&1; then
            sudo apt-get update
            sudo apt-get install -y libraw-dev liblcms2-dev pkg-config
        elif command -v dnf >/dev/null 2>&1; then
            sudo dnf install -y LibRaw-devel lcms2-devel pkgconf-pkg-config
        elif command -v pacman >/dev/null 2>&1; then
            sudo pacman -S --noconfirm libraw lcms2 pkgconf
        else
            echo "No supported package manager found on this Linux distro." >&2
            echo "Install libraw (>=0.21) and lcms2 dev headers manually." >&2
            exit 1
        fi
        ;;
    Darwin)
        if ! command -v brew >/dev/null 2>&1; then
            echo "Homebrew not found. Install from https://brew.sh and re-run." >&2
            exit 1
        fi
        brew install libraw little-cms2 pkg-config
        ;;
    MINGW*|MSYS*|CYGWIN*)
        echo "Windows: install via vcpkg:" >&2
        echo "  vcpkg install libraw lcms" >&2
        echo "Then set VCPKG_ROOT before running cargo." >&2
        exit 1
        ;;
    *)
        echo "Unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac

echo
echo "Verifying installation..."
pkg-config --modversion libraw_r || pkg-config --modversion libraw
pkg-config --modversion lcms2
echo "OK."
```

Make it executable:

```bash
chmod +x scripts/install-pipeline-deps.sh
```

- [ ] **Step 2: Add a "Pipeline dependencies" section to `CLAUDE.md`.**

Find the existing "Run locally" subsection and insert this block immediately before it:

```markdown
## Pipeline dependencies (sub-project #2 onward)

`shoebox-pipeline` and `libraw-sys` dynamically link against libraw
(>= 0.21) and lcms2. First-time setup:

```bash
./scripts/install-pipeline-deps.sh
```

On Linux this installs `libraw-dev` + `liblcms2-dev`; on macOS,
`brew install libraw little-cms2`; Windows currently needs manual
`vcpkg install libraw lcms` (CI integration arrives with Plan 2.4).
```

- [ ] **Step 3: Update the "Implementation status" section in `CLAUDE.md`.**

Find the existing implementation-status block and add a new bullet under it:

```markdown
- `crates/shoebox-pipeline` + `crates/libraw-sys` — Plan 2.1:
  - libraw-sys: bindgen wrapper, dynamically linked against system libraw (>= 0.21) via pkg-config.
  - shoebox-pipeline: `bake_linear()` decodes + demosaics a RAW through libraw with `output_color = 8` (linear Rec.2020), `output_bps = 16`, `user_qual = AHD`. Output is a `LinearImage { width, height, pixels: Vec<f16> }`.
  - Spec deviation: `BakeOptions::camera_profile_icc` is omitted in v1 (libraw's built-in matrix is used). lcms2 is a workspace dep but not invoked from the bake path; it lands in Plan 2.3 for render-time color transforms.
  - No cache, no GPU rendering, no UI integration yet — those are Plans 2.2 / 2.3 / sub-project #4.
```

- [ ] **Step 4: Verify the script runs cleanly on the current dev machine.**

Run: `./scripts/install-pipeline-deps.sh`
Expected: PASS (libs already installed from earlier in this plan).

- [ ] **Step 5: Commit.**

```bash
git add scripts/install-pipeline-deps.sh CLAUDE.md
git commit -m "docs: document pipeline dev-deps and Plan 2.1 status"
```

---

## Final verification

After Task 13:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass. The `bake_e2e` test should report 1 passed (not skipped) assuming the DNG fixture was successfully committed in Task 10.

## Out-of-scope (deferred to later plans)

- `.linear` file format and `LinearCache` — Plan 2.2.
- `Renderer` + wgpu plumbing + `OutputColorSpace::Srgb8` shader — Plan 2.3.
- CI / release packaging (libraw + lcms2 bundling, cross-compile setup, Windows vcpkg) — Plan 2.4.
- Develop-stage math (exposure, WB, curves, masks) — sub-project #4.
- `BakeOptions::camera_profile_icc` and lcms2-driven camera-ICC transforms — backlog (post-Plan 2.4).
- Render quality A/B vs. Lightroom/Capture One — beyond v1.
