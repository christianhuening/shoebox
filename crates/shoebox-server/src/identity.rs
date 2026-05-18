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
        // Use `to_bytes_be()` — not `raw_serial()` — so the encoding matches
        // the one used in `ca.rs` and `mtls.rs`.  `raw_serial()` returns the
        // raw DER content octets, which include a leading 0x00 padding byte
        // when the serial's high bit is set (i.e. ~50 % of the time with
        // rcgen's 20-byte random serials).  `to_bytes_be()` strips that
        // padding, giving the canonical big-endian integer representation.
        let serial_hex = hex::encode(parsed.serial.to_bytes_be());
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

impl<S: Send + Sync> FromRequestParts<S> for ClientIdentity {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
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

impl<S: Send + Sync> FromRequestParts<S> for MaybeClientIdentity {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeClientIdentity(
            ClientIdentity::from_request_parts(parts, state).await.ok(),
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
        KeyUsagePurpose, SerialNumber,
    };
    use x509_parser::prelude::*;

    /// Issue a cert from a throw-away CA with the given serial bytes.
    ///
    /// The serial is set explicitly so tests can force particular bit patterns
    /// without relying on rcgen's random generation.
    fn issue_test_cert_der_with_serial(serial_bytes: &[u8]) -> Vec<u8> {
        // Build a self-signed one-off CA, then wrap it in an `Issuer` for
        // the leaf-signing step (rcgen 0.14 signed_by takes &Issuer).
        let ca_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        ca_params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "test-ca");
            dn
        };
        let ca_issuer = rcgen::Issuer::new(ca_params, ca_kp);

        // Build a leaf cert with the supplied serial.
        let leaf_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let mut leaf_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        leaf_params.serial_number = Some(SerialNumber::from_slice(serial_bytes));
        leaf_params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "test-user");
            dn.push(DnType::OrganizationalUnitName, "test-machine");
            dn
        };
        let leaf_cert = leaf_params.signed_by(&leaf_kp, &ca_issuer).unwrap();
        leaf_cert.der().to_vec()
    }

    /// Regression test: `PeerCertChain::from_der` must produce the same serial
    /// hex as `x509_parser`'s `serial.to_bytes_be()` — the encoding used by
    /// `ca.rs` and `mtls.rs`.
    ///
    /// We test two serials deterministically:
    ///
    /// * A low-bit serial (high bit clear): `raw_serial()` and `to_bytes_be()`
    ///   agree — both produce the same bytes.
    /// * A high-bit serial (high bit set): `raw_serial()` adds a leading `0x00`
    ///   DER padding byte, making its hex string two chars longer.  `to_bytes_be()`
    ///   strips the padding and gives the canonical integer.  This is the bug that
    ///   existed when `identity.rs` used `raw_serial()`.
    #[test]
    fn leaf_serial_hex_matches_to_bytes_be() {
        // Low-bit serial: 0x7F — high bit clear, no DER padding needed.
        let low_bit_serial = &[0x7F, 0xAB, 0xCD];
        // High-bit serial: 0x80... — high bit set, DER requires a 0x00 prefix.
        let high_bit_serial = &[0x80, 0xAB, 0xCD];

        for serial_bytes in [low_bit_serial.as_ref(), high_bit_serial.as_ref()] {
            let der = issue_test_cert_der_with_serial(serial_bytes);

            // The encoding produced by `PeerCertChain::from_der`.
            let chain =
                PeerCertChain::from_der(der.clone()).expect("from_der must parse the test cert");

            // The canonical encoding used by ca.rs / mtls.rs.
            let (_, parsed) = X509Certificate::from_der(&der).unwrap();
            let canonical_hex = hex::encode(parsed.serial.to_bytes_be());

            // `from_der` must always match `to_bytes_be()`.
            assert_eq!(
                chain.leaf_serial_hex, canonical_hex,
                "leaf_serial_hex diverges from to_bytes_be() — serial encoding regression \
                 (serial_bytes={serial_bytes:02x?})"
            );
        }

        // Document the trap: for the high-bit serial, `raw_serial()` differs from
        // `to_bytes_be()` by a leading 0x00 padding byte.  This confirms that
        // `to_bytes_be()` is the correct choice and `raw_serial()` must not be used.
        let high_bit_der = issue_test_cert_der_with_serial(high_bit_serial);
        let (_, parsed) = X509Certificate::from_der(&high_bit_der).unwrap();
        let raw_hex = hex::encode(parsed.raw_serial());
        let canonical_hex = hex::encode(parsed.serial.to_bytes_be());
        assert_ne!(
            raw_hex, canonical_hex,
            "raw_serial() and to_bytes_be() should differ for a high-bit serial"
        );
        assert!(
            raw_hex.starts_with("00"),
            "raw_serial() must start with 0x00 padding for a high-bit serial"
        );
        assert_eq!(
            raw_hex.len(),
            canonical_hex.len() + 2,
            "raw_serial() should be exactly one byte (two hex chars) longer"
        );
    }
}
