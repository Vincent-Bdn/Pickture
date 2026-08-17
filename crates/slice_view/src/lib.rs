//! View: the image canvas.
//!
//! One rule governs this whole slice. The region immediately around the frame
//! is `surround` — achromatic, mid luminance, never black or white — because
//! the user is judging white balance against it. Chromatic adaptation would
//! shift their perception of the frame's colour, and surround luminance would
//! shift their perception of its contrast. Nothing else lives in this region.

use egui::{Color32, Pos2, Rect, Ui, Vec2};
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};
use pickture_ui_kit::Texture;

/// Geometry to apply to the displayed frame.
///
/// The canvas shows the frame **exactly as it will be written**: quarter turns,
/// the fine angle, and the aspect-preserving crop that follows a fine angle.
/// Showing a rotated frame *without* its crop produced the worst of both — a
/// tilted quad with the surround visible through its corners, which is neither
/// the original nor the result.
#[derive(Clone, Copy, Default)]
pub struct Geometry {
    pub quarter_turns: i32,
    pub angle: f32,
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
pub fn canvas(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    content: CanvasContent<'_>,
    padding: f32,
    ack: f32,
    thirds: bool,
) {
    paint::fill(ui.painter(), rect, theme.surround);
    let inner = rect.shrink(padding);

    match content {
        CanvasContent::Image { texture, geometry } => {
            draw_image(ui, rect, inner, texture, geometry, thirds, theme);
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
}

fn draw_image(
    ui: &mut Ui,
    clip: Rect,
    inner: Rect,
    texture: &Texture,
    geometry: Geometry,
    thirds: bool,
    theme: &Theme,
) {
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
        return;
    }

    // The crop is what makes this honest: after a fine angle the saved frame is
    // the largest centred rectangle of the original aspect that contains no
    // exposed border, so that is what gets drawn.
    let crop = upright * geometry.crop_scale(upright.x / upright.y);
    let visible = paint::fit_rect(inner, crop);
    let scale = visible.width() / crop.x.max(0.001);

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
    let drawn = Rect::from_center_size(visible.center(), texture.size * scale);

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
            }
            .crop_scale(aspect);
            assert!(s <= previous + 1e-6, "crop grew at {angle}°");
            assert!(s > 0.0);
            previous = s;
        }
        assert!(previous < 1.0, "10° should visibly crop");
    }

    #[test]
    fn total_rotation_combines_quarters_and_angle() {
        let g = Geometry {
            quarter_turns: 3,
            angle: -2.5,
        };
        assert_eq!(g.total_rotation(), 267.5);
    }
}
