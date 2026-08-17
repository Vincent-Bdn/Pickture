//! The write path, on a background thread.
//!
//! The MAUI build did this: encode to PNG in memory → write those bytes to
//! `Path.GetTempFileName()` → re-open and re-decode that file inside
//! `RotateAndCrop(string, ...)` → encode to PNG again. Three encode/decode
//! round-trips and a temp file, to apply one rotation.
//!
//! Here the frame is decoded once, every operation runs on the buffer, and it
//! is encoded once — in the format the extension actually names, with the
//! original's EXIF block carried across.

use pickture_kernel::{image_io, pixel_ops, EffectSpec};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;

pub struct WriteJob {
    pub source: PathBuf,
    pub destination_dir: PathBuf,
    pub stem: String,
    pub extension: String,
    pub spec: EffectSpec,
    /// Identifies the frame so the UI can mark it kept when this lands.
    pub frame_id: String,
}

#[derive(Debug, Clone)]
pub enum WriteProgress {
    Started {
        frame_id: String,
    },
    /// 0.0 ..= 1.0, coarse but honest — each step is a real stage, not a timer.
    Step {
        frame_id: String,
        fraction: f32,
    },
    Finished(WriteOutcome),
}

#[derive(Debug, Clone)]
pub struct WriteOutcome {
    pub frame_id: String,
    pub written: Option<PathBuf>,
    /// Set when the name collided and `_2` was appended, so the status bar can
    /// say so rather than silently renaming.
    pub renamed: bool,
    pub error: Option<String>,
}

pub struct Writer {
    tx: Sender<WriteJob>,
    rx: Receiver<WriteProgress>,
    worker: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn new() -> Self {
        let (tx_job, rx_job) = channel::<WriteJob>();
        let (tx_out, rx_out) = channel::<WriteProgress>();

        let worker = std::thread::Builder::new()
            .name("pickture-writer".into())
            .spawn(move || {
                while let Ok(job) = rx_job.recv() {
                    let id = job.frame_id.clone();
                    let _ = tx_out.send(WriteProgress::Started {
                        frame_id: id.clone(),
                    });
                    let outcome = run(&job, &tx_out);
                    let _ = tx_out.send(WriteProgress::Finished(outcome));
                }
            })
            .expect("spawn writer");

        Self {
            tx: tx_job,
            rx: rx_out,
            worker: Some(worker),
        }
    }

    pub fn submit(&self, job: WriteJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<WriteProgress> {
        self.rx.try_iter().collect()
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let (dead, _) = channel();
        let _ = std::mem::replace(&mut self.tx, dead);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn run(job: &WriteJob, tx: &Sender<WriteProgress>) -> WriteOutcome {
    let id = job.frame_id.clone();
    let step = |f: f32| {
        let _ = tx.send(WriteProgress::Step {
            frame_id: id.clone(),
            fraction: f,
        });
    };

    let fail = |msg: String| WriteOutcome {
        frame_id: job.frame_id.clone(),
        written: None,
        renamed: false,
        error: Some(msg),
    };

    if let Err(e) = std::fs::create_dir_all(&job.destination_dir) {
        return fail(format!("can't create the destination — {e}"));
    }

    let (out_path, renamed) =
        crate::output_path(&job.destination_dir, &job.stem, &job.spec, &job.extension);

    step(0.15);
    let decoded = match image_io::decode_full(&job.source) {
        Ok(img) => img,
        Err(e) => return fail(format!("{} can't be read — {e}", job.stem)),
    };

    step(0.55);
    let processed = pixel_ops::apply_all(decoded, &job.spec);

    step(0.80);
    let bytes = match image_io::encode(&processed, &out_path, 95) {
        Ok(b) => b,
        Err(e) => return fail(format!("can't encode {} — {e}", out_path.display())),
    };

    // Carry the original EXIF block across. The MAUI build lost it on every
    // save, so date taken, camera, lens and GPS never survived a keep.
    let bytes = image_io::carry_exif(bytes, &job.source, &out_path);

    if let Err(e) = write_atomic(&out_path, &bytes) {
        return fail(format!("can't write {} — {e}", out_path.display()));
    }

    let _ = image_io::carry_file_times(&job.source, &out_path);
    step(1.0);

    WriteOutcome {
        frame_id: job.frame_id.clone(),
        written: Some(out_path),
        renamed,
        error: None,
    }
}

/// Write beside the target and rename, so an interrupted keep never leaves a
/// half-written frame that looks like a successful one.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("pickture-tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
