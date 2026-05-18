//! End-to-end: enroll, use cert successfully, revoke serial, refresh CRL,
//! subsequent connection with the same cert is rejected at TLS handshake.
//!
//! Architecture note: peer-cert capture uses `shoebox_server::tls_server`
//! (the `PeerCertAcceptor` acceptor pattern). The production CRL refresher
//! runs every 30 s; here we call `CrlCache::replace` directly to avoid
//! waiting.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn revoked_cert_cannot_reconnect() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
    let conn = db.connect().unwrap();
    let shoebox_server::secret::EnsureOutcome::Generated {
        plaintext: secret_plaintext,
    } = shoebox_server::secret::ensure_present(&conn).await.unwrap()
    else {
        panic!("freshly created db should generate a secret")
    };
    drop(conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let sans = vec!["127.0.0.1".to_string()];
    let (server_cert, server_kp) = ca.issue_server_cert(&sans).unwrap();

    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl.clone())
            .unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: "http://127.0.0.1:0".to_string(),
        sqld_grpc_url: "http://127.0.0.1:0".to_string(),
        cache_dir: tmp.path().to_path_buf(),
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await
            .unwrap();
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Build root store from CA cert.
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();

    // Generate client keypair + CSR.
    let client_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "x");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&client_kp)
        .unwrap()
        .pem()
        .unwrap();

    // Enroll (no client cert yet).
    let enroll_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_cfg)
        .build()
        .unwrap();
    let resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret_plaintext,
            "csr_pem": csr_pem,
            "display_name": "Bob",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();
    let cert_serial = body["cert_serial_hex"].as_str().unwrap().to_string();

    // Build authenticated client using the new cert.  We disable connection
    // pooling so that every `send()` opens a fresh TCP connection and triggers
    // a new TLS handshake — this is essential for testing cert revocation,
    // because the verifier runs only at handshake time.
    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();

    // Helper closure: build a fresh no-pool authenticated client.
    let make_authed_client = || {
        let client_key_der = parse_first_private_key(&client_kp.serialize_pem()).unwrap();
        let authed_cfg = ClientConfig::builder()
            .with_root_certificates(root_store.clone())
            .with_client_auth_cert(
                vec![CertificateDer::from(client_cert_der.clone())],
                client_key_der,
            )
            .unwrap();
        Client::builder()
            .use_preconfigured_tls(authed_cfg)
            // Disable connection pooling: every request opens a new TCP
            // connection and performs a fresh TLS handshake, which is when
            // the CrlAwareVerifier runs.
            .pool_max_idle_per_host(0)
            .build()
            .unwrap()
    };

    // First connection with the cert: /whoami should succeed.
    let resp = make_authed_client()
        .get(format!("https://{addr}/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "first /whoami should succeed");

    // Revoke the cert: persist to DB and immediately update the in-memory
    // CrlCache (simulating what the 30-second background refresher would do).
    db.insert_revoked_cert(&cert_serial, Some("test"), None)
        .await
        .unwrap();
    let mut revoked_set = std::collections::HashSet::new();
    revoked_set.insert(cert_serial.clone());
    crl.replace(revoked_set);

    // Second connection: the TLS handshake should be rejected because the
    // `CrlAwareVerifier` finds the serial in the CrlCache.
    // reqwest surfaces this as a connection-level error (not an HTTP response).
    let result = make_authed_client()
        .get(format!("https://{addr}/whoami"))
        .send()
        .await;
    assert!(
        result.is_err(),
        "revoked cert must fail at TLS handshake; got: {result:?}"
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

// ── PEM helpers ───────────────────────────────────────────────────────────────

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        if let Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    None
}
