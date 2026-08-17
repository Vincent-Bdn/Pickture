//! Enhance: the confirm modal, and the levels instrument inside it.
//!
//! Two requirements from the brief are load-bearing here and neither is
//! negotiable:
//!
//! * **There is no Apply button.** Dragging any handle updates the preview
//!   immediately, computed on a downscaled proxy. The MAUI build recomputed at
//!   full resolution behind an *Apply* press, which is why that control felt
//!   like operating machinery.
//! * **Levels are never typed-only.** The handles on the histogram are the
//!   primary instrument — levels are a spatial idea, and typing `187` into a box
//!   is the wrong tool — with numeric readouts always visible and exact entry
//!   always available.

use egui::{Key, Pos2, Rect, Sense, Ui, Vec2};
use pickture_kernel::{EffectMode, EffectSpec, ANGLE_LIMIT, GAMMA_MAX, GAMMA_MIN};
use pickture_ui_kit::paint;
use pickture_ui_kit::tokens::{self, metric, size, Theme, EYEBROW_TRACKING};

pub mod histogram;
pub use histogram::Histogram;

#[derive(Debug, Clone, PartialEq)]
pub enum EnhanceEvent {
    /// The spec changed — recompute the proxy.
    SpecChanged(EffectSpec),
    Cancel,
    Confirm,
}

/// Which handle a drag is currently moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Black,
    Gamma,
    White,
    Angle,
}

#[derive(Default)]
pub struct EnhanceState {
    pub open: bool,
    pub drag: Option<Handle>,
    /// Set when a readout is being edited by keyboard after a double-click.
    pub editing: Option<(Handle, String)>,
    /// 0.0..=1.0 while a full-resolution write is running.
    pub write_progress: Option<f32>,
}

impl EnhanceState {
    pub fn close(&mut self) {
        self.open = false;
        self.drag = None;
        self.editing = None;
        self.write_progress = None;
    }
}

const PANEL_PAD: f32 = 18.0;
const GROUP_GAP: f32 = 20.0;
const SEGMENT_H: f32 = 30.0;
const READOUT_H: f32 = 44.0;
const ROT_H: f32 = 30.0;

/// The control panel. Returns an event when something changed.
#[allow(clippy::too_many_arguments)]
pub fn control_panel(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    spec: &EffectSpec,
    hist: &Histogram,
    state: &mut EnhanceState,
) -> Option<EnhanceEvent> {
    paint::fill(ui.painter(), rect, theme.rail);
    paint::rule_left(ui.painter(), rect, theme.hair);

    let inner = rect.shrink(PANEL_PAD);
    let mut next = *spec;
    let mut changed = false;
    let mut event = None;
    let mut y = inner.top();

    // ---- 1 · effect selector -------------------------------------------
    paint::tracked_text(
        ui.painter(),
        Pos2::new(inner.left(), y + 5.0),
        "EFFECT",
        tokens::mono(size::MONO_XS),
        theme.fg_muted,
        EYEBROW_TRACKING,
    );
    y += 18.0;

    let seg_rect = Rect::from_min_size(
        Pos2::new(inner.left(), y),
        Vec2::new(inner.width(), SEGMENT_H),
    );
    if let Some(mode) = segmented(ui, theme, seg_rect, spec.mode) {
        next.mode = mode;
        changed = true;
    }
    y = seg_rect.bottom() + GROUP_GAP;

    // ---- 2 · histogram --------------------------------------------------
    paint::tracked_text(
        ui.painter(),
        Pos2::new(inner.left(), y + 5.0),
        "LEVELS · LUMINANCE",
        tokens::mono(size::MONO_XS),
        theme.fg_muted,
        EYEBROW_TRACKING,
    );
    paint::text_right(
        ui.painter(),
        Pos2::new(inner.right(), y + 5.0),
        if spec.is_clipping() {
            "clipping"
        } else {
            "no clipping"
        },
        tokens::mono(size::MONO_XS),
        theme.sodium,
    );
    y += 18.0;

    let hist_rect = Rect::from_min_size(
        Pos2::new(inner.left(), y),
        Vec2::new(inner.width(), metric::HISTOGRAM_H),
    );
    if histogram::draw(ui, theme, hist_rect, hist, &mut next, state) {
        changed = true;
    }
    y = hist_rect.bottom() + metric::S12;

    // ---- 3 · readouts ---------------------------------------------------
    let gap = metric::S8;
    let cell_w = (inner.width() - gap * 2.0) / 3.0;
    let readouts = [
        (Handle::Black, "BLACK", format!("{}", next.low), theme.fg),
        (
            Handle::Gamma,
            "GAMMA",
            format!("{:.2}", next.gamma),
            theme.sodium,
        ),
        (Handle::White, "WHITE", format!("{}", next.high), theme.fg),
    ];
    for (i, (handle, label, value, marker)) in readouts.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(inner.left() + i as f32 * (cell_w + gap), y),
            Vec2::new(cell_w, READOUT_H),
        );
        if readout(
            ui, theme, cell, *handle, label, value, *marker, &mut next, state,
        ) {
            changed = true;
        }
    }
    y += READOUT_H + metric::S8;

    paint::text_left(
        ui.painter(),
        Pos2::new(inner.left(), y + 8.0),
        "Drag on the histogram or on a readout.",
        tokens::sans(size::SANS_XS),
        theme.fg_muted,
    );
    paint::text_left(
        ui.painter(),
        Pos2::new(inner.left(), y + 24.0),
        "Shift halves the step.",
        tokens::sans(size::SANS_XS),
        theme.fg_muted,
    );
    y += 32.0 + GROUP_GAP;

    // ---- 4 · rotation ---------------------------------------------------
    paint::tracked_text(
        ui.painter(),
        Pos2::new(inner.left(), y + 5.0),
        "ROTATION",
        tokens::mono(size::MONO_XS),
        theme.fg_muted,
        EYEBROW_TRACKING,
    );
    y += 18.0;

    if rotation_row(
        ui,
        theme,
        Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(inner.width(), ROT_H)),
        &mut next,
        state,
    ) {
        changed = true;
    }

    // ---- 5 · progress and actions ---------------------------------------
    let actions_h = 42.0;
    let progress_h = 22.0;
    let bottom = inner.bottom();

    if let Some(fraction) = state.write_progress {
        let track = Rect::from_min_size(
            Pos2::new(inner.left(), bottom - actions_h - progress_h),
            Vec2::new(inner.width(), metric::RAIL),
        );
        paint::progress(ui.painter(), theme, track, fraction);
        paint::text_left(
            ui.painter(),
            Pos2::new(inner.left(), track.bottom() + 9.0),
            &format!(
                "writing full resolution · {}%",
                (fraction * 100.0).round() as i32
            ),
            tokens::mono(size::MONO_XS),
            theme.fg_muted,
        );
    }

    let actions = Rect::from_min_size(
        Pos2::new(inner.left(), bottom - actions_h),
        Vec2::new(inner.width(), actions_h),
    );
    if let Some(e) = action_buttons(ui, theme, actions, state.write_progress.is_some()) {
        event = Some(e);
    }

    if changed && event.is_none() {
        event = Some(EnhanceEvent::SpecChanged(next));
    } else if changed {
        // A confirm and a change in the same frame: the change still matters,
        // because it is what gets written.
        return Some(EnhanceEvent::SpecChanged(next));
    }

    event
}

// ---------------------------------------------------------------------------
// Segmented control
// ---------------------------------------------------------------------------

/// A segmented control, not three buttons — the four modes are mutually
/// exclusive, and a row of buttons would not say so.
fn segmented(ui: &mut Ui, theme: &Theme, rect: Rect, current: EffectMode) -> Option<EffectMode> {
    paint::border(ui.painter(), rect, theme.hair);

    let modes = [
        EffectMode::None,
        EffectMode::WbValue,
        EffectMode::WbRgb,
        EffectMode::Levels,
    ];
    let seg_w = rect.width() / modes.len() as f32;
    let mut picked = None;

    for (i, mode) in modes.iter().enumerate() {
        let seg = Rect::from_min_size(
            Pos2::new(rect.left() + i as f32 * seg_w, rect.top()),
            Vec2::new(seg_w, rect.height()),
        );
        let response = ui.interact(seg, ui.id().with(("segment", i)), Sense::click());
        let selected = *mode == current;

        if selected {
            paint::fill(ui.painter(), seg, theme.sodium);
        } else if response.hovered() {
            paint::fill(ui.painter(), seg, theme.chrome_hover);
        }
        if i > 0 {
            paint::rule_left(ui.painter(), seg, theme.hair);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        paint::text_center(
            ui.painter(),
            seg.center(),
            mode.label(),
            tokens::sans(size::SANS_S),
            if selected {
                theme.on_sodium
            } else {
                theme.fg_secondary
            },
        );

        if response.clicked() {
            picked = Some(*mode);
        }
    }

    picked
}

// ---------------------------------------------------------------------------
// Readouts
// ---------------------------------------------------------------------------

/// A readout is also a drag target, and double-clicking it types an exact value.
#[allow(clippy::too_many_arguments)]
fn readout(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    handle: Handle,
    label: &str,
    value: &str,
    marker: egui::Color32,
    spec: &mut EffectSpec,
    state: &mut EnhanceState,
) -> bool {
    paint::fill(ui.painter(), rect, theme.chrome);
    // A 3 pt top border marks which handle this cell belongs to.
    let (top, _) = paint::split_top(rect, metric::RAIL);
    paint::fill(ui.painter(), top, marker);

    let response = ui.interact(
        rect,
        ui.id().with(("readout", label)),
        Sense::click_and_drag(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    paint::text_left(
        ui.painter(),
        Pos2::new(rect.left() + metric::S8, rect.top() + 15.0),
        label,
        tokens::mono(size::MONO_XS),
        theme.fg_muted,
    );

    // Editing state takes over the value display.
    if let Some((editing_handle, buffer)) = &state.editing {
        if *editing_handle == handle {
            paint::text_left(
                ui.painter(),
                Pos2::new(rect.left() + metric::S8, rect.top() + 32.0),
                &format!("{buffer}▌"),
                tokens::mono(size::MONO_L),
                theme.sodium,
            );
            return commit_editing(ui, spec, state);
        }
    }

    paint::text_left(
        ui.painter(),
        Pos2::new(rect.left() + metric::S8, rect.top() + 32.0),
        value,
        tokens::mono(size::MONO_L),
        theme.fg,
    );

    if response.double_clicked() {
        state.editing = Some((handle, String::new()));
        return false;
    }

    if response.dragged() {
        let shift = ui.input(|i| i.modifiers.shift);
        let step = if shift { 0.5 } else { 1.0 };
        let dx = response.drag_delta().x * step;
        return nudge(spec, handle, dx);
    }

    false
}

/// Keyboard entry after a double-click.
fn commit_editing(ui: &Ui, spec: &mut EffectSpec, state: &mut EnhanceState) -> bool {
    let mut changed = false;
    let mut finish = false;

    ui.input(|i| {
        if let Some((handle, buffer)) = &mut state.editing {
            for event in &i.events {
                match event {
                    egui::Event::Text(t) => {
                        for ch in t.chars() {
                            if ch.is_ascii_digit() || ch == '.' {
                                buffer.push(ch);
                            }
                        }
                    }
                    egui::Event::Key {
                        key: Key::Backspace,
                        pressed: true,
                        ..
                    } => {
                        buffer.pop();
                    }
                    egui::Event::Key {
                        key: Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        if let Ok(v) = buffer.parse::<f32>() {
                            match handle {
                                Handle::Black => spec.set_low(v.clamp(0.0, 255.0) as u8),
                                Handle::White => spec.set_high(v.clamp(0.0, 255.0) as u8),
                                Handle::Gamma => spec.set_gamma(v),
                                Handle::Angle => spec.set_angle(v),
                            }
                            changed = true;
                        }
                        finish = true;
                    }
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        ..
                    } => finish = true,
                    _ => {}
                }
            }
        }
    });

    if finish {
        state.editing = None;
    }
    changed
}

/// Move a handle by a pixel delta, in that handle's own units.
fn nudge(spec: &mut EffectSpec, handle: Handle, dx: f32) -> bool {
    if dx == 0.0 {
        return false;
    }
    match handle {
        Handle::Black => {
            let v = (spec.low as f32 + dx).clamp(0.0, 255.0) as u8;
            spec.set_low(v);
        }
        Handle::White => {
            let v = (spec.high as f32 + dx).clamp(0.0, 255.0) as u8;
            spec.set_high(v);
        }
        Handle::Gamma => {
            // The gamma range is 2.2 wide across 255 px of travel.
            spec.set_gamma(spec.gamma + dx * (GAMMA_MAX - GAMMA_MIN) / 255.0);
        }
        Handle::Angle => {
            spec.set_angle(spec.angle + dx * 0.05);
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

fn rotation_row(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    spec: &mut EffectSpec,
    state: &mut EnhanceState,
) -> bool {
    let mut changed = false;
    let btn = Vec2::new(34.0, ROT_H);

    let left = Rect::from_min_size(rect.min, btn);
    if hairline_button(ui, theme, left, "↺", "rot-l") {
        spec.quarter_turns -= 1;
        changed = true;
    }

    let right = Rect::from_min_size(Pos2::new(left.right() + metric::S8, rect.top()), btn);
    if hairline_button(ui, theme, right, "↻", "rot-r") {
        spec.quarter_turns += 1;
        changed = true;
    }

    // Fine-angle track.
    let readout_w = 56.0;
    let track = Rect::from_min_max(
        Pos2::new(right.right() + metric::S8, rect.top()),
        Pos2::new(rect.right() - readout_w - metric::S8, rect.bottom()),
    );
    paint::fill(ui.painter(), track, theme.chrome);
    paint::border(ui.painter(), track, theme.hair);

    let response = ui.interact(track, ui.id().with("angle-track"), Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if response.drag_started() {
        state.drag = Some(Handle::Angle);
    }
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
            let raw = t * (ANGLE_LIMIT * 2.0) - ANGLE_LIMIT;
            let shift = ui.input(|i| i.modifiers.shift);
            let snapped = if shift {
                (raw * 20.0).round() / 20.0
            } else {
                (raw * 10.0).round() / 10.0
            };
            spec.set_angle(snapped);
            changed = true;
        }
    }
    if response.drag_stopped() {
        state.drag = None;
    }

    // 2 pt sodium indicator.
    let t = (spec.angle + ANGLE_LIMIT) / (ANGLE_LIMIT * 2.0);
    let x = track.left() + track.width() * t.clamp(0.0, 1.0);
    paint::fill(
        ui.painter(),
        Rect::from_min_size(
            Pos2::new(x - 1.0, track.top()),
            Vec2::new(2.0, track.height()),
        ),
        theme.sodium,
    );

    paint::text_right(
        ui.painter(),
        Pos2::new(rect.right(), rect.center().y),
        &format!(
            "{}{:.1}°",
            if spec.angle > 0.0 { "+" } else { "" },
            spec.angle
        ),
        tokens::mono(13.0),
        theme.fg,
    );

    changed
}

fn hairline_button(ui: &mut Ui, theme: &Theme, rect: Rect, glyph: &str, id: &str) -> bool {
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    if response.hovered() {
        paint::fill(ui.painter(), rect, theme.chrome_hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint::border(ui.painter(), rect, theme.hair);
    paint::text_center(
        ui.painter(),
        rect.center(),
        glyph,
        tokens::sans(13.0),
        theme.fg,
    );
    response.clicked()
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn action_buttons(ui: &mut Ui, theme: &Theme, rect: Rect, busy: bool) -> Option<EnhanceEvent> {
    let mut event = None;
    let font = tokens::sans(13.0);
    let key_font = tokens::mono(size::MONO_S);

    let confirm_w = paint::text_width(ui.painter(), "Confirm", &font) + 58.0;
    let cancel_w = paint::text_width(ui.painter(), "Cancel", &font) + 54.0;

    let confirm = Rect::from_min_size(
        Pos2::new(rect.right() - confirm_w, rect.top()),
        Vec2::new(confirm_w, rect.height()),
    );
    let cancel = Rect::from_min_size(
        Pos2::new(confirm.left() - metric::S8 - cancel_w, rect.top()),
        Vec2::new(cancel_w, rect.height()),
    );

    // -- cancel --
    let c = ui.interact(cancel, ui.id().with("modal-cancel"), Sense::click());
    if c.hovered() {
        paint::fill(ui.painter(), cancel, theme.chrome_hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint::border(ui.painter(), cancel, theme.hair);
    let cx = cancel.left() + 16.0;
    let w = paint::text_left(
        ui.painter(),
        Pos2::new(cx, cancel.center().y),
        "Cancel",
        font.clone(),
        theme.fg_secondary,
    )
    .width();
    paint::text_left(
        ui.painter(),
        Pos2::new(cx + w + metric::S8, cancel.center().y),
        "esc",
        key_font.clone(),
        theme.fg_disabled,
    );
    if c.clicked() {
        event = Some(EnhanceEvent::Cancel);
    }

    // -- confirm --
    let k = ui.interact(confirm, ui.id().with("modal-confirm"), Sense::click());
    let bg = if busy {
        theme.sodium.gamma_multiply(0.5)
    } else if k.hovered() {
        theme.sodium.gamma_multiply(0.88)
    } else {
        theme.sodium
    };
    paint::fill(ui.painter(), confirm, bg);
    if k.hovered() && !busy {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let kx = confirm.left() + 16.0;
    let w = paint::text_left(
        ui.painter(),
        Pos2::new(kx, confirm.center().y),
        "Confirm",
        tokens::sans_medium(13.0),
        theme.on_sodium,
    )
    .width();
    paint::text_left(
        ui.painter(),
        Pos2::new(kx + w + metric::S8, confirm.center().y),
        "↵",
        key_font,
        theme.on_sodium,
    );
    if k.clicked() && !busy {
        event = Some(EnhanceEvent::Confirm);
    }

    event
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

pub fn modal_header(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    filename: &str,
) -> Option<EnhanceEvent> {
    paint::fill(ui.painter(), rect, theme.chrome);
    paint::rule_bottom(ui.painter(), rect, theme.hair);

    let x = rect.left() + 18.0;
    let w = paint::text_left(
        ui.painter(),
        Pos2::new(x, rect.center().y),
        "Confirm image selection",
        tokens::sans_medium(size::SANS_M),
        theme.fg,
    )
    .width();
    paint::text_left(
        ui.painter(),
        Pos2::new(x + w + metric::S8, rect.center().y),
        &format!("· {filename}"),
        tokens::mono(size::MONO_M),
        theme.fg_muted,
    );

    let close = Rect::from_center_size(
        Pos2::new(rect.right() - 26.0, rect.center().y),
        Vec2::splat(28.0),
    );
    let response = ui.interact(close, ui.id().with("modal-close"), Sense::click());
    let hovered = response.hovered();
    if hovered {
        paint::fill(ui.painter(), close, theme.chrome_hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint::cross(
        ui.painter(),
        close.center(),
        5.0,
        if hovered { theme.fg } else { theme.fg_muted },
    );

    response.clicked().then_some(EnhanceEvent::Cancel)
}
