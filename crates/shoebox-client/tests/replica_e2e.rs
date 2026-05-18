//! End-to-end: seed the server's catalog with users + a photo BEFORE the
//! client enrolls, then assert the client's replica reads the seeded data
//! back and that a write from the client round-trips to the server's
//! catalog through the proxy.
//!
//! Skipped when `sqld` is not on PATH (and `SHOEBOX_SQLD_PATH` is unset),
//! mirroring the gating used by `first_run_e2e.rs` and
//! `shoebox-server`'s `proxy_e2e.rs` / `locks_e2e.rs`.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn replica_round_trips_writes_back_to_server() {
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

    // Seed two users and a photo BEFORE the client touches anything.
    let seed_timestamp = 1_000_000_i64;
    setup_conn
        .execute(
            "INSERT INTO users (id, display_name, created_at) VALUES ('seed-1', 'Alice', ?1)",
            [seed_timestamp],
        )
        .await
        .unwrap();
    setup_conn
        .execute(
            "INSERT INTO users (id, display_name, created_at) VALUES ('seed-2', 'Bob', ?1)",
            [seed_timestamp],
        )
        .await
        .unwrap();
    setup_conn
        .execute(
            "INSERT INTO photos (id, file_size, file_format, imported_at) \
             VALUES ('photo-1', 100, 'PEF', ?1)",
            [seed_timestamp],
        )
        .await
        .unwrap();
    drop(setup_conn);

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

    // Enroll a client.
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url)
        .await
        .unwrap();
    let enroll_result =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "ReplicaTest")
            .await
            .unwrap();

    // Open replica + initial sync.
    let client_tmp = TempDir::new().unwrap();
    let replica = shoebox_client::replica::Replica::open(
        &client_tmp.path().join("catalog.db"),
        &server_url,
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .await
    .unwrap();
    replica.sync().await.unwrap();

    let conn = replica.conn().unwrap();

    // Assert seeded data is visible through the replica.
    let mut user_count_row = conn.query("SELECT COUNT(*) FROM users", ()).await.unwrap();
    let user_count: i64 = user_count_row
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    // 2 seeded ("Alice", "Bob") + 1 from /enroll ("ReplicaTest") = 3.
    assert_eq!(user_count, 3);
    let mut photo_count_row = conn.query("SELECT COUNT(*) FROM photos", ()).await.unwrap();
    let photo_count: i64 = photo_count_row
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(photo_count, 1);

    // Write a new user from the client; re-sync; verify it shows up on
    // a fresh server-side read.
    conn.execute(
        "INSERT INTO users (id, display_name, created_at) VALUES ('client-side', 'Cara', ?1)",
        [seed_timestamp],
    )
    .await
    .unwrap();
    replica.sync().await.unwrap();

    let server_side_conn = db.connect().unwrap();
    let mut server_view_row = server_side_conn
        .query(
            "SELECT display_name FROM users WHERE id = 'client-side'",
            (),
        )
        .await
        .unwrap();
    let server_view_name: String = server_view_row
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(server_view_name, "Cara");

    let _ = shutdown_tx.send(());
    let _ = server.await;
    test_db.shutdown().await;
}
