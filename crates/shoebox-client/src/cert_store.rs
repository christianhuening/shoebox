//! Per-server client cert + key storage. Default backend is the OS
//! keychain (via the `keyring` crate); explicit-consent fallback to a
//! mode-0600 file under the OS app-data dir is added in Task 5.
//!
//! Keying: each (`server_url`, kind) pair gets its own keychain entry.
//! That way one client paired with multiple servers keeps cert sets
//! separate, and the cert ↔ key are stored as siblings.

use anyhow::{anyhow, Context, Result};

const SERVICE_PREFIX: &str = "shoebox-client";

/// Identifies which half of a cert pair an entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Cert,
    Key,
}

impl EntryKind {
    fn suffix(self) -> &'static str {
        match self {
            EntryKind::Cert => "cert",
            EntryKind::Key => "key",
        }
    }
}

fn service_name(server_url: &str, kind: EntryKind) -> String {
    format!("{SERVICE_PREFIX}::{}::{}", kind.suffix(), server_url)
}

fn keyring_entry(server_url: &str, kind: EntryKind) -> Result<keyring::Entry> {
    let service = service_name(server_url, kind);
    keyring::Entry::new(&service, "default-user")
        .with_context(|| format!("opening keyring entry for {service}"))
}

/// Store the (`cert_pem`, `key_pem`) pair for `server_url` in the OS keychain.
///
/// # Errors
/// Returns the underlying keyring error if either write fails. On failure
/// of the second write, the first write is rolled back (best-effort delete).
pub fn store_in_keyring(server_url: &str, cert_pem: &str, key_pem: &str) -> Result<()> {
    let cert_entry = keyring_entry(server_url, EntryKind::Cert)?;
    cert_entry
        .set_password(cert_pem)
        .context("writing cert to keyring")?;

    let key_entry = keyring_entry(server_url, EntryKind::Key)?;
    if let Err(key_err) = key_entry.set_password(key_pem) {
        let _ = cert_entry.delete_credential();
        return Err(anyhow!("writing key to keyring: {key_err}"));
    }
    Ok(())
}

/// Load the (`cert_pem`, `key_pem`) pair for `server_url` from the OS keychain,
/// or `None` if no entry exists.
///
/// # Errors
/// Returns an error only on backend failure (not on "entry missing", which
/// returns `Ok(None)`).
pub fn load_from_keyring(server_url: &str) -> Result<Option<(String, String)>> {
    let cert_pem = match keyring_entry(server_url, EntryKind::Cert)?.get_password() {
        Ok(pem) => pem,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(other) => return Err(anyhow!("reading cert from keyring: {other}")),
    };
    let key_pem = match keyring_entry(server_url, EntryKind::Key)?.get_password() {
        Ok(pem) => pem,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(other) => return Err(anyhow!("reading key from keyring: {other}")),
    };
    Ok(Some((cert_pem, key_pem)))
}

/// Delete the cert + key entries for `server_url` from the OS keychain.
/// Missing entries are not an error.
///
/// # Errors
/// Returns the first non-`NoEntry` backend error encountered.
pub fn delete_from_keyring(server_url: &str) -> Result<()> {
    for kind in [EntryKind::Cert, EntryKind::Key] {
        match keyring_entry(server_url, kind)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(delete_err) => return Err(anyhow!("deleting {kind:?} from keyring: {delete_err}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skip-if-no-secret-service helper. Some CI Linux runners have no
    /// Secret Service backend; those env runs print "skipping" and return.
    ///
    /// We probe by writing through one `Entry` and reading through a
    /// freshly-constructed `Entry` for the same service+user. The default
    /// fallback backend (`keyring::mock`) only persists data inside the
    /// originating `Entry` instance, so a second `Entry::new` for the
    /// same service comes back empty — that's the signal to skip.
    fn skip_if_no_backend() -> bool {
        let service = format!("shoebox-test-probe-{}", uuid_like());
        let Ok(writer) = keyring::Entry::new(&service, "probe-user") else {
            eprintln!("skipping: keyring backend unavailable");
            return true;
        };
        if writer.set_password("probe").is_err() {
            eprintln!("skipping: keyring backend present but write failed");
            return true;
        }
        let Ok(reader) = keyring::Entry::new(&service, "probe-user") else {
            let _ = writer.delete_credential();
            eprintln!("skipping: keyring backend unavailable on re-open");
            return true;
        };
        let read_back = reader.get_password();
        let _ = writer.delete_credential();
        let _ = reader.delete_credential();
        if matches!(read_back, Ok(ref value) if value == "probe") {
            false
        } else {
            eprintln!("skipping: keyring backend does not persist across Entry::new");
            true
        }
    }

    #[test]
    fn round_trip_via_keyring() {
        if skip_if_no_backend() {
            return;
        }
        let server_url = format!("https://test-{}.local:9000", uuid_like());
        let cert = "-----BEGIN CERTIFICATE-----\nfake-cert\n-----END CERTIFICATE-----\n";
        let key = "-----BEGIN PRIVATE KEY-----\nfake-key\n-----END PRIVATE KEY-----\n";

        store_in_keyring(&server_url, cert, key).unwrap();
        let loaded = load_from_keyring(&server_url)
            .unwrap()
            .expect("entry should exist");
        assert_eq!(loaded.0, cert);
        assert_eq!(loaded.1, key);

        delete_from_keyring(&server_url).unwrap();
        let after_delete = load_from_keyring(&server_url).unwrap();
        assert!(after_delete.is_none());
    }

    #[test]
    fn load_missing_returns_none() {
        if skip_if_no_backend() {
            return;
        }
        let server_url = format!("https://nonexistent-{}.local:9000", uuid_like());
        assert!(load_from_keyring(&server_url).unwrap().is_none());
    }

    #[test]
    fn delete_missing_is_ok() {
        if skip_if_no_backend() {
            return;
        }
        let server_url = format!("https://also-missing-{}.local:9000", uuid_like());
        delete_from_keyring(&server_url).unwrap();
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{nanos:x}")
    }
}
