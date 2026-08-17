//! Font installation.
//!
//! Two families, three weights, both SIL OFL 1.1 — the licence permits
//! embedding and redistribution in a binary, which is the constraint that ruled
//! out most alternatives. Static files only; egui rasterises into a glyph atlas
//! and has no variable-axis support.
//!
//! Budget: four files. Adding a fifth means adding another atlas.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

use crate::tokens::family;

const SANS_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
const SANS_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf");
const SANS_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf");

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();

    // egui's built-in families carry a symbol/emoji fallback. Plex covers Latin
    // and punctuation but not the interface glyphs the design uses — `↵` on the
    // keep keycap, `⌫`, `↺ ↻`, `▾`, `▸`, `→`. Without a fallback chain each of
    // those renders as tofu, so every custom family keeps the defaults behind it.
    let fallback_sans: Vec<String> = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let fallback_mono: Vec<String> = defs
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    let mut add = |key: &str, bytes: &'static [u8], fallback: &[String]| {
        defs.font_data
            .insert(key.to_owned(), Arc::new(FontData::from_static(bytes)));
        let mut chain = vec![key.to_owned()];
        chain.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name(key.into()), chain);
    };

    add(family::SANS, SANS_REGULAR, &fallback_sans);
    add(family::SANS_MEDIUM, SANS_MEDIUM, &fallback_sans);
    add(family::SANS_SEMIBOLD, SANS_SEMIBOLD, &fallback_sans);
    add(family::MONO, MONO_REGULAR, &fallback_mono);

    // Point the built-in families at Plex too, so any stock widget we have not
    // explicitly styled still renders in the right typeface rather than
    // silently falling back.
    defs.families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, family::SANS.to_owned());
    defs.families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, family::MONO.to_owned());

    ctx.set_fonts(defs);
}
