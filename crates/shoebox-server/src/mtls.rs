//! TLS server configuration and client-cert verifier.

use anyhow::{anyhow, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::sync::Arc;

use crate::ca::Ca;
use crate::ca::IssuedCert;

/// Install the default rustls crypto provider exactly once at startup.
pub fn install_crypto_provider() {
    use rustls::crypto::ring::default_provider;
    let _ = default_provider().install_default();
}

/// Build a server TLS config that:
///   - serves our server cert
///   - REQUESTS (but does not require) a client cert
///   - if a client cert is presented, it must chain to our CA root
///
/// Per-route "require auth" is enforced separately in middleware
/// (Task 10) by checking whether the peer cert extension was populated.
pub fn mtls_server_config(
    server_cert: &IssuedCert,
    server_keypair: &rcgen::KeyPair,
    ca: &Ca,
) -> Result<Arc<ServerConfig>> {
    let cert_der = CertificateDer::from(server_cert.cert_der.clone());
    let key_pem = server_keypair.serialize_pem();
    let key_der = parse_first_private_key(&key_pem)?;

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .map_err(|e| anyhow!("loading CA root into trust store: {e}"))?;
    let roots = Arc::new(roots);

    // WebPkiClientVerifier in optional mode: request but don't require.
    let verifier = WebPkiClientVerifier::builder(roots)
        .allow_unauthenticated()
        .build()
        .map_err(|e| anyhow!("building client verifier: {e}"))?;

    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
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
