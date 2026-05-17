//! Local libSQL embedded replica that syncs from `shoebox-server`'s
//! mTLS-proxied `sqld` at `<server_url>/v1/...`.
//!
//! Wraps `libsql::Builder::new_remote_replica` with a custom
//! `tower::Service<http::Uri>` connector built on hyper 0.14 +
//! hyper-rustls 0.25 (libsql 0.6's own connector-trait shape). The
//! connector presents this machine's mTLS client cert and pins the
//! shoebox CA — same trust material as `mtls_http.rs`, just expressed
//! against the rustls 0.22 instance that hyper-rustls 0.25 requires.
//!
//! The remote URL passed to libsql is `<server_url>/v1` (the proxy
//! prefix exposed by `shoebox-server`'s axum router), which forwards
//! to the embedded `sqld` subprocess described in Plan 1.3.

use anyhow::{anyhow, Context, Result};
use hyper_014::client::connect::dns::GaiResolver;
use hyper_014::client::HttpConnector;
use hyper_rustls_025::{HttpsConnector, HttpsConnectorBuilder};
use libsql::{Builder, Connection, Database};
use rustls_022::pki_types::{CertificateDer, PrivateKeyDer};
use rustls_022::{ClientConfig as Rustls022ClientConfig, RootCertStore};
use std::path::Path;
use std::sync::Arc;

/// Local libSQL embedded replica with mTLS sync against shoebox-server.
pub struct Replica {
    database: Arc<Database>,
}

impl std::fmt::Debug for Replica {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Replica").finish_non_exhaustive()
    }
}

impl Replica {
    /// Open (or create) a local replica file at `local_path`, syncing
    /// against `<server_url>/v1` over the mTLS proxy.
    ///
    /// Pass the same PEM bundles you'd hand to `mtls_http::build_mtls_client`:
    /// `ca_pem` is the shoebox CA returned by `GET /ca-cert`, and
    /// `(client_cert_pem, client_key_pem)` is the enrolled cert/key from
    /// `cert_store`.
    ///
    /// # Errors
    /// Returns an error if the local replica dir can't be created, the
    /// PEMs can't be parsed, rustls rejects the mTLS config, or libsql
    /// fails to open the embedded replica.
    pub async fn open(
        local_path: &Path,
        server_url: &str,
        ca_pem: &str,
        client_cert_pem: &str,
        client_key_pem: &str,
    ) -> Result<Self> {
        if let Some(parent_dir) = local_path.parent() {
            std::fs::create_dir_all(parent_dir).with_context(|| {
                format!("creating replica parent directory {}", parent_dir.display())
            })?;
        }

        let path_string = local_path
            .to_str()
            .ok_or_else(|| anyhow!("replica path is not valid UTF-8: {}", local_path.display()))?
            .to_owned();

        let sync_url = build_sync_url(server_url);

        let tls_connector = build_mtls_connector(ca_pem, client_cert_pem, client_key_pem)
            .context("building libsql mTLS connector")?;

        // Auth token is unused on the wire — `shoebox-server`'s proxy
        // authenticates via the client cert. Pass an empty string.
        let database = Builder::new_remote_replica(&path_string, sync_url, String::new())
            .connector(tls_connector)
            // We drive sync manually from main.rs on a 30 s ticker
            // (Plan 1.4 Task 18) — no background timer here.
            .read_your_writes(true)
            .build()
            .await
            .context("opening libsql embedded replica")?;

        Ok(Self {
            database: Arc::new(database),
        })
    }

    /// Run an incremental WAL catch-up against the server. Returns the
    /// committed frame number, or 0 if libsql didn't report one (e.g.
    /// the local replica was already at the head).
    ///
    /// # Errors
    /// Returns the underlying libsql error on transport / protocol failure.
    pub async fn sync(&self) -> Result<u64> {
        let replicated = self.database.sync().await.context("libsql replica sync")?;
        Ok(replicated.frame_no().unwrap_or(0))
    }

    /// Hand out a fresh `Connection` for queries. libsql `Connection`s
    /// are cheap and safe to create per-query; the underlying replica
    /// state is shared via `Arc<Database>`.
    ///
    /// # Errors
    /// Returns an error if libsql can't construct the connection.
    pub fn conn(&self) -> Result<Connection> {
        self.database
            .connect()
            .context("creating libsql connection from replica")
    }
}

/// Trim any trailing slash from the user-supplied server URL and append
/// the `/v1` path segment that the shoebox-server proxy listens on.
fn build_sync_url(server_url: &str) -> String {
    let trimmed = server_url.trim_end_matches('/');
    format!("{trimmed}/v1")
}

/// Build a hyper 0.14 HTTPS connector preloaded with the shoebox CA as
/// the only trusted root and our enrolled cert/key for mTLS.
fn build_mtls_connector(
    ca_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> Result<HttpsConnector<HttpConnector<GaiResolver>>> {
    let root_store = build_root_store(ca_pem)?;
    let client_chain = parse_cert_chain(client_cert_pem)?;
    let client_key = parse_private_key(client_key_pem)?;

    let tls_config = Rustls022ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(client_chain, client_key)
        .context("building rustls 0.22 client-auth config for libsql")?;

    let mut plain_http: HttpConnector<GaiResolver> = HttpConnector::new();
    plain_http.enforce_http(false);
    plain_http.set_nodelay(true);

    let tls_connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(plain_http);

    Ok(tls_connector)
}

fn build_root_store(ca_pem: &str) -> Result<RootCertStore> {
    let mut root_store = RootCertStore::empty();
    let mut cursor = ca_pem.as_bytes();
    let mut any_added = false;
    for cert_result in rustls_pemfile::certs(&mut cursor) {
        let cert_der: CertificateDer<'static> =
            cert_result.context("parsing CA cert PEM for libsql connector")?;
        root_store
            .add(cert_der)
            .context("adding CA cert to libsql root store")?;
        any_added = true;
    }
    if !any_added {
        return Err(anyhow!(
            "no certificates found in CA PEM for libsql connector"
        ));
    }
    Ok(root_store)
}

fn parse_cert_chain(cert_pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut cursor = cert_pem.as_bytes();
    let mut chain = Vec::new();
    for cert_result in rustls_pemfile::certs(&mut cursor) {
        chain.push(cert_result.context("parsing client cert PEM for libsql connector")?);
    }
    if chain.is_empty() {
        return Err(anyhow!(
            "no certificates found in client PEM for libsql connector"
        ));
    }
    Ok(chain)
}

fn parse_private_key(key_pem: &str) -> Result<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cursor = key_pem.as_bytes();
    while let Some(item_result) = rustls_pemfile::read_one(&mut cursor).transpose() {
        let item = item_result.context("parsing client key PEM for libsql connector")?;
        match item {
            Item::Pkcs8Key(key_bytes) => return Ok(PrivateKeyDer::Pkcs8(key_bytes)),
            Item::Pkcs1Key(key_bytes) => return Ok(PrivateKeyDer::Pkcs1(key_bytes)),
            Item::Sec1Key(key_bytes) => return Ok(PrivateKeyDer::Sec1(key_bytes)),
            _ => {}
        }
    }
    Err(anyhow!(
        "no private key found in client key PEM for libsql connector"
    ))
}
