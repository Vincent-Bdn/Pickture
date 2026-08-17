//! Per-folder sessions and their persistence.
//!
//! The design's rule is that switching folders is not a restart: each folder
//! keeps its own cursor, judgements, per-frame edits and destination, and
//! returning to one lands on the frame you left with the counts intact. Storing
//! that on disk means it also doubles as resume-where-you-left-off.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{is_supported, Destination, EffectSpec, Frame, FrameId, Judgement};

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// Non-recursive scan of a folder, sorted by file name.
///
/// Enumeration was never the bottleneck — `read_dir` over a few thousand
/// entries is milliseconds — so this stays deliberately simple.
pub fn scan_folder(folder: &Path) -> Vec<Frame> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };

    let mut frames: Vec<Frame> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() || !is_supported(&path) {
                return None;
            }
            let meta = entry.metadata().ok();
            Some(Frame::new(
                path,
                meta.as_ref().map(|m| m.len()).unwrap_or(0),
                meta.as_ref().and_then(|m| m.modified().ok()),
            ))
        })
        .collect();

    frames.sort_by(|a, b| natural_cmp(&a.id, &b.id));
    frames
}

/// Compare names so `_DSC9.jpg` sorts before `_DSC10.jpg`. Plain lexicographic
/// ordering puts 10 before 9, which reads as scrambled in a filmstrip.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.char_indices().peekable();
    let mut bi = b.char_indices().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((ax, ac)), Some((bx, bc))) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let a_end = a[ax..]
                        .find(|c: char| !c.is_ascii_digit())
                        .map(|i| ax + i)
                        .unwrap_or(a.len());
                    let b_end = b[bx..]
                        .find(|c: char| !c.is_ascii_digit())
                        .map(|i| bx + i)
                        .unwrap_or(b.len());

                    let an = a[ax..a_end].trim_start_matches('0');
                    let bn = b[bx..b_end].trim_start_matches('0');

                    let ord = an.len().cmp(&bn.len()).then_with(|| an.cmp(bn));
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                    while ai.peek().map(|(i, _)| *i < a_end).unwrap_or(false) {
                        ai.next();
                    }
                    while bi.peek().map(|(i, _)| *i < b_end).unwrap_or(false) {
                        bi.next();
                    }
                } else {
                    let ord = ac.to_ascii_lowercase().cmp(&bc.to_ascii_lowercase());
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedSession {
    #[serde(default)]
    pub cursor_id: Option<FrameId>,
    #[serde(default)]
    pub judgement: HashMap<FrameId, Judgement>,
    #[serde(default)]
    pub edits: HashMap<FrameId, EffectSpec>,
    #[serde(default)]
    pub destination: Destination,
    /// Absolute destinations offered again next time this folder is opened.
    #[serde(default)]
    pub remembered_destinations: Vec<PathBuf>,
    #[serde(default)]
    pub frame_count: usize,
    #[serde(default)]
    pub last_opened_secs: u64,
}

/// A live session: the persisted state plus the scanned frames.
pub struct Session {
    pub folder: PathBuf,
    pub frames: Vec<Frame>,
    pub cursor: usize,
    pub judgement: HashMap<FrameId, Judgement>,
    pub edits: HashMap<FrameId, EffectSpec>,
    pub destination: Destination,
    pub remembered_destinations: Vec<PathBuf>,
    /// Frames found already present in the destination when the folder opened.
    pub already_in_destination: std::collections::HashSet<FrameId>,
}

impl Session {
    pub fn open(folder: PathBuf, persisted: PersistedSession) -> Self {
        let frames = scan_folder(&folder);
        let cursor = persisted
            .cursor_id
            .as_ref()
            .and_then(|id| frames.iter().position(|f| &f.id == id))
            .unwrap_or(0);

        let mut session = Self {
            folder,
            frames,
            cursor,
            judgement: persisted.judgement,
            edits: persisted.edits,
            destination: persisted.destination,
            remembered_destinations: persisted.remembered_destinations,
            already_in_destination: Default::default(),
        };
        session.rescan_destination();
        session
    }

    pub fn to_persisted(&self) -> PersistedSession {
        PersistedSession {
            cursor_id: self.current().map(|f| f.id.clone()),
            judgement: self.judgement.clone(),
            edits: self.edits.clone(),
            destination: self.destination.clone(),
            remembered_destinations: self.remembered_destinations.clone(),
            frame_count: self.frames.len(),
            last_opened_secs: now_secs(),
        }
    }

    pub fn current(&self) -> Option<&Frame> {
        self.frames.get(self.cursor)
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn advance(&mut self, delta: isize) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
    }

    pub fn judgement_of(&self, id: &FrameId) -> Option<Judgement> {
        self.judgement.get(id).copied()
    }

    pub fn effect_of(&self, id: &FrameId) -> EffectSpec {
        self.edits.get(id).copied().unwrap_or_default()
    }

    pub fn set_effect(&mut self, id: FrameId, spec: EffectSpec) {
        if spec == EffectSpec::default() {
            self.edits.remove(&id);
        } else {
            self.edits.insert(id, spec);
        }
    }

    /// Frames kept in this session *or* already present in the destination.
    ///
    /// Counting only this session's judgements would report "0 kept" while the
    /// filmstrip shows cells marked KEPT from a previous sitting, which reads as
    /// a bug even though both numbers are individually defensible.
    pub fn kept_count(&self) -> usize {
        self.frames
            .iter()
            .filter(|f| {
                self.judgement.get(&f.id) == Some(&Judgement::Kept)
                    || self.already_in_destination.contains(&f.id)
            })
            .count()
    }

    pub fn passed_count(&self) -> usize {
        self.judgement
            .values()
            .filter(|j| **j == Judgement::Passed)
            .count()
    }

    pub fn seen_count(&self) -> usize {
        self.judgement.len()
    }

    pub fn is_in_destination(&self, id: &FrameId) -> bool {
        self.already_in_destination.contains(id)
    }

    /// Work out which frames are already present in the destination.
    ///
    /// Matches on the stem with any known effect suffix stripped, so a frame
    /// kept in a previous session as `_DSC4118_WBV.jpg` still marks
    /// `_DSC4118.jpg` as already kept rather than writing it a second time.
    pub fn rescan_destination(&mut self) {
        use crate::model::EffectMode;

        self.already_in_destination.clear();
        let dir = self.destination.resolve(&self.folder);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };

        let mut present = std::collections::HashSet::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let mut base = stem.to_ascii_lowercase();

            // Order matters: the collision counter is appended *after* the
            // effect suffix, so `a_ORIG_2` has to lose the `_2` before `_orig`
            // is visible at the end of the string.
            if let Some(cut) = base.rfind('_') {
                if base[cut + 1..].chars().all(|c| c.is_ascii_digit()) && cut + 1 < base.len() {
                    base.truncate(cut);
                }
            }
            for suffix in EffectMode::all_suffixes() {
                let lower = suffix.to_ascii_lowercase();
                if base.ends_with(&lower) {
                    base.truncate(base.len() - lower.len());
                    break;
                }
            }
            present.insert(base);
        }

        for frame in &self.frames {
            if present.contains(&frame.stem.to_ascii_lowercase()) {
                self.already_in_destination.insert(frame.id.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStore {
    #[serde(default)]
    pub sessions: HashMap<PathBuf, PersistedSession>,
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    #[serde(default)]
    pub prefer_light_theme: Option<bool>,
}

const MAX_RECENT: usize = 12;

impl SessionStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            // A UTF-8 BOM is not valid JSON, and several Windows editors add one
            // on save. Without this, hand-editing the file silently discards
            // every remembered folder rather than reporting a problem.
            .map(|s| s.trim_start_matches('\u{feff}').to_string())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            // Write-then-rename: a crash mid-save must not corrupt the record of
            // every folder the user has culled.
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    pub fn touch_recent(&mut self, folder: &Path) {
        self.recent.retain(|p| p != folder);
        self.recent.insert(0, folder.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn get(&self, folder: &Path) -> PersistedSession {
        self.sessions.get(folder).cloned().unwrap_or_default()
    }

    pub fn put(&mut self, folder: PathBuf, session: PersistedSession) {
        self.sessions.insert(folder, session);
    }

    /// "412 frames · 120 seen · 38 kept" — what the switcher menu shows so a
    /// folder change is an informed move rather than a gamble.
    pub fn progress_line(&self, folder: &Path) -> String {
        match self.sessions.get(folder) {
            None => "not started".to_string(),
            Some(s) => {
                let seen = s.judgement.len();
                let kept = s
                    .judgement
                    .values()
                    .filter(|j| **j == Judgement::Kept)
                    .count();
                if seen == 0 {
                    format!("{} frames · not started", s.frame_count)
                } else {
                    format!("{} frames · {} seen · {} kept", s.frame_count, seen, kept)
                }
            }
        }
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "2 days ago" for the recent list.
pub fn relative_age(secs: u64) -> String {
    if secs == 0 {
        return "never".into();
    }
    let now = now_secs();
    let delta = now.saturating_sub(secs);
    match delta {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", delta / 60),
        3600..=86_399 => format!("{} h ago", delta / 3600),
        86_400..=172_799 => "yesterday".into(),
        172_800..=2_591_999 => format!("{} days ago", delta / 86_400),
        _ => format!("{} weeks ago", delta / 604_800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_puts_9_before_10() {
        let mut names = vec![
            "_DSC10.jpg".to_string(),
            "_DSC9.jpg".to_string(),
            "_DSC100.jpg".to_string(),
            "_DSC1.jpg".to_string(),
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec!["_DSC1.jpg", "_DSC9.jpg", "_DSC10.jpg", "_DSC100.jpg"]
        );
    }

    #[test]
    fn natural_order_ignores_leading_zeros_but_is_stable() {
        assert_eq!(natural_cmp("a007", "a7"), std::cmp::Ordering::Equal);
        assert_eq!(natural_cmp("a08", "a9"), std::cmp::Ordering::Less);
    }

    #[test]
    fn advance_clamps_at_both_ends() {
        let mut s = Session {
            folder: PathBuf::from("/x"),
            frames: (0..3)
                .map(|i| Frame::new(PathBuf::from(format!("/x/{i}.jpg")), 0, None))
                .collect(),
            cursor: 0,
            judgement: Default::default(),
            edits: Default::default(),
            destination: Default::default(),
            remembered_destinations: Default::default(),
            already_in_destination: Default::default(),
        };
        s.advance(-1);
        assert_eq!(s.cursor, 0);
        s.advance(10);
        assert_eq!(s.cursor, 2);
        s.advance(-1);
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn identity_effects_are_not_stored() {
        let mut s = Session {
            folder: PathBuf::from("/x"),
            frames: vec![],
            cursor: 0,
            judgement: Default::default(),
            edits: Default::default(),
            destination: Default::default(),
            remembered_destinations: Default::default(),
            already_in_destination: Default::default(),
        };
        s.set_effect("a.jpg".into(), EffectSpec::default());
        assert!(s.edits.is_empty());
        let spec = EffectSpec {
            quarter_turns: 1,
            ..Default::default()
        };
        s.set_effect("a.jpg".into(), spec);
        assert_eq!(s.edits.len(), 1);
    }

    #[test]
    fn recent_is_most_recent_first_and_deduped() {
        let mut store = SessionStore::default();
        store.touch_recent(Path::new("/a"));
        store.touch_recent(Path::new("/b"));
        store.touch_recent(Path::new("/a"));
        assert_eq!(store.recent, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
