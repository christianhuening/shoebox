use shoebox_server::{ca, config, db, http, identity, logging, mdns, mtls, secret};
use std::sync::Arc;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();
    mtls::install_crypto_provider();

    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    let cfg = if let Some(p) = cfg_path {
        tracing::info!(event = "config.load", path = %p, "loading config file");
        config::Config::load_from_path(std::path::Path::new(&p))?
    } else {
        tracing::info!(event = "config.load", source = "env", "no SHOEBOX_CONFIG; building from env");
        config::Config::from_env_with_defaults()
    };

    tracing::info!(
        event = "startup",
        server_name = %cfg.server_name,
        bind_addr = %cfg.bind_addr,
        health_bind_addr = %cfg.health_bind_addr,
        data_dir = ?cfg.data_dir,
        "shoebox-server starting"
    );

    std::fs::create_dir_all(&cfg.data_dir)?;
    let db = Arc::new(db::Db::open(&cfg.data_dir.join("catalog.db")).await?);

    // Bootstrap CA and ensure server cert.
    let ca = Arc::new(ca::Ca::open(&cfg.data_dir)?);
    let sans = ca::build_server_sans(&cfg.server_name, &cfg.extra_sans);
    let (server_cert, server_kp) = ca.issue_server_cert(&sans)?;

    // Build the CRL cache, populate it once synchronously, then spawn a
    // background task to refresh it every 30 seconds.
    let crl = mtls::CrlCache::new();
    refresh_crl(&db, &crl).await?;
    tokio::spawn({
        let db_for_crl = db.clone();
        let crl_for_task = crl.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                if let Err(e) = refresh_crl(&db_for_crl, &crl_for_task).await {
                    tracing::warn!(event = "crl.refresh.error", error = %e);
                }
            }
        }
    });

    let tls_cfg = mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl.clone())?;

    // Bootstrap shared catalog secret.
    let conn = db.connect()?;
    match secret::ensure_present(&conn).await? {
        secret::EnsureOutcome::Generated { plaintext } => {
            tracing::warn!(
                event = "secret.generated",
                secret = %plaintext,
                "Generated new enrollment secret — share with users out-of-band; \
                 it will not be shown again"
            );
        }
        secret::EnsureOutcome::AlreadySet => {
            tracing::info!(event = "secret.loaded", "enrollment secret already configured");
        }
    }

    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca,
    };

    let broadcaster = mdns::MdnsBroadcaster::start(
        &cfg.server_name,
        cfg.bind_addr.port(),
        shoebox_common::SCHEMA_VERSION,
        &mdns::local_ips(),
    )?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown.signal", "received ctrl-c, shutting down");
        let _ = shutdown_tx.send(());
    });

    let (shutdown_health_tx, shutdown_health_rx) = oneshot::channel();
    tokio::spawn({
        let state = state.clone();
        let addr = cfg.health_bind_addr;
        async move {
            if let Err(e) = serve_health(addr, state, shutdown_health_rx).await {
                tracing::error!(event = "health.serve.error", error = %e);
            }
        }
    });

    let result = serve_public_tls(cfg.bind_addr, state, tls_cfg, shutdown_rx).await;
    let _ = shutdown_health_tx.send(());
    broadcaster.shutdown();
    result
}

async fn serve_health(
    addr: std::net::SocketAddr,
    state: http::AppState,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(event = "http.listen.health", addr = %addr, "health server bound");
    axum::serve(listener, http::health_router(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
        })
        .await?;
    Ok(())
}

async fn serve_public_tls(
    addr: std::net::SocketAddr,
    state: http::AppState,
    tls_cfg: std::sync::Arc<rustls::ServerConfig>,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};

    let rustls_cfg = RustlsConfig::from_config(tls_cfg);
    let inner_acceptor = RustlsAcceptor::new(rustls_cfg);
    let acceptor = PeerCertAcceptor { inner: inner_acceptor };

    tracing::info!(event = "https.listen.public", addr = %addr, "public TLS server bound");
    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = shutdown.await;
        handle_for_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });

    axum_server::bind(addr)
        .acceptor(acceptor)
        .handle(handle)
        .serve(http::public_router(state).into_make_service())
        .await?;
    Ok(())
}

// ── PeerCertAcceptor ──────────────────────────────────────────────────────────
//
// Custom `Accept` implementation that wraps `RustlsAcceptor`.  After the TLS
// handshake completes we read `peer_certificates()` from the
// `tokio_rustls::server::TlsStream`, parse the leaf cert into a
// `PeerCertChain`, and wrap the per-connection service in a `CertInjectService`
// that inserts the chain (or nothing, if the client sent no cert) into every
// request's extension map.
//
// This is the only reliable way to surface peer-cert data to axum handlers in
// axum-server 0.7: the crate exposes no higher-level API for it.

/// Wraps `RustlsAcceptor` to capture the peer cert at handshake time and
/// inject it as a request extension via `CertInjectService`.
#[derive(Clone)]
struct PeerCertAcceptor {
    inner: axum_server::tls_rustls::RustlsAcceptor,
}

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

impl<I, S> axum_server::accept::Accept<I, S> for PeerCertAcceptor
where
    axum_server::tls_rustls::RustlsAcceptor: axum_server::accept::Accept<
        I,
        S,
        Stream = tokio_rustls::server::TlsStream<I>,
        Service = S,
    >,
    <axum_server::tls_rustls::RustlsAcceptor as axum_server::accept::Accept<I, S>>::Future:
        Send + 'static,
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    S: Clone + Send + 'static,
{
    type Stream = tokio_rustls::server::TlsStream<I>;
    type Service = CertInjectService<S>;
    /// Boxed future — avoids the need for `pin-project` / unsafe.
    type Future = Pin<
        Box<
            dyn Future<
                    Output = io::Result<(
                        tokio_rustls::server::TlsStream<I>,
                        CertInjectService<S>,
                    )>,
                > + Send,
        >,
    >;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let inner_future = axum_server::accept::Accept::accept(&self.inner, stream, service);
        Box::pin(async move {
            let (tls_stream, svc) = inner_future.await?;
            // Extract peer cert chain from the completed TLS handshake.
            // `get_ref()` → `(&IO, &ServerConnection)`.
            // `ServerConnection: Deref<Target = CommonState>`,
            // which has `peer_certificates() → Option<&[CertificateDer]>`.
            let peer_chain = tls_stream
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|certs| certs.first())
                .and_then(|der| identity::PeerCertChain::from_der(der.to_vec()));
            Ok((tls_stream, CertInjectService { inner: svc, peer_chain }))
        })
    }
}


// ── CertInjectService ─────────────────────────────────────────────────────────
//
// A thin tower `Service` wrapper that inserts an `Option<PeerCertChain>` into
// request extensions before forwarding to the inner service.  A `PeerCertChain`
// is present only when the client presented a valid cert during the TLS handshake.

#[derive(Clone)]
struct CertInjectService<S> {
    inner: S,
    peer_chain: Option<identity::PeerCertChain>,
}

impl<S, ReqBody> tower::Service<axum::http::Request<ReqBody>> for CertInjectService<S>
where
    S: tower::Service<axum::http::Request<ReqBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), <S as tower::Service<axum::http::Request<ReqBody>>>::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: axum::http::Request<ReqBody>) -> S::Future {
        if let Some(chain) = self.peer_chain.clone() {
            req.extensions_mut().insert(chain);
        }
        self.inner.call(req)
    }
}

// ── CRL refresh ───────────────────────────────────────────────────────────────

/// Load all revoked cert serials from the database and update the in-memory
/// CRL cache. Called once at startup and then every 30 seconds in a background
/// task, so revocation takes effect within at most one refresh interval.
async fn refresh_crl(
    db: &std::sync::Arc<db::Db>,
    crl: &mtls::CrlCache,
) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let mut rows = conn
        .query("SELECT serial_number FROM revoked_certs", ())
        .await?;
    let mut revoked_set = std::collections::HashSet::new();
    while let Some(row) = rows.next().await? {
        revoked_set.insert(row.get::<String>(0)?);
    }
    let revoked_count = revoked_set.len();
    crl.replace(revoked_set);
    tracing::debug!(event = "crl.refresh", revoked_count, "CRL cache refreshed");
    Ok(())
}
