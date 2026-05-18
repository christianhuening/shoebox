//! End-to-end: spawn a real shoebox-server in-process, run the
//! client's enroll -> cert-store -> mTLS-client -> replica-open ->
//! create-user -> load-stats flow against it. Verifies the wizard's
//! plumbing without going through Iced's runtime.
//!
//! Skipped when `sqld` is not on PATH (and `SHOEBOX_SQLD_PATH` is unset),
//! mirroring the gating used by `shoebox-server`'s `proxy_e2e.rs` and
//! `locks_e2e.rs`.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn first_run_round_trips_to_library_state() {
    if shoebox_server::skip_unless_sqld!() {
        return;
    }

    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let test_db = shoebox_server::test_helpers::TestDb::start().await;
    let data_dir = test_db.data_dir.path().to_path_buf();
    let cache_dir = data_dir.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let db = test_db.db.clone();
    let setup_conn = db.connect().unwrap();
    let shared_secret = match shoebox_server::secret::ensure_present(&setup_conn)
        .await
        .unwrap()
    {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        shoebox_server::secret::EnsureOutcome::AlreadySet => {
            panic!("freshly created db should generate a shared secret")
        }
    };
    drop(setup_conn);

    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let mut server_sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    server_sans.push("127.0.0.1".to_string());
    let (server_cert, server_keypair) = ca.issue_server_cert(&server_sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_keypair, &ca, crl).unwrap();

    // sqld is the one already spawned by TestDb above.
    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: test_db.embedded.local_url.clone(),
        sqld_grpc_url: test_db.embedded.local_grpc_url.clone(),
        cache_dir: cache_dir.clone(),
    };

    // Bind ephemeral loopback port, then drop the std listener so
    // axum_server can re-bind.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        shoebox_server::tls_server::serve_public_tls(addr, state, tls_cfg, shutdown_rx)
            .await
            .unwrap();
    });

    // Give the server a moment to bind before issuing requests.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let server_url = format!("https://{addr}");

    // Step 1: fetch CA via /ca-cert (unauth, accepts invalid cert).
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url)
        .await
        .unwrap();
    assert!(ca_pem.contains("-----BEGIN CERTIFICATE-----"));

    // Step 2: enroll.
    let enroll_result =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "TestUser")
            .await
            .expect("enroll should succeed");
    assert!(!enroll_result.client_cert_pem.is_empty());
    assert!(!enroll_result.client_key_pem.is_empty());

    // Step 3: store the cert (use file storage to avoid keychain
    // side-effects in CI).
    let unique_server_url = format!("{server_url}/test-{}", rand_suffix());
    shoebox_client::cert_store::store_in_file(
        &unique_server_url,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .unwrap();
    let loaded = shoebox_client::cert_store::load_from_file(&unique_server_url)
        .unwrap()
        .expect("file storage round-trip should find the cert we just wrote");
    assert_eq!(loaded.0, enroll_result.client_cert_pem);
    assert_eq!(loaded.1, enroll_result.client_key_pem);
    // Clean up the on-disk cert so this test leaves no traces in the user
    // data dir.
    shoebox_client::cert_store::delete_from_file(&unique_server_url).unwrap();

    // Step 4: build mTLS client (verifies the PEMs round-trip through
    // rustls 0.23). Held so the connection-pool teardown is observable.
    let mtls_client = shoebox_client::mtls_http::build_mtls_client(
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .unwrap();
    let _ = mtls_client;

    // Step 5: open the libSQL embedded replica + initial sync.
    let client_tmp = TempDir::new().unwrap();
    let replica_path = client_tmp.path().join("catalog.db");
    let replica = shoebox_client::replica::Replica::open(
        &replica_path,
        &server_url,
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .await
    .expect("replica open");
    replica.sync().await.expect("initial replica sync");

    let conn = replica.conn().expect("opening replica connection");
    let users = shoebox_client::screens::profile_picker::load_users(&conn)
        .await
        .unwrap();
    // /enroll created exactly one user for "TestUser".
    assert_eq!(
        users.len(),
        1,
        "expected exactly one user after enrollment, got {users:?}"
    );
    assert_eq!(users[0].display_name, "TestUser");

    // Step 6: create a second user via the helper.
    let new_user = shoebox_client::screens::profile_picker::create_user(&conn, "Second")
        .await
        .unwrap();
    assert_eq!(new_user.display_name, "Second");

    // Step 7: re-sync and re-read; should see two users now.
    replica.sync().await.expect("post-write replica sync");
    let users_again = shoebox_client::screens::profile_picker::load_users(&conn)
        .await
        .unwrap();
    assert_eq!(users_again.len(), 2);

    // Step 8: load library stats for the first (enrolled) user.
    let stats = shoebox_client::screens::library::load_stats(&conn, Some(&users[0].id))
        .await
        .unwrap();
    assert_eq!(stats.schema_version, shoebox_common::SCHEMA_VERSION);
    assert_eq!(stats.active_user_display_name, "TestUser");

    let _ = shutdown_tx.send(());
    let _ = server.await;
    test_db.shutdown().await;
}

/// Unique-ish suffix for file-storage paths so reruns of this test don't
/// collide with stale state.
fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{nanos:x}")
}
