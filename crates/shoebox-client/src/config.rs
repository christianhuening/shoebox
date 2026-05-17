//! Client configuration persisted to `client.toml` under the OS's
//! per-user config directory (via `directories::ProjectDirs`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ClientConfig {
    /// `https://host:port` of the paired shoebox-server. Empty string
    /// means first-run.
    #[serde(default)]
    pub server_url: String,
    /// Hex-encoded serial of the client cert in keychain. Empty string
    /// means no cert yet.
    #[serde(default)]
    pub cert_serial_hex: String,
    /// `users.id` of the user last picked in the profile picker.
    #[serde(default)]
    pub last_active_user_id: Option<String>,
}

impl ClientConfig {
    /// True if the client has never completed first-run.
    #[must_use]
    pub fn is_first_run(&self) -> bool {
        self.server_url.is_empty() || self.cert_serial_hex.is_empty()
    }

    /// Read the config from `path`, or return defaults if the file is missing.
    ///
    /// # Errors
    /// Returns an error only on read or parse failure of an existing file.
    pub fn read_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(toml_text) => toml::from_str(&toml_text)
                .with_context(|| format!("parsing client config {}", path.display())),
            Err(read_err) if read_err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(read_err) => Err(read_err).context("reading client config"),
        }
    }

    /// Atomic write: serialize to a sibling `.tmp` file, then rename.
    ///
    /// # Errors
    /// Returns an error if the parent directory can't be created, or if
    /// any of the write / rename steps fail.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        if let Some(parent_dir) = path.parent() {
            std::fs::create_dir_all(parent_dir)
                .with_context(|| format!("creating config dir {}", parent_dir.display()))?;
        }
        let toml_text = toml::to_string_pretty(self).context("serializing client config")?;
        let temp_path = path.with_extension("toml.tmp");
        std::fs::write(&temp_path, toml_text)
            .with_context(|| format!("writing {}", temp_path.display()))?;
        std::fs::rename(&temp_path, path)
            .with_context(|| format!("renaming {} -> {}", temp_path.display(), path.display()))?;
        Ok(())
    }
}

/// Returns the canonical location of `client.toml` for this user, based
/// on `directories::ProjectDirs`. Returns `None` if the directories
/// crate can't determine a config dir (extremely rare; headless build).
#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("io", "shoebox", "shoebox-client")
        .map(|project_dirs| project_dirs.config_dir().join("client.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let cfg = ClientConfig::read_from(&tmp.path().join("absent.toml")).unwrap();
        assert!(cfg.is_first_run());
        assert!(cfg.server_url.is_empty());
        assert!(cfg.last_active_user_id.is_none());
    }

    #[test]
    fn round_trip_full_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("client.toml");
        let written = ClientConfig {
            server_url: "https://nas.local:9000".to_string(),
            cert_serial_hex: "abc123".to_string(),
            last_active_user_id: Some("user-1".to_string()),
        };
        written.write_to(&path).unwrap();
        let read_back = ClientConfig::read_from(&path).unwrap();
        assert_eq!(read_back.server_url, written.server_url);
        assert_eq!(read_back.cert_serial_hex, written.cert_serial_hex);
        assert_eq!(read_back.last_active_user_id, written.last_active_user_id);
        assert!(!read_back.is_first_run());
    }

    #[test]
    fn partial_file_missing_optional_field() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("partial.toml");
        std::fs::write(
            &path,
            r#"
            server_url = "https://x:9000"
            cert_serial_hex = "deadbeef"
            "#,
        )
        .unwrap();
        let cfg = ClientConfig::read_from(&path).unwrap();
        assert!(!cfg.is_first_run());
        assert!(cfg.last_active_user_id.is_none());
    }

    #[test]
    fn is_first_run_true_when_either_field_empty() {
        assert!(ClientConfig::default().is_first_run());
        assert!(ClientConfig {
            server_url: "x".into(),
            ..Default::default()
        }
        .is_first_run());
        assert!(ClientConfig {
            cert_serial_hex: "x".into(),
            ..Default::default()
        }
        .is_first_run());
    }
}
