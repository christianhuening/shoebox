//! /enroll handler: validate shared secret, create user if needed, sign
//! the presented CSR, return cert chain.

use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, response::Json, routing::post, Router};
use serde::{Deserialize, Serialize};
use shoebox_common::{MachineId, UserId};

use crate::ca::IssuedCert;
use crate::http::AppState;
use crate::secret;

#[derive(Debug, Deserialize)]
pub struct EnrollRequest {
    pub shared_secret: String,
    pub csr_pem: String,
    pub display_name: String,
    /// If set, re-enroll an existing user from a new machine. If absent,
    /// a new user row is created.
    pub existing_user_id: Option<UserId>,
    /// Stable identifier for the client install; if absent, a new one
    /// is generated.
    pub machine_id: Option<MachineId>,
}

#[derive(Debug, Serialize)]
pub struct EnrollResponse {
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub user_id: UserId,
    pub machine_id: MachineId,
    pub cert_serial_hex: String,
    pub not_after_unix: i64,
}

pub fn route() -> Router<AppState> {
    Router::new().route("/enroll", post(enroll_handler))
}

async fn enroll_handler(
    State(state): State<AppState>,
    Json(req): Json<EnrollRequest>,
) -> Result<(StatusCode, Json<EnrollResponse>), (StatusCode, String)> {
    let conn = state
        .db
        .connect()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;

    let ok = secret::verify(&conn, &req.shared_secret)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("verify: {e}")))?;
    if !ok {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid shared secret".to_string(),
        ));
    }

    // Resolve user_id: either re-use existing (verify it exists) or create.
    let user_id = if let Some(uid) = &req.existing_user_id {
        let mut rows = conn
            .query("SELECT 1 FROM users WHERE id = ?1", [uid.to_string()])
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
        if rows
            .next()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?
            .is_none()
        {
            return Err((StatusCode::NOT_FOUND, format!("user {uid} not found")));
        }
        uid.clone()
    } else {
        let new_uid = random_user_id();
        conn.execute(
            "INSERT INTO users (id, display_name, created_at, last_seen_at) \
             VALUES (?1, ?2, ?3, ?3)",
            (new_uid.to_string(), req.display_name.clone(), now_ms()),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
        new_uid
    };

    let machine_id = req.machine_id.unwrap_or_else(random_machine_id);

    let issued = sign_csr(&state.ca, &req.csr_pem, &user_id, &machine_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("csr: {e}")))?;

    tracing::info!(
        event = "enrollment.completed",
        user_id = %user_id,
        machine_id = %machine_id,
        serial = %issued.serial_hex,
        "client enrolled"
    );

    Ok((
        StatusCode::OK,
        Json(EnrollResponse {
            client_cert_pem: issued.cert_pem,
            ca_cert_pem: state.ca.root_cert_pem.clone(),
            user_id,
            machine_id,
            cert_serial_hex: issued.serial_hex,
            not_after_unix: issued.not_after.unix_timestamp(),
        }),
    ))
}

fn sign_csr(
    ca: &crate::ca::Ca,
    csr_pem: &str,
    user_id: &UserId,
    machine_id: &MachineId,
) -> Result<IssuedCert> {
    // Parse the CSR to extract the public key.
    let csr =
        rcgen::CertificateSigningRequestParams::from_pem(csr_pem).context("parsing CSR PEM")?;
    ca.issue_client_cert(&csr.public_key, user_id, machine_id)
}

fn random_user_id() -> UserId {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    UserId(hex::encode(bytes))
}

fn random_machine_id() -> MachineId {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    MachineId(hex::encode(bytes))
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ── /renew ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RenewRequest {
    pub csr_pem: String,
}

#[derive(Debug, Serialize)]
pub struct RenewResponse {
    pub client_cert_pem: String,
    pub cert_serial_hex: String,
    pub not_after_unix: i64,
}

pub fn renew_route() -> Router<AppState> {
    Router::new().route("/renew", post(renew_handler))
}

/// Renew a client certificate. The caller must already hold a valid client
/// cert (identity is extracted from the mTLS connection); no shared-secret
/// validation is performed. The new cert carries the same `user_id` and
/// `machine_id` as the existing one.
async fn renew_handler(
    State(state): State<AppState>,
    identity: crate::identity::ClientIdentity,
    Json(req): Json<RenewRequest>,
) -> Result<(StatusCode, Json<RenewResponse>), (StatusCode, String)> {
    let issued = sign_csr(
        &state.ca,
        &req.csr_pem,
        &identity.user_id,
        &identity.machine_id,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("csr: {e}")))?;

    tracing::info!(
        event = "renewal.completed",
        user_id = %identity.user_id,
        machine_id = %identity.machine_id,
        old_serial = %identity.cert_serial_hex,
        new_serial = %issued.serial_hex,
        "client cert renewed"
    );

    Ok((
        StatusCode::OK,
        Json(RenewResponse {
            client_cert_pem: issued.cert_pem,
            cert_serial_hex: issued.serial_hex,
            not_after_unix: issued.not_after.unix_timestamp(),
        }),
    ))
}
