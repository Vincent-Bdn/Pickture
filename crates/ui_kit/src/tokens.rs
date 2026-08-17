//! Design tokens — direction 1a, "Darkroom instrument".
//!
//! Values are transcribed from the handoff and are not to be tuned by feel.
//! Two rules in particular are functional rather than decorative:
//!
//! * `surround` is achromatic (< 4% saturation) at mid luminance. The user is
//!   judging white balance against it, so a tinted surround makes the tool
//!   worse at its job.
//! * `sodium` appears only in outer chrome — the judgement rail, the notch, the
//!   keycap chip, the kept count, the gamma marker, progress fills. Never as a
//!   field adjacent to the canvas.

use egui::{Color32, FontFamily, FontId};

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub mode: Mode,

    // Surfaces
    pub window: Color32,
    pub rail: Color32,
    pub chrome: Color32,
    pub chrome_hover: Color32,
    pub surround: Color32,
    pub thumb_empty: Color32,
    pub hist_ground: Color32,
    pub hist_bar: Color32,

    // Line work
    pub hair: Color32,

    // Content
    pub fg: Color32,
    pub fg_secondary: Color32,
    pub fg_muted: Color32,
    pub fg_disabled: Color32,

    // The single accent
    pub sodium: Color32,
    /// Text drawn *on* sodium — the window ground in dark, near-white in light.
    pub on_sodium: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

pub const DARK: Theme = Theme {
    mode: Mode::Dark,
    window: rgb(0x17181A),
    rail: rgb(0x1B1D1F),
    chrome: rgb(0x1E2022),
    chrome_hover: rgb(0x232527),
    surround: rgb(0x4A4B4D),
    thumb_empty: rgb(0x26282A),
    hist_ground: rgb(0x202224),
    hist_bar: rgb(0x6E6B66),
    hair: rgb(0x2C2E31),
    fg: rgb(0xE7E5E1),
    fg_secondary: rgb(0xA9A6A1),
    fg_muted: rgb(0x8C8A86),
    fg_disabled: rgb(0x5C5A56),
    sodium: rgb(0xC98A2E),
    on_sodium: rgb(0x17181A),
};

pub const LIGHT: Theme = Theme {
    mode: Mode::Light,
    window: rgb(0xF2F2F0),
    rail: rgb(0xFBFBFA),
    chrome: rgb(0xFBFBFA),
    chrome_hover: rgb(0xEDEDEA),
    surround: rgb(0xB9BAB9),
    thumb_empty: rgb(0xE6E6E3),
    hist_ground: rgb(0xEDEDEA),
    hist_bar: rgb(0xA0A19E),
    hair: rgb(0xDFDFDC),
    fg: rgb(0x16171A),
    fg_secondary: rgb(0x43454A),
    fg_muted: rgb(0x6E7075),
    fg_disabled: rgb(0x8B8D8A),
    // Darkened from the dark-theme sodium to hold 4.5:1 on near-white.
    sodium: rgb(0x8A5A14),
    on_sodium: rgb(0xFBFBFA),
};

impl Theme {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => DARK,
            Mode::Light => LIGHT,
        }
    }

    /// Judgement-rail colour for the "current frame" state, which is the only
    /// place `fg` is used as a fill.
    pub fn rail_current(&self) -> Color32 {
        self.fg
    }
}

// ---------------------------------------------------------------------------
// Metrics — logical points
// ---------------------------------------------------------------------------

pub mod metric {
    /// Spacing scale. Nothing outside this list should appear as a gap.
    pub const S2: f32 = 2.0;
    pub const S4: f32 = 4.0;
    pub const S6: f32 = 6.0;
    pub const S8: f32 = 8.0;
    pub const S12: f32 = 12.0;
    pub const S16: f32 = 16.0;
    pub const S24: f32 = 24.0;

    /// Hairlines land on whole logical points so they stay 1 px at 100%, and
    /// round to 1 / 1 / 2 px at 125 / 150% with no half-pixel blur.
    pub const HAIR: f32 = 1.0;
    /// The judgement rail: 3 pt, becoming 4 and 5 px at 125 and 150%.
    pub const RAIL: f32 = 3.0;

    pub const TITLE_BAR: f32 = 38.0;
    pub const INFO_BAR: f32 = 46.0;
    pub const STATUS_BAR: f32 = 34.0;
    pub const MODAL_HEADER: f32 = 46.0;

    pub const FILMSTRIP_W: f32 = 214.0;
    pub const THUMB_W: f32 = 190.0;
    pub const CELL_GAP: f32 = 8.0;

    pub const PANEL_W: f32 = 356.0;
    pub const HISTOGRAM_H: f32 = 150.0;

    pub const SWITCHER_W: f32 = 496.0;
    pub const DEST_W: f32 = 400.0;

    pub const CANVAS_PAD: f32 = 26.0;
    pub const MODAL_CANVAS_PAD: f32 = 30.0;

    pub const NOTCH: f32 = 14.0;
    /// Focus and current-cell outline: 1 pt at 2 pt offset.
    pub const OUTLINE_OFFSET: f32 = 2.0;
}

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

pub mod motion {
    /// The keep acknowledgement. Satisfying at the first repetition, invisible
    /// by the fiftieth — so opacity only, no scale, no bounce, no sound.
    pub const ACK: f32 = 0.120;
    /// Hover, selection, state changes.
    pub const STATE: f32 = 0.090;
}

// ---------------------------------------------------------------------------
// Type
// ---------------------------------------------------------------------------

pub mod family {
    pub const SANS: &str = "plex-sans";
    pub const SANS_MEDIUM: &str = "plex-sans-medium";
    pub const SANS_SEMIBOLD: &str = "plex-sans-semibold";
    pub const MONO: &str = "plex-mono";
}

fn named(name: &'static str, size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(name.into()))
}

/// IBM Plex Sans 400.
pub fn sans(size: f32) -> FontId {
    named(family::SANS, size)
}
/// IBM Plex Sans 500 — buttons, filenames, headings.
pub fn sans_medium(size: f32) -> FontId {
    named(family::SANS_MEDIUM, size)
}
/// IBM Plex Sans 600 — the wordmark and screen titles only.
pub fn sans_semibold(size: f32) -> FontId {
    named(family::SANS_SEMIBOLD, size)
}
/// IBM Plex Mono 400 — every numeric, path, filename, EXIF value and key hint,
/// so digits stay tabular while a handle is being dragged.
pub fn mono(size: f32) -> FontId {
    named(family::MONO, size)
}

/// Sizes named rather than spelled, so a stray `13.0` stands out in review.
pub mod size {
    pub const SANS_XS: f32 = 11.0;
    pub const SANS_S: f32 = 12.0;
    pub const SANS_M: f32 = 14.0;
    pub const SANS_L: f32 = 18.0;
    pub const SANS_XL: f32 = 28.0;

    pub const MONO_XS: f32 = 10.0;
    pub const MONO_S: f32 = 11.0;
    pub const MONO_M: f32 = 12.0;
    /// Levels readouts and the angle value.
    pub const MONO_L: f32 = 14.0;
}

/// Tracking on the uppercase eyebrow labels, as a fraction of the font size.
pub const EYEBROW_TRACKING: f32 = 0.14;
