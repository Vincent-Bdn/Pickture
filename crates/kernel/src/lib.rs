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

pub mod cache;
pub mod image_io;
pub mod jobs;
pub mod model;
pub mod pixel_ops;
pub mod session;

pub use model::{
    frame_id, is_supported, supported_label, today_stamp, Destination, EffectMode, EffectSpec,
    ExifSummary, Frame, FrameId, Judgement, ANGLE_LIMIT, GAMMA_MAX, GAMMA_MIN, LEVELS_MIN_SPAN,
    SUPPORTED_EXTENSIONS,
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
