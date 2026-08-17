//! GPU texture ownership.
//!
//! This module exists because of the single worst bug in the abandoned Rust
//! draft. Its render function ran:
//!
//! ```ignore
//! let img = pic.image().clone();                     // clone ~72 MB
//! let tex = dynamic_image_to_texture(ctx, &id, img); // to_rgba8 + load_texture
//! ```
//!
//! `Context::load_texture` allocates a *new* texture and uploads the whole
//! buffer — it is not a cache lookup. Sitting inside the immediate-mode frame
//! loop, that re-uploaded a static image sixty times a second: roughly 15 GB/s
//! of memory traffic for a picture that had not changed.
//!
//! The rule this type enforces: **upload once, keyed by content; clone the
//! handle every frame.** `TextureHandle` is `Arc`-backed, so cloning it is free
//! and moves no pixels.

use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Approximate resident bytes for a texture of this size.
fn weight_of(size: Vec2) -> usize {
    (size.x as usize) * (size.y as usize) * 4
}

pub struct Texture {
    pub handle: TextureHandle,
    pub size: Vec2,
}

impl Texture {
    pub fn aspect(&self) -> f32 {
        if self.size.y <= 0.0 {
            1.0
        } else {
            self.size.x / self.size.y
        }
    }
}

struct ThumbEntry {
    texture: Texture,
    tick: u64,
}

/// Thumbnails, the canvas preview, and the enhance proxy.
///
/// Thumbnails are bounded by a byte budget rather than an item count, because
/// item counts do not bound memory: a hundred entries could be 4 MB or 4 GB.
pub struct TextureStore {
    thumbs: HashMap<PathBuf, ThumbEntry>,
    thumb_budget: usize,
    thumb_used: usize,
    tick: u64,

    /// Previews for the frames around the cursor, not just the current one.
    ///
    /// Culling is a linear scan, so the next frame is nearly always
    /// predictable. Holding a window of decoded neighbours is what turns
    /// arrowing through a folder from "wait for a decode" into "already there".
    previews: HashMap<PathBuf, ThumbEntry>,
    preview_budget: usize,
    preview_used: usize,

    proxy: Option<Texture>,
}

impl TextureStore {
    pub fn new(thumb_budget_bytes: usize, preview_budget_bytes: usize) -> Self {
        Self {
            thumbs: HashMap::new(),
            thumb_budget: thumb_budget_bytes,
            thumb_used: 0,
            tick: 0,
            previews: HashMap::new(),
            preview_budget: preview_budget_bytes,
            preview_used: 0,
            proxy: None,
        }
    }

    // -- thumbnails ---------------------------------------------------------

    pub fn has_thumb(&self, path: &Path) -> bool {
        self.thumbs.contains_key(path)
    }

    /// Read a thumbnail for drawing. Marks it as recently used.
    pub fn thumb(&mut self, path: &Path) -> Option<&Texture> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.thumbs.get_mut(path)?;
        entry.tick = tick;
        Some(&entry.texture)
    }

    /// Read without touching the LRU order — for layout passes that must not
    /// influence which textures survive eviction.
    pub fn peek_thumb(&self, path: &Path) -> Option<&Texture> {
        self.thumbs.get(path).map(|e| &e.texture)
    }

    /// Upload a decoded thumbnail. Called once per image, from the point where
    /// a decode lands — never from inside a draw call.
    pub fn put_thumb(&mut self, ctx: &Context, path: PathBuf, image: &image::RgbaImage) {
        let size = Vec2::new(image.width() as f32, image.height() as f32);
        let handle = upload(ctx, &format!("thumb:{}", path.display()), image);
        self.tick += 1;

        if let Some(old) = self.thumbs.remove(&path) {
            self.thumb_used = self.thumb_used.saturating_sub(weight_of(old.texture.size));
        }
        self.thumb_used += weight_of(size);
        self.thumbs.insert(
            path,
            ThumbEntry {
                texture: Texture { handle, size },
                tick: self.tick,
            },
        );
        self.evict_thumbs();
    }

    fn evict_thumbs(&mut self) {
        while self.thumb_used > self.thumb_budget && self.thumbs.len() > 1 {
            let victim = self
                .thumbs
                .iter()
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    if let Some(e) = self.thumbs.remove(&k) {
                        self.thumb_used = self.thumb_used.saturating_sub(weight_of(e.texture.size));
                    }
                }
                None => break,
            }
        }
    }

    // -- previews -----------------------------------------------------------

    /// Read a preview for drawing. Marks it as recently used, so the frames you
    /// are actually looking at outlive the prefetched ones.
    pub fn preview_for(&mut self, path: &Path) -> Option<&Texture> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.previews.get_mut(path)?;
        entry.tick = tick;
        Some(&entry.texture)
    }

    pub fn has_preview(&self, path: &Path) -> bool {
        self.previews.contains_key(path)
    }

    pub fn put_preview(&mut self, ctx: &Context, path: PathBuf, image: &image::RgbaImage) {
        let size = Vec2::new(image.width() as f32, image.height() as f32);
        let handle = upload(ctx, &format!("preview:{}", path.display()), image);
        self.tick += 1;

        if let Some(old) = self.previews.remove(&path) {
            self.preview_used = self
                .preview_used
                .saturating_sub(weight_of(old.texture.size));
        }
        self.preview_used += weight_of(size);
        self.previews.insert(
            path,
            ThumbEntry {
                texture: Texture { handle, size },
                tick: self.tick,
            },
        );
        self.evict_previews();
    }

    fn evict_previews(&mut self) {
        while self.preview_used > self.preview_budget && self.previews.len() > 1 {
            let victim = self
                .previews
                .iter()
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    if let Some(e) = self.previews.remove(&k) {
                        self.preview_used =
                            self.preview_used.saturating_sub(weight_of(e.texture.size));
                    }
                }
                None => break,
            }
        }
    }

    pub fn clear_previews(&mut self) {
        self.previews.clear();
        self.preview_used = 0;
    }

    pub fn preview_bytes(&self) -> usize {
        self.preview_used
    }

    pub fn preview_count(&self) -> usize {
        self.previews.len()
    }

    // -- enhance proxy ------------------------------------------------------

    /// The downscaled buffer the enhance modal draws. Replacing it every time a
    /// levels handle moves is exactly one upload of a ~1600 px image, which is
    /// what makes live dragging affordable.
    pub fn proxy(&self) -> Option<&Texture> {
        self.proxy.as_ref()
    }

    pub fn put_proxy(&mut self, ctx: &Context, image: &image::RgbaImage) {
        let size = Vec2::new(image.width() as f32, image.height() as f32);
        let handle = upload(ctx, "enhance-proxy", image);
        self.proxy = Some(Texture { handle, size });
    }

    pub fn clear_proxy(&mut self) {
        self.proxy = None;
    }

    // -- folder switching ---------------------------------------------------

    /// Drop previews and the proxy, keeping thumbnails.
    ///
    /// The design asks that switching folders cancels the outgoing decode but
    /// leaves its cache warm, so switching back is instant. Thumbnails are that
    /// warm cache — small, and the whole strip's worth. Previews are an order of
    /// magnitude larger each, so holding two folders' worth is not a trade
    /// worth making.
    pub fn release_frame_textures(&mut self) {
        self.clear_previews();
        self.proxy = None;
    }

    pub fn thumb_bytes(&self) -> usize {
        self.thumb_used
    }

    pub fn thumb_count(&self) -> usize {
        self.thumbs.len()
    }
}

fn upload(ctx: &Context, name: &str, image: &image::RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    ctx.load_texture(
        name.to_owned(),
        color,
        TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            ..Default::default()
        },
    )
}
