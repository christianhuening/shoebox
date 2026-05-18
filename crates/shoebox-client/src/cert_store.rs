//! Per-server client cert + key storage. Default backend is the OS
//! keychain (via the `keyring` crate); explicit-consent fallback to a
//! mode-0600 file under the OS app-data dir is added in Task 5.
//!
//! Keying: each (`server_url`, kind) pair gets its own keychain entry.
//! That way one client paired with multiple servers keeps cert sets
//! separate, and the cert ↔ key are stored as siblings.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Once;

const SERVICE_PREFIX: &str = "shoebox-client";
const FILE_CERT_NAME: &str = "client.cert.pem";
const FILE_KEY_NAME: &str = "client.key.pem";

/// keyring 4 dropped its default-backend auto-init; callers must register
/// one before any `Entry::new`. Do it lazily on first access.
///
/// `not_keyutils = true` selects Secret Service on Linux (persistent
/// across reboots) over kernel keyutils (session-scoped, lost on logout) —
/// matches the persistence guarantee shoebox needs for client certs.
static INIT_KEYRING_BACKEND: Once = Once::new();
fn ensure_keyring_backend() {
    INIT_KEYRING_BACKEND.call_once(|| {
        // Errors here just leave the default store unset; subsequent
        // Entry::new() calls will fail loudly with NoDefaultStore, which
        // is what we want (surface backend issues to the consent flow).
        let _ = keyring::use_native_store(true);
    });
}

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

fn keyring_entry(server_url: &str, kind: EntryKind) -> Result<keyring_core::Entry> {
    ensure_keyring_backend();
    let service = service_name(server_url, kind);
    keyring_core::Entry::new(&service, "default-user")
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
        Err(keyring_core::Error::NoEntry) => return Ok(None),
        Err(other) => return Err(anyhow!("reading cert from keyring: {other}")),
    };
    let key_pem = match keyring_entry(server_url, EntryKind::Key)?.get_password() {
        Ok(pem) => pem,
        Err(keyring_core::Error::NoEntry) => return Ok(None),
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
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(delete_err) => return Err(anyhow!("deleting {kind:?} from keyring: {delete_err}")),
        }
    }
    Ok(())
}

/// Returns the directory under which `store_in_file` / `load_from_file`
/// place the cert + key files for `server_url`. Hashes the URL into the
/// filename so multiple servers don't collide.
fn file_storage_dir(server_url: &str) -> Option<PathBuf> {
    let project_dirs = directories::ProjectDirs::from("io", "shoebox", "shoebox-client")?;
    let server_slug = hex::encode(blake3::hash(server_url.as_bytes()).as_bytes());
    Some(
        project_dirs
            .data_local_dir()
            .join("certs")
            .join(server_slug),
    )
}

/// Store (cert, key) on disk under the app-data dir with mode 0600 on Unix.
/// Caller has already consented to file storage (e.g., keychain unavailable).
///
/// # Errors
/// Returns an error on directory creation, file write, or permission set
/// failure.
pub fn store_in_file(server_url: &str, cert_pem: &str, key_pem: &str) -> Result<()> {
    let storage_dir =
        file_storage_dir(server_url).ok_or_else(|| anyhow!("could not determine app-data dir"))?;
    std::fs::create_dir_all(&storage_dir)
        .with_context(|| format!("creating {}", storage_dir.display()))?;

    let cert_path = storage_dir.join(FILE_CERT_NAME);
    let key_path = storage_dir.join(FILE_KEY_NAME);
    write_with_mode_0600(&cert_path, cert_pem)?;
    write_with_mode_0600(&key_path, key_pem)?;
    Ok(())
}

/// Load (cert, key) from the file-storage dir, or `None` if not present.
///
/// # Errors
/// Returns an error only on read failure of an existing file (missing
/// files yield `Ok(None)`).
pub fn load_from_file(server_url: &str) -> Result<Option<(String, String)>> {
    let storage_dir =
        file_storage_dir(server_url).ok_or_else(|| anyhow!("could not determine app-data dir"))?;
    let cert_path = storage_dir.join(FILE_CERT_NAME);
    let key_path = storage_dir.join(FILE_KEY_NAME);
    if !cert_path.exists() || !key_path.exists() {
        return Ok(None);
    }
    let cert_pem = std::fs::read_to_string(&cert_path)
        .with_context(|| format!("reading {}", cert_path.display()))?;
    let key_pem = std::fs::read_to_string(&key_path)
        .with_context(|| format!("reading {}", key_path.display()))?;
    Ok(Some((cert_pem, key_pem)))
}

/// Delete the file-stored cert + key for `server_url`. Missing files are OK.
///
/// # Errors
/// Returns an error only on filesystem failure other than `NotFound`.
pub fn delete_from_file(server_url: &str) -> Result<()> {
    let Some(storage_dir) = file_storage_dir(server_url) else {
        return Ok(());
    };
    for filename in [FILE_CERT_NAME, FILE_KEY_NAME] {
        let target = storage_dir.join(filename);
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(remove_err) if remove_err.kind() == std::io::ErrorKind::NotFound => {}
            Err(remove_err) => {
                return Err(remove_err).with_context(|| format!("deleting {}", target.display()));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn write_with_mode_0600(path: &Path, body: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_with_mode_0600(path: &Path, body: &str) -> Result<()> {
    // On Windows, ACL the file to the current user. For v1 we rely on
    // the per-user data dir already being protected; Plan 1.4b can
    // tighten with a proper SDDL ACL.
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
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
        let Ok(writer) = keyring_core::Entry::new(&service, "probe-user") else {
            eprintln!("skipping: keyring backend unavailable");
            return true;
        };
        if writer.set_password("probe").is_err() {
            eprintln!("skipping: keyring backend present but write failed");
            return true;
        }
        let Ok(reader) = keyring_core::Entry::new(&service, "probe-user") else {
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

    #[test]
    fn file_storage_round_trip() {
        let server_url = format!("https://file-test-{}.local:9000", uuid_like());
        let cert = "fake-cert-bytes";
        let key = "fake-key-bytes";

        store_in_file(&server_url, cert, key).unwrap();
        let loaded = load_from_file(&server_url).unwrap().unwrap();
        assert_eq!(loaded.0, cert);
        assert_eq!(loaded.1, key);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let storage_dir = file_storage_dir(&server_url).unwrap();
            let cert_path = storage_dir.join(FILE_CERT_NAME);
            let mode = std::fs::metadata(&cert_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "cert file must be mode 0600, got {mode:o}");
        }

        delete_from_file(&server_url).unwrap();
        assert!(load_from_file(&server_url).unwrap().is_none());
    }

    #[test]
    fn file_load_missing_returns_none() {
        let server_url = format!("https://file-absent-{}.local:9000", uuid_like());
        assert!(load_from_file(&server_url).unwrap().is_none());
    }
}
