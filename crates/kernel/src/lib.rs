//! Pickture kernel — everything the slices share, and nothing about the UI.
//!
//! This crate has no `egui` dependency by design. The dependency rule the
//! workspace enforces is:
//!
//! ```text
//! app      -> every slice + ui_kit + kernel
//! slice_*  -> ui_kit + kernel          (never another slice)
//! ui_kit   -> kernel
//! kernel   -> nothing in this workspace
//! ```
//!
//! Cargo makes that structural rather than a convention: a slice physically
//! cannot reach another slice, because it is not in its manifest.

// Two lints here want APIs newer than the MSRV this workspace promises
// (`rust-version = "1.82"`): `slice::as_chunks` landed in 1.88 and
// `usize::is_multiple_of` in 1.87. Taking either suggestion would make the
// crate refuse to build on the toolchain the manifest advertises, so both are
// declined — and since CI runs clippy with `-D warnings`, declining them has to
// be said out loud rather than left as noise in the log.
#![allow(clippy::chunks_exact_to_as_chunks, clippy::manual_is_multiple_of)]

pub mod cache;
pub mod image_io;
pub mod jobs;
pub mod model;
pub mod pixel_ops;
pub mod session;

pub use model::{
    frame_id, is_supported, supported_label, today_stamp, CropRatio, CropRect, Destination,
    EffectMode, EffectSpec, ExifSummary, Frame, FrameId, Judgement, Levels, LevelsChannel,
    ANGLE_LIMIT, GAMMA_MAX, GAMMA_MIN, LEVELS_MIN_SPAN, SUPPORTED_EXTENSIONS,
};
pub use session::{scan_folder, PersistedSession, Session, SessionStore};

use std::path::PathBuf;

/// Where sessions and the thumbnail cache live.
pub mod paths {
    use super::PathBuf;

    fn project() -> Option<directories::ProjectDirs> {
        directories::ProjectDirs::from("", "", "Pickture")
    }

    /// `%APPDATA%/Pickture/sessions.json` on Windows.
    pub fn sessions_file() -> PathBuf {
        project()
            .map(|d| d.config_dir().join("sessions.json"))
            .unwrap_or_else(|| PathBuf::from("pickture-sessions.json"))
    }

    /// Thumbnails. Safe to delete at any time — it rebuilds on demand.
    pub fn thumbnail_cache_dir() -> Option<PathBuf> {
        project().map(|d| d.cache_dir().join("thumbnails"))
    }
}
