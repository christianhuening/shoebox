# shoebox-server Auth & mTLS Implementation Plan (Plan 1.2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the auth layer to `shoebox-server`: an internal Ed25519 root CA generated on first launch, a shared catalog secret for enrollment bootstrap, an mTLS-protected HTTPS listener for all client traffic, `/enroll` and `/renew` endpoints that mint signed leaf certs, a CRL-aware handshake that honours `revoked_certs` rows, and a `revoke` CLI subcommand. The Plan 1.1 `/health` endpoint moves to a separate unauthenticated localhost listener so container healthchecks keep working.

**Architecture:** Two listeners. The "public" listener (default `0.0.0.0:9000`) terminates mTLS via `axum-server` + `rustls`; every connection requires a valid client certificate signed by the internal CA. A connection extractor surfaces the cert's CN as a `UserId` extension on the request, and an mTLS-time verifier rejects revoked serials. A second "health" listener (default `127.0.0.1:9001`) serves only `/health` over plain HTTP for healthchecks and is bound to loopback. All cert and secret material lives under `<data_dir>/ca/` and `<data_dir>/secret.json` respectively, mode 0600. The CLI grows a `clap`-based subcommand structure (`serve`, default; `revoke --serial <hex>`) without breaking the existing zero-arg invocation.

**Tech Stack:** Adds to Plan 1.1's stack: `rcgen` (cert generation, Ed25519), `rustls` + `tokio-rustls` (TLS), `axum-server` (axum-compatible TLS bind with rustls config), `argon2` (shared-secret hashing), `clap` with the `derive` feature (CLI), `x509-parser` (extract cert serial and subject CN from peer certs at handshake/middleware time), `time` (cert validity windows). No removals.

**Prerequisites for the implementing engineer:**
- Plan 1.1 complete (workspace builds, 12 tests pass, server runs).
- Familiarity with TLS basics (handshake, cert chain validation, client vs server certs). The mTLS plumbing is the hardest part of this plan — read the rustls and axum-server examples linked in Task 7 before writing code.

---

## File Structure

This plan adds the following files and modifies a few from Plan 1.1.

```
shoebox/
├── crates/
│   ├── shoebox-common/
│   │   └── src/
│   │       └── identity.rs                  ← NEW: UserId / MachineId types
│   └── shoebox-server/
│       ├── Cargo.toml                       ← add rcgen, rustls, tokio-rustls,
│       │                                       axum-server, argon2, clap,
│       │                                       x509-parser, time
│       ├── src/
│       │   ├── lib.rs                       ← expose new modules
│       │   ├── main.rs                      ← clap CLI, dual-listener startup
│       │   ├── config.rs                    ← add health_bind_addr, extra_sans
│       │   ├── ca.rs                        ← NEW: root CA + server cert
│       │   ├── secret.rs                    ← NEW: shared catalog secret
│       │   ├── enroll.rs                    ← NEW: /enroll + /renew handlers
│       │   ├── mtls.rs                      ← NEW: TLS config + cert verifier
│       │   ├── identity.rs                  ← NEW: extractor for UserId
│       │   ├── revoke.rs                    ← NEW: revoke subcommand
│       │   ├── http.rs                      ← split: public router vs health
│       │   ├── db.rs                        ← add CRL helpers (insert/list)
│       │   ├── logging.rs                   ← unchanged
│       │   └── mdns.rs                      ← unchanged
│       └── tests/
│           ├── health_e2e.rs                ← unchanged
│           ├── enroll_e2e.rs                ← NEW: full enrollment + mTLS test
│           └── revoke_e2e.rs                ← NEW: revocation invalidates cert
└── docs/
    └── superpowers/plans/
        └── 2026-05-17-sub-1-2-server-auth-mtls.md   ← this file
```

**Responsibility split:**
- `ca.rs` — root CA generation/persistence, server cert issuance with SANs, leaf cert signing. Pure cert lifecycle; no HTTP, no DB.
- `secret.rs` — generate / argon2-hash / verify the shared catalog secret. No HTTP, no certs.
- `enroll.rs` — the `/enroll` and `/renew` HTTP handlers. Glue between secret, ca, and db.
- `mtls.rs` — rustls `ServerConfig` builder that requires client certs signed by our CA AND checks the CRL on every handshake. No business logic.
- `identity.rs` (server crate) — `axum::extract::FromRequestParts` impl that pulls the peer cert chain off the connection extensions and yields a `UserId`.
- `identity.rs` (common crate) — the `UserId(String)` and `MachineId(String)` newtypes that both server and (future) client share.
- `revoke.rs` — implementation of the `revoke --serial <hex>` CLI subcommand.
- `http.rs` — split into `public_router(state)` (mTLS-required routes: `/enroll`, `/renew`, `/whoami`) and `health_router(state)` (just `/health`). The serve loops in `main.rs` bind each to its own listener.
- `main.rs` — clap parses the subcommand; `serve` (default) starts the dual listeners + background renewer; `revoke` calls `revoke::run(...)` and exits.
- `db.rs` — add `Db::insert_revoked_cert` and `Db::is_serial_revoked` helpers used by both the verifier and the revoke command.

---

## Task 1: Add crypto, TLS, CLI, and helper dependencies

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/shoebox-server/Cargo.toml` (add the new deps to `[dependencies]`)
- Modify: `crates/shoebox-common/Cargo.toml` (no change here; declared for clarity)

- [ ] **Step 1: Append to workspace `[workspace.dependencies]` in `Cargo.toml`.**

```toml
rcgen = { version = "0.13", default-features = false, features = ["crypto", "pem"] }
rustls = { version = "0.23", default-features = false, features = ["ring"] }
rustls-pemfile = "2"
tokio-rustls = { version = "0.26", default-features = false, features = ["ring"] }
axum-server = { version = "0.7", features = ["tls-rustls-no-provider"] }
argon2 = "0.5"
clap = { version = "4", features = ["derive"] }
x509-parser = "0.16"
time = { version = "0.3", features = ["std", "formatting", "parsing"] }
hex = "0.4"
```

Notes:
- `rustls` uses `ring` (default provider, pure-Rust enough). We pin `default-features = false` to avoid pulling in `aws-lc-rs` on platforms where it's the default.
- `axum-server` 0.7 with `tls-rustls-no-provider` means we install the rustls crypto provider explicitly at startup; this matches `rustls 0.23`'s newer API.

- [ ] **Step 2: Append to `crates/shoebox-server/Cargo.toml` `[dependencies]`.**

```toml
rcgen = { workspace = true }
rustls = { workspace = true }
rustls-pemfile = { workspace = true }
tokio-rustls = { workspace = true }
axum-server = { workspace = true }
argon2 = { workspace = true }
clap = { workspace = true }
x509-parser = { workspace = true }
time = { workspace = true }
hex = { workspace = true }
```

- [ ] **Step 3: Verify the workspace builds.**

Run: `cargo build -p shoebox-server`
Expected: clean build with the new deps fetched and compiled. May take 2-3 minutes the first time.

- [ ] **Step 4: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml
git commit -m "build: add rcgen/rustls/argon2/clap/x509-parser deps for auth+mTLS"
```

---

## Task 2: Add `UserId` and `MachineId` types to `shoebox-common`

**Files:**
- Create: `crates/shoebox-common/src/identity.rs`
- Modify: `crates/shoebox-common/src/lib.rs` (export the new module)
- Modify: `crates/shoebox-common/Cargo.toml` (`hex` workspace dep for parsing)

- [ ] **Step 1: Add `hex` to `crates/shoebox-common/Cargo.toml` `[dependencies]`.**

```toml
hex = { workspace = true }
```

- [ ] **Step 2: Write `crates/shoebox-common/src/identity.rs`.**

```rust
//! Stable identity newtypes used across server and client.
//!
//! `UserId`: 16-byte UUID rendered as 32-char lowercase hex.
//! `MachineId`: 16-byte UUID rendered as 32-char lowercase hex.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

impl UserId {
    /// Generate a new random UserId (32-char lowercase hex of 16 random bytes).
    #[must_use]
    pub fn new_random() -> Self {
        let mut bytes = [0u8; 16];
        getrandom_bytes(&mut bytes);
        Self(hex::encode(bytes))
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for UserId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            Ok(Self(s.to_string()))
        } else {
            Err(format!("invalid UserId: {s:?} (expected 32 lowercase hex chars)"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineId(pub String);

impl MachineId {
    #[must_use]
    pub fn new_random() -> Self {
        let mut bytes = [0u8; 16];
        getrandom_bytes(&mut bytes);
        Self(hex::encode(bytes))
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for MachineId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            Ok(Self(s.to_string()))
        } else {
            Err(format!("invalid MachineId: {s:?} (expected 32 lowercase hex chars)"))
        }
    }
}

fn getrandom_bytes(buf: &mut [u8]) {
    // ring is a transitive dep via rustls; pull randomness from it without
    // adding a new direct dep. If this trait isn't visible from common,
    // swap to the `rand` crate.
    use std::time::{SystemTime, UNIX_EPOCH};
    // Mix system-time entropy with a per-process counter to avoid needing
    // ring/rand here. For real randomness in production, the server is
    // expected to use the OS RNG via `rand` or `getrandom`. This module
    // intentionally uses a minimal seed because IDs are server-generated
    // via the user-creation flow which uses `rand::random`.
    let _ = (buf, SystemTime::now().duration_since(UNIX_EPOCH));
    unreachable!(
        "UserId::new_random/MachineId::new_random must be replaced with rand-backed impl; \
         common-crate placeholder is intentional to avoid the rand dep here"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_parses_lowercase_hex() {
        let s = "0123456789abcdef0123456789abcdef";
        let u: UserId = s.parse().unwrap();
        assert_eq!(u.to_string(), s);
    }

    #[test]
    fn user_id_rejects_uppercase() {
        let s = "0123456789ABCDEF0123456789ABCDEF";
        assert!(s.parse::<UserId>().is_err());
    }

    #[test]
    fn user_id_rejects_wrong_length() {
        assert!("abc".parse::<UserId>().is_err());
        assert!("0123456789abcdef".parse::<UserId>().is_err());
    }

    #[test]
    fn round_trip_via_display() {
        let s = "deadbeefcafebabe0000000011112222";
        let u: UserId = s.parse().unwrap();
        let back: UserId = u.to_string().parse().unwrap();
        assert_eq!(u, back);
    }
}
```

NOTE on `getrandom_bytes`: the placeholder above panics if anyone actually calls `new_random()` from the common crate, because we don't want to add a `rand`/`getrandom` dep here. The server crate will provide its own random ID generation in Task 3 via the `rand` ecosystem already pulled in transitively. Tests in this module only exercise parsing/display, not generation, so the panic is unreachable in the test suite.

If the implementer prefers, they can add `rand = "0.8"` to `shoebox-common` and replace the placeholder with `rand::thread_rng().fill_bytes(buf)`. Either way is fine; what matters is that the panic doesn't fire in tests.

- [ ] **Step 3: Update `crates/shoebox-common/src/lib.rs`.**

Replace the file with:

```rust
//! Shared types and utilities for shoebox.

pub mod error;
pub mod identity;

pub use error::{Error, Result};
pub use identity::{MachineId, UserId};

/// Schema version this build of shoebox understands.
/// Update this when the migration set changes.
pub const SCHEMA_VERSION: i64 = 6;
```

- [ ] **Step 4: Run tests.**

Run: `cargo test -p shoebox-common`
Expected: 4 tests pass (the new `identity::tests` module).

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-common/Cargo.toml crates/shoebox-common/src/identity.rs \
        crates/shoebox-common/src/lib.rs
git commit -m "feat(common): add UserId and MachineId 32-char hex identity types"
```

---

## Task 3: Implement the internal Ed25519 root CA

**Files:**
- Create: `crates/shoebox-server/src/ca.rs`
- Modify: `crates/shoebox-server/src/lib.rs` (export the module)
- Modify: `crates/shoebox-server/src/main.rs` (call `Ca::open` at startup)

- [ ] **Step 1: Write `crates/shoebox-server/src/ca.rs`.**

```rust
//! Internal Certificate Authority for shoebox-server.
//!
//! On first launch, generates an Ed25519 root keypair and self-signed
//! root cert. Stores the key (mode 0600) and cert PEM in the data dir
//! under `ca/`. On subsequent launches, loads them.
//!
//! Issues:
//! - server certs signed by the root, SANs from network interfaces + extras
//! - client leaf certs signed by the root, subject CN = user_id, OU = machine_id

use anyhow::{anyhow, Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
    KeyUsagePurpose, SanType, ExtendedKeyUsagePurpose,
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

pub struct Ca {
    pub root_keypair: KeyPair,
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
        fs::create_dir_all(&ca_dir).with_context(|| format!("creating {ca_dir:?}"))?;

        let key_path = ca_dir.join("ca.key");
        let cert_path = ca_dir.join("ca.crt");

        let (root_keypair, root_cert_der, root_cert_pem) =
            if key_path.exists() && cert_path.exists() {
                tracing::info!(event = "ca.load", "loading existing root CA");
                let key_pem = fs::read_to_string(&key_path)
                    .with_context(|| format!("reading {key_path:?}"))?;
                let cert_pem = fs::read_to_string(&cert_path)
                    .with_context(|| format!("reading {cert_path:?}"))?;
                let kp = KeyPair::from_pem(&key_pem)
                    .map_err(|e| anyhow!("parsing CA key PEM: {e}"))?;
                let der = pem_to_der(&cert_pem)
                    .ok_or_else(|| anyhow!("CA cert PEM has no CERTIFICATE block"))?;
                (kp, der, cert_pem)
            } else {
                tracing::info!(event = "ca.bootstrap", "generating new root CA");
                let kp = KeyPair::generate_for(&rcgen::PKCS_ED25519)
                    .map_err(|e| anyhow!("generating CA keypair: {e}"))?;

                let mut params = CertificateParams::new(Vec::<String>::new())
                    .map_err(|e| anyhow!("building CA params: {e}"))?;
                params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
                params.key_usages = vec![
                    KeyUsagePurpose::KeyCertSign,
                    KeyUsagePurpose::CrlSign,
                ];
                params.distinguished_name = {
                    let mut dn = DistinguishedName::new();
                    dn.push(DnType::CommonName, "shoebox-server internal CA");
                    dn
                };
                let now = OffsetDateTime::now_utc();
                params.not_before = now - Duration::seconds(NOT_BEFORE_BACKDATE_SECS);
                params.not_after = now + Duration::days(ROOT_CA_VALIDITY_DAYS);

                let cert = params
                    .self_signed(&kp)
                    .map_err(|e| anyhow!("self-signing CA cert: {e}"))?;

                let cert_pem = cert.pem();
                let cert_der = cert.der().to_vec();
                let key_pem = kp.serialize_pem();

                fs::write(&cert_path, &cert_pem)
                    .with_context(|| format!("writing {cert_path:?}"))?;
                fs::write(&key_path, &key_pem)
                    .with_context(|| format!("writing {key_path:?}"))?;
                set_owner_only(&key_path)?;

                (kp, cert_der, cert_pem)
            };

        Ok(Self {
            root_keypair,
            root_cert_der,
            root_cert_pem,
            data_dir: data_dir.to_path_buf(),
        })
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
        params.not_after = now + Duration::days(SERVER_CERT_VALIDITY_DAYS);

        let cert = params
            .signed_by(&kp, &self.root_keypair_as_issuer()?)
            .map_err(|e| anyhow!("signing server cert: {e}"))?;

        Ok((
            IssuedCert {
                cert_pem: cert.pem(),
                cert_der: cert.der().to_vec(),
                serial_hex: serial_hex(cert.der()),
                not_after: params.not_after,
            },
            kp,
        ))
    }

    /// Sign an arbitrary CSR-derived `CertificateParams` as a client leaf.
    /// `user_id` becomes the subject CN; `machine_id` becomes the OU.
    pub fn issue_client_cert(
        &self,
        public_key: &rcgen::SubjectPublicKeyInfo,
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
        params.not_after = now + Duration::days(CLIENT_CERT_VALIDITY_DAYS);

        let cert = params
            .signed_by_pubkey(public_key, &self.root_keypair_as_issuer()?)
            .map_err(|e| anyhow!("signing client cert: {e}"))?;

        Ok(IssuedCert {
            cert_pem: cert.pem(),
            cert_der: cert.der().to_vec(),
            serial_hex: serial_hex(cert.der()),
            not_after: params.not_after,
        })
    }

    fn root_keypair_as_issuer(&self) -> Result<rcgen::Issuer<'_, KeyPair>> {
        // rcgen 0.13: build an Issuer view over our stored key + cert PEM
        // so signed_by()/signed_by_pubkey() can produce the chain.
        let cert_params = CertificateParams::from_ca_cert_pem(&self.root_cert_pem)
            .map_err(|e| anyhow!("re-parsing CA cert as issuer: {e}"))?;
        Ok(rcgen::Issuer::new(cert_params, &self.root_keypair))
    }
}

/// Enumerate IPs to put in the server cert SAN list. Pulls all non-loopback
/// addresses from local interfaces.
pub fn local_san_ips() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for a in addrs {
            if a.is_loopback() {
                continue;
            }
            out.push(a.ip().to_string());
        }
    }
    out
}

/// Build the full SAN list for the server cert: hostname, mDNS .local
/// name, all non-loopback IPs, plus any operator-supplied extras.
pub fn build_server_sans(server_name: &str, extras: &[String]) -> Vec<String> {
    let mut sans: Vec<String> = Vec::new();
    if let Ok(h) = hostname::get() {
        if let Ok(s) = h.into_string() {
            sans.push(s.clone());
            sans.push(format!("{}.local", s));
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
        let (issued, _kp) = ca
            .issue_server_cert(&["127.0.0.1".to_string()])
            .unwrap();
        assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(issued.serial_hex.len() % 2, 0);
    }
}
```

The `if_addrs` crate is new. Add it to workspace and crate deps:

In `Cargo.toml` `[workspace.dependencies]`:
```toml
if-addrs = "0.13"
```

In `crates/shoebox-server/Cargo.toml` `[dependencies]`:
```toml
if-addrs = { workspace = true }
```

- [ ] **Step 2: Expose `ca` module via `crates/shoebox-server/src/lib.rs`.** Add `pub mod ca;` line.

- [ ] **Step 3: Run tests.**

Run: `cargo test -p shoebox-server ca`
Expected: 4 tests pass (`bootstrap_creates_ca_files`, `second_open_reuses_existing_ca`, `build_server_sans_includes_local_label`, `issued_server_cert_is_valid_pem`).

- [ ] **Step 4: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml \
        crates/shoebox-server/src/ca.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): internal Ed25519 root CA, server cert issuance, SAN enumeration"
```

---

## Task 4: Shared catalog secret (generate, hash, verify)

**Files:**
- Create: `crates/shoebox-server/src/secret.rs`
- Modify: `crates/shoebox-server/src/lib.rs` (export module)

- [ ] **Step 1: Write `crates/shoebox-server/src/secret.rs`.**

```rust
//! Shared catalog secret used during enrollment.
//!
//! On first launch, a random 24-byte secret is generated, argon2id-hashed,
//! and the hash is persisted to the catalog `config` table under the key
//! `enrollment_secret_hash`. The plaintext is printed once to the log and
//! never stored on disk. Operators can override via the `SHOEBOX_SECRET`
//! env var at startup.

use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use libsql::Connection;

const CONFIG_KEY: &str = "enrollment_secret_hash";

/// Verify a presented plaintext against the stored argon2id hash.
pub async fn verify(conn: &Connection, presented: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT value FROM config WHERE key = ?1",
            [CONFIG_KEY],
        )
        .await
        .context("reading enrollment_secret_hash")?;
    let row = match rows.next().await? {
        Some(r) => r,
        None => return Ok(false),
    };
    let hash_str: String = row.get(0)?;
    let parsed = PasswordHash::new(&hash_str)
        .map_err(|e| anyhow!("malformed stored secret hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(presented.as_bytes(), &parsed)
        .is_ok())
}

/// Ensure a secret hash is present in the catalog. If one isn't, either
/// use the `SHOEBOX_SECRET` env var (if set), or generate a random secret.
/// The plaintext (whether supplied or generated) is returned so the caller
/// can log it exactly once at bootstrap.
pub async fn ensure_present(conn: &Connection) -> Result<EnsureOutcome> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM config WHERE key = ?1",
            [CONFIG_KEY],
        )
        .await?;
    if rows.next().await?.is_some() {
        return Ok(EnsureOutcome::AlreadySet);
    }

    let plaintext = match std::env::var("SHOEBOX_SECRET") {
        Ok(v) if !v.is_empty() => v,
        _ => generate_random_secret(),
    };

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?
        .to_string();

    conn.execute(
        "INSERT INTO config (key, value) VALUES (?1, ?2)",
        (CONFIG_KEY, hash),
    )
    .await
    .context("inserting enrollment_secret_hash")?;

    Ok(EnsureOutcome::Generated { plaintext })
}

fn generate_random_secret() -> String {
    use rand::{distributions::Alphanumeric, Rng};
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}

pub enum EnsureOutcome {
    AlreadySet,
    Generated { plaintext: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use tempfile::TempDir;

    #[tokio::test]
    async fn first_call_generates_and_persists() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        let conn = db.connect().unwrap();

        match ensure_present(&conn).await.unwrap() {
            EnsureOutcome::Generated { plaintext } => assert_eq!(plaintext.len(), 24),
            EnsureOutcome::AlreadySet => panic!("should generate on fresh DB"),
        }

        match ensure_present(&conn).await.unwrap() {
            EnsureOutcome::AlreadySet => {}
            EnsureOutcome::Generated { .. } => panic!("should be idempotent"),
        }
    }

    #[tokio::test]
    async fn verify_accepts_correct_secret() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        let conn = db.connect().unwrap();
        let plaintext = match ensure_present(&conn).await.unwrap() {
            EnsureOutcome::Generated { plaintext } => plaintext,
            EnsureOutcome::AlreadySet => panic!(),
        };

        assert!(verify(&conn, &plaintext).await.unwrap());
        assert!(!verify(&conn, "wrong-secret").await.unwrap());
    }
}
```

The `rand` crate is new. Add it:

In `Cargo.toml` `[workspace.dependencies]`:
```toml
rand = "0.8"
```

In `crates/shoebox-server/Cargo.toml` `[dependencies]`:
```toml
rand = { workspace = true }
```

- [ ] **Step 2: Expose `secret` module in `crates/shoebox-server/src/lib.rs`.** Add `pub mod secret;`.

- [ ] **Step 3: Run tests.**

Run: `cargo test -p shoebox-server secret`
Expected: 2 tests pass.

- [ ] **Step 4: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml \
        crates/shoebox-server/src/secret.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): shared catalog secret with argon2id hash + verify"
```

---

## Task 5: Add `health_bind_addr` and `extra_sans` to `Config`

**Files:**
- Modify: `crates/shoebox-server/src/config.rs`

- [ ] **Step 1: Add two fields to `Config` and update the env-defaults builder.**

Replace the `Config` struct and `from_env_with_defaults` impl in `crates/shoebox-server/src/config.rs` with:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server_name: String,
    pub bind_addr: SocketAddr,
    #[serde(default = "default_health_bind_addr")]
    pub health_bind_addr: SocketAddr,
    pub data_dir: PathBuf,
    pub photos_dir: PathBuf,
    pub cache_dir: PathBuf,
    #[serde(default)]
    pub extra_sans: Vec<String>,
}

fn default_health_bind_addr() -> SocketAddr {
    "127.0.0.1:9001".parse().expect("valid default")
}
```

And in `from_env_with_defaults`, add the two new fields just before the closing `}`:

```rust
            health_bind_addr: std::env::var("SHOEBOX_HEALTH_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:9001".into())
                .parse()
                .expect("SHOEBOX_HEALTH_BIND_ADDR must parse as SocketAddr"),
            extra_sans: std::env::var("SHOEBOX_EXTRA_SANS")
                .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default(),
```

And in `apply_env_overrides`, add at the end (before `self`):

```rust
        if let Ok(v) = std::env::var("SHOEBOX_HEALTH_BIND_ADDR") {
            match v.parse() {
                Ok(addr) => self.health_bind_addr = addr,
                Err(e) => tracing::warn!(
                    event = "config.health_bind_addr.invalid",
                    value = %v, error = %e,
                    "SHOEBOX_HEALTH_BIND_ADDR could not be parsed; keeping value from config"
                ),
            }
        }
        if let Ok(v) = std::env::var("SHOEBOX_EXTRA_SANS") {
            self.extra_sans = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
```

- [ ] **Step 2: Update `server.toml.example` to show the new optional fields.**

Append to `/home/chris/shoebox/server.toml.example`:

```toml

# Optional: bind address for the unauthenticated /health endpoint.
# Defaults to 127.0.0.1:9001 (loopback only). Used by container
# healthchecks and Kubernetes readiness probes.
# health_bind_addr = "127.0.0.1:9001"

# Optional: extra subjectAltName entries to include in the server cert
# (e.g. external DNS names, reverse-proxy hostnames). Defaults to none.
# extra_sans = ["shoebox.example.com", "10.0.5.42"]
```

- [ ] **Step 3: Extend the existing config test to cover the new defaults.**

Replace the `parses_minimal_toml` test in `crates/shoebox-server/src/config.rs` (inside `mod tests`) with:

```rust
    #[test]
    fn parses_minimal_toml_with_defaults() {
        let s = r#"
            server_name = "shoebox-test"
            bind_addr = "127.0.0.1:9000"
            data_dir = "/var/lib/shoebox"
            photos_dir = "/photos"
            cache_dir = "/shoebox-cache"
        "#;
        let cfg = Config::from_toml_str(s).unwrap();
        assert_eq!(cfg.server_name, "shoebox-test");
        assert_eq!(cfg.bind_addr.port(), 9000);
        assert_eq!(cfg.health_bind_addr.port(), 9001);
        assert!(cfg.extra_sans.is_empty());
    }

    #[test]
    fn parses_toml_with_extra_sans() {
        let s = r#"
            server_name = "x"
            bind_addr = "127.0.0.1:9000"
            data_dir = "/d"
            photos_dir = "/p"
            cache_dir = "/c"
            extra_sans = ["a.example.com", "b.example.com"]
        "#;
        let cfg = Config::from_toml_str(s).unwrap();
        assert_eq!(cfg.extra_sans, vec!["a.example.com", "b.example.com"]);
    }
```

- [ ] **Step 4: Run tests.**

Run: `cargo test -p shoebox-server config`
Expected: 3 tests pass (the renamed `parses_minimal_toml_with_defaults`, the new `parses_toml_with_extra_sans`, and the existing `env_overrides_take_precedence`).

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/config.rs server.toml.example
git commit -m "feat(server): Config gains health_bind_addr and extra_sans"
```

---

## Task 6: Add CRL helpers to `db.rs`

**Files:**
- Modify: `crates/shoebox-server/src/db.rs`

- [ ] **Step 1: Append to the `impl Db` block in `crates/shoebox-server/src/db.rs` (after `connect`):**

```rust
    /// Insert a row into revoked_certs. `serial_hex` is the lowercase-hex
    /// serial number of the leaf cert being revoked.
    pub async fn insert_revoked_cert(
        &self,
        serial_hex: &str,
        reason: Option<&str>,
        revoked_by: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        let now_ms = now_ms();
        conn.execute(
            "INSERT INTO revoked_certs (serial_number, revoked_at, reason, revoked_by) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(serial_number) DO NOTHING",
            (
                serial_hex.to_string(),
                now_ms,
                reason.map(str::to_string),
                revoked_by.map(str::to_string),
            ),
        )
        .await?;
        Ok(())
    }

    /// Return true if the given hex serial appears in `revoked_certs`.
    pub async fn is_serial_revoked(&self, serial_hex: &str) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT 1 FROM revoked_certs WHERE serial_number = ?1",
                [serial_hex],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }
```

(The `now_ms` helper already exists at the bottom of the file from Plan 1.1.)

- [ ] **Step 2: Add a test in the `mod tests` block.**

```rust
    #[tokio::test]
    async fn revoked_serial_round_trips() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        assert!(!db.is_serial_revoked("abc123").await.unwrap());
        db.insert_revoked_cert("abc123", Some("test"), None).await.unwrap();
        assert!(db.is_serial_revoked("abc123").await.unwrap());
        // Idempotent: inserting again does not error.
        db.insert_revoked_cert("abc123", Some("test"), None).await.unwrap();
    }
```

- [ ] **Step 3: Run tests.**

Run: `cargo test -p shoebox-server db::tests::revoked_serial_round_trips`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/shoebox-server/src/db.rs
git commit -m "feat(db): add insert_revoked_cert and is_serial_revoked helpers"
```

---

## Task 7: TLS server with no client-cert requirement (yet)

**Files:**
- Create: `crates/shoebox-server/src/mtls.rs`
- Modify: `crates/shoebox-server/src/http.rs` (split into public/health routers)
- Modify: `crates/shoebox-server/src/lib.rs`
- Modify: `crates/shoebox-server/src/main.rs` (use TLS for public port + plain HTTP for health port)

This task replaces the plain HTTP bind on port 9000 with a TLS bind, and adds the separate plain HTTP listener on `127.0.0.1:9001` for `/health`. mTLS (client cert requirement) lands in Task 9.

- [ ] **Step 1: Write `crates/shoebox-server/src/mtls.rs`.**

```rust
//! TLS server configuration and (in Task 9) client-cert verifier.

use anyhow::{anyhow, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::sync::Arc;

use crate::ca::IssuedCert;

/// Install the default rustls crypto provider exactly once at startup.
pub fn install_crypto_provider() {
    use rustls::crypto::ring::default_provider;
    let _ = default_provider().install_default();
}

/// Build a server TLS config from an issued server cert + its keypair.
/// Does NOT yet require client certs (that's Task 9).
pub fn server_only_tls_config(
    server_cert: &IssuedCert,
    server_keypair: &rcgen::KeyPair,
) -> Result<Arc<ServerConfig>> {
    let cert_der = CertificateDer::from(server_cert.cert_der.clone());
    let key_pem = server_keypair.serialize_pem();
    let key_der = parse_first_private_key(&key_pem)?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|e| anyhow!("building rustls ServerConfig: {e}"))?;
    Ok(Arc::new(config))
}

fn parse_first_private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Ok(PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Ok(PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Ok(PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    Err(anyhow!("no private key found in PEM"))
}
```

- [ ] **Step 2: Replace `crates/shoebox-server/src/http.rs` with the split-router version.**

```rust
//! HTTP routers. `public_router` carries auth-required endpoints and is
//! served over mTLS; `health_router` carries only /health and is served
//! over plain HTTP on a loopback-only port.

use axum::{extract::State, http::StatusCode, response::Json, routing::get, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub schema_version: i64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub schema_version: i64,
}

/// Endpoints that require mTLS (gated by the TLS layer, not the router).
/// In Task 8 this gains /enroll; in Task 11 it gains /whoami.
pub fn public_router(state: AppState) -> Router {
    Router::new().with_state(state)
}

/// Plain-HTTP /health endpoint for container/k8s healthchecks. Bound to
/// loopback only; never exposed off-host.
pub fn health_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            schema_version: state.schema_version,
        }),
    )
}
```

- [ ] **Step 3: Add `pub mod mtls;` to `crates/shoebox-server/src/lib.rs`.**

- [ ] **Step 4: Update `crates/shoebox-server/src/main.rs` to bind two listeners.**

Replace the contents of `main.rs` with:

```rust
use shoebox_server::{ca, config, db, http, logging, mdns, mtls, secret};
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
    let ca = ca::Ca::open(&cfg.data_dir)?;
    let sans = ca::build_server_sans(&cfg.server_name, &cfg.extra_sans);
    let (server_cert, server_kp) = ca.issue_server_cert(&sans)?;
    let tls_cfg = mtls::server_only_tls_config(&server_cert, &server_kp)?;

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
    };

    let broadcaster = mdns::MdnsBroadcaster::start(
        &cfg.server_name,
        cfg.bind_addr.port(),
        shoebox_common::SCHEMA_VERSION,
        mdns::local_ips(),
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
    use axum_server::tls_rustls::RustlsConfig;
    let rustls_cfg = RustlsConfig::from_config(tls_cfg);
    tracing::info!(event = "https.listen.public", addr = %addr, "public TLS server bound");
    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = shutdown.await;
        handle_for_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });
    axum_server::bind_rustls(addr, rustls_cfg)
        .handle(handle)
        .serve(http::public_router(state).into_make_service())
        .await?;
    Ok(())
}
```

- [ ] **Step 5: Update `crates/shoebox-server/tests/health_e2e.rs`** — it currently calls `http::router(state)`, which is gone. The integration test now exercises the `health_router`:

Replace the test file contents with:

```rust
//! End-to-end test for the /health listener (plain HTTP on loopback).

use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[tokio::test]
async fn full_server_serves_health() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(
        shoebox_server::db::Db::open(&tmp.path().join("catalog.db"))
            .await
            .unwrap(),
    );

    let state = shoebox_server::http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();

    let app = shoebox_server::http::health_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    let resp = reqwest::get(format!("http://{addr}/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["schema_version"], 6);

    let _ = tx.send(());
    server.await.unwrap();
}
```

- [ ] **Step 6: Run tests + smoke-test the new dual-listener startup.**

```
cargo test -p shoebox-server
```

Expected: all tests pass.

Smoke test:
```bash
rm -rf ./data
cargo run -p shoebox-server &
SERVER_PID=$!
sleep 3
echo "=== /health (plain HTTP on :9001) ==="
curl -sf http://127.0.0.1:9001/health
echo
echo "=== /health on TLS port should fail (cert is self-signed) ==="
curl -sf https://127.0.0.1:9000/ -k -o /dev/null -w "%{http_code}\n" || true
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null || true
```

Expected: `/health` on :9001 returns JSON. TLS port responds (likely 404 since no routes registered yet, but the TLS handshake should succeed with `-k`).

- [ ] **Step 7: Commit.**

```bash
git add crates/shoebox-server/src/mtls.rs crates/shoebox-server/src/http.rs \
        crates/shoebox-server/src/main.rs crates/shoebox-server/src/lib.rs \
        crates/shoebox-server/tests/health_e2e.rs
git commit -m "feat(server): TLS public listener + plain-HTTP loopback /health listener"
```

---

## Task 8: `/enroll` endpoint

**Files:**
- Create: `crates/shoebox-server/src/enroll.rs`
- Modify: `crates/shoebox-server/src/http.rs` (wire route + extend AppState)
- Modify: `crates/shoebox-server/src/lib.rs` (export module)
- Modify: `crates/shoebox-server/src/main.rs` (populate new AppState fields)

The /enroll endpoint accepts a JSON body containing the shared secret, a CSR (PEM-encoded), a chosen display name, and an optional pre-known user_id (for re-enrolling an existing user from a new machine). It validates the secret, creates a user row if needed, signs the CSR, and returns the issued cert + the CA root cert.

- [ ] **Step 1: Extend `AppState` in `crates/shoebox-server/src/http.rs`** with the new state needed by /enroll. Replace the struct:

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub schema_version: i64,
    pub ca: Arc<crate::ca::Ca>,
}
```

- [ ] **Step 2: Write `crates/shoebox-server/src/enroll.rs`.**

```rust
//! /enroll handler: validate shared secret, create user if needed, sign
//! the presented CSR, return cert chain.

use anyhow::{anyhow, Context, Result};
use axum::{extract::State, http::StatusCode, response::Json, Router, routing::post};
use serde::{Deserialize, Serialize};
use shoebox_common::{MachineId, UserId};

use crate::ca::IssuedCert;
use crate::http::AppState;
use crate::secret;

#[derive(Debug, Deserialize)]
pub struct EnrollRequest {
    pub shared_secret: String,
    pub csr_pem: String,
    pub display_name: String,
    /// If set, re-enroll an existing user from a new machine. If absent,
    /// a new user row is created.
    pub existing_user_id: Option<UserId>,
    /// Stable identifier for the client install; if absent, a new one
    /// is generated.
    pub machine_id: Option<MachineId>,
}

#[derive(Debug, Serialize)]
pub struct EnrollResponse {
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub user_id: UserId,
    pub machine_id: MachineId,
    pub cert_serial_hex: String,
    pub not_after_unix: i64,
}

pub fn route() -> Router<AppState> {
    Router::new().route("/enroll", post(enroll_handler))
}

async fn enroll_handler(
    State(state): State<AppState>,
    Json(req): Json<EnrollRequest>,
) -> Result<(StatusCode, Json<EnrollResponse>), (StatusCode, String)> {
    let conn = state
        .db
        .connect()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;

    let ok = secret::verify(&conn, &req.shared_secret)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("verify: {e}")))?;
    if !ok {
        return Err((StatusCode::UNAUTHORIZED, "invalid shared secret".to_string()));
    }

    // Resolve user_id: either re-use existing (verify it exists) or create.
    let user_id = match &req.existing_user_id {
        Some(uid) => {
            let mut rows = conn
                .query("SELECT 1 FROM users WHERE id = ?1", [uid.to_string()])
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
            if rows
                .next()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?
                .is_none()
            {
                return Err((StatusCode::NOT_FOUND, format!("user {uid} not found")));
            }
            uid.clone()
        }
        None => {
            let new_uid = UserId::new_random();
            conn.execute(
                "INSERT INTO users (id, display_name, created_at, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?3)",
                (
                    new_uid.to_string(),
                    req.display_name.clone(),
                    now_ms(),
                ),
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
            new_uid
        }
    };

    let machine_id = req.machine_id.unwrap_or_else(MachineId::new_random);

    let issued = sign_csr(&state.ca, &req.csr_pem, &user_id, &machine_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("csr: {e}")))?;

    tracing::info!(
        event = "enrollment.completed",
        user_id = %user_id,
        machine_id = %machine_id,
        serial = %issued.serial_hex,
        "client enrolled"
    );

    Ok((
        StatusCode::OK,
        Json(EnrollResponse {
            client_cert_pem: issued.cert_pem,
            ca_cert_pem: state.ca.root_cert_pem.clone(),
            user_id,
            machine_id,
            cert_serial_hex: issued.serial_hex,
            not_after_unix: issued.not_after.unix_timestamp(),
        }),
    ))
}

fn sign_csr(
    ca: &crate::ca::Ca,
    csr_pem: &str,
    user_id: &UserId,
    machine_id: &MachineId,
) -> Result<IssuedCert> {
    // Parse the CSR to extract the public key.
    let csr = rcgen::CertificateSigningRequestParams::from_pem(csr_pem)
        .context("parsing CSR PEM")?;
    let pubkey = csr.public_key;
    ca.issue_client_cert(&pubkey, user_id, machine_id)
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
```

NOTE on `UserId::new_random` / `MachineId::new_random`: the placeholders in `shoebox-common` panic (per Task 2's note). The implementer should either:
(a) Add `rand` to `shoebox-common` and replace the panic with `rand::thread_rng().fill_bytes(buf)`, or
(b) Do random generation inline here in `enroll.rs` using `rand::thread_rng()` and construct `UserId(hex::encode(bytes))` directly, bypassing `new_random`.

Option (b) is preferred to keep `shoebox-common` dep-light. Replace the `UserId::new_random()` calls in this file with a helper:

```rust
fn random_user_id() -> UserId {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    UserId(hex::encode(bytes))
}

fn random_machine_id() -> MachineId {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    MachineId(hex::encode(bytes))
}
```

And replace `UserId::new_random()` → `random_user_id()`, `MachineId::new_random()` → `random_machine_id()` in `enroll_handler`.

- [ ] **Step 3: Wire the route into `public_router`** in `crates/shoebox-server/src/http.rs`:

```rust
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::enroll::route())
        .with_state(state)
}
```

- [ ] **Step 4: Expose `enroll` module** in `crates/shoebox-server/src/lib.rs`: add `pub mod enroll;`.

- [ ] **Step 5: Populate the new `AppState.ca` field in `main.rs`.** In the `AppState` construction block, change:

```rust
    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
    };
```

to:

```rust
    let state = http::AppState {
        db,
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: Arc::new(ca),
    };
```

Move `let ca = ca::Ca::open(...)` to come before the `state` construction (it already does — verify).

- [ ] **Step 6: Build and run tests.**

Run: `cargo test -p shoebox-server`
Expected: all tests pass. Note: there's no unit test for `enroll_handler` yet — the full /enroll flow is covered by the e2e test in Task 14.

- [ ] **Step 7: Commit.**

```bash
git add crates/shoebox-server/src/enroll.rs crates/shoebox-server/src/http.rs \
        crates/shoebox-server/src/lib.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): /enroll endpoint validates shared secret and signs client CSR"
```

---

## Task 9: Switch the public listener to require client certs (mTLS)

**Files:**
- Modify: `crates/shoebox-server/src/mtls.rs` (add mTLS server config)
- Modify: `crates/shoebox-server/src/main.rs` (use mTLS config + carve out /enroll exception)

This task makes the TLS layer require a client cert. `/enroll` is a paradox: it's the endpoint that gets you a client cert, so it can't itself require one. Solution: `/enroll` is served via a third listener (default `0.0.0.0:9000` but with TLS-only, no client cert required), and the "main" mTLS listener serves everything else.

Wait — that means three listeners. Let me reconsider. Simpler: serve everything on the same TLS port, but configure rustls to *request but not require* client certs, and gate per-route in middleware. That's the standard pattern.

- [ ] **Step 1: Replace `crates/shoebox-server/src/mtls.rs` to support the request-but-not-require pattern.**

```rust
//! TLS server configuration and client-cert verifier.

use anyhow::{anyhow, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{
    ClientCertVerified, ClientCertVerifier, WebPkiClientVerifier,
};
use rustls::{DigitallySignedStruct, RootCertStore, ServerConfig};
use std::sync::Arc;

use crate::ca::Ca;
use crate::ca::IssuedCert;

/// Install the default rustls crypto provider exactly once at startup.
pub fn install_crypto_provider() {
    use rustls::crypto::ring::default_provider;
    let _ = default_provider().install_default();
}

/// Build a server TLS config that:
///   - serves our server cert
///   - REQUESTS (but does not require) a client cert
///   - if a client cert is presented, it must chain to our CA root
///
/// Per-route "require auth" is enforced separately in middleware
/// (Task 10) by checking whether the peer cert extension was populated.
pub fn mtls_server_config(
    server_cert: &IssuedCert,
    server_keypair: &rcgen::KeyPair,
    ca: &Ca,
) -> Result<Arc<ServerConfig>> {
    let cert_der = CertificateDer::from(server_cert.cert_der.clone());
    let key_pem = server_keypair.serialize_pem();
    let key_der = parse_first_private_key(&key_pem)?;

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .map_err(|e| anyhow!("loading CA root into trust store: {e}"))?;
    let roots = Arc::new(roots);

    // WebPkiClientVerifier in `optional` mode: request but don't require.
    let verifier = WebPkiClientVerifier::builder(roots)
        .allow_unauthenticated()
        .build()
        .map_err(|e| anyhow!("building client verifier: {e}"))?;

    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|e| anyhow!("building rustls ServerConfig: {e}"))?;
    Ok(Arc::new(config))
}

fn parse_first_private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Ok(PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Ok(PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Ok(PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    Err(anyhow!("no private key found in PEM"))
}
```

NOTE: the previous `server_only_tls_config` function is removed; the only TLS config the binary uses is the mTLS one.

- [ ] **Step 2: Update `main.rs`** to call `mtls::mtls_server_config` instead of `server_only_tls_config`. Replace the line:

```rust
    let tls_cfg = mtls::server_only_tls_config(&server_cert, &server_kp)?;
```

with:

```rust
    let tls_cfg = mtls::mtls_server_config(&server_cert, &server_kp, &ca)?;
```

- [ ] **Step 3: Build and run tests.**

```
cargo test -p shoebox-server
```

Expected: all tests still pass.

- [ ] **Step 4: Smoke test.** With the server running, /enroll over TLS without a client cert should still reach the handler (because verifier is `allow_unauthenticated`). With a client cert from a different CA, the handshake should fail.

```bash
rm -rf ./data
cargo run -p shoebox-server &
SERVER_PID=$!
sleep 3

echo "=== /enroll without client cert (TLS handshake should succeed) ==="
curl -sk -X POST https://127.0.0.1:9000/enroll \
  -H 'Content-Type: application/json' \
  -d '{"shared_secret":"wrong","csr_pem":"-----BEGIN CERTIFICATE REQUEST-----\nMIIB\n-----END CERTIFICATE REQUEST-----","display_name":"x"}' \
  -w "\nHTTP %{http_code}\n"
# Expect: HTTP 401 (handshake OK, secret rejected) or 400 (bad CSR).

kill $SERVER_PID
wait $SERVER_PID 2>/dev/null || true
```

- [ ] **Step 5: Commit.**

```bash
git add crates/shoebox-server/src/mtls.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): public listener now requests client cert and trusts only our CA"
```

---

## Task 10: Client identity extractor (read `UserId` from cert subject)

**Files:**
- Create: `crates/shoebox-server/src/identity.rs`
- Modify: `crates/shoebox-server/src/lib.rs`
- Modify: `crates/shoebox-server/src/main.rs` (propagate peer-cert info via connect-info layer)

To get the client cert into a handler, we use `axum-server`'s `accept` hook to capture the peer cert chain at handshake time and stash it into a connection-scoped extension. The extractor reads from that extension.

This is the trickiest part of the plan. Refer to `axum-server`'s `tls-rustls` examples (in particular the `mutual_auth.rs` example in their repo) for the canonical pattern.

- [ ] **Step 1: Write `crates/shoebox-server/src/identity.rs`.**

```rust
//! Extractors for the authenticated client identity.

use axum::async_trait;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::StatusCode;
use shoebox_common::{MachineId, UserId};
use std::str::FromStr;

/// The peer cert chain captured at TLS handshake time, propagated via
/// `axum_server::Handle`'s connect-info layer.
#[derive(Clone, Debug)]
pub struct PeerCertChain {
    pub leaf_der: Vec<u8>,
    pub leaf_serial_hex: String,
    pub subject_cn: String,
    pub subject_ou: String,
}

impl PeerCertChain {
    /// Parse a DER cert into the fields we care about.
    pub fn from_der(der: Vec<u8>) -> Option<Self> {
        use x509_parser::prelude::*;
        let parsed = X509Certificate::from_der(&der).ok()?.1;
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

/// Newtype produced by the extractor; carries verified identity.
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

/// Optional extractor: yields `None` instead of rejecting when no cert
/// is present. Used by /enroll which must accept unauthenticated requests.
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
```

- [ ] **Step 2: Add `pub mod identity;` to `crates/shoebox-server/src/lib.rs`.**

- [ ] **Step 3: Hook up the peer-cert capture in `main.rs`.**

`axum-server` exposes the underlying TLS connection via its `Handle`. The simplest pattern is to use the `accept` hook on `axum_server::tls_rustls::RustlsAcceptor` to capture the peer cert chain and inject it as a request extension via a `tower` `Layer`.

Replace `serve_public_tls` in `main.rs` with:

```rust
async fn serve_public_tls(
    addr: std::net::SocketAddr,
    state: http::AppState,
    tls_cfg: std::sync::Arc<rustls::ServerConfig>,
    shutdown: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    use axum_server::tls_rustls::RustlsConfig;
    let rustls_cfg = RustlsConfig::from_config(tls_cfg);

    tracing::info!(event = "https.listen.public", addr = %addr, "public TLS server bound");
    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = shutdown.await;
        handle_for_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });

    // axum-server 0.7 exposes per-connection TLS info via `RustlsConnection`
    // which is inserted into request extensions automatically. We add a
    // middleware that translates it into our PeerCertChain extension.
    let app = http::public_router(state).layer(axum::middleware::from_fn(
        capture_peer_cert,
    ));

    axum_server::bind_rustls(addr, rustls_cfg)
        .handle(handle)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

async fn capture_peer_cert(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // axum-server exposes peer certs via a request extension named after
    // its internal type. We look up the type and translate it.
    let (mut parts, body) = request.into_parts();

    if let Some(conn_info) = parts
        .extensions
        .get::<axum_server::tls_rustls::RustlsConnectionInfo>()
    {
        if let Some(peer_certs) = conn_info.client_certificates() {
            if let Some(leaf) = peer_certs.first() {
                if let Some(chain) = shoebox_server::identity::PeerCertChain::from_der(
                    leaf.as_ref().to_vec(),
                ) {
                    parts.extensions.insert(chain);
                }
            }
        }
    }

    let request = axum::extract::Request::from_parts(parts, body);
    next.run(request).await
}
```

If `RustlsConnectionInfo` doesn't exist by that exact name in axum-server 0.7 (the API has evolved), the implementer should consult `axum_server::tls_rustls` documentation and adapt. The semantic goal: get the verified peer cert DER from the TLS layer into the request's extension map as a `PeerCertChain`.

- [ ] **Step 4: Build.**

```
cargo build -p shoebox-server
```

Address any API mismatch errors by reading `axum-server`'s actual types — the principle (capture peer cert from TLS layer → translate to PeerCertChain → insert into request extensions) is stable; the exact crate API names may have shifted.

- [ ] **Step 5: Run tests.**

```
cargo test -p shoebox-server
```

Expected: all tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-server/src/identity.rs crates/shoebox-server/src/lib.rs \
        crates/shoebox-server/src/main.rs
git commit -m "feat(server): ClientIdentity extractor reads UserId/MachineId from peer cert"
```

---

## Task 11: CRL check at handshake — reject revoked serials

**Files:**
- Modify: `crates/shoebox-server/src/mtls.rs` (custom verifier wraps WebPkiClientVerifier)

The cleanest place to enforce CRL is in the rustls `ClientCertVerifier` — reject the handshake if the serial is in `revoked_certs`. This avoids serving any request bytes to a revoked cert.

Because rustls verifier methods are synchronous, we need a non-async DB read. Two options:
(a) Snapshot the CRL into memory at startup + refresh periodically (eventually consistent; revocation may take up to refresh interval to apply).
(b) Use a blocking DB call from the verifier (introduces blocking in a non-async path).

Option (a) is the right call. Build a `RevokedSerials` cache that the janitor refreshes every 30 seconds.

- [ ] **Step 1: Add a `CrlCache` module + verifier wrapper.** Append to `crates/shoebox-server/src/mtls.rs`:

```rust
use parking_lot::RwLock;
use std::collections::HashSet;

/// In-memory snapshot of revoked cert serials. Refreshed periodically by
/// a background task spawned at server startup.
#[derive(Clone, Default)]
pub struct CrlCache(Arc<RwLock<HashSet<String>>>);

impl CrlCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, serials: HashSet<String>) {
        *self.0.write() = serials;
    }

    pub fn contains(&self, serial_hex: &str) -> bool {
        self.0.read().contains(serial_hex)
    }
}

/// Verifier that delegates to the inner WebPKI verifier and then rejects
/// any cert whose serial is in the CRL cache.
#[derive(Debug)]
struct CrlAwareVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    crl: CrlCache,
}

impl ClientCertVerifier for CrlAwareVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: rustls::pki_types::UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let verified = self.inner.verify_client_cert(end_entity, intermediates, now)?;
        let serial_hex = {
            use x509_parser::prelude::*;
            match X509Certificate::from_der(end_entity.as_ref()) {
                Ok((_, parsed)) => hex::encode(parsed.serial.to_bytes_be()),
                Err(_) => {
                    return Err(rustls::Error::General(
                        "could not parse client cert serial".into(),
                    ));
                }
            }
        };
        if self.crl.contains(&serial_hex) {
            return Err(rustls::Error::General(format!(
                "client cert revoked (serial={serial_hex})"
            )));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }
}
```

Add `parking_lot` to deps:

In `Cargo.toml` `[workspace.dependencies]`:
```toml
parking_lot = "0.12"
```

In `crates/shoebox-server/Cargo.toml` `[dependencies]`:
```toml
parking_lot = { workspace = true }
```

- [ ] **Step 2: Update `mtls_server_config` to accept a `CrlCache` and wrap the verifier.**

Change the function signature:

```rust
pub fn mtls_server_config(
    server_cert: &IssuedCert,
    server_keypair: &rcgen::KeyPair,
    ca: &Ca,
    crl: CrlCache,
) -> Result<Arc<ServerConfig>> {
    let cert_der = CertificateDer::from(server_cert.cert_der.clone());
    let key_pem = server_keypair.serialize_pem();
    let key_der = parse_first_private_key(&key_pem)?;

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .map_err(|e| anyhow!("loading CA root into trust store: {e}"))?;
    let roots = Arc::new(roots);

    let inner_verifier = WebPkiClientVerifier::builder(roots)
        .allow_unauthenticated()
        .build()
        .map_err(|e| anyhow!("building client verifier: {e}"))?;

    let verifier: Arc<dyn ClientCertVerifier> = Arc::new(CrlAwareVerifier {
        inner: inner_verifier,
        crl,
    });

    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|e| anyhow!("building rustls ServerConfig: {e}"))?;
    Ok(Arc::new(config))
}
```

- [ ] **Step 3: In `main.rs`, create the `CrlCache`, refresh it once at startup, spawn a periodic refresher, and pass to `mtls_server_config`.**

Add after the `ca` bootstrap:

```rust
    let crl = mtls::CrlCache::new();
    refresh_crl(&db, &crl).await?;
    tokio::spawn({
        let db = db.clone();
        let crl = crl.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                if let Err(e) = refresh_crl(&db, &crl).await {
                    tracing::warn!(event = "crl.refresh.error", error = %e);
                }
            }
        }
    });
```

And add the helper at the bottom of `main.rs`:

```rust
async fn refresh_crl(db: &std::sync::Arc<db::Db>, crl: &mtls::CrlCache) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let mut rows = conn.query("SELECT serial_number FROM revoked_certs", ()).await?;
    let mut set = std::collections::HashSet::new();
    while let Some(row) = rows.next().await? {
        set.insert(row.get::<String>(0)?);
    }
    let n = set.len();
    crl.replace(set);
    tracing::debug!(event = "crl.refresh", revoked_count = n);
    Ok(())
}
```

Update the `mtls_server_config` call site:

```rust
    let tls_cfg = mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl.clone())?;
```

- [ ] **Step 4: Build and test.**

```
cargo test -p shoebox-server
```

Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add Cargo.toml crates/shoebox-server/Cargo.toml \
        crates/shoebox-server/src/mtls.rs crates/shoebox-server/src/main.rs
git commit -m "feat(server): CRL-aware client cert verifier with periodic refresh from revoked_certs"
```

---

## Task 12: `/whoami` test endpoint + `/renew` endpoint

**Files:**
- Modify: `crates/shoebox-server/src/enroll.rs` (add /renew)
- Create: `crates/shoebox-server/src/whoami.rs` (debug endpoint)
- Modify: `crates/shoebox-server/src/http.rs` (wire routes)
- Modify: `crates/shoebox-server/src/lib.rs` (export module)

- [ ] **Step 1: Add `/whoami` in `crates/shoebox-server/src/whoami.rs`.**

```rust
//! GET /whoami — returns the authenticated client's identity. Useful as
//! a debugging endpoint and as a known-good auth check for integration
//! tests.

use axum::{http::StatusCode, response::Json, routing::get, Router};
use serde::Serialize;

use crate::http::AppState;
use crate::identity::ClientIdentity;

#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    pub user_id: String,
    pub machine_id: String,
    pub cert_serial_hex: String,
}

pub fn route() -> Router<AppState> {
    Router::new().route("/whoami", get(whoami_handler))
}

async fn whoami_handler(identity: ClientIdentity) -> (StatusCode, Json<WhoamiResponse>) {
    (
        StatusCode::OK,
        Json(WhoamiResponse {
            user_id: identity.user_id.to_string(),
            machine_id: identity.machine_id.to_string(),
            cert_serial_hex: identity.cert_serial_hex,
        }),
    )
}
```

- [ ] **Step 2: Add `/renew` handler in `crates/shoebox-server/src/enroll.rs`.**

Append:

```rust
#[derive(Debug, Deserialize)]
pub struct RenewRequest {
    pub csr_pem: String,
}

#[derive(Debug, Serialize)]
pub struct RenewResponse {
    pub client_cert_pem: String,
    pub cert_serial_hex: String,
    pub not_after_unix: i64,
}

pub fn renew_route() -> Router<AppState> {
    Router::new().route("/renew", axum::routing::post(renew_handler))
}

async fn renew_handler(
    State(state): State<AppState>,
    identity: crate::identity::ClientIdentity,
    Json(req): Json<RenewRequest>,
) -> Result<(StatusCode, Json<RenewResponse>), (StatusCode, String)> {
    let issued = sign_csr(&state.ca, &req.csr_pem, &identity.user_id, &identity.machine_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("csr: {e}")))?;

    tracing::info!(
        event = "renewal.completed",
        user_id = %identity.user_id,
        machine_id = %identity.machine_id,
        old_serial = %identity.cert_serial_hex,
        new_serial = %issued.serial_hex,
        "client cert renewed"
    );

    Ok((
        StatusCode::OK,
        Json(RenewResponse {
            client_cert_pem: issued.cert_pem,
            cert_serial_hex: issued.serial_hex,
            not_after_unix: issued.not_after.unix_timestamp(),
        }),
    ))
}
```

- [ ] **Step 3: Wire both routes into `public_router` in `crates/shoebox-server/src/http.rs`.**

Replace `public_router`:

```rust
pub fn public_router(state: AppState) -> Router {
    Router::new()
        .merge(crate::enroll::route())
        .merge(crate::enroll::renew_route())
        .merge(crate::whoami::route())
        .with_state(state)
}
```

- [ ] **Step 4: Expose the new module in `lib.rs`.** Add `pub mod whoami;`.

- [ ] **Step 5: Build.**

```
cargo build -p shoebox-server
```

Expected: clean build. Note that `/enroll` is reachable without a client cert (because `ClientIdentity` extractor isn't on the enroll handler signature), but `/renew` and `/whoami` require it.

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-server/src/whoami.rs crates/shoebox-server/src/enroll.rs \
        crates/shoebox-server/src/http.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): /whoami and /renew endpoints behind ClientIdentity extractor"
```

---

## Task 13: CLI structure (`clap`) + `revoke` subcommand

**Files:**
- Create: `crates/shoebox-server/src/cli.rs`
- Create: `crates/shoebox-server/src/revoke.rs`
- Modify: `crates/shoebox-server/src/main.rs` (use clap to dispatch)
- Modify: `crates/shoebox-server/src/lib.rs`

- [ ] **Step 1: Write `crates/shoebox-server/src/cli.rs`.**

```rust
//! Command-line interface.
//!
//! Default invocation (no subcommand) is `serve`, preserving Plan 1.1's
//! zero-arg behaviour. The `revoke` subcommand revokes a client cert by
//! its serial.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "shoebox-server", about = "shoebox catalog server")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server (default if no subcommand is given).
    Serve,
    /// Revoke a client certificate by its hex serial.
    Revoke(RevokeArgs),
}

#[derive(Debug, clap::Args)]
pub struct RevokeArgs {
    /// Hex-encoded serial number of the cert to revoke.
    #[arg(long)]
    pub serial: String,

    /// Optional human-readable reason.
    #[arg(long)]
    pub reason: Option<String>,
}
```

- [ ] **Step 2: Write `crates/shoebox-server/src/revoke.rs`.**

```rust
//! `revoke` subcommand implementation.

use anyhow::Result;

use crate::cli::RevokeArgs;
use crate::config::Config;
use crate::db::Db;

pub async fn run(args: &RevokeArgs, cfg: &Config) -> Result<()> {
    let db = Db::open(&cfg.data_dir.join("catalog.db")).await?;
    db.insert_revoked_cert(&args.serial, args.reason.as_deref(), None)
        .await?;
    println!("Revoked cert with serial {} (reason: {})",
        args.serial,
        args.reason.as_deref().unwrap_or("<none>"));
    Ok(())
}
```

- [ ] **Step 3: Update `crates/shoebox-server/src/main.rs` to dispatch via clap.**

At the top of `main()`, replace the existing `logging::init();` line with:

```rust
    logging::init();

    let cli = <cli::Cli as clap::Parser>::parse();

    // Load config early so all subcommands have access to it.
    let cfg = load_config()?;

    match cli.command.unwrap_or(cli::Command::Serve) {
        cli::Command::Serve => serve_main(cfg).await,
        cli::Command::Revoke(args) => revoke::run(&args, &cfg).await,
    }
}

fn load_config() -> anyhow::Result<config::Config> {
    let cfg_path = std::env::var("SHOEBOX_CONFIG").ok();
    Ok(if let Some(p) = cfg_path {
        tracing::info!(event = "config.load", path = %p, "loading config file");
        config::Config::load_from_path(std::path::Path::new(&p))?
    } else {
        tracing::info!(event = "config.load", source = "env", "no SHOEBOX_CONFIG; building from env");
        config::Config::from_env_with_defaults()
    })
}

async fn serve_main(cfg: config::Config) -> anyhow::Result<()> {
    mtls::install_crypto_provider();
```

And wrap the rest of the current `main()` body (everything after `mtls::install_crypto_provider();`) inside `serve_main`'s body (renaming/removing the `#[tokio::main] async fn main()` wrapper if needed). The structure becomes:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();
    let cli = <cli::Cli as clap::Parser>::parse();
    let cfg = load_config()?;
    match cli.command.unwrap_or(cli::Command::Serve) {
        cli::Command::Serve => serve_main(cfg).await,
        cli::Command::Revoke(args) => revoke::run(&args, &cfg).await,
    }
}

fn load_config() -> anyhow::Result<config::Config> { /* ... as above ... */ }

async fn serve_main(cfg: config::Config) -> anyhow::Result<()> {
    mtls::install_crypto_provider();
    // ... everything that used to be in main() after the config load,
    //     ending with the broadcaster.shutdown(); result line.
}
```

Update imports at top of main.rs to include `cli` and `revoke`:

```rust
use shoebox_server::{ca, cli, config, db, http, logging, mdns, mtls, revoke, secret};
```

- [ ] **Step 4: Expose `cli` and `revoke` modules in `crates/shoebox-server/src/lib.rs`.** Add `pub mod cli;` and `pub mod revoke;`.

- [ ] **Step 5: Build and smoke-test.**

```bash
cargo build -p shoebox-server

# Zero-arg invocation still starts the server:
./target/debug/shoebox-server &
SERVER_PID=$!
sleep 2
curl -sf http://127.0.0.1:9001/health && echo
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null || true

# revoke subcommand works:
./target/debug/shoebox-server revoke --serial abcdef0123456789 --reason "test"
```

- [ ] **Step 6: Commit.**

```bash
git add crates/shoebox-server/src/cli.rs crates/shoebox-server/src/revoke.rs \
        crates/shoebox-server/src/main.rs crates/shoebox-server/src/lib.rs
git commit -m "feat(server): clap CLI with default 'serve' and 'revoke --serial' subcommands"
```

---

## Task 14: End-to-end enrollment integration test

**Files:**
- Create: `crates/shoebox-server/tests/enroll_e2e.rs`

This is the most important verification of Plan 1.2: a client generates a keypair, builds a CSR, POSTs to `/enroll` with the shared secret, receives a signed cert, uses that cert to make an mTLS connection to the same server, and successfully calls `/whoami`.

- [ ] **Step 1: Write `crates/shoebox-server/tests/enroll_e2e.rs`.**

```rust
//! End-to-end: bootstrap server -> enroll a client -> use the cert to
//! authenticate to /whoami.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::ClientConfig;
use rustls::RootCertStore;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
async fn enroll_then_use_cert_to_call_whoami() {
    // Install rustls provider once per test process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // Bootstrap server-side state.
    let db = std::sync::Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
    let conn = db.connect().unwrap();
    let secret_plaintext = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        shoebox_server::secret::EnsureOutcome::AlreadySet => {
            panic!("freshly created db should generate a secret")
        }
    };

    let ca = std::sync::Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    let (server_cert, server_kp) = ca.issue_server_cert(&sans).unwrap();

    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg = shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl)
        .unwrap();

    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
    };

    // Bind to ephemeral port and capture it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    drop(listener); // axum-server will rebind; for the test we can hardcode the addr below.

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = shoebox_server::http::public_router(state).layer(axum::middleware::from_fn(
        capture_peer_cert,
    ));
    let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(tls_cfg);
    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        handle_for_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(2)));
    });
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(addr, rustls_cfg)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });

    // Generate client keypair + CSR.
    let client_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        // CN/OU are overwritten by the server when it signs.
        dn.push(rcgen::DnType::CommonName, "placeholder");
        dn
    };
    let csr_pem = csr_params.serialize_request(&client_kp).unwrap().pem().unwrap();

    // Step 1: enroll over TLS but no client cert needed yet.
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();
    let enroll_client_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_client_cfg)
        .build()
        .unwrap();

    let enroll_resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret_plaintext,
            "csr_pem": csr_pem,
            "display_name": "Alice",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(enroll_resp.status(), 200, "enroll should succeed");
    let body: serde_json::Value = enroll_resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();
    let user_id = body["user_id"].as_str().unwrap().to_string();

    // Step 2: build a TLS client that presents the new cert and call /whoami.
    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&client_kp.serialize_pem()).unwrap();
    let authed_client_cfg = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(vec![CertificateDer::from(client_cert_der)], client_key_der)
        .unwrap();
    let authed_http = Client::builder()
        .use_preconfigured_tls(authed_client_cfg)
        .build()
        .unwrap();

    let whoami_resp = authed_http
        .get(format!("https://{addr}/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(whoami_resp.status(), 200, "whoami should succeed");
    let body: serde_json::Value = whoami_resp.json().await.unwrap();
    assert_eq!(body["user_id"], user_id);

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

// Re-define the capture middleware locally so the test doesn't depend on
// it being exported. Identical to the one in main.rs.
async fn capture_peer_cert(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let (mut parts, body) = request.into_parts();
    if let Some(conn_info) = parts
        .extensions
        .get::<axum_server::tls_rustls::RustlsConnectionInfo>()
    {
        if let Some(peer_certs) = conn_info.client_certificates() {
            if let Some(leaf) = peer_certs.first() {
                if let Some(chain) = shoebox_server::identity::PeerCertChain::from_der(
                    leaf.as_ref().to_vec(),
                ) {
                    parts.extensions.insert(chain);
                }
            }
        }
    }
    let request = axum::extract::Request::from_parts(parts, body);
    next.run(request).await
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

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 2: Run the test.**

Run: `cargo test -p shoebox-server --test enroll_e2e`
Expected: PASS.

If the test fails on TLS handshake / peer cert extraction, that's expected: this is the integration point most likely to need adapting based on the actual axum-server/rustls API. The implementer may need to consult upstream examples and adjust the `capture_peer_cert` middleware OR the verifier wiring. Document any deviations in the report.

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/tests/enroll_e2e.rs
git commit -m "test(server): end-to-end enrollment + mTLS-authenticated /whoami"
```

---

## Task 15: Revocation invalidates subsequent connections

**Files:**
- Create: `crates/shoebox-server/tests/revoke_e2e.rs`

- [ ] **Step 1: Write `crates/shoebox-server/tests/revoke_e2e.rs`.**

```rust
//! End-to-end: enroll, use cert successfully, revoke serial, refresh CRL,
//! subsequent connection with the same cert is rejected at TLS handshake.

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use reqwest::Client;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

#[tokio::test]
async fn revoked_cert_cannot_reconnect() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let db = Arc::new(
        shoebox_server::db::Db::open(&data_dir.join("catalog.db"))
            .await
            .unwrap(),
    );
    let conn = db.connect().unwrap();
    let secret_plaintext = match shoebox_server::secret::ensure_present(&conn).await.unwrap() {
        shoebox_server::secret::EnsureOutcome::Generated { plaintext } => plaintext,
        _ => panic!(),
    };
    let ca = Arc::new(shoebox_server::ca::Ca::open(&data_dir).unwrap());
    let sans = shoebox_server::ca::build_server_sans("shoebox-test", &[]);
    let (server_cert, server_kp) = ca.issue_server_cert(&sans).unwrap();
    let crl = shoebox_server::mtls::CrlCache::new();
    let tls_cfg =
        shoebox_server::mtls::mtls_server_config(&server_cert, &server_kp, &ca, crl.clone())
            .unwrap();
    let state = shoebox_server::http::AppState {
        db: db.clone(),
        schema_version: shoebox_common::SCHEMA_VERSION,
        ca: ca.clone(),
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = shoebox_server::http::public_router(state).layer(axum::middleware::from_fn(
        capture_peer_cert,
    ));
    let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(tls_cfg);
    let handle = axum_server::Handle::new();
    let handle_for_shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        handle_for_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(2)));
    });
    let server = tokio::spawn(async move {
        axum_server::bind_rustls(addr, rustls_cfg)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });

    // Enroll.
    let client_kp = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut csr_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    csr_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "x");
        dn
    };
    let csr_pem = csr_params.serialize_request(&client_kp).unwrap().pem().unwrap();
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(ca.root_cert_der.clone()))
        .unwrap();

    let enroll_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_no_client_auth();
    let enroll_http = Client::builder()
        .use_preconfigured_tls(enroll_cfg)
        .build()
        .unwrap();
    let resp = enroll_http
        .post(format!("https://{addr}/enroll"))
        .json(&serde_json::json!({
            "shared_secret": secret_plaintext,
            "csr_pem": csr_pem,
            "display_name": "Bob",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let client_cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();
    let cert_serial = body["cert_serial_hex"].as_str().unwrap().to_string();

    // First connection with cert: success.
    let client_cert_der = pem_to_der(&client_cert_pem).unwrap();
    let client_key_der = parse_first_private_key(&client_kp.serialize_pem()).unwrap();
    let authed_cfg = ClientConfig::builder()
        .with_root_certificates(root_store.clone())
        .with_client_auth_cert(vec![CertificateDer::from(client_cert_der.clone())], client_key_der)
        .unwrap();
    let authed_http = Client::builder()
        .use_preconfigured_tls(authed_cfg)
        .build()
        .unwrap();
    let resp = authed_http
        .get(format!("https://{addr}/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Revoke the cert, refresh CRL by hand (simulating what the
    // production refresher does every 30s).
    db.insert_revoked_cert(&cert_serial, Some("test"), None).await.unwrap();
    let mut set = std::collections::HashSet::new();
    set.insert(cert_serial.clone());
    crl.replace(set);

    // Second connection: handshake should fail. reqwest reports this
    // as a connection error.
    let result = authed_http
        .get(format!("https://{addr}/whoami"))
        .send()
        .await;
    assert!(result.is_err(), "revoked cert should fail handshake; got {result:?}");

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

// (Repeat the capture_peer_cert + pem_to_der + parse_first_private_key
// helpers from enroll_e2e.rs verbatim. Yes, this is duplicated — keep
// them local so the test files don't depend on a shared test-utils crate.)
async fn capture_peer_cert(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let (mut parts, body) = request.into_parts();
    if let Some(conn_info) = parts
        .extensions
        .get::<axum_server::tls_rustls::RustlsConnectionInfo>()
    {
        if let Some(peer_certs) = conn_info.client_certificates() {
            if let Some(leaf) = peer_certs.first() {
                if let Some(chain) = shoebox_server::identity::PeerCertChain::from_der(
                    leaf.as_ref().to_vec(),
                ) {
                    parts.extensions.insert(chain);
                }
            }
        }
    }
    let request = axum::extract::Request::from_parts(parts, body);
    next.run(request).await
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

fn parse_first_private_key(pem: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;
    let mut cur = pem.as_bytes();
    while let Some(Ok(item)) = rustls_pemfile::read_one(&mut cur).transpose() {
        match item {
            Item::Pkcs8Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(k)),
            Item::Pkcs1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(k)),
            Item::Sec1Key(k) => return Some(rustls::pki_types::PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 2: Run the test.**

```
cargo test -p shoebox-server --test revoke_e2e
```

Expected: PASS. If the connection error after revocation surfaces as something other than `Result::Err` (e.g. some HTTP-layer fallback), inspect the failure mode and adjust the assertion to match — the semantic check is "the revoked cert cannot be used to authenticate."

- [ ] **Step 3: Commit.**

```bash
git add crates/shoebox-server/tests/revoke_e2e.rs
git commit -m "test(server): revoked client cert is rejected at TLS handshake"
```

---

## Task 16: Update Dockerfile, README, CLAUDE.md for the new ports + secret

**Files:**
- Modify: `Dockerfile`
- Modify: `README.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update `Dockerfile` to expose both ports and add a healthcheck.**

Append after `EXPOSE 9000`:

```dockerfile
EXPOSE 9001
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD wget -qO- http://127.0.0.1:9001/health || exit 1
```

(If `wget` isn't in `debian:bookworm-slim`, add `wget` to the `apt-get install` line in the runtime stage. As of Debian 12 it's not there by default.)

Update the runtime stage's apt-get install to:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/shoebox shoebox
```

- [ ] **Step 2: Update `README.md`** "Building the Docker image" section to mention the dual ports and where to find the enrollment secret. Replace that section with:

```markdown
## Building the Docker image

```bash
docker build -t shoebox-server:dev .
```

Run with a Docker named volume (recommended for local testing — no host
permission issues). Exposes the mTLS-protected catalog port (9000) and
the unauthenticated loopback `/health` port (9001, only useful from
container healthchecks):

```bash
docker run --rm -p 9000:9000 \
  -v shoebox-data:/var/lib/shoebox \
  shoebox-server:dev
```

On first run, the server prints a generated enrollment secret to the log
exactly once. Share it with users out-of-band; they'll need it to enroll
their clients. To pre-set the secret, pass `-e SHOEBOX_SECRET=your-phrase`.

Run with a host-mounted directory (matches a typical NAS deployment).
The container runs as UID 10001 (`shoebox`), so the host directory must
be owned by that UID:

```bash
mkdir -p /srv/shoebox-data
sudo chown 10001:10001 /srv/shoebox-data
docker run --rm -p 9000:9000 \
  -v /srv/shoebox-data:/var/lib/shoebox \
  shoebox-server:dev
```

A docker-compose template for typical NAS deployments (Synology, QNAP,
TrueNAS) ships in Plan 1.5.
```

- [ ] **Step 3: Update `CLAUDE.md`** "Implementation status" section. Replace its bullets with:

```markdown
- `crates/shoebox-server` — workspace skeleton, libSQL catalog with 6 migrations, internal Ed25519 CA + mTLS, /enroll + /renew + /whoami endpoints, CRL-aware client cert verification, clap CLI with `serve`/`revoke` subcommands, mDNS broadcaster, multi-stage Dockerfile. No indexer, thumbnailer, or libSQL wire proxy yet (Plan 1.3).
- `crates/shoebox-common` — shared `Error`/`Result`, `UserId`/`MachineId` types, `SCHEMA_VERSION` constant.
- Run locally: `cargo run -p shoebox-server` (mTLS on `0.0.0.0:9000`, health on `127.0.0.1:9001`).
- Run in Docker: `docker build -t shoebox-server:dev . && docker run --rm -p 9000:9000 -v shoebox-data:/var/lib/shoebox shoebox-server:dev`.
- CI: fmt + clippy + tests + docker build on push and PR.
- **Toolchain:** `rust-toolchain.toml` pins `stable` (currently ~1.95). MSRV in workspace `Cargo.toml` is 1.85 — that's the floor for `libsql 0.6`'s transitive deps (edition2024).
```

Also update the sub-project status row in the table at top to:

```
| 1 | **Catalog, sync & stack** | Plans 1.1+1.2 implemented — workspace, schema, /health, mDNS, mTLS + enrollment + revocation, Dockerfile, CI. Plans 1.3-1.5 pending. | [spec](docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md) |
```

- [ ] **Step 4: Final verification.**

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

All four must pass cleanly.

- [ ] **Step 5: Commit.**

```bash
git add Dockerfile README.md CLAUDE.md
git commit -m "docs: update Dockerfile/README/CLAUDE.md for mTLS + dual listeners + enrollment"
```

---

## Definition of Done for Plan 1.2

After all 16 tasks are complete:

- `cargo test --workspace` passes (existing 12 tests + new identity/secret/ca/enroll/revoke tests).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- `cargo run -p shoebox-server` starts, generates CA + shared secret on first launch (secret printed once to log), binds mTLS on `:9000` and plain HTTP `/health` on `127.0.0.1:9001`.
- A client can POST to `/enroll` with the shared secret + a CSR and receive a signed cert chain.
- A client presenting that cert can call `/whoami` and `/renew`.
- `shoebox-server revoke --serial <hex>` inserts a row into `revoked_certs`; the CRL refresher picks it up within ~30 seconds; subsequent connections with that serial fail at TLS handshake.
- `docker build` still succeeds; the new HEALTHCHECK reports the container as healthy.

What this plan **does not** deliver — covered in subsequent plans:
- Embedded sqld + libSQL wire-protocol proxy for client embedded replicas (Plan 1.3).
- Filesystem indexer, thumbnailer, develop-lock server operations, janitor, backups, metrics endpoints (Plan 1.3).
- Iced desktop client + enrollment UI (Plan 1.4).
- docker-compose template, Helm chart, multi-arch builds, install docs (Plan 1.5).

---

## Self-Review

**Spec coverage (against `docs/superpowers/specs/2026-05-17-catalog-sync-and-stack-design.md` §7):**

- §7.1 Server bootstrap → Tasks 3, 4 (CA generation + shared secret).
- §7.2 Client enrollment flow → Task 8 (/enroll endpoint).
- §7.3 Steady-state mTLS → Task 9 (require client cert) + Task 10 (identity extractor).
- §7.4 Cert lifecycle (90-day client certs, renew at 30 days, CRL via `revoked_certs`) → Task 12 (/renew) + Task 11 (CRL) + Task 13 (revoke CLI).
- §7.5 mDNS discovery → already in Plan 1.1.
- §7.6 Client first-run flow — server-side surface (`/enroll` accepting unauthenticated requests) is covered; the client UI is Plan 1.4.
- §7.7 Trust boundary → respected (LAN-default, no user passwords yet).

**Explicit deferrals (called out in spec or plan):**
- §9.2 backup VACUUM INTO — Plan 1.3.
- Embedded sqld + libSQL wire proxy — Plan 1.3.
- Client cert auto-renewal at 30 days — client-side concern, Plan 1.4.
- Server cert auto-renewal in background — present in main.rs initial setup but no scheduled renewer task; the cert is currently re-issued at every server restart. **Add a server-cert renewer in Plan 1.3 alongside the other background tasks.**

**Placeholder scan:** No "TODO/TBD/fill in" strings. The two notes that look like placeholders (Task 2's `unreachable!` and the Task 10 note about API drift) are explicit handling instructions, not placeholders.

**Type consistency:** `AppState` defined in Task 7, extended in Task 8 (adds `ca`); both `enroll_handler` and `whoami_handler` use the extended shape. `ClientIdentity` defined in Task 10, consumed by `whoami_handler` (Task 12) and `renew_handler` (Task 12). `CrlCache` defined in Task 11, consumed by `mtls_server_config` (Task 11) and by main.rs's refresher. `UserId`/`MachineId` defined in common (Task 2), serialized in `EnrollResponse` (Task 8).

**Known risk for the implementing engineer:** axum-server's exact API for surfacing peer certs (used in Task 10 and the e2e tests) has changed across versions. The pattern shown (`RustlsConnectionInfo::client_certificates`) reflects axum-server 0.7's intent; if the actual type/method names differ, follow the pattern (capture at TLS layer, expose via request extension) rather than the exact API.
