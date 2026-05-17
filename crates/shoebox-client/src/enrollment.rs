//! Bootstrap + enrollment HTTP calls.
//!
//! `fetch_ca_cert` is intentionally unauthenticated and disables TLS
//! validation — the client has no CA to pin yet. The first thing it
//! does on success is hand the CA PEM to subsequent calls so they CAN
//! validate. The trust boundary is parent-spec §7.7 (LAN-trusted).
//!
//! `enroll` validates the server cert chain against the CA PEM that
//! `fetch_ca_cert` returned, generates an Ed25519 keypair + CSR via
//! `rcgen`, POSTs `/enroll`, and returns the parsed response.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::mtls_http;

/// Parsed shape of `/enroll`'s response (mirrors `shoebox-server`'s
/// `EnrollResponse`).
#[derive(Debug, Deserialize)]
pub struct EnrollResult {
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub user_id: String,
    pub machine_id: String,
    pub cert_serial_hex: String,
    pub not_after_unix: i64,
    /// Filled in client-side after the response is parsed (rcgen produces
    /// the key locally and never sends it over the wire).
    #[serde(skip)]
    pub client_key_pem: String,
}

#[derive(Debug, Serialize)]
struct EnrollRequest<'a> {
    shared_secret: &'a str,
    csr_pem: String,
    display_name: &'a str,
}

/// Errors that callers in `screens/` want to discriminate on for inline
/// error messaging.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("network failure: {0}")]
    Network(String),
    #[error("invalid shared secret")]
    BadSecret,
    #[error("server returned {status}: {body}")]
    ServerError { status: u16, body: String },
    #[error("CSR generation: {0}")]
    Csr(String),
    #[error("client build: {0}")]
    Client(String),
    #[error("response parse: {0}")]
    Parse(String),
}

/// Hit `GET <server_url>/ca-cert` with TLS validation disabled. Returns
/// the CA PEM body. The caller must immediately pin it for all subsequent
/// requests.
///
/// # Errors
/// Returns an error on network failure, non-2xx response, or empty body.
pub async fn fetch_ca_cert(server_url: &str) -> Result<String> {
    let http_client = Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()
        .context("building unauth client for ca-cert bootstrap")?;
    let resp = http_client
        .get(format!("{server_url}/ca-cert"))
        .send()
        .await
        .context("GET /ca-cert")?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("ca-cert returned {status}: {body}");
    }
    let ca_pem = resp.text().await.context("reading ca-cert body")?;
    if !ca_pem.contains("-----BEGIN CERTIFICATE-----") {
        anyhow::bail!("ca-cert body does not look like a PEM cert");
    }
    Ok(ca_pem)
}

/// POST `/enroll` over a TLS-validated connection (validating against
/// `ca_pem`). Generates the keypair + CSR locally; returns the issued
/// cert plus the locally-generated key.
///
/// # Errors
/// Returns an `EnrollError` variant on CSR generation failure, client
/// build failure, network failure, non-2xx response, or response parse
/// failure. Returns `EnrollError::BadSecret` specifically on HTTP 401.
pub async fn enroll(
    server_url: &str,
    ca_pem: &str,
    shared_secret: &str,
    display_name: &str,
) -> Result<EnrollResult, EnrollError> {
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ED25519)
        .map_err(|csr_err| EnrollError::Csr(format!("keypair: {csr_err}")))?;
    let key_pem = key_pair.serialize_pem();

    let mut csr_params = CertificateParams::new(Vec::<String>::new())
        .map_err(|csr_err| EnrollError::Csr(format!("params: {csr_err}")))?;
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "client-csr-placeholder");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&key_pair)
        .map_err(|csr_err| EnrollError::Csr(format!("serialize: {csr_err}")))?
        .pem()
        .map_err(|csr_err| EnrollError::Csr(format!("pem: {csr_err}")))?;

    let http_client = mtls_http::build_unauth_pinned_client(ca_pem)
        .map_err(|client_err| EnrollError::Client(client_err.to_string()))?;

    let body = EnrollRequest {
        shared_secret,
        csr_pem,
        display_name,
    };
    let resp = http_client
        .post(format!("{server_url}/enroll"))
        .json(&body)
        .send()
        .await
        .map_err(|net_err| EnrollError::Network(net_err.to_string()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(EnrollError::BadSecret);
    }
    if !status.is_success() {
        let response_body = resp.text().await.unwrap_or_default();
        return Err(EnrollError::ServerError {
            status: status.as_u16(),
            body: response_body,
        });
    }
    let mut parsed: EnrollResult = resp
        .json()
        .await
        .map_err(|parse_err| EnrollError::Parse(parse_err.to_string()))?;
    parsed.client_key_pem = key_pem;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_ca_cert_rejects_non_pem_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Spin up a tiny HTTP server that returns garbage on /ca-cert.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0u8; 1024]).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\nnot pem")
                .await;
        });
        let err = fetch_ca_cert(&format!("http://{addr}")).await.unwrap_err();
        assert!(
            err.to_string().contains("does not look like a PEM"),
            "got: {err}"
        );
        let _ = server.await;
    }
}
