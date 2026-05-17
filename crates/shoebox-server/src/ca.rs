//! Internal Certificate Authority for shoebox-server.
//!
//! On first launch, generates an Ed25519 root keypair and self-signed
//! root cert. Stores the key (mode 0600) and cert PEM in the data dir
//! under `ca/`. On subsequent launches, loads them.
//!
//! Issues:
//! - server certs signed by the root, SANs from network interfaces + extras
//! - client leaf certs signed by the root, subject CN = `user_id`, OU = `machine_id`

use anyhow::{anyhow, Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SubjectPublicKeyInfo,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

use shoebox_common::{MachineId, UserId};

pub const ROOT_CA_VALIDITY_DAYS: i64 = 10 * 365;
pub const SERVER_CERT_VALIDITY_DAYS: i64 = 90;
pub const CLIENT_CERT_VALIDITY_DAYS: i64 = 90;
pub const NOT_BEFORE_BACKDATE_SECS: i64 = 300; // 5 minutes

/// The live root CA state held in memory.
///
/// `root_cert` is kept solely so it can be passed as the `issuer` argument
/// to `CertificateParams::signed_by`. Its DER is NOT the persisted cert when
/// loaded from disk (a fresh self-signed cert is produced from the loaded
/// params to satisfy the API); the persisted cert is tracked via
/// `root_cert_pem` and `root_cert_der`.
pub struct Ca {
    pub root_keypair: KeyPair,
    /// A `Certificate` object whose params match the root CA, used as the
    /// issuer reference when signing leaf certs.
    root_cert: Certificate,
    pub root_cert_der: Vec<u8>,
    pub root_cert_pem: String,
    pub data_dir: PathBuf,
}

pub struct IssuedCert {
    pub cert_pem: String,
    pub cert_der: Vec<u8>,
    pub serial_hex: String,
    pub not_after: OffsetDateTime,
}

impl Ca {
    /// Load the CA from disk, generating fresh material on first launch.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let ca_dir = data_dir.join("ca");
        fs::create_dir_all(&ca_dir).with_context(|| format!("creating {}", ca_dir.display()))?;

        let key_path = ca_dir.join("ca.key");
        let cert_path = ca_dir.join("ca.crt");

        if key_path.exists() && cert_path.exists() {
            tracing::info!(event = "ca.load", "loading existing root CA");
            let key_pem = fs::read_to_string(&key_path)
                .with_context(|| format!("reading {}", key_path.display()))?;
            let cert_pem = fs::read_to_string(&cert_path)
                .with_context(|| format!("reading {}", cert_path.display()))?;

            let root_keypair = KeyPair::from_pem(&key_pem)
                .map_err(|e| anyhow!("parsing CA key PEM: {e}"))?;

            // Parse the persisted cert's params so we can reconstruct an issuer
            // Certificate. The reconstructed cert may have a new serial but its
            // DN matches — that is all rcgen needs when we call signed_by().
            let loaded_params = CertificateParams::from_ca_cert_pem(&cert_pem)
                .map_err(|e| anyhow!("parsing CA cert PEM: {e}"))?;
            let root_cert = loaded_params
                .self_signed(&root_keypair)
                .map_err(|e| anyhow!("reconstructing issuer certificate: {e}"))?;

            let cert_der = pem_to_der(&cert_pem)
                .ok_or_else(|| anyhow!("CA cert PEM has no CERTIFICATE block"))?;

            Ok(Self {
                root_keypair,
                root_cert,
                root_cert_der: cert_der,
                root_cert_pem: cert_pem,
                data_dir: data_dir.to_path_buf(),
            })
        } else {
            tracing::info!(event = "ca.bootstrap", "generating new root CA");
            let root_keypair = KeyPair::generate_for(&rcgen::PKCS_ED25519)
                .map_err(|e| anyhow!("generating CA keypair: {e}"))?;

            let mut params = CertificateParams::new(Vec::<String>::new())
                .map_err(|e| anyhow!("building CA params: {e}"))?;
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            params.distinguished_name = {
                let mut dn = DistinguishedName::new();
                dn.push(DnType::CommonName, "shoebox-server internal CA");
                dn
            };
            let now = OffsetDateTime::now_utc();
            params.not_before = now - Duration::seconds(NOT_BEFORE_BACKDATE_SECS);
            params.not_after = now + Duration::days(ROOT_CA_VALIDITY_DAYS);

            let root_cert = params
                .self_signed(&root_keypair)
                .map_err(|e| anyhow!("self-signing CA cert: {e}"))?;

            let root_cert_pem = root_cert.pem();
            let root_cert_der = root_cert.der().to_vec();
            let key_pem = root_keypair.serialize_pem();

            fs::write(&cert_path, &root_cert_pem)
                .with_context(|| format!("writing {}", cert_path.display()))?;
            fs::write(&key_path, &key_pem)
                .with_context(|| format!("writing {}", key_path.display()))?;
            set_owner_only(&key_path)?;

            Ok(Self {
                root_keypair,
                root_cert,
                root_cert_der,
                root_cert_pem,
                data_dir: data_dir.to_path_buf(),
            })
        }
    }

    /// Issue a server cert covering the given SANs. Returns the issued
    /// material; the caller persists / hot-loads as appropriate.
    pub fn issue_server_cert(&self, sans: &[String]) -> Result<(IssuedCert, KeyPair)> {
        let kp = KeyPair::generate_for(&rcgen::PKCS_ED25519)
            .map_err(|e| anyhow!("generating server keypair: {e}"))?;

        let mut params = CertificateParams::new(sans.to_vec())
            .map_err(|e| anyhow!("building server cert params: {e}"))?;
        params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "shoebox-server");
            dn
        };
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::seconds(NOT_BEFORE_BACKDATE_SECS);
        let not_after = now + Duration::days(SERVER_CERT_VALIDITY_DAYS);
        params.not_after = not_after;

        // rcgen 0.13 API: signed_by(self, public_key, issuer: &Certificate, issuer_key)
        // KeyPair implements PublicKeyData, so we pass &kp as the public key.
        let cert = params
            .signed_by(&kp, &self.root_cert, &self.root_keypair)
            .map_err(|e| anyhow!("signing server cert: {e}"))?;

        Ok((
            IssuedCert {
                cert_pem: cert.pem(),
                serial_hex: serial_hex(cert.der()),
                cert_der: cert.der().to_vec(),
                not_after,
            },
            kp,
        ))
    }

    /// Sign a client leaf cert over an external public key (from enrollment CSR).
    /// `user_id` becomes the subject CN; `machine_id` becomes the OU.
    ///
    /// # API deviation from plan
    /// The plan used `params.signed_by_pubkey(public_key, &issuer)` which does
    /// not exist in rcgen 0.13.  Instead we use `CertificateParams::signed_by`
    /// with a `SubjectPublicKeyInfo` value, which implements `PublicKeyData` and
    /// is accepted by the same method.
    pub fn issue_client_cert(
        &self,
        public_key: &SubjectPublicKeyInfo,
        user_id: &UserId,
        machine_id: &MachineId,
    ) -> Result<IssuedCert> {
        let mut params = CertificateParams::new(Vec::<String>::new())
            .map_err(|e| anyhow!("building client cert params: {e}"))?;
        params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, user_id.to_string());
            dn.push(DnType::OrganizationalUnitName, machine_id.to_string());
            dn
        };
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::seconds(NOT_BEFORE_BACKDATE_SECS);
        let not_after = now + Duration::days(CLIENT_CERT_VALIDITY_DAYS);
        params.not_after = not_after;

        // SubjectPublicKeyInfo implements PublicKeyData — use signed_by directly.
        let cert = params
            .signed_by(public_key, &self.root_cert, &self.root_keypair)
            .map_err(|e| anyhow!("signing client cert: {e}"))?;

        Ok(IssuedCert {
            cert_pem: cert.pem(),
            serial_hex: serial_hex(cert.der()),
            cert_der: cert.der().to_vec(),
            not_after,
        })
    }
}

/// Enumerate IPs to put in the server cert SAN list. Pulls all non-loopback
/// addresses from local interfaces.
#[must_use]
pub fn local_san_ips() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for addr in addrs {
            if addr.is_loopback() {
                continue;
            }
            out.push(addr.ip().to_string());
        }
    }
    out
}

/// Build the full SAN list for the server cert: hostname, mDNS .local
/// name, all non-loopback IPs, plus any operator-supplied extras.
#[must_use]
pub fn build_server_sans(server_name: &str, extras: &[String]) -> Vec<String> {
    let mut sans: Vec<String> = Vec::new();
    if let Ok(h) = hostname::get() {
        if let Ok(s) = h.into_string() {
            sans.push(s.clone());
            sans.push(format!("{s}.local"));
        }
    }
    sans.push(format!("{}.local", sanitize_host_label(server_name)));
    sans.extend(local_san_ips());
    sans.extend(extras.iter().cloned());
    sans.sort();
    sans.dedup();
    sans
}

fn sanitize_host_label(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

fn serial_hex(der: &[u8]) -> String {
    use x509_parser::prelude::*;
    if let Ok((_, parsed)) = X509Certificate::from_der(der) {
        return hex::encode(parsed.serial.to_bytes_be());
    }
    "unknown".to_string()
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        if let Item::X509Certificate(der) = item {
            return Some(der.to_vec());
        }
    }
    None
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bootstrap_creates_ca_files() {
        let tmp = TempDir::new().unwrap();
        let ca = Ca::open(tmp.path()).unwrap();
        assert!(tmp.path().join("ca/ca.key").exists());
        assert!(tmp.path().join("ca/ca.crt").exists());
        assert!(!ca.root_cert_pem.is_empty());
    }

    #[test]
    fn second_open_reuses_existing_ca() {
        let tmp = TempDir::new().unwrap();
        let first = Ca::open(tmp.path()).unwrap();
        let second = Ca::open(tmp.path()).unwrap();
        assert_eq!(first.root_cert_pem, second.root_cert_pem);
    }

    #[test]
    fn build_server_sans_includes_local_label() {
        let sans = build_server_sans("shoebox-test", &["extra.example.com".to_string()]);
        assert!(sans.iter().any(|s| s == "shoebox-test.local"));
        assert!(sans.iter().any(|s| s == "extra.example.com"));
    }

    #[test]
    fn issued_server_cert_is_valid_pem() {
        let tmp = TempDir::new().unwrap();
        let ca = Ca::open(tmp.path()).unwrap();
        let (issued, _kp) = ca.issue_server_cert(&["127.0.0.1".to_string()]).unwrap();
        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(issued.serial_hex.len() % 2, 0);
    }
}
