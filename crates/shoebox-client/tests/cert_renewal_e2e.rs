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
    // Skip gate (matches first_run_e2e.rs / replica_e2e.rs pattern).
    let sqld_binary_name =
        std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    if which::which(&sqld_binary_name).is_err() {
        eprintln!(
            "skipping cert_renewal_e2e: sqld not on PATH (set SHOEBOX_SQLD_PATH to override)"
        );
        return;
    }

    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_tmp = TempDir::new().unwrap();
    let data_dir = server_tmp.path().to_path_buf();
    let cache_dir = server_tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Bootstrap server-side state (mirrors first_run_e2e.rs).
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

    // Bootstrap CA + enroll the initial client cert.
    let ca_pem = shoebox_client::enrollment::fetch_ca_cert(&server_url)
        .await
        .unwrap();
    let enroll_result =
        shoebox_client::enrollment::enroll(&server_url, &ca_pem, &shared_secret, "RenewalTest")
            .await
            .unwrap();

    // Store the initial cert in a per-test file-storage namespace so
    // reruns don't collide and we leave no global side effects.
    let test_server_url = format!("{server_url}/renewal-{}", rand_suffix());
    shoebox_client::cert_store::store_in_file(
        &test_server_url,
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
        server_url: test_server_url.clone(),
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
            server_url: test_server_url.clone(),
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

    // Best-effort cleanup of the per-test cert file. Keychain entries
    // (if used) are namespaced by test_server_url and harmless.
    let _ = shoebox_client::cert_store::delete_from_file(&test_server_url);

    let _ = shutdown_tx.send(());
    let _ = server.await;
    embedded_sqld.shutdown().await;
}

/// Unique-ish suffix for the per-test file-storage namespace so reruns
/// don't collide with stale state.
fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{nanos:x}")
}
