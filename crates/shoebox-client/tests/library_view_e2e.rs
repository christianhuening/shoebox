//! End-to-end: spin up a real shoebox-server, enroll a single client,
//! seed a folder + 3 photos + 4 variants through the libsql proxy, then
//! drive the `library_state` helpers (folder tree, grid, detail, rating,
//! keyword, virtual copy) and assert each one round-trips through the
//! replica.
//!
//! Skipped when `sqld` is not on PATH (and `SHOEBOX_SQLD_PATH` is unset),
//! mirroring the gating used by `first_run_e2e.rs` and
//! `shoebox-server`'s `proxy_e2e.rs` / `locks_e2e.rs`.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn library_view_round_trips_folder_grid_detail_and_edits() {
    // Skip gate (matches server's proxy_e2e.rs / locks_e2e.rs pattern).
    let sqld_binary_name =
        std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    if which::which(&sqld_binary_name).is_err() {
        eprintln!(
            "skipping library_view_e2e: sqld not on PATH (set SHOEBOX_SQLD_PATH to override)"
        );
        return;
    }

    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_tmp = TempDir::new().unwrap();
    let data_dir = server_tmp.path().to_path_buf();
    let cache_dir = server_tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Bootstrap server-side state (mirrors locks_e2e.rs / proxy_e2e.rs).
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

    // Enroll Alice.
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url)
        .await
        .unwrap();
    let enroll_result =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "Alice")
            .await
            .expect("enroll should succeed");

    // Open Alice's replica + initial sync.
    let client_tmp = TempDir::new().unwrap();
    let replica = shoebox_client::replica::Replica::open(
        &client_tmp.path().join("catalog.db"),
        &server_url,
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .await
    .expect("replica open");
    replica.sync().await.expect("initial replica sync");

    let conn = replica.conn().expect("opening replica connection");

    // Load the enrolled user (just one, "Alice").
    let users = shoebox_client::screens::profile_picker::load_users(&conn)
        .await
        .unwrap();
    assert_eq!(users.len(), 1, "expected exactly one user, got {users:?}");
    assert_eq!(users[0].display_name, "Alice");
    let user_id = users[0].id.clone();

    // Seed catalog through the libsql proxy.
    conn.execute(
        "INSERT INTO folders(id, path, name) VALUES('f1', '/seed', 'seed')",
        (),
    )
    .await
    .unwrap();

    for (photo_id, file_path, captured_at) in [
        ("p1", "/seed/one.pef", 100_i64),
        ("p2", "/seed/two.pef", 200_i64),
        ("p3", "/seed/three.pef", 300_i64),
    ] {
        conn.execute(
            "INSERT INTO photos(id, file_size, file_format, captured_at, imported_at) \
             VALUES (?1, 100, 'PEF', ?2, 0)",
            (photo_id, captured_at),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO photo_files(id, photo_id, folder_id, path, file_mtime, last_seen_at) \
             VALUES (?1, ?2, 'f1', ?3, 0, 0)",
            (format!("{photo_id}-file"), photo_id, file_path),
        )
        .await
        .unwrap();
    }

    // 4 variants total: p1 -> index 0 + index 1 (virtual copy seeded
    // directly), p2 -> index 0, p3 -> index 0.
    let variant_seed: [(&str, &str, i64); 4] = [
        ("v_p1_0", "p1", 0),
        ("v_p1_1", "p1", 1),
        ("v_p2_0", "p2", 0),
        ("v_p3_0", "p3", 0),
    ];
    for (variant_id, photo_id, variant_index) in variant_seed {
        conn.execute(
            "INSERT INTO variants(id, photo_id, variant_index, created_by, created_at, \
                develop_settings_json, develop_settings_version, \
                develop_updated_at, develop_updated_by) \
             VALUES (?1, ?2, ?3, ?4, 0, '{}', 1, 0, ?4)",
            (variant_id, photo_id, variant_index, user_id.as_str()),
        )
        .await
        .unwrap();
    }

    // Push the seed inserts upstream.
    replica.sync().await.expect("post-seed replica sync");

    // Assertions on the library_state helpers.
    let folder_tree = shoebox_client::library_state::load_folder_tree(&conn)
        .await
        .unwrap();
    assert_eq!(folder_tree.len(), 1, "expected one folder, got {folder_tree:?}");
    let folder_id = folder_tree[0].id.clone();

    let grid = shoebox_client::library_state::load_grid_for_folder(&conn, &folder_id, &user_id)
        .await
        .unwrap();
    assert_eq!(grid.len(), 4, "expected 4 grid cells, got {}", grid.len());

    // upsert_rating on the first variant.
    let first_variant_id = grid[0].variant_id.clone();
    let first_photo_id = grid[0].photo_id.clone();
    shoebox_client::library_state::upsert_rating(&conn, &first_variant_id, &user_id, 4)
        .await
        .unwrap();
    let detail = shoebox_client::library_state::load_detail(&conn, &first_variant_id, &user_id)
        .await
        .unwrap();
    assert_eq!(detail.rating, 4);

    // add_keyword on the first photo.
    shoebox_client::library_state::add_keyword(&conn, &first_photo_id, &user_id, "tested")
        .await
        .unwrap();
    let detail_with_keyword =
        shoebox_client::library_state::load_detail(&conn, &first_variant_id, &user_id)
            .await
            .unwrap();
    assert!(
        detail_with_keyword
            .keywords
            .iter()
            .any(|keyword_row| keyword_row.name == "tested"),
        "expected keyword 'tested' on first photo, got {:?}",
        detail_with_keyword.keywords
    );

    // create_virtual_copy on the first photo: grid grows from 4 -> 5.
    shoebox_client::library_state::create_virtual_copy(&conn, &first_photo_id, &user_id)
        .await
        .unwrap();
    let grid_after_copy =
        shoebox_client::library_state::load_grid_for_folder(&conn, &folder_id, &user_id)
            .await
            .unwrap();
    assert_eq!(
        grid_after_copy.len(),
        5,
        "expected 5 grid cells after virtual copy, got {}",
        grid_after_copy.len()
    );

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded_sqld.shutdown().await;
}
