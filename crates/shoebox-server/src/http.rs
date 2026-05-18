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
    /// Loopback URL of the embedded `sqld` subprocess (Hrana HTTP),
    /// e.g. `http://127.0.0.1:53421`. The proxy forwards non-gRPC
    /// `/v1/*` and `/v2/*` requests here.
    pub sqld_url: String,
    /// Loopback URL of the embedded `sqld` subprocess (replication gRPC),
    /// e.g. `http://127.0.0.1:53422`. The proxy forwards requests with
    /// `Content-Type: application/grpc*` here. Same backing db as
    /// `sqld_url`.
    pub sqld_grpc_url: String,
    /// Directory holding generated thumbnails. Populated for forward
    /// compatibility with the thumbnail HTTP endpoints (Plan 1.3 Task 13).
    pub cache_dir: std::path::PathBuf,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub schema_version: i64,
}

/// Endpoints served over the mTLS listener.
/// - /enroll — accepts unauthenticated requests (no client cert required)
/// - /renew  — requires a valid client cert (`ClientIdentity` extractor enforces this)
/// - /whoami — requires a valid client cert (`ClientIdentity` extractor enforces this)
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::ca_cert::route())
        .merge(crate::enroll::route())
        .merge(crate::enroll::renew_route())
        .merge(crate::whoami::route())
        .merge(crate::proxy::routes())
        .merge(crate::thumbs_http::routes())
        .merge(crate::locks_http::routes())
        .with_state(state)
}

/// Plain-HTTP /health endpoint for container/k8s healthchecks. Bound to
/// loopback only; never exposed off-host.
pub fn health_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .merge(crate::metrics::route())
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
    use crate::test_helpers::TestDb;
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn health_endpoint_returns_ok_with_schema_version() {
        let test_db = TestDb::start().await;
        let ca_dir = TempDir::new().unwrap();
        let ca = Arc::new(crate::ca::Ca::open(ca_dir.path()).unwrap());
        let state = AppState {
            db: test_db.db.clone(),
            schema_version: shoebox_common::SCHEMA_VERSION,
            ca,
            sqld_url: test_db.embedded.local_url.clone(),
            sqld_grpc_url: test_db.embedded.local_grpc_url.clone(),
            cache_dir: ca_dir.path().to_path_buf(),
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
        assert_eq!(body["schema_version"], shoebox_common::SCHEMA_VERSION);

        let _ = tx.send(());
        server.await.unwrap();
    }
}
