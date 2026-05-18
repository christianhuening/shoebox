//! End-to-end: enroll two clients (Alice + Bob), exercise the develop-lock
//! REST flow between them — acquire, conflict, takeover, release, re-acquire.
//!
//! Sub-1-3-5 routed Db through sqld, so this test now depends on sqld too
//! (TestDb spawns one). Skipped when sqld is not on PATH.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn develop_lock_acquire_takeover_release() {
    if shoebox_server::skip_unless_sqld!() {
        return;
    }
    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().to_path_buf();
    let cache_dir = temp_dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Bootstrap server-side state.
    let test_db = shoebox_server::test_helpers::TestDb::start().await;
    let db = test_db.db.clone();
    let conn = db.connect().unwrap();
    let secret_plaintext = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        shoebox_server::secret::EnsureOutcome::AlreadySet => {
            panic!("freshly created db should generate a secret")
        }
    };

    // Seed a variant we can lock. The lock helpers don't enforce FK against
    // `variants`, but we seed one anyway so the schema stays consistent and
    // any future FK addition won't silently break this test.
    let seed_timestamp = 1_000_i64;
    conn.execute(
        "INSERT INTO users (id, display_name, created_at) VALUES ('seed', 'seed', ?1)",
        [seed_timestamp],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO photos (id, file_size, file_format, imported_at) \
         VALUES ('p1', 100, 'PEF', ?1)",
        [seed_timestamp],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO variants (id, photo_id, variant_index, created_by, created_at, \
         develop_settings_json, develop_settings_version, develop_updated_at, develop_updated_by) \
         VALUES ('v1', 'p1', 0, 'seed', ?1, '{}', 1, ?1, 'seed')",
        [seed_timestamp],
    )
    .await
    .unwrap();
    drop(conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut server_sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    server_sans.push("127.0.0.1".to_string());
    let (server_cert, server_keypair) = ca.issue_server_cert(&server_sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_keypair, &ca, crl).unwrap();

    // TestDb already spawned sqld; use its loopback URLs in the state.
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

    // Enroll two clients with independent certs => independent session IDs.
    // The /enroll handler now creates a `sessions` row per issued cert, so
    // there's no need to seed those rows here.
    let alice_identity = enroll(addr, &secret_plaintext, &root_store, "Alice").await;
    let bob_identity = enroll(addr, &secret_plaintext, &root_store, "Bob").await;

    let alice = &alice_identity.http_client;
    let bob = &bob_identity.http_client;

    // Alice acquires — 200.
    let alice_acquire_resp = alice
        .post(format!("https://{addr}/locks/v1"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        alice_acquire_resp.status(),
        200,
        "Alice's initial acquire should succeed"
    );

    // Bob tries to acquire — should get 409 Conflict.
    let bob_conflict_resp = bob
        .post(format!("https://{addr}/locks/v1"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_conflict_resp.status(),
        409,
        "Bob's acquire should conflict while Alice holds the lock"
    );

    // Bob requests takeover — 200.
    let bob_takeover_resp = bob
        .post(format!("https://{addr}/locks/v1/takeover"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_takeover_resp.status(),
        200,
        "Bob's takeover request should be recorded"
    );

    // Alice releases — 204 No Content.
    let alice_release_resp = alice
        .delete(format!("https://{addr}/locks/v1"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        alice_release_resp.status(),
        204,
        "Alice's release should succeed"
    );

    // Bob acquires successfully now that the lock is free — 200.
    let bob_acquire_resp = bob
        .post(format!("https://{addr}/locks/v1"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bob_acquire_resp.status(),
        200,
        "Bob's re-acquire after Alice's release should succeed"
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

/// Bundle returned by `enroll`: an authed reqwest client that presents the
/// issued client cert on every request.
struct EnrolledClient {
    http_client: Client,
}

/// Generate a fresh keypair + CSR, enroll over plain TLS (no client cert),
/// then return an `EnrolledClient` whose reqwest `Client` presents the
/// issued cert on every request.
async fn enroll(
    addr: std::net::SocketAddr,
    secret: &str,
    root_store: &RootCertStore,
    display_name: &str,
) -> EnrolledClient {
    let client_keypair = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut distinguished_name = DistinguishedName::new();
        // CN is overwritten by the server with the assigned user_id; this is
        // just a placeholder so the CSR is well-formed.
        distinguished_name.push(rcgen::DnType::CommonName, "x");
        distinguished_name
    };
    let csr_pem = csr_params
        .serialize_request(&client_keypair)
        .unwrap()
        .pem()
        .unwrap();

    let enroll_client_config = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_client_config)
        .build()
        .unwrap();

    let enroll_resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret,
            "csr_pem": csr_pem,
            "display_name": display_name,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        enroll_resp.status(),
        200,
        "enroll for {display_name} should succeed"
    );
    let enroll_body: serde_json::Value = enroll_resp.json().await.unwrap();
    let client_cert_pem = enroll_body["client_cert_pem"].as_str().unwrap().to_string();

    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&client_keypair.serialize_pem()).unwrap();
    let tls_client_config = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_client_auth_cert(vec![CertificateDer::from(client_cert_der)], client_key_der)
        .unwrap();
    let http_client = Client::builder()
        .use_preconfigured_tls(tls_client_config)
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();

    EnrolledClient { http_client }
}

// ── PEM helpers ───────────────────────────────────────────────────────────────

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::Item;
    let mut pem_cursor = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut pem_cursor).transpose() {
        if let Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut pem_cursor = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut pem_cursor).transpose() {
        match item {
            Item::Pkcs8Key(key) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(key)),
            Item::Pkcs1Key(key) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(key)),
            Item::Sec1Key(key) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(key)),
            _ => {}
        }
    }
    None
}
