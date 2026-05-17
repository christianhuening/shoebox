# Sub-project 1.4b — Demo Library View Design

**Status:** Approved (2026-05-18)
**Parent spec:** `2026-05-17-sub-1-4-desktop-client-design.md`
**Predecessor plan:** `2026-05-17-sub-1-4-desktop-client-foundation.md` (executed)
**Scope tag in parent spec:** §11.1 "shoebox-client" — finishes the foundation+library row

---

## 1. Goal

Replace the placeholder Library screen shipped in Plan 1.4 with a usable
three-pane demo library: folder tree on the left, photo grid in the middle,
EXIF + edit-state panel on the right. Wire up the catalog-mutating actions
called out in the parent spec — rate (per-user), keyword (shared), virtual
copy — and surface develop-lock state so two-machine collaboration is
visible end-to-end.

This is the last bit of the `shoebox-client` row in the parent spec's §11.1
implementation matrix. After 1.4b, sub-project #1 (catalog + sync + stack)
is done except for Plan 1.5 (deployment).

## 2. Non-goals

- Full library browser performance work (10k+ photos, virtualized grid,
  search, filter, drag-and-drop, multi-select batch ops). Those are
  sub-project #3.
- Full-screen viewer or 1:1 pixel zoom. Sub-projects #3 / #4.
- Any develop sliders or curves. Sub-project #4.
- Export pipeline. Sub-project #5.
- XMP sidecar writer. Deferred (see `project_xmp_sidecar_exporter_deferred`).

## 3. Architecture

### 3.1 Where 1.4b lives in the Plan 1.4 client

The Plan 1.4 client is a single Iced `Application` driven by a `Screen` enum
state machine. Plan 1.4b adds one new screen variant (`Library`, replacing
the placeholder) plus the supporting modules:

```
crates/shoebox-client/src/
├── main.rs                       (existing — extend with new Message arms)
├── replica.rs                    (existing — read via fresh Connection per query)
├── mtls_http.rs                  (existing — used for lock + thumb HTTP)
├── thumb_cache.rs                (NEW)
├── library_state.rs              (NEW — DB query helpers + LibraryViewState)
└── screens/
    ├── mod.rs                    (existing — extend Screen + Message)
    ├── library_view/             (NEW directory)
    │   ├── mod.rs                (view() entry + keyboard handler)
    │   ├── folder_tree.rs        (pure view of the tree pane)
    │   ├── photo_grid.rs         (pure view of the grid pane)
    │   └── detail_panel.rs       (pure view of the EXIF/edit pane)
```

### 3.2 Module responsibilities

- **`thumb_cache.rs`** — `ThumbCache` struct. In-memory LRU (1024 entries
  via `lru` crate) over `Arc<image::DynamicImage>` keyed by `blake3` hash;
  disk-backed under `~/.local/share/shoebox-client/thumbs/<hash>.jpg` (or
  the equivalent `directories::ProjectDirs` path on macOS/Windows). On
  `get(hash)`: in-memory hit → return; in-flight dedup map → await existing
  future; disk hit → decode + insert into memory cache; cold miss → HTTPS
  GET to `<server>/thumbs/<hash>` via the existing `mtls_http` client,
  persist to disk, decode, insert. Returns `Result<Arc<DynamicImage>, ThumbError>`.

- **`library_state.rs`** — Two things:
  1. The `LibraryViewState` struct held in `AppState` while the Library
     screen is open: `folder_tree: Vec<FolderRow>`, `selected_folder_id:
     Option<i64>`, `grid: Vec<GridCell>`, `selected_grid_index: Option<usize>`,
     `detail: Option<DetailLoaded>`, `lock_status: LockStatus`, plus
     in-flight loaders to deduplicate.
  2. Async DB-query helpers — `load_folder_tree(&Replica)`,
     `load_grid_for_folder(&Replica, folder_id)`, `load_detail(&Replica,
     variant_id, user_id)`, `load_lock_status(&Replica, variant_id, user_id)`,
     `lock_status_from_row(row, user_id) -> LockStatus`. These return
     domain types; they do not touch Iced.

- **`screens/library_view/mod.rs`** — `view(state: &AppState) -> Element<Message>`
  composes the three pure-view modules in an Iced `row![tree, grid, detail]`
  with the appropriate fill weights (parent spec §7.6 mockup: ~220 px
  fixed-width tree, flexible grid, ~320 px fixed-width detail). Also owns
  the keyboard subscription that turns arrow keys + `0`-`5` into Messages.

- **`screens/library_view/folder_tree.rs`** — Pure view. Scrollable column
  of `button(text("  ".repeat(depth) + name))`-style rows. Emits
  `Message::LibraryFolderSelected(folder_id)`.

- **`screens/library_view/photo_grid.rs`** — Pure view. `wrap`-style grid
  of fixed 256 px tiles. Each tile shows the thumbnail (placeholder rect
  if not yet loaded), a 5-star widget, and the keyword count. Selected
  tile gets a 2 px accent border. Emits `LibraryGridCellSelected(index)`
  and `LibraryRatingChanged { variant_id, rating }`.

- **`screens/library_view/detail_panel.rs`** — Pure view. Shows the EXIF
  table, full keyword list with add/remove controls, "New virtual copy"
  button, and the lock-status banner (Free / HeldByYou / HeldByOther
  with optional Request-takeover button / HeldByYouTakeoverPending /
  TakeoverRequestedOfMe).

### 3.3 New Message variants (added to `screens::Message`)

```
LibraryFolderTreeLoaded(Result<Vec<FolderRow>>)
LibraryFolderSelected(i64)
LibraryGridLoaded { folder_id: i64, cells: Result<Vec<GridCell>> }
LibraryThumbReady { hash: String, image: Arc<DynamicImage> }   // posted from ThumbCache loader
LibraryGridCellSelected(usize)
LibraryDetailLoaded(Result<DetailLoaded>)
LibraryRatingChanged { variant_id: i64, rating: u8 }
LibraryRatingPersisted(Result<()>)
LibraryKeywordAdded { variant_id: i64, name: String }
LibraryKeywordPersisted(Result<()>)
LibraryKeywordRemoveClicked { variant_id: i64, keyword_id: i64 }
LibraryKeywordRemovePersisted(Result<()>)
LibraryNewVirtualCopyClicked(i64)                              // photo_id of selected variant
LibraryVirtualCopyPersisted(Result<i64>)
LibraryLockStatusTick                                          // 5 s poll
LibraryLockStatusLoaded(Result<LockStatus>)
LibraryAcquireLockClicked(i64)                                 // variant_id
LibraryRequestTakeoverClicked(i64)
LibraryReleaseLockClicked(i64)
LibraryLockActionPersisted(Result<()>)
LibraryLockHeartbeatTick                                       // 5 min — when we hold a lock
LibraryKeyboardNavigation(NavigationDirection)                 // Up/Down/Left/Right
LibraryKeyboardRating(u8)                                      // 0-5
```

## 4. Data flow

### 4.1 Initial library open
After Plan 1.4's ProfilePicker confirms a user, the controller fires:

1. `Command::perform(load_folder_tree, LibraryFolderTreeLoaded)`.
2. On success, transitions `Screen::Library` and auto-selects the first
   root folder, firing `LibraryFolderSelected(first_folder_id)`.
3. That triggers `Command::perform(load_grid_for_folder, LibraryGridLoaded)`.
4. On grid load, the controller batches `Command::perform(thumb_cache.get(hash), …)`
   for every cell (the ThumbCache deduplicates and rate-limits internally).
5. Each completed thumb fires `LibraryThumbReady` which mutates the
   in-memory grid cell.

### 4.2 Selection
On `LibraryGridCellSelected(index)`:

1. Update `selected_grid_index`.
2. Fire `Command::batch([load_detail, load_lock_status])` for the selected
   variant.
3. Subscribe (via the existing Iced subscription bus) to a 5 s lock-status
   polling tick keyed on the selected variant.

### 4.3 Editing actions
All go through fresh `Replica.conn()` writes (libsql commits replicate up
to the server via the same WAL push the 30 s catchup uses):

- **Rating** — `INSERT INTO variant_user_state(variant_id,user_id,rating)
  VALUES(?,?,?) ON CONFLICT(variant_id,user_id) DO UPDATE SET rating=excluded.rating`.
- **Keyword add** — Two-step in a single transaction: try `INSERT INTO
  keywords(name) VALUES(?) RETURNING keyword_id`; on UNIQUE conflict
  fall back to `SELECT keyword_id FROM keywords WHERE name=?`; then
  `INSERT OR IGNORE INTO photo_keywords(photo_id,keyword_id) VALUES(?,?)`.
- **Keyword remove** — `DELETE FROM photo_keywords WHERE photo_id=? AND
  keyword_id=?`.
- **Virtual copy** — `INSERT INTO variants(photo_id,parent_variant_id,
  copy_index,is_master,develop_settings) VALUES(?,?,next,0,?)` where
  `next = MAX(copy_index)+1` for that photo and `develop_settings` is the
  parent's settings JSON.

After each persist, reload the affected slice (detail panel for
rating/keywords, grid for virtual copy) from the local replica so the UI
reflects the committed state, not the optimistic guess.

### 4.4 Develop locks
Read state from the local replica's `develop_locks` table (every 5 s while
a variant is selected). Writes go through the server endpoints because
they enforce policy:

- Acquire → `POST /locks/<variant_id>` with mTLS client.
- Heartbeat → `POST /locks/<variant_id>/heartbeat` every 5 min while we
  hold the lock. Stops when we release or the selection changes.
- Release → `DELETE /locks/<variant_id>`.
- Request takeover → `POST /locks/<variant_id>/takeover`.

The server's writes propagate down through libsql replication, so the
5 s read-poll picks them up without a separate notification channel.

## 5. Error handling

| Failure | UI behavior |
|---|---|
| `load_folder_tree` Err | Library screen shows centered error card with "Retry". |
| `load_grid_for_folder` Err | Grid pane shows error card with "Retry"; tree + detail still usable. |
| `thumb_cache.get` HTTP error | Tile keeps placeholder; small overlay "no thumbnail"; click to retry that one tile. (Do not block grid render.) |
| `load_detail` Err | Detail pane shows error card with "Retry"; grid + tree still usable. |
| Rating persist Err | Toast "couldn't save rating"; revert optimistic UI; offer retry. |
| Keyword add Err | Same pattern — toast + revert + retry. |
| Keyword remove Err | Same. |
| Virtual copy Err | Toast "couldn't create copy"; no grid change. |
| Lock acquire 409 | NOT an error — server says someone else holds it. Re-read lock status and update banner to `HeldByOther`. |
| Lock acquire 5xx / transport | Toast "lock service unreachable"; banner stays as last-known state. |
| Heartbeat Err | Log; one retry on next tick; if two consecutive heartbeats fail, banner switches to "lock heartbeat failed — your lock may be expiring" with a manual "Reacquire" button. |
| Local replica corrupted at startup | Same handler as Plan 1.4: boot back to Discovery with the existing reset path. |

Reads from the local replica do not retry across reconnects — if the
local libSQL file errors, it's a hard failure that propagates up.

## 6. Testing

### 6.1 Unit tests (`#[cfg(test)] mod tests`)

| Module | What's tested |
|---|---|
| `thumb_cache` | Hot-cache hit; in-flight dedup; disk-cache hit on cold memory; LRU eviction at capacity. Uses a stub axum endpoint serving canned JPEG bytes (same pattern as `mtls_http` tests). |
| `library_state` | `load_folder_tree` builds correct flat-indented order from a seeded `folders` table; `load_grid_for_folder` filters + joins correctly; `load_rating` returns `None` when no `variant_user_state` row exists. Uses tempdir + real `Db::open` for an in-memory libsql. |
| Keyword race-resolution helper | UNIQUE conflict path resolves to the existing `keyword_id`; concurrent insertions converge. |
| Keyboard message translator | `Key::Named(ArrowRight)` advances selected index by 1; `Down` advances by `cells_per_row`; `0`-`5` map to `LibraryKeyboardRating(0..=5)`. |
| `lock_status_from_row` | All four states (Free, HeldByYou, HeldByOther, HeldByYouTakeoverPending) derive correctly from row fields + current user_id. |

### 6.2 Integration tests (`crates/shoebox-client/tests/`)

- **`library_view_e2e.rs`** — spawn `shoebox-server` in-process (Plan 1.4
  harness). Seed catalog with 1 folder, 3 photos, 2 variants on one of
  them. Enroll a client + open the replica. Drive the loaders directly:
  folder tree → grid → rating UPSERT → keyword add → virtual copy. Assert
  each read after each write returns the expected shape.
- **`library_lock_e2e.rs`** — same harness, two enrolled clients sharing
  one server. Client A acquires; both replicas catch up; assert
  `load_lock_status` returns `HeldByYou` on A and `HeldByOther` on B.
  B requests takeover; assert A sees `HeldByYouTakeoverPending`. A
  releases; both see `Free`.
- **`thumb_cache_e2e.rs`** — standalone (no `sqld` needed). Tiny axum
  server returns canned JPEG bytes on `/thumbs/<hash>`; assert two `get`
  calls hit the server exactly once.

The first two skip on this WSL2 host alongside the Plan 1.4 e2e tests
(they require running `sqld`).

### 6.3 Manual / cross-platform smoke

On Linux + macOS + Windows desktops:
- Drop ~30 RAW files into a server-watched folder. Open the client. Grid
  populates with thumbnails within ~10 s after server-side thumbnailing.
- Click around: EXIF + rating + keyword data updates. Rate with mouse
  and with 0-5 keys. Restart app: ratings persist.
- Add keyword "test"; restart; keyword still on the variant.
- Create a virtual copy; new grid cell appears with `(2)` suffix.
- Two-machine flow: machine A acquires lock; machine B sees "Held by …"
  banner; B requests takeover; A sees takeover banner; A releases; both
  banners go to Free cleanly.
- Arrow keys navigate the grid; 0-5 rate the selected cell.
- Kill server mid-session: cached folder/grid/EXIF still visible; writes
  fail with inline errors; restart server; next 30 s catchup rejoins.
- Revoke cert server-side: next sync boots client back to Discovery
  (Plan 1.4 behavior preserved).

### 6.4 Out of scope for tests

- Iced UI snapshot / golden-image tests. Iced's testing story is too
  weak; the screen modules are thin enough that manual + module unit
  tests cover correctness.
- Performance under 10,000+ photo folders. Explicit sub-project #3 scope.
- Drag-and-drop, multi-select batch ops, search/filter. Sub-project #3.
- Full-screen view, 1:1 pixel zoom. Sub-projects #3 / #4.

## 7. Backlog (deferred from 1.4b)

- Smart-folder / saved-search support.
- Reordering within virtual-copy stacks via drag.
- Bulk keyword apply across selection.
- Folder rename + move from the tree (catalog operation, not filesystem).
- Custom thumbnail sort keys beyond filename (capture time, rating, …).
- Inline filter chips above the grid (rating ≥ N, has-keyword, etc.).
- All of the above are sub-project #3 territory.
