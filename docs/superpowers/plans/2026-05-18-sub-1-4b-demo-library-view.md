# Plan 1.4b — Demo Library View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the placeholder Library screen with a three-pane Lightroom-shaped demo (folder tree, photo grid with thumbnails, EXIF + edit detail panel), wire rate / keyword / virtual-copy actions through the local replica, and surface develop-lock state from the server's `/locks/:id` endpoints.

**Architecture:** Extend the existing Plan 1.4 Iced `Application` with a new `library_view` module tree. All reads go through `Replica::conn()`; mutations write to the local libSQL replica (which propagates to the server via the existing WAL push). Develop-lock mutations go through the mTLS `/locks/:id` HTTP endpoints because the server enforces policy. A new `ThumbCache` (in-memory LRU + on-disk JPEG cache, mTLS HTTP to `/thumbs/<hash>`) feeds the photo grid. A 5 s lock-status polling tick keeps the detail panel's lock banner in sync; a 5 min heartbeat tick keeps held locks alive.

**Tech Stack:** Rust 2021 (MSRV 1.85), Iced 0.13 (`wgpu` + `tokio` + `advanced`), libsql 0.6 (embedded replica), reqwest 0.12 (mTLS via existing `mtls_http.rs`), image 0.25 (JPEG decode for the cache), `lru = "0.12"` (in-memory LRU), `tokio::sync::OnceCell` (in-flight dedup), `parking_lot::Mutex` (cache state), `directories = "5"` (per-OS cache dir).

**Spec:** `docs/superpowers/specs/2026-05-18-sub-1-4b-demo-library-view-design.md` (committed in `58707b6`).

---

## Schema reference

All IDs in the catalog are TEXT (UUIDs / hex), not integers. Tasks must use `String` (Rust) ↔ `TEXT` (SQL) throughout.

- `photos.id` is the BLAKE3 hash of the RAW file (hex). This same string is the `/thumbs/<hash>` URL path segment — no separate hash column exists.
- `folders` is hierarchical via `parent_id`. Root folders have `parent_id IS NULL`.
- `variants` numbering uses `variant_index` (INTEGER, 0 = master, >0 = virtual copy). UNIQUE on `(photo_id, variant_index)`. Optional `name` column for user-facing labels.
- `variant_user_state` is keyed by `(variant_id, user_id)` and holds `rating`, `flag`, `color_label`. UPSERT pattern: `ON CONFLICT(variant_id, user_id) DO UPDATE SET …`.
- `keywords` is hierarchical with `UNIQUE (parent_id, name)`. For 1.4b we only create root-level keywords (`parent_id = NULL`).
- `photo_keywords` is shared (catalog-wide), keyed by `(photo_id, keyword_id)`.
- `develop_locks` columns: `variant_id` (PK), `session_id`, `user_id`, `acquired_at`, `expires_at`, `takeover_requested_by` (nullable), `takeover_requested_at` (nullable).

## Server endpoint reference

- `POST   /locks/:variant_id`            — acquire (200 + holder info; 409 if held)
- `PUT    /locks/:variant_id`            — heartbeat (200 if extended; 404 if not held by you)
- `DELETE /locks/:variant_id`            — release (204; 404 if not held by you)
- `POST   /locks/:variant_id/takeover`   — request takeover (200; 409 if pending or free)
- `GET    /thumbs/:hash`                 — JPEG bytes (200; 404 if no thumb yet)

## File structure

| Path | New / Modify | Responsibility |
|---|---|---|
| `crates/shoebox-client/src/thumb_cache.rs` | New | In-memory LRU + on-disk JPEG cache; mTLS HTTP fetch; dedup via `tokio::sync::OnceCell`. |
| `crates/shoebox-client/src/library_state.rs` | New | Domain types (`FolderRow`, `GridCell`, `DetailLoaded`, `LockStatus`, `LibraryViewState`); async DB-query helpers; mutation helpers; pure keyboard-navigation helpers. |
| `crates/shoebox-client/src/screens/library_view/mod.rs` | New | Three-pane `view(&AppState) -> Element<Message>`; keyboard subscription. |
| `crates/shoebox-client/src/screens/library_view/folder_tree.rs` | New | Pure left-pane view. |
| `crates/shoebox-client/src/screens/library_view/photo_grid.rs` | New | Pure center-pane view. |
| `crates/shoebox-client/src/screens/library_view/detail_panel.rs` | New | Pure right-pane view (EXIF + rating + keywords + virtual-copy button + lock banner). |
| `crates/shoebox-client/src/screens/library.rs` | Modify (trim) | Keeps `LibraryStats` + `load_stats` (still used at startup); the old `view()` is removed. |
| `crates/shoebox-client/src/screens/mod.rs` | Modify | Declares `library_view`; adds ~24 new `Message` variants. |
| `crates/shoebox-client/src/app_state.rs` | Modify | Embeds `LibraryViewState` plus `Option<ThumbCache>`. |
| `crates/shoebox-client/src/main.rs` | Modify | Routes `Screen::Library` to `library_view::view`; adds new message handlers + subscription tickers. |
| `crates/shoebox-client/Cargo.toml` | Modify | Adds `lru` dep. |
| `Cargo.toml` (workspace) | Modify | Adds `lru = "0.12"` to `[workspace.dependencies]`. |
| `crates/shoebox-client/tests/thumb_cache_e2e.rs` | New | Standalone test (no sqld) — verifies in-flight dedup against a counted axum endpoint. |
| `crates/shoebox-client/tests/library_view_e2e.rs` | New | Real-server e2e — drives folder/grid/detail loaders + editing actions. |
| `crates/shoebox-client/tests/library_lock_e2e.rs` | New | Two-client e2e — exercises all four lock states. |
| `CLAUDE.md` | Modify | Updates sub-project #1 status row; notes Plan 1.4b complete. |

---

## Task 1: Add `lru` dependency

**Files:**
- Modify: `Cargo.toml` (workspace root, `[workspace.dependencies]` section, end)
- Modify: `crates/shoebox-client/Cargo.toml` (`[dependencies]` section, end of main block)

- [ ] **Step 1: Add to workspace deps**

Edit `Cargo.toml` (workspace root). Add this line to `[workspace.dependencies]`, placed alphabetically after `keyring`:

```toml
lru = "0.12"
```

- [ ] **Step 2: Pull into shoebox-client**

Edit `crates/shoebox-client/Cargo.toml`. Add to the `[dependencies]` block (after `directories`):

```toml
lru = { workspace = true }
image = { workspace = true }
```

(`image` is already a workspace dep; it joins the client's deps here because `ThumbCache` returns `Arc<image::DynamicImage>`.)

- [ ] **Step 3: Verify workspace compiles**

Run: `cargo check -p shoebox-client`
Expected: clean compile.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/shoebox-client/Cargo.toml
git -c commit.gpgsign=false commit -m "build(client): add lru + image deps for thumb cache (Plan 1.4b Task 1)"
```

---

## Task 2: `ThumbCache` skeleton — types, constructor, disk path resolution

**Files:**
- Create: `crates/shoebox-client/src/thumb_cache.rs`
- Modify: `crates/shoebox-client/src/lib.rs` (add `pub mod thumb_cache;`)

- [ ] **Step 1: Write failing test for path resolution**

Create `crates/shoebox-client/src/thumb_cache.rs`:

```rust
//! On-disk + in-memory cache for thumbnails fetched from
//! `<server>/thumbs/<hash>`. Reads share a single in-flight
//! `tokio::sync::OnceCell` per hash so concurrent `get`s for the same
//! key do one HTTP round-trip.

use anyhow::Result;
use image::DynamicImage;
use lru::LruCache;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

const MEMORY_CAPACITY: usize = 1024;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ThumbError {
    #[error("network: {0}")]
    Http(String),
    #[error("io: {0}")]
    Io(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("server returned status {0}")]
    Status(u16),
}

pub type CachedResult = std::result::Result<Arc<DynamicImage>, ThumbError>;

struct State {
    memory: LruCache<String, Arc<DynamicImage>>,
    in_flight: HashMap<String, Arc<OnceCell<CachedResult>>>,
}

#[derive(Clone)]
pub struct ThumbCache {
    state: Arc<Mutex<State>>,
    http_client: reqwest::Client,
    server_base_url: String,
    disk_dir: PathBuf,
}

impl ThumbCache {
    /// Construct a cache. `disk_dir` is created if missing.
    pub fn new(
        http_client: reqwest::Client,
        server_base_url: String,
        disk_dir: PathBuf,
    ) -> Result<Self> {
        std::fs::create_dir_all(&disk_dir)?;
        let capacity =
            NonZeroUsize::new(MEMORY_CAPACITY).expect("MEMORY_CAPACITY is non-zero");
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                memory: LruCache::new(capacity),
                in_flight: HashMap::new(),
            })),
            http_client,
            server_base_url: server_base_url.trim_end_matches('/').to_string(),
            disk_dir,
        })
    }

    /// Path on disk where the JPEG for `hash` is (or would be) cached.
    #[must_use]
    pub fn disk_path_for(&self, hash: &str) -> PathBuf {
        self.disk_dir.join(format!("{hash}.jpg"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn build_cache(disk_dir: PathBuf) -> ThumbCache {
        ThumbCache::new(reqwest::Client::new(), "https://server.invalid".into(), disk_dir)
            .expect("cache constructs")
    }

    #[test]
    fn disk_path_includes_hash_and_jpg_extension() {
        let tmp = TempDir::new().unwrap();
        let cache = build_cache(tmp.path().to_path_buf());
        let path = cache.disk_path_for("deadbeef");
        assert_eq!(path.file_name().unwrap(), "deadbeef.jpg");
        assert_eq!(path.parent().unwrap(), tmp.path());
    }

    #[test]
    fn new_creates_disk_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested/dir");
        let _cache = build_cache(nested.clone());
        assert!(nested.is_dir());
    }
}
```

Now register the module. Edit `crates/shoebox-client/src/lib.rs` and add (sorted alphabetically with existing modules):

```rust
pub mod thumb_cache;
```

- [ ] **Step 2: Run tests — both should pass**

Run: `cargo test -p shoebox-client --lib thumb_cache`
Expected: `disk_path_includes_hash_and_jpg_extension` PASS, `new_creates_disk_dir` PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/thumb_cache.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): ThumbCache skeleton with disk-path resolution (Plan 1.4b Task 2)"
```

---

## Task 3: `ThumbCache::get` — HTTP fetch, in-memory cache, in-flight dedup

**Files:**
- Modify: `crates/shoebox-client/src/thumb_cache.rs`

- [ ] **Step 1: Add the `get` method and its helpers**

Append the following inside the `impl ThumbCache { … }` block (before the `#[cfg(test)]`):

```rust
    /// Fetch a thumbnail. Returns an in-memory hit instantly; otherwise
    /// resolves a single in-flight load and caches the result.
    pub async fn get(&self, hash: &str) -> CachedResult {
        if let Some(image) = self.state.lock().memory.get(hash).cloned() {
            return Ok(image);
        }

        let cell = {
            let mut state = self.state.lock();
            // Re-check inside the lock in case another caller filled it.
            if let Some(image) = state.memory.get(hash).cloned() {
                return Ok(image);
            }
            state
                .in_flight
                .entry(hash.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cell
            .get_or_init(|| async { self.load_uncached(hash).await })
            .await
            .clone();

        {
            let mut state = self.state.lock();
            state.in_flight.remove(hash);
            if let Ok(image) = &result {
                state.memory.put(hash.to_string(), image.clone());
            }
        }
        result
    }

    async fn load_uncached(&self, hash: &str) -> CachedResult {
        // Disk hit first.
        let disk_path = self.disk_path_for(hash);
        if disk_path.exists() {
            return match tokio::fs::read(&disk_path).await {
                Ok(bytes) => decode_jpeg(&bytes),
                Err(error) => Err(ThumbError::Io(error.to_string())),
            };
        }
        // Cold miss — HTTP.
        let url = format!("{}/thumbs/{hash}", self.server_base_url);
        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|error| ThumbError::Http(error.to_string()))?;
        if !response.status().is_success() {
            return Err(ThumbError::Status(response.status().as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| ThumbError::Http(error.to_string()))?;
        // Persist to disk best-effort (don't fail the read if write fails).
        if let Err(error) = tokio::fs::write(&disk_path, &bytes).await {
            tracing::warn!(%hash, "failed to persist thumbnail to disk: {error}");
        }
        decode_jpeg(&bytes)
    }
}

fn decode_jpeg(bytes: &[u8]) -> CachedResult {
    let cursor = std::io::Cursor::new(bytes);
    let reader = image::ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|error| ThumbError::Decode(error.to_string()))?;
    let image = reader
        .decode()
        .map_err(|error| ThumbError::Decode(error.to_string()))?;
    Ok(Arc::new(image))
}
```

(Note the trailing closing `}` for `impl ThumbCache` — the original had it; merge carefully.)

- [ ] **Step 2: Add unit test for in-flight dedup**

Append to the `#[cfg(test)] mod tests` block in `crates/shoebox-client/src/thumb_cache.rs`:

```rust
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    fn tiny_jpeg() -> Vec<u8> {
        // A 1x1 white JPEG generated via the `image` crate.
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 255, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    async fn spawn_counting_server(jpeg: Vec<u8>) -> (String, StdArc<AtomicUsize>) {
        use axum::{routing::get, Router};
        let counter = StdArc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let jpeg_clone = jpeg.clone();
        let app = Router::new().route(
            "/thumbs/:hash",
            get(move || {
                let counter = counter_clone.clone();
                let jpeg = jpeg_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (
                        [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                        jpeg,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), counter)
    }

    #[tokio::test]
    async fn concurrent_gets_for_same_hash_do_one_http_request() {
        let tmp = TempDir::new().unwrap();
        let (url, counter) = spawn_counting_server(tiny_jpeg()).await;
        let cache =
            ThumbCache::new(reqwest::Client::new(), url, tmp.path().to_path_buf()).unwrap();
        let cache_a = cache.clone();
        let cache_b = cache.clone();
        let (result_a, result_b) =
            tokio::join!(cache_a.get("hash1"), cache_b.get("hash1"));
        assert!(result_a.is_ok());
        assert!(result_b.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_call_after_first_succeeds_is_memory_hit() {
        let tmp = TempDir::new().unwrap();
        let (url, counter) = spawn_counting_server(tiny_jpeg()).await;
        let cache =
            ThumbCache::new(reqwest::Client::new(), url, tmp.path().to_path_buf()).unwrap();
        assert!(cache.get("hash1").await.is_ok());
        assert!(cache.get("hash1").await.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
```

- [ ] **Step 3: Run tests — all four should pass**

Run: `cargo test -p shoebox-client --lib thumb_cache -- --nocapture`
Expected: 4 PASS (`disk_path_includes_hash_and_jpg_extension`, `new_creates_disk_dir`, `concurrent_gets_for_same_hash_do_one_http_request`, `second_call_after_first_succeeds_is_memory_hit`).

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/thumb_cache.rs
git -c commit.gpgsign=false commit -m "feat(client): ThumbCache::get with in-flight dedup + memory LRU (Plan 1.4b Task 3)"
```

---

## Task 4: `ThumbCache` — disk-cache hit on cold memory

**Files:**
- Modify: `crates/shoebox-client/src/thumb_cache.rs`

- [ ] **Step 1: Add test for disk-hit-on-cold-memory**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn cold_memory_hot_disk_does_not_hit_server() {
        let tmp = TempDir::new().unwrap();
        let (url, counter) = spawn_counting_server(tiny_jpeg()).await;
        // Pre-seed the disk with the JPEG.
        std::fs::write(tmp.path().join("hash1.jpg"), tiny_jpeg()).unwrap();
        let cache =
            ThumbCache::new(reqwest::Client::new(), url, tmp.path().to_path_buf()).unwrap();
        assert!(cache.get("hash1").await.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
```

The existing `load_uncached` already prefers the disk path — this test exists to lock that behavior down. No production-code change is needed.

- [ ] **Step 2: Run tests — all five should pass**

Run: `cargo test -p shoebox-client --lib thumb_cache`
Expected: 5 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/thumb_cache.rs
git -c commit.gpgsign=false commit -m "test(client): ThumbCache disk-hit-on-cold-memory case (Plan 1.4b Task 4)"
```

---

## Task 5: `library_state` — domain types

**Files:**
- Create: `crates/shoebox-client/src/library_state.rs`
- Modify: `crates/shoebox-client/src/lib.rs` (add `pub mod library_state;`)

- [ ] **Step 1: Create the module with types only**

Create `crates/shoebox-client/src/library_state.rs`:

```rust
//! Domain types + async DB helpers + mutation helpers + pure
//! keyboard-navigation helpers for the demo library view (Plan 1.4b).
//!
//! All reads/writes use a fresh `libsql::Connection` per call (cheap;
//! the underlying `Database` is shared).

use anyhow::{Context, Result};
use std::sync::Arc;

use crate::thumb_cache::ThumbCache;

/// One row in the folder-tree pane. `depth` is the indentation level
/// (0 = root). Computed during tree-flattening, not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub id: String,
    pub name: String,
    pub depth: usize,
}

/// One cell in the photo grid. `display_name` is the filename for masters
/// and `"<filename> (<variant_index + 1>)"` for virtual copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell {
    pub variant_id: String,
    pub photo_id: String,
    pub variant_index: i64,
    pub display_name: String,
    /// Per-user rating for this (variant, current-user), 0 if unset.
    pub rating: u8,
    /// Cached thumbnail image; `None` until the cache resolves it.
    pub thumbnail: Option<Arc<image::DynamicImage>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExifSummary {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub aperture: Option<f64>,
    pub shutter_us: Option<i64>,
    pub focal_length_mm: Option<f64>,
    pub width_px: Option<i64>,
    pub height_px: Option<i64>,
    pub captured_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct KeywordRow {
    pub id: String,
    pub name: String,
}

/// Full payload returned by `load_detail`.
#[derive(Debug, Clone)]
pub struct DetailLoaded {
    pub variant_id: String,
    pub photo_id: String,
    pub exif: ExifSummary,
    pub rating: u8,
    pub keywords: Vec<KeywordRow>,
}

/// Develop-lock state from this client's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockStatus {
    Free,
    HeldByYou,
    HeldByYouTakeoverPending {
        requested_by_display_name: String,
    },
    HeldByOther {
        holder_display_name: String,
    },
    HeldByOtherTakeoverPending {
        holder_display_name: String,
    },
}

/// View state for the Library screen. Created when the user picks a
/// profile; lives in `AppState`.
#[derive(Debug, Clone, Default)]
pub struct LibraryViewState {
    pub folder_tree: Vec<FolderRow>,
    pub selected_folder_id: Option<String>,
    pub grid: Vec<GridCell>,
    pub selected_grid_index: Option<usize>,
    pub detail: Option<DetailLoaded>,
    pub lock_status: LockStatus,
    pub error: Option<String>,
    /// Number of cells per grid row at last layout — used by the keyboard
    /// helper to compute `Down`/`Up` jumps. Updated by `photo_grid::view`.
    pub cells_per_row: usize,
}

impl Default for LockStatus {
    fn default() -> Self {
        Self::Free
    }
}

/// Bundle handed to `library_view::view`; lets the screen pull just what
/// it needs without grabbing all of `AppState`.
#[derive(Debug, Clone)]
pub struct LibraryThumbBundle {
    pub cache: Option<ThumbCache>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Returns the new selected index given the current index, total cells,
/// cells per row, and the direction. Saturates at the edges.
#[must_use]
pub fn advance_selection(
    current: Option<usize>,
    total: usize,
    cells_per_row: usize,
    direction: NavigationDirection,
) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let cells_per_row = cells_per_row.max(1);
    let current = current.unwrap_or(0).min(total - 1);
    let next = match direction {
        NavigationDirection::Left => current.saturating_sub(1),
        NavigationDirection::Right => (current + 1).min(total - 1),
        NavigationDirection::Up => current.saturating_sub(cells_per_row),
        NavigationDirection::Down => (current + cells_per_row).min(total - 1),
    };
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_selection_right_increments() {
        assert_eq!(
            advance_selection(Some(5), 10, 4, NavigationDirection::Right),
            Some(6)
        );
    }

    #[test]
    fn advance_selection_left_saturates_at_zero() {
        assert_eq!(
            advance_selection(Some(0), 10, 4, NavigationDirection::Left),
            Some(0)
        );
    }

    #[test]
    fn advance_selection_down_jumps_one_row() {
        assert_eq!(
            advance_selection(Some(2), 12, 4, NavigationDirection::Down),
            Some(6)
        );
    }

    #[test]
    fn advance_selection_up_saturates_at_first_row() {
        assert_eq!(
            advance_selection(Some(1), 12, 4, NavigationDirection::Up),
            Some(0)
        );
    }

    #[test]
    fn advance_selection_right_at_end_stays() {
        assert_eq!(
            advance_selection(Some(9), 10, 4, NavigationDirection::Right),
            Some(9)
        );
    }

    #[test]
    fn advance_selection_with_empty_grid_returns_none() {
        assert_eq!(
            advance_selection(Some(0), 0, 4, NavigationDirection::Right),
            None
        );
    }
}

// Silences the unused-import warning until later tasks add real callers.
#[allow(dead_code)]
fn _force_imports_used(_: Context<()>, _: Result<()>) {}
```

The `_force_imports_used` stub is removed in Task 6 when the helpers start using `Context` / `Result`.

Edit `crates/shoebox-client/src/lib.rs` and add:

```rust
pub mod library_state;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 6 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs crates/shoebox-client/src/lib.rs
git -c commit.gpgsign=false commit -m "feat(client): library_state domain types + keyboard nav helper (Plan 1.4b Task 5)"
```

---

## Task 6: `load_folder_tree` — flat-indented order from `folders` table

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Add the loader**

Replace the `_force_imports_used` stub with the real loader. Append (above `#[cfg(test)]`):

```rust
/// Load all folders into a depth-first flat-indented list (roots first,
/// each subtree expanded inline before the next root).
pub async fn load_folder_tree(conn: &libsql::Connection) -> Result<Vec<FolderRow>> {
    let mut all_rows = Vec::new();
    let mut rows = conn
        .query(
            "SELECT id, parent_id, name FROM folders ORDER BY name",
            (),
        )
        .await
        .context("loading folder tree")?;
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let parent_id: Option<String> = row.get(1)?;
        let name: String = row.get(2)?;
        all_rows.push((id, parent_id, name));
    }
    Ok(flatten_folders(&all_rows))
}

fn flatten_folders(raw: &[(String, Option<String>, String)]) -> Vec<FolderRow> {
    use std::collections::HashMap;
    let mut children: HashMap<Option<String>, Vec<&(String, Option<String>, String)>> =
        HashMap::new();
    for row in raw {
        children.entry(row.1.clone()).or_default().push(row);
    }
    let mut out = Vec::with_capacity(raw.len());
    fn walk<'a>(
        parent: Option<&'a String>,
        depth: usize,
        children: &HashMap<Option<String>, Vec<&'a (String, Option<String>, String)>>,
        out: &mut Vec<FolderRow>,
    ) {
        let key = parent.cloned();
        if let Some(group) = children.get(&key) {
            for row in group {
                out.push(FolderRow {
                    id: row.0.clone(),
                    name: row.2.clone(),
                    depth,
                });
                walk(Some(&row.0), depth + 1, children, out);
            }
        }
    }
    walk(None, 0, &children, &mut out);
    out
}
```

- [ ] **Step 2: Add test using a real libsql in-memory connection**

Append to the test module:

```rust
    async fn open_test_conn() -> libsql::Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE folders (
                id TEXT PRIMARY KEY,
                parent_id TEXT REFERENCES folders(id),
                path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                last_indexed_at INTEGER
            );",
        )
        .await
        .unwrap();
        conn
    }

    #[tokio::test]
    async fn load_folder_tree_returns_flat_indented_order() {
        let conn = open_test_conn().await;
        for (id, parent, path, name) in [
            ("a", None, "/a", "alpha"),
            ("b", Some("a"), "/a/b", "bravo"),
            ("c", Some("a"), "/a/c", "charlie"),
            ("d", None, "/d", "delta"),
        ] {
            conn.execute(
                "INSERT INTO folders(id, parent_id, path, name) VALUES (?1, ?2, ?3, ?4)",
                (id, parent, path, name),
            )
            .await
            .unwrap();
        }
        let tree = load_folder_tree(&conn).await.unwrap();
        let summary: Vec<_> = tree.iter().map(|r| (r.name.clone(), r.depth)).collect();
        assert_eq!(
            summary,
            vec![
                ("alpha".to_string(), 0),
                ("bravo".to_string(), 1),
                ("charlie".to_string(), 1),
                ("delta".to_string(), 0),
            ]
        );
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 7 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): load_folder_tree builds flat-indented folder list (Plan 1.4b Task 6)"
```

---

## Task 7: `load_grid_for_folder` — variants joined to photos

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append loader**

Append below `flatten_folders`:

```rust
/// Load all variants (master + virtual copies) for photos whose
/// `photo_files.folder_id = folder_id`. Each variant is one grid cell.
/// Cells are ordered by `(captured_at, photo_id, variant_index)`.
pub async fn load_grid_for_folder(
    conn: &libsql::Connection,
    folder_id: &str,
    user_id: &str,
) -> Result<Vec<GridCell>> {
    // Pick the first photo_files row per photo (path used to derive the
    // display filename). MIN(path) is a stable choice.
    let mut rows = conn
        .query(
            "SELECT v.id, v.photo_id, v.variant_index, v.name,
                    COALESCE(vus.rating, 0) AS rating,
                    pf.path AS file_path,
                    p.captured_at
             FROM variants v
             JOIN photos p ON p.id = v.photo_id
             JOIN (
                 SELECT photo_id, MIN(path) AS path, folder_id
                 FROM photo_files
                 GROUP BY photo_id
             ) pf ON pf.photo_id = p.id
             LEFT JOIN variant_user_state vus
                 ON vus.variant_id = v.id AND vus.user_id = ?2
             WHERE pf.folder_id = ?1
             ORDER BY p.captured_at NULLS LAST, p.id, v.variant_index",
            (folder_id, user_id),
        )
        .await
        .context("loading grid")?;

    let mut cells = Vec::new();
    while let Some(row) = rows.next().await? {
        let variant_id: String = row.get(0)?;
        let photo_id: String = row.get(1)?;
        let variant_index: i64 = row.get(2)?;
        let variant_name: Option<String> = row.get(3)?;
        let rating: i64 = row.get(4)?;
        let file_path: String = row.get(5)?;

        let base_name = std::path::Path::new(&file_path)
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or(&file_path)
            .to_string();
        let display_name = match (&variant_name, variant_index) {
            (Some(name), _) => name.clone(),
            (None, 0) => base_name,
            (None, n) => format!("{base_name} ({})", n + 1),
        };

        cells.push(GridCell {
            variant_id,
            photo_id,
            variant_index,
            display_name,
            rating: u8::try_from(rating.clamp(0, 5)).unwrap_or(0),
            thumbnail: None,
        });
    }
    Ok(cells)
}
```

- [ ] **Step 2: Add test**

Append to the test module:

```rust
    async fn seed_full_schema(conn: &libsql::Connection) {
        conn.execute_batch(
            "CREATE TABLE folders (
                id TEXT PRIMARY KEY, parent_id TEXT, path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL, last_indexed_at INTEGER
            );
            CREATE TABLE photos (
                id TEXT PRIMARY KEY, file_size INTEGER NOT NULL, file_format TEXT NOT NULL,
                captured_at INTEGER, camera_make TEXT, camera_model TEXT, lens TEXT,
                iso INTEGER, aperture REAL, shutter_us INTEGER, focal_length_mm REAL,
                width_px INTEGER, height_px INTEGER, orientation INTEGER,
                imported_at INTEGER NOT NULL, exif_json TEXT
            );
            CREATE TABLE photo_files (
                id TEXT PRIMARY KEY, photo_id TEXT NOT NULL, folder_id TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE, file_mtime INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL, is_present INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE users (
                id TEXT PRIMARY KEY, display_name TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE variants (
                id TEXT PRIMARY KEY, photo_id TEXT NOT NULL, variant_index INTEGER NOT NULL,
                name TEXT, created_by TEXT NOT NULL, created_at INTEGER NOT NULL,
                develop_settings_json TEXT NOT NULL, develop_settings_version INTEGER NOT NULL,
                develop_updated_at INTEGER NOT NULL, develop_updated_by TEXT NOT NULL,
                UNIQUE(photo_id, variant_index)
            );
            CREATE TABLE variant_user_state (
                variant_id TEXT NOT NULL, user_id TEXT NOT NULL, rating INTEGER,
                flag TEXT, color_label TEXT, updated_at INTEGER NOT NULL,
                PRIMARY KEY (variant_id, user_id)
            );
            CREATE TABLE keywords (
                id TEXT PRIMARY KEY, parent_id TEXT, name TEXT NOT NULL,
                created_at INTEGER NOT NULL, UNIQUE(parent_id, name)
            );
            CREATE TABLE photo_keywords (
                photo_id TEXT NOT NULL, keyword_id TEXT NOT NULL, added_by TEXT NOT NULL,
                added_at INTEGER NOT NULL, PRIMARY KEY (photo_id, keyword_id)
            );
            CREATE TABLE develop_locks (
                variant_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                user_id TEXT NOT NULL, acquired_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL, takeover_requested_by TEXT,
                takeover_requested_at INTEGER
            );
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, user_id TEXT NOT NULL,
                machine_id TEXT NOT NULL, started_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );",
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO users(id, display_name, created_at) VALUES('u1', 'Alice', 0)",
            (),
        )
        .await
        .unwrap();
    }

    async fn open_full_conn() -> libsql::Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        seed_full_schema(&conn).await;
        conn
    }

    async fn insert_photo(conn: &libsql::Connection, photo_id: &str, folder_id: &str, path: &str, captured_at: i64) {
        conn.execute("INSERT INTO photos(id, file_size, file_format, captured_at, imported_at) VALUES(?1, 100, 'PEF', ?2, 0)", (photo_id, captured_at)).await.unwrap();
        conn.execute("INSERT INTO photo_files(id, photo_id, folder_id, path, file_mtime, last_seen_at) VALUES(?1, ?2, ?3, ?4, 0, 0)",
            (format!("{photo_id}-file"), photo_id, folder_id, path)).await.unwrap();
    }

    async fn insert_variant(conn: &libsql::Connection, id: &str, photo_id: &str, idx: i64) {
        conn.execute(
            "INSERT INTO variants(id, photo_id, variant_index, created_by, created_at,
                develop_settings_json, develop_settings_version,
                develop_updated_at, develop_updated_by)
             VALUES(?1, ?2, ?3, 'u1', 0, '{}', 1, 0, 'u1')",
            (id, photo_id, idx),
        ).await.unwrap();
    }

    #[tokio::test]
    async fn load_grid_returns_one_cell_per_variant_in_folder() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        insert_variant(&conn, "v1", "p1", 0).await;
        insert_variant(&conn, "v2", "p1", 1).await;
        insert_photo(&conn, "p2", "f1", "/x/two.pef", 200).await;
        insert_variant(&conn, "v3", "p2", 0).await;

        let cells = load_grid_for_folder(&conn, "f1", "u1").await.unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].variant_id, "v1");
        assert_eq!(cells[0].display_name, "one.pef");
        assert_eq!(cells[1].variant_id, "v2");
        assert_eq!(cells[1].display_name, "one.pef (2)");
        assert_eq!(cells[2].variant_id, "v3");
        assert_eq!(cells[2].display_name, "two.pef");
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 8 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): load_grid_for_folder joins variants+photos (Plan 1.4b Task 7)"
```

---

## Task 8: `load_detail` — EXIF + rating + keywords for a variant

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append loader**

```rust
pub async fn load_detail(
    conn: &libsql::Connection,
    variant_id: &str,
    user_id: &str,
) -> Result<DetailLoaded> {
    let mut rows = conn
        .query(
            "SELECT p.id, p.camera_make, p.camera_model, p.lens,
                    p.iso, p.aperture, p.shutter_us, p.focal_length_mm,
                    p.width_px, p.height_px, p.captured_at
             FROM variants v JOIN photos p ON p.id = v.photo_id
             WHERE v.id = ?1",
            [variant_id],
        )
        .await
        .context("loading detail row")?;
    let row = rows
        .next()
        .await?
        .context("no variant row")?;

    let photo_id: String = row.get(0)?;
    let exif = ExifSummary {
        camera_make: row.get(1)?,
        camera_model: row.get(2)?,
        lens: row.get(3)?,
        iso: row.get(4)?,
        aperture: row.get(5)?,
        shutter_us: row.get(6)?,
        focal_length_mm: row.get(7)?,
        width_px: row.get(8)?,
        height_px: row.get(9)?,
        captured_at_unix_ms: row.get(10)?,
    };

    let mut rating_rows = conn
        .query(
            "SELECT rating FROM variant_user_state WHERE variant_id=?1 AND user_id=?2",
            (variant_id, user_id),
        )
        .await?;
    let rating = if let Some(row) = rating_rows.next().await? {
        let value: Option<i64> = row.get(0)?;
        u8::try_from(value.unwrap_or(0).clamp(0, 5)).unwrap_or(0)
    } else {
        0
    };

    let mut keyword_rows = conn
        .query(
            "SELECT k.id, k.name FROM photo_keywords pk
             JOIN keywords k ON k.id = pk.keyword_id
             WHERE pk.photo_id = ?1 ORDER BY k.name",
            [&photo_id],
        )
        .await?;
    let mut keywords = Vec::new();
    while let Some(row) = keyword_rows.next().await? {
        keywords.push(KeywordRow {
            id: row.get(0)?,
            name: row.get(1)?,
        });
    }

    Ok(DetailLoaded {
        variant_id: variant_id.to_string(),
        photo_id,
        exif,
        rating,
        keywords,
    })
}
```

- [ ] **Step 2: Add test**

Append:

```rust
    #[tokio::test]
    async fn load_detail_returns_exif_rating_and_keywords() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        conn.execute("UPDATE photos SET camera_make='Pentax', camera_model='K-1', iso=400 WHERE id='p1'", ()).await.unwrap();
        insert_variant(&conn, "v1", "p1", 0).await;
        conn.execute("INSERT INTO variant_user_state(variant_id, user_id, rating, updated_at) VALUES('v1','u1', 4, 0)", ()).await.unwrap();
        conn.execute("INSERT INTO keywords(id, name, created_at) VALUES('k1', 'landscape', 0)", ()).await.unwrap();
        conn.execute("INSERT INTO photo_keywords(photo_id, keyword_id, added_by, added_at) VALUES('p1','k1','u1',0)", ()).await.unwrap();

        let detail = load_detail(&conn, "v1", "u1").await.unwrap();
        assert_eq!(detail.exif.camera_make.as_deref(), Some("Pentax"));
        assert_eq!(detail.exif.iso, Some(400));
        assert_eq!(detail.rating, 4);
        assert_eq!(detail.keywords.len(), 1);
        assert_eq!(detail.keywords[0].name, "landscape");
    }

    #[tokio::test]
    async fn load_detail_returns_zero_rating_when_no_user_state() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        insert_variant(&conn, "v1", "p1", 0).await;
        let detail = load_detail(&conn, "v1", "u1").await.unwrap();
        assert_eq!(detail.rating, 0);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 10 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): load_detail aggregates EXIF+rating+keywords (Plan 1.4b Task 8)"
```

---

## Task 9: Lock helpers — `lock_status_from_row` + `load_lock_status`

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append helpers**

```rust
/// Inputs needed by the pure decoder to produce a `LockStatus`.
#[derive(Debug, Clone)]
pub struct LockRowSnapshot {
    pub holder_user_id: String,
    pub holder_display_name: String,
    pub takeover_requested_by: Option<String>,
    pub takeover_requested_by_display_name: Option<String>,
}

#[must_use]
pub fn lock_status_from_row(row: Option<&LockRowSnapshot>, current_user_id: &str) -> LockStatus {
    let Some(row) = row else {
        return LockStatus::Free;
    };
    let i_hold = row.holder_user_id == current_user_id;
    let takeover_by_me = row.takeover_requested_by.as_deref() == Some(current_user_id);
    match (i_hold, row.takeover_requested_by.is_some()) {
        (true, true) => LockStatus::HeldByYouTakeoverPending {
            requested_by_display_name: row
                .takeover_requested_by_display_name
                .clone()
                .unwrap_or_default(),
        },
        (true, false) => LockStatus::HeldByYou,
        (false, true) if takeover_by_me => LockStatus::HeldByOtherTakeoverPending {
            holder_display_name: row.holder_display_name.clone(),
        },
        (false, true) => LockStatus::HeldByOther {
            holder_display_name: row.holder_display_name.clone(),
        },
        (false, false) => LockStatus::HeldByOther {
            holder_display_name: row.holder_display_name.clone(),
        },
    }
}

pub async fn load_lock_status(
    conn: &libsql::Connection,
    variant_id: &str,
    current_user_id: &str,
) -> Result<LockStatus> {
    let mut rows = conn
        .query(
            "SELECT dl.user_id, holder.display_name,
                    dl.takeover_requested_by, requester.display_name
             FROM develop_locks dl
             JOIN users holder ON holder.id = dl.user_id
             LEFT JOIN users requester ON requester.id = dl.takeover_requested_by
             WHERE dl.variant_id = ?1",
            [variant_id],
        )
        .await
        .context("loading lock status")?;
    let snap = if let Some(row) = rows.next().await? {
        Some(LockRowSnapshot {
            holder_user_id: row.get(0)?,
            holder_display_name: row.get(1)?,
            takeover_requested_by: row.get(2)?,
            takeover_requested_by_display_name: row.get(3)?,
        })
    } else {
        None
    };
    Ok(lock_status_from_row(snap.as_ref(), current_user_id))
}
```

- [ ] **Step 2: Add tests for all four states**

Append:

```rust
    fn snapshot(holder: &str, holder_name: &str, requester: Option<(&str, &str)>) -> LockRowSnapshot {
        LockRowSnapshot {
            holder_user_id: holder.into(),
            holder_display_name: holder_name.into(),
            takeover_requested_by: requester.map(|(id, _)| id.into()),
            takeover_requested_by_display_name: requester.map(|(_, name)| name.into()),
        }
    }

    #[test]
    fn lock_status_free_when_no_row() {
        assert_eq!(lock_status_from_row(None, "me"), LockStatus::Free);
    }

    #[test]
    fn lock_status_held_by_you_when_you_hold() {
        let snap = snapshot("me", "Me", None);
        assert_eq!(lock_status_from_row(Some(&snap), "me"), LockStatus::HeldByYou);
    }

    #[test]
    fn lock_status_held_by_other_when_other_holds() {
        let snap = snapshot("alice", "Alice", None);
        assert_eq!(
            lock_status_from_row(Some(&snap), "me"),
            LockStatus::HeldByOther { holder_display_name: "Alice".into() }
        );
    }

    #[test]
    fn lock_status_held_by_you_takeover_pending_when_you_hold_and_request_came() {
        let snap = snapshot("me", "Me", Some(("alice", "Alice")));
        assert_eq!(
            lock_status_from_row(Some(&snap), "me"),
            LockStatus::HeldByYouTakeoverPending { requested_by_display_name: "Alice".into() }
        );
    }

    #[test]
    fn lock_status_held_by_other_takeover_pending_when_you_requested() {
        let snap = snapshot("alice", "Alice", Some(("me", "Me")));
        assert_eq!(
            lock_status_from_row(Some(&snap), "me"),
            LockStatus::HeldByOtherTakeoverPending { holder_display_name: "Alice".into() }
        );
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 15 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): lock status helpers cover all four states (Plan 1.4b Task 9)"
```

---

## Task 10: Rating UPSERT helper

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append helper + test**

```rust
/// Insert-or-update the per-(variant, user) rating.
pub async fn upsert_rating(
    conn: &libsql::Connection,
    variant_id: &str,
    user_id: &str,
    rating: u8,
) -> Result<()> {
    let now_ms = now_unix_ms();
    let rating_int = i64::from(rating.clamp(0, 5));
    conn.execute(
        "INSERT INTO variant_user_state(variant_id, user_id, rating, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(variant_id, user_id)
         DO UPDATE SET rating = excluded.rating, updated_at = excluded.updated_at",
        (variant_id, user_id, rating_int, now_ms),
    )
    .await
    .context("upserting rating")?;
    Ok(())
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}
```

Append to test module:

```rust
    #[tokio::test]
    async fn upsert_rating_inserts_then_updates() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        insert_variant(&conn, "v1", "p1", 0).await;

        upsert_rating(&conn, "v1", "u1", 3).await.unwrap();
        let detail = load_detail(&conn, "v1", "u1").await.unwrap();
        assert_eq!(detail.rating, 3);

        upsert_rating(&conn, "v1", "u1", 5).await.unwrap();
        let detail = load_detail(&conn, "v1", "u1").await.unwrap();
        assert_eq!(detail.rating, 5);
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 16 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): upsert_rating helper (Plan 1.4b Task 10)"
```

---

## Task 11: Keyword add (race-resolved) + remove helpers

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append helpers**

```rust
/// Attach a keyword to a photo. Creates the root-level keyword if it
/// doesn't already exist. On UNIQUE conflict (a concurrent insert won),
/// resolves the existing `keyword_id` and proceeds.
pub async fn add_keyword(
    conn: &libsql::Connection,
    photo_id: &str,
    user_id: &str,
    name: &str,
) -> Result<String> {
    let now_ms = now_unix_ms();
    let new_id = uuid_v4_hex();

    // Try to insert the keyword. If a row already exists with the same
    // (parent_id=NULL, name) it'll fail with UNIQUE constraint.
    let insert_result = conn
        .execute(
            "INSERT INTO keywords(id, parent_id, name, created_at) VALUES (?1, NULL, ?2, ?3)",
            (&new_id, name, now_ms),
        )
        .await;

    let keyword_id = match insert_result {
        Ok(_) => new_id,
        Err(error) => {
            let msg = error.to_string().to_lowercase();
            if !(msg.contains("unique") || msg.contains("constraint")) {
                return Err(error).context("inserting keyword");
            }
            let mut rows = conn
                .query(
                    "SELECT id FROM keywords WHERE parent_id IS NULL AND name = ?1",
                    [name],
                )
                .await?;
            let existing = rows
                .next()
                .await?
                .context("keyword INSERT failed UNIQUE but no row found")?;
            existing.get::<String>(0)?
        }
    };

    conn.execute(
        "INSERT OR IGNORE INTO photo_keywords(photo_id, keyword_id, added_by, added_at)
         VALUES (?1, ?2, ?3, ?4)",
        (photo_id, &keyword_id, user_id, now_ms),
    )
    .await
    .context("attaching keyword to photo")?;
    Ok(keyword_id)
}

pub async fn remove_keyword(
    conn: &libsql::Connection,
    photo_id: &str,
    keyword_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM photo_keywords WHERE photo_id = ?1 AND keyword_id = ?2",
        (photo_id, keyword_id),
    )
    .await
    .context("removing keyword")?;
    Ok(())
}

fn uuid_v4_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
```

- [ ] **Step 2: Append tests**

```rust
    #[tokio::test]
    async fn add_keyword_creates_and_attaches() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;

        let id = add_keyword(&conn, "p1", "u1", "trees").await.unwrap();
        insert_variant(&conn, "v1", "p1", 0).await;
        let detail = load_detail(&conn, "v1", "u1").await.unwrap();
        assert_eq!(detail.keywords.len(), 1);
        assert_eq!(detail.keywords[0].id, id);
        assert_eq!(detail.keywords[0].name, "trees");
    }

    #[tokio::test]
    async fn add_keyword_twice_resolves_to_same_id() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        insert_photo(&conn, "p2", "f1", "/x/two.pef", 200).await;
        let id_a = add_keyword(&conn, "p1", "u1", "trees").await.unwrap();
        let id_b = add_keyword(&conn, "p2", "u1", "trees").await.unwrap();
        assert_eq!(id_a, id_b);
    }

    #[tokio::test]
    async fn remove_keyword_detaches_only_specified_pair() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        insert_variant(&conn, "v1", "p1", 0).await;
        let id = add_keyword(&conn, "p1", "u1", "trees").await.unwrap();
        remove_keyword(&conn, "p1", &id).await.unwrap();
        let detail = load_detail(&conn, "v1", "u1").await.unwrap();
        assert!(detail.keywords.is_empty());
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 19 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): keyword add+remove with race-resolved insert (Plan 1.4b Task 11)"
```

---

## Task 12: Virtual copy helper

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append helper + test**

```rust
/// Create a virtual copy of `photo_id` by cloning the next variant_index.
/// Picks `MAX(variant_index) + 1`; returns the new variant's id.
pub async fn create_virtual_copy(
    conn: &libsql::Connection,
    photo_id: &str,
    user_id: &str,
) -> Result<String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(variant_index), -1) FROM variants WHERE photo_id = ?1",
            [photo_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .context("no rows returned for MAX(variant_index)")?;
    let max_index: i64 = row.get(0)?;
    let next_index = max_index + 1;

    let mut parent_rows = conn
        .query(
            "SELECT develop_settings_json, develop_settings_version FROM variants
             WHERE photo_id = ?1 ORDER BY variant_index LIMIT 1",
            [photo_id],
        )
        .await?;
    let (parent_json, parent_version): (String, i64) = if let Some(parent_row) =
        parent_rows.next().await?
    {
        (parent_row.get(0)?, parent_row.get(1)?)
    } else {
        ("{}".to_string(), 1)
    };

    let new_id = uuid_v4_hex();
    let now_ms = now_unix_ms();
    conn.execute(
        "INSERT INTO variants(id, photo_id, variant_index, created_by, created_at,
            develop_settings_json, develop_settings_version,
            develop_updated_at, develop_updated_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?5, ?4)",
        (
            &new_id,
            photo_id,
            next_index,
            user_id,
            now_ms,
            parent_json,
            parent_version,
        ),
    )
    .await
    .context("creating virtual copy")?;
    Ok(new_id)
}
```

Append test:

```rust
    #[tokio::test]
    async fn create_virtual_copy_appends_next_index() {
        let conn = open_full_conn().await;
        conn.execute("INSERT INTO folders(id, path, name) VALUES('f1', '/x', 'X')", ()).await.unwrap();
        insert_photo(&conn, "p1", "f1", "/x/one.pef", 100).await;
        insert_variant(&conn, "v1", "p1", 0).await;

        let new_id = create_virtual_copy(&conn, "p1", "u1").await.unwrap();
        let cells = load_grid_for_folder(&conn, "f1", "u1").await.unwrap();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[1].variant_id, new_id);
        assert_eq!(cells[1].variant_index, 1);
        assert_eq!(cells[1].display_name, "one.pef (2)");
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p shoebox-client --lib library_state`
Expected: 20 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): create_virtual_copy helper (Plan 1.4b Task 12)"
```

---

## Task 13: HTTP helpers for lock endpoints

**Files:**
- Modify: `crates/shoebox-client/src/library_state.rs`

- [ ] **Step 1: Append HTTP helpers**

```rust
/// Outcome distinguishes "took it" from "someone else has it" so the
/// caller can route 409 to a status refresh instead of an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockAcquireOutcome {
    Acquired,
    AlreadyHeld,
}

pub async fn http_acquire_lock(
    client: &reqwest::Client,
    server_base_url: &str,
    variant_id: &str,
) -> Result<LockAcquireOutcome> {
    let url = format!(
        "{}/locks/{variant_id}",
        server_base_url.trim_end_matches('/')
    );
    let response = client
        .post(&url)
        .send()
        .await
        .context("POST /locks/:id")?;
    match response.status().as_u16() {
        200 => Ok(LockAcquireOutcome::Acquired),
        409 => Ok(LockAcquireOutcome::AlreadyHeld),
        status => Err(anyhow::anyhow!("acquire returned status {status}")),
    }
}

pub async fn http_heartbeat_lock(
    client: &reqwest::Client,
    server_base_url: &str,
    variant_id: &str,
) -> Result<()> {
    let url = format!(
        "{}/locks/{variant_id}",
        server_base_url.trim_end_matches('/')
    );
    let response = client
        .put(&url)
        .send()
        .await
        .context("PUT /locks/:id")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "heartbeat returned status {}",
            response.status()
        ))
    }
}

pub async fn http_release_lock(
    client: &reqwest::Client,
    server_base_url: &str,
    variant_id: &str,
) -> Result<()> {
    let url = format!(
        "{}/locks/{variant_id}",
        server_base_url.trim_end_matches('/')
    );
    let response = client
        .delete(&url)
        .send()
        .await
        .context("DELETE /locks/:id")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "release returned status {}",
            response.status()
        ))
    }
}

pub async fn http_request_takeover(
    client: &reqwest::Client,
    server_base_url: &str,
    variant_id: &str,
) -> Result<()> {
    let url = format!(
        "{}/locks/{variant_id}/takeover",
        server_base_url.trim_end_matches('/')
    );
    let response = client
        .post(&url)
        .send()
        .await
        .context("POST /locks/:id/takeover")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "takeover returned status {}",
            response.status()
        ))
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean compile.

(These helpers don't get unit tests at this stage; they're exercised by `library_lock_e2e.rs` in Task 23.)

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): HTTP helpers for /locks/:id endpoints (Plan 1.4b Task 13)"
```

---

## Task 14: Embed `LibraryViewState` + `ThumbCache` in `AppState`; declare `library_view` module + Message variants

**Files:**
- Modify: `crates/shoebox-client/src/app_state.rs`
- Modify: `crates/shoebox-client/src/screens/mod.rs`

- [ ] **Step 1: Extend `AppState`**

Edit `crates/shoebox-client/src/app_state.rs`. Add an import at the top:

```rust
use crate::library_state::LibraryViewState;
use crate::thumb_cache::ThumbCache;
```

Add fields to `AppState`:

```rust
    /// View state for the demo library screen. Reset on logout.
    pub library_view: LibraryViewState,
    /// Created when the replica opens; shared by the photo grid and detail
    /// panel via clones (it's internally `Arc<Mutex<…>>`).
    pub thumb_cache: Option<ThumbCache>,
```

Add initial values to `AppState::new`:

```rust
            library_view: LibraryViewState::default(),
            thumb_cache: None,
```

- [ ] **Step 2: Declare module + add Message variants**

Edit `crates/shoebox-client/src/screens/mod.rs`. Below `pub mod library;` add:

```rust
pub mod library_view;
```

Add at the top of the imports section:

```rust
use crate::library_state::{DetailLoaded, FolderRow, GridCell, LockStatus, NavigationDirection};
use crate::thumb_cache::CachedResult;
```

Append the following variants to the `Message` enum (before the `Generic` block):

```rust
    // Library view
    LibraryFolderTreeLoaded(Result<Vec<FolderRow>, String>),
    LibraryFolderSelected(String),
    LibraryGridLoaded {
        folder_id: String,
        cells: Result<Vec<GridCell>, String>,
    },
    LibraryThumbReady {
        hash: String,
        result: CachedResult,
    },
    LibraryGridCellSelected(usize),
    LibraryDetailLoaded(Result<DetailLoaded, String>),
    LibraryRatingChanged {
        variant_id: String,
        rating: u8,
    },
    LibraryRatingPersisted(Result<(), String>),
    LibraryKeywordInputChanged(String),
    LibraryKeywordSubmitted,
    LibraryKeywordAddPersisted(Result<(), String>),
    LibraryKeywordRemoveClicked {
        keyword_id: String,
    },
    LibraryKeywordRemovePersisted(Result<(), String>),
    LibraryNewVirtualCopyClicked,
    LibraryVirtualCopyPersisted(Result<String, String>),
    LibraryLockStatusTick,
    LibraryLockStatusLoaded(Result<LockStatus, String>),
    LibraryAcquireLockClicked,
    LibraryRequestTakeoverClicked,
    LibraryReleaseLockClicked,
    LibraryLockActionPersisted(Result<(), String>),
    LibraryLockHeartbeatTick,
    LibraryKeyboardNavigation(NavigationDirection),
    LibraryKeyboardRating(u8),
    LibraryClearError,
```

The keyword-input intermediate state lives in `LibraryViewState`. Add this field to `LibraryViewState` in `library_state.rs`:

```rust
    pub keyword_input: String,
```

(Already covered by `#[derive(Default)]`; just add the line.)

- [ ] **Step 3: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean (warnings about unused message variants are fine — they get wired in later tasks).

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/app_state.rs crates/shoebox-client/src/screens/mod.rs crates/shoebox-client/src/library_state.rs
git -c commit.gpgsign=false commit -m "feat(client): embed LibraryViewState; declare library_view module and Messages (Plan 1.4b Task 14)"
```

---

## Task 15: Pure view — `folder_tree.rs`

**Files:**
- Create: `crates/shoebox-client/src/screens/library_view/folder_tree.rs`

- [ ] **Step 1: Create directory and file**

Create `crates/shoebox-client/src/screens/library_view/folder_tree.rs`:

```rust
//! Left-pane: scrollable folder tree.

use iced::widget::{button, column, container, scrollable, text};
use iced::{Element, Length};

use crate::library_state::FolderRow;
use crate::screens::Message;

#[must_use]
pub fn view<'a>(
    rows: &'a [FolderRow],
    selected: Option<&'a str>,
) -> Element<'a, Message> {
    let mut column_widget = column![text("Folders").size(18)].spacing(2).padding(8);
    if rows.is_empty() {
        column_widget = column_widget.push(text("(empty)"));
    }
    for row in rows {
        let indent = "  ".repeat(row.depth);
        let label = format!("{indent}{}", row.name);
        let is_selected = selected == Some(row.id.as_str());
        let style = if is_selected {
            button::primary
        } else {
            button::text
        };
        column_widget = column_widget.push(
            button(text(label))
                .on_press(Message::LibraryFolderSelected(row.id.clone()))
                .style(style)
                .width(Length::Fill),
        );
    }
    container(scrollable(column_widget).height(Length::Fill))
        .width(Length::Fixed(220.0))
        .height(Length::Fill)
        .into()
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/screens/library_view/folder_tree.rs
git -c commit.gpgsign=false commit -m "feat(client): folder_tree pure view (Plan 1.4b Task 15)"
```

---

## Task 16: Pure view — `photo_grid.rs`

**Files:**
- Create: `crates/shoebox-client/src/screens/library_view/photo_grid.rs`

- [ ] **Step 1: Create file**

```rust
//! Center-pane: photo grid as a wrapping row of fixed 256 px tiles.

use iced::widget::image::{Handle, Image};
use iced::widget::{
    button, column as col, container, row, scrollable, text, Column, Row,
};
use iced::{Color, Element, Length, Padding};

use crate::library_state::GridCell;
use crate::screens::Message;

const TILE_PX: f32 = 256.0;
const TILE_PAD: f32 = 8.0;

#[must_use]
pub fn view<'a>(cells: &'a [GridCell], selected: Option<usize>) -> Element<'a, Message> {
    if cells.is_empty() {
        return container(text("(no photos)")).padding(20).into();
    }
    let mut grid: Column<Message> = col![].spacing(8);
    let cells_per_row = 4;
    let mut current_row: Row<Message> = row![].spacing(8);
    let mut in_row = 0usize;
    for (index, cell) in cells.iter().enumerate() {
        current_row = current_row.push(tile(cell, Some(index) == selected, index));
        in_row += 1;
        if in_row == cells_per_row {
            grid = grid.push(current_row);
            current_row = row![].spacing(8);
            in_row = 0;
        }
    }
    if in_row > 0 {
        grid = grid.push(current_row);
    }
    scrollable(container(grid).padding(Padding::from(12)))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn tile(cell: &GridCell, selected: bool, index: usize) -> Element<'_, Message> {
    let image: Element<Message> = match &cell.thumbnail {
        Some(image) => {
            let rgba = image.to_rgba8();
            let handle = Handle::from_rgba(
                rgba.width(),
                rgba.height(),
                rgba.into_raw(),
            );
            Image::new(handle)
                .width(Length::Fixed(TILE_PX))
                .height(Length::Fixed(TILE_PX))
                .into()
        }
        None => container(text("…"))
            .width(Length::Fixed(TILE_PX))
            .height(Length::Fixed(TILE_PX))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into(),
    };
    let stars = star_row(cell.variant_id.clone(), cell.rating);
    let label = text(&cell.display_name).size(12);
    let inner = col![image, label, stars].spacing(4).padding(TILE_PAD);
    let bg = if selected { Color::from_rgb(0.2, 0.5, 0.9) } else { Color::from_rgb(0.15, 0.15, 0.15) };
    button(container(inner).style(move |_| {
        container::Style {
            background: Some(iced::Background::Color(bg)),
            ..container::Style::default()
        }
    }))
    .on_press(Message::LibraryGridCellSelected(index))
    .padding(0)
    .into()
}

fn star_row(variant_id: String, rating: u8) -> Element<'static, Message> {
    let mut star_row: Row<Message> = row![].spacing(2);
    for star_index in 1u8..=5 {
        let glyph = if star_index <= rating { "★" } else { "☆" };
        let vid = variant_id.clone();
        star_row = star_row.push(
            button(text(glyph)).on_press(Message::LibraryRatingChanged {
                variant_id: vid,
                rating: star_index,
            }).style(button::text),
        );
    }
    star_row.into()
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/screens/library_view/photo_grid.rs
git -c commit.gpgsign=false commit -m "feat(client): photo_grid pure view with star rating tiles (Plan 1.4b Task 16)"
```

---

## Task 17: Pure view — `detail_panel.rs`

**Files:**
- Create: `crates/shoebox-client/src/screens/library_view/detail_panel.rs`

- [ ] **Step 1: Create file**

```rust
//! Right-pane: EXIF + rating + keyword editor + virtual-copy button +
//! lock-status banner.

use iced::widget::{
    button, column as col, container, row, scrollable, text, text_input, Row,
};
use iced::{Element, Length};

use crate::library_state::{DetailLoaded, LockStatus};
use crate::screens::Message;

#[must_use]
pub fn view<'a>(
    detail: Option<&'a DetailLoaded>,
    lock_status: &'a LockStatus,
    keyword_input: &'a str,
) -> Element<'a, Message> {
    let body: Element<Message> = match detail {
        None => text("Select a photo to see details.").into(),
        Some(detail) => detail_body(detail, lock_status, keyword_input),
    };
    container(scrollable(body).height(Length::Fill))
        .width(Length::Fixed(320.0))
        .height(Length::Fill)
        .padding(12)
        .into()
}

fn detail_body<'a>(
    detail: &'a DetailLoaded,
    lock_status: &'a LockStatus,
    keyword_input: &'a str,
) -> Element<'a, Message> {
    let exif = &detail.exif;

    let camera_line = format!(
        "{} {}",
        exif.camera_make.clone().unwrap_or_default(),
        exif.camera_model.clone().unwrap_or_default()
    );
    let lens_line = exif.lens.clone().unwrap_or_else(|| "—".into());
    let dimensions = match (exif.width_px, exif.height_px) {
        (Some(width), Some(height)) => format!("{width}×{height}"),
        _ => "—".into(),
    };
    let shutter = exif
        .shutter_us
        .map(|us| format!("1/{:.0}s", 1_000_000.0 / us as f64))
        .unwrap_or_else(|| "—".into());
    let aperture = exif
        .aperture
        .map(|f| format!("f/{f:.1}"))
        .unwrap_or_else(|| "—".into());
    let iso = exif
        .iso
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".into());
    let focal = exif
        .focal_length_mm
        .map(|f| format!("{f:.0}mm"))
        .unwrap_or_else(|| "—".into());

    let exif_block = col![
        text("EXIF").size(16),
        text(camera_line),
        text(format!("Lens: {lens_line}")),
        text(format!("Pixels: {dimensions}")),
        text(format!("{aperture} · {shutter} · ISO {iso} · {focal}")),
    ]
    .spacing(2);

    let stars = stars_for(detail.rating);
    let mut keyword_row: Row<Message> = row![].spacing(4);
    for keyword in &detail.keywords {
        let kid = keyword.id.clone();
        keyword_row = keyword_row.push(
            button(text(format!("{} ×", keyword.name)))
                .on_press(Message::LibraryKeywordRemoveClicked { keyword_id: kid }),
        );
    }
    let keyword_input_row = row![
        text_input("add keyword…", keyword_input)
            .on_input(Message::LibraryKeywordInputChanged)
            .on_submit(Message::LibraryKeywordSubmitted),
        button(text("Add")).on_press(Message::LibraryKeywordSubmitted),
    ]
    .spacing(6);

    let lock_block = lock_banner(lock_status);
    let virtual_copy_button = button(text("New virtual copy"))
        .on_press(Message::LibraryNewVirtualCopyClicked);

    col![
        exif_block,
        text("Rating").size(16),
        stars,
        text("Keywords").size(16),
        keyword_row,
        keyword_input_row,
        text("Variants").size(16),
        virtual_copy_button,
        text("Lock").size(16),
        lock_block,
    ]
    .spacing(10)
    .into()
}

fn stars_for(rating: u8) -> Element<'static, Message> {
    let mut star_row: Row<Message> = row![].spacing(2);
    for star_index in 0u8..=5 {
        let glyph = if star_index == 0 {
            "—".to_string()
        } else if star_index <= rating {
            "★".to_string()
        } else {
            "☆".to_string()
        };
        // Star 0 = clear. Other stars = set to that rating. We need a
        // variant_id but the detail panel doesn't have it as a String —
        // we route via a "current selection" message in main.rs instead.
        star_row = star_row.push(
            button(text(glyph))
                .on_press(Message::LibraryKeyboardRating(star_index))
                .style(button::text),
        );
    }
    star_row.into()
}

fn lock_banner(status: &LockStatus) -> Element<'_, Message> {
    match status {
        LockStatus::Free => row![
            text("No lock — anyone can edit"),
            button(text("Acquire")).on_press(Message::LibraryAcquireLockClicked),
        ]
        .spacing(8)
        .into(),
        LockStatus::HeldByYou => row![
            text("You hold the lock."),
            button(text("Release")).on_press(Message::LibraryReleaseLockClicked),
        ]
        .spacing(8)
        .into(),
        LockStatus::HeldByYouTakeoverPending {
            requested_by_display_name,
        } => col![
            text(format!(
                "{requested_by_display_name} requested takeover of your lock"
            )),
            row![
                button(text("Release")).on_press(Message::LibraryReleaseLockClicked),
            ]
            .spacing(8),
        ]
        .spacing(4)
        .into(),
        LockStatus::HeldByOther { holder_display_name } => row![
            text(format!("Held by {holder_display_name}")),
            button(text("Request takeover"))
                .on_press(Message::LibraryRequestTakeoverClicked),
        ]
        .spacing(8)
        .into(),
        LockStatus::HeldByOtherTakeoverPending { holder_display_name } => {
            text(format!(
                "Waiting on {holder_display_name} to release the lock…"
            ))
            .into()
        }
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/screens/library_view/detail_panel.rs
git -c commit.gpgsign=false commit -m "feat(client): detail_panel pure view with EXIF, keywords, lock banner (Plan 1.4b Task 17)"
```

---

## Task 18: Compose pane — `library_view/mod.rs` + keyboard subscription

**Files:**
- Create: `crates/shoebox-client/src/screens/library_view/mod.rs`

- [ ] **Step 1: Create file**

```rust
//! Composes the three demo-library panes into one screen and exposes
//! the keyboard subscription that turns arrow keys + 0-5 into Messages.

pub mod detail_panel;
pub mod folder_tree;
pub mod photo_grid;

use iced::keyboard::{key::Named, Key};
use iced::widget::{column as col, container, row, text};
use iced::{Element, Length, Subscription};

use crate::app_state::AppState;
use crate::library_state::NavigationDirection;
use crate::screens::Message;

#[must_use]
pub fn view(state: &AppState) -> Element<'_, Message> {
    let panes = row![
        folder_tree::view(
            &state.library_view.folder_tree,
            state.library_view.selected_folder_id.as_deref(),
        ),
        photo_grid::view(
            &state.library_view.grid,
            state.library_view.selected_grid_index,
        ),
        detail_panel::view(
            state.library_view.detail.as_ref(),
            &state.library_view.lock_status,
            &state.library_view.keyword_input,
        ),
    ]
    .height(Length::Fill);

    let error_banner: Element<Message> = match &state.library_view.error {
        Some(message) => text(format!("⚠ {message}")).into(),
        None => text("").into(),
    };

    container(col![error_banner, panes].spacing(4))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[must_use]
pub fn keyboard_subscription() -> Subscription<Message> {
    iced::keyboard::on_key_press(|key, _modifiers| match key {
        Key::Named(Named::ArrowLeft) => {
            Some(Message::LibraryKeyboardNavigation(NavigationDirection::Left))
        }
        Key::Named(Named::ArrowRight) => Some(Message::LibraryKeyboardNavigation(
            NavigationDirection::Right,
        )),
        Key::Named(Named::ArrowUp) => {
            Some(Message::LibraryKeyboardNavigation(NavigationDirection::Up))
        }
        Key::Named(Named::ArrowDown) => Some(Message::LibraryKeyboardNavigation(
            NavigationDirection::Down,
        )),
        Key::Character(c) => match c.as_str() {
            "0" => Some(Message::LibraryKeyboardRating(0)),
            "1" => Some(Message::LibraryKeyboardRating(1)),
            "2" => Some(Message::LibraryKeyboardRating(2)),
            "3" => Some(Message::LibraryKeyboardRating(3)),
            "4" => Some(Message::LibraryKeyboardRating(4)),
            "5" => Some(Message::LibraryKeyboardRating(5)),
            _ => None,
        },
        _ => None,
    })
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/screens/library_view/mod.rs
git -c commit.gpgsign=false commit -m "feat(client): library_view compose + keyboard subscription (Plan 1.4b Task 18)"
```

---

## Task 19: Wire `Screen::Library` rendering and initial loads in `main.rs`

**Files:**
- Modify: `crates/shoebox-client/src/main.rs`

- [ ] **Step 1: Route Screen::Library through library_view::view**

In `main.rs`, find the `view()` function's match on `Screen::Library` (currently delegates to `library::view(…)`) and replace that arm with:

```rust
        Screen::Library => crate::screens::library_view::view(state),
```

Remove the unused `library::view` call's imports if the compiler complains; the `library::load_stats` + `LibraryStats` helpers stay (used by the initial sync flow that runs before the new loaders take over).

- [ ] **Step 2: Add the library subscription**

Find the `subscription()` function in `main.rs`. Combine its existing return with the library keyboard subscription using `Subscription::batch`:

```rust
fn subscription(state: &AppState) -> Subscription<Message> {
    let mut subs: Vec<Subscription<Message>> = vec![/* existing ones — keep verbatim */];
    if matches!(state.screen, Screen::Library) {
        subs.push(crate::screens::library_view::keyboard_subscription());
        subs.push(
            iced::time::every(std::time::Duration::from_secs(5))
                .map(|_| Message::LibraryLockStatusTick),
        );
        subs.push(
            iced::time::every(std::time::Duration::from_secs(300))
                .map(|_| Message::LibraryLockHeartbeatTick),
        );
    }
    Subscription::batch(subs)
}
```

(Adjust the existing subscription body if it's not list-shaped; the goal is to add the three library-only subs while keeping the cert-renewal + replica-sync subs running.)

- [ ] **Step 3: Add initial folder-tree load on Library entry**

Locate the `Message::UserPicked(_)` arm and the `Message::ReplicaOpenedAndStatsLoaded(Ok(_))` arm — both transition into `Screen::Library`. Right after each one sets `state.screen = Screen::Library`, append:

```rust
return Command::perform(
    {
        let replica = state.replica.clone().expect("replica present after login");
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            crate::library_state::load_folder_tree(&conn)
                .await
                .map_err(|error| error.to_string())
        }
    },
    Message::LibraryFolderTreeLoaded,
);
```

- [ ] **Step 4: Handle LibraryFolderTreeLoaded**

Add an arm to `update()`:

```rust
        Message::LibraryFolderTreeLoaded(Ok(rows)) => {
            state.library_view.folder_tree = rows;
            state.library_view.error = None;
            if let Some(first) = state.library_view.folder_tree.first().cloned() {
                state.library_view.selected_folder_id = Some(first.id.clone());
                return command_for_grid(state, first.id);
            }
            Command::none()
        }
        Message::LibraryFolderTreeLoaded(Err(e)) => {
            state.library_view.error = Some(format!("Folder tree failed: {e}"));
            Command::none()
        }
        Message::LibraryFolderSelected(folder_id) => {
            state.library_view.selected_folder_id = Some(folder_id.clone());
            state.library_view.selected_grid_index = None;
            state.library_view.detail = None;
            state.library_view.lock_status = crate::library_state::LockStatus::Free;
            command_for_grid(state, folder_id)
        }
```

Add the `command_for_grid` helper to `main.rs` (top-level fn):

```rust
fn command_for_grid(state: &AppState, folder_id: String) -> Command<Message> {
    let Some(replica) = state.replica.clone() else {
        return Command::none();
    };
    let Some(user_id) = state.config.active_user_id.clone() else {
        return Command::none();
    };
    Command::perform(
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            crate::library_state::load_grid_for_folder(&conn, &folder_id, &user_id)
                .await
                .map_err(|error| error.to_string())
                .map(|cells| (folder_id, cells))
        },
        |result| match result {
            Ok((folder_id, cells)) => Message::LibraryGridLoaded {
                folder_id,
                cells: Ok(cells),
            },
            Err(error) => Message::LibraryGridLoaded {
                folder_id: String::new(),
                cells: Err(error),
            },
        },
    )
}
```

- [ ] **Step 5: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean (some unused-message-variant warnings are fine).

- [ ] **Step 6: Commit**

```bash
git add crates/shoebox-client/src/main.rs
git -c commit.gpgsign=false commit -m "feat(client): route Library screen to library_view + initial folder load (Plan 1.4b Task 19)"
```

---

## Task 20: Grid + thumbnails — handle `LibraryGridLoaded` + `LibraryThumbReady` + selection

**Files:**
- Modify: `crates/shoebox-client/src/main.rs`

- [ ] **Step 1: Build the thumb cache on replica open**

Find the function that constructs the `OpenedReplicaBundle` (steady-state init in `main.rs`). After the replica + reqwest client are created, also build the thumb cache:

```rust
let thumb_dir = directories::ProjectDirs::from("net", "shoebox", "shoebox-client")
    .map(|p| p.cache_dir().join("thumbs"))
    .unwrap_or_else(|| std::path::PathBuf::from("./shoebox-thumbs"));
let thumb_cache = crate::thumb_cache::ThumbCache::new(
    client.clone(),
    server_url.clone(),
    thumb_dir,
)
.map_err(|error| error.to_string())?;
```

…and stash it in the bundle / state right after `state.replica = Some(replica.clone())`:

```rust
state.thumb_cache = Some(thumb_cache);
```

(Do the equivalent in the post-enrollment finalization path so the cache exists both for first-run and steady-state.)

- [ ] **Step 2: Handle grid-loaded message**

Add arms:

```rust
        Message::LibraryGridLoaded { folder_id, cells } => {
            if state.library_view.selected_folder_id.as_deref() != Some(&folder_id) && !folder_id.is_empty() {
                return Command::none();
            }
            match cells {
                Ok(cells) => {
                    state.library_view.grid = cells;
                    state.library_view.error = None;
                    let cmds = thumb_fetch_commands(state);
                    return Command::batch(cmds);
                }
                Err(e) => {
                    state.library_view.error = Some(format!("Grid load failed: {e}"));
                    Command::none()
                }
            }
        }
        Message::LibraryThumbReady { hash, result } => {
            if let Ok(image) = result {
                for cell in state.library_view.grid.iter_mut() {
                    if cell.photo_id == hash {
                        cell.thumbnail = Some(image.clone());
                    }
                }
            }
            Command::none()
        }
        Message::LibraryGridCellSelected(index) => {
            state.library_view.selected_grid_index = Some(index);
            command_for_detail(state)
        }
```

Add the `thumb_fetch_commands` helper:

```rust
fn thumb_fetch_commands(state: &AppState) -> Vec<Command<Message>> {
    let Some(cache) = state.thumb_cache.clone() else {
        return Vec::new();
    };
    state
        .library_view
        .grid
        .iter()
        .filter(|cell| cell.thumbnail.is_none())
        .map(|cell| {
            let hash = cell.photo_id.clone();
            let cache = cache.clone();
            Command::perform(
                async move {
                    let result = cache.get(&hash).await;
                    (hash, result)
                },
                |(hash, result)| Message::LibraryThumbReady { hash, result },
            )
        })
        .collect()
}
```

Add `command_for_detail` (will be expanded in next task):

```rust
fn command_for_detail(state: &AppState) -> Command<Message> {
    let Some(replica) = state.replica.clone() else {
        return Command::none();
    };
    let Some(user_id) = state.config.active_user_id.clone() else {
        return Command::none();
    };
    let Some(index) = state.library_view.selected_grid_index else {
        return Command::none();
    };
    let Some(cell) = state.library_view.grid.get(index).cloned() else {
        return Command::none();
    };
    Command::perform(
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            crate::library_state::load_detail(&conn, &cell.variant_id, &user_id)
                .await
                .map_err(|error| error.to_string())
        },
        Message::LibraryDetailLoaded,
    )
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/main.rs
git -c commit.gpgsign=false commit -m "feat(client): grid+thumbnail message handlers and cache wiring (Plan 1.4b Task 20)"
```

---

## Task 21: Detail + rating + keyword + virtual copy handlers

**Files:**
- Modify: `crates/shoebox-client/src/main.rs`

- [ ] **Step 1: Add arms**

```rust
        Message::LibraryDetailLoaded(Ok(detail)) => {
            state.library_view.detail = Some(detail);
            state.library_view.error = None;
            // Kick off a fresh lock-status read for this selection.
            return command_for_lock_status(state);
        }
        Message::LibraryDetailLoaded(Err(e)) => {
            state.library_view.error = Some(format!("Detail load failed: {e}"));
            Command::none()
        }

        Message::LibraryRatingChanged { variant_id, rating } => {
            persist_rating(state, variant_id, rating)
        }
        Message::LibraryKeyboardRating(rating) => {
            let Some(index) = state.library_view.selected_grid_index else {
                return Command::none();
            };
            let Some(cell) = state.library_view.grid.get(index).cloned() else {
                return Command::none();
            };
            persist_rating(state, cell.variant_id, rating)
        }
        Message::LibraryRatingPersisted(Ok(())) => {
            // Reload detail + grid cell from local replica.
            command_for_detail_and_grid(state)
        }
        Message::LibraryRatingPersisted(Err(e)) => {
            state.library_view.error = Some(format!("Save rating failed: {e}"));
            Command::none()
        }

        Message::LibraryKeywordInputChanged(value) => {
            state.library_view.keyword_input = value;
            Command::none()
        }
        Message::LibraryKeywordSubmitted => {
            let name = std::mem::take(&mut state.library_view.keyword_input);
            let name = name.trim().to_string();
            if name.is_empty() {
                return Command::none();
            }
            let Some(replica) = state.replica.clone() else {
                return Command::none();
            };
            let Some(user_id) = state.config.active_user_id.clone() else {
                return Command::none();
            };
            let Some(detail) = state.library_view.detail.clone() else {
                return Command::none();
            };
            Command::perform(
                async move {
                    let conn = replica.conn().map_err(|error| error.to_string())?;
                    crate::library_state::add_keyword(&conn, &detail.photo_id, &user_id, &name)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                },
                Message::LibraryKeywordAddPersisted,
            )
        }
        Message::LibraryKeywordAddPersisted(Ok(())) => command_for_detail(state),
        Message::LibraryKeywordAddPersisted(Err(e)) => {
            state.library_view.error = Some(format!("Add keyword failed: {e}"));
            Command::none()
        }

        Message::LibraryKeywordRemoveClicked { keyword_id } => {
            let Some(replica) = state.replica.clone() else { return Command::none(); };
            let Some(detail) = state.library_view.detail.clone() else { return Command::none(); };
            Command::perform(
                async move {
                    let conn = replica.conn().map_err(|error| error.to_string())?;
                    crate::library_state::remove_keyword(&conn, &detail.photo_id, &keyword_id)
                        .await
                        .map_err(|error| error.to_string())
                },
                Message::LibraryKeywordRemovePersisted,
            )
        }
        Message::LibraryKeywordRemovePersisted(Ok(())) => command_for_detail(state),
        Message::LibraryKeywordRemovePersisted(Err(e)) => {
            state.library_view.error = Some(format!("Remove keyword failed: {e}"));
            Command::none()
        }

        Message::LibraryNewVirtualCopyClicked => {
            let Some(replica) = state.replica.clone() else { return Command::none(); };
            let Some(user_id) = state.config.active_user_id.clone() else { return Command::none(); };
            let Some(detail) = state.library_view.detail.clone() else { return Command::none(); };
            Command::perform(
                async move {
                    let conn = replica.conn().map_err(|error| error.to_string())?;
                    crate::library_state::create_virtual_copy(&conn, &detail.photo_id, &user_id)
                        .await
                        .map_err(|error| error.to_string())
                },
                Message::LibraryVirtualCopyPersisted,
            )
        }
        Message::LibraryVirtualCopyPersisted(Ok(_)) => {
            // Reload the grid for the current folder.
            let Some(folder_id) = state.library_view.selected_folder_id.clone() else {
                return Command::none();
            };
            command_for_grid(state, folder_id)
        }
        Message::LibraryVirtualCopyPersisted(Err(e)) => {
            state.library_view.error = Some(format!("Create virtual copy failed: {e}"));
            Command::none()
        }
```

Add the helpers:

```rust
fn persist_rating(state: &AppState, variant_id: String, rating: u8) -> Command<Message> {
    let Some(replica) = state.replica.clone() else { return Command::none(); };
    let Some(user_id) = state.config.active_user_id.clone() else { return Command::none(); };
    Command::perform(
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            crate::library_state::upsert_rating(&conn, &variant_id, &user_id, rating)
                .await
                .map_err(|error| error.to_string())
        },
        Message::LibraryRatingPersisted,
    )
}

fn command_for_detail_and_grid(state: &AppState) -> Command<Message> {
    let mut cmds = vec![command_for_detail(state)];
    if let Some(folder_id) = state.library_view.selected_folder_id.clone() {
        cmds.push(command_for_grid(state, folder_id));
    }
    Command::batch(cmds)
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/main.rs
git -c commit.gpgsign=false commit -m "feat(client): detail+rating+keyword+virtual-copy handlers (Plan 1.4b Task 21)"
```

---

## Task 22: Lock-status tick + acquire/release/takeover/heartbeat handlers

**Files:**
- Modify: `crates/shoebox-client/src/main.rs`

- [ ] **Step 1: Add arms**

```rust
        Message::LibraryLockStatusTick => command_for_lock_status(state),
        Message::LibraryLockStatusLoaded(Ok(status)) => {
            state.library_view.lock_status = status;
            Command::none()
        }
        Message::LibraryLockStatusLoaded(Err(e)) => {
            state.library_view.error = Some(format!("Lock status load failed: {e}"));
            Command::none()
        }

        Message::LibraryAcquireLockClicked => http_lock_command(state, LockAction::Acquire),
        Message::LibraryReleaseLockClicked => http_lock_command(state, LockAction::Release),
        Message::LibraryRequestTakeoverClicked => http_lock_command(state, LockAction::Takeover),
        Message::LibraryLockActionPersisted(Ok(())) => command_for_lock_status(state),
        Message::LibraryLockActionPersisted(Err(e)) => {
            state.library_view.error = Some(format!("Lock action failed: {e}"));
            Command::none()
        }
        Message::LibraryLockHeartbeatTick => {
            if !matches!(state.library_view.lock_status, crate::library_state::LockStatus::HeldByYou | crate::library_state::LockStatus::HeldByYouTakeoverPending { .. }) {
                return Command::none();
            }
            let Some(client) = state.client.clone() else { return Command::none(); };
            let Some(detail) = state.library_view.detail.clone() else { return Command::none(); };
            let server_url = state.config.server_url.clone();
            Command::perform(
                async move {
                    crate::library_state::http_heartbeat_lock(&client, &server_url, &detail.variant_id)
                        .await
                        .map_err(|error| error.to_string())
                },
                Message::LibraryLockActionPersisted,
            )
        }
```

Add the lock action helpers:

```rust
enum LockAction { Acquire, Release, Takeover }

fn http_lock_command(state: &AppState, action: LockAction) -> Command<Message> {
    let Some(client) = state.client.clone() else { return Command::none(); };
    let Some(detail) = state.library_view.detail.clone() else { return Command::none(); };
    let server_url = state.config.server_url.clone();
    match action {
        LockAction::Acquire => Command::perform(
            async move {
                crate::library_state::http_acquire_lock(&client, &server_url, &detail.variant_id)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
            Message::LibraryLockActionPersisted,
        ),
        LockAction::Release => Command::perform(
            async move {
                crate::library_state::http_release_lock(&client, &server_url, &detail.variant_id)
                    .await
                    .map_err(|error| error.to_string())
            },
            Message::LibraryLockActionPersisted,
        ),
        LockAction::Takeover => Command::perform(
            async move {
                crate::library_state::http_request_takeover(&client, &server_url, &detail.variant_id)
                    .await
                    .map_err(|error| error.to_string())
            },
            Message::LibraryLockActionPersisted,
        ),
    }
}

fn command_for_lock_status(state: &AppState) -> Command<Message> {
    let Some(replica) = state.replica.clone() else { return Command::none(); };
    let Some(user_id) = state.config.active_user_id.clone() else { return Command::none(); };
    let Some(detail) = state.library_view.detail.clone() else { return Command::none(); };
    Command::perform(
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            crate::library_state::load_lock_status(&conn, &detail.variant_id, &user_id)
                .await
                .map_err(|error| error.to_string())
        },
        Message::LibraryLockStatusLoaded,
    )
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p shoebox-client`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/src/main.rs
git -c commit.gpgsign=false commit -m "feat(client): lock status tick, acquire/release/takeover, heartbeat (Plan 1.4b Task 22)"
```

---

## Task 23: Keyboard navigation + trim old `library.rs` placeholder

**Files:**
- Modify: `crates/shoebox-client/src/main.rs`
- Modify: `crates/shoebox-client/src/screens/library.rs`

- [ ] **Step 1: Handle navigation**

```rust
        Message::LibraryKeyboardNavigation(direction) => {
            let total = state.library_view.grid.len();
            let cells_per_row = state.library_view.cells_per_row.max(4);
            let next = crate::library_state::advance_selection(
                state.library_view.selected_grid_index,
                total,
                cells_per_row,
                direction,
            );
            state.library_view.selected_grid_index = next;
            command_for_detail(state)
        }
        Message::LibraryClearError => {
            state.library_view.error = None;
            Command::none()
        }
```

Set `cells_per_row = 4` directly in `LibraryViewState::default()` (override the `Default` derive with an explicit impl) — edit `library_state.rs`:

```rust
impl Default for LibraryViewState {
    fn default() -> Self {
        Self {
            folder_tree: Vec::new(),
            selected_folder_id: None,
            grid: Vec::new(),
            selected_grid_index: None,
            detail: None,
            lock_status: LockStatus::Free,
            error: None,
            cells_per_row: 4,
            keyword_input: String::new(),
        }
    }
}
```

(Remove `#[derive(Default)]` from `LibraryViewState` when you add the manual impl.)

- [ ] **Step 2: Trim `library.rs` to just the helpers used at startup**

Edit `crates/shoebox-client/src/screens/library.rs`. Delete the `view()` function and its imports of `column`, `container`, `row`, `text`, `Element`, `Message`, `ConnectionStatus`. Keep `LibraryStats` and `load_stats` unchanged (still consumed by the steady-state init). The file should look like:

```rust
//! Helper used during startup to surface initial catalog stats. The
//! library screen itself is rendered by `screens::library_view`.

/// Stats loaded once at startup so logs / future telemetry can surface
/// "what we synced". Not displayed in the UI as of Plan 1.4b.
#[derive(Debug, Default, Clone)]
pub struct LibraryStats {
    pub schema_version: i64,
    pub photo_count: i64,
    pub folder_count: i64,
    pub active_user_display_name: String,
    pub frame_no: u64,
}

/// # Errors
/// Returns an error on query failure.
pub async fn load_stats(
    conn: &libsql::Connection,
    active_user_id: Option<&str>,
) -> Result<LibraryStats, anyhow::Error> {
    let mut stats = LibraryStats::default();

    let mut rows = conn
        .query("SELECT COALESCE(MAX(version), 0) FROM _schema_migrations", ())
        .await?;
    if let Some(r) = rows.next().await? {
        stats.schema_version = r.get(0)?;
    }

    let mut rows = conn.query("SELECT COUNT(*) FROM photos", ()).await?;
    if let Some(r) = rows.next().await? {
        stats.photo_count = r.get(0)?;
    }

    let mut rows = conn.query("SELECT COUNT(*) FROM folders", ()).await?;
    if let Some(r) = rows.next().await? {
        stats.folder_count = r.get(0)?;
    }

    if let Some(user_id) = active_user_id {
        let mut rows = conn
            .query("SELECT display_name FROM users WHERE id = ?1", [user_id])
            .await?;
        if let Some(r) = rows.next().await? {
            stats.active_user_display_name = r.get(0)?;
        }
    }
    Ok(stats)
}
```

- [ ] **Step 3: Run the full test + lint suite**

Run: `cargo test -p shoebox-client --lib && cargo clippy -p shoebox-client --all-targets --no-deps -- -D warnings`
Expected: all PASS, no clippy errors.

- [ ] **Step 4: Commit**

```bash
git add crates/shoebox-client/src/main.rs crates/shoebox-client/src/library_state.rs crates/shoebox-client/src/screens/library.rs
git -c commit.gpgsign=false commit -m "feat(client): keyboard nav + trim old library.rs view (Plan 1.4b Task 23)"
```

---

## Task 24: Integration test — `thumb_cache_e2e.rs`

**Files:**
- Create: `crates/shoebox-client/tests/thumb_cache_e2e.rs`

- [ ] **Step 1: Create test**

```rust
//! Standalone e2e — exercises ThumbCache against a counted axum endpoint.
//! Runs on every platform (no sqld dependency).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

fn tiny_jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .unwrap();
    bytes
}

async fn serve(jpeg: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    use axum::{routing::get, Router};
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let jpeg_clone = jpeg.clone();
    let app = Router::new().route(
        "/thumbs/:hash",
        get(move || {
            let counter = counter_clone.clone();
            let jpeg = jpeg_clone.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], jpeg)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), counter)
}

#[tokio::test]
async fn two_gets_one_http_request() {
    let tmp = TempDir::new().unwrap();
    let (url, counter) = serve(tiny_jpeg()).await;
    let cache = shoebox_client::thumb_cache::ThumbCache::new(
        reqwest::Client::new(),
        url,
        tmp.path().to_path_buf(),
    )
    .unwrap();
    assert!(cache.get("abc").await.is_ok());
    assert!(cache.get("abc").await.is_ok());
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
```

Add `axum = { workspace = true }` to `[dev-dependencies]` in `crates/shoebox-client/Cargo.toml` if not already present.

- [ ] **Step 2: Run**

Run: `cargo test -p shoebox-client --test thumb_cache_e2e`
Expected: 1 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/tests/thumb_cache_e2e.rs crates/shoebox-client/Cargo.toml
git -c commit.gpgsign=false commit -m "test(client): thumb_cache_e2e dedup integration test (Plan 1.4b Task 24)"
```

---

## Task 25: Integration test — `library_view_e2e.rs`

**Files:**
- Create: `crates/shoebox-client/tests/library_view_e2e.rs`

- [ ] **Step 1: Create test**

```rust
//! End-to-end: open a server in-process, seed a folder + photos + variants,
//! enroll a client, drive folder/grid/detail loaders + editing actions.
//!
//! Skipped on hosts without `sqld` in PATH (WSL2 build agents).

use shoebox_client::library_state::{
    add_keyword, create_virtual_copy, load_detail, load_folder_tree, load_grid_for_folder,
    upsert_rating,
};

mod common {
    pub use crate::*;
}

#[tokio::test]
async fn folder_tree_grid_rating_keyword_virtual_copy_round_trip() {
    if which::which("sqld").is_err() {
        eprintln!("sqld not found on PATH, skipping");
        return;
    }
    // Pattern mirrors first_run_e2e.rs: spin up a server, enroll a client,
    // open the replica, run loaders.
    let harness = ServerHarness::spawn().await;
    let client_bundle = harness.enroll_client().await;
    seed_demo_catalog(&harness, &client_bundle.user_id).await;

    // Wait for the replica to catch up to the seed (small ticker).
    client_bundle.replica.sync().await.unwrap();

    let conn = client_bundle.replica.conn().unwrap();
    let tree = load_folder_tree(&conn).await.unwrap();
    assert!(!tree.is_empty(), "tree has at least one folder");
    let folder_id = &tree[0].id;

    let cells = load_grid_for_folder(&conn, folder_id, &client_bundle.user_id)
        .await
        .unwrap();
    assert_eq!(cells.len(), 4, "3 masters + 1 virtual copy = 4 cells");

    let first = &cells[0];
    upsert_rating(&conn, &first.variant_id, &client_bundle.user_id, 4).await.unwrap();
    let detail = load_detail(&conn, &first.variant_id, &client_bundle.user_id).await.unwrap();
    assert_eq!(detail.rating, 4);

    add_keyword(&conn, &first.photo_id, &client_bundle.user_id, "tested").await.unwrap();
    let detail = load_detail(&conn, &first.variant_id, &client_bundle.user_id).await.unwrap();
    assert!(detail.keywords.iter().any(|k| k.name == "tested"));

    create_virtual_copy(&conn, &first.photo_id, &client_bundle.user_id).await.unwrap();
    let cells_after = load_grid_for_folder(&conn, folder_id, &client_bundle.user_id).await.unwrap();
    assert_eq!(cells_after.len(), 5);
}

// The harness helpers below mirror those used in first_run_e2e.rs;
// extract them to a shared `tests/common/mod.rs` if you'd prefer.
struct ServerHarness { /* … */ }

struct ClientBundle {
    user_id: String,
    replica: std::sync::Arc<shoebox_client::replica::Replica>,
}

impl ServerHarness {
    async fn spawn() -> Self {
        unimplemented!("copy the spawn helper from tests/first_run_e2e.rs");
    }
    async fn enroll_client(&self) -> ClientBundle {
        unimplemented!("copy the enroll-and-open-replica helper from tests/first_run_e2e.rs");
    }
}

async fn seed_demo_catalog(_harness: &ServerHarness, _user_id: &str) {
    // Insert 1 folder, 3 photos, 4 variants (one photo has a virtual copy).
    // Implementation: drop the appropriate raw files into the server's
    // watched directory so its indexer creates the rows; then manually
    // INSERT one extra variants row via the server's loopback sqld connection.
    unimplemented!("seed via dropping RAW files + insert one virtual copy via sqld");
}
```

**Note for implementer:** Don't actually leave `unimplemented!()` in the final file. Copy `ServerHarness` / `ClientBundle` / `seed_demo_catalog` from `tests/first_run_e2e.rs`, which already does the equivalent spin-up. The skeleton above is the spec for the test; fill in the harness using the existing pattern.

- [ ] **Step 2: Run (will skip if sqld unavailable)**

Run: `cargo test -p shoebox-client --test library_view_e2e -- --nocapture`
Expected: 1 PASS or SKIP (with "sqld not found" message).

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/tests/library_view_e2e.rs
git -c commit.gpgsign=false commit -m "test(client): library_view_e2e folder/grid/detail/edit round-trip (Plan 1.4b Task 25)"
```

---

## Task 26: Integration test — `library_lock_e2e.rs`

**Files:**
- Create: `crates/shoebox-client/tests/library_lock_e2e.rs`

- [ ] **Step 1: Create test**

```rust
//! End-to-end: two enrolled clients exercising all four lock states.
//! Skipped on hosts without `sqld`.

use shoebox_client::library_state::{
    http_acquire_lock, http_release_lock, http_request_takeover, load_lock_status, LockStatus,
};

#[tokio::test]
async fn two_clients_traverse_all_lock_states() {
    if which::which("sqld").is_err() {
        eprintln!("sqld not found on PATH, skipping");
        return;
    }
    // 1. Spawn server + enroll client A and client B with display names
    //    "Alice" and "Bob" respectively. Seed a variant they share.
    let (harness, client_a, client_b, variant_id) = setup_two_clients().await;

    // 2. A acquires.
    let outcome = http_acquire_lock(&client_a.http, &harness.server_url, &variant_id).await.unwrap();
    assert!(matches!(
        outcome,
        shoebox_client::library_state::LockAcquireOutcome::Acquired
    ));

    // 3. Both replicas catch up.
    client_a.replica.sync().await.unwrap();
    client_b.replica.sync().await.unwrap();

    // 4. A sees HeldByYou; B sees HeldByOther { holder: "Alice" }.
    let a_conn = client_a.replica.conn().unwrap();
    let a_status = load_lock_status(&a_conn, &variant_id, &client_a.user_id).await.unwrap();
    assert_eq!(a_status, LockStatus::HeldByYou);
    let b_conn = client_b.replica.conn().unwrap();
    let b_status = load_lock_status(&b_conn, &variant_id, &client_b.user_id).await.unwrap();
    assert_eq!(b_status, LockStatus::HeldByOther { holder_display_name: "Alice".into() });

    // 5. B requests takeover.
    http_request_takeover(&client_b.http, &harness.server_url, &variant_id).await.unwrap();
    client_a.replica.sync().await.unwrap();
    client_b.replica.sync().await.unwrap();
    let a_status = load_lock_status(&a_conn, &variant_id, &client_a.user_id).await.unwrap();
    assert_eq!(a_status, LockStatus::HeldByYouTakeoverPending { requested_by_display_name: "Bob".into() });
    let b_status = load_lock_status(&b_conn, &variant_id, &client_b.user_id).await.unwrap();
    assert_eq!(b_status, LockStatus::HeldByOtherTakeoverPending { holder_display_name: "Alice".into() });

    // 6. A releases. Both go to Free.
    http_release_lock(&client_a.http, &harness.server_url, &variant_id).await.unwrap();
    client_a.replica.sync().await.unwrap();
    client_b.replica.sync().await.unwrap();
    let a_status = load_lock_status(&a_conn, &variant_id, &client_a.user_id).await.unwrap();
    assert_eq!(a_status, LockStatus::Free);
    let b_status = load_lock_status(&b_conn, &variant_id, &client_b.user_id).await.unwrap();
    assert_eq!(b_status, LockStatus::Free);
}

struct Harness {
    server_url: String,
    // …
}
struct Client {
    user_id: String,
    http: reqwest::Client,
    replica: std::sync::Arc<shoebox_client::replica::Replica>,
}

async fn setup_two_clients() -> (Harness, Client, Client, String) {
    unimplemented!(
        "Spin up a server (copy from tests/first_run_e2e.rs). Enroll client A and B \
         using the existing /enroll path with two different display names. Seed one folder + \
         one photo + one variant. Return (harness, client_a, client_b, variant_id)."
    );
}
```

Same note as Task 25: replace `unimplemented!()` with the actual harness composed from `first_run_e2e.rs` patterns.

- [ ] **Step 2: Run**

Run: `cargo test -p shoebox-client --test library_lock_e2e -- --nocapture`
Expected: 1 PASS or SKIP.

- [ ] **Step 3: Commit**

```bash
git add crates/shoebox-client/tests/library_lock_e2e.rs
git -c commit.gpgsign=false commit -m "test(client): library_lock_e2e covers all four lock states (Plan 1.4b Task 26)"
```

---

## Task 27: CLAUDE.md update + final smoke

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update sub-project status row**

In `CLAUDE.md`, update the table row for sub-project #1 from:

> Plan 1.4 implemented … Plans 1.4b (demo library view) and 1.5 (deployment) pending.

to:

> Plans 1.1–1.4b implemented. Plan 1.5 (deployment) pending.

In the `## Implementation status` section, append below the `crates/shoebox-client` bullet block:

```
- `crates/shoebox-client` — demo library view (Plan 1.4b):
  - Three-pane Library screen: folder tree / photo grid with thumbnails / EXIF + edit detail panel
  - `ThumbCache`: in-memory LRU (1024) + on-disk JPEG cache; mTLS fetch from `<server>/thumbs/<hash>`
  - Editing actions through local replica: rate (per-user UPSERT), keyword add/remove (race-resolved), virtual copy
  - Develop-lock UI: 5 s status poll from local replica; acquire/release/takeover via `/locks/:id`; 5 min heartbeat
  - Keyboard: arrows navigate grid; 0-5 set rating on selected variant
```

And in `## Known limitations`, append:

```
- **No grid virtualization.** Folders with thousands of photos will render slowly. Plan 1.4b grids ~30-photo test sets cleanly; full virtualization is sub-project #3.
- **Lock UI surfaces 4 states, no auto-release on app exit.** Releasing a lock requires the user clicking Release; if the app dies, the lock expires via the server janitor's 30 min TTL instead.
```

- [ ] **Step 2: Run final smoke**

Run these in parallel and confirm clean:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo test -p shoebox-client --lib
cargo test -p shoebox-client --tests
```

Expected: all green. The two sqld-gated e2e tests will print "sqld not found" on a WSL2 dev box; that's acceptable here.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git -c commit.gpgsign=false commit -m "docs(claude.md): mark Plan 1.4b complete; note grid virtualization + lock auto-release limitations"
```

---

## Self-review notes

**Spec coverage:**

| Spec section | Tasks |
|---|---|
| §3.1 file layout | Tasks 2 (thumb_cache), 5 (library_state), 14–18 (screens/library_view + state), 23 (trim library.rs) |
| §3.2 module responsibilities | Tasks 2–18 |
| §3.3 message variants | Task 14 |
| §4.1 initial load flow | Task 19 |
| §4.2 selection flow | Task 20 (grid sel), Task 21 (detail), Task 22 (lock) |
| §4.3 editing actions | Tasks 10–12 helpers, Task 21 handlers |
| §4.4 develop locks | Task 9 (read), Task 13 (HTTP), Task 22 (UI handlers + heartbeat) |
| §5 error table | Inline in Tasks 19–22 (each handler maps Err → `state.library_view.error = …`) |
| §6.1 unit tests | Tasks 2–12 each include their own tests |
| §6.2 integration tests | Tasks 24–26 |
| §6.3 manual smoke | Outside this plan (cross-platform) |

**Type-consistency check:** All IDs are `String` (matches TEXT schema); `variant_index` is `i64`; `rating` is `u8` at the boundary, `i64` in SQL via `i64::from(rating)`. `LockStatus` variants match between `library_state.rs`, `detail_panel.rs`, and the lock handlers in `main.rs`. Helper names (`upsert_rating`, `add_keyword`, `remove_keyword`, `create_virtual_copy`, `load_*`, `http_*_lock`) are consistent across tasks.

**Placeholder scan:** The two integration tests (Tasks 25–26) use `unimplemented!()` for the harness setup with an explicit "copy from `tests/first_run_e2e.rs`" instruction. This is intentional — the harness already exists in the test crate and shouldn't be retyped here; the implementer fills in by copying. Everywhere else, full code is inline.
