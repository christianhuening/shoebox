use shoebox_server::{
    backup, ca, cert_renewal, cli, config, db, http, indexer, janitor, logging, mdns, metrics,
    mtls, revoke, secret, sqld_embed, tls_server,
};
use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();
    let cli = <cli::Cli as clap::Parser>::parse();
    let cfg = load_config()?;
    match cli.command.unwrap_or(cli::Command::Serve) {
        cli::Command::Serve => serve_main(cfg).await,
        cli::Command::Revoke(args) => revoke::run(&args, &cfg).await,
    }
}

fn load_config() -> anyhow::Result<config::Config> {
    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    Ok(if let Some(p) = cfg_path {
        tracing::info!(event = "config.load", path = %p, "loading config file");
        config::Config::load_from_path(std::path::Path::new(&p))?
    } else {
        tracing::info!(
            event = "config.load",
            source = "env",
            "no SHOEBOX_CONFIG; building from env"
        );
        config::Config::from_env_with_defaults()
    })
}

#[allow(clippy::too_many_lines)]
async fn serve_main(cfg: config::Config) -> anyhow::Result<()> {
    mtls::install_crypto_provider();

    tracing::info!(
        event = "startup",
        server_name = %cfg.server_name,
        bind_addr = %cfg.bind_addr,
        health_bind_addr = %cfg.health_bind_addr,
        data_dir = ?cfg.data_dir,
        "shoebox-server starting"
    );

    std::fs::create_dir_all(&cfg.data_dir)?;
    let db = Arc::new(db::Db::open(&cfg.data_dir.join("catalog.db")).await?);

    // Bootstrap CA and ensure server cert.
    let ca = Arc::new(ca::Ca::open(&cfg.data_dir)?);
    let sans = ca::build_server_sans(&cfg.server_name, &cfg.extra_sans);
    let (server_cert, server_kp) = ca.issue_server_cert(&sans)?;

    // Capture the server cert's expiry so the renewal task can compute
    // `days_remaining` (and update the Prometheus gauge) on its first tick
    // without re-issuing.
    let initial_not_after = server_cert.not_after.unix_timestamp();
    let (cert_shutdown_tx, cert_shutdown_rx) = oneshot::channel();
    let cert_task = tokio::spawn(cert_renewal::run(
        ca.clone(),
        cfg.clone(),
        initial_not_after,
        cert_shutdown_rx,
    ));

    // Build the CRL cache, populate it once synchronously, then spawn a
    // background task to refresh it every 30 seconds.
    let crl = mtls::CrlCache::new();
    refresh_crl(&db, &crl).await?;
    spawn_crl_refresher(db.clone(), crl.clone());

    let tls_cfg = mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl.clone())?;

    // Bootstrap shared catalog secret.
    let conn = db.connect()?;
    match secret::ensure_present(&conn).await? {
        secret::EnsureOutcome::Generated { plaintext } => {
            tracing::warn!(
                event = "secret.generated",
                secret = %plaintext,
                "Generated new enrollment secret — share with users out-of-band; \
                 it will not be shown again"
            );
        }
        secret::EnsureOutcome::AlreadySet => {
            tracing::info!(
                event = "secret.loaded",
                "enrollment secret already configured"
            );
        }
    }

    // Spawn the embedded sqld subprocess (loopback only). The proxy
    // forwards mTLS-authenticated `/v1/*` and `/v2/*` traffic to this URL.
    // `embedded_sqld` is bound to a local variable so it lives for the
    // duration of `serve_main`; `Drop` (via `kill_on_drop(true)`) ensures
    // the child is terminated on shutdown even if `shutdown().await` is
    // not reached due to an early error return.
    let embedded_sqld = sqld_embed::start(cfg.data_dir.clone()).await?;

    // Initial scan + live watcher (see `start_indexer`). Missing photos
    // directory is treated as a no-op — both bindings stay `None` and the
    // shutdown block below is a no-op for the indexer.
    let (mut indexer_shutdown_tx, mut indexer_task) =
        start_indexer(db.clone(), &cfg.photos_dir, &cfg.cache_dir).await?;

    // Periodic cleanup: stale lock expiry, abandoned session cleanup,
    // orphaned thumbnail GC. Always runs (unlike the indexer, which is
    // gated on `photos_dir` existing).
    let (janitor_shutdown_tx, janitor_shutdown_rx) = oneshot::channel();
    let janitor_task = tokio::spawn(janitor::run(
        db.clone(),
        cfg.cache_dir.clone(),
        janitor_shutdown_rx,
    ));

    // Periodic VACUUM INTO snapshot of the catalog with last-14 retention.
    let (backup_shutdown_tx, backup_task) = spawn_backup(db.clone(), &cfg.data_dir);
    // Periodic refresh of Prometheus gauges; aborted when the runtime drops.
    let _metrics_updater = spawn_metrics_updater(db.clone());

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca,
        sqld_url: embedded_sqld.local_url.clone(),
        sqld_grpc_url: embedded_sqld.local_grpc_url.clone(),
        cache_dir: cfg.cache_dir.clone(),
    };

    let broadcaster = mdns::MdnsBroadcaster::start(
        &cfg.server_name,
        cfg.bind_addr.port(),
        shoebox_common::SCHEMA_VERSION,
        &mdns::local_ips(),
    )?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown.signal", "received ctrl-c, shutting down");
        let _ = shutdown_tx.send(());
    });

    let (shutdown_health_tx, shutdown_health_rx) = oneshot::channel();
    tokio::spawn({
        let state = state.clone();
        let addr = cfg.health_bind_addr;
        async move {
            if let Err(e) = serve_health(addr, state, shutdown_health_rx).await {
                tracing::error!(event = "health.serve.error", error = %e);
            }
        }
    });

    let result = tls_server::serve_public_tls(cfg.bind_addr, state, tls_cfg, shutdown_rx).await;
    let _ = shutdown_health_tx.send(());
    if let Some(tx) = indexer_shutdown_tx.take() {
        let _ = tx.send(());
    }
    if let Some(task) = indexer_task.take() {
        let _ = task.await;
    }
    let _ = janitor_shutdown_tx.send(());
    let _ = janitor_task.await;
    let _ = backup_shutdown_tx.send(());
    let _ = backup_task.await;
    let _ = cert_shutdown_tx.send(());
    let _ = cert_task.await;
    broadcaster.shutdown();
    embedded_sqld.shutdown().await;
    result
}

/// Spawn the periodic CRL-refresh background task. The handle is dropped
/// intentionally — the task is aborted when the tokio runtime shuts down
/// at the end of `serve_main`. Errors during a refresh are logged but do
/// not stop the loop; the previously-cached CRL stays in effect.
fn spawn_crl_refresher(db: Arc<db::Db>, crl: mtls::CrlCache) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if let Err(e) = refresh_crl(&db, &crl).await {
                tracing::warn!(event = "crl.refresh.error", error = %e);
            }
        }
    });
}

/// Spawn the periodic Prometheus gauge updater. Refreshes the
/// `shoebox_active_sessions` and `shoebox_active_develop_locks` gauges every
/// 30 seconds via direct catalog queries. `shoebox_disk_bytes_free` is left
/// at zero in v1 (no `statfs` dep yet); `shoebox_cert_days_until_expiry`
/// will be populated by Task 18 (cert auto-renewal).
///
/// Returns a `JoinHandle` that the caller is free to drop — the task is
/// aborted implicitly when the tokio runtime shuts down at the end of
/// `serve_main`.
fn spawn_metrics_updater(db: Arc<db::Db>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            ticker.tick().await;
            let Ok(conn) = db.connect() else { continue };
            if let Ok(mut rows) = conn.query("SELECT COUNT(*) FROM sessions", ()).await {
                if let Ok(Some(row)) = rows.next().await {
                    if let Ok(count) = row.get::<i64>(0) {
                        metrics::METRICS.active_sessions.set(count);
                    }
                }
            }
            if let Ok(mut rows) = conn.query("SELECT COUNT(*) FROM develop_locks", ()).await {
                if let Ok(Some(row)) = rows.next().await {
                    if let Ok(count) = row.get::<i64>(0) {
                        metrics::METRICS.active_develop_locks.set(count);
                    }
                }
            }
        }
    })
}

/// Spawn the periodic catalog-backup task and return its shutdown channel
/// alongside the spawned `JoinHandle`. Backups land in
/// `<data_dir>/backups/`; the directory is created on first tick if it
/// doesn't already exist.
fn spawn_backup(
    db: Arc<db::Db>,
    data_dir: &std::path::Path,
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let (backup_shutdown_tx, backup_shutdown_rx) = oneshot::channel();
    let backup_dir = data_dir.join("backups");
    let backup_task = tokio::spawn(backup::run(db, backup_dir, backup_shutdown_rx));
    (backup_shutdown_tx, backup_task)
}

/// Run the initial photo-library scan and spawn the live FS watcher.
///
/// If `photos_root` does not exist (e.g. a fresh install before the NAS
/// share is mounted), both the scan and the watcher are skipped with a
/// warning and `(None, None)` is returned so the caller's shutdown block
/// becomes a no-op for the indexer. Otherwise returns the watcher's
/// shutdown channel and `JoinHandle` so the caller can stop it on
/// shutdown. `cache_dir` is forwarded to the indexer so newly-discovered
/// photos trigger background thumbnail rendering.
async fn start_indexer(
    db: Arc<db::Db>,
    photos_root: &std::path::Path,
    cache_dir: &std::path::Path,
) -> anyhow::Result<(
    Option<oneshot::Sender<()>>,
    Option<tokio::task::JoinHandle<()>>,
)> {
    if !photos_root.exists() {
        tracing::warn!(
            event = "indexer.skip",
            photos_dir = %photos_root.display(),
            "photos directory does not exist; skipping initial scan and live watcher"
        );
        return Ok((None, None));
    }
    let scan_stats = indexer::initial_scan(db.clone(), photos_root, cache_dir).await?;
    tracing::info!(
        event = "indexer.initial_scan",
        folders_seen = scan_stats.folders_seen,
        files_seen = scan_stats.files_seen,
        photos_added = scan_stats.photos_added,
        photo_files_added = scan_stats.photo_files_added,
        "initial scan complete"
    );
    let (watcher_shutdown_tx, watcher_shutdown_rx) = oneshot::channel();
    let indexer_db = db;
    let indexer_root = photos_root.to_path_buf();
    let indexer_cache_dir = cache_dir.to_path_buf();
    let watcher_task = tokio::spawn(async move {
        if let Err(e) = indexer::run_watcher(
            indexer_db,
            indexer_root,
            indexer_cache_dir,
            watcher_shutdown_rx,
        )
        .await
        {
            tracing::error!(event = "indexer.run.error", error = %e);
        }
    });
    Ok((Some(watcher_shutdown_tx), Some(watcher_task)))
}

async fn serve_health(
    addr: std::net::SocketAddr,
    state: http::AppState,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(event = "http.listen.health", addr = %addr, "health server bound");
    axum::serve(listener, http::health_router(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
        })
        .await?;
    Ok(())
}

// ── CRL refresh ───────────────────────────────────────────────────────────────

/// Load all revoked cert serials from the database and update the in-memory
/// CRL cache. Called once at startup and then every 30 seconds in a background
/// task, so revocation takes effect within at most one refresh interval.
async fn refresh_crl(db: &std::sync::Arc<db::Db>, crl: &mtls::CrlCache) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT serial_number FROM revoked_certs", ())
        .await?;
    let mut revoked_set = std::collections::HashSet::new();
    while let Some(row) = rows.next().await? {
        revoked_set.insert(row.get::<String>(0)?);
    }
    let revoked_count = revoked_set.len();
    crl.replace(revoked_set);
    tracing::debug!(event = "crl.refresh", revoked_count, "CRL cache refreshed");
    Ok(())
}
