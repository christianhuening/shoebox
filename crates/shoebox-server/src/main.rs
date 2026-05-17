mod config;
mod db;
mod http;
mod logging;

use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();
    tracing::info!(event = "startup", "shoebox-server starting");

    // Minimal in-place defaults for Plan 1.1; replaced by Config in Task 14.
    let data_dir = std::path::PathBuf::from(
        std::env::var("SHOEBOX_DATA_DIR").unwrap_or_else(|_| "./data".into()),
    );
    std::fs::create_dir_all(&data_dir)?;
    let db = Arc::new(db::Db::open(&data_dir.join("catalog.db")).await?);

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };
    let addr: std::net::SocketAddr = std::env::var("SHOEBOX_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:9000".into())
        .parse()?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(());
    });

    http::serve(addr, state, shutdown_rx).await
}
