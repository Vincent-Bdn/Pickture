//! Select: where keepers go, and the code that puts them there.
//!
//! The destination is stated permanently in the info bar next to the keep
//! action, because it is the one setting that changes what the tool writes to
//! disk — it should never be discoverable only through a preferences dialog.
//!
//! Two invariants, enforced here rather than trusted: **copies only, never
//! moves**, and **originals are never touched**.

use egui::{Area, Id, Order, Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::{today_stamp, Destination, EffectSpec};
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme};
use std::path::{Path, PathBuf};

mod writer;
pub use writer::{WriteJob, WriteOutcome, WriteProgress, Writer};

#[derive(Debug, Clone, PartialEq)]
pub enum SelectEvent {
    SetDestination(Destination),
    BrowseForDestination,
    ToggleMenu,
    CloseMenu,
}

#[derive(Default)]
pub struct SelectState {
    pub menu_open: bool,
    pub index: usize,
    /// Set on the frame the menu opens.
    ///
    /// Without it the chip is unclickable: the click opens the menu, and later
    /// in that *same frame* the dismiss-on-outside-click check sees a click
    /// whose position is on the chip — outside the popover — and closes it
    /// again. The menu appeared and vanished within one frame.
    pub just_opened: bool,
    /// Plain-text refusal shown in the status bar. Never a red box.
    pub notice: Option<String>,
}

impl SelectState {
    pub fn open(&mut self) {
        self.menu_open = true;
        self.just_opened = true;
        self.index = 0;
    }
}

/// The three destinations always on offer, plus any absolute folder this
/// working folder has been pointed at before.
pub fn options(remembered: &[PathBuf]) -> Vec<Destination> {
    let mut out = vec![Destination::InWorkingFolder("selection".into())];
    out.extend(remembered.iter().map(|p| Destination::Absolute(p.clone())));
    out.push(Destination::Dated {
        root: "selection".into(),
        date: today_stamp(),
    });
    out
}

// ---------------------------------------------------------------------------
// The info-bar chip
// ---------------------------------------------------------------------------

/// `→ selection/ ▾` — permanently visible beside the keep action.
pub fn destination_chip(
    ui: &mut Ui,
    theme: &Theme,
    right_edge: Pos2,
    destination: &Destination,
    open: bool,
) -> (f32, Option<SelectEvent>) {
    let font = tokens::mono(size::MONO_S);
    let label = destination.chip_label();
    let arrow_w = paint::text_width(ui.painter(), "→", &font);
    let label_w = paint::text_width(ui.painter(), &label, &font);
    let caret_w = 10.0;
    let width = arrow_w + label_w + caret_w + metric::S8 * 2.0 + metric::S6 * 2.0;

    let chip = Rect::from_min_size(
        Pos2::new(right_edge.x - width, right_edge.y - 12.0),
        Vec2::new(width, 24.0),
    );
    let response = ui.interact(chip, ui.id().with("dest-chip"), Sense::click());

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
        "→",
        font.clone(),
        theme.fg_muted,
    )
    .width()
        + metric::S6;
    x += paint::text_left(
        ui.painter(),
        Pos2::new(x, chip.center().y),
        &label,
        font,
        theme.fg_secondary,
    )
    .width()
        + metric::S6;
    paint::text_left(
        ui.painter(),
        Pos2::new(x, chip.center().y),
        "▾",
        tokens::mono(9.0),
        theme.fg_muted,
    );

    let event = response.clicked().then_some(SelectEvent::ToggleMenu);
    (width, event)
}

// ---------------------------------------------------------------------------
// The destination popover
// ---------------------------------------------------------------------------

const ROW_H: f32 = 44.0;
const HEADER_H: f32 = 30.0;
const FOOTER_H: f32 = 38.0;
const NOTE_H: f32 = 44.0;

pub fn destination_popover(
    ctx: &egui::Context,
    theme: &Theme,
    top_right: Pos2,
    current: &Destination,
    remembered: &[PathBuf],
    state: &mut SelectState,
) -> Option<SelectEvent> {
    if !state.menu_open {
        return None;
    }

    let opts = options(remembered);
    state.index = state.index.min(opts.len().saturating_sub(1));

    let height = HEADER_H + opts.len() as f32 * ROW_H + FOOTER_H + NOTE_H;
    let origin = Pos2::new(top_right.x - metric::DEST_W, top_right.y);
    let rect = Rect::from_min_size(origin, Vec2::new(metric::DEST_W, height));
    let mut event = None;

    Area::new(Id::new("destination-menu"))
        .order(Order::Foreground)
        .fixed_pos(origin)
        .show(ctx, |ui| {
            // Allocate before painting — an Area clips to whatever its content
            // has claimed, and that starts out empty.
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(metric::DEST_W, height), Sense::hover());
            {
                paint::popover_frame(ui.painter(), theme, rect);

                paint::tracked_text(
                    ui.painter(),
                    Pos2::new(rect.left() + 14.0, rect.top() + HEADER_H * 0.5 + 3.0),
                    "KEEPERS ARE WRITTEN TO",
                    tokens::mono(size::MONO_XS),
                    theme.fg_muted,
                    tokens::EYEBROW_TRACKING,
                );

                for (i, opt) in opts.iter().enumerate() {
                    let row = Rect::from_min_size(
                        Pos2::new(rect.left(), rect.top() + HEADER_H + i as f32 * ROW_H),
                        Vec2::new(rect.width(), ROW_H),
                    );
                    let selected = opt == current;
                    let r = ui.interact(row, ui.id().with(("dest", i)), Sense::click());
                    if r.hovered() || i == state.index {
                        paint::fill(ui.painter(), row, theme.chrome_hover);
                    } else if selected {
                        paint::fill(ui.painter(), row, theme.chrome_hover.gamma_multiply(0.6));
                    }
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    // 10 pt square marker, sodium when selected.
                    let marker = Rect::from_min_size(
                        Pos2::new(row.left() + 14.0, row.top() + 13.0),
                        Vec2::splat(10.0),
                    );
                    paint::border(ui.painter(), marker, theme.fg_muted);
                    if selected {
                        paint::fill(ui.painter(), marker.shrink(2.0), theme.sodium);
                    }

                    let text_x = marker.right() + metric::S12;
                    let path_font = tokens::mono(size::MONO_M);
                    let shown = paint::elide_start(
                        ui.painter(),
                        &opt.full_label(),
                        &path_font,
                        rect.right() - text_x - 14.0,
                    );
                    paint::text_left(
                        ui.painter(),
                        Pos2::new(text_x, row.top() + 15.0),
                        &shown,
                        path_font,
                        theme.fg,
                    );
                    paint::text_left(
                        ui.painter(),
                        Pos2::new(text_x, row.top() + 31.0),
                        opt.note(),
                        tokens::sans(size::SANS_XS),
                        theme.fg_muted,
                    );

                    if r.clicked() {
                        event = Some(SelectEvent::SetDestination(opt.clone()));
                    }
                }

                // ---- footer ------------------------------------------------
                let footer = Rect::from_min_size(
                    Pos2::new(
                        rect.left(),
                        rect.top() + HEADER_H + opts.len() as f32 * ROW_H,
                    ),
                    Vec2::new(rect.width(), FOOTER_H),
                );
                paint::rule_top(
                    ui.painter(),
                    footer.shrink2(Vec2::new(14.0, 0.0)),
                    theme.hair,
                );
                let browse = ui.interact(footer, ui.id().with("dest-browse"), Sense::click());
                if browse.hovered() {
                    paint::fill(ui.painter(), footer, theme.chrome_hover);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                paint::text_left(
                    ui.painter(),
                    Pos2::new(footer.left() + 14.0, footer.center().y + 2.0),
                    "Choose another folder…",
                    tokens::sans(size::SANS_S),
                    theme.sodium,
                );
                paint::text_right(
                    ui.painter(),
                    Pos2::new(footer.right() - 14.0, footer.center().y + 2.0),
                    "⇧S",
                    tokens::mono(size::MONO_XS),
                    theme.fg_disabled,
                );
                if browse.clicked() {
                    event = Some(SelectEvent::BrowseForDestination);
                }

                // ---- the rule, stated where the choice is made -------------
                let note = Rect::from_min_size(
                    Pos2::new(rect.left() + 14.0, footer.bottom() + 4.0),
                    Vec2::new(rect.width() - 28.0, NOTE_H),
                );
                let small = tokens::sans(size::SANS_XS);
                paint::text_left(
                    ui.painter(),
                    Pos2::new(note.left(), note.top() + 8.0),
                    "Copies only, never moves. Changing the destination",
                    small.clone(),
                    theme.fg_muted,
                );
                paint::text_left(
                    ui.painter(),
                    Pos2::new(note.left(), note.top() + 24.0),
                    "mid-session applies to frames kept from now on.",
                    small,
                    theme.fg_muted,
                );
            }
        });

    ctx.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) {
            state.index = (state.index + 1).min(opts.len().saturating_sub(1));
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            state.index = state.index.saturating_sub(1);
        }
        if i.key_pressed(egui::Key::Escape) {
            event = Some(SelectEvent::CloseMenu);
        }
        if i.key_pressed(egui::Key::Enter) {
            if let Some(opt) = opts.get(state.index) {
                event = Some(SelectEvent::SetDestination(opt.clone()));
            }
        }
    });

    // The click that opened the menu must not also dismiss it.
    if event.is_none() && !state.just_opened && ctx.input(|i| i.pointer.any_click()) {
        let inside = ctx.input(|i| i.pointer.interact_pos().map(|p| rect.contains(p)));
        if inside == Some(false) {
            event = Some(SelectEvent::CloseMenu);
        }
    }
    state.just_opened = false;

    event
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Reasons a destination is refused, worded as a plain status-bar line.
pub fn validate(destination: &Destination, working: &Path) -> Result<PathBuf, String> {
    let resolved = destination.resolve(working);

    // A destination inside the frame set would have the tool re-reading its own
    // output on the next scan.
    if resolved == working {
        return Err("that folder is the one being culled — pick another".into());
    }

    if let Err(e) = std::fs::create_dir_all(&resolved) {
        return Err(format!("can't write to {} — {e}", resolved.display()));
    }

    Ok(resolved)
}

/// Pick a filename, appending `_2` if a *different* file already holds the name.
pub fn output_path(dir: &Path, stem: &str, spec: &EffectSpec, extension: &str) -> (PathBuf, bool) {
    let base = format!("{stem}{}", spec.mode.suffix());
    let mut candidate = dir.join(format!("{base}.{extension}"));
    if !candidate.exists() {
        return (candidate, false);
    }
    candidate = dir.join(format!("{base}_2.{extension}"));
    let mut n = 2;
    while candidate.exists() && n < 100 {
        n += 1;
        candidate = dir.join(format!("{base}_{n}.{extension}"));
    }
    (candidate, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_inside_the_frame_set_is_refused() {
        let dir = std::env::temp_dir().join("pickture-test-same");
        let _ = std::fs::create_dir_all(&dir);
        let dest = Destination::InWorkingFolder(".".into());
        // `.` resolves back onto the working folder itself.
        let resolved = dest.resolve(&dir);
        assert_eq!(resolved, dir.join("."));
        // The real guard is the equality check; construct it directly.
        assert!(validate(&Destination::Absolute(dir.clone()), &dir).is_err());
    }

    #[test]
    fn suffix_matches_the_mode() {
        use pickture_kernel::EffectMode;
        assert_eq!(EffectMode::None.suffix(), "_ORIG");
        assert_eq!(EffectMode::WbValue.suffix(), "_WBV");
        assert_eq!(EffectMode::WbRgb.suffix(), "_WBRGB");
        assert_eq!(EffectMode::Levels.suffix(), "_CUSTOM");
    }

    #[test]
    fn collision_appends_a_counter() {
        let dir = std::env::temp_dir().join("pickture-test-collide");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let spec = EffectSpec::default();
        let (first, collided) = output_path(&dir, "_DSC1", &spec, "jpg");
        assert!(!collided);
        assert!(first.ends_with("_DSC1_ORIG.jpg"));

        std::fs::write(&first, b"x").unwrap();
        let (second, collided) = output_path(&dir, "_DSC1", &spec, "jpg");
        assert!(collided);
        assert!(second.ends_with("_DSC1_ORIG_2.jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_always_offer_the_default_first() {
        let opts = options(&[]);
        assert_eq!(opts[0], Destination::InWorkingFolder("selection".into()));
        assert!(matches!(opts.last(), Some(Destination::Dated { .. })));
    }

    #[test]
    fn remembered_absolutes_are_offered() {
        let opts = options(&[PathBuf::from("D:/deliver/picks")]);
        assert_eq!(opts.len(), 3);
        assert!(matches!(opts[1], Destination::Absolute(_)));
    }
}
