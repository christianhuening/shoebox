//! End-to-end: bootstrap server -> enroll a client -> use the cert to
//! authenticate to /whoami.
//!
//! Architecture note: the plan's `capture_peer_cert` axum middleware approach
//! does not work with axum-server 0.7 (peer certs are not surfaced via the
//! `RustlsConnectionInfo` extension). Instead we use the `PeerCertAcceptor` +
//! `CertInjectService` types extracted into `shoebox_server::tls_server`.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
async fn enroll_then_use_cert_to_call_whoami() {
    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // Bootstrap server-side state.
    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
    let conn = db.connect().unwrap();
    let secret_plaintext = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        shoebox_server::secret::EnsureOutcome::AlreadySet => {
            panic!("freshly created db should generate a secret")
        }
    };
    drop(conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    // Include 127.0.0.1 in the SAN list so reqwest can validate the cert
    // when connecting to the loopback address.
    let sans = vec!["127.0.0.1".to_string()];
    let (server_cert, server_kp) = ca.issue_server_cert(&sans).unwrap();

    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg = shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl)
        .unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
    };

    // Bind to an ephemeral port on loopback.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    // Release the std listener so axum_server can re-bind.
    drop(listener);

    // Spin up the TLS server using the library's PeerCertAcceptor path.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = {
        let state = state.clone();
        let tls_cfg = tls_cfg.clone();
        tokio::spawn(async move {
            shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
                .await
                .unwrap();
        })
    };

    // Give the server a moment to bind before issuing requests.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Build a root store from our CA cert.
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();

    // Generate client keypair + CSR.
    let client_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "placeholder");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&client_kp)
        .unwrap()
        .pem()
        .unwrap();

    // Step 1: enroll over TLS — no client cert required.
    let enroll_client_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_client_cfg)
        .build()
        .unwrap();

    let enroll_resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret_plaintext,
            "csr_pem": csr_pem,
            "display_name": "Alice",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(enroll_resp.status(), 200, "enroll should succeed");
    let body: serde_json::Value = enroll_resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();
    let user_id = body["user_id"].as_str().unwrap().to_string();

    // Step 2: build a TLS client that presents the new cert and call /whoami.
    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&client_kp.serialize_pem()).unwrap();
    let authed_client_cfg = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(
            vec![CertificateDer::from(client_cert_der)],
            client_key_der,
        )
        .unwrap();
    let authed_http = Client::builder()
        .use_preconfigured_tls(authed_client_cfg)
        .build()
        .unwrap();

    let whoami_resp = authed_http
        .get(format!("https://{addr}/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(whoami_resp.status(), 200, "whoami should succeed");
    let body: serde_json::Value = whoami_resp.json().await.unwrap();
    assert_eq!(
        body["user_id"], user_id,
        "whoami user_id should match the enrolled user"
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
