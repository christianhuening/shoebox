//! Public TLS server helpers: the custom `Accept` implementation that captures
//! peer certificates at handshake time, and a `serve_public_tls` function used
//! by both `main.rs` and integration tests.
//!
//! # Why a custom `Accept`?
//!
//! `axum-server` 0.7 does not expose an API for surfacing peer certificates to
//! request handlers. The only reliable path is to implement the
//! [`axum_server::accept::Accept`] trait on a wrapper that reads
//! `peer_certificates()` from the `TlsStream` immediately after the handshake
//! and injects it into every request's extension map via a thin `tower::Service`
//! shim (`CertInjectService`).

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::Result;
use tokio::sync::oneshot;

use crate::http;
use crate::identity;

// ── PeerCertAcceptor ──────────────────────────────────────────────────────────

/// Wraps `RustlsAcceptor` to capture the peer cert at handshake time and
/// inject it as a request extension via `CertInjectService`.
#[derive(Clone)]
pub struct PeerCertAcceptor {
    pub inner: axum_server::tls_rustls::RustlsAcceptor,
}

impl<I, S> axum_server::accept::Accept<I, S> for PeerCertAcceptor
where
    axum_server::tls_rustls::RustlsAcceptor:
        axum_server::accept::Accept<I, S, Stream = tokio_rustls::server::TlsStream<I>, Service = S>,
    <axum_server::tls_rustls::RustlsAcceptor as axum_server::accept::Accept<I, S>>::Future:
        Send + 'static,
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    S: Clone + Send + 'static,
{
    type Stream = tokio_rustls::server::TlsStream<I>;
    type Service = CertInjectService<S>;
    type Future = Pin<
        Box<
            dyn Future<
                    Output = io::Result<(tokio_rustls::server::TlsStream<I>, CertInjectService<S>)>,
                > + Send,
        >,
    >;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let inner_future = axum_server::accept::Accept::accept(&self.inner, stream, service);
        Box::pin(async move {
            let (tls_stream, svc) = inner_future.await?;
            let peer_chain = tls_stream
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|certs| certs.first())
                .and_then(|der| identity::PeerCertChain::from_der(der.to_vec()));
            Ok((
                tls_stream,
                CertInjectService {
                    inner: svc,
                    peer_chain,
                },
            ))
        })
    }
}

// ── CertInjectService ─────────────────────────────────────────────────────────

/// Tower `Service` wrapper that inserts an `Option<PeerCertChain>` into
/// request extensions before forwarding to the inner service.
#[derive(Clone)]
pub struct CertInjectService<S> {
    pub inner: S,
    pub peer_chain: Option<identity::PeerCertChain>,
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

// ── serve_public_tls ─────────────────────────────────────────────────────────

/// Serve the public TLS listener with the `PeerCertAcceptor` acceptor.
///
/// Binds to `addr`, wraps the `tls_cfg` in a `PeerCertAcceptor`, and runs
/// until `shutdown` fires (graceful 5 s drain).
pub async fn serve_public_tls(
    addr: std::net::SocketAddr,
    state: http::AppState,
    tls_cfg: Arc<rustls::ServerConfig>,
    shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};

    let rustls_cfg = RustlsConfig::from_config(tls_cfg);
    let inner_acceptor = RustlsAcceptor::new(rustls_cfg);
    let acceptor = PeerCertAcceptor {
        inner: inner_acceptor,
    };

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
