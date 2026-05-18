//! Background client-cert renewal task. Mirrors `shoebox-server`'s
//! `cert_renewal.rs` shape: 12h ticker, re-issue when <30 days remain.
//!
//! Unlike the server-side task, this one (a) calls the remote `/renew`
//! endpoint over the established mTLS connection, (b) persists the new
//! cert via `cert_store`, and (c) updates `client.toml`'s
//! `cert_serial_hex`. The in-process reqwest client is NOT swapped at
//! runtime (Iced's state lives behind a lock; rebuilding the client
//! during a tick is a Plan 1.4b refinement). For v1, the warning is
//! logged and the user picks up the new cert at next launch.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const RENEW_WHEN_DAYS_REMAINING: i64 = 30;
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Debug, Deserialize)]
struct RenewResponse {
    client_cert_pem: String,
    cert_serial_hex: String,
    not_after_unix: i64,
}

#[derive(Debug, Serialize)]
struct RenewRequest {
    csr_pem: String,
}

pub struct RenewalContext {
    pub server_url: String,
    pub client: Client,
    /// Path to the local `client.toml`; we rewrite `cert_serial_hex`
    /// after a successful renewal.
    pub config_path: PathBuf,
    /// Current cert's `not_after`. Updated in place after each renewal.
    pub not_after_unix: i64,
}

/// Run the renewal loop until `shutdown` resolves. Re-issues the cert
/// whenever `not_after_unix` is within `RENEW_WHEN_DAYS_REMAINING` days.
pub async fn run(
    context: Arc<parking_lot::Mutex<RenewalContext>>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(event = "client.cert_renewal.shutdown");
                return;
            }
            _ = ticker.tick() => {
                if let Err(renewal_err) = run_one(&context).await {
                    tracing::warn!(
                        event = "client.cert_renewal.error",
                        error = %renewal_err,
                    );
                }
            }
        }
    }
}

/// Public for `cert_renewal_e2e.rs` — runs exactly one renewal check.
///
/// # Errors
/// Returns an error on network failure, CSR generation failure, server
/// rejection, or config write failure.
pub async fn run_one(context: &Arc<parking_lot::Mutex<RenewalContext>>) -> Result<()> {
    let (server_url, client, config_path, current_not_after) = {
        let guard = context.lock();
        (
            guard.server_url.clone(),
            guard.client.clone(),
            guard.config_path.clone(),
            guard.not_after_unix,
        )
    };

    let now_secs = now_secs();
    let days_remaining = (current_not_after.saturating_sub(now_secs)) / SECONDS_PER_DAY;
    log_days_remaining(days_remaining);
    if days_remaining > RENEW_WHEN_DAYS_REMAINING {
        return Ok(());
    }

    // Generate a new keypair + CSR.
    let key_pair =
        KeyPair::generate_for(&rcgen::PKCS_ED25519).context("generating renewal keypair")?;
    let new_key_pem = key_pair.serialize_pem();
    let mut csr_params =
        CertificateParams::new(Vec::<String>::new()).context("renewal csr params")?;
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "renewal-csr-placeholder");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&key_pair)
        .context("renewal serialize csr")?
        .pem()
        .context("renewal csr pem")?;

    let resp = client
        .post(format!("{server_url}/renew"))
        .json(&RenewRequest { csr_pem })
        .send()
        .await
        .context("POST /renew")?;
    if !resp.status().is_success() {
        anyhow::bail!("renew returned {}", resp.status());
    }
    let renewed: RenewResponse = resp.json().await.context("parsing renew response")?;

    // Persist the new cert + key. Keychain is preferred; if it fails we
    // attempt file storage with a logged warning (renewal isn't user-
    // interactive, so no "explicit consent" prompt fires — we keep the
    // existing storage location).
    //
    // Both keyring and file are sync APIs. The keyring backend on Linux
    // (`dbus-secret-service`/`zbus`) calls `block_on` internally — that
    // panics if invoked from a tokio worker thread. Defer to
    // `spawn_blocking` so the call runs on a dedicated blocking thread
    // regardless of platform.
    let keyring_result = tokio::task::spawn_blocking({
        let server_url = server_url.clone();
        let cert_pem = renewed.client_cert_pem.clone();
        let key_pem = new_key_pem.clone();
        move || crate::cert_store::store_in_keyring(&server_url, &cert_pem, &key_pem)
    })
    .await
    .context("joining keyring blocking task")?;
    if let Err(keyring_err) = keyring_result {
        tracing::warn!(
            event = "client.cert_renewal.keyring_fallback",
            error = %keyring_err,
        );
        let file_result = tokio::task::spawn_blocking({
            let server_url = server_url.clone();
            let cert_pem = renewed.client_cert_pem.clone();
            let key_pem = new_key_pem.clone();
            move || crate::cert_store::store_in_file(&server_url, &cert_pem, &key_pem)
        })
        .await
        .context("joining file-store blocking task")?;
        file_result.context("file fallback for renewal cert store")?;
    }

    // Update client.toml's cert_serial_hex.
    let mut config = crate::config::ClientConfig::read_from(&config_path)
        .context("re-reading client.toml during renewal")?;
    config.cert_serial_hex.clone_from(&renewed.cert_serial_hex);
    config
        .write_to(&config_path)
        .context("writing client.toml after renewal")?;

    // Update the in-memory not_after so the next tick uses the new
    // expiry.
    context.lock().not_after_unix = renewed.not_after_unix;

    tracing::warn!(
        event = "client.cert_renewal.reissued",
        days_remaining,
        new_serial = %renewed.cert_serial_hex,
        new_not_after_unix = renewed.not_after_unix,
        "client cert re-issued — running connection still uses the previous cert; restart to switch over"
    );
    Ok(())
}

fn log_days_remaining(days: i64) {
    tracing::debug!(event = "client.cert_renewal.tick", days_remaining = days);
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
