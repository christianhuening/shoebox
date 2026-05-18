//! End-to-end: /metrics returns Prometheus text format with our gauges.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_format() {
    if shoebox_server::skip_unless_sqld!() {
        return;
    }
    let temp_dir = TempDir::new().unwrap();
    let test_db = shoebox_server::test_helpers::TestDb::start().await;
    let ca = Arc::new(shoebox_server::ca::Ca::open(temp_dir.path()).unwrap());
    let state = shoebox_server::http::AppState {
        db: test_db.db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca,
        sqld_url: test_db.embedded.local_url.clone(),
        sqld_grpc_url: test_db.embedded.local_grpc_url.clone(),
        cache_dir: temp_dir.path().to_path_buf(),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let health_app = shoebox_server::http::health_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, health_app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let metrics_resp = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .unwrap();
    assert_eq!(metrics_resp.status(), 200);
    let metrics_body = metrics_resp.text().await.unwrap();
    assert!(metrics_body.contains("shoebox_active_sessions"));
    assert!(metrics_body.contains("shoebox_active_develop_locks"));
    assert!(metrics_body.contains("shoebox_cert_days_until_expiry"));

    let _ = shutdown_tx.send(());
    server.await.unwrap();
}
