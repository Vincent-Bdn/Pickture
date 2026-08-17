//! Browse: the folder picker, the filmstrip, and the folder switcher.
//!
//! Owns everything about *which* frames you are looking at and *where* you are
//! in them. Knows nothing about the canvas, the effects, or the write path.

use egui::{Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::{Judgement, Session, SessionStore};
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme, EYEBROW_TRACKING};
use pickture_ui_kit::{mark, TextureStore};
use std::path::PathBuf;

pub mod filmstrip;
pub mod picker;
pub mod switcher;

pub use filmstrip::filmstrip;
pub use picker::folder_picker;
pub use switcher::{folder_switcher_popover, switcher_anchor};

/// What the browse surfaces ask the composition root to do. Slices never call
/// each other — they return intent, and `app` routes it.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseEvent {
    /// Open a folder without tearing anything down.
    OpenFolder(PathBuf),
    /// Show the OS picker.
    BrowseForFolder,
    /// Move the cursor to an absolute index.
    SelectFrame(usize),
    ToggleSwitcher,
    CloseSwitcher,
}

/// UI state owned by this slice.
#[derive(Default)]
pub struct BrowseState {
    pub switcher_open: bool,
    /// Set on the frame the switcher opens, so the click that opened it is not
    /// also read as a click outside it. See `SelectState::just_opened`.
    pub switcher_just_opened: bool,
    /// Highlighted row while the switcher is being driven from the keyboard.
    pub switcher_index: usize,
    /// Highlighted row in the folder picker's recent list.
    pub recent_index: usize,
    /// Set when the cursor moves, so the strip scrolls the new frame into view
    /// exactly once rather than fighting the user's own scrolling every frame.
    pub scroll_to_cursor: bool,
}

impl BrowseState {
    pub fn close_popovers(&mut self) {
        self.switcher_open = false;
        self.switcher_just_opened = false;
    }

    pub fn open_switcher(&mut self) {
        self.switcher_open = true;
        self.switcher_just_opened = true;
        self.switcher_index = 0;
    }
}

// ---------------------------------------------------------------------------
// Shared cell geometry
// ---------------------------------------------------------------------------

/// Width available to the thumbnail inside a filmstrip cell.
///
/// The rail (3) and its gap (8) are subtracted from the 190 pt content width,
/// which itself sits inside the 214 pt strip with 12 pt of padding each side.
pub const CELL_CONTENT_W: f32 = metric::THUMB_W - metric::RAIL - metric::S8;
const LABEL_H: f32 = 14.0;
const LABEL_GAP: f32 = 5.0;
const CELL_PAD_X: f32 = 12.0;

/// Clamp on cell height so a panorama or an extreme portrait cannot take over
/// the strip. Aspect is still honoured inside these bounds.
const MIN_THUMB_H: f32 = 56.0;
const MAX_THUMB_H: f32 = 232.0;

/// Height of a cell's thumbnail, from the frame's real aspect ratio.
///
/// Read from the file header during the scan, never inferred once the decode
/// lands — a portrait resolving late would shift every cell beneath it, and the
/// design requires the pending placeholder to already be at its final size.
pub fn thumb_height(aspect: f32) -> f32 {
    let a = if aspect.is_finite() && aspect > 0.01 {
        aspect
    } else {
        1.5
    };
    (CELL_CONTENT_W / a).clamp(MIN_THUMB_H, MAX_THUMB_H).round()
}

pub fn cell_height(aspect: f32) -> f32 {
    thumb_height(aspect) + LABEL_GAP + LABEL_H
}

fn frame_aspect(frame: &pickture_kernel::Frame, textures: &TextureStore) -> f32 {
    if let Some((w, h)) = frame.dimensions {
        if h > 0 {
            return w as f32 / h as f32;
        }
    }
    // Falls back to the decoded thumbnail, then to 3:2.
    textures
        .peek_thumb(&frame.path)
        .map(|t| t.aspect())
        .unwrap_or(1.5)
}

// ---------------------------------------------------------------------------
// Small shared pieces
// ---------------------------------------------------------------------------

/// Uppercase tracked label used above every group.
pub fn eyebrow(ui: &Ui, theme: &Theme, pos: Pos2, text: &str) -> f32 {
    paint::tracked_text(
        ui.painter(),
        pos,
        &text.to_uppercase(),
        tokens::mono(size::MONO_XS),
        theme.fg_muted,
        EYEBROW_TRACKING,
    )
}

/// The lockup used on the picker screen and in the title bar.
pub fn lockup(ui: &Ui, theme: &Theme, rect: Rect, mark_size: f32) {
    let mark_rect = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - mark_size * 0.5),
        Vec2::splat(mark_size),
    );
    mark::draw(ui.painter(), theme, mark_rect);
}

// ---------------------------------------------------------------------------
// Row helper shared by the picker's recent list and the switcher
// ---------------------------------------------------------------------------

pub(crate) struct FolderRow {
    pub path: PathBuf,
    pub meta: String,
    pub is_current: bool,
    pub is_focused: bool,
}

/// One folder row: a 3 pt sodium left border when current, chrome ground when
/// focused, path over progress.
pub(crate) fn folder_row(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    row: &FolderRow,
    two_line: bool,
) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("folder-row", &row.path)),
        Sense::click(),
    );
    let hovered = response.hovered();

    if row.is_focused || hovered {
        paint::fill(ui.painter(), rect, theme.chrome_hover);
    } else if row.is_current {
        paint::fill(ui.painter(), rect, theme.chrome);
    }

    let (rail, _) = paint::split_left(rect, metric::RAIL);
    if row.is_current {
        paint::fill(ui.painter(), rail, theme.sodium);
    }

    let text_left = rect.left() + metric::RAIL + metric::S12;
    let text_right = rect.right() - metric::S12;
    let path_font = tokens::mono(size::MONO_M);
    let meta_font = tokens::mono(size::MONO_XS);

    let path_str = row.path.display().to_string();
    let fg = if row.is_current {
        theme.fg
    } else {
        theme.fg_secondary
    };

    if two_line {
        let avail = text_right - text_left;
        let shown = paint::elide_start(ui.painter(), &path_str, &path_font, avail);
        paint::text_left(
            ui.painter(),
            Pos2::new(text_left, rect.top() + 15.0),
            &shown,
            path_font,
            fg,
        );
        paint::text_left(
            ui.painter(),
            Pos2::new(text_left, rect.top() + 31.0),
            &row.meta,
            meta_font,
            theme.fg_muted,
        );
    } else {
        let meta_w = paint::text_width(ui.painter(), &row.meta, &meta_font);
        let avail = (text_right - meta_w - metric::S16) - text_left;
        let shown = paint::elide_start(ui.painter(), &path_str, &path_font, avail.max(40.0));
        paint::text_left(
            ui.painter(),
            Pos2::new(text_left, rect.center().y),
            &shown,
            path_font,
            fg,
        );
        paint::text_right(
            ui.painter(),
            Pos2::new(text_right, rect.center().y),
            &row.meta,
            meta_font,
            theme.fg_muted,
        );
    }

    response.clicked()
}

/// Rows offered by the switcher: every folder with a session, most recent
/// first, with the active one marked.
pub fn switcher_rows(
    store: &SessionStore,
    active: Option<&std::path::Path>,
) -> Vec<(PathBuf, String, bool)> {
    store
        .recent
        .iter()
        .map(|p| {
            let meta = store.progress_line(p);
            let is_current = active.map(|a| a == p.as_path()).unwrap_or(false);
            (p.clone(), meta, is_current)
        })
        .collect()
}

/// Counts for the status bar, computed here because this slice owns the strip.
pub fn counts(session: &Session) -> (usize, usize, usize, usize) {
    (
        session.cursor.saturating_add(1).min(session.frames.len()),
        session.frames.len(),
        session.kept_count(),
        session.passed_count(),
    )
}

pub(crate) fn judgement_label(
    session: &Session,
    frame: &pickture_kernel::Frame,
) -> (&'static str, bool) {
    if session.is_in_destination(&frame.id) {
        return ("KEPT", true);
    }
    match session.judgement_of(&frame.id) {
        Some(Judgement::Kept) => ("KEPT", true),
        Some(Judgement::Passed) => ("passed", false),
        None => ("—", false),
    }
}
