//! Shared value types. No I/O, no UI — every slice speaks in these.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Stable identity for a frame within a session: the file name, which is unique
/// inside a non-recursive folder scan. Using the name rather than the full path
/// keeps sessions portable if a card dump is moved.
pub type FrameId = String;

pub fn frame_id(path: &Path) -> FrameId {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// File extensions Pickture will scan for.
///
/// This deliberately matches what the decoder can actually open. The design
/// comps mention raw formats (ARW/CR3/NEF/DNG); those need a raw pipeline that
/// does not exist yet, so advertising them here would be a lie.
pub const SUPPORTED_EXTENSIONS: &[&str] =
    &["jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "webp"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            SUPPORTED_EXTENSIONS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

/// Human-readable list for the empty-folder state.
pub fn supported_label() -> String {
    "JPEG, PNG, BMP, GIF, TIFF, WebP".to_string()
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Frame {
    pub path: PathBuf,
    pub id: FrameId,
    /// Name without extension — what the filmstrip label shows.
    pub stem: String,
    pub file_size: u64,
    pub modified: Option<std::time::SystemTime>,
    /// Filled in lazily once the frame has been decoded at least once.
    pub dimensions: Option<(u32, u32)>,
    pub exif: Option<ExifSummary>,
}

impl Frame {
    pub fn new(path: PathBuf, file_size: u64, modified: Option<std::time::SystemTime>) -> Self {
        let id = frame_id(&path);
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.clone());
        Self {
            path,
            id,
            stem,
            file_size,
            modified,
            dimensions: None,
            exif: None,
        }
    }
}

/// The four values the info bar shows. Anything we cannot read is simply absent
/// rather than rendered as a placeholder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifSummary {
    pub shutter: Option<String>,
    pub aperture: Option<String>,
    pub iso: Option<String>,
    pub focal: Option<String>,
}

impl ExifSummary {
    pub fn is_empty(&self) -> bool {
        self.shutter.is_none()
            && self.aperture.is_none()
            && self.iso.is_none()
            && self.focal.is_none()
    }

    /// "1/640 · f/2.8 · ISO 400 · 85mm"
    pub fn line(&self) -> String {
        let parts: Vec<&str> = [
            self.shutter.as_deref(),
            self.aperture.as_deref(),
            self.iso.as_deref(),
            self.focal.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        parts.join(" · ")
    }
}

// ---------------------------------------------------------------------------
// Judgement
// ---------------------------------------------------------------------------

/// Two-way judgement, with "absent from the map" as the third (unjudged) state.
/// The design specifies shipping two-way; a three-way reject is designed but
/// deliberately not built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Judgement {
    Kept,
    Passed,
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EffectMode {
    #[default]
    None,
    /// White balance applied to the value channel only — preserves hue and
    /// saturation, adjusts brightness.
    WbValue,
    /// Per-channel white balance: each RGB channel stretched independently.
    WbRgb,
    /// Manual levels: black point, white point, gamma.
    Levels,
}

impl EffectMode {
    /// Suffix appended to the file written into the destination, preserving the
    /// convention established by the MAUI build so existing `selection/`
    /// folders still light up as already-kept.
    pub fn suffix(self) -> &'static str {
        match self {
            EffectMode::None => "_ORIG",
            EffectMode::WbValue => "_WBV",
            EffectMode::WbRgb => "_WBRGB",
            EffectMode::Levels => "_CUSTOM",
        }
    }

    pub fn all_suffixes() -> &'static [&'static str] {
        &["_ORIG", "_WBV", "_WBRGB", "_CUSTOM"]
    }

    pub fn label(self) -> &'static str {
        match self {
            EffectMode::None => "None",
            EffectMode::WbValue => "WB · V",
            EffectMode::WbRgb => "WB · RGB",
            EffectMode::Levels => "Levels",
        }
    }
}

pub const GAMMA_MIN: f32 = 0.30;
pub const GAMMA_MAX: f32 = 2.50;
pub const ANGLE_LIMIT: f32 = 10.0;
/// `low <= high - LEVELS_MIN_SPAN`
pub const LEVELS_MIN_SPAN: u8 = 4;

/// One distribution's three handles: black point, white point, midtone bend.
///
/// The luminance curve keeps the flat `low` / `high` / `gamma` fields it has
/// always had on `EffectSpec`, so sessions written by an earlier build still
/// load. This is the shape the three colour curves take, and both are reached
/// through `EffectSpec::levels_of` — so the instrument never needs to know which
/// of the two it is driving.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub low: u8,
    pub high: u8,
    pub gamma: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            low: 0,
            high: 255,
            gamma: 1.0,
        }
    }
}

impl Levels {
    pub fn set_low(&mut self, v: u8) {
        self.low = v.min(self.high.saturating_sub(LEVELS_MIN_SPAN));
    }

    pub fn set_high(&mut self, v: u8) {
        self.high = v.max(self.low.saturating_add(LEVELS_MIN_SPAN));
    }

    pub fn set_gamma(&mut self, v: f32) {
        self.gamma = v.clamp(GAMMA_MIN, GAMMA_MAX);
    }

    pub fn is_identity(&self) -> bool {
        self.low == 0 && self.high == 255 && self.gamma == 1.0
    }

    /// Far enough for the clamp to crush shadows or blow highlights.
    pub fn is_clipping(&self) -> bool {
        self.low > 26 || self.high < 232
    }
}

/// Which distribution the levels handles are driving.
///
/// `Luma` is the default, and the only one an earlier build could produce: one
/// curve on the value channel, hue and saturation preserved. The three colour
/// channels are deliberately the opposite — each clamps on its own, which is
/// what makes a cast correctable by hand rather than only by the automatic
/// `WB · RGB` pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LevelsChannel {
    #[default]
    Luma,
    Red,
    Green,
    Blue,
}

impl LevelsChannel {
    pub const ALL: [LevelsChannel; 4] = [Self::Luma, Self::Red, Self::Green, Self::Blue];

    /// Label on the selector.
    pub fn label(self) -> &'static str {
        match self {
            LevelsChannel::Luma => "LUMA",
            LevelsChannel::Red => "R",
            LevelsChannel::Green => "G",
            LevelsChannel::Blue => "B",
        }
    }

    /// Full word for the eyebrow above the histogram.
    pub fn eyebrow(self) -> &'static str {
        match self {
            LevelsChannel::Luma => "LUMINANCE",
            LevelsChannel::Red => "RED",
            LevelsChannel::Green => "GREEN",
            LevelsChannel::Blue => "BLUE",
        }
    }

    /// Index into `EffectSpec::rgb`; `None` is the luminance curve.
    pub fn index(self) -> Option<usize> {
        match self {
            LevelsChannel::Luma => None,
            LevelsChannel::Red => Some(0),
            LevelsChannel::Green => Some(1),
            LevelsChannel::Blue => Some(2),
        }
    }
}

// ---------------------------------------------------------------------------
// Crop
// ---------------------------------------------------------------------------

/// The aspect a crop is held to.
///
/// `Original` is the default because it is the only one that cannot change what
/// the frame *is* — it keeps the shape the camera recorded, which is also what
/// the fine-rotation crop already preserves. `Custom` is last on the list for
/// the same reason it is last here: it is the escape hatch, not the norm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CropRatio {
    #[default]
    Original,
    Square,
    FiveFour,
    FourThree,
    ThreeTwo,
    SixteenNine,
    /// Unconstrained — every edge moves on its own.
    Custom,
}

impl CropRatio {
    pub const ALL: [CropRatio; 7] = [
        CropRatio::Original,
        CropRatio::Square,
        CropRatio::FiveFour,
        CropRatio::FourThree,
        CropRatio::ThreeTwo,
        CropRatio::SixteenNine,
        CropRatio::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CropRatio::Original => "Original",
            CropRatio::Square => "1:1",
            CropRatio::FiveFour => "5:4",
            CropRatio::FourThree => "4:3",
            CropRatio::ThreeTwo => "3:2",
            CropRatio::SixteenNine => "16:9",
            CropRatio::Custom => "Custom",
        }
    }

    /// Whether turning the ratio on its side means anything for it.
    pub fn is_turnable(self) -> bool {
        !matches!(self, CropRatio::Square | CropRatio::Custom)
    }

    /// The aspect (width / height, in pixels) this ratio holds a crop to on a
    /// frame whose own aspect is `frame_aspect`. `None` is unconstrained.
    ///
    /// The named ratios are stated landscape, so a portrait frame takes them
    /// turned — 4:3 on a portrait frame means 3:4, which is what anyone asking
    /// for it meant. `swap` turns them the other way again.
    pub fn aspect(self, frame_aspect: f32, swap: bool) -> Option<f32> {
        if !frame_aspect.is_finite() || frame_aspect <= 0.0 {
            return None;
        }
        let base = match self {
            CropRatio::Custom => return None,
            CropRatio::Original => {
                return Some(if swap {
                    1.0 / frame_aspect
                } else {
                    frame_aspect
                })
            }
            CropRatio::Square => return Some(1.0),
            CropRatio::FiveFour => 5.0 / 4.0,
            CropRatio::FourThree => 4.0 / 3.0,
            CropRatio::ThreeTwo => 3.0 / 2.0,
            CropRatio::SixteenNine => 16.0 / 9.0,
        };
        let portrait = frame_aspect < 1.0;
        Some(if portrait != swap { 1.0 / base } else { base })
    }
}

/// A crop, normalised 0..1 against the frame that survives rotation.
///
/// Normalised rather than in pixels so it survives the proxy: the 1600 px
/// preview the enhance page drags on and the full-resolution frame the writer
/// crops are then the same rectangle, with no scaling factor to get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self::FULL
    }
}

impl CropRect {
    pub const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    /// A crop never collapses to nothing, however hard a handle is dragged.
    pub const MIN: f32 = 0.05;

    pub fn is_full(&self) -> bool {
        self.x <= 1e-4 && self.y <= 1e-4 && self.w >= 1.0 - 1e-4 && self.h >= 1.0 - 1e-4
    }

    /// Inside the frame, and never smaller than `MIN` on either axis.
    pub fn clamped(mut self) -> Self {
        self.w = if self.w.is_finite() {
            self.w.clamp(Self::MIN, 1.0)
        } else {
            1.0
        };
        self.h = if self.h.is_finite() {
            self.h.clamp(Self::MIN, 1.0)
        } else {
            1.0
        };
        self.x = if self.x.is_finite() {
            self.x.clamp(0.0, 1.0 - self.w)
        } else {
            0.0
        };
        self.y = if self.y.is_finite() {
            self.y.clamp(0.0, 1.0 - self.h)
        } else {
            0.0
        };
        self
    }

    /// The largest centred rect of `aspect` that fits a frame of
    /// `frame_aspect`.
    pub fn centred(frame_aspect: f32, aspect: f32) -> Self {
        if !(frame_aspect.is_finite() && aspect.is_finite()) || frame_aspect <= 0.0 || aspect <= 0.0
        {
            return Self::FULL;
        }
        if aspect >= frame_aspect {
            let h = frame_aspect / aspect;
            Self {
                x: 0.0,
                y: (1.0 - h) * 0.5,
                w: 1.0,
                h,
            }
        } else {
            let w = aspect / frame_aspect;
            Self {
                x: (1.0 - w) * 0.5,
                y: 0.0,
                w,
                h: 1.0,
            }
        }
        .clamped()
    }

    /// Re-shape to `aspect` while holding the current centre and roughly the
    /// current size, so changing ratio does not throw away a crop already
    /// placed.
    pub fn conform(self, frame_aspect: f32, aspect: f32) -> Self {
        if !(frame_aspect.is_finite() && aspect.is_finite()) || frame_aspect <= 0.0 || aspect <= 0.0
        {
            return self.clamped();
        }
        let here = self.clamped();
        let (cx, cy) = (here.x + here.w * 0.5, here.y + here.h * 0.5);

        let mut w = here.w;
        let mut h = w * frame_aspect / aspect;
        // Both axes scale together, never clamp apart, or the shape asked for is
        // not the shape returned.
        let shrink = (1.0 / w).min(1.0 / h).min(1.0);
        w *= shrink;
        h *= shrink;
        let grow = (Self::MIN / w).max(Self::MIN / h).max(1.0);
        w = (w * grow).min(1.0);
        h = (h * grow).min(1.0);

        Self {
            x: cx - w * 0.5,
            y: cy - h * 0.5,
            w,
            h,
        }
        .clamped()
    }

    /// Carry the crop through `quarters` 90° turns of the frame.
    ///
    /// Rotation swaps the axes, so a rectangle left at the same normalised
    /// coordinates would land on a different part of the picture. This is the
    /// same index remap `pixel_ops::rotate_quarters` performs, expressed on a
    /// rect: one clockwise turn sends the top edge to the right-hand edge.
    pub fn turned(self, quarters: i32) -> Self {
        let mut r = self.clamped();
        for _ in 0..quarters.rem_euclid(4) {
            r = Self {
                x: 1.0 - (r.y + r.h),
                y: r.x,
                w: r.h,
                h: r.w,
            };
        }
        r.clamped()
    }

    /// Fraction of the frame's area this crop keeps.
    pub fn area_fraction(&self) -> f32 {
        (self.w * self.h).clamp(0.0, 1.0)
    }

    /// Pixel origin and size against a frame of `w` x `h`.
    pub fn pixels(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let c = self.clamped();
        let x = (c.x * w as f32).round().max(0.0) as u32;
        let y = (c.y * h as f32).round().max(0.0) as u32;
        let cw = ((c.w * w as f32).round() as u32).clamp(1, w.saturating_sub(x).max(1));
        let ch = ((c.h * h as f32).round() as u32).clamp(1, h.saturating_sub(y).max(1));
        (x, y, cw, ch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EffectSpec {
    pub mode: EffectMode,
    pub low: u8,
    pub high: u8,
    pub gamma: f32,
    /// The three colour curves, driven when `per_channel` is set. Held beside
    /// the luminance curve rather than replacing it, so toggling between them
    /// discards neither set of handles.
    #[serde(default)]
    pub rgb: [Levels; 3],
    #[serde(default)]
    pub per_channel: bool,
    /// Whole 90° steps, applied before the fine angle.
    pub quarter_turns: i32,
    /// Fine rotation in degrees, ±10.
    pub angle: f32,
    #[serde(default)]
    pub crop_ratio: CropRatio,
    /// Normalised against the frame left standing after rotation.
    #[serde(default)]
    pub crop: CropRect,
    /// The chosen ratio turned on its side.
    #[serde(default)]
    pub crop_swap: bool,
}

impl Default for EffectSpec {
    fn default() -> Self {
        Self {
            mode: EffectMode::None,
            low: 0,
            high: 255,
            gamma: 1.0,
            rgb: [Levels::default(); 3],
            per_channel: false,
            quarter_turns: 0,
            angle: 0.0,
            crop_ratio: CropRatio::Original,
            crop: CropRect::FULL,
            crop_swap: false,
        }
    }
}

impl EffectSpec {
    pub fn is_identity(&self) -> bool {
        self.mode == EffectMode::None
            && self.quarter_turns % 4 == 0
            && self.angle == 0.0
            && self.crop.is_full()
    }

    pub fn set_low(&mut self, v: u8) {
        self.low = v.min(self.high.saturating_sub(LEVELS_MIN_SPAN));
    }

    pub fn set_high(&mut self, v: u8) {
        self.high = v.max(self.low.saturating_add(LEVELS_MIN_SPAN));
    }

    pub fn set_gamma(&mut self, v: f32) {
        self.gamma = v.clamp(GAMMA_MIN, GAMMA_MAX);
    }

    pub fn set_angle(&mut self, v: f32) {
        self.angle = v.clamp(-ANGLE_LIMIT, ANGLE_LIMIT);
    }

    /// The handles for one channel, wherever they are stored.
    pub fn levels_of(&self, channel: LevelsChannel) -> Levels {
        match channel.index() {
            None => Levels {
                low: self.low,
                high: self.high,
                gamma: self.gamma,
            },
            Some(i) => self.rgb[i],
        }
    }

    pub fn set_levels(&mut self, channel: LevelsChannel, levels: Levels) {
        match channel.index() {
            None => {
                self.low = levels.low;
                self.high = levels.high;
                self.gamma = levels.gamma;
            }
            Some(i) => self.rgb[i] = levels,
        }
    }

    /// Edit one channel's handles in place — the only path the instrument uses,
    /// so the luminance and colour curves cannot drift apart in how they clamp.
    pub fn edit_levels(&mut self, channel: LevelsChannel, f: impl FnOnce(&mut Levels)) {
        let mut levels = self.levels_of(channel);
        f(&mut levels);
        self.set_levels(channel, levels);
    }

    /// Total rotation applied to the displayed frame.
    pub fn total_rotation(&self) -> f32 {
        (self.quarter_turns as f32) * 90.0 + self.angle
    }

    /// The design calls out clipping when the levels handles have moved far
    /// enough to crush shadows or blow highlights. In per-channel mode any one
    /// channel clipping counts, because any one of them is visible.
    pub fn is_clipping(&self) -> bool {
        if self.mode != EffectMode::Levels {
            return false;
        }
        if self.per_channel {
            self.rgb.iter().any(Levels::is_clipping)
        } else {
            self.levels_of(LevelsChannel::Luma).is_clipping()
        }
    }

    /// The aspect the crop is held to on a frame of `frame_aspect`.
    pub fn crop_aspect(&self, frame_aspect: f32) -> Option<f32> {
        self.crop_ratio.aspect(frame_aspect, self.crop_swap)
    }
}

// ---------------------------------------------------------------------------
// Destination
// ---------------------------------------------------------------------------

/// Where keepers are written. Copies only — originals are never touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Destination {
    /// A relative folder inside the working folder. Default is `selection`.
    InWorkingFolder(String),
    /// Any absolute path, including another drive.
    Absolute(PathBuf),
    /// `<working>/selection/<date>` — a second pass over an already-culled shoot.
    Dated { root: String, date: String },
}

impl Default for Destination {
    fn default() -> Self {
        Destination::InWorkingFolder("selection".to_string())
    }
}

impl Destination {
    pub fn resolve(&self, working: &Path) -> PathBuf {
        match self {
            Destination::InWorkingFolder(name) => working.join(name),
            Destination::Absolute(p) => p.clone(),
            Destination::Dated { root, date } => working.join(root).join(date),
        }
    }

    /// Short form for the info-bar chip.
    pub fn chip_label(&self) -> String {
        match self {
            Destination::InWorkingFolder(name) => format!("{name}/"),
            Destination::Absolute(p) => p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            Destination::Dated { root, date } => format!("{root}/{date}"),
        }
    }

    /// Full form for the destination popover rows.
    pub fn full_label(&self) -> String {
        match self {
            Destination::InWorkingFolder(name) => format!(".\\{name}\\"),
            Destination::Absolute(p) => format!("{}\\", p.display()),
            Destination::Dated { root, date } => format!(".\\{root}\\{date}\\"),
        }
    }

    pub fn note(&self) -> &'static str {
        match self {
            Destination::InWorkingFolder(_) => "Inside the working folder — the default convention",
            Destination::Absolute(_) => "An absolute folder, remembered per working folder",
            Destination::Dated { .. } => "A dated subfolder, so two passes never collide",
        }
    }
}

pub fn today_stamp() -> String {
    // Avoids pulling in `chrono` for one string. Days-since-epoch converted with
    // the civil-from-days algorithm (Howard Hinnant), valid well past 2100.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
