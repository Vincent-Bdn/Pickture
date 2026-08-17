//! Pickture — a keyboard-driven photo-culling tool.
//!
//! A thousand frames down to forty, in one sitting. Keepers are copied to a
//! destination folder; originals are never touched.

// The window is undecorated and draws its own title bar, so a console window
// behind it would be visible on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;

use app::PicktureApp;

/// The taskbar and window icon.
///
/// Rasterised from the mark rather than loaded from a file, so the icon is
/// always the mark the application draws. Without this, winit falls back to a
/// generic default — which is where the stray "e" came from.
fn icon() -> egui::IconData {
    const SIZE: u32 = 64;
    egui::IconData {
        rgba: pickture_ui_kit::mark::app_icon(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pickture")
            .with_icon(icon())
            // Logical points, so this is what a 1366×768 laptop at 100% and a
            // 2880×1620 panel at 200% both have room for. A larger default
            // silently pushes the status bar off the bottom of the work area.
            .with_inner_size([1200.0, 700.0])
            .with_min_inner_size([940.0, 560.0])
            // Decorations off: the working path lives in our own title bar and
            // acts as the folder switcher.
            .with_decorations(false)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "Pickture",
        options,
        Box::new(|cc| Ok(Box::new(PicktureApp::new(cc)))),
    )
}
