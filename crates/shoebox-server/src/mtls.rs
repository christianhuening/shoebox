//! TLS server configuration and client-cert verifier.

use anyhow::{anyhow, Result};
use parking_lot::RwLock;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::{DigitallySignedStruct, RootCertStore, ServerConfig};
use std::collections::HashSet;
use std::sync::Arc;

use crate::ca::Ca;
use crate::ca::IssuedCert;

/// Install the default rustls crypto provider exactly once at startup.
pub fn install_crypto_provider() {
    use rustls::crypto::ring::default_provider;
    let _ = default_provider().install_default();
}

/// In-memory snapshot of revoked cert serials. Refreshed periodically by
/// a background task spawned at server startup.
#[derive(Clone, Default, Debug)]
pub struct CrlCache(Arc<RwLock<HashSet<String>>>);

impl CrlCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, serials: HashSet<String>) {
        *self.0.write() = serials;
    }

    #[must_use]
    pub fn contains(&self, serial_hex: &str) -> bool {
        self.0.read().contains(serial_hex)
    }
}

/// Verifier that delegates to the inner `WebPkiClientVerifier` and then rejects
/// any cert whose serial is in the CRL cache.
#[derive(Debug)]
struct CrlAwareVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    crl: CrlCache,
}

impl ClientCertVerifier for CrlAwareVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: rustls::pki_types::UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let verified = self
            .inner
            .verify_client_cert(end_entity, intermediates, now)?;
        let serial_hex = {
            use x509_parser::prelude::*;
            match X509Certificate::from_der(end_entity.as_ref()) {
                Ok((_, parsed)) => hex::encode(parsed.serial.to_bytes_be()),
                Err(_) => {
                    return Err(rustls::Error::General(
                        "could not parse client cert serial".into(),
                    ));
                }
            }
        };
        if self.crl.contains(&serial_hex) {
            return Err(rustls::Error::General(format!(
                "client cert revoked (serial={serial_hex})"
            )));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }
}

/// Build a server TLS config that:
///   - serves our server cert
///   - REQUESTS (but does not require) a client cert
///   - if a client cert is presented, it must chain to our CA root
///   - rejects any cert whose serial is in the CRL cache
///
/// Per-route "require auth" is enforced separately in middleware
/// (Task 10) by checking whether the peer cert extension was populated.
pub fn mtls_server_config(
    server_cert: &IssuedCert,
    server_keypair: &rcgen::KeyPair,
    ca: &Ca,
    crl: CrlCache,
) -> Result<Arc<ServerConfig>> {
    let cert_der = CertificateDer::from(server_cert.cert_der.clone());
    let key_pem = server_keypair.serialize_pem();
    let key_der = parse_first_private_key(&key_pem)?;

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .map_err(|e| anyhow!("loading CA root into trust store: {e}"))?;
    let roots = Arc::new(roots);

    let inner_verifier = WebPkiClientVerifier::builder(roots)
        .allow_unauthenticated()
        .build()
        .map_err(|e| anyhow!("building client verifier: {e}"))?;

    let verifier: Arc<dyn ClientCertVerifier> = Arc::new(CrlAwareVerifier {
        inner: inner_verifier,
        crl,
    });

    let mut config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|e| anyhow!("building rustls ServerConfig: {e}"))?;
    // ALPN advertises h2 (gRPC replication) ahead of http/1.1 (Hrana,
    // /enroll, /thumbs, /locks). Clients negotiate whichever they need;
    // the proxy further branches on Content-Type to route gRPC traffic
    // to sqld's grpc port and Hrana traffic to sqld's http port.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
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
