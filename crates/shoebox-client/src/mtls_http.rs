//! Build a `reqwest::Client` configured for mTLS against the paired
//! shoebox-server. Pure builder; caller owns the client and caches it
//! in `AppState`.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use std::time::Duration;

/// Build a `reqwest::Client` that:
///   - validates the server cert against `root_cert_pem` (the CA PEM
///     returned by `GET /ca-cert`);
///   - presents `(client_cert_pem, client_key_pem)` for mTLS auth;
///   - times out at 30 s; pool stays small.
///
/// # Errors
/// Returns an error if any PEM is malformed or rustls rejects the config.
pub fn build_mtls_client(
    root_cert_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> Result<Client> {
    let root_store = build_root_store(root_cert_pem)?;
    let client_cert_chain = parse_cert_chain(client_cert_pem)?;
    let client_key = parse_private_key(client_key_pem)?;

    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(client_cert_chain, client_key)
        .context("building rustls client auth config")?;

    Client::builder()
        .use_preconfigured_tls(tls_config)
        .pool_max_idle_per_host(2)
        .timeout(Duration::from_secs(30))
        .build()
        .context("building reqwest mtls client")
}

/// Build a `reqwest::Client` that validates the server cert against
/// `root_cert_pem` but presents no client cert. Used during the bootstrap
/// step after `/ca-cert` returns and before `/enroll` runs.
///
/// # Errors
/// Returns an error if the root PEM is malformed.
pub fn build_unauth_pinned_client(root_cert_pem: &str) -> Result<Client> {
    let root_store = build_root_store(root_cert_pem)?;
    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Client::builder()
        .use_preconfigured_tls(tls_config)
        .pool_max_idle_per_host(1)
        .timeout(Duration::from_secs(30))
        .build()
        .context("building reqwest unauth-pinned client")
}

fn build_root_store(root_cert_pem: &str) -> Result<RootCertStore> {
    let mut root_store = RootCertStore::empty();
    let mut cursor = root_cert_pem.as_bytes();
    for cert_result in rustls_pemfile::certs(&mut cursor) {
        let cert_der = cert_result.context("parsing CA cert PEM")?;
        root_store
            .add(cert_der)
            .context("adding CA cert to root store")?;
    }
    if root_store.is_empty() {
        return Err(anyhow!("no certificates found in CA PEM"));
    }
    Ok(root_store)
}

fn parse_cert_chain(cert_pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut cursor = cert_pem.as_bytes();
    let mut chain = Vec::new();
    for cert_result in rustls_pemfile::certs(&mut cursor) {
        chain.push(cert_result.context("parsing client cert PEM")?);
    }
    if chain.is_empty() {
        return Err(anyhow!("no certificates found in client PEM"));
    }
    Ok(chain)
}

fn parse_private_key(key_pem: &str) -> Result<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cursor = key_pem.as_bytes();
    while let Some(item_result) = rustls_pemfile::read_one(&mut cursor).transpose() {
        let item = item_result.context("parsing client key PEM")?;
        match item {
            Item::Pkcs8Key(k) => return Ok(PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Ok(PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Ok(PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    Err(anyhow!("no private key found in key PEM"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{generate_simple_self_signed, CertifiedKey};

    fn install_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn fresh_cert_pair() -> (String, String) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["test.local".to_string()]).unwrap();
        (cert.pem(), signing_key.serialize_pem())
    }

    #[test]
    fn build_unauth_client_with_valid_ca() {
        install_crypto_provider();
        let (ca_pem, _key) = fresh_cert_pair();
        build_unauth_pinned_client(&ca_pem).unwrap();
    }

    #[test]
    fn build_unauth_client_rejects_garbage_ca() {
        install_crypto_provider();
        let err = build_unauth_pinned_client("not a pem").unwrap_err();
        assert!(err.to_string().contains("no certificates"), "got: {err}");
    }

    #[test]
    fn build_mtls_client_with_valid_inputs() {
        install_crypto_provider();
        let (ca_pem, _ca_key) = fresh_cert_pair();
        let (client_cert, client_key) = fresh_cert_pair();
        build_mtls_client(&ca_pem, &client_cert, &client_key).unwrap();
    }

    #[test]
    fn build_mtls_client_rejects_garbage_key() {
        install_crypto_provider();
        let (ca_pem, _ca_key) = fresh_cert_pair();
        let (client_cert, _) = fresh_cert_pair();
        let err = build_mtls_client(&ca_pem, &client_cert, "garbage").unwrap_err();
        assert!(err.to_string().contains("no private key"), "got: {err}");
    }
}
