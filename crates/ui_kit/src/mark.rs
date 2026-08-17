//! The Pickture mark.
//!
//! Three rectangles on a 32×32 grid: two bars forming the pile, and one frame
//! lifted clear of it. The name is *pick* + *picture*, and the brief's central
//! note was that every previous version spent itself on "picture" — a stack of
//! photos, a hand, a magic wand — while the product is the act of choosing. The
//! lifted frame is that act.
//!
//! It reads at 16 px because it is three whole-pixel rectangles and nothing
//! else. No camera hardware, no photograph, no sparkles.

use egui::{Color32, Painter, Pos2, Rect, Vec2};

use crate::paint;
use crate::tokens::Theme;

/// Rect on the 32-unit grid: (x, y, w, h).
const PILE_UPPER: (f32, f32, f32, f32) = (4.0, 18.0, 24.0, 3.0);
const PILE_LOWER: (f32, f32, f32, f32) = (4.0, 24.0, 24.0, 3.0);
const LIFTED: (f32, f32, f32, f32) = (4.0, 5.0, 18.0, 9.0);

const GRID: f32 = 32.0;

fn unit_rect(origin: Pos2, scale: f32, r: (f32, f32, f32, f32)) -> Rect {
    Rect::from_min_size(
        Pos2::new(origin.x + r.0 * scale, origin.y + r.1 * scale),
        Vec2::new(r.2 * scale, r.3 * scale),
    )
}

/// Draw the two-tone mark, fitted into `rect` as a square.
pub fn draw(painter: &Painter, theme: &Theme, rect: Rect) {
    draw_with(painter, rect, theme.fg_muted, theme.sodium);
}

/// Single-colour variant — used where sodium would be the wrong tone, and the
/// version that must work before any colour version is considered final.
pub fn draw_mono(painter: &Painter, rect: Rect, color: Color32) {
    draw_with(painter, rect, color, color);
}

/// Rasterise the mark as RGBA for the window and taskbar icon.
///
/// Generated from the same three rectangles that paint the in-app mark, so the
/// icon cannot drift from the interface. Rectangles are snapped to whole pixels
/// with a one-pixel floor, which is what lets it stay legible at 16 px — the
/// size it is seen at most often.
pub fn rgba_icon(size: u32, ground: [u8; 4], pile: [u8; 4], lifted: [u8; 4]) -> Vec<u8> {
    let size = size.max(8);
    let mut buf = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        buf.extend_from_slice(&ground);
    }

    let scale = size as f32 / GRID;
    let mut fill = |r: (f32, f32, f32, f32), colour: [u8; 4]| {
        let x0 = (r.0 * scale).round() as i64;
        let y0 = (r.1 * scale).round() as i64;
        // A bar that rounds away to nothing is worse than one pixel too thick.
        let w = ((r.2 * scale).round() as i64).max(1);
        let h = ((r.3 * scale).round() as i64).max(1);
        for y in y0..(y0 + h) {
            for x in x0..(x0 + w) {
                if x < 0 || y < 0 || x >= size as i64 || y >= size as i64 {
                    continue;
                }
                let i = ((y as u32 * size + x as u32) * 4) as usize;
                buf[i..i + 4].copy_from_slice(&colour);
            }
        }
    };

    fill(PILE_UPPER, pile);
    fill(PILE_LOWER, pile);
    fill(LIFTED, lifted);
    buf
}

/// The icon in the application's own colours, on the window ground.
pub fn app_icon(size: u32) -> Vec<u8> {
    rgba_icon(
        size,
        [0x17, 0x18, 0x1A, 0xFF],
        [0x8C, 0x8A, 0x86, 0xFF],
        [0xC9, 0x8A, 0x2E, 0xFF],
    )
}

fn draw_with(painter: &Painter, rect: Rect, pile: Color32, lifted: Color32) {
    let side = rect.width().min(rect.height());
    let scale = side / GRID;
    let origin = Pos2::new(rect.center().x - side * 0.5, rect.center().y - side * 0.5);

    paint::fill(painter, unit_rect(origin, scale, PILE_UPPER), pile);
    paint::fill(painter, unit_rect(origin, scale, PILE_LOWER), pile);
    paint::fill(painter, unit_rect(origin, scale, LIFTED), lifted);
}
