//! On-disk + in-memory cache for thumbnails fetched from
//! `<server>/thumbs/<hash>`. Reads share a single in-flight
//! `tokio::sync::OnceCell` per hash so concurrent `get`s for the same
//! key do one HTTP round-trip.

use anyhow::Result;
use image::DynamicImage;
use lru::LruCache;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

const MEMORY_CAPACITY: usize = 1024;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ThumbError {
    #[error("network: {0}")]
    Http(String),
    #[error("io: {0}")]
    Io(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("server returned status {0}")]
    Status(u16),
}

pub type CachedResult = std::result::Result<Arc<DynamicImage>, ThumbError>;

struct State {
    memory: LruCache<String, Arc<DynamicImage>>,
    in_flight: HashMap<String, Arc<OnceCell<CachedResult>>>,
}

#[derive(Clone)]
pub struct ThumbCache {
    state: Arc<Mutex<State>>,
    http_client: reqwest::Client,
    server_base_url: String,
    disk_dir: PathBuf,
}

impl ThumbCache {
    /// Construct a cache. `disk_dir` is created if missing.
    pub fn new(
        http_client: reqwest::Client,
        server_base_url: String,
        disk_dir: PathBuf,
    ) -> Result<Self> {
        std::fs::create_dir_all(&disk_dir)?;
        let capacity =
            NonZeroUsize::new(MEMORY_CAPACITY).expect("MEMORY_CAPACITY is non-zero");
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                memory: LruCache::new(capacity),
                in_flight: HashMap::new(),
            })),
            http_client,
            server_base_url: server_base_url.trim_end_matches('/').to_string(),
            disk_dir,
        })
    }

    /// Path on disk where the JPEG for `hash` is (or would be) cached.
    #[must_use]
    pub fn disk_path_for(&self, hash: &str) -> PathBuf {
        self.disk_dir.join(format!("{hash}.jpg"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn build_cache(disk_dir: PathBuf) -> ThumbCache {
        ThumbCache::new(reqwest::Client::new(), "https://server.invalid".into(), disk_dir)
            .expect("cache constructs")
    }

    #[test]
    fn disk_path_includes_hash_and_jpg_extension() {
        let tmp = TempDir::new().unwrap();
        let cache = build_cache(tmp.path().to_path_buf());
        let path = cache.disk_path_for("deadbeef");
        assert_eq!(path.file_name().unwrap(), "deadbeef.jpg");
        assert_eq!(path.parent().unwrap(), tmp.path());
    }

    #[test]
    fn new_creates_disk_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested/dir");
        let _cache = build_cache(nested.clone());
        assert!(nested.is_dir());
    }
}
