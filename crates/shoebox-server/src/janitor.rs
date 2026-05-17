//! Periodic cleanup tasks: stale lock expiry, abandoned session cleanup,
//! orphaned thumbnail GC.
//!
//! A single 60-second ticker drives three sweeps at different cadences:
//! * every tick: release expired develop locks,
//! * every 5th tick (~5 min): delete sessions idle > 24 h,
//! * every 60th tick (~1 h): walk `<cache_dir>/{thumbnails,previews}/` and
//!   delete `<hash>.jpg` files whose hash is not in `photos.id`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;

const TICK: Duration = Duration::from_secs(60);
const SESSION_IDLE_MS: i64 = 24 * 60 * 60 * 1000;

/// Run the janitor loop until `shutdown` resolves.
///
/// Each sweep error is logged at `warn` and swallowed so a transient
/// database hiccup never tears down the loop.
pub async fn run(
    db: Arc<Db>,
    cache_dir: PathBuf,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(TICK);
    let mut sweep_count: u64 = 0;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "janitor.shutdown");
                return;
            }
            _ = ticker.tick() => {
                if let Err(lock_err) = lock_sweep(&db).await {
                    tracing::warn!(event = "janitor.lock.error", error = %lock_err);
                }
                if sweep_count % 5 == 0 {
                    if let Err(session_err) = session_sweep(&db).await {
                        tracing::warn!(event = "janitor.session.error", error = %session_err);
                    }
                }
                if sweep_count % 60 == 0 && sweep_count > 0 {
                    if let Err(thumb_err) = thumb_gc(&db, &cache_dir).await {
                        tracing::warn!(event = "janitor.thumb_gc.error", error = %thumb_err);
                    }
                }
                sweep_count = sweep_count.wrapping_add(1);
            }
        }
    }
}

async fn lock_sweep(db: &Db) -> anyhow::Result<()> {
    let released_count = db.lock_release_expired().await?;
    if released_count > 0 {
        tracing::info!(event = "janitor.lock.expired", released = released_count);
    }
    Ok(())
}

async fn session_sweep(db: &Db) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let cutoff = now_ms() - SESSION_IDLE_MS;
    let deleted_count = conn
        .execute("DELETE FROM sessions WHERE last_active_at < ?1", [cutoff])
        .await?;
    if deleted_count > 0 {
        tracing::info!(event = "janitor.session.cleanup", deleted = deleted_count);
    }
    Ok(())
}

async fn thumb_gc(db: &Db, cache_dir: &std::path::Path) -> anyhow::Result<()> {
    use std::collections::HashSet;

    let conn = db.connect()?;
    let mut rows = conn.query("SELECT id FROM photos", ()).await?;
    let mut known_photo_hashes: HashSet<String> = HashSet::new();
    while let Some(row) = rows.next().await? {
        known_photo_hashes.insert(row.get::<String>(0)?);
    }

    let mut removed_count: u64 = 0;
    for cache_subdir_name in ["thumbnails", "previews"] {
        let cache_subdir_path = cache_dir.join(cache_subdir_name);
        if !cache_subdir_path.exists() {
            continue;
        }
        let mut dir_entries = tokio::fs::read_dir(&cache_subdir_path).await?;
        while let Some(dir_entry) = dir_entries.next_entry().await? {
            if let Some(filename_str) = dir_entry.file_name().to_str() {
                if let Some(hash_hex_candidate) = filename_str.strip_suffix(".jpg") {
                    if !known_photo_hashes.contains(hash_hex_candidate) {
                        let _ = tokio::fs::remove_file(dir_entry.path()).await;
                        removed_count += 1;
                    }
                }
            }
        }
    }
    if removed_count > 0 {
        tracing::info!(event = "janitor.thumb_gc", removed = removed_count);
    }
    Ok(())
}

// local copy; `db::now_ms` is private and we don't want to widen the API
// just for the janitor.
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
