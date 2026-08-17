//! The composition root.
//!
//! Slices never call each other. Each returns an intent, and this module routes
//! it — which is what keeps `slice_browse` from knowing the write path exists,
//! and `slice_enhance` from knowing there is a filmstrip.

use egui::{Context, Key, Pos2, Rect, Vec2};
use image::RgbaImage;
use pickture_kernel::jobs::{
    worker_split, DecodeKind, DecodeOutcome, DecodeRequest, ImageLoader, ScanLoader, ScanOutcome,
};
use pickture_kernel::{
    paths, pixel_ops, session::Session, supported_label, Destination, EffectSpec, Judgement,
    SessionStore,
};
use pickture_slice_browse::{
    filmstrip::AckState, folder_switcher_popover, picker, switcher_anchor, BrowseEvent, BrowseState,
};
use pickture_slice_enhance::{control_panel, modal_header, EnhanceEvent, EnhanceState, Histogram};
use pickture_slice_select::{
    destination_popover, validate, SelectEvent, SelectState, WriteJob, WriteProgress, Writer,
};
use pickture_slice_view::{canvas, CanvasContent, Geometry};
use pickture_ui_kit::tokens::{metric, Mode, Theme};
use pickture_ui_kit::{paint, TextureStore};
use std::path::PathBuf;

use crate::chrome::{self, InfoBar, InfoEvent, Status};

/// Thumbnails are decoded into this box. 190 pt wide at up to 150% scaling.
const THUMB_BOX: (u32, u32) = (300, 300);
/// Canvas previews. Comfortably above what a 4K panel shows for the canvas
/// region, and small enough that a window of them fits in the texture budget.
const PREVIEW_DIM: u32 = 2048;
/// The enhance proxy. Small enough that a full colour pass is instant, large
/// enough to judge the result.
const PROXY_DIM: u32 = 1600;
/// Thumbnail texture budget. Roughly 700 cells at 300×300.
const THUMB_BUDGET: usize = 256 * 1024 * 1024;
/// Preview texture budget. A 2048-wide frame is around 11 MB, so this holds
/// roughly 45 — comfortably more than the prefetch window, with room for the
/// ones you have just walked past.
const PREVIEW_BUDGET: usize = 512 * 1024 * 1024;
/// How far either side of the cursor thumbnails are queued.
const THUMB_PREFETCH: usize = 24;
/// How far either side of the cursor previews are decoded ahead of time.
///
/// Culling is a linear scan, so the frames you are about to reach are
/// predictable. Ten each way covers a fast arrow-key run without decoding a
/// whole folder speculatively.
const PREVIEW_PREFETCH: usize = 10;

pub struct PicktureApp {
    theme: Theme,
    store: SessionStore,
    session: Option<Session>,

    textures: TextureStore,
    thumbs: ImageLoader,
    previews: ImageLoader,
    scanner: ScanLoader,
    writer: Writer,

    browse: BrowseState,
    select: SelectState,
    enhance: EnhanceState,

    /// `(frames probed, total)` while a folder is being scanned.
    scanning: Option<(usize, usize)>,
    pending_folder: Option<PathBuf>,

    /// Keep acknowledgement: which cell, and when it started.
    ack: Option<(usize, f64)>,

    /// The unprocessed proxy the enhance modal works from, plus its histogram.
    proxy_base: Option<RgbaImage>,
    proxy_for: Option<PathBuf>,
    /// A processed proxy waiting to be uploaded at the top of the next frame.
    /// Uploads never happen inside a draw call.
    pending_proxy: Option<RgbaImage>,
    histogram: Histogram,
    working_spec: EffectSpec,

    /// Session state has changed since the last write to disk.
    dirty: bool,
    last_saved: f64,

    notice: Option<(String, f64)>,
    decode_ms: u32,
    show_all_shortcuts: bool,
    zooming: bool,
    failed: Option<(PathBuf, String)>,
}

impl PicktureApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        pickture_ui_kit::fonts::install(&cc.egui_ctx);

        let store = SessionStore::load(&paths::sessions_file());
        let theme = match store.prefer_light_theme {
            Some(true) => Theme::for_mode(Mode::Light),
            Some(false) => Theme::for_mode(Mode::Dark),
            // Dark is the design's default ground.
            None => Theme::for_mode(Mode::Dark),
        };
        pickture_ui_kit::apply_style(&cc.egui_ctx, &theme);

        let (thumb_workers, preview_workers) = worker_split();

        Self {
            theme,
            store,
            session: None,
            textures: TextureStore::new(THUMB_BUDGET, PREVIEW_BUDGET),
            thumbs: ImageLoader::new(
                DecodeKind::Thumbnail {
                    box_w: THUMB_BOX.0,
                    box_h: THUMB_BOX.1,
                },
                thumb_workers,
                paths::thumbnail_cache_dir(),
            ),
            previews: ImageLoader::new(
                DecodeKind::Preview {
                    max_dim: PREVIEW_DIM,
                },
                preview_workers,
                None,
            ),
            scanner: ScanLoader::new(),
            writer: Writer::new(),
            browse: BrowseState::default(),
            select: SelectState::default(),
            enhance: EnhanceState::default(),
            scanning: None,
            pending_folder: None,
            ack: None,
            proxy_base: None,
            proxy_for: None,
            pending_proxy: None,
            histogram: Histogram::default(),
            working_spec: EffectSpec::default(),
            dirty: false,
            last_saved: 0.0,
            notice: None,
            decode_ms: 0,
            show_all_shortcuts: false,
            zooming: false,
            failed: None,
        }
    }

    // -- folder lifecycle --------------------------------------------------

    /// Switch the working folder in place.
    ///
    /// Nothing is torn down: no window teardown, no GPU re-init. The outgoing
    /// decode queue is *cancelled rather than paused*, and both caches stay
    /// warm, so switching back is instant.
    fn open_folder(&mut self, folder: PathBuf) {
        self.persist_active();

        self.thumbs.cancel_all();
        self.previews.cancel_all();
        self.textures.release_frame_textures();
        self.browse.close_popovers();
        self.select.menu_open = false;
        self.enhance.close();
        self.proxy_base = None;
        self.proxy_for = None;
        self.failed = None;

        self.store.touch_recent(&folder);
        self.pending_folder = Some(folder.clone());
        self.scanning = Some((0, 0));
        self.scanner.request(folder);
    }

    fn persist_active(&mut self) {
        if let Some(session) = &self.session {
            self.store
                .put(session.folder.clone(), session.to_persisted());
        }
        self.store.save(&paths::sessions_file());
        self.dirty = false;
        self.last_saved = now();
    }

    /// Write the session out periodically rather than only on a clean exit.
    ///
    /// A culling run is hundreds of decisions; losing them to a crash or a
    /// force-quit would be worse than the cost of a small JSON write. Throttled
    /// so holding an arrow key does not write on every frame.
    fn autosave(&mut self) {
        const INTERVAL: f64 = 2.0;
        if self.dirty && now() - self.last_saved > INTERVAL {
            self.persist_active();
        }
    }

    fn browse_for_folder(&mut self) {
        let start = self
            .session
            .as_ref()
            .map(|s| s.folder.clone())
            .or_else(|| self.store.recent.first().cloned());
        let mut dialog = rfd::FileDialog::new().set_title("Choose a folder of frames");
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        if let Some(picked) = dialog.pick_folder() {
            self.open_folder(picked);
        }
    }

    fn browse_for_destination(&mut self) {
        let Some(session) = &self.session else { return };
        let mut dialog = rfd::FileDialog::new().set_title("Where should keepers go?");
        dialog = dialog.set_directory(&session.folder);
        if let Some(picked) = dialog.pick_folder() {
            self.set_destination(Destination::Absolute(picked));
        }
    }

    fn set_destination(&mut self, destination: Destination) {
        let Some(session) = &mut self.session else {
            return;
        };
        match validate(&destination, &session.folder) {
            Ok(_) => {
                if let Destination::Absolute(p) = &destination {
                    if !session.remembered_destinations.contains(p) {
                        session.remembered_destinations.push(p.clone());
                    }
                }
                session.destination = destination;
                session.rescan_destination();
                self.select.menu_open = false;
                self.dirty = true;
            }
            Err(message) => self.notify(message),
        }
    }

    fn notify(&mut self, message: String) {
        self.notice = Some((message, now()));
    }

    // -- judgement ---------------------------------------------------------

    fn keep_current(&mut self, spec: Option<EffectSpec>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(frame) = session.frames.get(session.cursor).cloned() else {
            return;
        };

        let spec = spec.unwrap_or_else(|| session.effect_of(&frame.id));
        let destination = session.destination.clone();
        let folder = session.folder.clone();

        let dir = match validate(&destination, &folder) {
            Ok(d) => d,
            Err(message) => {
                self.notify(message);
                return;
            }
        };

        session.judgement.insert(frame.id.clone(), Judgement::Kept);
        session.set_effect(frame.id.clone(), spec);
        let cursor = session.cursor;

        let extension = frame
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_ascii_lowercase();

        self.writer.submit(WriteJob {
            source: frame.path.clone(),
            destination_dir: dir,
            stem: frame.stem.clone(),
            extension,
            spec,
            frame_id: frame.id.clone(),
        });

        self.ack = Some((cursor, now()));
        self.dirty = true;
    }

    fn pass_current(&mut self) {
        {
            let Some(session) = &mut self.session else {
                return;
            };
            let Some(frame) = session.frames.get(session.cursor) else {
                return;
            };
            let id = frame.id.clone();
            session.judgement.insert(id, Judgement::Passed);
            session.advance(1);
        }
        self.browse.scroll_to_cursor = true;
        self.dirty = true;
    }

    // -- enhance -----------------------------------------------------------

    fn open_enhance(&mut self) {
        let Some(session) = &self.session else { return };
        let Some(frame) = session.current() else {
            return;
        };
        self.working_spec = session.effect_of(&frame.id);
        self.enhance.open = true;
        self.enhance.write_progress = None;

        // Decode the proxy synchronously only if it is not already the right
        // frame; at 1600 px this is tens of milliseconds and happens once per
        // modal open, not per drag.
        if self.proxy_for.as_deref() != Some(frame.path.as_path()) {
            match pickture_kernel::image_io::decode_preview(&frame.path, PROXY_DIM) {
                Ok(img) => {
                    self.histogram = Histogram::from_bins(&pixel_ops::histogram_luma(&img));
                    self.proxy_base = Some(img);
                    self.proxy_for = Some(frame.path.clone());
                }
                Err(e) => {
                    self.notify(format!("{} can't be read — {e}", frame.stem));
                    self.enhance.close();
                    return;
                }
            }
        }
        self.rebuild_proxy();
    }

    /// Re-run the colour pass on the proxy and upload it.
    ///
    /// This is what replaces the *Apply* button: one pass over a 1600 px buffer
    /// per handle movement, which is fast enough to be immediate. The fine
    /// rotation angle is applied at draw time instead, so dragging it costs no
    /// pixel work at all.
    fn rebuild_proxy(&mut self) {
        let Some(base) = &self.proxy_base else { return };
        let mut img = base.clone();
        pixel_ops::apply_colour(&mut img, &self.working_spec);
        let img = pixel_ops::rotate_quarters(&img, self.working_spec.quarter_turns);
        self.pending_proxy = Some(img);
    }

    fn confirm_enhance(&mut self) {
        let spec = self.working_spec;
        if let Some(session) = &mut self.session {
            if let Some(frame) = session.current().cloned() {
                session.set_effect(frame.id, spec);
            }
        }
        self.keep_current(Some(spec));
        self.enhance.close();
    }

    // -- background results ------------------------------------------------

    fn drain_workers(&mut self, ctx: &Context) {
        // ---- scan --------------------------------------------------------
        for outcome in self.scanner.poll() {
            match outcome {
                ScanOutcome::Progress { folder, found } => {
                    if self.pending_folder.as_deref() == Some(folder.as_path()) {
                        let total = self.scanning.map(|(_, t)| t).unwrap_or(0).max(found);
                        self.scanning = Some((found, total.max(found + 1)));
                    }
                }
                ScanOutcome::Done { folder, frames } => {
                    if self.pending_folder.as_deref() != Some(folder.as_path()) {
                        continue;
                    }
                    let persisted = self.store.get(&folder);
                    let mut session = Session::open(folder.clone(), persisted);
                    // `Session::open` rescans; replace its frames with the ones
                    // that already carry probed dimensions.
                    let cursor_id = session.current().map(|f| f.id.clone());
                    session.frames = frames;
                    session.cursor = cursor_id
                        .and_then(|id| session.frames.iter().position(|f| f.id == id))
                        .unwrap_or(0);
                    session.rescan_destination();

                    self.session = Some(session);
                    self.scanning = None;
                    self.pending_folder = None;
                    self.browse.scroll_to_cursor = true;
                    self.failed = None;
                }
            }
        }

        // ---- thumbnails --------------------------------------------------
        for outcome in self.thumbs.poll() {
            if let DecodeOutcome::Ready { path, image, .. } = outcome {
                self.textures.put_thumb(ctx, path, &image);
            }
        }

        // ---- previews ----------------------------------------------------
        let current = self
            .session
            .as_ref()
            .and_then(|s| s.current().map(|f| f.path.clone()));
        for outcome in self.previews.poll() {
            match outcome {
                DecodeOutcome::Ready {
                    path,
                    image,
                    millis,
                } => {
                    // Only the frame actually on screen sets the reported decode
                    // time; a prefetch landing in the background would otherwise
                    // make the number meaningless.
                    if current.as_deref() == Some(path.as_path()) {
                        self.decode_ms = millis;
                    }
                    if self.has_failed(&path) {
                        self.failed = None;
                    }
                    self.textures.put_preview(ctx, path, &image);
                }
                DecodeOutcome::Failed { path, message } => {
                    self.failed = Some((path, message));
                }
            }
        }

        // ---- writer ------------------------------------------------------
        for progress in self.writer.poll() {
            match progress {
                WriteProgress::Started { .. } => {
                    if self.enhance.open {
                        self.enhance.write_progress = Some(0.0);
                    }
                }
                WriteProgress::Step { fraction, .. } => {
                    if self.enhance.open {
                        self.enhance.write_progress = Some(fraction);
                    }
                }
                WriteProgress::Finished(outcome) => {
                    self.enhance.write_progress = None;
                    if let Some(message) = outcome.error {
                        self.notify(message);
                        // A failed write must not leave the frame marked kept.
                        if let Some(session) = &mut self.session {
                            session.judgement.remove(&outcome.frame_id);
                        }
                    } else {
                        if outcome.renamed {
                            if let Some(p) = &outcome.written {
                                let name = p
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                self.notify(format!(
                                    "a different file had that name — wrote {name}"
                                ));
                            }
                        }
                        if let Some(session) = &mut self.session {
                            session.already_in_destination.insert(outcome.frame_id);
                        }
                    }
                }
            }
        }
    }

    /// Queue decodes for a window around the cursor, nearest first.
    ///
    /// Both loaders are fed from here. The priority is distance from the
    /// cursor, so the current frame always jumps the queue and the prefetch
    /// fills in behind it. Re-requesting something already queued, in flight or
    /// cached is free, so this can run every frame.
    fn queue_decodes(&mut self) {
        let Some(session) = &self.session else { return };
        if session.frames.is_empty() {
            return;
        }
        let cursor = session.cursor as isize;
        let last = session.frames.len() as isize - 1;

        // Walk outwards from the cursor: 0, +1, -1, +2, -2, … so the frame you
        // are on is queued first and the direction you are most likely heading
        // is never starved.
        let window = THUMB_PREFETCH.max(PREVIEW_PREFETCH) as isize;
        for offset in 0..=window {
            for signed in [offset, -offset] {
                if offset == 0 && signed != 0 {
                    continue;
                }
                let i = cursor + signed;
                if i < 0 || i > last {
                    continue;
                }
                let frame = &session.frames[i as usize];
                let distance = offset as u32;

                if distance <= THUMB_PREFETCH as u32
                    && !self.textures.has_thumb(&frame.path)
                    && !self.thumbs.is_outstanding(&frame.path)
                {
                    self.thumbs.request(DecodeRequest {
                        path: frame.path.clone(),
                        modified: frame.modified,
                        len: frame.file_size,
                        priority: distance,
                    });
                }

                if distance <= PREVIEW_PREFETCH as u32
                    && !self.textures.has_preview(&frame.path)
                    && !self.previews.is_outstanding(&frame.path)
                    && !self.has_failed(&frame.path)
                {
                    self.previews.request(DecodeRequest {
                        path: frame.path.clone(),
                        modified: frame.modified,
                        len: frame.file_size,
                        priority: distance,
                    });
                }
            }
        }
    }

    fn has_failed(&self, path: &std::path::Path) -> bool {
        self.failed.iter().any(|(p, _)| p == path)
    }

    // -- input -------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &Context) {
        // Popovers own the keyboard while they are open.
        if self.browse.switcher_open || self.select.menu_open {
            return;
        }

        if self.enhance.open {
            let (escape, enter) =
                ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)));
            if escape {
                self.enhance.close();
            } else if enter && self.enhance.editing.is_none() {
                self.confirm_enhance();
            }
            return;
        }

        if self.session.is_none() {
            return;
        }

        let input = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Backspace) || i.key_pressed(Key::Delete),
                i.key_pressed(Key::E),
                i.key_pressed(Key::O),
                i.key_pressed(Key::S),
                i.key_pressed(Key::OpenBracket),
                i.key_pressed(Key::CloseBracket),
                i.key_pressed(Key::Slash),
                i.key_down(Key::Z),
                i.modifiers.shift,
                i.modifiers.command || i.modifiers.ctrl,
            )
        });
        let (next, prev, enter, pass, enhance, o, s, rot_l, rot_r, slash, z, shift, ctrl) = input;

        self.zooming = z;

        if next {
            if let Some(session) = &mut self.session {
                session.advance(1);
            }
            self.browse.scroll_to_cursor = true;
            self.dirty = true;
        }
        if prev {
            if let Some(session) = &mut self.session {
                session.advance(-1);
            }
            self.browse.scroll_to_cursor = true;
            self.dirty = true;
        }
        // Enhance is the main pass, not a detour off it. Choosing a frame opens
        // the enhance page, where confirming with no effect selected saves the
        // original — so the common case is still two keys, and the correction
        // case never needs a different one.
        if enter && !ctrl {
            self.open_enhance();
        }
        // The escape hatch: keep exactly as shot, without the round trip.
        if enter && ctrl {
            self.keep_current(None);
        }
        if pass {
            self.pass_current();
        }
        if enhance {
            self.open_enhance();
        }
        if o {
            if shift {
                self.browse_for_folder();
            } else {
                self.browse.open_switcher();
            }
        }
        if s {
            if shift {
                self.browse_for_destination();
            } else {
                self.select.open();
            }
        }
        if rot_l || rot_r {
            self.rotate_current(if rot_l { -1 } else { 1 });
        }
        if slash {
            self.show_all_shortcuts = !self.show_all_shortcuts;
        }
    }

    fn rotate_current(&mut self, delta: i32) {
        {
            let Some(session) = &mut self.session else {
                return;
            };
            let Some(frame) = session.current().cloned() else {
                return;
            };
            let mut spec = session.effect_of(&frame.id);
            spec.quarter_turns += delta;
            session.set_effect(frame.id, spec);
        }
        self.dirty = true;
    }

    fn handle_dropped_folders(&mut self, ctx: &Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            let folder = if path.is_dir() {
                Some(path)
            } else {
                path.parent().map(|p| p.to_path_buf())
            };
            if let Some(folder) = folder {
                self.open_folder(folder);
                break;
            }
        }
    }

    // -- animation ---------------------------------------------------------

    fn ack_state(&self) -> AckState {
        match self.ack {
            Some((index, started)) => {
                let t = ((now() - started) / pickture_ui_kit::motion::ACK as f64) as f32;
                if t >= 1.0 {
                    AckState {
                        index: None,
                        alpha: 0.0,
                    }
                } else {
                    // Linear-out: opacity only, no scale, no bounce.
                    AckState {
                        index: Some(index),
                        alpha: 1.0 - t,
                    }
                }
            }
            None => AckState {
                index: None,
                alpha: 0.0,
            },
        }
    }
}

// A pending proxy upload, kept out of the struct literal above for readability.
impl PicktureApp {
    fn flush_proxy(&mut self, ctx: &Context) {
        if let Some(img) = self.pending_proxy.take() {
            self.textures.put_proxy(ctx, &img);
        }
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl eframe::App for PicktureApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.theme.window;
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.persist_active();
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_workers(ctx);
        self.flush_proxy(ctx);
        self.handle_dropped_folders(ctx);
        self.handle_keys(ctx);
        self.queue_decodes();
        self.autosave();

        // Expire a notice after a few seconds.
        if let Some((_, at)) = &self.notice {
            if now() - at > 6.0 {
                self.notice = None;
            }
        }

        let theme = self.theme;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme.window))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let (title, body) = paint::split_top(full, metric::TITLE_BAR);

                let folder = self.session.as_ref().map(|s| s.folder.clone());
                if let Some(event) = chrome::title_bar(
                    ui,
                    &theme,
                    title,
                    folder.as_deref(),
                    self.browse.switcher_open,
                ) {
                    self.route_browse(event);
                }

                if self.session.is_some() {
                    self.gallery(ui, &theme, body);
                } else {
                    self.picker_screen(ui, &theme, body);
                }
            });

        // Popovers float above everything.
        let anchor = switcher_anchor(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(0.0, metric::TITLE_BAR),
        ));
        if let Some(event) = folder_switcher_popover(
            ctx,
            &theme,
            anchor,
            &self.store,
            self.session.as_ref().map(|s| s.folder.as_path()),
            &mut self.browse,
        ) {
            self.route_browse(event);
        }

        if self.enhance.open {
            self.enhance_modal(ctx, &theme);
        }

        // Repaint only while something is actually moving.
        if self.ack.is_some()
            || self.scanning.is_some()
            || self.thumbs.pending() > 0
            || self.enhance.write_progress.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if let Some((_, started)) = self.ack {
            if now() - started > pickture_ui_kit::motion::ACK as f64 {
                self.ack = None;
            }
        }
    }
}

impl PicktureApp {
    fn route_browse(&mut self, event: BrowseEvent) {
        match event {
            BrowseEvent::OpenFolder(path) => self.open_folder(path),
            BrowseEvent::BrowseForFolder => {
                self.browse.switcher_open = false;
                self.browse_for_folder();
            }
            BrowseEvent::SelectFrame(index) => {
                if let Some(session) = &mut self.session {
                    session.cursor = index.min(session.frames.len().saturating_sub(1));
                }
                self.dirty = true;
            }
            BrowseEvent::ToggleSwitcher => {
                if self.browse.switcher_open {
                    self.browse.close_popovers();
                } else {
                    self.browse.open_switcher();
                }
                self.select.menu_open = false;
            }
            BrowseEvent::CloseSwitcher => self.browse.switcher_open = false,
        }
    }

    fn route_select(&mut self, event: SelectEvent) {
        match event {
            SelectEvent::SetDestination(d) => self.set_destination(d),
            SelectEvent::BrowseForDestination => {
                self.select.menu_open = false;
                self.browse_for_destination();
            }
            SelectEvent::ToggleMenu => {
                if self.select.menu_open {
                    self.select.menu_open = false;
                } else {
                    self.select.open();
                }
                self.browse.close_popovers();
            }
            SelectEvent::CloseMenu => self.select.menu_open = false,
        }
    }

    fn picker_screen(&mut self, ui: &mut egui::Ui, theme: &Theme, body: egui::Rect) {
        let (status, content) = paint::split_bottom(body, metric::STATUS_BAR);

        if let Some(event) =
            picker::folder_picker(ui, theme, content, &self.store, &mut self.browse)
        {
            self.route_browse(event);
        }
        if let Some(event) = picker::picker_keys(ui, &self.store, &mut self.browse) {
            self.route_browse(event);
        }

        // The picker's own status line.
        paint::fill(ui.painter(), status, theme.chrome);
        paint::rule_top(ui.painter(), status, theme.hair);
        let font = pickture_ui_kit::tokens::mono(pickture_ui_kit::size::MONO_S);
        paint::text_left(
            ui.painter(),
            Pos2::new(status.left() + 18.0, status.center().y),
            self.notice
                .as_ref()
                .map(|(m, _)| m.as_str())
                .unwrap_or("ready"),
            font.clone(),
            if self.notice.is_some() {
                theme.sodium
            } else {
                theme.fg_muted
            },
        );
        let mut x = status.right() - 18.0;
        for text in ["O browse", "↵ open", "↑ ↓ recent"] {
            let w = paint::text_width(ui.painter(), text, &font);
            x -= w;
            paint::text_left(
                ui.painter(),
                Pos2::new(x, status.center().y),
                text,
                font.clone(),
                theme.fg_muted,
            );
            x -= 22.0;
        }
    }

    fn gallery(&mut self, ui: &mut egui::Ui, theme: &Theme, body: egui::Rect) {
        let (strip_rect, right) = paint::split_left(body, metric::FILMSTRIP_W);
        let (info_rect, rest) = paint::split_top(right, metric::INFO_BAR);
        let (status_rect, canvas_rect) = paint::split_bottom(rest, metric::STATUS_BAR);

        let ack = self.ack_state();

        // ---- filmstrip ----------------------------------------------------
        {
            let session = self.session.as_ref().unwrap();
            let event = pickture_slice_browse::filmstrip(
                ui,
                theme,
                strip_rect,
                session,
                &mut self.textures,
                &mut self.browse,
                &ack,
                self.scanning,
            );
            if let Some(event) = event {
                self.route_browse(event);
            }
        }

        // ---- info bar ------------------------------------------------------
        let (name, exif, already, destination) = {
            let session = self.session.as_ref().unwrap();
            match session.current() {
                Some(frame) => (
                    frame.id.clone(),
                    frame.exif.clone(),
                    session.is_in_destination(&frame.id),
                    session.destination.clone(),
                ),
                None => (String::new(), None, false, session.destination.clone()),
            }
        };

        if let Some(event) = chrome::info_bar(
            ui,
            theme,
            info_rect,
            InfoBar {
                filename: &name,
                exif: exif.as_ref(),
                already_kept: already,
                destination: &destination,
                destination_open: self.select.menu_open,
            },
        ) {
            match event {
                // Same route as `↵` — the click and the key must not disagree
                // about what "add to selection" does.
                InfoEvent::Keep => self.open_enhance(),
                InfoEvent::Select(e) => self.route_select(e),
            }
        }

        // ---- canvas --------------------------------------------------------
        // The canvas shows the frame as it will be written, so the geometry the
        // write path applies is the geometry the canvas draws.
        let geom = {
            let session = self.session.as_ref().unwrap();
            session
                .current()
                .map(|f| {
                    let spec = session.effect_of(&f.id);
                    Geometry {
                        quarter_turns: spec.quarter_turns,
                        angle: spec.angle,
                    }
                })
                .unwrap_or_default()
        };

        let content = {
            let session = self.session.as_ref().unwrap();
            if session.frames.is_empty() && self.scanning.is_none() {
                CanvasContent::EmptyFolder {
                    supported: &SUPPORTED,
                }
            } else if let Some(frame) = session.current() {
                if let Some((failed, _)) = &self.failed {
                    if failed == &frame.path {
                        CanvasContent::Unreadable {
                            name: &frame.id,
                            reason: "can't be read — file may be truncated",
                        }
                    } else {
                        match self.textures.preview_for(&frame.path) {
                            Some(t) => CanvasContent::Image {
                                texture: t,
                                geometry: geom,
                            },
                            None => CanvasContent::Decoding,
                        }
                    }
                } else {
                    match self.textures.preview_for(&frame.path) {
                        Some(t) => CanvasContent::Image {
                            texture: t,
                            geometry: geom,
                        },
                        None => CanvasContent::Decoding,
                    }
                }
            } else {
                CanvasContent::Decoding
            }
        };

        let pad = if self.zooming {
            0.0
        } else {
            metric::CANVAS_PAD
        };
        canvas(ui, theme, canvas_rect, content, pad, ack.alpha, false);

        // ---- status bar ----------------------------------------------------
        {
            let session = self.session.as_ref().unwrap();
            let (position, total, kept, passed) = pickture_slice_browse::counts(session);
            chrome::status_bar(
                ui,
                theme,
                status_rect,
                Status {
                    position,
                    total,
                    kept,
                    passed,
                    decode_ms: self.decode_ms,
                    notice: self.notice.as_ref().map(|(m, _)| m.as_str()),
                    show_all_shortcuts: self.show_all_shortcuts,
                },
            );
        }

        // ---- destination popover -------------------------------------------
        let remembered = self
            .session
            .as_ref()
            .map(|s| s.remembered_destinations.clone())
            .unwrap_or_default();
        if let Some(event) = destination_popover(
            ui.ctx(),
            theme,
            Pos2::new(info_rect.right() - 18.0, info_rect.bottom()),
            &destination,
            &remembered,
            &mut self.select,
        ) {
            self.route_select(event);
        }
    }

    fn enhance_modal(&mut self, ctx: &Context, theme: &Theme) {
        let screen = ctx.screen_rect();
        let mut event = None;

        egui::Area::new(egui::Id::new("enhance-modal"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                paint::fill(ui.painter(), screen, theme.window);

                let (header, body) = paint::split_top(screen, metric::MODAL_HEADER);
                let name = self
                    .session
                    .as_ref()
                    .and_then(|s| s.current().map(|f| f.id.clone()))
                    .unwrap_or_default();

                if let Some(e) = modal_header(&mut ui, theme, header, &name) {
                    event = Some(e);
                }

                let (panel, canvas_rect) = paint::split_right(body, metric::PANEL_W);

                let content = match self.textures.proxy() {
                    Some(t) => CanvasContent::Image {
                        texture: t,
                        // Quarter turns are already baked into the proxy pixels;
                        // only the fine angle and its crop are applied at draw
                        // time, so dragging costs no pixel work.
                        geometry: Geometry {
                            quarter_turns: 0,
                            angle: self.working_spec.angle,
                        },
                    },
                    None => CanvasContent::Decoding,
                };
                canvas(
                    &mut ui,
                    theme,
                    canvas_rect,
                    content,
                    metric::MODAL_CANVAS_PAD,
                    0.0,
                    true,
                );

                let hist = self.histogram.clone();
                if let Some(e) = control_panel(
                    &mut ui,
                    theme,
                    panel,
                    &self.working_spec,
                    &hist,
                    &mut self.enhance,
                ) {
                    event = Some(e);
                }
            });

        match event {
            Some(EnhanceEvent::SpecChanged(spec)) => {
                if spec != self.working_spec {
                    self.working_spec = spec;
                    self.rebuild_proxy();
                }
            }
            Some(EnhanceEvent::Cancel) => self.enhance.close(),
            Some(EnhanceEvent::Confirm) => self.confirm_enhance(),
            None => {}
        }
    }
}

static SUPPORTED: std::sync::LazyLock<String> = std::sync::LazyLock::new(supported_label);
