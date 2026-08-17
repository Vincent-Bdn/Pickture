//! Screen 01 — the folder picker.
//!
//! States what the tool is for in one line, then gets out of the way. On every
//! run after the first, the recent list is the fast path. No tour, no
//! onboarding, no illustration.

use egui::{Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::session::relative_age;
use pickture_kernel::SessionStore;
use pickture_ui_kit::mark;
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};

use crate::{folder_row, BrowseEvent, BrowseState, FolderRow};

const COLUMN_W: f32 = 720.0;
const GAP: f32 = 34.0;
const MARK: f32 = 46.0;
const ROW_H: f32 = 42.0;

pub fn folder_picker(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    store: &SessionStore,
    state: &mut BrowseState,
) -> Option<BrowseEvent> {
    paint::fill(ui.painter(), rect, theme.window);

    let recent: Vec<_> = store.recent.iter().take(6).cloned().collect();
    let mut event = None;

    // The column is centred horizontally and vertically in the body.
    let recent_h = if recent.is_empty() {
        0.0
    } else {
        24.0 + recent.len() as f32 * ROW_H
    };
    let content_h = MARK.max(58.0)
        + GAP
        + 40.0
        + if recent.is_empty() {
            0.0
        } else {
            GAP + recent_h
        };

    let col = Rect::from_center_size(
        rect.center(),
        Vec2::new(COLUMN_W.min(rect.width() - 80.0), content_h),
    );

    // ---- lockup ---------------------------------------------------------
    let mark_rect = Rect::from_min_size(Pos2::new(col.left(), col.top()), Vec2::splat(MARK));
    mark::draw(ui.painter(), theme, mark_rect);

    let text_x = mark_rect.right() + metric::S16;
    paint::text_left(
        ui.painter(),
        Pos2::new(text_x, col.top() + 15.0),
        "Pickture",
        tokens::sans_medium(size::SANS_XL),
        theme.fg,
    );

    // The one-liner, with `selection/` picked out in mono sodium because it is
    // the one piece of it that names something on disk.
    let line_y = col.top() + 41.0;
    let body_font = tokens::sans(size::SANS_M);
    let mono_font = tokens::mono(13.0);
    let mut x = text_x;
    let lead = "A thousand frames down to forty, in one sitting. Keepers are copied to ";
    x += paint::text_left(
        ui.painter(),
        Pos2::new(x, line_y),
        lead,
        body_font.clone(),
        theme.fg_secondary,
    )
    .width();
    x += paint::text_left(
        ui.painter(),
        Pos2::new(x, line_y),
        "selection/",
        mono_font,
        theme.sodium,
    )
    .width();
    paint::text_left(
        ui.painter(),
        Pos2::new(x, line_y),
        " — originals are never touched.",
        body_font,
        theme.fg_secondary,
    );

    // ---- action row -----------------------------------------------------
    let action_y = col.top() + MARK.max(58.0) + GAP;
    let btn_font = tokens::sans_medium(size::SANS_M);
    let btn_label = "Choose a folder";
    let btn_w = paint::text_width(ui.painter(), btn_label, &btn_font) + 40.0;
    let btn_rect = Rect::from_min_size(
        Pos2::new(col.left(), action_y),
        Vec2::new(btn_w, size::SANS_M + 24.0),
    );
    let btn = ui.interact(btn_rect, ui.id().with("pick-folder"), Sense::click());
    let btn_bg = if btn.hovered() {
        theme.sodium.gamma_multiply(0.88)
    } else {
        theme.sodium
    };
    paint::fill(ui.painter(), btn_rect, btn_bg);
    paint::text_center(
        ui.painter(),
        btn_rect.center(),
        btn_label,
        btn_font,
        theme.on_sodium,
    );
    if btn.clicked() {
        event = Some(BrowseEvent::BrowseForFolder);
    }
    if btn.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    paint::text_left(
        ui.painter(),
        Pos2::new(btn_rect.right() + metric::S12, btn_rect.center().y),
        "or drop one anywhere in this window",
        tokens::mono(size::MONO_M),
        theme.fg_muted,
    );

    // ---- recent ---------------------------------------------------------
    if !recent.is_empty() {
        let list_top = btn_rect.bottom() + GAP;
        crate::eyebrow(ui, theme, Pos2::new(col.left(), list_top), "Recent");

        state.recent_index = state.recent_index.min(recent.len().saturating_sub(1));

        for (i, path) in recent.iter().enumerate() {
            let row_rect = Rect::from_min_size(
                Pos2::new(col.left(), list_top + 16.0 + i as f32 * ROW_H),
                Vec2::new(col.width(), ROW_H),
            );
            let persisted = store.sessions.get(path);
            let meta = match persisted {
                Some(p) if p.last_opened_secs > 0 => format!(
                    "{} · {}",
                    store.progress_line(path),
                    relative_age(p.last_opened_secs)
                ),
                _ => store.progress_line(path),
            };
            let row = FolderRow {
                path: path.clone(),
                meta,
                is_current: i == state.recent_index,
                is_focused: i == state.recent_index,
            };
            if folder_row(ui, theme, row_rect, &row, false) {
                event = Some(BrowseEvent::OpenFolder(path.clone()));
            }
        }
    }

    event
}

/// Keyboard handling for the picker: `↑ ↓` move through recents, `↵` opens the
/// highlighted one, `O` goes to the OS picker.
pub fn picker_keys(ui: &Ui, store: &SessionStore, state: &mut BrowseState) -> Option<BrowseEvent> {
    let recent_len = store.recent.len().min(6);
    ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) && recent_len > 0 {
            state.recent_index = (state.recent_index + 1).min(recent_len - 1);
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            state.recent_index = state.recent_index.saturating_sub(1);
        }
        if i.key_pressed(egui::Key::O) {
            return Some(BrowseEvent::BrowseForFolder);
        }
        if i.key_pressed(egui::Key::Enter) && recent_len > 0 {
            return store
                .recent
                .get(state.recent_index)
                .cloned()
                .map(BrowseEvent::OpenFolder);
        }
        None
    })
}
