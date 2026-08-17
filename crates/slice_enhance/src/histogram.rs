//! The histogram — the signature instrument of the application.
//!
//! The MAUI build drew a plain chart with three numeric text fields beneath it
//! (`Low`, `High`, `Gamma`) and an *Apply* button. Levels are a spatial idea:
//! the black point, the white point and the midtone bend are *positions on a
//! distribution*, and the right instrument is a handle on that distribution.
//!
//! The clamped regions are overlaid and terminated by a 1 pt edge, and **that
//! edge is the handle** — there is no separate grip to hunt for. Pointer-down
//! anywhere grabs whichever of the three is nearest.

use egui::{Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::{EffectMode, EffectSpec, GAMMA_MAX, GAMMA_MIN};
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::metric;
use pickture_ui_kit::tokens::Theme;

use crate::{EnhanceState, Handle};

/// Bars drawn across the 256 luminance bins.
const BARS: usize = 56;
/// Every bar keeps a floor so an empty bin still reads as a bin rather than a
/// gap in the chart.
const MIN_BAR: f32 = 0.10;

/// A 256-bin histogram reduced to the bar heights the panel draws.
#[derive(Clone)]
pub struct Histogram {
    pub bars: [f32; BARS],
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            bars: [MIN_BAR; BARS],
        }
    }
}

impl Histogram {
    /// Collapse 256 bins into `BARS`, normalised against the tallest.
    pub fn from_bins(bins: &[u32; 256]) -> Self {
        let mut bars = [0f32; BARS];
        let per = 256 / BARS;
        let remainder = 256 % BARS;

        let mut start = 0usize;
        for (i, bar) in bars.iter_mut().enumerate() {
            let width = per + usize::from(i < remainder);
            let end = (start + width).min(256);
            let sum: u64 = bins[start..end].iter().map(|v| *v as u64).sum();
            *bar = sum as f32 / width.max(1) as f32;
            start = end;
        }

        let max = bars.iter().cloned().fold(0.0f32, f32::max);
        if max > 0.0 {
            for b in bars.iter_mut() {
                *b = MIN_BAR + (*b / max) * (1.0 - MIN_BAR);
            }
        } else {
            bars = [MIN_BAR; BARS];
        }

        Self { bars }
    }
}

/// Map a gamma value onto its 0..255 position on the track.
fn gamma_to_x(gamma: f32) -> f32 {
    (gamma - GAMMA_MIN) / (GAMMA_MAX - GAMMA_MIN) * 255.0
}

fn x_to_gamma(x: f32) -> f32 {
    GAMMA_MIN + (x / 255.0) * (GAMMA_MAX - GAMMA_MIN)
}

/// Whichever handle is nearest the pointer, in 0..255 space.
fn nearest(spec: &EffectSpec, value: f32) -> Handle {
    let candidates = [
        (Handle::Black, (value - spec.low as f32).abs()),
        (Handle::White, (value - spec.high as f32).abs()),
        (Handle::Gamma, (value - gamma_to_x(spec.gamma)).abs()),
    ];
    candidates
        .iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| *h)
        .unwrap_or(Handle::Black)
}

/// Draw and drive the histogram. Returns true when the spec changed.
pub fn draw(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    hist: &Histogram,
    spec: &mut EffectSpec,
    state: &mut EnhanceState,
) -> bool {
    paint::fill(ui.painter(), rect, theme.hist_ground);

    // ---- bars ------------------------------------------------------------
    let inner = rect.shrink(metric::HAIR);
    let bar_w = inner.width() / BARS as f32;
    for (i, h) in hist.bars.iter().enumerate() {
        let height = inner.height() * h.clamp(0.0, 1.0);
        let bar = Rect::from_min_size(
            Pos2::new(inner.left() + i as f32 * bar_w, inner.bottom() - height),
            // 1 pt gap between bars.
            Vec2::new((bar_w - 1.0).max(1.0), height),
        );
        paint::fill(ui.painter(), bar, theme.hist_bar);
    }

    let value_to_x = |v: f32| inner.left() + (v / 255.0) * inner.width();

    // ---- clamped regions -------------------------------------------------
    // Overlaid with the window ground at 72%, terminated by a 1 pt edge — the
    // edge is the handle.
    let veil = theme.window.gamma_multiply(0.72);

    let low_x = value_to_x(spec.low as f32);
    if spec.low > 0 {
        paint::fill(
            ui.painter(),
            Rect::from_min_max(inner.left_top(), Pos2::new(low_x, inner.bottom())),
            veil,
        );
    }
    let high_x = value_to_x(spec.high as f32);
    if spec.high < 255 {
        paint::fill(
            ui.painter(),
            Rect::from_min_max(Pos2::new(high_x, inner.top()), inner.right_bottom()),
            veil,
        );
    }

    let edge = |x: f32| {
        paint::fill(
            ui.painter(),
            Rect::from_min_size(
                Pos2::new(x - metric::HAIR * 0.5, inner.top()),
                Vec2::new(metric::HAIR, inner.height()),
            ),
            theme.fg,
        );
    };
    edge(low_x);
    edge(high_x);

    // ---- gamma marker ----------------------------------------------------
    let gx = value_to_x(gamma_to_x(spec.gamma));
    paint::fill(
        ui.painter(),
        Rect::from_min_size(
            Pos2::new(gx - metric::HAIR * 0.5, inner.top()),
            Vec2::new(metric::HAIR, inner.height()),
        ),
        theme.sodium,
    );

    paint::border(ui.painter(), rect, theme.hair);

    // ---- interaction -----------------------------------------------------
    let response = ui.interact(rect, ui.id().with("histogram"), Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    let mut changed = false;

    if response.drag_started() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let v = ((pos.x - inner.left()) / inner.width().max(1.0) * 255.0).clamp(0.0, 255.0);
            state.drag = Some(nearest(spec, v));
        }
    }

    if response.dragged() {
        if let (Some(handle), Some(pos)) = (state.drag, response.interact_pointer_pos()) {
            let raw = ((pos.x - inner.left()) / inner.width().max(1.0) * 255.0).clamp(0.0, 255.0);
            // Shift halves the step by pulling the target back toward where the
            // handle already is.
            let v = if ui.input(|i| i.modifiers.shift) {
                let current = match handle {
                    Handle::Black => spec.low as f32,
                    Handle::White => spec.high as f32,
                    Handle::Gamma => gamma_to_x(spec.gamma),
                    Handle::Angle => raw,
                };
                current + (raw - current) * 0.5
            } else {
                raw
            };

            match handle {
                Handle::Black => spec.set_low(v.round() as u8),
                Handle::White => spec.set_high(v.round() as u8),
                Handle::Gamma => spec.set_gamma(x_to_gamma(v)),
                Handle::Angle => {}
            }
            changed = true;
        }
    }

    if response.drag_stopped() {
        state.drag = None;
    }

    // Arrow keys nudge the focused handle; Tab cycles black → gamma → white.
    if response.has_focus() || state.drag.is_some() {
        let handle = state.drag.unwrap_or(Handle::Black);
        let step = if ui.input(|i| i.modifiers.shift) {
            0.5
        } else {
            1.0
        };
        let mut delta = 0.0;
        ui.input(|i| {
            if i.key_pressed(egui::Key::ArrowLeft) {
                delta -= step;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                delta += step;
            }
        });
        if delta != 0.0 {
            match handle {
                Handle::Black => spec.set_low((spec.low as f32 + delta).clamp(0.0, 255.0) as u8),
                Handle::White => spec.set_high((spec.high as f32 + delta).clamp(0.0, 255.0) as u8),
                Handle::Gamma => {
                    spec.set_gamma(spec.gamma + delta * (GAMMA_MAX - GAMMA_MIN) / 255.0)
                }
                Handle::Angle => {}
            }
            changed = true;
        }
    }

    // Touching a handle means you are working on levels — say so rather than
    // silently editing a mode that is not selected.
    if changed && spec.mode != EffectMode::Levels {
        spec.mode = EffectMode::Levels;
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_normalised_with_a_floor() {
        let mut bins = [0u32; 256];
        bins[128] = 1000;
        let h = Histogram::from_bins(&bins);
        assert!(h.bars.iter().all(|b| *b >= MIN_BAR - 1e-6));
        assert!(h.bars.iter().cloned().fold(0.0f32, f32::max) <= 1.0 + 1e-6);
        // The populated bin must be the tallest.
        let max_index = h
            .bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!((24..=32).contains(&max_index), "peak landed at {max_index}");
    }

    #[test]
    fn empty_histogram_is_a_flat_floor() {
        let h = Histogram::from_bins(&[0u32; 256]);
        assert!(h.bars.iter().all(|b| (*b - MIN_BAR).abs() < 1e-6));
    }

    #[test]
    fn every_bin_is_counted_exactly_once() {
        // A uniform distribution must produce uniform bars, which only holds if
        // the 256 bins are partitioned without gaps or overlap.
        let bins = [7u32; 256];
        let h = Histogram::from_bins(&bins);
        for b in h.bars.iter() {
            assert!((*b - 1.0).abs() < 1e-5, "bar was {b}");
        }
    }

    #[test]
    fn gamma_maps_round_trip() {
        for g in [0.30, 1.0, 1.75, 2.50] {
            let x = gamma_to_x(g);
            assert!((x_to_gamma(x) - g).abs() < 1e-3, "gamma {g} -> {x}");
        }
    }

    #[test]
    fn nearest_handle_prefers_the_closest() {
        let spec = EffectSpec {
            low: 20,
            high: 240,
            gamma: 1.0,
            ..Default::default()
        };
        assert_eq!(nearest(&spec, 22.0), Handle::Black);
        assert_eq!(nearest(&spec, 238.0), Handle::White);
        assert_eq!(nearest(&spec, gamma_to_x(1.0)), Handle::Gamma);
    }
}
