//! View: the image canvas.
//!
//! One rule governs this whole slice. The region immediately around the frame
//! is `surround` — achromatic, mid luminance, never black or white — because
//! the user is judging white balance against it. Chromatic adaptation would
//! shift their perception of the frame's colour, and surround luminance would
//! shift their perception of its contrast. Nothing else lives in this region.

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::CropRect;
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};
use pickture_ui_kit::Texture;

/// Geometry to apply to the displayed frame.
///
/// Which surface asks for it decides how much of it there is:
///
/// * **Culling** passes none. That canvas is a view of the file on disk, and no
///   operation may appear to have altered an original the tool never writes to.
///   What the copy will get is stated in the info bar instead.
/// * **Enhance** passes the fine angle, because that is where the result is
///   being decided and has to be seen.
///
/// Whatever is passed is drawn *with* its crop. Showing a rotated frame without
/// the aspect-preserving crop that follows it produced the worst of both — a
/// tilted quad with the surround visible through its corners, which is neither
/// the original nor the result.
#[derive(Clone, Copy, Default)]
pub struct Geometry {
    pub quarter_turns: i32,
    pub angle: f32,
    /// The user's crop, normalised against the frame the rotation leaves
    /// standing. `CropRect::FULL` — the default — draws the whole frame.
    pub crop: CropRect,
}

impl Geometry {
    pub fn total_rotation(&self) -> f32 {
        self.quarter_turns as f32 * 90.0 + self.angle
    }

    /// Fraction of the frame that survives the aspect-preserving crop.
    ///
    /// Mirrors `pixel_ops::crop_to_aspect`, which is what the write path
    /// actually applies — the two must not drift apart.
    pub fn crop_scale(&self, aspect: f32) -> f32 {
        if self.angle == 0.0 || !aspect.is_finite() || aspect <= 0.0 {
            return 1.0;
        }
        let rad = self.angle.to_radians();
        let (sin, cos) = (rad.sin().abs(), rad.cos().abs());
        1.0 / ((cos + sin / aspect).max(aspect * sin + cos))
    }
}

/// What the canvas should be showing.
pub enum CanvasContent<'a> {
    Image {
        texture: &'a Texture,
        geometry: Geometry,
    },
    Decoding,
    Unreadable {
        name: &'a str,
        reason: &'a str,
    },
    EmptyFolder {
        supported: &'a str,
    },
    NoFolder,
}

/// Draw the canvas.
///
/// `ack` is the keep acknowledgement: a 3 pt sodium inset border at the given
/// opacity. Opacity only — no scale, no bounce, no sound, so it stays
/// satisfying at the first repetition and invisible by the fiftieth.
///
/// Returns the rect the frame occupies on screen — the crop editor hangs its
/// handles on exactly that rect, so the two cannot disagree about where the
/// frame is.
pub fn canvas(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    content: CanvasContent<'_>,
    padding: f32,
    ack: f32,
    thirds: bool,
) -> Option<Rect> {
    paint::fill(ui.painter(), rect, theme.surround);
    let inner = rect.shrink(padding);
    let mut frame = None;

    match content {
        CanvasContent::Image { texture, geometry } => {
            frame = draw_image(ui, rect, inner, texture, geometry, thirds, theme);
        }
        CanvasContent::Decoding => {
            // Rare after the rewrite, so it is a caption rather than a spinner —
            // a spinner would advertise a wait that is not happening.
            paint::text_center(
                ui.painter(),
                rect.center(),
                "decoding",
                tokens::mono(size::MONO_S),
                theme.fg,
            );
        }
        CanvasContent::Unreadable { name, reason } => {
            paint::text_center(
                ui.painter(),
                rect.center() - Vec2::new(0.0, 9.0),
                name,
                tokens::mono(size::MONO_S),
                theme.fg,
            );
            paint::text_center(
                ui.painter(),
                rect.center() + Vec2::new(0.0, 9.0),
                reason,
                tokens::mono(size::MONO_XS),
                theme.fg_secondary,
            );
        }
        CanvasContent::EmptyFolder { supported } => {
            paint::text_center(
                ui.painter(),
                rect.center() - Vec2::new(0.0, 16.0),
                "No supported images here.",
                tokens::sans(size::SANS_M),
                theme.fg,
            );
            paint::text_center(
                ui.painter(),
                rect.center() + Vec2::new(0.0, 4.0),
                &format!("Pickture reads {supported}."),
                tokens::sans(size::SANS_XS),
                theme.fg_secondary,
            );
            paint::text_center(
                ui.painter(),
                rect.center() + Vec2::new(0.0, 26.0),
                "Choose another folder · O",
                tokens::mono(size::MONO_S),
                theme.sodium,
            );
        }
        CanvasContent::NoFolder => {}
    }

    if ack > 0.001 {
        // Inset so the flash reads as belonging to the canvas rather than to
        // the window edge.
        ui.painter().rect_stroke(
            rect.shrink(metric::RAIL * 0.5),
            egui::CornerRadius::ZERO,
            egui::Stroke::new(metric::RAIL, theme.sodium.gamma_multiply(ack)),
            egui::StrokeKind::Inside,
        );
    }

    frame
}

fn draw_image(
    ui: &mut Ui,
    clip: Rect,
    inner: Rect,
    texture: &Texture,
    geometry: Geometry,
    thirds: bool,
    theme: &Theme,
) -> Option<Rect> {
    let rotation = geometry.total_rotation();
    let quarters = geometry.quarter_turns.rem_euclid(4);

    // Size the frame presents after its quarter turns — the axes swap on an odd
    // number of them.
    let upright = if quarters % 2 == 1 {
        Vec2::new(texture.size.y, texture.size.x)
    } else {
        texture.size
    };
    if upright.x <= 0.0 || upright.y <= 0.0 {
        return None;
    }

    // Two crops, in the order the write path applies them. The first is what
    // makes this honest: after a fine angle the saved frame is the largest
    // centred rectangle of the original aspect that contains no exposed border.
    // The second is the user's own, normalised against what the first leaves.
    let standing = upright * geometry.crop_scale(upright.x / upright.y);
    let crop = geometry.crop.clamped();
    let kept = Vec2::new(standing.x * crop.w, standing.y * crop.h);

    let visible = paint::fit_rect(inner, kept);
    let scale = visible.width() / kept.x.max(0.001);
    // Where the crop's centre sits relative to the frame's, in frame pixels.
    // The frame is drawn shifted by this, and clipped, so what fills `visible`
    // is the region that will be written.
    let offset = Vec2::new(
        (crop.x + crop.w * 0.5 - 0.5) * standing.x,
        (crop.y + crop.h * 0.5 - 0.5) * standing.y,
    );

    paint::soft_shadow(
        ui.painter(),
        visible,
        Vec2::new(0.0, 2.0),
        18.0,
        Color32::from_black_alpha(90),
    );

    // Everything outside the crop is clipped away, so the surround is never
    // visible through a rotated corner.
    let painter = ui.painter_at(clip.intersect(visible));
    let drawn = Rect::from_center_size(visible.center() - offset * scale, texture.size * scale);

    if rotation.abs() < 0.001 {
        painter.image(
            texture.handle.id(),
            drawn,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        // egui has no transform on `Image`, so the quad is built by hand and
        // its four corners rotated about the centre.
        let mut mesh = egui::Mesh::with_texture(texture.handle.id());
        let c = drawn.center();
        let (s, co) = rotation.to_radians().sin_cos();
        let corners = [
            drawn.left_top(),
            drawn.right_top(),
            drawn.right_bottom(),
            drawn.left_bottom(),
        ];
        let uvs = [
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(0.0, 1.0),
        ];
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            let d = *corner - c;
            let p = Pos2::new(c.x + d.x * co - d.y * s, c.y + d.x * s + d.y * co);
            // Pushed directly rather than via `colored_vertex`, which asserts
            // the mesh is untextured and panics on a textured one.
            mesh.vertices.push(egui::epaint::Vertex {
                pos: p,
                uv: *uv,
                color: Color32::WHITE,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }

    if thirds {
        // Drawn on the crop, not on the full rotated frame — the thirds are a
        // composition guide for the image that will actually be saved.
        draw_thirds(&painter, visible, theme);
    }

    Some(visible)
}

// ---------------------------------------------------------------------------
// The crop editor
// ---------------------------------------------------------------------------

/// Which part of the crop rectangle a drag is moving.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grip {
    Move,
    N,
    S,
    E,
    W,
    Nw,
    Ne,
    Sw,
    Se,
}

/// Which side of an axis the drag moves. `None` leaves that axis alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    None,
    Min,
    Max,
}

/// Grab zone for an edge or a corner.
const GRIP: f32 = 20.0;
/// Length of the corner brackets.
const BRACKET: f32 = 16.0;

impl Grip {
    const ALL: [Grip; 9] = [
        // The move zone is registered first so the edges and corners sitting on
        // top of it win the pointer.
        Grip::Move,
        Grip::N,
        Grip::S,
        Grip::E,
        Grip::W,
        Grip::Nw,
        Grip::Ne,
        Grip::Sw,
        Grip::Se,
    ];

    /// Which edges this grip drives.
    fn sides(self) -> (Side, Side) {
        match self {
            Grip::Move => (Side::None, Side::None),
            Grip::W => (Side::Min, Side::None),
            Grip::E => (Side::Max, Side::None),
            Grip::N => (Side::None, Side::Min),
            Grip::S => (Side::None, Side::Max),
            Grip::Nw => (Side::Min, Side::Min),
            Grip::Ne => (Side::Max, Side::Min),
            Grip::Sw => (Side::Min, Side::Max),
            Grip::Se => (Side::Max, Side::Max),
        }
    }

    /// The rect that grabs it, given the crop rectangle on screen.
    fn zone(self, r: Rect) -> Rect {
        let g = GRIP.min(r.width() * 0.45).min(r.height() * 0.45);
        let corner = Vec2::splat(g);
        match self {
            Grip::Move => r,
            Grip::Nw => Rect::from_center_size(r.left_top(), corner),
            Grip::Ne => Rect::from_center_size(r.right_top(), corner),
            Grip::Sw => Rect::from_center_size(r.left_bottom(), corner),
            Grip::Se => Rect::from_center_size(r.right_bottom(), corner),
            Grip::N => Rect::from_min_max(
                Pos2::new(r.left() + g, r.top() - g * 0.5),
                Pos2::new(r.right() - g, r.top() + g * 0.5),
            ),
            Grip::S => Rect::from_min_max(
                Pos2::new(r.left() + g, r.bottom() - g * 0.5),
                Pos2::new(r.right() - g, r.bottom() + g * 0.5),
            ),
            Grip::W => Rect::from_min_max(
                Pos2::new(r.left() - g * 0.5, r.top() + g),
                Pos2::new(r.left() + g * 0.5, r.bottom() - g),
            ),
            Grip::E => Rect::from_min_max(
                Pos2::new(r.right() - g * 0.5, r.top() + g),
                Pos2::new(r.right() + g * 0.5, r.bottom() - g),
            ),
        }
    }

    fn cursor(self) -> egui::CursorIcon {
        match self {
            Grip::Move => egui::CursorIcon::Grab,
            Grip::N | Grip::S => egui::CursorIcon::ResizeVertical,
            Grip::E | Grip::W => egui::CursorIcon::ResizeHorizontal,
            Grip::Nw | Grip::Se => egui::CursorIcon::ResizeNwSe,
            Grip::Ne | Grip::Sw => egui::CursorIcon::ResizeNeSw,
        }
    }
}

/// Draw and drive the crop rectangle over a frame.
///
/// `frame` is the rect the uncropped frame occupies — exactly what `canvas`
/// returns when it is handed `CropRect::FULL`, which is why the editing view
/// never applies the crop itself: you cannot place a crop you cannot see
/// outside of.
///
/// `aspect` is the shape the crop is held to, or `None` for a custom crop where
/// each edge moves on its own. Under a lock both axes are always scaled
/// together — clamping them apart would silently hand back a shape other than
/// the one asked for.
pub fn crop_overlay(
    ui: &mut Ui,
    theme: &Theme,
    frame: Rect,
    crop: CropRect,
    aspect: Option<f32>,
) -> Option<CropRect> {
    if frame.width() < 2.0 || frame.height() < 2.0 {
        return None;
    }
    let here = crop.clamped();
    let r = Rect::from_min_size(
        frame.min + Vec2::new(here.x * frame.width(), here.y * frame.height()),
        Vec2::new(here.w * frame.width(), here.h * frame.height()),
    );

    // ---- interaction, before painting so the drag lands this frame --------
    let min = Vec2::new(
        frame.width() * CropRect::MIN,
        frame.height() * CropRect::MIN,
    );
    let mut changed = None;
    for (i, grip) in Grip::ALL.iter().enumerate() {
        let response = ui.interact(grip.zone(r), ui.id().with(("crop-grip", i)), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(grip.cursor());
        }
        if !response.dragged() {
            continue;
        }
        let delta = response.drag_delta();
        if delta == Vec2::ZERO {
            continue;
        }
        let moved = match grip {
            Grip::Move => settle(r.translate(delta), frame, aspect),
            _ => resize(r, frame, *grip, delta, aspect, min),
        };
        changed = Some(normalise(moved, frame));
    }

    // ---- the veil --------------------------------------------------------
    // Excluded ground is veiled with the window colour, the same treatment the
    // histogram gives a clamped region: the same idea, so the same mark.
    if !here.is_full() {
        let veil = theme.window.gamma_multiply(0.72);
        for band in [
            Rect::from_min_max(frame.left_top(), Pos2::new(frame.right(), r.top())),
            Rect::from_min_max(Pos2::new(frame.left(), r.bottom()), frame.right_bottom()),
            Rect::from_min_max(
                Pos2::new(frame.left(), r.top()),
                Pos2::new(r.left(), r.bottom()),
            ),
            Rect::from_min_max(
                Pos2::new(r.right(), r.top()),
                Pos2::new(frame.right(), r.bottom()),
            ),
        ] {
            if band.width() > 0.0 && band.height() > 0.0 {
                paint::fill(ui.painter(), band, veil);
            }
        }
    }

    // ---- the rectangle itself --------------------------------------------
    paint::border(ui.painter(), r, theme.fg);
    draw_thirds(ui.painter(), r, theme);

    // Corner brackets, 3 pt inside the border: the grip is the corner, so it is
    // marked rather than decorated with a separate handle.
    let stroke = egui::Stroke::new(metric::RAIL, theme.fg);
    let arm = BRACKET.min(r.width() * 0.4).min(r.height() * 0.4);
    let inset = metric::RAIL * 0.5;
    for (corner, dx, dy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.left_bottom(), 1.0, -1.0),
        (r.right_bottom(), -1.0, -1.0),
    ] {
        let o = Pos2::new(corner.x + dx * inset, corner.y + dy * inset);
        ui.painter()
            .line_segment([o, Pos2::new(o.x + dx * arm, o.y)], stroke);
        ui.painter()
            .line_segment([o, Pos2::new(o.x, o.y + dy * arm)], stroke);
    }

    changed
}

/// Move one edge or corner, holding the shape when there is one to hold.
fn resize(r: Rect, frame: Rect, grip: Grip, d: Vec2, aspect: Option<f32>, min: Vec2) -> Rect {
    let (hx, vy) = grip.sides();

    // Floored before anything else: a drag far enough past the opposite edge
    // would otherwise turn the rectangle inside out, and a negative width put
    // through the aspect lock takes the whole crop with it.
    let mut w = (r.width()
        + match hx {
            Side::Min => -d.x,
            Side::Max => d.x,
            Side::None => 0.0,
        })
    .max(min.x);
    let mut h = (r.height()
        + match vy {
            Side::Min => -d.y,
            Side::Max => d.y,
            Side::None => 0.0,
        })
    .max(min.y);

    if let Some(a) = aspect {
        // On a corner the axis the pointer moved further along leads, so the
        // drag goes where the hand went.
        let lead_x = match (hx, vy) {
            (Side::None, _) => false,
            (_, Side::None) => true,
            _ => d.x.abs() >= d.y.abs(),
        };
        if lead_x {
            h = w / a;
        } else {
            w = h * a;
        }
    }

    // How far this grip can travel before it leaves the frame.
    let max_w = match hx {
        Side::Min => r.right() - frame.left(),
        Side::Max => frame.right() - r.left(),
        Side::None => frame.width(),
    };
    let max_h = match vy {
        Side::Min => r.bottom() - frame.top(),
        Side::Max => frame.bottom() - r.top(),
        Side::None => frame.height(),
    };

    if aspect.is_some() {
        let shrink = (max_w / w.max(0.001)).min(max_h / h.max(0.001)).min(1.0);
        w *= shrink;
        h *= shrink;
        let grow = (min.x / w.max(0.001)).max(min.y / h.max(0.001)).max(1.0);
        w *= grow;
        h *= grow;
    } else {
        w = w.clamp(min.x, max_w.max(min.x));
        h = h.clamp(min.y, max_h.max(min.y));
    }

    let left = match hx {
        Side::Min => r.right() - w,
        _ => r.left(),
    };
    let top = match vy {
        Side::Min => r.bottom() - h,
        _ => r.top(),
    };
    let mut moved = Rect::from_min_size(Pos2::new(left, top), Vec2::new(w, h));

    // An axis the grip does not drive but the lock changed grows about its own
    // centre, not from an edge — dragging the right edge of a locked crop should
    // not also walk it down the frame.
    if aspect.is_some() {
        if hx == Side::None {
            moved = Rect::from_center_size(Pos2::new(r.center().x, moved.center().y), moved.size());
        }
        if vy == Side::None {
            moved = Rect::from_center_size(Pos2::new(moved.center().x, r.center().y), moved.size());
        }
    }

    settle(moved, frame, aspect)
}

/// Bring a rect back inside the frame: scaled down if it is too big, then
/// pushed in if it is merely outside.
fn settle(mut r: Rect, frame: Rect, aspect: Option<f32>) -> Rect {
    if r.width() > frame.width() || r.height() > frame.height() {
        let s = (frame.width() / r.width().max(0.001)).min(frame.height() / r.height().max(0.001));
        let size = if aspect.is_some() {
            r.size() * s
        } else {
            Vec2::new(r.width().min(frame.width()), r.height().min(frame.height()))
        };
        r = Rect::from_center_size(r.center(), size);
    }
    let dx = (frame.left() - r.left()).max(0.0) - (r.right() - frame.right()).max(0.0);
    let dy = (frame.top() - r.top()).max(0.0) - (r.bottom() - frame.bottom()).max(0.0);
    r.translate(Vec2::new(dx, dy))
}

fn normalise(r: Rect, frame: Rect) -> CropRect {
    CropRect {
        x: (r.left() - frame.left()) / frame.width(),
        y: (r.top() - frame.top()) / frame.height(),
        w: r.width() / frame.width(),
        h: r.height() / frame.height(),
    }
    .clamped()
}

/// Rule-of-thirds overlay: 1 pt lines at exact thirds, `fg` at 28% opacity.
fn draw_thirds(painter: &egui::Painter, rect: Rect, theme: &Theme) {
    let stroke = egui::Stroke::new(metric::HAIR, theme.fg.gamma_multiply(0.28));
    for i in 1..3 {
        let t = i as f32 / 3.0;
        let x = rect.left() + rect.width() * t;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
        let y = rect.top() + rect.height() * t;
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            stroke,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pickture_kernel::pixel_ops;

    #[test]
    fn no_angle_means_no_crop() {
        let g = Geometry {
            quarter_turns: 2,
            angle: 0.0,
            crop: CropRect::FULL,
        };
        assert_eq!(g.crop_scale(1.5), 1.0);
    }

    #[test]
    fn crop_scale_matches_what_the_write_path_produces() {
        // The canvas draws the crop and the writer applies it. If these two ever
        // disagree, the preview stops being a preview.
        for (w, h) in [(300u32, 200u32), (200, 300), (400, 400)] {
            for angle in [1.0f32, 4.0, -7.5, 10.0] {
                let g = Geometry {
                    quarter_turns: 0,
                    angle,
                    crop: CropRect::FULL,
                };
                let predicted = g.crop_scale(w as f32 / h as f32);

                let src = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
                let rotated = pixel_ops::rotate_free(&src, angle);
                let cropped = pixel_ops::crop_to_aspect(&rotated, w, h, angle);
                let actual = cropped.width() as f32 / w as f32;

                assert!(
                    (predicted - actual).abs() < 0.02,
                    "{w}x{h} at {angle}°: canvas {predicted:.4} vs writer {actual:.4}"
                );
            }
        }
    }

    #[test]
    fn crop_shrinks_as_the_angle_grows() {
        let aspect = 1.5;
        let mut previous = 1.0;
        for angle in [0.0f32, 2.0, 5.0, 10.0] {
            let s = Geometry {
                quarter_turns: 0,
                angle,
                crop: CropRect::FULL,
            }
            .crop_scale(aspect);
            assert!(s <= previous + 1e-6, "crop grew at {angle}°");
            assert!(s > 0.0);
            previous = s;
        }
        assert!(previous < 1.0, "10° should visibly crop");
    }

    #[test]
    fn a_locked_resize_keeps_its_shape() {
        let frame = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0));
        let start = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(300.0, 200.0));
        let min = Vec2::new(30.0, 20.0);
        for grip in [Grip::Se, Grip::Nw, Grip::E, Grip::N] {
            for d in [
                Vec2::new(40.0, 5.0),
                Vec2::new(-90.0, -60.0),
                Vec2::new(900.0, 900.0),
            ] {
                let r = resize(start, frame, grip, d, Some(1.5), min);
                assert!(
                    (r.width() / r.height() - 1.5).abs() < 0.02,
                    "aspect drifted to {}",
                    r.width() / r.height()
                );
                assert!(frame.contains_rect(r.shrink(0.01)), "left the frame: {r:?}");
            }
        }
    }

    #[test]
    fn a_custom_resize_moves_one_edge_only() {
        let frame = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0));
        let start = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(300.0, 200.0));
        let r = resize(
            start,
            frame,
            Grip::E,
            Vec2::new(50.0, 40.0),
            None,
            Vec2::new(30.0, 20.0),
        );
        assert_eq!(r.left(), start.left());
        assert_eq!(r.height(), start.height());
        assert!((r.width() - 350.0).abs() < 0.01);
    }

    #[test]
    fn a_drag_cannot_push_the_crop_out_of_the_frame() {
        let frame = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(600.0, 400.0));
        let start = Rect::from_min_size(Pos2::new(60.0, 70.0), Vec2::new(300.0, 200.0));
        let moved = settle(
            start.translate(Vec2::new(-4000.0, 4000.0)),
            frame,
            Some(1.5),
        );
        assert!(frame.contains_rect(moved.shrink(0.01)));
        let norm = normalise(moved, frame);
        assert!(norm.x >= 0.0 && norm.y >= 0.0);
        assert!(norm.x + norm.w <= 1.0001 && norm.y + norm.h <= 1.0001);
    }

    #[test]
    fn a_crop_is_what_the_canvas_would_show() {
        // The canvas fits `kept` into the available space, so a half-width crop
        // of a 3:2 frame presents as 3:4 of the original aspect.
        let g = Geometry {
            quarter_turns: 0,
            angle: 0.0,
            crop: CropRect {
                x: 0.25,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            },
        };
        let standing = Vec2::new(300.0, 200.0) * g.crop_scale(1.5);
        let kept = Vec2::new(standing.x * g.crop.w, standing.y * g.crop.h);
        assert!((kept.x / kept.y - 0.75).abs() < 1e-4);
    }

    #[test]
    fn total_rotation_combines_quarters_and_angle() {
        let g = Geometry {
            quarter_turns: 3,
            angle: -2.5,
            crop: CropRect::FULL,
        };
        assert_eq!(g.total_rotation(), 267.5);
    }
}
