//! End-to-end test: spawn the server's components in-process, hit
//! /health over loopback, verify response.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn full_server_serves_health() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(
        shoebox_server::db::Db::open(&tmp.path().join("catalog.db"))
            .await
            .unwrap(),
    );

    let state = shoebox_server::http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    let app = shoebox_server::http::router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    let resp = reqwest::get(format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["schema_version"], 6);

    let _ = tx.send(());
    server.await.unwrap();
}
