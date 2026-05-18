//! Periodic `VACUUM INTO` backups of the catalog with last-N retention.
//!
//! Every 6 hours (skipping the immediate startup tick), runs
//! `VACUUM INTO '<data_dir>/backups/catalog-<unix_secs>.db'` to produce a
//! compacted snapshot, then rotates older snapshots so at most `RETAIN`
//! files remain. Errors are logged and swallowed so a transient failure
//! never tears down the loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;

const TICK: Duration = Duration::from_secs(6 * 60 * 60);
const RETAIN: usize = 14;

/// Run the backup loop until `shutdown` resolves.
pub async fn run(
    db: Arc<Db>,
    backup_dir: PathBuf,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    if let Err(mkdir_err) = std::fs::create_dir_all(&backup_dir) {
        tracing::error!(
            event = "backup.mkdir.error",
            dir = %backup_dir.display(),
            error = %mkdir_err,
        );
        return;
    }
    let mut ticker = tokio::time::interval(TICK);
    // The first tick fires immediately; skip it so we don't back up at
    // startup before there's anything useful to back up.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "backup.shutdown");
                return;
            }
            _ = ticker.tick() => {
                if let Err(backup_err) = run_one(&db, &backup_dir).await {
                    tracing::warn!(event = "backup.error", error = %backup_err);
                }
            }
        }
    }
}

/// Take one backup snapshot and rotate older snapshots down to `RETAIN`.
///
/// The output filename is `catalog-<unix_seconds>.db` so the natural
/// lexicographic order of filenames matches creation order, which lets
/// rotation use a simple sort + drop-oldest.
///
/// # Errors
/// Returns an error if the database connection fails or `VACUUM INTO`
/// fails (e.g. the backup file already exists, the directory is not
/// writable, or libSQL rejects the statement).
pub async fn run_one(db: &Db, backup_dir: &std::path::Path) -> anyhow::Result<()> {
    let timestamp_secs = unix_seconds_now();
    let backup_file_path = backup_dir.join(format!("catalog-{timestamp_secs}.db"));
    let conn = db.connect()?;
    // NOTE: `backup_dir` is server-configured (derived from `cfg.data_dir`),
    // not user input, so we don't escape single quotes inside the
    // interpolated path. If that assumption changes, escape `'` → `''`
    // before formatting into the SQL string.
    conn.execute(&format!("VACUUM INTO '{}'", backup_file_path.display()), ())
        .await?;
    tracing::info!(event = "backup.created", path = %backup_file_path.display());
    rotate(backup_dir, RETAIN)?;
    Ok(())
}

fn rotate(dir: &std::path::Path, keep: usize) -> anyhow::Result<()> {
    let mut existing_backups: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .filter(|dir_entry| {
            dir_entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "db")
        })
        .collect();
    existing_backups.sort_by_key(std::fs::DirEntry::path);
    while existing_backups.len() > keep {
        let oldest_entry = existing_backups.remove(0);
        let _ = std::fs::remove_file(oldest_entry.path());
    }
    Ok(())
}

fn unix_seconds_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed_since_epoch| elapsed_since_epoch.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::TestDb;
    use tempfile::TempDir;

    #[tokio::test]
    async fn run_one_creates_a_backup_file() {
        let test_db = TestDb::start().await;
        let backup_dir_tmp = TempDir::new().unwrap();
        let backup_dir = backup_dir_tmp.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();
        run_one(&test_db.db, &backup_dir).await.unwrap();
        let entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap().collect();
        assert_eq!(
            entries.len(),
            1,
            "exactly one backup file should be written"
        );
    }
}
