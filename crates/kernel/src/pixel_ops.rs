//! The four image operations, ported from the MAUI/OpenCvSharp build.
//!
//! Two structural changes from the original:
//!
//! 1. **No per-pixel interop.** The C# version walked images with
//!    `Mat.At<byte>(y, x)` / `Set<float>(y, x)`, crossing the managed→native
//!    boundary roughly 48 million times for a 24 MP frame. Here every operation
//!    is a flat iteration over `&mut [u8]`, split across cores with rayon.
//!
//! 2. **No HSV round-trip.** OpenCV's 8-bit `BGR2HSV` quantises hue to 0..179
//!    and the round-trip bands smooth skies. "Adjust the value channel, preserve
//!    hue and saturation" is exactly `v = max(r,g,b)` followed by scaling all
//!    three channels by `new_v / v`, which needs no colour-space conversion and
//!    loses no precision.

use image::RgbaImage;
use rayon::prelude::*;

use crate::model::EffectSpec;

/// Rows handed to each rayon task. Large enough that scheduling overhead is
/// irrelevant, small enough that work stays balanced.
const CHUNK: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Lookup tables
// ---------------------------------------------------------------------------

/// A levels curve collapsed to 256 entries.
///
/// Gamma is constant for a given handle position, so the whole curve is 256
/// `powf` calls rather than one per pixel. This is what makes dragging a
/// histogram handle interactive at full frame size.
pub fn levels_lut(low: u8, high: u8, gamma: f32) -> [u8; 256] {
    let mut lut = [0u8; 256];
    let low_f = low as f32;
    let span = (high as f32 - low_f).max(1.0);
    let inv_gamma = 1.0 / gamma.max(0.01);
    for (i, out) in lut.iter_mut().enumerate() {
        let n = ((i as f32 - low_f) / span).clamp(0.0, 1.0);
        *out = (n.powf(inv_gamma) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

/// Linear stretch of `[low, high]` onto `[0, 255]`, no gamma.
fn stretch_lut(low: u8, high: u8) -> [u8; 256] {
    let mut lut = [0u8; 256];
    if high <= low {
        for (i, o) in lut.iter_mut().enumerate() {
            *o = i as u8;
        }
        return lut;
    }
    let scale = 255.0 / (high as f32 - low as f32);
    for (i, o) in lut.iter_mut().enumerate() {
        *o = (((i as f32 - low as f32) * scale).round()).clamp(0.0, 255.0) as u8;
    }
    lut
}

// ---------------------------------------------------------------------------
// Histograms
// ---------------------------------------------------------------------------

/// Rec. 709 luminance histogram — what the levels panel draws.
pub fn histogram_luma(img: &RgbaImage) -> [u32; 256] {
    img.as_raw()
        .par_chunks(CHUNK * 4)
        .map(|chunk| {
            let mut local = [0u32; 256];
            for px in chunk.chunks_exact(4) {
                let l = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32)
                    .round() as usize;
                local[l.min(255)] += 1;
            }
            local
        })
        .reduce(
            || [0u32; 256],
            |mut a, b| {
                for i in 0..256 {
                    a[i] += b[i];
                }
                a
            },
        )
}

/// Value-channel histogram, where value is `max(r, g, b)`.
fn histogram_value(img: &RgbaImage) -> [u32; 256] {
    img.as_raw()
        .par_chunks(CHUNK * 4)
        .map(|chunk| {
            let mut local = [0u32; 256];
            for px in chunk.chunks_exact(4) {
                local[px[0].max(px[1]).max(px[2]) as usize] += 1;
            }
            local
        })
        .reduce(
            || [0u32; 256],
            |mut a, b| {
                for i in 0..256 {
                    a[i] += b[i];
                }
                a
            },
        )
}

fn histogram_channel(img: &RgbaImage, c: usize) -> [u32; 256] {
    img.as_raw()
        .par_chunks(CHUNK * 4)
        .map(|chunk| {
            let mut local = [0u32; 256];
            for px in chunk.chunks_exact(4) {
                local[px[c] as usize] += 1;
            }
            local
        })
        .reduce(
            || [0u32; 256],
            |mut a, b| {
                for i in 0..256 {
                    a[i] += b[i];
                }
                a
            },
        )
}

/// Lowest and highest non-empty bin — the auto black/white points the original
/// `ApplyLevelsToChannel` derived by scanning every pixel twice.
fn nonzero_bounds(hist: &[u32; 256]) -> (u8, u8) {
    let low = hist
        .iter()
        .skip(1)
        .position(|&c| c > 0)
        .map(|i| i as u8 + 1)
        .unwrap_or(0);
    let high = (0..256).rev().find(|&i| hist[i] > 0).unwrap_or(255) as u8;
    (low, high)
}

/// Bounds after discarding `discard` percent of pixels from each tail.
fn discard_bounds(hist: &[u32; 256], total: u64, discard: f64) -> (u8, u8) {
    let threshold = (total as f64 * (discard / 100.0)).max(1.0);

    let mut acc = 0u64;
    let mut low = 0u8;
    for (i, &c) in hist.iter().enumerate() {
        acc += c as u64;
        if acc as f64 >= threshold {
            low = i as u8;
            break;
        }
    }

    let mut acc = 0u64;
    let mut high = 255u8;
    for i in (0..256).rev() {
        acc += hist[i] as u64;
        if acc as f64 >= threshold {
            high = i as u8;
            break;
        }
    }

    (low, high)
}

// ---------------------------------------------------------------------------
// Value-channel application
// ---------------------------------------------------------------------------

/// Apply a lookup table to the value channel, preserving hue and saturation.
///
/// `v = max(r, g, b)`; the whole pixel is scaled by `lut[v] / v`. Achromatic
/// pixels and pure black are handled by the `v == 0` guard.
fn apply_value_lut(img: &mut RgbaImage, lut: &[u8; 256]) {
    img.as_mut().par_chunks_mut(CHUNK * 4).for_each(|chunk| {
        for px in chunk.chunks_exact_mut(4) {
            let v = px[0].max(px[1]).max(px[2]);
            if v == 0 {
                continue;
            }
            let nv = lut[v as usize];
            if nv == v {
                continue;
            }
            let k = nv as f32 / v as f32;
            px[0] = ((px[0] as f32 * k) + 0.5).min(255.0) as u8;
            px[1] = ((px[1] as f32 * k) + 0.5).min(255.0) as u8;
            px[2] = ((px[2] as f32 * k) + 0.5).min(255.0) as u8;
        }
    });
}

// ---------------------------------------------------------------------------
// The four operations
// ---------------------------------------------------------------------------

/// White balance on the value channel only.
///
/// Auto black/white points from the value histogram, then a gamma of 1.15 —
/// the same constants the MAUI build used.
pub fn white_balance_value(img: &mut RgbaImage) {
    let hist = histogram_value(img);
    let (low, high) = nonzero_bounds(&hist);
    if high <= low {
        return;
    }
    let lut = levels_lut(low, high, 1.15);
    apply_value_lut(img, &lut);
}

/// Per-channel white balance: each of R, G and B stretched independently after
/// discarding outliers. This is the operation that shifts colour casts, and it
/// is why it is offered separately from the value-only version.
pub fn white_balance_rgb(img: &mut RgbaImage, discard: f64) {
    let total = (img.width() as u64) * (img.height() as u64);
    if total == 0 {
        return;
    }

    let luts: Vec<[u8; 256]> = (0..3)
        .map(|c| {
            let hist = histogram_channel(img, c);
            let (low, high) = discard_bounds(&hist, total, discard);
            stretch_lut(low, high)
        })
        .collect();

    img.as_mut().par_chunks_mut(CHUNK * 4).for_each(|chunk| {
        for px in chunk.chunks_exact_mut(4) {
            px[0] = luts[0][px[0] as usize];
            px[1] = luts[1][px[1] as usize];
            px[2] = luts[2][px[2] as usize];
        }
    });
}

/// Manual levels: black point, white point and gamma, applied to the value
/// channel. Driven live by the histogram handles.
pub fn levels_custom(img: &mut RgbaImage, low: u8, high: u8, gamma: f32) {
    let lut = levels_lut(low, high, gamma);
    apply_value_lut(img, &lut);
}

/// Run whichever colour operation the spec selects. Rotation is applied
/// separately because it changes the image dimensions.
pub fn apply_colour(img: &mut RgbaImage, spec: &EffectSpec) {
    use crate::model::EffectMode::*;
    match spec.mode {
        None => {}
        WbValue => white_balance_value(img),
        WbRgb => white_balance_rgb(img, 0.05),
        Levels => levels_custom(img, spec.low, spec.high, spec.gamma),
    }
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

/// Fill used for the triangles exposed by a fine rotation, matching the
/// original build's white border.
const FILL: [u8; 4] = [255, 255, 255, 255];

/// Exact quarter turns — a pure index remap, no interpolation and no quality
/// loss. `turns` is taken modulo 4.
pub fn rotate_quarters(img: &RgbaImage, turns: i32) -> RgbaImage {
    let t = turns.rem_euclid(4);
    if t == 0 {
        return img.clone();
    }
    let (w, h) = (img.width(), img.height());
    let (nw, nh) = if t % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = RgbaImage::new(nw, nh);
    for y in 0..h {
        for x in 0..w {
            let p = *img.get_pixel(x, y);
            let (dx, dy) = match t {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            out.put_pixel(dx, dy, p);
        }
    }
    out
}

/// Rotate by an arbitrary angle onto an expanded canvas, bilinear sampled.
pub fn rotate_free(img: &RgbaImage, degrees: f32) -> RgbaImage {
    if degrees == 0.0 {
        return img.clone();
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let rad = degrees.to_radians();
    let (sin, cos) = rad.sin_cos();
    let (abs_sin, abs_cos) = (sin.abs(), cos.abs());

    let nw = (w * abs_cos + h * abs_sin).round().max(1.0);
    let nh = (w * abs_sin + h * abs_cos).round().max(1.0);

    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ncx, ncy) = (nw / 2.0, nh / 2.0);

    let mut out = RgbaImage::new(nw as u32, nh as u32);
    let src_w = img.width();
    let src_h = img.height();

    out.as_mut()
        .par_chunks_mut(nw as usize * 4)
        .enumerate()
        .for_each(|(row, line)| {
            let dy = row as f32 - ncy + 0.5;
            for (col, px) in line.chunks_exact_mut(4).enumerate() {
                let dx = col as f32 - ncx + 0.5;
                // Inverse rotation back into source space.
                let sx = dx * cos + dy * sin + cx - 0.5;
                let sy = -dx * sin + dy * cos + cy - 0.5;
                px.copy_from_slice(&sample_bilinear(img, src_w, src_h, sx, sy));
            }
        });

    out
}

fn sample_bilinear(img: &RgbaImage, w: u32, h: u32, x: f32, y: f32) -> [u8; 4] {
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return FILL;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;

    let get = |ix: f32, iy: f32| -> [f32; 4] {
        if ix < 0.0 || iy < 0.0 || ix >= w as f32 || iy >= h as f32 {
            return [
                FILL[0] as f32,
                FILL[1] as f32,
                FILL[2] as f32,
                FILL[3] as f32,
            ];
        }
        let p = img.get_pixel(ix as u32, iy as u32).0;
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };

    let p00 = get(x0, y0);
    let p10 = get(x0 + 1.0, y0);
    let p01 = get(x0, y0 + 1.0);
    let p11 = get(x0 + 1.0, y0 + 1.0);

    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bot = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Crop a freely-rotated frame back to the largest centred rectangle that keeps
/// the original aspect ratio and contains no exposed fill.
pub fn crop_to_aspect(img: &RgbaImage, orig_w: u32, orig_h: u32, degrees: f32) -> RgbaImage {
    if orig_w == 0 || orig_h == 0 {
        return img.clone();
    }
    let rad = degrees.to_radians();
    let (sin, cos) = rad.sin_cos();
    let (abs_sin, abs_cos) = (sin.abs(), cos.abs());

    let r = orig_w as f32 / orig_h as f32;
    let c1 = abs_cos + (1.0 / r) * abs_sin;
    let c2 = r * abs_sin + abs_cos;
    let s = 1.0 / c1.max(c2);

    let cw = (s * orig_w as f32).round().max(1.0) as u32;
    let ch = (s * orig_h as f32).round().max(1.0) as u32;

    let x = img.width().saturating_sub(cw) / 2;
    let y = img.height().saturating_sub(ch) / 2;
    let cw = cw.min(img.width().saturating_sub(x)).max(1);
    let ch = ch.min(img.height().saturating_sub(y)).max(1);

    image::imageops::crop_imm(img, x, y, cw, ch).to_image()
}

/// Apply a spec's full rotation: quarter turns first (lossless), then the fine
/// angle with an aspect-preserving crop.
pub fn apply_rotation(img: &RgbaImage, spec: &EffectSpec) -> RgbaImage {
    let turned = rotate_quarters(img, spec.quarter_turns);
    if spec.angle == 0.0 {
        return turned;
    }
    let (w, h) = (turned.width(), turned.height());
    let rotated = rotate_free(&turned, spec.angle);
    crop_to_aspect(&rotated, w, h, spec.angle)
}

/// Full pipeline: colour then geometry, on an owned buffer.
pub fn apply_all(mut img: RgbaImage, spec: &EffectSpec) -> RgbaImage {
    apply_colour(&mut img, spec);
    apply_rotation(&img, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(px))
    }

    #[test]
    fn identity_lut_is_identity() {
        let lut = levels_lut(0, 255, 1.0);
        for (i, mapped) in lut.iter().enumerate() {
            assert_eq!(*mapped, i as u8, "at {i}");
        }
    }

    #[test]
    fn value_lut_preserves_hue_ratio() {
        // A mid red: raising the value must scale all channels together, so the
        // ratio between channels — and therefore the hue — is unchanged.
        let mut img = solid(4, 4, [120, 60, 30, 255]);
        let lut = levels_lut(0, 120, 1.0); // maps 120 -> 255
        apply_value_lut(&mut img, &lut);
        let p = img.get_pixel(0, 0).0;
        assert_eq!(p[3], 255);
        assert!((p[0] as f32 / p[1] as f32 - 2.0).abs() < 0.05);
        assert!((p[1] as f32 / p[2] as f32 - 2.0).abs() < 0.06);
    }

    #[test]
    fn quarter_turns_are_lossless_and_swap_axes() {
        let mut img = RgbaImage::new(3, 2);
        img.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        let r = rotate_quarters(&img, 1);
        assert_eq!((r.width(), r.height()), (2, 3));
        // Four quarter turns returns the original exactly.
        let full = rotate_quarters(&rotate_quarters(&rotate_quarters(&r, 1), 1), 1);
        assert_eq!(full.as_raw(), img.as_raw());
    }

    #[test]
    fn quarter_turns_wrap() {
        let img = solid(2, 3, [9, 9, 9, 255]);
        assert_eq!(rotate_quarters(&img, 4).dimensions(), (2, 3));
        assert_eq!(rotate_quarters(&img, -1).dimensions(), (3, 2));
    }

    #[test]
    fn levels_span_is_clamped_by_spec() {
        let mut s = EffectSpec::default();
        s.set_high(10);
        s.set_low(200);
        assert!(s.low <= s.high - 4, "low {} high {}", s.low, s.high);
    }

    #[test]
    fn histogram_totals_match_pixel_count() {
        let img = solid(8, 5, [10, 10, 10, 255]);
        let h = histogram_luma(&img);
        assert_eq!(h.iter().sum::<u32>(), 40);
    }

    #[test]
    fn crop_to_aspect_keeps_ratio() {
        let img = solid(200, 100, [0, 0, 0, 255]);
        let rot = rotate_free(&img, 5.0);
        let cropped = crop_to_aspect(&rot, 200, 100, 5.0);
        let ratio = cropped.width() as f32 / cropped.height() as f32;
        assert!((ratio - 2.0).abs() < 0.05, "ratio was {ratio}");
        assert!(cropped.width() < rot.width());
    }
}
