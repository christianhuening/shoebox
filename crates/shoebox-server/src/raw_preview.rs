//! Extract the embedded JPEG preview from a RAW file.
//!
//! Supported formats follow whatever `rawler` supports (covers Pentax
//! PEF/DNG and Fuji RAF as required by the spec).
//!
//! # Implementation note — why we re-encode
//!
//! `rawler` 0.7's `Decoder` trait only exposes the embedded preview as a
//! fully-decoded `image::DynamicImage` (the JPEG bytes are read internally
//! and decoded before we see them). Some decoders — PEF in particular —
//! additionally apply a crop to strip camera-internal black borders, so
//! even if we got the raw JPEG we'd want the cropped output anyway.
//!
//! Net effect: one JPEG-decode (inside rawler) + one JPEG-encode (here)
//! per call. Task 10's thumbnailer consumes the returned JPEG bytes and
//! immediately decodes them again for resize — there's room to fuse those
//! passes if benchmarks ever flag the cost.

use anyhow::{anyhow, Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageBuffer, Rgb};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use std::io::Cursor;
use std::path::Path;

/// JPEG quality (1–100) used when re-encoding the embedded preview.
/// 90 matches typical camera-embedded preview quality without
/// noticeably re-compressing.
const JPEG_REENCODE_QUALITY: u8 = 90;

/// Returns JPEG-encoded bytes of the largest available embedded preview
/// in the RAW at `raw_path`.
///
/// Tries `full_image` first (the full-resolution embedded preview on
/// PEF/RAF/DNG), then `preview_image`, then `thumbnail_image`. The
/// returned bytes are always a valid baseline JPEG suitable for feeding
/// directly into `image::load_from_memory_with_format`.
///
/// # Errors
///
/// Returns an error if the file cannot be opened, if `rawler` cannot
/// identify or decode the file, if none of the preview accessors returns
/// an image, or if JPEG re-encoding fails.
pub fn extract_preview(raw_path: &Path) -> Result<Vec<u8>> {
    let raw_source =
        RawSource::new(raw_path).with_context(|| format!("opening RAW {}", raw_path.display()))?;

    let decoder = rawler::get_decoder(&raw_source).map_err(|rawler_error| {
        anyhow!(
            "rawler could not identify {}: {rawler_error}",
            raw_path.display()
        )
    })?;

    // Try the accessors in descending order of fidelity. PEF/RAF only
    // implement `full_image`; DNG implements `thumbnail_image` and
    // `full_image`. Anything else falls back gracefully.
    let decode_params = RawDecodeParams::default();
    let decoded_preview = decoder
        .full_image(&raw_source, &decode_params)
        .map_err(|rawler_error| {
            anyhow!(
                "rawler full_image failed for {}: {rawler_error}",
                raw_path.display()
            )
        })?
        .or_else(|| {
            decoder
                .preview_image(&raw_source, &decode_params)
                .ok()
                .flatten()
        })
        .or_else(|| {
            decoder
                .thumbnail_image(&raw_source, &decode_params)
                .ok()
                .flatten()
        })
        .ok_or_else(|| anyhow!("no embedded preview found in {}", raw_path.display()))?;

    // rawler 0.7's DynamicImage is the same type as our workspace's
    // image-0.25, so we can hand it straight to the encoder.
    let rgb_image: ImageBuffer<Rgb<u8>, Vec<u8>> = decoded_preview.into_rgb8();

    let mut jpeg_output = Cursor::new(Vec::<u8>::new());
    let jpeg_encoder = JpegEncoder::new_with_quality(&mut jpeg_output, JPEG_REENCODE_QUALITY);
    DynamicImage::ImageRgb8(rgb_image)
        .write_with_encoder(jpeg_encoder)
        .with_context(|| format!("re-encoding preview for {}", raw_path.display()))?;

    Ok(jpeg_output.into_inner())
}

#[cfg(test)]
mod tests {
    // No unit test — exercising rawler requires real RAW files we don't
    // ship in CI. The thumbnailer e2e (planned post-Plan 1.3) will cover
    // this path with a fixture.
}
