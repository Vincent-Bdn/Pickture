//! Decoding, encoding and metadata.
//!
//! The thumbnail path is the reason the rewrite exists, so it is worth being
//! explicit about the order it tries things in:
//!
//! 1. **The JPEG embedded in the EXIF APP1 segment.** Almost every camera and
//!    phone writes one. Reading it is a seek and a tiny decode.
//! 2. **A DCT-scaled decode.** The JPEG decoder can emit at 1/8, 1/4 or 1/2
//!    without ever materialising full resolution.
//! 3. **A full decode plus resize.** Non-JPEG formats only.
//!
//! The abandoned draft did step 3 for everything, then kept the full-resolution
//! buffer alive forever. That is the whole story of why it was slow.

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

use crate::model::ExifSummary;

// ---------------------------------------------------------------------------
// EXIF
// ---------------------------------------------------------------------------

fn read_exif(path: &Path) -> Option<exif::Exif> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    exif::Reader::new().read_from_container(&mut reader).ok()
}

/// The four values the info bar shows. Missing fields stay `None` rather than
/// being rendered as placeholders.
pub fn read_exif_summary(path: &Path) -> Option<ExifSummary> {
    use exif::{In, Tag};
    let exif = read_exif(path)?;

    let display = |tag: Tag| -> Option<String> {
        exif.get_field(tag, In::PRIMARY)
            .map(|f| f.display_value().to_string())
    };

    let shutter = exif
        .get_field(Tag::ExposureTime, In::PRIMARY)
        .map(|f| f.display_value().to_string())
        .map(|s| s.replace(" s", ""));

    let aperture = exif
        .get_field(Tag::FNumber, In::PRIMARY)
        .and_then(|f| match &f.value {
            exif::Value::Rational(r) if !r.is_empty() => Some(format!("f/{:.1}", r[0].to_f64())),
            _ => None,
        });

    let iso = display(Tag::PhotographicSensitivity).map(|v| format!("ISO {v}"));

    let focal = exif
        .get_field(Tag::FocalLength, In::PRIMARY)
        .and_then(|f| match &f.value {
            exif::Value::Rational(r) if !r.is_empty() => {
                Some(format!("{}mm", r[0].to_f64().round() as i64))
            }
            _ => None,
        });

    let summary = ExifSummary {
        shutter,
        aperture,
        iso,
        focal,
    };
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

/// EXIF orientation, 1..8. Anything else is treated as 1.
fn read_orientation(path: &Path) -> u16 {
    use exif::{In, Tag};
    read_exif(path)
        .and_then(|e| {
            e.get_field(Tag::Orientation, In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
        })
        .map(|v| v as u16)
        .unwrap_or(1)
}

/// Apply an EXIF orientation so portrait frames are not shown on their side.
fn apply_orientation(img: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

// ---------------------------------------------------------------------------
// The embedded thumbnail
// ---------------------------------------------------------------------------

/// Pull the JPEG stored in IFD1 of the EXIF block, if there is one.
fn embedded_thumbnail(path: &Path) -> Option<DynamicImage> {
    use exif::{In, Tag};
    let exif = read_exif(path)?;

    let offset = exif
        .get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL)?
        .value
        .get_uint(0)? as usize;
    let length = exif
        .get_field(Tag::JPEGInterchangeFormatLength, In::THUMBNAIL)?
        .value
        .get_uint(0)? as usize;

    // Offsets are relative to the start of the TIFF header, which is exactly
    // what `buf()` returns. Bounds-check before slicing — a truncated file will
    // otherwise panic here rather than falling through to the next strategy.
    let buf = exif.buf();
    let end = offset.checked_add(length)?;
    if length == 0 || end > buf.len() {
        return None;
    }

    image::load_from_memory_with_format(&buf[offset..end], ImageFormat::Jpeg).ok()
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn is_jpeg(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg") | Some("jpeg")
    )
}

/// Decode a JPEG with the decoder emitting at the smallest DCT scale that still
/// covers `target`, which never allocates the full-resolution buffer.
///
/// This goes through `jpeg-decoder` rather than the `image` facade: as of
/// `image` 0.25.10 the JPEG backend is zune-jpeg, which has no scaled-decode
/// API. Scaling here is not an optimisation, it is the reason thumbnailing is
/// fast enough to be worth doing at all — roughly 12 ms against 150 ms for a
/// 24 MP frame.
fn decode_jpeg_scaled(path: &Path, target: u32) -> Result<DynamicImage> {
    use jpeg_decoder::PixelFormat;

    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(file));

    let t = target.clamp(1, u16::MAX as u32) as u16;
    // Requests the smallest supported factor that still covers `t` on at least
    // one axis. A decoder that declines to scale simply decodes at full size.
    let _ = decoder.scale(t, t);

    let pixels = decoder.decode().map_err(|e| anyhow!("decode jpeg: {e}"))?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow!("jpeg carried no frame header"))?;

    let (w, h) = (info.width as u32, info.height as u32);
    let expected = |n: usize| -> Result<()> {
        let want = (w as usize) * (h as usize) * n;
        if pixels.len() < want {
            return Err(anyhow!(
                "jpeg pixel buffer short: {} < {want}",
                pixels.len()
            ));
        }
        Ok(())
    };

    let img = match info.pixel_format {
        PixelFormat::RGB24 => {
            expected(3)?;
            DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(w, h, pixels)
                    .ok_or_else(|| anyhow!("rgb buffer did not fit"))?,
            )
        }
        PixelFormat::L8 => {
            expected(1)?;
            DynamicImage::ImageLuma8(
                image::GrayImage::from_raw(w, h, pixels)
                    .ok_or_else(|| anyhow!("luma buffer did not fit"))?,
            )
        }
        PixelFormat::L16 => {
            expected(2)?;
            let shorts: Vec<u16> = pixels
                .chunks_exact(2)
                .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                .collect();
            DynamicImage::ImageLuma16(
                image::ImageBuffer::from_raw(w, h, shorts)
                    .ok_or_else(|| anyhow!("luma16 buffer did not fit"))?,
            )
        }
        PixelFormat::CMYK32 => {
            expected(4)?;
            // Adobe CMYK JPEGs store inverted values; this is the conversion
            // every decoder applies for them.
            let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
            for px in pixels.chunks_exact(4) {
                let k = px[3] as u32;
                rgb.push((px[0] as u32 * k / 255) as u8);
                rgb.push((px[1] as u32 * k / 255) as u8);
                rgb.push((px[2] as u32 * k / 255) as u8);
            }
            DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(w, h, rgb)
                    .ok_or_else(|| anyhow!("cmyk buffer did not fit"))?,
            )
        }
    };

    Ok(img)
}

/// Below this reduction factor, DCT-scaled decoding is not worth it.
///
/// `jpeg-decoder` is the only crate exposing scaled decoding, and it is roughly
/// three times slower *per pixel* than the zune backend `image` uses. At 1/2
/// scale it therefore decodes half the pixels at three times the cost and comes
/// out behind a plain full decode; at 1/4 and 1/8 the pixel reduction wins
/// easily. Measured on a 6000×4000 frame: full decode 86 ms, 1/2 scaled 243 ms,
/// 1/8 scaled 51 ms. See the `decode_budget_on_a_24mp_frame` test.
const DCT_WORTH_IT_AT: u32 = 4;

/// Decode a JPEG by whichever route is actually cheaper for this target size.
fn decode_jpeg_for(path: &Path, max_dim: u32) -> Result<DynamicImage> {
    let reduction = read_dimensions(path)
        .map(|(w, h)| w.max(h) / max_dim.max(1))
        .unwrap_or(0);

    if reduction >= DCT_WORTH_IT_AT {
        decode_jpeg_scaled(path, max_dim)
    } else {
        decode_any(path)
    }
}

fn decode_any(path: &Path) -> Result<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("identify {}", path.display()))?;
    Ok(reader.decode()?)
}

/// Fit `img` inside `max_w` x `max_h` without enlarging it, and hand back RGBA.
///
/// Uses `fast_image_resize` rather than `image::DynamicImage::resize`. The
/// latter costs roughly 210 ms downscaling a 24 MP frame to 2048 px — more than
/// twice the decode it follows, and the single biggest cost in showing a photo.
///
/// The resize runs *before* the RGBA conversion where it can, so the widening
/// pass touches 2.8 M pixels instead of 24 M.
fn fit_within(img: DynamicImage, max_w: u32, max_h: u32) -> Result<RgbaImage> {
    use fast_image_resize::images::Image as FirImage;
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Ok(RgbaImage::new(0, 0));
    }
    if w <= max_w && h <= max_h {
        return Ok(img.to_rgba8());
    }

    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    let dst_w = ((w as f32 * scale).round() as u32).max(1);
    let dst_h = ((h as f32 * scale).round() as u32).max(1);

    let mut resizer = Resizer::new();
    // Lanczos would be sharper, but at a 3x reduction the difference is not
    // visible on a culling preview and it costs noticeably more.
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear));

    // Photographs are the overwhelmingly common case and carry no alpha, so
    // staying in three channels avoids widening 24 M pixels for nothing.
    if let DynamicImage::ImageRgb8(rgb) = img {
        let src = FirImage::from_vec_u8(w, h, rgb.into_raw(), PixelType::U8x3)
            .map_err(|e| anyhow!("wrap source for resize: {e}"))?;
        let mut dst = FirImage::new(dst_w, dst_h, PixelType::U8x3);
        resizer
            .resize(&src, &mut dst, &options)
            .map_err(|e| anyhow!("resize: {e}"))?;
        let rgb = image::RgbImage::from_raw(dst_w, dst_h, dst.into_vec())
            .ok_or_else(|| anyhow!("resized rgb buffer did not fit"))?;
        return Ok(DynamicImage::ImageRgb8(rgb).to_rgba8());
    }

    let rgba = img.to_rgba8();
    let src = FirImage::from_vec_u8(w, h, rgba.into_raw(), PixelType::U8x4)
        .map_err(|e| anyhow!("wrap source for resize: {e}"))?;
    let mut dst = FirImage::new(dst_w, dst_h, PixelType::U8x4);
    resizer
        .resize(&src, &mut dst, &options)
        .map_err(|e| anyhow!("resize: {e}"))?;
    RgbaImage::from_raw(dst_w, dst_h, dst.into_vec())
        .ok_or_else(|| anyhow!("resized rgba buffer did not fit"))
}

/// A thumbnail no larger than `max_w` x `max_h`, by the cheapest route that
/// works for this file.
pub fn decode_thumbnail(path: &Path, max_w: u32, max_h: u32) -> Result<RgbaImage> {
    let orientation = read_orientation(path);

    // 1 — the embedded JPEG, if it is big enough to be worth using.
    if let Some(thumb) = embedded_thumbnail(path) {
        if thumb.width() >= max_w || thumb.height() >= max_h {
            let img = apply_orientation(thumb, orientation);
            return fit_within(img, max_w, max_h);
        }
    }

    // 2 — DCT-scaled decode where that is cheaper, otherwise a plain one.
    let decoded = if is_jpeg(path) {
        decode_jpeg_for(path, max_w.max(max_h))?
    } else {
        // 3 — full decode; only non-JPEG formats reach here.
        decode_any(path)?
    };

    let img = apply_orientation(decoded, orientation);
    fit_within(img, max_w, max_h)
}

/// A preview sized for display rather than for the sensor. Decoding a 24 MP
/// frame to a 2560 px canvas wastes roughly 90% of the work.
pub fn decode_preview(path: &Path, max_dim: u32) -> Result<RgbaImage> {
    let orientation = read_orientation(path);
    let decoded = if is_jpeg(path) {
        decode_jpeg_for(path, max_dim)?
    } else {
        decode_any(path)?
    };
    let img = apply_orientation(decoded, orientation);
    fit_within(img, max_dim, max_dim)
}

/// Full resolution, orientation applied. Used only on the write path.
pub fn decode_full(path: &Path) -> Result<RgbaImage> {
    let orientation = read_orientation(path);
    let img = apply_orientation(decode_any(path)?, orientation);
    Ok(img.to_rgba8())
}

/// Dimensions without decoding pixels.
pub fn read_dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Pick the output format from the destination extension.
///
/// The MAUI build always encoded PNG but named the file with the *original*
/// extension, so a picked JPEG was written as PNG bytes in a `.jpg` file.
/// Encoding to match the extension is the fix.
fn format_for(path: &Path) -> ImageFormat {
    ImageFormat::from_path(path).unwrap_or(ImageFormat::Jpeg)
}

/// Encode to an in-memory buffer in the format the extension names.
pub fn encode(img: &RgbaImage, path: &Path, jpeg_quality: u8) -> Result<Vec<u8>> {
    let format = format_for(path);
    let mut out = Cursor::new(Vec::new());

    match format {
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel.
            let rgb = DynamicImage::ImageRgba8(img.clone()).to_rgb8();
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality);
            enc.encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        other => {
            DynamicImage::ImageRgba8(img.clone()).write_to(&mut out, other)?;
        }
    }

    Ok(out.into_inner())
}

// ---------------------------------------------------------------------------
// EXIF passthrough
// ---------------------------------------------------------------------------

/// Find the whole `APP1 / Exif` segment in a JPEG, marker and length included.
fn find_app1(jpeg: &[u8]) -> Option<&[u8]> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return None;
        }
        let marker = jpeg[i + 1];
        // Standalone markers carry no length.
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        // Start of scan — no metadata past here.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if len < 2 || i + 2 + len > jpeg.len() {
            return None;
        }
        if marker == 0xE1 {
            let payload = &jpeg[i + 4..i + 2 + len];
            if payload.starts_with(b"Exif\0\0") {
                return Some(&jpeg[i..i + 2 + len]);
            }
        }
        i += 2 + len;
    }
    None
}

/// Splice an APP1 segment in immediately after SOI.
fn insert_app1(jpeg: &[u8], app1: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(jpeg.len() + app1.len());
    out.extend_from_slice(&jpeg[..2]); // SOI
    out.extend_from_slice(app1);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Copy the original's EXIF block into freshly-encoded JPEG bytes.
///
/// The MAUI build's `PreserveImageMetadataAsync` only copied filesystem
/// timestamps, so date taken, camera, lens and GPS were all lost on save —
/// which is exactly what Google Photos reads. This carries the real block over.
///
/// Only meaningful when both sides are JPEG; other formats pass through
/// untouched.
pub fn carry_exif(encoded: Vec<u8>, original: &Path, out_path: &Path) -> Vec<u8> {
    if format_for(out_path) != ImageFormat::Jpeg || !is_jpeg(original) {
        return encoded;
    }
    if find_app1(&encoded).is_some() {
        return encoded;
    }

    let mut src = Vec::new();
    if File::open(original)
        .and_then(|mut f| f.read_to_end(&mut src))
        .is_err()
    {
        return encoded;
    }

    match find_app1(&src) {
        Some(app1) => insert_app1(&encoded, app1),
        None => encoded,
    }
}

/// Copy filesystem timestamps so file managers sort the copy with its original.
pub fn carry_file_times(original: &Path, out_path: &Path) -> Result<()> {
    let meta = std::fs::metadata(original)?;
    let modified = meta.modified()?;
    let file = File::options().write(true).open(out_path)?;
    file.set_modified(modified)
        .map_err(|e| anyhow!("set modified time: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app1_is_found_and_reinserted() {
        // SOI, APP1(Exif) with a 2-byte payload, then EOI.
        let app1: Vec<u8> = {
            let mut v = vec![0xFF, 0xE1];
            let payload = b"Exif\0\0ab";
            v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            v.extend_from_slice(payload);
            v
        };
        let mut src = vec![0xFF, 0xD8];
        src.extend_from_slice(&app1);
        src.extend_from_slice(&[0xFF, 0xD9]);

        assert_eq!(find_app1(&src), Some(app1.as_slice()));

        let bare = vec![0xFF, 0xD8, 0xFF, 0xD9];
        assert!(find_app1(&bare).is_none());

        let merged = insert_app1(&bare, &app1);
        assert_eq!(find_app1(&merged), Some(app1.as_slice()));
        // SOI stays first — a JPEG that does not start FFD8 is not a JPEG.
        assert_eq!(&merged[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn truncated_segment_does_not_panic() {
        // Declares a length that runs past the end of the buffer.
        let src = vec![0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFE, 0x00];
        assert!(find_app1(&src).is_none());
    }

    #[test]
    fn format_follows_the_extension_not_the_source() {
        assert_eq!(format_for(Path::new("a/b_WBV.jpg")), ImageFormat::Jpeg);
        assert_eq!(format_for(Path::new("a/b_WBV.png")), ImageFormat::Png);
    }

    #[test]
    fn encoded_jpeg_round_trips() {
        let img = RgbaImage::from_pixel(8, 8, image::Rgba([200, 100, 50, 255]));
        let bytes = encode(&img, Path::new("x.jpg"), 90).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!(back.width(), 8);
    }
}
