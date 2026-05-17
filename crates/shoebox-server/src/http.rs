//! HTTP routers. `public_router` carries auth-required endpoints and is
//! served over mTLS; `health_router` carries only /health and is served
//! over plain HTTP on a loopback-only port.

use axum::{extract::State, http::StatusCode, response::Json, routing::get, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub schema_version: i64,
    pub ca: Arc<crate::ca::Ca>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub schema_version: i64,
}

/// Endpoints that require mTLS (gated by the TLS layer, not the router).
/// In Task 8 this gains /enroll; in Task 11 it gains /whoami.
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::enroll::route())
        .with_state(state)
}

/// Plain-HTTP /health endpoint for container/k8s healthchecks. Bound to
/// loopback only; never exposed off-host.
pub fn health_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            schema_version: state.schema_version,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn health_endpoint_returns_ok_with_schema_version() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("catalog.db");
        let db = Arc::new(Db::open(&db_path).await.unwrap());
        let ca = Arc::new(crate::ca::Ca::open(tmp.path()).unwrap());
        let state = AppState {
            db,
            schema_version: shoebox_common::SCHEMA_VERSION,
            ca,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();

        let app = health_router(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        let resp = reqwest::get(format!("http://{addr}/health")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["schema_version"], 6);

        let _ = tx.send(());
        server.await.unwrap();
    }
}
