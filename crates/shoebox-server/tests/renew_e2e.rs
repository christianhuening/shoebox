//! End-to-end: enroll a client, call /whoami, renew the cert, then call
//! /whoami again with the renewed cert and assert the same `user_id` is returned.
//!
//! Architecture note: peer-cert capture uses `shoebox_server::tls_server`
//! (the `PeerCertAcceptor` acceptor pattern), consistent with the other e2e
//! tests.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn renewed_cert_can_call_whoami_with_same_user_id() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // ── Server setup ──────────────────────────────────────────────────────────

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
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl).unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: "http://127.0.0.1:0".to_string(),
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

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // ── Root store ────────────────────────────────────────────────────────────

    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();

    // ── Step 1: generate client keypair + CSR, enroll ─────────────────────────

    let client_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let csr_pem = make_csr_pem(&client_kp);

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
            "display_name": "Alice",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "enroll should succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();
    let user_id = body["user_id"].as_str().unwrap().to_string();

    // ── Step 2: /whoami with the enrolled cert ────────────────────────────────

    let authed_http = build_authed_client(&root_store, &client_cert_pem, &client_kp);
    let resp = authed_http
        .get(format!("https://{addr}/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "whoami with enrolled cert should succeed"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["user_id"], user_id,
        "whoami should return the enrolled user_id"
    );

    // ── Step 3: POST fresh CSR to /renew using the enrolled cert ─────────────

    let renewed_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let renewal_csr_pem = make_csr_pem(&renewed_kp);

    // Disable pooling so every request triggers a fresh TLS handshake.
    let authed_http_no_pool = Client::builder()
        .use_preconfigured_tls(build_authed_client_cfg(
            &root_store,
            &client_cert_pem,
            &client_kp,
        ))
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();

    let resp = authed_http_no_pool
        .post(format!("https://{addr}/renew"))
        .json(&serde_json::json!({ "csr_pem": renewal_csr_pem }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "renew should succeed");
    let renew_body: serde_json::Value = resp.json().await.unwrap();
    let renewed_cert_pem = renew_body["client_cert_pem"].as_str().unwrap().to_string();

    // ── Step 4: /whoami with the RENEWED cert ─────────────────────────────────

    let renewed_http = Client::builder()
        .use_preconfigured_tls(build_authed_client_cfg(
            &root_store,
            &renewed_cert_pem,
            &renewed_kp,
        ))
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();

    let resp = renewed_http
        .get(format!("https://{addr}/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "whoami with renewed cert should succeed"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["user_id"], user_id,
        "whoami after renewal must return the same user_id"
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_csr_pem(kp: &KeyPair) -> String {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "placeholder");
        dn
    };
    params.serialize_request(kp).unwrap().pem().unwrap()
}

fn build_authed_client_cfg(
    root_store: &RootCertStore,
    cert_pem: &str,
    kp: &KeyPair,
) -> ClientConfig {
    let cert_der = pem_to_der(cert_pem).unwrap();
    let key_der = parse_first_private_key(&kp.serialize_pem()).unwrap();
    ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_client_auth_cert(vec![CertificateDer::from(cert_der)], key_der)
        .unwrap()
}

fn build_authed_client(root_store: &RootCertStore, cert_pem: &str, kp: &KeyPair) -> Client {
    Client::builder()
        .use_preconfigured_tls(build_authed_client_cfg(root_store, cert_pem, kp))
        .build()
        .unwrap()
}

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
