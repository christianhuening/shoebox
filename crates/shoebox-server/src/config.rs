//! Server configuration: loaded from TOML, overridable by env vars.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server_name: String,
    pub bind_addr: SocketAddr,
    pub data_dir: PathBuf,
    pub photos_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Config {
    pub fn from_toml_str(toml_str: &str) -> anyhow::Result<Self> {
        toml::from_str(toml_str).map_err(|e| anyhow::anyhow!("invalid config TOML: {e}"))
    }

    #[must_use]
    pub fn apply_env_overrides(mut self) -> Self {
        if let Ok(v) = std::env::var("SHOEBOX_BIND_ADDR") {
            if let Ok(addr) = v.parse() {
                self.bind_addr = addr;
            }
        }
        if let Ok(v) = std::env::var("SHOEBOX_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("SHOEBOX_PHOTOS_DIR") {
            self.photos_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("SHOEBOX_CACHE_DIR") {
            self.cache_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("SHOEBOX_SERVER_NAME") {
            self.server_name = v;
        }
        self
    }
}

impl Config {
    pub fn load_from_path(path: &std::path::Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config {}: {e}", path.display()))?;
        Ok(Self::from_toml_str(&s)?.apply_env_overrides())
    }

    /// Build a Config from environment variables alone, with sensible
    /// defaults for any not set. Used when no `server.toml` is present.
    #[must_use]
    pub fn from_env_with_defaults() -> Self {
        Self {
            server_name: std::env::var("SHOEBOX_SERVER_NAME").unwrap_or_else(|_| {
                hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| "shoebox".to_string())
            }),
            bind_addr: std::env::var("SHOEBOX_BIND_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:9000".into())
                .parse()
                .expect("SHOEBOX_BIND_ADDR must parse as SocketAddr"),
            data_dir: std::path::PathBuf::from(
                std::env::var("SHOEBOX_DATA_DIR").unwrap_or_else(|_| "/var/lib/shoebox".into()),
            ),
            photos_dir: std::path::PathBuf::from(
                std::env::var("SHOEBOX_PHOTOS_DIR").unwrap_or_else(|_| "/photos".into()),
            ),
            cache_dir: std::path::PathBuf::from(
                std::env::var("SHOEBOX_CACHE_DIR").unwrap_or_else(|_| "/shoebox-cache".into()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_toml() {
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
        assert_eq!(cfg.data_dir, PathBuf::from("/var/lib/shoebox"));
    }

    #[test]
    fn env_overrides_take_precedence() {
        std::env::set_var("SHOEBOX_BIND_ADDR", "0.0.0.0:8888");
        let s = r#"
            server_name = "x"
            bind_addr = "127.0.0.1:9000"
            data_dir = "/d"
            photos_dir = "/p"
            cache_dir = "/c"
        "#;
        let cfg = Config::from_toml_str(s).unwrap().apply_env_overrides();
        assert_eq!(cfg.bind_addr.port(), 8888);
        std::env::remove_var("SHOEBOX_BIND_ADDR");
    }
}
