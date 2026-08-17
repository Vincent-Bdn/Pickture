//! Painting primitives shared by the custom-drawn surfaces.
//!
//! egui has no CSS, so the things a stylesheet would have given us — tracked
//! uppercase labels, soft shadows, a notch clipped out of a corner — are drawn
//! here once and reused, rather than being reinvented per call site.

use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2,
};

use crate::tokens::{metric, Theme};

// ---------------------------------------------------------------------------
// Rectangles and rules
// ---------------------------------------------------------------------------

/// Every fill in this design has square corners.
pub fn fill(painter: &Painter, rect: Rect, color: Color32) {
    painter.rect_filled(rect, CornerRadius::ZERO, color);
}

/// A 1 pt border drawn *inside* the rect, so the box keeps its stated size.
pub fn border(painter: &Painter, rect: Rect, color: Color32) {
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(metric::HAIR, color),
        StrokeKind::Inside,
    );
}

/// Focus and current-cell treatment: 1 pt outline at 2 pt offset.
pub fn outline_offset(painter: &Painter, rect: Rect, color: Color32) {
    painter.rect_stroke(
        rect.expand(metric::OUTLINE_OFFSET),
        CornerRadius::ZERO,
        Stroke::new(metric::HAIR, color),
        StrokeKind::Outside,
    );
}

/// Horizontal hairline across the full width of `rect` at its bottom edge.
pub fn rule_bottom(painter: &Painter, rect: Rect, color: Color32) {
    let y = rect.bottom() - metric::HAIR * 0.5;
    painter.line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        Stroke::new(metric::HAIR, color),
    );
}

/// Horizontal hairline at the top edge.
pub fn rule_top(painter: &Painter, rect: Rect, color: Color32) {
    let y = rect.top() + metric::HAIR * 0.5;
    painter.line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        Stroke::new(metric::HAIR, color),
    );
}

/// Vertical hairline at the right edge.
pub fn rule_right(painter: &Painter, rect: Rect, color: Color32) {
    let x = rect.right() - metric::HAIR * 0.5;
    painter.line_segment(
        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
        Stroke::new(metric::HAIR, color),
    );
}

/// Vertical hairline at the left edge.
pub fn rule_left(painter: &Painter, rect: Rect, color: Color32) {
    let x = rect.left() + metric::HAIR * 0.5;
    painter.line_segment(
        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
        Stroke::new(metric::HAIR, color),
    );
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Paint text and return the rect it occupied.
pub fn text_left(painter: &Painter, pos: Pos2, s: &str, font: FontId, color: Color32) -> Rect {
    painter.text(pos, Align2::LEFT_CENTER, s, font, color)
}

pub fn text_right(painter: &Painter, pos: Pos2, s: &str, font: FontId, color: Color32) -> Rect {
    painter.text(pos, Align2::RIGHT_CENTER, s, font, color)
}

pub fn text_center(painter: &Painter, pos: Pos2, s: &str, font: FontId, color: Color32) -> Rect {
    painter.text(pos, Align2::CENTER_CENTER, s, font, color)
}

/// Width of a string in a given font, without painting it.
pub fn text_width(painter: &Painter, s: &str, font: &FontId) -> f32 {
    painter
        .ctx()
        .fonts(|f| s.chars().map(|c| f.glyph_width(font, c)).sum())
}

/// Uppercase label with positive tracking.
///
/// egui has no letter-spacing, so each glyph is placed individually and the
/// advance is padded. Only ever used on short eyebrow labels, where the cost is
/// irrelevant and the look is load-bearing.
pub fn tracked_text(
    painter: &Painter,
    pos: Pos2,
    s: &str,
    font: FontId,
    color: Color32,
    tracking: f32,
) -> f32 {
    let extra = font.size * tracking;
    let mut x = pos.x;
    for ch in s.chars() {
        let w = painter.ctx().fonts(|f| f.glyph_width(&font, ch));
        painter.text(
            Pos2::new(x, pos.y),
            Align2::LEFT_CENTER,
            ch,
            font.clone(),
            color,
        );
        x += w + extra;
    }
    x - pos.x
}

/// Width `tracked_text` will occupy.
pub fn tracked_width(painter: &Painter, s: &str, font: &FontId, tracking: f32) -> f32 {
    let extra = font.size * tracking;
    let glyphs: f32 = painter
        .ctx()
        .fonts(|f| s.chars().map(|c| f.glyph_width(font, c)).sum());
    glyphs + extra * s.chars().count() as f32
}

/// Truncate to fit `max_w`, appending an ellipsis. Used on paths and filenames,
/// which are frequently longer than the chrome that holds them.
pub fn elide(painter: &Painter, s: &str, font: &FontId, max_w: f32) -> String {
    if text_width(painter, s, font) <= max_w {
        return s.to_string();
    }
    let ell = "…";
    let ell_w = text_width(painter, ell, font);
    let mut out = String::new();
    let mut w = 0.0;
    for ch in s.chars() {
        let cw = painter.ctx().fonts(|f| f.glyph_width(font, ch));
        if w + cw + ell_w > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Truncate a path from the left, keeping the tail — `…\2026-08-14_harbour` is
/// far more useful than `D:\shoots\2026-0…`.
pub fn elide_start(painter: &Painter, s: &str, font: &FontId, max_w: f32) -> String {
    if text_width(painter, s, font) <= max_w {
        return s.to_string();
    }
    let ell_w = text_width(painter, "…", font);
    let mut tail: Vec<char> = Vec::new();
    let mut w = 0.0;
    for ch in s.chars().rev() {
        let cw = painter.ctx().fonts(|f| f.glyph_width(font, ch));
        if w + cw + ell_w > max_w {
            break;
        }
        tail.push(ch);
        w += cw;
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// The `↵` chip: the key that performs the action, drawn as a key.
///
/// Returns the width consumed so callers can lay out around it.
pub fn keycap(painter: &Painter, theme: &Theme, right_center: Pos2, label: &str) -> f32 {
    let font = crate::tokens::mono(crate::tokens::size::MONO_S);
    let w = text_width(painter, label, &font);
    let pad_x = 7.0;
    let pad_y = 3.0;
    let h = font.size + pad_y * 2.0;
    let rect = Rect::from_min_size(
        Pos2::new(right_center.x - (w + pad_x * 2.0), right_center.y - h * 0.5),
        Vec2::new(w + pad_x * 2.0, h),
    );
    fill(painter, rect, theme.sodium);
    text_center(painter, rect.center(), label, font, theme.on_sodium);
    rect.width()
}

/// Kept-frame notch: a filled triangle in the top-right corner of the thumbnail.
/// The design pairs it with the sodium rail so judgement survives peripheral
/// scanning and colour blindness — form as well as colour.
pub fn notch(painter: &Painter, thumb: Rect, size: f32, color: Color32, opacity: f32) {
    if opacity <= 0.001 {
        return;
    }
    let c = color.gamma_multiply(opacity);
    let tr = thumb.right_top();
    painter.add(Shape::convex_polygon(
        vec![
            tr,
            Pos2::new(tr.x - size, tr.y),
            Pos2::new(tr.x, tr.y + size),
        ],
        c,
        Stroke::NONE,
    ));
}

/// A close mark, drawn rather than typeset.
///
/// `✕` (U+2715) is absent from IBM Plex and from egui's fallback fonts, so
/// setting it as text produces tofu. Dismissing a dialog must not depend on
/// glyph coverage.
pub fn cross(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(metric::HAIR, color);
    let r = radius;
    painter.line_segment(
        [
            Pos2::new(center.x - r, center.y - r),
            Pos2::new(center.x + r, center.y + r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(center.x + r, center.y - r),
            Pos2::new(center.x - r, center.y + r),
        ],
        stroke,
    );
}

/// Soft drop shadow, built from concentric translucent rects.
///
/// egui does have a shadow primitive, but stacking rects keeps the falloff
/// under our control and costs nothing at these sizes.
pub fn soft_shadow(painter: &Painter, rect: Rect, offset: Vec2, blur: f32, color: Color32) {
    let steps = 6;
    for i in (1..=steps).rev() {
        let t = i as f32 / steps as f32;
        let grow = blur * t;
        let alpha = (color.a() as f32 / 255.0) * (1.0 - t) * 0.55;
        painter.rect_filled(
            rect.translate(offset).expand(grow),
            CornerRadius::ZERO,
            Color32::from_black_alpha((alpha * 255.0) as u8),
        );
    }
}

/// Popover chrome: shadow, ground, hairline border.
pub fn popover_frame(painter: &Painter, theme: &Theme, rect: Rect) {
    soft_shadow(
        painter,
        rect,
        Vec2::new(0.0, 8.0),
        28.0,
        Color32::from_black_alpha(128),
    );
    fill(painter, rect, theme.chrome);
    border(painter, rect, theme.hair);
}

/// Progress track: 3 pt hairline ground with a sodium fill.
pub fn progress(painter: &Painter, theme: &Theme, rect: Rect, fraction: f32) {
    fill(painter, rect, theme.hair);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut filled = rect;
        filled.set_width(rect.width() * f);
        fill(painter, filled, theme.sodium);
    }
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// Split a horizontal band off the top of `rect`, returning (band, remainder).
pub fn split_top(rect: Rect, height: f32) -> (Rect, Rect) {
    let h = height.min(rect.height());
    let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), h));
    let rest = Rect::from_min_max(Pos2::new(rect.left(), rect.top() + h), rect.max);
    (top, rest)
}

pub fn split_bottom(rect: Rect, height: f32) -> (Rect, Rect) {
    let h = height.min(rect.height());
    let bottom = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - h), rect.max);
    let rest = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.bottom() - h));
    (bottom, rest)
}

pub fn split_left(rect: Rect, width: f32) -> (Rect, Rect) {
    let w = width.min(rect.width());
    let left = Rect::from_min_size(rect.min, Vec2::new(w, rect.height()));
    let rest = Rect::from_min_max(Pos2::new(rect.left() + w, rect.top()), rect.max);
    (left, rest)
}

pub fn split_right(rect: Rect, width: f32) -> (Rect, Rect) {
    let w = width.min(rect.width());
    let right = Rect::from_min_max(Pos2::new(rect.right() - w, rect.top()), rect.max);
    let rest = Rect::from_min_max(rect.min, Pos2::new(rect.right() - w, rect.bottom()));
    (right, rest)
}

/// Fit `content` inside `bounds` preserving aspect, never enlarging beyond the
/// bounds. Used for both the canvas and every thumbnail.
pub fn fit_rect(bounds: Rect, content: Vec2) -> Rect {
    if content.x <= 0.0 || content.y <= 0.0 {
        return Rect::from_center_size(bounds.center(), Vec2::ZERO);
    }
    let scale = (bounds.width() / content.x).min(bounds.height() / content.y);
    let size = content * scale;
    Rect::from_center_size(bounds.center(), size)
}
