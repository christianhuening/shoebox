//! End-to-end: server with embedded sqld + mTLS proxy; authenticated
//! HTTP request reaches the embedded sqld through the proxy.
//!
//! Skipped when `sqld` is not on PATH (and `SHOEBOX_SQLD_PATH` is unset).
//! Mirrors the gating convention used by
//! `sqld_embed::tests::starts_subprocess_if_sqld_present`.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn libsql_http_request_reaches_embedded_sqld_through_proxy() {
    if shoebox_server::skip_unless_sqld!() {
        return;
    }

    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Bootstrap server-side state through sqld (single source of truth).
    let test_db = shoebox_server::test_helpers::TestDb::start().await;
    let db = test_db.db.clone();
    let conn = db.connect().unwrap();
    let secret_plaintext = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        shoebox_server::secret::EnsureOutcome::AlreadySet => {
            panic!("freshly created db should generate a secret")
        }
    };
    drop(conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut server_sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    server_sans.push("127.0.0.1".to_string());
    let (server_cert, server_keypair) = ca.issue_server_cert(&server_sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_keypair, &ca, crl).unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: test_db.embedded.local_url.clone(),
        sqld_grpc_url: test_db.embedded.local_grpc_url.clone(),
        cache_dir: cache_dir.clone(),
    };

    // Bind ephemeral loopback port, then drop the std listener so axum_server
    // can re-bind.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

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

    // Trust store with our CA root.
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();

    // Enroll a client cert (unauthenticated reqwest, validates server cert).
    let client_keypair = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "placeholder");
        dn
    };
    let csr_pem = csr_params
        .serialize_request(&client_keypair)
        .unwrap()
        .pem()
        .unwrap();

    let enroll_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_cfg)
        .build()
        .unwrap();

    let enroll_resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret_plaintext,
            "csr_pem": csr_pem,
            "display_name": "ProxyTest",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(enroll_resp.status(), 200, "enroll should succeed");
    let enroll_body: serde_json::Value = enroll_resp.json().await.unwrap();
    let client_cert_pem = enroll_body["client_cert_pem"].as_str().unwrap().to_string();

    // Build an authed reqwest client that presents the enrolled cert.
    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&client_keypair.serialize_pem()).unwrap();
    let authed_cfg = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(vec![CertificateDer::from(client_cert_der)], client_key_der)
        .unwrap();
    let authed_http = Client::builder()
        .use_preconfigured_tls(authed_cfg)
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();

    // Hit sqld's Hrana v2 pipeline endpoint through the proxy with a
    // minimal "close-baton" request. Success indicates the proxy is
    // wired correctly; 401 would mean the auth extractor rejected us;
    // 502 would mean the upstream is unreachable. sqld doesn't expose
    // a /v1/health or /v2/health path, so probing those is meaningless
    // — `/v2/pipeline` is the actual Hrana endpoint we care about.
    let probe_resp = authed_http
        .post(format!("https://{addr}/v2/pipeline"))
        .header("content-type", "application/json")
        .body(r#"{"requests":[]}"#)
        .send()
        .await
        .unwrap();
    assert!(
        probe_resp.status().is_success(),
        "expected /v2/pipeline through the proxy to return 2xx, got: {}",
        probe_resp.status()
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
    test_db.shutdown().await;
}

// ── PEM helpers ───────────────────────────────────────────────────────────────

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::Item;
    let mut cursor = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cursor).transpose() {
        if let Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cursor = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cursor).transpose() {
        match item {
            Item::Pkcs8Key(key) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(key)),
            Item::Pkcs1Key(key) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(key)),
            Item::Sec1Key(key) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(key)),
            _ => {}
        }
    }
    None
}
