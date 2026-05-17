//! Filesystem indexer. Task 7 of Plan 1.3 contributes the initial-scan
//! path: walk the photo root, BLAKE3-hash every RAW file, populate
//! `folders` + `photos` + `photo_files` rows. Task 8 adds the live
//! `notify`-based FS watcher (`run_watcher`) that reacts to incremental
//! create/modify/remove events using the same upsert logic. Task 9 wires
//! both into `main.rs`.

use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use crate::db::Db;
use crate::hashing;

/// File extensions treated as RAW originals. Match is case-insensitive.
pub const RAW_EXTENSIONS: &[&str] = &["pef", "dng", "raf"];

#[derive(Debug, Clone, Default)]
pub struct IndexerStats {
    pub folders_seen: usize,
    pub files_seen: usize,
    pub photos_added: usize,
    pub photo_files_added: usize,
    pub photo_files_updated: usize,
}

/// Walk `photos_root` and ingest every RAW file beneath it into the catalog.
///
/// For each RAW file found:
/// - the folder chain from `photos_root` down to the file's directory is
///   materialised in the `folders` table (idempotent).
/// - the file is BLAKE3-hashed (off the async runtime via `spawn_blocking`).
/// - a `photos` row is inserted keyed by the hash if one doesn't already exist.
/// - a `photo_files` row is inserted (or its `file_mtime`/`last_seen_at`/`is_present`
///   refreshed) keyed by the file's full path.
///
/// # Errors
///
/// Returns an error if directory traversal fails fatally, if hashing fails,
/// or if any catalog upsert fails.
pub async fn initial_scan(db: Arc<Db>, photos_root: &Path) -> Result<IndexerStats> {
    let mut stats = IndexerStats::default();

    let mut known_folder_paths: HashSet<PathBuf> = HashSet::new();
    known_folder_paths.insert(photos_root.to_path_buf());

    for entry in WalkDir::new(photos_root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_dir() {
            known_folder_paths.insert(entry.path().to_path_buf());
            stats.folders_seen += 1;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        stats.files_seen += 1;
        if !is_raw_file(entry.path()) {
            continue;
        }

        if let Some(parent_dir) = entry.path().parent() {
            ensure_folder_chain(&db, photos_root, parent_dir).await?;
        }

        let file_path = entry.path();
        let metadata = entry.metadata()?;
        let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        let file_mtime = i64::try_from(
            metadata
                .modified()
                .ok()
                .and_then(|modified_time| modified_time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_millis()),
        )
        .unwrap_or(0);

        let path_to_hash = file_path.to_path_buf();
        let hash_hex =
            tokio::task::spawn_blocking(move || hashing::blake3_hex(&path_to_hash)).await??;

        let outcome = upsert_photo_and_file(&db, &hash_hex, file_size, file_path, file_mtime)
            .await
            .with_context(|| format!("upserting {}", file_path.display()))?;

        match outcome {
            UpsertOutcome::PhotoAndFileNew => {
                stats.photos_added += 1;
                stats.photo_files_added += 1;
            }
            UpsertOutcome::FileNew => stats.photo_files_added += 1,
            UpsertOutcome::FileUpdated => stats.photo_files_updated += 1,
            UpsertOutcome::NoChange => {}
        }
    }

    Ok(stats)
}

#[derive(Debug)]
enum UpsertOutcome {
    PhotoAndFileNew,
    FileNew,
    FileUpdated,
    NoChange,
}

async fn upsert_photo_and_file(
    db: &Db,
    hash_hex: &str,
    file_size: i64,
    path: &Path,
    file_mtime: i64,
) -> Result<UpsertOutcome> {
    let conn = db.connect()?;
    let now_ms_v = now_ms();

    // photos row: insert if absent (keyed by content hash).
    let mut existing_photo_rows = conn
        .query("SELECT 1 FROM photos WHERE id = ?1", [hash_hex])
        .await?;
    let photo_existed = existing_photo_rows.next().await?.is_some();
    if !photo_existed {
        let format = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_uppercase)
            .unwrap_or_default();
        conn.execute(
            "INSERT INTO photos (id, file_size, file_format, imported_at) \
             VALUES (?1, ?2, ?3, ?4)",
            (hash_hex.to_string(), file_size, format, now_ms_v),
        )
        .await?;
    }

    // photo_files row: insert if new, refresh mtime/last_seen_at/is_present otherwise.
    let path_str = path.to_string_lossy().to_string();
    let mut existing_file_rows = conn
        .query(
            "SELECT id, file_mtime FROM photo_files WHERE path = ?1",
            [path_str.clone()],
        )
        .await?;
    if let Some(existing_row) = existing_file_rows.next().await? {
        let existing_id: String = existing_row.get(0)?;
        let existing_mtime: i64 = existing_row.get(1)?;
        conn.execute(
            "UPDATE photo_files SET file_mtime = ?1, last_seen_at = ?2, is_present = 1 \
             WHERE id = ?3",
            (file_mtime, now_ms_v, existing_id),
        )
        .await?;
        if existing_mtime == file_mtime {
            Ok(UpsertOutcome::NoChange)
        } else {
            Ok(UpsertOutcome::FileUpdated)
        }
    } else {
        let parent_path_str = path
            .parent()
            .map(|parent_path| parent_path.to_string_lossy().to_string())
            .unwrap_or_default();
        let folder_id: String = {
            let mut folder_rows = conn
                .query(
                    "SELECT id FROM folders WHERE path = ?1",
                    [parent_path_str.clone()],
                )
                .await?;
            let folder_row = folder_rows
                .next()
                .await?
                .ok_or_else(|| anyhow!("folder row missing for {parent_path_str}"))?;
            folder_row.get(0)?
        };
        let new_file_id = uuid_hex();
        conn.execute(
            "INSERT INTO photo_files \
             (id, photo_id, folder_id, path, file_mtime, last_seen_at, is_present) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            (
                new_file_id,
                hash_hex.to_string(),
                folder_id,
                path_str,
                file_mtime,
                now_ms_v,
            ),
        )
        .await?;
        if photo_existed {
            Ok(UpsertOutcome::FileNew)
        } else {
            Ok(UpsertOutcome::PhotoAndFileNew)
        }
    }
}

/// Insert any missing `folders` rows from `photos_root` down through `dir`.
///
/// Walks upward from `dir`, collecting paths that are inside the photo root
/// and don't yet have a row, then inserts them in root-first order so that
/// each child's `parent_id` foreign key is satisfied at insert time.
async fn ensure_folder_chain(db: &Db, photos_root: &Path, dir: &Path) -> Result<()> {
    let conn = db.connect()?;
    let mut paths_to_insert: Vec<PathBuf> = Vec::new();
    let mut current_dir = Some(dir.to_path_buf());

    while let Some(candidate) = current_dir {
        if candidate == photos_root || candidate.starts_with(photos_root) {
            let candidate_str = candidate.to_string_lossy().to_string();
            let mut existing_rows = conn
                .query(
                    "SELECT 1 FROM folders WHERE path = ?1",
                    [candidate_str.clone()],
                )
                .await?;
            if existing_rows.next().await?.is_some() {
                break;
            }
            paths_to_insert.push(candidate.clone());
        } else {
            // We've walked above photos_root; stop without inserting.
            break;
        }
        let next = candidate.parent().map(Path::to_path_buf);
        // Stop once we've reached photos_root's parent: nothing above the
        // root should be materialised.
        if let Some(ref next_path) = next {
            if next_path.as_path() == photos_root.parent().unwrap_or(Path::new("/")) {
                current_dir = None;
                continue;
            }
        }
        current_dir = next;
    }

    paths_to_insert.reverse();
    let now_ms_v = now_ms();
    for path in paths_to_insert {
        let path_str = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|name_os| name_os.to_string_lossy().to_string())
            .unwrap_or_default();
        let parent_id: Option<String> = match path.parent() {
            Some(parent_path) => {
                let parent_str = parent_path.to_string_lossy().to_string();
                let mut parent_rows = conn
                    .query("SELECT id FROM folders WHERE path = ?1", [parent_str])
                    .await?;
                match parent_rows.next().await? {
                    Some(parent_row) => Some(parent_row.get::<String>(0)?),
                    None => None,
                }
            }
            None => None,
        };
        let new_folder_id = uuid_hex();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name, last_indexed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (new_folder_id, parent_id, path_str, name, now_ms_v),
        )
        .await?;
    }
    Ok(())
}

fn is_raw_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            RAW_EXTENSIONS
                .iter()
                .any(|raw| raw.eq_ignore_ascii_case(extension))
        })
}

fn uuid_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Run the live filesystem watcher loop. Returns only on error or shutdown.
///
/// Subscribes (recursively) to `photos_root` via the `notify` crate and
/// reacts to create/modify/remove events by calling into the same upsert
/// logic as [`initial_scan`]. The loop also terminates when `shutdown`
/// resolves (or its sender is dropped), letting callers cleanly stop the
/// watcher during server shutdown.
///
/// # Errors
///
/// Returns an error if the watcher cannot be constructed or fails to
/// register a watch on `photos_root`. Per-event handler errors are logged
/// at warn level and do not terminate the loop.
pub async fn run_watcher(
    db: Arc<Db>,
    photos_root: PathBuf,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    use notify::{RecommendedWatcher, RecursiveMode, Watcher};
    use tokio::sync::mpsc;

    let (event_sender, mut event_receiver) =
        mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |watch_result| {
        let _ = event_sender.send(watch_result);
    })?;
    watcher.watch(&photos_root, RecursiveMode::Recursive)?;

    tracing::info!(
        event = "indexer.watcher.start",
        photos_root = %photos_root.display(),
        "filesystem watcher started"
    );

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "indexer.watcher.shutdown");
                break;
            }
            event_result = event_receiver.recv() => match event_result {
                Some(Ok(event)) => {
                    if let Err(handle_err) = handle_event(&db, &photos_root, &event).await {
                        tracing::warn!(
                            event = "indexer.handle.error",
                            error = %handle_err
                        );
                    }
                }
                Some(Err(watcher_err)) => tracing::warn!(
                    event = "indexer.watch.error",
                    error = %watcher_err
                ),
                None => break,
            }
        }
    }

    // Keep the watcher alive until the end of the function so events are
    // delivered for the entire loop lifetime.
    drop(watcher);
    Ok(())
}

async fn handle_event(db: &Db, photos_root: &Path, event: &notify::Event) -> Result<()> {
    use notify::EventKind;
    for path in &event.paths {
        if !is_raw_file(path) {
            continue;
        }
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                if !path.is_file() {
                    continue;
                }
                let metadata = std::fs::metadata(path)?;
                let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
                let file_mtime = i64::try_from(
                    metadata
                        .modified()
                        .ok()
                        .and_then(|modified_time| {
                            modified_time.duration_since(std::time::UNIX_EPOCH).ok()
                        })
                        .map_or(0, |duration| duration.as_millis()),
                )
                .unwrap_or(0);
                if let Some(parent_dir) = path.parent() {
                    ensure_folder_chain(db, photos_root, parent_dir).await?;
                }
                let path_to_hash = path.clone();
                let hash_hex =
                    tokio::task::spawn_blocking(move || hashing::blake3_hex(&path_to_hash))
                        .await??;
                upsert_photo_and_file(db, &hash_hex, file_size, path, file_mtime).await?;
            }
            EventKind::Remove(_) => {
                let conn = db.connect()?;
                let path_str = path.to_string_lossy().to_string();
                conn.execute(
                    "UPDATE photo_files SET is_present = 0 WHERE path = ?1",
                    [path_str],
                )
                .await?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[tokio::test]
    async fn initial_scan_picks_up_raw_files() {
        let tmp = TempDir::new().unwrap();
        let photos = tmp.path().join("photos");
        fs::create_dir_all(photos.join("2024")).unwrap();
        fs::create_dir_all(photos.join("2025")).unwrap();
        File::create(photos.join("2024/_DSC0001.PEF")).unwrap();
        File::create(photos.join("2024/_DSC0002.PEF")).unwrap();
        File::create(photos.join("2024/notes.txt")).unwrap(); // ignored
        File::create(photos.join("2025/_DSC0003.RAF")).unwrap();

        let db = Arc::new(
            crate::db::Db::open(&tmp.path().join("catalog.db"))
                .await
                .unwrap(),
        );
        let stats = initial_scan(db.clone(), &photos).await.unwrap();

        // 3 empty RAW files all hash to the same BLAKE3, so just 1 photo row,
        // but 3 photo_files rows (one per distinct path).
        assert_eq!(stats.photos_added, 1, "expected 1 deduped photo row");
        assert_eq!(stats.photo_files_added, 3, "expected 3 distinct path rows");
        assert_eq!(stats.photo_files_updated, 0);

        // Confirm the rows actually landed.
        let conn = db.connect().unwrap();
        let mut rows = conn.query("SELECT COUNT(*) FROM photos", ()).await.unwrap();
        let photo_count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(photo_count, 1);
        let mut rows = conn
            .query("SELECT COUNT(*) FROM photo_files", ())
            .await
            .unwrap();
        let file_count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(file_count, 3);

        // Folders for photos_root, 2024, 2025 should all be present.
        let mut rows = conn
            .query("SELECT COUNT(*) FROM folders", ())
            .await
            .unwrap();
        let folder_count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(folder_count, 3);
    }
}
