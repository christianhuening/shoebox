use shoebox_server::{ca, config, db, http, logging, mdns, mtls, secret};
use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();
    mtls::install_crypto_provider();

    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    let cfg = if let Some(p) = cfg_path {
        tracing::info!(event = "config.load", path = %p, "loading config file");
        config::Config::load_from_path(std::path::Path::new(&p))?
    } else {
        tracing::info!(event = "config.load", source = "env", "no SHOEBOX_CONFIG; building from env");
        config::Config::from_env_with_defaults()
    };

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
    let tls_cfg = mtls::mtls_server_config(&server_cert, &server_kp, &ca)?;

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
            tracing::info!(event = "secret.loaded", "enrollment secret already configured");
        }
    }

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca,
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

    let result = serve_public_tls(cfg.bind_addr, state, tls_cfg, shutdown_rx).await;
    let _ = shutdown_health_tx.send(());
    broadcaster.shutdown();
    result
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

async fn serve_public_tls(
    addr: std::net::SocketAddr,
    state: http::AppState,
    tls_cfg: std::sync::Arc<rustls::ServerConfig>,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    use axum_server::tls_rustls::RustlsConfig;
    let rustls_cfg = RustlsConfig::from_config(tls_cfg);
    tracing::info!(event = "https.listen.public", addr = %addr, "public TLS server bound");
    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = shutdown.await;
        handle_for_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });
    axum_server::bind_rustls(addr, rustls_cfg)
        .handle(handle)
        .serve(http::public_router(state).into_make_service())
        .await?;
    Ok(())
}
