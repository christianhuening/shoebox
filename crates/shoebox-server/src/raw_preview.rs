//! Extract the embedded JPEG preview from a RAW file.
//!
//! Supported formats follow whatever `rawler` supports (covers Pentax
//! PEF/DNG and Fuji RAF as required by the spec).
//!
//! # Implementation note — why we re-encode
//!
//! `rawler` 0.6 does not expose the raw embedded JPEG byte stream from
//! a RAW file publicly. Its `Decoder::full_image`/`preview_image`/
//! `thumbnail_image` methods all eagerly decode any embedded JPEG via
//! `image::load_from_memory_with_format` and return a `DynamicImage`.
//! Some decoders (notably PEF) additionally apply a crop to strip
//! camera-internal black borders, so even if rawler exposed the raw
//! bytes we'd want the cropped result anyway.
//!
//! Additionally, `rawler` 0.6 pins `image = "0.24"` while this
//! workspace uses `image = "0.25"`. The two `DynamicImage` types are
//! distinct, so we extract the decoded RGB8 pixels from rawler's
//! 0.24-typed image and rebuild an `image` 0.25 buffer for JPEG
//! re-encoding.
//!
//! Net effect: one JPEG-decode + one JPEG-encode per call. Task 10
//! (thumbnailer) consumes the returned JPEG bytes, so it can use the
//! workspace's `image` crate directly without a separate decode step
//! beyond the resize-time JPEG decode it already needs.

use anyhow::{anyhow, Context, Result};
use image::{ImageBuffer, Rgb};
use rawler::RawFile;
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

/// JPEG quality (1–100) used when re-encoding the embedded preview.
/// 90 matches typical camera-embedded preview quality without
/// noticeably re-compressing.
const JPEG_REENCODE_QUALITY: u8 = 90;

/// Returns JPEG-encoded bytes of the largest available embedded
/// preview in the RAW at `raw_path`.
///
/// Tries `full_image` first (the full-resolution embedded preview on
/// PEF/RAF/DNG), then `preview_image`, then `thumbnail_image`. The
/// returned bytes are always a valid baseline JPEG suitable for
/// feeding directly into `image::load_from_memory_with_format`.
///
/// # Errors
///
/// Returns an error if the file cannot be opened, if `rawler` cannot
/// identify or decode the file, if none of the preview accessors
/// returns an image, or if JPEG re-encoding fails.
pub fn extract_preview(raw_path: &Path) -> Result<Vec<u8>> {
    let raw_file_handle = File::open(raw_path)
        .with_context(|| format!("opening RAW {}", raw_path.display()))?;
    let mut raw_file = RawFile::from(BufReader::new(raw_file_handle));

    let decoder = rawler::get_decoder(&mut raw_file).map_err(|rawler_error| {
        anyhow!(
            "rawler could not identify {}: {rawler_error}",
            raw_path.display()
        )
    })?;

    // Try the accessors in descending order of fidelity. PEF/RAF only
    // implement `full_image`; DNG implements `thumbnail_image` and
    // `full_image`. Anything else falls back gracefully.
    let decoded_preview = decoder
        .full_image(&mut raw_file)
        .map_err(|rawler_error| {
            anyhow!(
                "rawler full_image failed for {}: {rawler_error}",
                raw_path.display()
            )
        })?
        .or_else(|| {
            decoder
                .preview_image(&mut raw_file)
                .ok()
                .flatten()
        })
        .or_else(|| {
            decoder
                .thumbnail_image(&mut raw_file)
                .ok()
                .flatten()
        })
        .ok_or_else(|| {
            anyhow!(
                "no embedded preview found in {}",
                raw_path.display()
            )
        })?;

    // rawler returns an `image` 0.24 `DynamicImage`. We can't name
    // that type here (workspace uses `image` 0.25), but we can call
    // methods on it without naming it. `to_rgb8()` is defined on
    // 0.24's DynamicImage and returns an `ImageBuffer<Rgb<u8>>` we
    // immediately decompose into raw bytes + dimensions.
    let preview_width = decoded_preview.width();
    let preview_height = decoded_preview.height();
    let rgb8_bytes = decoded_preview.to_rgb8().into_raw();

    let reencode_buffer: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(preview_width, preview_height, rgb8_bytes)
            .ok_or_else(|| {
                anyhow!(
                    "rawler returned an RGB buffer with mismatched dimensions for {}",
                    raw_path.display()
                )
            })?;

    let mut jpeg_output = Cursor::new(Vec::<u8>::new());
    let jpeg_encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        &mut jpeg_output,
        JPEG_REENCODE_QUALITY,
    );
    image::DynamicImage::ImageRgb8(reencode_buffer)
        .write_with_encoder(jpeg_encoder)
        .with_context(|| format!("re-encoding preview for {}", raw_path.display()))?;

    Ok(jpeg_output.into_inner())
}

#[cfg(test)]
mod tests {
    // No unit test in this task — exercising rawler requires real RAW
    // files we don't ship in CI. Task 18 (thumbnailer_e2e) covers this
    // path with a fixture.
}
