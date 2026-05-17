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
