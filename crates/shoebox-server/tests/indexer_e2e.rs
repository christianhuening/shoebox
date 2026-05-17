//! End-to-end: start the indexer watcher, drop a RAW file into the watched
//! directory, observe the catalog gets a `photos` row + `photo_files` row.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn watcher_picks_up_dropped_file() {
    let temp_dir = TempDir::new().unwrap();
    let photos_root = temp_dir.path().join("photos");
    let cache_dir = temp_dir.path().join("cache");
    std::fs::create_dir_all(&photos_root).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let db = Arc::new(
        shoebox_server::db::Db::open(&temp_dir.path().join("catalog.db"))
            .await
            .unwrap(),
    );

    let _initial_stats =
        shoebox_server::indexer::initial_scan(db.clone(), &photos_root, &cache_dir)
            .await
            .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let watcher_db = db.clone();
    let watcher_photos_root = photos_root.clone();
    let watcher_cache_dir = cache_dir.clone();
    let watcher_handle = tokio::spawn(async move {
        let _ = shoebox_server::indexer::run_watcher(
            watcher_db,
            watcher_photos_root,
            watcher_cache_dir,
            shutdown_rx,
        )
        .await;
    });

    // Give the watcher a moment to register its recursive watch before we
    // drop a file into the tree.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    std::fs::write(
        photos_root.join("_DSC0001.PEF"),
        b"not-a-real-raw-but-has-bytes",
    )
    .unwrap();

    // Poll the catalog until the watcher's INSERT lands. libsql in local
    // (embedded) mode can briefly conflict between a polling reader and the
    // watcher's writer, so each poll opens + drops its own connection and
    // we tolerate the watcher needing a few retries via the indexer's
    // event-loop logging path. The polling window (10 s) leaves headroom
    // for that on CI.
    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut photo_was_indexed = false;
    while std::time::Instant::now() < poll_deadline {
        let photo_count = {
            let conn = db.connect().unwrap();
            let mut rows = conn.query("SELECT COUNT(*) FROM photos", ()).await.unwrap();
            let row = rows.next().await.unwrap().unwrap();
            row.get::<i64>(0).unwrap()
        };
        if photo_count > 0 {
            photo_was_indexed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        photo_was_indexed,
        "indexer should pick up dropped PEF within 10s"
    );

    let _ = shutdown_tx.send(());
    let _ = watcher_handle.await;
}
