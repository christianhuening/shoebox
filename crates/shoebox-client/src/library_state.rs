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
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    pub variant_id: String,
    pub photo_id: String,
    pub variant_index: i64,
    pub display_name: String,
    pub rating: u8,
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

#[derive(Debug, Clone)]
pub struct DetailLoaded {
    pub variant_id: String,
    pub photo_id: String,
    pub exif: ExifSummary,
    pub rating: u8,
    pub keywords: Vec<KeywordRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LockStatus {
    #[default]
    Free,
    HeldByYou,
    HeldByYouTakeoverPending { requested_by_display_name: String },
    HeldByOther { holder_display_name: String },
    HeldByOtherTakeoverPending { holder_display_name: String },
}

#[derive(Debug, Clone, Default)]
pub struct LibraryViewState {
    pub folder_tree: Vec<FolderRow>,
    pub selected_folder_id: Option<String>,
    pub grid: Vec<GridCell>,
    pub selected_grid_index: Option<usize>,
    pub detail: Option<DetailLoaded>,
    pub lock_status: LockStatus,
    pub error: Option<String>,
    pub cells_per_row: usize,
}

#[allow(dead_code)]
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
    type RawRow = (String, Option<String>, String);
    type ChildMap<'m> = HashMap<Option<String>, Vec<&'m RawRow>>;
    let mut children: ChildMap<'_> = HashMap::new();
    for row in raw {
        children.entry(row.1.clone()).or_default().push(row);
    }
    let mut out = Vec::with_capacity(raw.len());
    #[allow(clippy::items_after_statements)]
    fn walk<'a>(
        parent: Option<&'a String>,
        depth: usize,
        children: &ChildMap<'a>,
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

/// Load all variants (master + virtual copies) for photos whose
/// `photo_files.folder_id = folder_id`. Each variant is one grid cell.
/// Cells are ordered by `(captured_at, photo_id, variant_index)`.
pub async fn load_grid_for_folder(
    conn: &libsql::Connection,
    folder_id: &str,
    user_id: &str,
) -> Result<Vec<GridCell>> {
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
            [photo_id.as_str()],
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

/// Inputs needed by the pure decoder to produce a `LockStatus`.
#[derive(Debug, Clone)]
pub struct LockRowSnapshot {
    pub holder_user_id: String,
    pub holder_display_name: String,
    pub takeover_requested_by: Option<String>,
    pub takeover_requested_by_display_name: Option<String>,
}

#[must_use]
#[allow(clippy::match_same_arms)]
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
            .map_or(0, |duration| duration.as_millis()),
    )
    .unwrap_or(0)
}

/// Attach a keyword to a photo. Creates the root-level keyword if it
/// doesn't already exist. On UNIQUE conflict (a concurrent insert won),
/// resolves the existing `keyword_id` and proceeds.
///
/// Note: `SQLite` treats two NULLs as distinct in UNIQUE indexes, so
/// `UNIQUE(parent_id, name)` does not prevent duplicate root keywords via
/// conflict alone. We use a SELECT-first strategy for root keywords to
/// guarantee at-most-one row, then fall back to INSERT if absent.
pub async fn add_keyword(
    conn: &libsql::Connection,
    photo_id: &str,
    user_id: &str,
    name: &str,
) -> Result<String> {
    let now_ms = now_unix_ms();

    // SELECT-first for root keywords (NULL parent) to avoid SQLite's
    // NULL-distinct UNIQUE behaviour creating duplicates.
    let mut existing_rows = conn
        .query(
            "SELECT id FROM keywords WHERE parent_id IS NULL AND name = ?1",
            [name],
        )
        .await
        .context("checking for existing keyword")?;

    let keyword_id = if let Some(existing_row) = existing_rows.next().await? {
        existing_row.get::<String>(0)?
    } else {
        let new_id = uuid_v4_hex();
        let insert_result = conn
            .execute(
                "INSERT INTO keywords(id, parent_id, name, created_at) VALUES (?1, NULL, ?2, ?3)",
                (new_id.as_str(), name, now_ms),
            )
            .await;

        match insert_result {
            Ok(_) => new_id,
            Err(error) => {
                let msg = error.to_string().to_lowercase();
                if !(msg.contains("unique") || msg.contains("constraint")) {
                    return Err(error).context("inserting keyword");
                }
                // Lost a race: another writer inserted between our SELECT
                // and our INSERT. Re-query for the winner's id.
                let mut rows = conn
                    .query(
                        "SELECT id FROM keywords WHERE parent_id IS NULL AND name = ?1",
                        [name],
                    )
                    .await?;
                let resolved = rows
                    .next()
                    .await?
                    .context("keyword INSERT failed UNIQUE but no row found")?;
                resolved.get::<String>(0)?
            }
        }
    };

    conn.execute(
        "INSERT OR IGNORE INTO photo_keywords(photo_id, keyword_id, added_by, added_at)
         VALUES (?1, ?2, ?3, ?4)",
        (photo_id, keyword_id.as_str(), user_id, now_ms),
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

/// Create a virtual copy of `photo_id` by cloning the next `variant_index`.
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
            new_id.as_str(),
            photo_id,
            next_index,
            user_id,
            now_ms,
            parent_json.as_str(),
            parent_version,
        ),
    )
    .await
    .context("creating virtual copy")?;
    Ok(new_id)
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
    #[allow(clippy::too_many_lines)]
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
}
