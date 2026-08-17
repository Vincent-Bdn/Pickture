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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EffectSpec {
    pub mode: EffectMode,
    pub low: u8,
    pub high: u8,
    pub gamma: f32,
    /// Whole 90° steps, applied before the fine angle.
    pub quarter_turns: i32,
    /// Fine rotation in degrees, ±10.
    pub angle: f32,
}

impl Default for EffectSpec {
    fn default() -> Self {
        Self {
            mode: EffectMode::None,
            low: 0,
            high: 255,
            gamma: 1.0,
            quarter_turns: 0,
            angle: 0.0,
        }
    }
}

impl EffectSpec {
    pub fn is_identity(&self) -> bool {
        self.mode == EffectMode::None && self.quarter_turns % 4 == 0 && self.angle == 0.0
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

    /// Total rotation applied to the displayed frame.
    pub fn total_rotation(&self) -> f32 {
        (self.quarter_turns as f32) * 90.0 + self.angle
    }

    /// The design calls out clipping when the levels handles have moved far
    /// enough to crush shadows or blow highlights.
    pub fn is_clipping(&self) -> bool {
        self.mode == EffectMode::Levels && (self.low > 26 || self.high < 232)
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
