//! Window chrome: title bar, info bar, status bar.
//!
//! The window is undecorated, so the title bar is ours to draw — which is what
//! lets the working path live in it and act as the folder switcher.

use egui::{Pos2, Rect, Sense, Ui, Vec2, ViewportCommand};
use pickture_kernel::{Destination, ExifSummary};
use pickture_slice_browse::{switcher::switcher_chip, BrowseEvent};
use pickture_slice_select::{destination_chip, SelectEvent};
use pickture_ui_kit::mark;
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};

/// Title bar. Returns a browse event when the path chip is clicked.
pub fn title_bar(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    folder: Option<&std::path::Path>,
    switcher_open: bool,
) -> Option<BrowseEvent> {
    paint::fill(ui.painter(), rect, theme.chrome);
    paint::rule_bottom(ui.painter(), rect, theme.hair);

    // Dragging the bar moves the window; the buttons sit above this in z-order
    // because they are interacted with first.
    let drag = ui.interact(rect, ui.id().with("title-drag"), Sense::click_and_drag());
    if drag.is_pointer_button_down_on() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }

    let mark_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 14.0, rect.center().y - 7.0),
        Vec2::splat(14.0),
    );
    mark::draw(ui.painter(), theme, mark_rect);

    let mut event = None;
    match folder {
        Some(path) => {
            let area = Rect::from_min_max(
                Pos2::new(mark_rect.right() + metric::S8, rect.top()),
                Pos2::new(rect.right() - 120.0, rect.bottom()),
            );
            event = switcher_chip(ui, theme, area, path, switcher_open);
        }
        None => {
            paint::text_left(
                ui.painter(),
                Pos2::new(mark_rect.right() + metric::S8 + 2.0, rect.center().y),
                "Pickture",
                tokens::mono(size::MONO_M),
                theme.fg_secondary,
            );
        }
    }

    window_buttons(ui, theme, rect);
    event
}

/// Minimise, maximise and close.
///
/// Drawn as shapes rather than typeset as `— ▢ ✕`. Whether the user can close
/// the window should not depend on a font happening to carry a glyph, and these
/// three marks are simpler to draw than to describe.
fn window_buttons(ui: &mut Ui, theme: &Theme, bar: Rect) {
    let w = 30.0;
    for i in 0..3usize {
        let btn = Rect::from_min_size(
            Pos2::new(
                bar.right() - (3 - i) as f32 * (w + metric::S8) + metric::S8,
                bar.top(),
            ),
            Vec2::new(w, bar.height()),
        );
        let response = ui.interact(btn, ui.id().with(("win-btn", i)), Sense::click());
        let hovered = response.hovered();
        if hovered {
            paint::fill(ui.painter(), btn, theme.chrome_hover);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let ink = if hovered { theme.fg } else { theme.fg_disabled };
        let stroke = egui::Stroke::new(metric::HAIR, ink);
        let c = btn.center();
        let r = 4.5;

        match i {
            0 => {
                ui.painter()
                    .line_segment([Pos2::new(c.x - r, c.y), Pos2::new(c.x + r, c.y)], stroke);
            }
            1 => {
                ui.painter().rect_stroke(
                    Rect::from_center_size(c, Vec2::splat(r * 2.0)),
                    egui::CornerRadius::ZERO,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            _ => paint::cross(ui.painter(), c, r, ink),
        }

        if response.clicked() {
            let cmd = match i {
                0 => ViewportCommand::Minimized(true),
                1 => {
                    let maximized = ui.input(|inp| inp.viewport().maximized.unwrap_or(false));
                    ViewportCommand::Maximized(!maximized)
                }
                _ => ViewportCommand::Close,
            };
            ui.ctx().send_viewport_cmd(cmd);
        }
    }
}

// ---------------------------------------------------------------------------
// Info bar
// ---------------------------------------------------------------------------

pub struct InfoBar<'a> {
    pub filename: &'a str,
    pub exif: Option<&'a ExifSummary>,
    pub already_kept: bool,
    pub destination: &'a Destination,
    pub destination_open: bool,
}

pub enum InfoEvent {
    Keep,
    Select(SelectEvent),
}

pub fn info_bar(ui: &mut Ui, theme: &Theme, rect: Rect, info: InfoBar<'_>) -> Option<InfoEvent> {
    paint::fill(ui.painter(), rect, theme.chrome);
    paint::rule_bottom(ui.painter(), rect, theme.hair);

    // ---- right side first, so the left can be elided against it ----------
    let keycap_right = Pos2::new(rect.right() - 18.0, rect.center().y);
    let cap_w = paint::keycap(ui.painter(), theme, keycap_right, "↵");

    let label_font = tokens::mono(size::MONO_S);
    let label = "Add to selection";
    let label_w = paint::text_width(ui.painter(), label, &label_font);
    let label_x = keycap_right.x - cap_w - metric::S12 - label_w;
    paint::text_left(
        ui.painter(),
        Pos2::new(label_x, rect.center().y),
        label,
        label_font,
        theme.fg_muted,
    );

    let keep_zone = Rect::from_min_max(
        Pos2::new(label_x - metric::S6, rect.top() + 6.0),
        Pos2::new(rect.right() - 12.0, rect.bottom() - 6.0),
    );
    let keep = ui.interact(keep_zone, ui.id().with("keep-action"), Sense::click());
    if keep.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let (chip_w, chip_event) = destination_chip(
        ui,
        theme,
        Pos2::new(label_x - metric::S16, rect.center().y),
        info.destination,
        info.destination_open,
    );

    // ---- left side -------------------------------------------------------
    let left_limit = label_x - metric::S16 - chip_w - metric::S16;
    let mut x = rect.left() + 18.0;

    let name_font = tokens::sans_medium(size::SANS_M);
    let name = paint::elide(
        ui.painter(),
        info.filename,
        &name_font,
        (left_limit - x).max(60.0),
    );
    x += paint::text_left(
        ui.painter(),
        Pos2::new(x, rect.center().y),
        &name,
        name_font,
        theme.fg,
    )
    .width()
        + 14.0;

    if let Some(exif) = info.exif {
        let line = exif.line();
        if !line.is_empty() && x < left_limit {
            let font = tokens::mono(size::MONO_S);
            let shown = paint::elide(ui.painter(), &line, &font, (left_limit - x).max(0.0));
            x += paint::text_left(
                ui.painter(),
                Pos2::new(x, rect.center().y),
                &shown,
                font,
                theme.fg_muted,
            )
            .width()
                + 14.0;
        }
    }

    if info.already_kept && x < left_limit {
        paint::text_left(
            ui.painter(),
            Pos2::new(x, rect.center().y),
            "in selection/",
            tokens::mono(size::MONO_S),
            theme.sodium,
        );
    }

    if let Some(e) = chip_event {
        return Some(InfoEvent::Select(e));
    }
    keep.clicked().then_some(InfoEvent::Keep)
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

pub struct Status<'a> {
    pub position: usize,
    pub total: usize,
    pub kept: usize,
    pub passed: usize,
    pub decode_ms: u32,
    /// A refusal or confirmation, shown as a plain line. Never a red box.
    pub notice: Option<&'a str>,
    pub show_all_shortcuts: bool,
}

pub fn status_bar(ui: &mut Ui, theme: &Theme, rect: Rect, status: Status<'_>) {
    paint::fill(ui.painter(), rect, theme.chrome);
    paint::rule_top(ui.painter(), rect, theme.hair);

    let font = tokens::mono(size::MONO_S);
    let y = rect.center().y;
    let gap = 22.0;

    // A notice takes the whole left side — if the tool refused to do something,
    // that is more important than the counts.
    if let Some(notice) = status.notice {
        paint::text_left(
            ui.painter(),
            Pos2::new(rect.left() + 18.0, y),
            notice,
            font.clone(),
            theme.sodium,
        );
    } else {
        let mut x = rect.left() + 18.0;
        let item = |text: &str, colour: egui::Color32, x: &mut f32| {
            *x += paint::text_left(ui.painter(), Pos2::new(*x, y), text, font.clone(), colour)
                .width()
                + gap;
        };
        item(
            &format!("{} / {}", status.position, status.total),
            theme.fg_muted,
            &mut x,
        );
        item(&format!("{} kept", status.kept), theme.sodium, &mut x);
        item(&format!("{} passed", status.passed), theme.fg_muted, &mut x);
        if status.decode_ms > 0 {
            item(
                &format!("decode {}ms", status.decode_ms),
                theme.fg_muted,
                &mut x,
            );
        }
    }

    // The four core keys are permanently visible, so no shortcut lives only in
    // the README. `/` reveals the rest.
    let shortcuts: &[&str] = if status.show_all_shortcuts {
        &[
            "← → navigate",
            "↵ enhance",
            "^↵ keep as shot",
            "Del pass",
            "Z zoom",
            "[ ] rotate",
            "O folder",
            "S destination",
        ]
    } else {
        &[
            "← → navigate",
            "↵ enhance",
            "^↵ keep as shot",
            "Del pass",
            "/ more",
        ]
    };

    let mut x = rect.right() - 18.0;
    for text in shortcuts.iter().rev() {
        let w = paint::text_width(ui.painter(), text, &font);
        x -= w;
        paint::text_left(
            ui.painter(),
            Pos2::new(x, y),
            text,
            font.clone(),
            theme.fg_muted,
        );
        x -= gap;
    }
}
