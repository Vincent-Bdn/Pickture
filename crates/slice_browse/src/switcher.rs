//! Switching the working folder without restarting.
//!
//! The working path in the title bar *is* the switcher — the design's point
//! being that a setting this consequential should not be buried in a menu. `O`
//! opens it, `↑↓↵` chooses, `⇧O` goes straight to the OS picker.
//!
//! No window is torn down and nothing is discarded: the composition root swaps
//! the active session, cancels the outgoing decode queue, and leaves both
//! caches warm so switching back is instant.

use egui::{Area, Id, Order, Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::SessionStore;
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};

use crate::{folder_row, BrowseEvent, BrowseState, FolderRow};

const ROW_H: f32 = 46.0;
const FOOTER_H: f32 = 38.0;
const HEADER_H: f32 = 26.0;

/// Where the popover hangs from — just under the title-bar path chip.
pub fn switcher_anchor(title_bar: Rect) -> Pos2 {
    Pos2::new(title_bar.left() + 20.0, title_bar.bottom() - 4.0)
}

/// The title-bar path control. Returns an event when clicked.
pub fn switcher_chip(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    folder: &std::path::Path,
    open: bool,
) -> Option<BrowseEvent> {
    let font = tokens::mono(size::MONO_M);
    let path = folder.display().to_string();

    let hint_font = tokens::mono(size::MONO_XS);
    let caret_font = tokens::mono(9.0);

    let max_text = rect.width() - 60.0;
    let shown = paint::elide_start(ui.painter(), &path, &font, max_text.max(60.0));
    let text_w = paint::text_width(ui.painter(), &shown, &font);

    let chip = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - 12.0),
        Vec2::new(text_w + 46.0, 24.0),
    );
    let response = ui.interact(chip, ui.id().with("switcher-chip"), Sense::click());

    if open || response.hovered() {
        paint::fill(ui.painter(), chip, theme.chrome_hover);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let mut x = chip.left() + metric::S8;
    x += paint::text_left(
        ui.painter(),
        Pos2::new(x, chip.center().y),
        &shown,
        font,
        theme.fg,
    )
    .width()
        + metric::S8;
    x += paint::text_left(
        ui.painter(),
        Pos2::new(x, chip.center().y),
        "▾",
        caret_font,
        theme.fg_muted,
    )
    .width()
        + metric::S6;
    paint::text_left(
        ui.painter(),
        Pos2::new(x, chip.center().y),
        "O",
        hint_font,
        theme.fg_disabled,
    );

    response.clicked().then_some(BrowseEvent::ToggleSwitcher)
}

/// The popover itself. Drawn in a foreground area so it floats over the strip.
pub fn folder_switcher_popover(
    ctx: &egui::Context,
    theme: &Theme,
    anchor: Pos2,
    store: &SessionStore,
    active: Option<&std::path::Path>,
    state: &mut BrowseState,
) -> Option<BrowseEvent> {
    if !state.switcher_open {
        return None;
    }

    let rows = crate::switcher_rows(store, active);
    if rows.is_empty() && store.recent.is_empty() {
        // Nothing to switch between — go straight to the OS picker rather than
        // showing an empty menu.
        state.switcher_open = false;
        return Some(BrowseEvent::BrowseForFolder);
    }

    state.switcher_index = state.switcher_index.min(rows.len().saturating_sub(1));

    let height = HEADER_H + rows.len() as f32 * ROW_H + FOOTER_H + metric::S12;
    let mut event = None;

    Area::new(Id::new("folder-switcher"))
        .order(Order::Foreground)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            // Space is claimed *before* anything is drawn: a fresh Area's clip
            // rect is empty until its content allocates, so painting first
            // silently discards every shape.
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(metric::SWITCHER_W, height), Sense::hover());
            {
                paint::popover_frame(ui.painter(), theme, rect);

                crate::eyebrow(
                    ui,
                    theme,
                    Pos2::new(rect.left() + 14.0, rect.top() + HEADER_H * 0.5 + 4.0),
                    "Switch folder · progress is kept",
                );

                for (i, (path, meta, is_current)) in rows.iter().enumerate() {
                    let row_rect = Rect::from_min_size(
                        Pos2::new(rect.left(), rect.top() + HEADER_H + i as f32 * ROW_H),
                        Vec2::new(rect.width(), ROW_H),
                    );
                    let row = FolderRow {
                        path: path.clone(),
                        meta: meta.clone(),
                        is_current: *is_current,
                        is_focused: i == state.switcher_index,
                    };
                    if folder_row(ui, theme, row_rect, &row, true) {
                        event = Some(BrowseEvent::OpenFolder(path.clone()));
                    }
                }

                // ---- footer ------------------------------------------------
                let footer = Rect::from_min_size(
                    Pos2::new(
                        rect.left(),
                        rect.top() + HEADER_H + rows.len() as f32 * ROW_H,
                    ),
                    Vec2::new(rect.width(), FOOTER_H),
                );
                paint::rule_top(
                    ui.painter(),
                    footer.shrink2(Vec2::new(14.0, 0.0)),
                    theme.hair,
                );
                let browse = ui.interact(footer, ui.id().with("switcher-browse"), Sense::click());
                if browse.hovered() {
                    paint::fill(ui.painter(), footer, theme.chrome_hover);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                paint::text_left(
                    ui.painter(),
                    Pos2::new(footer.left() + 14.0, footer.center().y + 2.0),
                    "Browse for a folder…",
                    tokens::sans(size::SANS_S),
                    theme.sodium,
                );
                paint::text_right(
                    ui.painter(),
                    Pos2::new(footer.right() - 14.0, footer.center().y + 2.0),
                    "⇧O",
                    tokens::mono(size::MONO_XS),
                    theme.fg_disabled,
                );
                if browse.clicked() {
                    event = Some(BrowseEvent::BrowseForFolder);
                }
            }
        });

    // ---- keyboard -------------------------------------------------------
    ctx.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) && !rows.is_empty() {
            state.switcher_index = (state.switcher_index + 1).min(rows.len() - 1);
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            state.switcher_index = state.switcher_index.saturating_sub(1);
        }
        if i.key_pressed(egui::Key::Escape) {
            event = Some(BrowseEvent::CloseSwitcher);
        }
        if i.key_pressed(egui::Key::Enter) {
            if let Some((path, _, _)) = rows.get(state.switcher_index) {
                event = Some(BrowseEvent::OpenFolder(path.clone()));
            }
        }
    });

    // A click anywhere else closes it, which is what every menu on the platform
    // does and therefore what the hand expects.
    // The click that opened the switcher must not also dismiss it.
    if event.is_none() && !state.switcher_just_opened && ctx.input(|i| i.pointer.any_click()) {
        let inside = ctx.input(|i| {
            i.pointer.interact_pos().map(|p| {
                Rect::from_min_size(anchor, Vec2::new(metric::SWITCHER_W, height)).contains(p)
            })
        });
        if inside == Some(false) {
            event = Some(BrowseEvent::CloseSwitcher);
        }
    }
    state.switcher_just_opened = false;

    event
}
