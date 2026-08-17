//! The filmstrip — the first of the six custom-painted surfaces.
//!
//! A photographer scanning nine hundred frames needs to know which ones they
//! have already judged without reading anything. So judgement is encoded in
//! **form as well as colour**: a 3 pt rail down the full cell height, and a
//! triangular notch cut into the thumbnail's top-right corner. Either one alone
//! would carry the state; together they survive both peripheral vision and
//! colour blindness.

use egui::{Pos2, Rect, ScrollArea, Sense, Ui, Vec2};
use pickture_kernel::{Judgement, Session};
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};
use pickture_ui_kit::TextureStore;

use crate::{
    cell_height, frame_aspect, judgement_label, thumb_height, BrowseEvent, BrowseState,
    CELL_CONTENT_W, CELL_PAD_X, LABEL_GAP, LABEL_H,
};

/// Frames whose acknowledgement flash is still running, as `(index, alpha)`.
pub struct AckState {
    pub index: Option<usize>,
    pub alpha: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn filmstrip(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    session: &Session,
    textures: &mut TextureStore,
    state: &mut BrowseState,
    ack: &AckState,
    scanning: Option<(usize, usize)>,
) -> Option<BrowseEvent> {
    paint::fill(ui.painter(), rect, theme.rail);
    paint::rule_right(ui.painter(), rect, theme.hair);

    let mut event = None;

    // ---- header --------------------------------------------------------
    let (header, body) = paint::split_top(rect, 26.0);
    crate::eyebrow(
        ui,
        theme,
        Pos2::new(header.left() + CELL_PAD_X, header.center().y + 4.0),
        "Frames",
    );
    let count_text = match scanning {
        Some((found, _)) => format!("{found}"),
        None => format!("{}", session.frames.len()),
    };
    paint::text_right(
        ui.painter(),
        Pos2::new(header.right() - CELL_PAD_X, header.center().y + 4.0),
        &count_text,
        tokens::mono(size::MONO_XS),
        theme.fg_muted,
    );

    // ---- scanning progress ---------------------------------------------
    let body = if let Some((found, total)) = scanning {
        let (bar_area, rest) = paint::split_top(body, 10.0);
        let track = Rect::from_min_size(
            Pos2::new(bar_area.left() + CELL_PAD_X, bar_area.top() + 4.0),
            Vec2::new(bar_area.width() - CELL_PAD_X * 2.0, 2.0),
        );
        let fraction = if total == 0 {
            0.0
        } else {
            found as f32 / total as f32
        };
        paint::progress(ui.painter(), theme, track, fraction);
        rest
    } else {
        body
    };

    // ---- cells ----------------------------------------------------------
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let width = body.width();
            let mut y = ui.cursor().top() + metric::S4;

            for (index, frame) in session.frames.iter().enumerate() {
                let aspect = frame_aspect(frame, textures);
                let h = cell_height(aspect);

                let (cell_rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, h), Sense::click());
                let _ = response;
                let cell_rect = Rect::from_min_size(
                    Pos2::new(body.left(), cell_rect.top()),
                    Vec2::new(width, h),
                );

                let clicked = draw_cell(
                    ui, theme, cell_rect, session, frame, index, textures, ack, aspect,
                );
                if clicked {
                    event = Some(BrowseEvent::SelectFrame(index));
                }

                if state.scroll_to_cursor && index == session.cursor {
                    ui.scroll_to_rect(cell_rect.expand2(Vec2::new(0.0, metric::S24)), None);
                }

                ui.add_space(metric::CELL_GAP);
                y += h + metric::CELL_GAP;
            }
            let _ = y;
            ui.add_space(metric::S8);
        });

    state.scroll_to_cursor = false;
    event
}

#[allow(clippy::too_many_arguments)]
fn draw_cell(
    ui: &mut Ui,
    theme: &Theme,
    cell: Rect,
    session: &Session,
    frame: &pickture_kernel::Frame,
    index: usize,
    textures: &mut TextureStore,
    ack: &AckState,
    aspect: f32,
) -> bool {
    let is_current = index == session.cursor;
    let judgement = session.judgement_of(&frame.id);
    let in_destination = session.is_in_destination(&frame.id);
    let kept = judgement == Some(Judgement::Kept) || in_destination;
    let passed = judgement == Some(Judgement::Passed);

    let inner = Rect::from_min_max(
        Pos2::new(cell.left() + CELL_PAD_X, cell.top()),
        Pos2::new(cell.right() - CELL_PAD_X, cell.bottom()),
    );

    let response = ui.interact(inner, ui.id().with(("cell", index)), Sense::click());

    // -- judgement rail ---------------------------------------------------
    let (rail, after_rail) = paint::split_left(inner, metric::RAIL);
    // Kept wins over current: a frame you have already chosen should read as
    // chosen even while you are looking at it.
    let mut rail_colour = if kept {
        theme.sodium
    } else if is_current {
        theme.rail_current()
    } else {
        theme.hair
    };
    if ack.index == Some(index) && ack.alpha > 0.0 {
        rail_colour = theme.sodium;
    }
    paint::fill(ui.painter(), rail, rail_colour);

    // -- thumbnail --------------------------------------------------------
    let content = Rect::from_min_max(
        Pos2::new(after_rail.left() + metric::S8, inner.top()),
        Pos2::new(inner.right(), inner.bottom()),
    );
    let th = thumb_height(aspect);
    let thumb_rect = Rect::from_min_size(content.min, Vec2::new(CELL_CONTENT_W, th));

    let dim = if passed { 0.5 } else { 1.0 };
    let has_thumb = textures.has_thumb(&frame.path);

    if has_thumb {
        if let Some(tex) = textures.thumb(&frame.path) {
            let fitted = paint::fit_rect(thumb_rect, tex.size);
            // The image is fitted inside the reserved box, so a frame whose
            // real aspect differs slightly from the header never overflows.
            paint::fill(ui.painter(), thumb_rect, theme.thumb_empty);
            ui.painter().image(
                tex.handle.id(),
                fitted,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                egui::Color32::WHITE.gamma_multiply(dim),
            );
        }
    } else {
        // Pending: the placeholder occupies the *exact* final size, so nothing
        // below it moves when the decode lands.
        paint::fill(ui.painter(), thumb_rect, theme.thumb_empty);
        paint::text_center(
            ui.painter(),
            thumb_rect.center(),
            "decoding",
            tokens::mono(size::MONO_XS),
            theme.fg_disabled,
        );
    }

    // -- kept notch -------------------------------------------------------
    let notch_alpha = if kept {
        1.0
    } else if ack.index == Some(index) {
        ack.alpha
    } else {
        0.0
    };
    paint::notch(
        ui.painter(),
        thumb_rect,
        metric::NOTCH,
        theme.sodium,
        notch_alpha,
    );

    // -- already in destination -------------------------------------------
    if in_destination {
        paint::text_left(
            ui.painter(),
            Pos2::new(thumb_rect.left() + 5.0, thumb_rect.bottom() - 9.0),
            "IN SELECTION/",
            tokens::mono(9.0),
            theme.fg,
        );
    }

    // -- current outline --------------------------------------------------
    if is_current {
        paint::outline_offset(ui.painter(), thumb_rect, theme.sodium);
    }

    // -- label row --------------------------------------------------------
    let label_y = thumb_rect.bottom() + LABEL_GAP + LABEL_H * 0.5;
    let label_font = tokens::mono(size::MONO_XS);
    let (state_text, state_is_sodium) = judgement_label(session, frame);

    let label_colour = if kept {
        theme.sodium
    } else if is_current {
        theme.fg
    } else {
        theme.fg_disabled
    };

    let state_text = if is_current && !kept {
        "▸"
    } else {
        state_text
    };
    let state_colour = if state_is_sodium {
        theme.sodium
    } else if is_current {
        theme.fg
    } else {
        theme.fg_disabled
    };

    let state_w = paint::text_width(ui.painter(), state_text, &label_font);
    let name_avail = content.width() - state_w - metric::S8;
    let name = paint::elide(ui.painter(), &frame.stem, &label_font, name_avail.max(20.0));

    paint::text_left(
        ui.painter(),
        Pos2::new(content.left(), label_y),
        &name,
        label_font.clone(),
        label_colour,
    );
    paint::text_right(
        ui.painter(),
        Pos2::new(content.right(), label_y),
        state_text,
        label_font,
        state_colour,
    );

    response.clicked()
}
