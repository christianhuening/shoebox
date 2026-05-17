//! End-to-end: spin up a real shoebox-server, enroll two clients ("Alice"
//! and "Bob") against the same shared secret, seed a single variant via
//! Alice, then walk through the four lock states:
//!
//!   1. Free initially (both replicas).
//!   2. Alice acquires -> Alice sees `HeldByYou`, Bob sees `HeldByOther`.
//!   3. Bob requests takeover -> Alice sees `HeldByYouTakeoverPending`,
//!      Bob sees `HeldByOtherTakeoverPending`.
//!   4. Alice releases -> both see `Free` again.
//!
//! Skipped when `sqld` is not on PATH (and `SHOEBOX_SQLD_PATH` is unset),
//! mirroring the gating used by `first_run_e2e.rs` and
//! `shoebox-server`'s `proxy_e2e.rs` / `locks_e2e.rs`.

use std::sync::Arc;
use tempfile::TempDir;

use shoebox_client::library_state::{LockAcquireOutcome, LockStatus};

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn library_lock_walks_through_all_four_states() {
    // Skip gate (matches server's proxy_e2e.rs / locks_e2e.rs pattern).
    let sqld_binary_name =
        std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    if which::which(&sqld_binary_name).is_err() {
        eprintln!(
            "skipping library_lock_e2e: sqld not on PATH (set SHOEBOX_SQLD_PATH to override)"
        );
        return;
    }

    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_tmp = TempDir::new().unwrap();
    let data_dir = server_tmp.path().to_path_buf();
    let cache_dir = server_tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Bootstrap server-side state.
    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
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

    // Spawn the sqld subprocess. `start` takes the DATA DIR (it creates
    // a `sqld/` subdir inside), not a .db path.
    let embedded_sqld = shoebox_server::sqld_embed::start(data_dir.clone())
        .await
        .unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
        sqld_url: embedded_sqld.local_url.clone(),
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

    // Step 1: fetch CA once; both clients use the same root.
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url)
        .await
        .unwrap();

    // Step 2: enroll Alice and Bob against the same shared secret. Each
    // enrollment yields its own cert + key and creates a fresh users row.
    let enroll_alice =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "Alice")
            .await
            .expect("enroll Alice");
    let enroll_bob =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "Bob")
            .await
            .expect("enroll Bob");

    // Step 3: per-client mTLS reqwest + per-client replica.
    let alice_client = shoebox_client::mtls_http::build_mtls_client(
        &ca_pem,
        &enroll_alice.client_cert_pem,
        &enroll_alice.client_key_pem,
    )
    .unwrap();
    let bob_client = shoebox_client::mtls_http::build_mtls_client(
        &ca_pem,
        &enroll_bob.client_cert_pem,
        &enroll_bob.client_key_pem,
    )
    .unwrap();

    let alice_tmp = TempDir::new().unwrap();
    let replica_a = shoebox_client::replica::Replica::open(
        &alice_tmp.path().join("catalog.db"),
        &server_url,
        &ca_pem,
        &enroll_alice.client_cert_pem,
        &enroll_alice.client_key_pem,
    )
    .await
    .expect("alice replica open");
    replica_a.sync().await.expect("alice initial sync");
    let conn_a = replica_a.conn().expect("alice replica conn");

    let bob_tmp = TempDir::new().unwrap();
    let replica_b = shoebox_client::replica::Replica::open(
        &bob_tmp.path().join("catalog.db"),
        &server_url,
        &ca_pem,
        &enroll_bob.client_cert_pem,
        &enroll_bob.client_key_pem,
    )
    .await
    .expect("bob replica open");
    replica_b.sync().await.expect("bob initial sync");
    let conn_b = replica_b.conn().expect("bob replica conn");

    // Step 4: resolve each user's id by display name (the enroll handler
    // creates one users row per enrollment).
    let users_after_enroll = shoebox_client::screens::profile_picker::load_users(&conn_a)
        .await
        .unwrap();
    assert_eq!(
        users_after_enroll.len(),
        2,
        "expected exactly two users (Alice + Bob), got {users_after_enroll:?}"
    );
    let alice_user_id = users_after_enroll
        .iter()
        .find(|row| row.display_name == "Alice")
        .expect("Alice user row")
        .id
        .clone();
    let bob_user_id = users_after_enroll
        .iter()
        .find(|row| row.display_name == "Bob")
        .expect("Bob user row")
        .id
        .clone();

    // Step 5: seed a single variant via Alice's conn.
    conn_a
        .execute(
            "INSERT INTO folders(id, path, name) VALUES('f1', '/lock-seed', 'lock-seed')",
            (),
        )
        .await
        .unwrap();
    conn_a
        .execute(
            "INSERT INTO photos(id, file_size, file_format, captured_at, imported_at) \
             VALUES ('p1', 100, 'PEF', 100, 0)",
            (),
        )
        .await
        .unwrap();
    conn_a
        .execute(
            "INSERT INTO photo_files(id, photo_id, folder_id, path, file_mtime, last_seen_at) \
             VALUES ('p1-file', 'p1', 'f1', '/lock-seed/one.pef', 0, 0)",
            (),
        )
        .await
        .unwrap();
    let variant_id = "v_p1_0".to_string();
    conn_a
        .execute(
            "INSERT INTO variants(id, photo_id, variant_index, created_by, created_at, \
                develop_settings_json, develop_settings_version, \
                develop_updated_at, develop_updated_by) \
             VALUES (?1, 'p1', 0, ?2, 0, '{}', 1, 0, ?2)",
            (variant_id.as_str(), alice_user_id.as_str()),
        )
        .await
        .unwrap();
    replica_a.sync().await.expect("alice post-seed sync");
    replica_b.sync().await.expect("bob post-seed sync");

    // ---- State 1: Free initially.
    let alice_status = shoebox_client::library_state::load_lock_status(
        &conn_a,
        &variant_id,
        &alice_user_id,
    )
    .await
    .unwrap();
    assert_eq!(alice_status, LockStatus::Free, "alice initial state");
    let bob_status =
        shoebox_client::library_state::load_lock_status(&conn_b, &variant_id, &bob_user_id)
            .await
            .unwrap();
    assert_eq!(bob_status, LockStatus::Free, "bob initial state");

    // ---- State 2: Alice acquires.
    let acquire_outcome = shoebox_client::library_state::http_acquire_lock(
        &alice_client,
        &server_url,
        &variant_id,
    )
    .await
    .unwrap();
    assert_eq!(acquire_outcome, LockAcquireOutcome::Acquired);
    replica_a.sync().await.expect("alice sync after acquire");
    replica_b.sync().await.expect("bob sync after acquire");

    let alice_status = shoebox_client::library_state::load_lock_status(
        &conn_a,
        &variant_id,
        &alice_user_id,
    )
    .await
    .unwrap();
    assert_eq!(alice_status, LockStatus::HeldByYou, "alice after acquire");
    let bob_status =
        shoebox_client::library_state::load_lock_status(&conn_b, &variant_id, &bob_user_id)
            .await
            .unwrap();
    assert_eq!(
        bob_status,
        LockStatus::HeldByOther {
            holder_display_name: "Alice".to_string()
        },
        "bob after alice-acquire"
    );

    // ---- State 3: Bob requests takeover.
    shoebox_client::library_state::http_request_takeover(&bob_client, &server_url, &variant_id)
        .await
        .unwrap();
    replica_a.sync().await.expect("alice sync after takeover");
    replica_b.sync().await.expect("bob sync after takeover");

    let alice_status = shoebox_client::library_state::load_lock_status(
        &conn_a,
        &variant_id,
        &alice_user_id,
    )
    .await
    .unwrap();
    assert_eq!(
        alice_status,
        LockStatus::HeldByYouTakeoverPending {
            requested_by_display_name: "Bob".to_string()
        },
        "alice after bob takeover request"
    );
    let bob_status =
        shoebox_client::library_state::load_lock_status(&conn_b, &variant_id, &bob_user_id)
            .await
            .unwrap();
    assert_eq!(
        bob_status,
        LockStatus::HeldByOtherTakeoverPending {
            holder_display_name: "Alice".to_string()
        },
        "bob after own takeover request"
    );

    // ---- State 4: Alice releases.
    shoebox_client::library_state::http_release_lock(&alice_client, &server_url, &variant_id)
        .await
        .unwrap();
    replica_a.sync().await.expect("alice sync after release");
    replica_b.sync().await.expect("bob sync after release");

    let alice_status = shoebox_client::library_state::load_lock_status(
        &conn_a,
        &variant_id,
        &alice_user_id,
    )
    .await
    .unwrap();
    assert_eq!(alice_status, LockStatus::Free, "alice after release");
    let bob_status =
        shoebox_client::library_state::load_lock_status(&conn_b, &variant_id, &bob_user_id)
            .await
            .unwrap();
    assert_eq!(bob_status, LockStatus::Free, "bob after release");

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded_sqld.shutdown().await;
}
