//! End-to-end: `cert_renewal::run_one` fires `/renew` when `not_after`
//! is within 30 days and persists the new cert serial to `client.toml`.
//!
//! Skipped when `sqld` is not on PATH (and `SHOEBOX_SQLD_PATH` is unset),
//! mirroring the gating used by `first_run_e2e.rs`, `replica_e2e.rs`,
//! and `shoebox-server`'s `proxy_e2e.rs` / `locks_e2e.rs`.

use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn renewal_fires_when_under_30_days_remaining() {
    if shoebox_server::skip_unless_sqld!() {
        return;
    }

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

    // Bootstrap CA + enroll the initial client cert.
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url)
        .await
        .unwrap();
    let enroll_result =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "RenewalTest")
            .await
            .unwrap();

    // Store the initial cert in the file-storage backend. The cert store
    // is namespaced by `server_url`, and each test run gets a fresh
    // ephemeral port (different `server_url` each run), so no collision
    // with stale state from prior runs.
    shoebox_client::cert_store::store_in_file(
        &server_url,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .unwrap();

    // Build the mTLS client `run_one` will use to hit /renew.
    let mtls_client = shoebox_client::mtls_http::build_mtls_client(
        &ca_pem,
        &enroll_result.client_cert_pem,
        &enroll_result.client_key_pem,
    )
    .unwrap();

    // Stage an isolated client.toml so the renewal's config write
    // doesn't touch the real user-config dir.
    let cfg_tmp = TempDir::new().unwrap();
    let cfg_path = cfg_tmp.path().join("client.toml");
    let initial_cfg = shoebox_client::config::ClientConfig {
        server_url: server_url.clone(),
        cert_serial_hex: enroll_result.cert_serial_hex.clone(),
        last_active_user_id: None,
    };
    initial_cfg.write_to(&cfg_path).unwrap();

    // Construct a RenewalContext whose `not_after_unix` is 5 days from
    // now — well under the 30-day renewal threshold, so the first
    // `run_one` tick must fire `/renew`.
    let now_secs_u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let now_secs = i64::try_from(now_secs_u64).unwrap_or(i64::MAX);
    let near_expiry_unix = now_secs + 5 * 24 * 60 * 60;
    let renewal_context = Arc::new(parking_lot::Mutex::new(
        shoebox_client::cert_renewal::RenewalContext {
            server_url: server_url.clone(),
            client: mtls_client.clone(),
            config_path: cfg_path.clone(),
            not_after_unix: near_expiry_unix,
        },
    ));

    // Run exactly one renewal tick.
    shoebox_client::cert_renewal::run_one(&renewal_context)
        .await
        .expect("run_one should succeed against a live /renew endpoint");

    // Assert the cert serial in client.toml changed (the renewal swapped
    // in a freshly-issued cert). We only assert on the serial in the
    // config — the on-disk cert bytes may be in keychain on systems where
    // that backend is available, which is fine; the config write is the
    // source of truth for the "did renewal fire" question.
    let post_cfg = shoebox_client::config::ClientConfig::read_from(&cfg_path).unwrap();
    assert_ne!(
        post_cfg.cert_serial_hex, enroll_result.cert_serial_hex,
        "renewal should have replaced cert_serial_hex"
    );
    assert!(
        !post_cfg.cert_serial_hex.is_empty(),
        "post-renewal cert_serial_hex should be non-empty"
    );

    // Confirm the in-memory not_after was advanced past the near-expiry
    // value we seeded.
    let post_not_after = renewal_context.lock().not_after_unix;
    assert!(
        post_not_after > near_expiry_unix,
        "renewal should have advanced not_after_unix (was {near_expiry_unix}, now {post_not_after})"
    );

    // Best-effort cleanup of the per-test cert file.
    let _ = shoebox_client::cert_store::delete_from_file(&server_url);

    let _ = shutdown_tx.send(());
    let _ = server.await;
    test_db.shutdown().await;
}
