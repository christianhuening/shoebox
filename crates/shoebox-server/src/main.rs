mod config;
mod db;
mod http;
mod logging;

use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();

    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    let cfg = match cfg_path {
        Some(p) => {
            tracing::info!(event = "config.load", path = %p, "loading config file");
            config::Config::load_from_path(std::path::Path::new(&p))?
        }
        None => {
            tracing::info!(event = "config.load", source = "env", "no SHOEBOX_CONFIG; building from env");
            config::Config::from_env_with_defaults()
        }
    };

    tracing::info!(
        event = "startup",
        server_name = %cfg.server_name,
        bind_addr = %cfg.bind_addr,
        data_dir = ?cfg.data_dir,
        "shoebox-server starting"
    );

    std::fs::create_dir_all(&cfg.data_dir)?;
    let db = Arc::new(db::Db::open(&cfg.data_dir.join("catalog.db")).await?);

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown.signal", "received ctrl-c, shutting down");
        let _ = shutdown_tx.send(());
    });

    http::serve(cfg.bind_addr, state, shutdown_rx).await
}
