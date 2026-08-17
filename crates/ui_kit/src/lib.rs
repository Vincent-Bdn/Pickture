//! Shared design system: tokens, painting primitives, the mark, and the theming
//! applied to stock egui widgets.
//!
//! The agreed scope is hybrid — the six signature surfaces are custom-painted
//! by the slices, and ordinary controls are stock widgets restyled through
//! [`apply_style`]. This crate is what makes both halves agree.

pub mod fonts;
pub mod mark;
pub mod paint;
pub mod textures;
pub mod tokens;

pub use textures::{Texture, TextureStore};
pub use tokens::{metric, motion, size, Mode, Theme, DARK, LIGHT};

use egui::{Context, CornerRadius, Margin, Stroke};

/// Push the token set into egui's own style, so themed stock widgets — buttons,
/// text fields, scrollbars, tooltips, menus — sit in the same system as the
/// custom-painted surfaces.
pub fn apply_style(ctx: &Context, theme: &Theme) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    v.dark_mode = theme.mode == Mode::Dark;
    v.override_text_color = Some(theme.fg);
    v.panel_fill = theme.window;
    v.window_fill = theme.chrome;
    v.extreme_bg_color = theme.chrome;
    v.faint_bg_color = theme.rail;
    v.window_stroke = Stroke::new(metric::HAIR, theme.hair);

    // Radius 0 everywhere; the token set permits 2 pt on stock widgets only,
    // and nothing so far has needed it.
    let sq = CornerRadius::ZERO;
    v.window_corner_radius = sq;
    v.menu_corner_radius = sq;

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = sq;
        w.bg_stroke = Stroke::new(metric::HAIR, theme.hair);
        w.fg_stroke = Stroke::new(metric::HAIR, theme.fg);
        w.expansion = 0.0;
    }

    v.widgets.noninteractive.bg_fill = theme.chrome;
    v.widgets.noninteractive.weak_bg_fill = theme.chrome;
    v.widgets.noninteractive.fg_stroke = Stroke::new(metric::HAIR, theme.fg_secondary);

    v.widgets.inactive.bg_fill = theme.chrome;
    v.widgets.inactive.weak_bg_fill = theme.chrome;
    v.widgets.inactive.fg_stroke = Stroke::new(metric::HAIR, theme.fg_secondary);

    // Hover is a colour step only — no displacement, per the interaction spec.
    v.widgets.hovered.bg_fill = theme.chrome_hover;
    v.widgets.hovered.weak_bg_fill = theme.chrome_hover;
    v.widgets.hovered.fg_stroke = Stroke::new(metric::HAIR, theme.fg);

    v.widgets.active.bg_fill = theme.chrome_hover;
    v.widgets.active.weak_bg_fill = theme.chrome_hover;
    v.widgets.active.fg_stroke = Stroke::new(metric::HAIR, theme.fg);

    v.selection.bg_fill = theme.sodium.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(metric::HAIR, theme.sodium);

    // The focus ring is the same treatment as the current-cell outline: 1 pt
    // sodium. Keyboard is the primary input, so focus must always be visible.
    v.widgets.hovered.bg_stroke = Stroke::new(metric::HAIR, theme.hair);
    v.widgets.active.bg_stroke = Stroke::new(metric::HAIR, theme.sodium);

    style.spacing.item_spacing = egui::vec2(metric::S8, metric::S8);
    style.spacing.button_padding = egui::vec2(metric::S12, metric::S6);
    style.spacing.window_margin = Margin::same(0);
    style.spacing.menu_margin = Margin::same(0);
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = false;

    style.text_styles = [
        (egui::TextStyle::Body, tokens::sans(size::SANS_S)),
        (egui::TextStyle::Button, tokens::sans_medium(size::SANS_S)),
        (egui::TextStyle::Small, tokens::mono(size::MONO_XS)),
        (egui::TextStyle::Monospace, tokens::mono(size::MONO_S)),
        (
            egui::TextStyle::Heading,
            tokens::sans_semibold(size::SANS_L),
        ),
    ]
    .into();

    ctx.set_style(style);
}
