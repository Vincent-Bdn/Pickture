//! Background decoding.
//!
//! Four properties the abandoned draft lacked, each of which it was punished
//! for:
//!
//! * **A bounded pool.** It called `std::thread::spawn` per thumbnail, with no
//!   pool and no ceiling.
//! * **Priority.** Work is ordered by distance from the cursor, so the frames
//!   you are about to look at decode first.
//! * **Prefetch.** Neighbours are decoded before you reach them. Culling is a
//!   linear scan, so the next frame is nearly always predictable — and a decode
//!   you have already done costs nothing when you arrive.
//! * **Cancellation.** Switching folders retires outstanding work instead of
//!   letting it finish into a cache nobody will read.
//!
//! Nothing here ever runs on the UI thread. The draft called its decode
//! synchronously from inside the render function, which is what produced the
//! multi-hundred-millisecond freeze on every arrow-key press.

use image::RgbaImage;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::cache::{thumb_key, DiskThumbCache};
use crate::image_io;

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// What size, and whether the on-disk cache applies.
#[derive(Clone, Copy, Debug)]
pub enum DecodeKind {
    /// Filmstrip thumbnail, backed by the on-disk cache so a folder you have
    /// already culled reopens instantly.
    Thumbnail { box_w: u32, box_h: u32 },
    /// Canvas preview at display size. Deliberately not disk-cached: at this
    /// size the cache entry approaches the original file, so it would trade a
    /// lot of disk for very little time.
    Preview { max_dim: u32 },
}

pub struct DecodeRequest {
    pub path: PathBuf,
    pub modified: Option<std::time::SystemTime>,
    pub len: u64,
    /// Distance from the cursor. Lower is decoded sooner.
    pub priority: u32,
}

pub enum DecodeOutcome {
    Ready {
        path: PathBuf,
        image: RgbaImage,
        /// Wall time of the decode, shown in the status bar.
        millis: u32,
    },
    Failed {
        path: PathBuf,
        message: String,
    },
}

impl DecodeOutcome {
    pub fn path(&self) -> &Path {
        match self {
            DecodeOutcome::Ready { path, .. } => path,
            DecodeOutcome::Failed { path, .. } => path,
        }
    }
}

struct Job {
    request: DecodeRequest,
    generation: u64,
    /// Tie-break so equal priorities keep insertion order rather than an
    /// arbitrary heap order.
    seq: u64,
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Job {}
impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; invert so the lowest priority number is
        // popped first.
        other
            .request
            .priority
            .cmp(&self.request.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

struct Queue {
    heap: BinaryHeap<Job>,
    /// Paths that are queued *or* currently decoding. A path stays here until
    /// its result has been published, so a caller that asks every frame does
    /// not enqueue the same decode dozens of times.
    outstanding: HashSet<PathBuf>,
    seq: u64,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    generation: AtomicU64,
    shutdown: AtomicBool,
}

pub struct ImageLoader {
    shared: Arc<Shared>,
    rx: Receiver<DecodeOutcome>,
    workers: Vec<JoinHandle<()>>,
    kind: DecodeKind,
}

impl ImageLoader {
    pub fn new(kind: DecodeKind, workers: usize, cache_dir: Option<PathBuf>) -> Self {
        let workers = workers.max(1);

        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                heap: BinaryHeap::new(),
                outstanding: HashSet::new(),
                seq: 0,
            }),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        });

        let (tx, rx) = channel();
        let disk = Arc::new(DiskThumbCache::new(cache_dir));

        let handles = (0..workers)
            .map(|i| {
                let shared = Arc::clone(&shared);
                let disk = Arc::clone(&disk);
                let tx: Sender<DecodeOutcome> = tx.clone();
                std::thread::Builder::new()
                    .name(format!("pickture-decode-{i}"))
                    .spawn(move || worker(shared, disk, tx, kind))
                    .expect("spawn decode worker")
            })
            .collect();

        Self {
            shared,
            rx,
            workers: handles,
            kind,
        }
    }

    pub fn kind(&self) -> DecodeKind {
        self.kind
    }

    /// Queue a decode. Re-requesting a path that is already queued or in flight
    /// is a no-op, so callers may ask on every frame.
    pub fn request(&self, request: DecodeRequest) {
        let generation = self.shared.generation.load(AtomicOrdering::SeqCst);
        let mut q = self.shared.queue.lock().unwrap();
        if !q.outstanding.insert(request.path.clone()) {
            return;
        }
        q.seq += 1;
        let seq = q.seq;
        q.heap.push(Job {
            request,
            generation,
            seq,
        });
        drop(q);
        self.shared.wake.notify_one();
    }

    pub fn is_outstanding(&self, path: &Path) -> bool {
        self.shared.queue.lock().unwrap().outstanding.contains(path)
    }

    /// Retire everything queued and mark in-flight results stale.
    ///
    /// Called on a folder switch. The design asks for the outgoing decode to be
    /// *cancelled rather than paused*, with its cache left warm — which is what
    /// this does: the queue empties, but nothing in either cache is dropped, so
    /// switching back is instant.
    pub fn cancel_all(&self) {
        self.shared.generation.fetch_add(1, AtomicOrdering::SeqCst);
        let mut q = self.shared.queue.lock().unwrap();
        q.heap.clear();
        q.outstanding.clear();
    }

    pub fn pending(&self) -> usize {
        self.shared.queue.lock().unwrap().heap.len()
    }

    /// Drain finished decodes. Non-blocking; call once per frame.
    pub fn poll(&self) -> Vec<DecodeOutcome> {
        self.rx.try_iter().collect()
    }
}

impl Drop for ImageLoader {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, AtomicOrdering::SeqCst);
        self.shared.wake.notify_all();
        for h in self.workers.drain(..) {
            let _ = h.join();
        }
    }
}

fn worker(
    shared: Arc<Shared>,
    disk: Arc<DiskThumbCache>,
    tx: Sender<DecodeOutcome>,
    kind: DecodeKind,
) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if shared.shutdown.load(AtomicOrdering::SeqCst) {
                    return;
                }
                if let Some(job) = q.heap.pop() {
                    break job;
                }
                q = shared.wake.wait(q).unwrap();
            }
        };

        let finish = |shared: &Shared, path: &Path| {
            shared.queue.lock().unwrap().outstanding.remove(path);
        };

        // Retired by a folder switch while it sat in the queue.
        if job.generation != shared.generation.load(AtomicOrdering::SeqCst) {
            finish(&shared, &job.request.path);
            continue;
        }

        let started = std::time::Instant::now();
        let path = job.request.path.clone();

        let outcome = match kind {
            DecodeKind::Thumbnail { box_w, box_h } => {
                let key = thumb_key(&path, job.request.modified, job.request.len, box_w, box_h);
                if let Some(cached) = disk.load(key) {
                    DecodeOutcome::Ready {
                        path: path.clone(),
                        image: cached,
                        millis: started.elapsed().as_millis() as u32,
                    }
                } else {
                    match image_io::decode_thumbnail(&path, box_w, box_h) {
                        Ok(img) => {
                            disk.store(key, &img);
                            DecodeOutcome::Ready {
                                path: path.clone(),
                                image: img,
                                millis: started.elapsed().as_millis() as u32,
                            }
                        }
                        Err(e) => DecodeOutcome::Failed {
                            path: path.clone(),
                            message: e.to_string(),
                        },
                    }
                }
            }
            DecodeKind::Preview { max_dim } => match image_io::decode_preview(&path, max_dim) {
                Ok(img) => DecodeOutcome::Ready {
                    path: path.clone(),
                    image: img,
                    millis: started.elapsed().as_millis() as u32,
                },
                Err(e) => DecodeOutcome::Failed {
                    path: path.clone(),
                    message: e.to_string(),
                },
            },
        };

        // Check once more before publishing: the folder may have changed during
        // the decode itself.
        if job.generation != shared.generation.load(AtomicOrdering::SeqCst) {
            finish(&shared, &path);
            continue;
        }

        let dead = tx.send(outcome).is_err();
        finish(&shared, &path);
        if dead {
            return;
        }
    }
}

/// Split the available cores between the two loaders.
///
/// Previews get the larger share: a missing thumbnail is a grey rectangle in a
/// strip, but a missing preview is the thing the user is actually looking at.
pub fn worker_split() -> (usize, usize) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let preview = (cores / 3).clamp(2, 6);
    // Leave one core for the UI thread.
    let thumbs = cores.saturating_sub(preview + 1).max(1);
    (thumbs, preview)
}

// ---------------------------------------------------------------------------
// Folder scanning
// ---------------------------------------------------------------------------

/// Scans a folder and probes each frame's dimensions off the UI thread.
///
/// Dimensions are read here, from the file header only, rather than being
/// inferred once a thumbnail lands. That is what lets a filmstrip cell be laid
/// out at its final height before its image exists — the design requires the
/// pending placeholder to be at "exact final size so nothing shifts", and a
/// portrait frame resolving later would shift every cell below it.
pub struct ScanLoader {
    tx: Sender<ScanRequest>,
    rx: Receiver<ScanOutcome>,
    worker: Option<JoinHandle<()>>,
    generation: Arc<AtomicU64>,
}

struct ScanRequest {
    folder: PathBuf,
    generation: u64,
}

pub enum ScanOutcome {
    Progress {
        folder: PathBuf,
        found: usize,
    },
    Done {
        folder: PathBuf,
        frames: Vec<crate::model::Frame>,
    },
}

impl ScanLoader {
    pub fn new() -> Self {
        let (tx_req, rx_req) = channel::<ScanRequest>();
        let (tx_out, rx_out) = channel::<ScanOutcome>();
        let generation = Arc::new(AtomicU64::new(0));
        let gen_worker = Arc::clone(&generation);

        let worker = std::thread::Builder::new()
            .name("pickture-scan".into())
            .spawn(move || {
                while let Ok(mut job) = rx_req.recv() {
                    while let Ok(newer) = rx_req.try_recv() {
                        job = newer;
                    }
                    if job.generation != gen_worker.load(AtomicOrdering::SeqCst) {
                        continue;
                    }

                    let mut frames = crate::session::scan_folder(&job.folder);
                    let total = frames.len();

                    for (i, frame) in frames.iter_mut().enumerate() {
                        if job.generation != gen_worker.load(AtomicOrdering::SeqCst) {
                            break;
                        }
                        frame.dimensions = image_io::read_dimensions(&frame.path);
                        // Report often enough for the bar to move, rarely
                        // enough that a 1,200-frame folder does not flood the
                        // channel with 1,200 messages.
                        if i % 16 == 0
                            && tx_out
                                .send(ScanOutcome::Progress {
                                    folder: job.folder.clone(),
                                    found: i.min(total),
                                })
                                .is_err()
                        {
                            return;
                        }
                    }

                    if job.generation != gen_worker.load(AtomicOrdering::SeqCst) {
                        continue;
                    }
                    if tx_out
                        .send(ScanOutcome::Done {
                            folder: job.folder.clone(),
                            frames,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .expect("spawn scan worker");

        Self {
            tx: tx_req,
            rx: rx_out,
            worker: Some(worker),
            generation,
        }
    }

    pub fn request(&self, folder: PathBuf) {
        self.generation.fetch_add(1, AtomicOrdering::SeqCst);
        let _ = self.tx.send(ScanRequest {
            folder,
            generation: self.generation.load(AtomicOrdering::SeqCst),
        });
    }

    pub fn poll(&self) -> Vec<ScanOutcome> {
        self.rx.try_iter().collect()
    }
}

impl Default for ScanLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScanLoader {
    fn drop(&mut self) {
        let (dead, _) = channel();
        let _ = std::mem::replace(&mut self.tx, dead);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(priority: u32, seq: u64) -> Job {
        Job {
            request: DecodeRequest {
                path: PathBuf::from(format!("{seq}.jpg")),
                modified: None,
                len: 0,
                priority,
            },
            generation: 0,
            seq,
        }
    }

    #[test]
    fn nearest_frame_is_decoded_first() {
        let mut heap = BinaryHeap::new();
        heap.push(job(10, 1));
        heap.push(job(1, 2));
        heap.push(job(5, 3));
        assert_eq!(heap.pop().unwrap().request.priority, 1);
        assert_eq!(heap.pop().unwrap().request.priority, 5);
        assert_eq!(heap.pop().unwrap().request.priority, 10);
    }

    #[test]
    fn equal_priority_keeps_insertion_order() {
        let mut heap = BinaryHeap::new();
        heap.push(job(4, 1));
        heap.push(job(4, 2));
        heap.push(job(4, 3));
        assert_eq!(heap.pop().unwrap().seq, 1);
        assert_eq!(heap.pop().unwrap().seq, 2);
    }

    #[test]
    fn duplicate_requests_are_dropped_and_cancel_empties_the_queue() {
        let loader = ImageLoader::new(
            DecodeKind::Thumbnail {
                box_w: 190,
                box_h: 150,
            },
            1,
            None,
        );
        let req = || DecodeRequest {
            path: PathBuf::from("does-not-exist-anywhere.jpg"),
            modified: None,
            len: 0,
            priority: 500,
        };
        loader.request(req());
        loader.request(req());
        assert!(loader.pending() <= 1);
        loader.cancel_all();
        assert_eq!(loader.pending(), 0);
    }

    #[test]
    fn worker_split_leaves_room_for_the_ui() {
        let (thumbs, preview) = worker_split();
        assert!(thumbs >= 1);
        assert!(preview >= 2);
    }

    #[test]
    fn preview_kind_reports_failure_rather_than_hanging() {
        let loader = ImageLoader::new(DecodeKind::Preview { max_dim: 512 }, 1, None);
        loader.request(DecodeRequest {
            path: PathBuf::from("no-such-frame-anywhere.jpg"),
            modified: None,
            len: 0,
            priority: 0,
        });
        // Give the worker a moment, then confirm a Failed outcome arrives and
        // the path is released so it can be retried.
        let mut seen = false;
        for _ in 0..80 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            for outcome in loader.poll() {
                assert!(matches!(outcome, DecodeOutcome::Failed { .. }));
                seen = true;
            }
            if seen {
                break;
            }
        }
        assert!(seen, "no outcome was published for a missing file");
        assert!(!loader.is_outstanding(Path::new("no-such-frame-anywhere.jpg")));
    }
}
