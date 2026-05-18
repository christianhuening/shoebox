//! HTTP endpoints serving cached thumbnails and previews. Files are
//! content-addressed by BLAKE3 hash; we only serve files under
//! `cache_dir` to prevent path traversal.
//!
//! Path-traversal safety: `is_valid_hash` rejects any input that isn't
//! exactly 64 lowercase hex chars, so the resulting filename contains
//! no `.` or `/` separators and cannot escape the
//! `cache_dir/{thumbnails,previews}/` directory built by
//! [`crate::thumbnailer::cache_path`].

use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::http::AppState;
use crate::identity::ClientIdentity;
use crate::thumbnailer::{cache_path, ThumbnailKind};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/thumbs/{hash}", get(get_thumb))
        .route("/previews/{hash}", get(get_preview))
}

async fn get_thumb(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    AxumPath(hash_hex_param): AxumPath<String>,
) -> Response {
    serve_cached_jpeg(
        state.cache_dir.as_path(),
        ThumbnailKind::Thumb,
        &hash_hex_param,
    )
    .await
}

async fn get_preview(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    AxumPath(hash_hex_param): AxumPath<String>,
) -> Response {
    serve_cached_jpeg(
        state.cache_dir.as_path(),
        ThumbnailKind::Preview,
        &hash_hex_param,
    )
    .await
}

async fn serve_cached_jpeg(
    cache_dir: &std::path::Path,
    kind: ThumbnailKind,
    hash_hex_param: &str,
) -> Response {
    if !is_valid_hash(hash_hex_param) {
        return (StatusCode::BAD_REQUEST, "invalid hash").into_response();
    }
    let cached_jpeg_path = cache_path(cache_dir, kind, hash_hex_param);
    match tokio::fs::read(&cached_jpeg_path).await {
        Ok(cached_jpeg_bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
            ],
            cached_jpeg_bytes,
        )
            .into_response(),
        Err(read_err) if read_err.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "thumbnail not ready").into_response()
        }
        Err(read_err) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("io: {read_err}")).into_response()
        }
    }
}

fn is_valid_hash(hash_candidate: &str) -> bool {
    hash_candidate.len() == 64
        && hash_candidate
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_hashes() {
        assert!(!is_valid_hash(""));
        assert!(!is_valid_hash("short"));
        assert!(!is_valid_hash(&"g".repeat(64))); // non-hex
        assert!(!is_valid_hash(&"A".repeat(64))); // uppercase
        assert!(is_valid_hash(&"a".repeat(64)));
        assert!(is_valid_hash(&"0123456789abcdef".repeat(4)));
    }
}
