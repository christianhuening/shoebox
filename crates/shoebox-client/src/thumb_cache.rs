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
    #[allow(clippy::needless_pass_by_value)]
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

    /// Fetch a thumbnail. Returns an in-memory hit instantly; otherwise
    /// resolves a single in-flight load and caches the result.
    pub async fn get(&self, hash: &str) -> CachedResult {
        if let Some(image) = self.state.lock().memory.get(hash).cloned() {
            return Ok(image);
        }

        let cell = {
            let mut state = self.state.lock();
            // Re-check inside the lock in case another caller filled it.
            if let Some(image) = state.memory.get(hash).cloned() {
                return Ok(image);
            }
            state
                .in_flight
                .entry(hash.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let result = cell
            .get_or_init(|| async { self.load_uncached(hash).await })
            .await
            .clone();

        {
            let mut state = self.state.lock();
            state.in_flight.remove(hash);
            if let Ok(image) = &result {
                state.memory.put(hash.to_string(), image.clone());
            }
        }
        result
    }

    async fn load_uncached(&self, hash: &str) -> CachedResult {
        // Disk hit first.
        let disk_path = self.disk_path_for(hash);
        if disk_path.exists() {
            return match tokio::fs::read(&disk_path).await {
                Ok(bytes) => decode_jpeg(&bytes),
                Err(error) => Err(ThumbError::Io(error.to_string())),
            };
        }
        // Cold miss — HTTP.
        let url = format!("{}/thumbs/{hash}", self.server_base_url);
        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|error| ThumbError::Http(error.to_string()))?;
        if !response.status().is_success() {
            return Err(ThumbError::Status(response.status().as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| ThumbError::Http(error.to_string()))?;
        // Persist to disk best-effort (don't fail the read if write fails).
        if let Err(error) = tokio::fs::write(&disk_path, &bytes).await {
            tracing::warn!(%hash, "failed to persist thumbnail to disk: {error}");
        }
        decode_jpeg(&bytes)
    }
}

fn decode_jpeg(bytes: &[u8]) -> CachedResult {
    let cursor = std::io::Cursor::new(bytes);
    let reader = image::ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|error| ThumbError::Decode(error.to_string()))?;
    let image = reader
        .decode()
        .map_err(|error| ThumbError::Decode(error.to_string()))?;
    Ok(Arc::new(image))
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

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 255, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    async fn spawn_counting_server(jpeg: Vec<u8>) -> (String, StdArc<AtomicUsize>) {
        use axum::{routing::get, Router};
        let counter = StdArc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let jpeg_clone = jpeg.clone();
        let app = Router::new().route(
            "/thumbs/:hash",
            get(move || {
                let counter = counter_clone.clone();
                let jpeg = jpeg_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (
                        [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                        jpeg,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), counter)
    }

    #[tokio::test]
    async fn concurrent_gets_for_same_hash_do_one_http_request() {
        let tmp = TempDir::new().unwrap();
        let (url, counter) = spawn_counting_server(tiny_jpeg()).await;
        let cache =
            ThumbCache::new(reqwest::Client::new(), url, tmp.path().to_path_buf()).unwrap();
        let cache_a = cache.clone();
        let cache_b = cache.clone();
        let (result_a, result_b) =
            tokio::join!(cache_a.get("hash1"), cache_b.get("hash1"));
        assert!(result_a.is_ok());
        assert!(result_b.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_call_after_first_succeeds_is_memory_hit() {
        let tmp = TempDir::new().unwrap();
        let (url, counter) = spawn_counting_server(tiny_jpeg()).await;
        let cache =
            ThumbCache::new(reqwest::Client::new(), url, tmp.path().to_path_buf()).unwrap();
        assert!(cache.get("hash1").await.is_ok());
        assert!(cache.get("hash1").await.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
