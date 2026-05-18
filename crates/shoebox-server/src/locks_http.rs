//! Develop-lock REST endpoints.
//!
//! - `POST   /locks/:variant_id`            — acquire (returns 200 + holder info, 409 if held)
//! - `PUT    /locks/:variant_id`            — heartbeat (returns 200 if extended, 404 if not held by you)
//! - `DELETE /locks/:variant_id`            — release (returns 204, or 404 if not held by you)
//! - `POST   /locks/:variant_id/takeover`   — request takeover (returns 200, 409 if already pending or lock free)
//!
//! The session identity is derived from the client cert serial
//! (`ClientIdentity::cert_serial_hex`) — each enrolled cert corresponds to
//! one develop session. The user identity is `ClientIdentity::user_id`.

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{delete, post, put},
    Router,
};
use serde::Serialize;

use crate::http::AppState;
use crate::identity::ClientIdentity;

/// Develop-lock TTL: 15 minutes. Clients must heartbeat before this elapses
/// or the janitor will reap the lock.
const LOCK_TTL_MS: i64 = 15 * 60 * 1000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/locks/{variant_id}", post(acquire))
        .route("/locks/{variant_id}", put(heartbeat))
        .route("/locks/{variant_id}", delete(release))
        .route("/locks/{variant_id}/takeover", post(takeover))
}

#[derive(Debug, Serialize)]
struct AcquireResponse {
    acquired: bool,
    holder_user_id: Option<String>,
}

async fn acquire(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    let session_id = identity.cert_serial_hex.clone();
    let acquire_result = state
        .db
        .lock_acquire(&variant_id, &session_id, &identity.user_id.0, LOCK_TTL_MS)
        .await;
    match acquire_result {
        Ok(true) => (
            StatusCode::OK,
            Json(AcquireResponse {
                acquired: true,
                holder_user_id: Some(identity.user_id.0.clone()),
            }),
        )
            .into_response(),
        Ok(false) => {
            // Lock already held; look up the current holder so the client
            // can decide whether to request a takeover.
            let current_holder_user_id = state.db.lock_holder(&variant_id).await.ok().flatten();
            (
                StatusCode::CONFLICT,
                Json(AcquireResponse {
                    acquired: false,
                    holder_user_id: current_holder_user_id,
                }),
            )
                .into_response()
        }
        Err(acquire_err) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{acquire_err}")).into_response()
        }
    }
}

async fn heartbeat(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    let session_id = identity.cert_serial_hex.clone();
    match state
        .db
        .lock_heartbeat(&variant_id, &session_id, LOCK_TTL_MS)
        .await
    {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "lock not held by you").into_response(),
        Err(heartbeat_err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{heartbeat_err}"),
        )
            .into_response(),
    }
}

async fn release(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    let session_id = identity.cert_serial_hex.clone();
    match state.db.lock_release(&variant_id, &session_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "lock not held by you").into_response(),
        Err(release_err) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{release_err}")).into_response()
        }
    }
}

async fn takeover(
    State(state): State<AppState>,
    identity: ClientIdentity,
    AxumPath(variant_id): AxumPath<String>,
) -> Response {
    match state
        .db
        .lock_request_takeover(&variant_id, &identity.user_id.0)
        .await
    {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            "takeover already pending or lock free",
        )
            .into_response(),
        Err(takeover_err) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{takeover_err}")).into_response()
        }
    }
}
