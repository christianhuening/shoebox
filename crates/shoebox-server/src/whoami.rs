//! GET /whoami — returns the authenticated client's identity. Useful as
//! a debugging endpoint and as a known-good auth check for integration
//! tests.

use axum::{http::StatusCode, response::Json, routing::get, Router};
use serde::Serialize;

use crate::http::AppState;
use crate::identity::ClientIdentity;

#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    pub user_id: String,
    pub machine_id: String,
    pub cert_serial_hex: String,
}

pub fn route() -> Router<AppState> {
    Router::new().route("/whoami", get(whoami_handler))
}

async fn whoami_handler(identity: ClientIdentity) -> (StatusCode, Json<WhoamiResponse>) {
    (
        StatusCode::OK,
        Json(WhoamiResponse {
            user_id: identity.user_id.to_string(),
            machine_id: identity.machine_id.to_string(),
            cert_serial_hex: identity.cert_serial_hex,
        }),
    )
}
