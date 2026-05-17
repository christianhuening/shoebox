//! Pulls the embedded JPEG preview from a RAW, renders 256 px + 2k
//! versions, writes them content-addressed under `<cache_dir>`.
//!
//! Output layout:
//!
//! ```text
//! <cache_dir>/
//!   thumbnails/<blake3-hex>.jpg   (longest edge <= 256 px)
//!   previews/<blake3-hex>.jpg     (longest edge <= 2048 px)
//! ```
//!
//! Writes are atomic: encode to `<final>.jpg.tmp` next to the final
//! path, fsync, then `rename` over the final path.
//!
//! This module is intentionally synchronous. Callers (Task 11's indexer
//! hook) wrap calls in `tokio::task::spawn_blocking`.

use anyhow::{anyhow, Context, Result};
use image::{codecs::jpeg::JpegEncoder, GenericImageView, ImageFormat};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use crate::raw_preview;

/// Longest-edge pixel target for the small grid thumbnail.
pub const THUMB_PX: u32 = 256;

/// Longest-edge pixel target for the loupe/zoom preview.
pub const PREVIEW_PX: u32 = 2048;

/// JPEG quality (1–100) used when re-encoding the resized preview.
/// 90 is a good balance between size and visible quality and matches
/// `raw_preview::JPEG_REENCODE_QUALITY`.
const JPEG_QUALITY: u8 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThumbnailKind {
    Thumb,
    Preview,
}

impl ThumbnailKind {
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            ThumbnailKind::Thumb => "thumbnails",
            ThumbnailKind::Preview => "previews",
        }
    }

    #[must_use]
    pub fn target_px(self) -> u32 {
        match self {
            ThumbnailKind::Thumb => THUMB_PX,
            ThumbnailKind::Preview => PREVIEW_PX,
        }
    }
}

/// Returns the path where the cached image lives.
#[must_use]
pub fn cache_path(cache_dir: &Path, kind: ThumbnailKind, hash_hex: &str) -> PathBuf {
    cache_dir
        .join(kind.dir_name())
        .join(format!("{hash_hex}.jpg"))
}

/// Build (if absent) the cached thumbnail/preview for one photo.
/// Returns `true` if a new file was written, `false` if it already
/// existed.
///
/// # Errors
///
/// Returns an error if the cache directory cannot be created, the
/// embedded preview cannot be extracted or decoded, the resized image
/// cannot be JPEG-encoded, or the atomic temp-file rename fails.
pub fn build_one(
    cache_dir: &Path,
    raw_path: &Path,
    hash_hex: &str,
    kind: ThumbnailKind,
) -> Result<bool> {
    let output_jpeg_path = cache_path(cache_dir, kind, hash_hex);
    if output_jpeg_path.exists() {
        return Ok(false);
    }
    let output_directory = output_jpeg_path.parent().ok_or_else(|| {
        anyhow!(
            "cache path {} has no parent directory",
            output_jpeg_path.display()
        )
    })?;
    std::fs::create_dir_all(output_directory)
        .with_context(|| format!("mkdir {}", output_directory.display()))?;

    let embedded_jpeg_bytes = raw_preview::extract_preview(raw_path)?;
    let decoded_image =
        image::load_from_memory_with_format(&embedded_jpeg_bytes, ImageFormat::Jpeg).map_err(
            |decode_error| {
                anyhow!(
                    "decoding embedded JPEG for {}: {decode_error}",
                    raw_path.display()
                )
            },
        )?;

    let (width, height) = decoded_image.dimensions();
    let target = kind.target_px();
    let resized_image = if width.max(height) > target {
        // Downscale dimensions; precision loss is intended and bounded
        // by target_px <= 2048, so the f32 round-trip is safe and the
        // resulting u32 truncation only drops sub-pixel remainders.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let (target_width, target_height) = {
            let ratio = target as f32 / width.max(height) as f32;
            let target_width = ((width as f32 * ratio) as u32).max(1);
            let target_height = ((height as f32 * ratio) as u32).max(1);
            (target_width, target_height)
        };
        decoded_image.thumbnail(target_width, target_height)
    } else {
        decoded_image
    };

    // Atomic write: encode to temp file in the same directory, fsync,
    // then rename over the final path.
    let temp_output_path = output_jpeg_path.with_extension("jpg.tmp");
    {
        let mut temp_file = std::fs::File::create(&temp_output_path)
            .with_context(|| format!("creating {}", temp_output_path.display()))?;
        let mut jpeg_buffer = Vec::new();
        let mut jpeg_cursor = Cursor::new(&mut jpeg_buffer);
        let jpeg_encoder = JpegEncoder::new_with_quality(&mut jpeg_cursor, JPEG_QUALITY);
        resized_image
            .write_with_encoder(jpeg_encoder)
            .map_err(|encode_error| anyhow!("JPEG encode failed: {encode_error}"))?;
        temp_file.write_all(&jpeg_buffer)?;
        temp_file.sync_all()?;
    }
    std::fs::rename(&temp_output_path, &output_jpeg_path).with_context(|| {
        format!(
            "renaming {} -> {}",
            temp_output_path.display(),
            output_jpeg_path.display()
        )
    })?;
    Ok(true)
}

/// Build both thumb and preview for one photo. Best-effort: logs and
/// returns `Ok` even if one of the two fails, so a single bad RAW
/// doesn't poison a whole indexer batch.
///
/// # Errors
///
/// Currently infallible (per-kind errors are logged and swallowed), but
/// the `Result` signature is preserved so callers don't need to change
/// if that policy is revisited.
pub fn build_both(cache_dir: &Path, raw_path: &Path, hash_hex: &str) -> Result<()> {
    match build_one(cache_dir, raw_path, hash_hex, ThumbnailKind::Thumb) {
        Ok(true) => tracing::debug!(event = "thumb.built", kind = "thumb", hash = %hash_hex),
        Ok(false) => {}
        Err(build_error) => tracing::warn!(
            event = "thumb.error",
            kind = "thumb",
            hash = %hash_hex,
            error = %build_error,
        ),
    }
    match build_one(cache_dir, raw_path, hash_hex, ThumbnailKind::Preview) {
        Ok(true) => tracing::debug!(event = "thumb.built", kind = "preview", hash = %hash_hex),
        Ok(false) => {}
        Err(build_error) => tracing::warn!(
            event = "thumb.error",
            kind = "preview",
            hash = %hash_hex,
            error = %build_error,
        ),
    }
    Ok(())
}
