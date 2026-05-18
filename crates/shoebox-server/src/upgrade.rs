//! Pre-startup upgrade helpers. Currently the only one is renaming the
//! legacy `catalog.db` from before sub-1-3-5 — when shoebox-server wrote
//! to that file directly instead of going through sqld.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// If `<data_dir>/catalog.db` exists, rename it to
/// `<data_dir>/catalog.db.legacy-pre-grpc-fix-<unix_ts>` and log a
/// `WARN catalog.legacy.renamed` event. Idempotent — does nothing if the
/// file is absent.
///
/// We rename rather than delete so an operator can manually inspect or
/// recover anything they care about. The renamed file is otherwise
/// unused by the new code path; it can be deleted at the operator's
/// discretion.
///
/// # Errors
/// Returns an error if the legacy file exists but cannot be renamed
/// (e.g. permissions, cross-device link). The system clock running
/// before the Unix epoch is also surfaced as an error.
pub fn rename_legacy_catalog_db(data_dir: &Path) -> Result<()> {
    let legacy = data_dir.join("catalog.db");
    if !legacy.exists() {
        return Ok(());
    }
    let unix_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("system clock before epoch: {e}"))?
        .as_secs();
    let renamed = data_dir.join(format!("catalog.db.legacy-pre-grpc-fix-{unix_ts}"));
    std::fs::rename(&legacy, &renamed)
        .with_context(|| format!("renaming {} → {}", legacy.display(), renamed.display()))?;
    tracing::warn!(
        event = "catalog.legacy.renamed",
        from = %legacy.display(),
        to = %renamed.display(),
        "found pre-sub-1-3-5 catalog.db; renamed (sqld is now the single source of truth)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn no_legacy_file_is_a_noop() {
        let dir = TempDir::new().unwrap();
        rename_legacy_catalog_db(dir.path()).unwrap();
        // Nothing created.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn existing_catalog_db_is_renamed() {
        let dir = TempDir::new().unwrap();
        let legacy = dir.path().join("catalog.db");
        std::fs::write(&legacy, b"old-data").unwrap();
        rename_legacy_catalog_db(dir.path()).unwrap();
        assert!(!legacy.exists(), "legacy file should be gone");
        let renamed_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("catalog.db.legacy-pre-grpc-fix-")
            })
            .count();
        assert_eq!(renamed_count, 1, "expected exactly one renamed file");
    }
}
