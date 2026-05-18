//! GET /ca-cert — returns the CA cert PEM. Unauthenticated (client has
//! no cert yet at the point it needs this). Served on the public mTLS
//! listener; clients use `dangerous_accept_invalid_certs(true)` for the
//! single bootstrap request, then validate everything subsequent against
//! the CA they just received.

use axum::{extract::State, http::StatusCode, routing::get, Router};

use crate::http::AppState;

pub fn route() -> Router<AppState> {
    Router::new().route("/ca-cert", get(handler))
}

async fn handler(
    State(state): State<AppState>,
) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    (
        StatusCode::OK,
        [("Content-Type", "application/x-pem-file")],
        state.ca.root_cert_pem.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn ca_cert_returns_pem_body() {
        let tmp = TempDir::new().unwrap();
        let db = Arc::new(Db::open(&tmp.path().join("catalog.db")).await.unwrap());
        let ca = Arc::new(crate::ca::Ca::open(tmp.path()).unwrap());
        let state = AppState {
            db,
            schema_version: shoebox_common::SCHEMA_VERSION,
            ca: ca.clone(),
            sqld_url: "http://127.0.0.1:0".to_string(),
            sqld_grpc_url: "http://127.0.0.1:0".to_string(),
            cache_dir: tmp.path().to_path_buf(),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let app = Router::new().merge(route()).with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        let resp = reqwest::get(format!("http://{addr}/ca-cert"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("-----BEGIN CERTIFICATE-----"));
        assert_eq!(body, ca.root_cert_pem);

        let _ = tx.send(());
        let _ = server.await;
    }
}
