//! Extractors for the authenticated client identity.
//!
//! # Peer-cert capture mechanism
//!
//! `axum-server` 0.7 does not expose an API for surfacing peer certificates to
//! request handlers directly.  The TLS stream is `tokio_rustls::server::TlsStream<TcpStream>`,
//! and after the handshake its `.get_ref()` returns `(&TcpStream, &ServerConnection)`.
//! `ServerConnection` implements `Deref<Target = CommonState>`, which has
//! `peer_certificates() -> Option<&[CertificateDer<'static>]>`.
//!
//! We capture this by implementing the [`axum_server::accept::Accept`] trait on a
//! `PeerCertAcceptor` wrapper (defined in `main.rs`). That acceptor:
//!   1. Delegates to the inner `RustlsAcceptor` to complete the TLS handshake.
//!   2. Reads `peer_certificates()` from the resulting `TlsStream`.
//!   3. Wraps the per-connection service in a `InjectCert` shim that inserts
//!      the `Option<PeerCertChain>` into every request's extensions.
//!
//! The `ClientIdentity` extractor then reads `PeerCertChain` out of the extensions.
//! If no cert was presented (unauthenticated request) the extension is absent and
//! `ClientIdentity::from_request_parts` returns `Err(StatusCode::UNAUTHORIZED)`.
//! `MaybeClientIdentity` wraps this as `Ok(None)` for routes like `/enroll` that
//! work with or without a cert.

use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use shoebox_common::{MachineId, UserId};
use std::str::FromStr;

// ── PeerCertChain ─────────────────────────────────────────────────────────────

/// The peer cert chain captured at TLS handshake time.
///
/// Stored as a request extension by `PeerCertAcceptor` (in `main.rs`).
/// Absent when the client did not present a certificate.
#[derive(Clone, Debug)]
pub struct PeerCertChain {
    /// Raw DER bytes of the leaf certificate.
    pub leaf_der: Vec<u8>,
    /// Hex-encoded serial number of the leaf cert.
    pub leaf_serial_hex: String,
    /// Subject CN field (mapped to `UserId`).
    pub subject_cn: String,
    /// Subject OU field (mapped to `MachineId`).
    pub subject_ou: String,
}

impl PeerCertChain {
    /// Parse a DER-encoded leaf certificate into the fields we care about.
    ///
    /// Returns `None` if the bytes are not a valid X.509 certificate or the
    /// required fields are absent.
    pub fn from_der(der: Vec<u8>) -> Option<Self> {
        use x509_parser::prelude::*;
        let (_, parsed) = X509Certificate::from_der(&der).ok()?;
        let serial_hex = hex::encode(parsed.raw_serial());
        let subject_cn = parsed
            .subject()
            .iter_common_name()
            .next()?
            .as_str()
            .ok()?
            .to_string();
        let subject_ou = parsed
            .subject()
            .iter_organizational_unit()
            .next()
            .and_then(|ou| ou.as_str().ok())
            .map(str::to_string)
            .unwrap_or_default();
        Some(Self {
            leaf_der: der,
            leaf_serial_hex: serial_hex,
            subject_cn,
            subject_ou,
        })
    }
}

// ── ClientIdentity ────────────────────────────────────────────────────────────

/// Verified identity extracted from the mTLS peer certificate.
///
/// Use this extractor on routes that require authentication.  If the client
/// did not present a cert, or the cert's CN/OU cannot be parsed as `UserId` /
/// `MachineId`, the request is rejected with `401 Unauthorized`.
#[derive(Clone, Debug)]
pub struct ClientIdentity {
    pub user_id: UserId,
    pub machine_id: MachineId,
    pub cert_serial_hex: String,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for ClientIdentity {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let chain = parts
            .extensions
            .get::<PeerCertChain>()
            .ok_or((StatusCode::UNAUTHORIZED, "no client certificate presented"))?;

        let user_id = UserId::from_str(&chain.subject_cn)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid user id in cert CN"))?;
        let machine_id = MachineId::from_str(&chain.subject_ou)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid machine id in cert OU"))?;

        Ok(ClientIdentity {
            user_id,
            machine_id,
            cert_serial_hex: chain.leaf_serial_hex.clone(),
        })
    }
}

// ── MaybeClientIdentity ───────────────────────────────────────────────────────

/// Optional identity extractor: yields `None` instead of rejecting when no
/// cert is present.  Used by `/enroll` which must accept unauthenticated
/// requests (it is the endpoint that grants you a cert).
pub struct MaybeClientIdentity(pub Option<ClientIdentity>);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for MaybeClientIdentity {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(MaybeClientIdentity(
            ClientIdentity::from_request_parts(parts, state).await.ok(),
        ))
    }
}
