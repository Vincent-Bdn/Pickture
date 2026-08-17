//! Caches.
//!
//! Two levels, and the reason for each:
//!
//! * **Memory** — bounded by *bytes*, not by item count. The abandoned draft
//!   kept every full-resolution decode in an unbounded `HashMap`, so browsing a
//!   hundred 24 MP frames reached roughly 7 GB resident and the machine started
//!   swapping. A byte budget makes that failure impossible.
//! * **Disk** — thumbnails survive a quit, so reopening a folder you have
//!   already culled is instant. This is the case that matters day to day.

use image::RgbaImage;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Cache keys
// ---------------------------------------------------------------------------

/// FNV-1a. Written out rather than using `DefaultHasher` because this value is
/// persisted to disk, and `DefaultHasher`'s output is explicitly not stable
/// across Rust releases — cache entries would silently orphan on every upgrade.
fn fnv1a(bytes: &[u8], mut hash: u64) -> u64 {
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// Identity of a thumbnail: path, modification time, size and target box.
/// Editing a file in place changes its mtime, so the stale thumbnail is missed
/// rather than served.
pub fn thumb_key(
    path: &Path,
    modified: Option<SystemTime>,
    len: u64,
    box_w: u32,
    box_h: u32,
) -> u64 {
    let mut h = fnv1a(path.to_string_lossy().as_bytes(), 0xcbf2_9ce4_8422_2325);
    let secs = modified
        .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    h = fnv1a(&secs.to_le_bytes(), h);
    h = fnv1a(&len.to_le_bytes(), h);
    h = fnv1a(&box_w.to_le_bytes(), h);
    fnv1a(&box_h.to_le_bytes(), h)
}

// ---------------------------------------------------------------------------
// Byte-budgeted LRU
// ---------------------------------------------------------------------------

pub trait Weighed {
    /// Approximate resident size in bytes.
    fn weight(&self) -> usize;
}

impl Weighed for RgbaImage {
    fn weight(&self) -> usize {
        (self.width() as usize) * (self.height() as usize) * 4
    }
}

impl Weighed for Vec<u8> {
    fn weight(&self) -> usize {
        self.len()
    }
}

struct Entry<V> {
    value: V,
    weight: usize,
    tick: u64,
}

/// Least-recently-used, evicting on a total byte budget.
pub struct BudgetCache<K: Eq + Hash + Clone, V: Weighed> {
    map: HashMap<K, Entry<V>>,
    budget: usize,
    used: usize,
    tick: u64,
}

impl<K: Eq + Hash + Clone, V: Weighed> BudgetCache<K, V> {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            budget: budget_bytes,
            used: 0,
            tick: 0,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.map.get_mut(key)?;
        entry.tick = tick;
        Some(&entry.value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub fn insert(&mut self, key: K, value: V) {
        let weight = value.weight();
        self.tick += 1;

        if let Some(old) = self.map.remove(&key) {
            self.used = self.used.saturating_sub(old.weight);
        }

        // An item larger than the whole budget is still stored — refusing it
        // would mean the current frame could not be displayed at all — but
        // everything else is evicted to make room.
        self.map.insert(
            key,
            Entry {
                value,
                weight,
                tick: self.tick,
            },
        );
        self.used += weight;
        self.evict();
    }

    fn evict(&mut self) {
        while self.used > self.budget && self.map.len() > 1 {
            let victim = self
                .map
                .iter()
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    if let Some(e) = self.map.remove(&k) {
                        self.used = self.used.saturating_sub(e.weight);
                    }
                }
                None => break,
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.used = 0;
    }

    /// Drop everything whose key fails the predicate — used when switching
    /// folders to release the outgoing preview without touching thumbnails.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut freed = 0usize;
        self.map.retain(|k, e| {
            let k2 = keep(k);
            if !k2 {
                freed += e.weight;
            }
            k2
        });
        self.used = self.used.saturating_sub(freed);
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Disk thumbnail store
// ---------------------------------------------------------------------------

/// Thumbnails on disk as small JPEGs, one file per key.
///
/// Deliberately not an embedded database: a folder of loose files is trivially
/// inspectable, trivially deletable, and needs no schema migration.
pub struct DiskThumbCache {
    dir: Option<PathBuf>,
}

impl DiskThumbCache {
    pub fn new(dir: Option<PathBuf>) -> Self {
        if let Some(d) = &dir {
            let _ = std::fs::create_dir_all(d);
        }
        Self { dir }
    }

    fn path_for(&self, key: u64) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{key:016x}.jpg")))
    }

    pub fn load(&self, key: u64) -> Option<RgbaImage> {
        let path = self.path_for(key)?;
        let bytes = std::fs::read(path).ok()?;
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
            .ok()
            .map(|i| i.to_rgba8())
    }

    pub fn store(&self, key: u64, img: &RgbaImage) {
        let Some(path) = self.path_for(key) else {
            return;
        };
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82);
        if enc
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .is_ok()
        {
            // Write beside the target then rename, so a crash mid-write cannot
            // leave a truncated JPEG that will fail to decode forever after.
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, buf.into_inner()).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_evicts_least_recently_used() {
        let mut c: BudgetCache<u32, Vec<u8>> = BudgetCache::new(30);
        c.insert(1, vec![0; 10]);
        c.insert(2, vec![0; 10]);
        c.insert(3, vec![0; 10]);
        // Touch 1 so 2 becomes the coldest.
        assert!(c.get(&1).is_some());
        c.insert(4, vec![0; 10]);
        assert!(c.contains(&1));
        assert!(!c.contains(&2));
        assert!(c.used_bytes() <= 30);
    }

    #[test]
    fn oversized_item_is_kept_but_clears_the_rest() {
        let mut c: BudgetCache<u32, Vec<u8>> = BudgetCache::new(20);
        c.insert(1, vec![0; 10]);
        c.insert(2, vec![0; 100]);
        assert!(c.contains(&2), "current frame must remain displayable");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn reinsert_does_not_double_count() {
        let mut c: BudgetCache<u32, Vec<u8>> = BudgetCache::new(1000);
        c.insert(1, vec![0; 10]);
        c.insert(1, vec![0; 10]);
        assert_eq!(c.used_bytes(), 10);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn keys_change_with_mtime_and_box() {
        let p = Path::new("/a/b.jpg");
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + std::time::Duration::from_secs(60);
        assert_ne!(
            thumb_key(p, Some(t0), 10, 190, 150),
            thumb_key(p, Some(t1), 10, 190, 150)
        );
        assert_ne!(
            thumb_key(p, Some(t0), 10, 190, 150),
            thumb_key(p, Some(t0), 11, 190, 150)
        );
        assert_ne!(
            thumb_key(p, Some(t0), 10, 190, 150),
            thumb_key(p, Some(t0), 10, 380, 150)
        );
        assert_eq!(
            thumb_key(p, Some(t0), 10, 190, 150),
            thumb_key(p, Some(t0), 10, 190, 150)
        );
    }

    #[test]
    fn retain_frees_weight() {
        let mut c: BudgetCache<u32, Vec<u8>> = BudgetCache::new(1000);
        c.insert(1, vec![0; 10]);
        c.insert(2, vec![0; 10]);
        c.retain(|k| *k == 1);
        assert_eq!(c.used_bytes(), 10);
        assert_eq!(c.len(), 1);
    }
}
