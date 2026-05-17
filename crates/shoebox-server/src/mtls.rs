//! TLS server configuration and (in Task 9) client-cert verifier.

use anyhow::{anyhow, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::sync::Arc;

use crate::ca::IssuedCert;

/// Install the default rustls crypto provider exactly once at startup.
pub fn install_crypto_provider() {
    use rustls::crypto::ring::default_provider;
    let _ = default_provider().install_default();
}

/// Build a server TLS config from an issued server cert + its keypair.
/// Does NOT yet require client certs (that's Task 9).
pub fn server_only_tls_config(
    server_cert: &IssuedCert,
    server_keypair: &rcgen::KeyPair,
) -> Result<Arc<ServerConfig>> {
    let cert_der = CertificateDer::from(server_cert.cert_der.clone());
    let key_pem = server_keypair.serialize_pem();
    let key_der = parse_first_private_key(&key_pem)?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|e| anyhow!("building rustls ServerConfig: {e}"))?;
    Ok(Arc::new(config))
}

fn parse_first_private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Ok(PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Ok(PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Ok(PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    Err(anyhow!("no private key found in PEM"))
}
