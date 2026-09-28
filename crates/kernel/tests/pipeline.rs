//! End-to-end checks over the real decode → effect → encode → write path.
//!
//! These build their own fixtures, so the suite needs no sample photographs
//! checked into the repository.

use image::{Rgba, RgbaImage};
use pickture_kernel::{
    image_io, pixel_ops,
    session::{scan_folder, Session},
    EffectMode, EffectSpec, PersistedSession,
};
use std::path::{Path, PathBuf};

fn fixture_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pickture-it-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A horizontal ramp, so levels changes are measurable rather than a matter of
/// opinion.
fn ramp(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = (40.0 + 170.0 * (x as f32 / w as f32)) as u8;
            img.put_pixel(
                x,
                y,
                Rgba([v, (v as f32 * 0.85) as u8, (v as f32 * 0.7) as u8, 255]),
            );
        }
    }
    img
}

fn write_jpeg(dir: &Path, name: &str, img: &RgbaImage) -> PathBuf {
    let path = dir.join(name);
    let bytes = image_io::encode(img, &path, 92).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Write a JPEG carrying an EXIF `Orientation`, the way a camera does.
///
/// The pixels go down sideways and the tag says which way up they belong —
/// which is the case the whole orientation path exists for.
fn write_jpeg_with_orientation(
    dir: &Path,
    name: &str,
    img: &RgbaImage,
    orientation: u16,
) -> PathBuf {
    let path = dir.join(name);
    let encoded = image_io::encode(img, &path, 92).unwrap();

    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42u16.to_le_bytes());
    tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 follows the header
    tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
    tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_le_bytes()); // count
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]); // rest of the value field
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no IFD1

    let payload_len = 6 + tiff.len();
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);

    let mut out = Vec::with_capacity(encoded.len() + app1.len());
    out.extend_from_slice(&encoded[..2]); // SOI
    out.extend_from_slice(&app1);
    out.extend_from_slice(&encoded[2..]);
    std::fs::write(&path, out).unwrap();
    path
}

fn orientation_of(path: &Path) -> Option<u16> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(|v| v as u16)
}

/// The frame the user rotated must come out of the destination rotated.
///
/// This is the whole of the bug it pins. Every decode bakes the EXIF
/// orientation into the pixels, and the user's quarter turns are applied on top
/// of that — so carrying the original tag across told the next viewer to turn
/// the frame a second time, and a portrait frame turned 90° by hand came back
/// out looking exactly like the original.
#[test]
fn a_rotation_survives_the_write_of_a_frame_that_was_shot_sideways() {
    let dir = fixture_dir("orientation");

    // 300x200 on disk, white along the top edge, tagged "turn me 90° clockwise
    // to display" — so Pickture shows it 200x300 with the white edge on the
    // right.
    let mut source = RgbaImage::from_pixel(300, 200, Rgba([40, 40, 200, 255]));
    for y in 0..24 {
        for x in 0..300 {
            source.put_pixel(x, y, Rgba([255, 255, 255, 255]));
        }
    }
    let src = write_jpeg_with_orientation(&dir, "_DSC9.jpg", &source, 6);

    let decoded = image_io::decode_full(&src).unwrap();
    assert_eq!(
        decoded.dimensions(),
        (200, 300),
        "the decode should already stand the frame up"
    );

    // The user turns it one more quarter, clockwise.
    let spec = EffectSpec {
        quarter_turns: 1,
        ..Default::default()
    };
    let processed = pixel_ops::apply_all(decoded, &spec);
    assert_eq!(
        processed.dimensions(),
        (300, 200),
        "two turns from the file"
    );

    let out_path = dir.join("_DSC9_ORIG.jpg");
    let bytes = image_io::encode(&processed, &out_path, 95).unwrap();
    let bytes = image_io::carry_exif(bytes, &src, &out_path);
    std::fs::write(&out_path, &bytes).unwrap();

    // The tag must now say upright. Anything else and the viewer turns the
    // frame again and the user's rotation looks lost.
    assert_eq!(
        orientation_of(&out_path),
        Some(1),
        "the written frame must not ask to be turned again"
    );

    // And the pixels must actually carry the rotation: white along the top of
    // the file, two clockwise turns, lands along the bottom.
    let written = image::open(&out_path).unwrap().to_rgba8();
    assert_eq!(written.dimensions(), (300, 200));
    let bottom = written.get_pixel(150, 196).0;
    let top = written.get_pixel(150, 4).0;
    assert!(
        bottom[0] > 200 && bottom[1] > 200 && bottom[2] > 200,
        "expected the white edge at the bottom, found {bottom:?}"
    );
    assert!(
        top[2] > 150 && top[0] < 100,
        "expected blue on top, found {top:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The one invariant the whole tool rests on: an original is never written to.
///
/// Every operation — colour, turns, fine angle, crop — runs on a buffer decoded
/// from the source and lands in a copy. This walks the write path with all four
/// applied at once and checks the source afterwards byte for byte, including its
/// modification time, because `carry_file_times` writes a timestamp and pointing
/// it at the wrong file would be the quietest possible way to break this.
#[test]
fn every_operation_leaves_the_original_untouched() {
    let dir = fixture_dir("untouched");
    let src = write_jpeg(&dir, "_DSC7.jpg", &ramp(400, 300));

    let before = std::fs::read(&src).unwrap();
    let before_meta = std::fs::metadata(&src).unwrap();
    let before_modified = before_meta.modified().unwrap();

    let spec = EffectSpec {
        mode: EffectMode::Levels,
        low: 12,
        high: 240,
        gamma: 1.3,
        per_channel: true,
        rgb: [
            pickture_kernel::Levels {
                low: 8,
                high: 250,
                gamma: 1.1,
            },
            pickture_kernel::Levels::default(),
            pickture_kernel::Levels {
                low: 0,
                high: 230,
                gamma: 0.9,
            },
        ],
        quarter_turns: 1,
        angle: 3.5,
        crop_ratio: pickture_kernel::CropRatio::Square,
        crop: pickture_kernel::CropRect {
            x: 0.1,
            y: 0.05,
            w: 0.7,
            h: 0.7,
        },
        crop_swap: false,
    };

    // The destination is a sibling folder, as `validate` insists.
    let dest = dir.join("selection");
    std::fs::create_dir_all(&dest).unwrap();
    let out_path = dest.join("_DSC7_CUSTOM.jpg");

    let processed = pixel_ops::apply_all(image_io::decode_full(&src).unwrap(), &spec);
    let bytes = image_io::encode(&processed, &out_path, 95).unwrap();
    let bytes = image_io::carry_exif(bytes, &src, &out_path);
    std::fs::write(&out_path, &bytes).unwrap();
    image_io::carry_file_times(&src, &out_path).unwrap();

    // The copy exists and carries the work.
    let written = image::open(&out_path).unwrap();
    assert!(written.width() > 0 && written.height() > 0);
    assert!(
        written.width() < 300 && written.height() < 400,
        "the copy should be cropped, got {}x{}",
        written.width(),
        written.height()
    );

    // The original is exactly as it was.
    assert_eq!(
        std::fs::read(&src).unwrap(),
        before,
        "the source bytes changed"
    );
    let after_meta = std::fs::metadata(&src).unwrap();
    assert_eq!(
        after_meta.len(),
        before_meta.len(),
        "the source size changed"
    );
    assert_eq!(
        after_meta.modified().unwrap(),
        before_modified,
        "the source modification time changed"
    );

    // And nothing but the copy appeared anywhere.
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, vec!["_DSC7.jpg".to_string(), "selection".to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A crop is part of what gets written, not just of what is shown.
#[test]
fn a_cropped_keep_is_written_cropped() {
    let dir = fixture_dir("crop");
    let src = write_jpeg(&dir, "_DSC4.jpg", &ramp(400, 300));

    // A centred square of a 4:3 frame: the largest 1:1 crop is 300x300.
    let aspect = 400.0 / 300.0;
    let spec = EffectSpec {
        crop_ratio: pickture_kernel::CropRatio::Square,
        crop: pickture_kernel::CropRect::centred(aspect, 1.0),
        ..Default::default()
    };
    let processed = pixel_ops::apply_all(image_io::decode_full(&src).unwrap(), &spec);
    let (w, h) = processed.dimensions();
    assert!(
        (w as i32 - h as i32).abs() <= 1,
        "a 1:1 crop should be square, got {w}x{h}"
    );
    assert!(
        (h as i32 - 300).abs() <= 1,
        "it should keep the full height"
    );

    let out_path = dir.join("_DSC4_ORIG.jpg");
    let bytes = image_io::encode(&processed, &out_path, 95).unwrap();
    std::fs::write(&out_path, &bytes).unwrap();
    let written = image::open(&out_path).unwrap();
    assert_eq!(written.width(), w);
    assert_eq!(written.height(), h);

    let _ = std::fs::remove_dir_all(&dir);
}

/// The picker cannot say how long a folder will take unless the scan says how
/// many frames there are, and it cannot say anything at all unless the scan
/// speaks before it has finished.
#[test]
fn a_scan_states_its_total_before_it_starts_reading() {
    use pickture_kernel::jobs::{ScanLoader, ScanOutcome};

    let dir = fixture_dir("scan-progress");
    let img = ramp(80, 60);
    for i in 0..5 {
        write_jpeg(&dir, &format!("_DSC{i}.jpg"), &img);
    }

    let loader = ScanLoader::new();
    loader.request(dir.clone());

    let mut first_progress = None;
    let mut frames = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while frames.is_none() && std::time::Instant::now() < deadline {
        for outcome in loader.poll() {
            match outcome {
                ScanOutcome::Progress {
                    probed,
                    total,
                    folder,
                } => {
                    assert_eq!(folder, dir);
                    if first_progress.is_none() {
                        first_progress = Some((probed, total));
                    }
                }
                ScanOutcome::Done { frames: f, .. } => frames = Some(f),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert_eq!(
        first_progress,
        Some((0, 5)),
        "the first word should be the total, before any header is read"
    );

    let frames = frames.expect("the scan should finish");
    assert_eq!(frames.len(), 5);
    // Every cell has to be layable out at its final height, so no frame may
    // arrive without its dimensions.
    for frame in &frames {
        assert_eq!(
            frame.dimensions,
            Some((80, 60)),
            "{} came back unprobed",
            frame.id
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two folders asked for in quick succession: the newest must arrive, and it
/// must not be the one that loses to a stale generation.
#[test]
fn the_last_folder_asked_for_is_the_one_delivered() {
    use pickture_kernel::jobs::{ScanLoader, ScanOutcome};

    let first = fixture_dir("scan-first");
    let second = fixture_dir("scan-second");
    let img = ramp(40, 30);
    write_jpeg(&first, "_A1.jpg", &img);
    for i in 0..3 {
        write_jpeg(&second, &format!("_B{i}.jpg"), &img);
    }

    let loader = ScanLoader::new();
    loader.request(first.clone());
    loader.request(second.clone());

    let mut delivered = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        for outcome in loader.poll() {
            if let ScanOutcome::Done { folder, frames } = outcome {
                delivered.push((folder, frames.len()));
            }
        }
        if delivered.iter().any(|(f, _)| f == &second) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(
        delivered.iter().any(|(f, n)| f == &second && *n == 3),
        "the folder asked for last was never delivered: {delivered:?}"
    );
    assert!(
        !delivered.iter().any(|(f, _)| f == &first),
        "a cancelled scan delivered anyway: {delivered:?}"
    );

    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);
}

#[test]
fn scan_finds_supported_frames_in_natural_order() {
    let dir = fixture_dir("scan");
    let img = ramp(64, 48);
    for name in ["_DSC10.jpg", "_DSC2.jpg", "_DSC1.jpg"] {
        write_jpeg(&dir, name, &img);
    }
    std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
    std::fs::write(dir.join("raw.arw"), b"unsupported for now").unwrap();

    let frames = scan_folder(&dir);
    let names: Vec<_> = frames.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(names, vec!["_DSC1.jpg", "_DSC2.jpg", "_DSC10.jpg"]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn thumbnails_decode_far_smaller_than_the_source() {
    let dir = fixture_dir("thumb");
    // Large enough that a DCT-scaled decode is meaningfully cheaper.
    let path = write_jpeg(&dir, "big.jpg", &ramp(2400, 1600));

    let thumb = image_io::decode_thumbnail(&path, 300, 300).unwrap();
    assert!(thumb.width() <= 300 && thumb.height() <= 300);
    // Aspect is preserved, which is what the filmstrip lays cells out against.
    let ratio = thumb.width() as f32 / thumb.height() as f32;
    assert!((ratio - 1.5).abs() < 0.05, "ratio was {ratio}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_decodes_to_display_size_not_sensor_size() {
    let dir = fixture_dir("preview");
    let path = write_jpeg(&dir, "big.jpg", &ramp(3000, 2000));

    let preview = image_io::decode_preview(&path, 1024).unwrap();
    assert!(
        preview.width() <= 1024,
        "preview was {} wide",
        preview.width()
    );
    assert!(preview.width() >= 512, "scaled too far down");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn levels_actually_change_pixels_and_stay_in_range() {
    let mut img = ramp(256, 8);
    let before = *img.get_pixel(200, 4);
    pixel_ops::levels_custom(&mut img, 40, 210, 1.0);
    let after = *img.get_pixel(200, 4);

    assert_ne!(before, after, "levels made no difference");
    // The white point maps the bright end up, never past 255.
    assert!(after.0[0] >= before.0[0]);
    assert!(img.pixels().all(|p| p.0[3] == 255), "alpha was disturbed");
}

#[test]
fn white_balance_rgb_neutralises_a_cast() {
    // A frame with a strong blue deficiency; per-channel stretching should pull
    // the channels back toward each other.
    let mut img = RgbaImage::new(128, 32);
    for y in 0..32 {
        for x in 0..128 {
            let v = (30 + x) as u8;
            img.put_pixel(x, y, Rgba([v, v, v / 3, 255]));
        }
    }
    let before = *img.get_pixel(120, 16);
    pixel_ops::white_balance_rgb(&mut img, 0.05);
    let after = *img.get_pixel(120, 16);

    let spread_before = before.0[0].abs_diff(before.0[2]);
    let spread_after = after.0[0].abs_diff(after.0[2]);
    assert!(
        spread_after < spread_before,
        "cast not reduced: {spread_before} -> {spread_after}"
    );
}

#[test]
fn every_effect_mode_produces_a_valid_image() {
    let base = ramp(200, 120);
    for mode in [
        EffectMode::None,
        EffectMode::WbValue,
        EffectMode::WbRgb,
        EffectMode::Levels,
    ] {
        let spec = EffectSpec {
            mode,
            low: 20,
            high: 235,
            gamma: 1.2,
            quarter_turns: 0,
            angle: 0.0,
            ..Default::default()
        };
        let out = pixel_ops::apply_all(base.clone(), &spec);
        assert_eq!(out.dimensions(), (200, 120), "mode {mode:?} changed size");
        assert!(
            out.pixels().all(|p| p.0[3] == 255),
            "mode {mode:?} lost alpha"
        );
    }
}

#[test]
fn quarter_turn_then_fine_angle_keeps_the_original_ratio() {
    let base = ramp(300, 200);
    let spec = EffectSpec {
        mode: EffectMode::None,
        low: 0,
        high: 255,
        gamma: 1.0,
        quarter_turns: 1,
        angle: 4.0,
        ..Default::default()
    };
    let out = pixel_ops::apply_all(base, &spec);
    // A quarter turn swaps the axes, so the target ratio is 200:300.
    let ratio = out.width() as f32 / out.height() as f32;
    assert!((ratio - (200.0 / 300.0)).abs() < 0.06, "ratio was {ratio}");
}

#[test]
fn saved_file_matches_its_extension_and_carries_exif() {
    // The MAUI build encoded PNG bytes into a file named `.jpg`, and dropped the
    // EXIF block entirely. Both are regressions worth pinning.
    let dir = fixture_dir("save");
    let src = write_jpeg(&dir, "_DSC1.jpg", &ramp(120, 80));

    let out_path = dir.join("_DSC1_WBV.jpg");
    let processed =
        pixel_ops::apply_all(image_io::decode_full(&src).unwrap(), &EffectSpec::default());
    let bytes = image_io::encode(&processed, &out_path, 95).unwrap();
    let bytes = image_io::carry_exif(bytes, &src, &out_path);
    std::fs::write(&out_path, &bytes).unwrap();

    // Really a JPEG, not PNG bytes wearing a .jpg name.
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "not a JPEG");
    let reloaded = image::ImageReader::open(&out_path)
        .unwrap()
        .with_guessed_format()
        .unwrap();
    assert_eq!(reloaded.format(), Some(image::ImageFormat::Jpeg));

    // And a PNG destination really is a PNG.
    let png_path = dir.join("_DSC1_WBV.png");
    let png = image_io::encode(&processed, &png_path, 95).unwrap();
    assert_eq!(&png[1..4], b"PNG");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn already_kept_frames_are_detected_across_every_suffix() {
    let dir = fixture_dir("kept");
    let img = ramp(64, 48);
    for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg"] {
        write_jpeg(&dir, name, &img);
    }

    let selection = dir.join("selection");
    std::fs::create_dir_all(&selection).unwrap();
    // One file per suffix the app can produce, plus a collision-renamed one.
    write_jpeg(&selection, "a_ORIG.jpg", &img);
    write_jpeg(&selection, "b_WBV.jpg", &img);
    write_jpeg(&selection, "c_WBRGB.jpg", &img);
    write_jpeg(&selection, "d_CUSTOM.jpg", &img);
    write_jpeg(&selection, "e_ORIG_2.jpg", &img);

    let session = Session::open(dir.clone(), PersistedSession::default());
    for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg"] {
        assert!(
            session.is_in_destination(&name.to_string()),
            "{name} was not recognised as already kept"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switching_folders_preserves_each_sessions_cursor_and_counts() {
    let a = fixture_dir("switch-a");
    let b = fixture_dir("switch-b");
    let img = ramp(64, 48);
    for i in 0..5 {
        write_jpeg(&a, &format!("a{i}.jpg"), &img);
        write_jpeg(&b, &format!("b{i}.jpg"), &img);
    }

    let mut store = pickture_kernel::SessionStore::default();

    // Work in A, land on the third frame, keep one.
    let mut sa = Session::open(a.clone(), store.get(&a));
    sa.cursor = 2;
    sa.judgement
        .insert("a1.jpg".to_string(), pickture_kernel::Judgement::Kept);
    store.put(a.clone(), sa.to_persisted());
    store.touch_recent(&a);

    // Switch to B and work there.
    let mut sb = Session::open(b.clone(), store.get(&b));
    sb.cursor = 4;
    store.put(b.clone(), sb.to_persisted());
    store.touch_recent(&b);

    // Coming back to A lands on the frame we left, with counts intact.
    let reopened = Session::open(a.clone(), store.get(&a));
    assert_eq!(reopened.cursor, 2, "cursor was not restored");
    assert_eq!(reopened.kept_count(), 1, "keeps were lost");
    assert_eq!(
        store.recent.first(),
        Some(&b),
        "recent list should be most-recent-first"
    );
    assert!(store.progress_line(&a).contains("1 kept"));

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn destination_choices_resolve_to_distinct_folders() {
    use pickture_kernel::Destination;
    let working = PathBuf::from("D:/shoots/harbour");

    let default = Destination::InWorkingFolder("selection".into());
    assert_eq!(default.resolve(&working), working.join("selection"));

    let absolute = Destination::Absolute(PathBuf::from("E:/deliver/picks"));
    assert_eq!(
        absolute.resolve(&working),
        PathBuf::from("E:/deliver/picks")
    );

    let dated = Destination::Dated {
        root: "selection".into(),
        date: "2026-08-16".into(),
    };
    assert_eq!(
        dated.resolve(&working),
        working.join("selection").join("2026-08-16")
    );

    // A second pass never collides with the first.
    assert_ne!(dated.resolve(&working), default.resolve(&working));
}

/// What a folder costs to open, which is the wait before the first frame.
///
/// Ignored by default because it builds 400 fixtures. Run it with:
/// `cargo test -p pickture-kernel --release --test pipeline -- --ignored --nocapture`
#[test]
#[ignore]
fn scan_cost_on_a_full_card() {
    use pickture_kernel::jobs::{ScanLoader, ScanOutcome};
    use rayon::prelude::*;

    const N: usize = 400;
    let dir = fixture_dir("scan-cost");
    let img = ramp(1200, 800);
    for i in 0..N {
        write_jpeg(&dir, &format!("_DSC{i:04}.jpg"), &img);
    }
    println!("\nfixture: {N} frames at 1200x800");

    let listed = std::time::Instant::now();
    let frames = scan_folder(&dir);
    println!(
        "  list the folder             {:>8.1} ms",
        listed.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(frames.len(), N);

    // What the probe used to cost: one header at a time on one thread.
    let serial = std::time::Instant::now();
    for frame in &frames {
        let _ = image_io::read_dimensions(&frame.path);
    }
    let serial = serial.elapsed().as_secs_f64() * 1000.0;
    println!("  probe headers, serial       {serial:>8.1} ms");

    let parallel = std::time::Instant::now();
    frames.par_iter().for_each(|frame| {
        let _ = image_io::read_dimensions(&frame.path);
    });
    let parallel = parallel.elapsed().as_secs_f64() * 1000.0;
    println!("  probe headers, in parallel  {parallel:>8.1} ms");
    println!("  speedup                     {:>8.1}x", serial / parallel);

    // End to end, as the app sees it: request to `Done`.
    let loader = ScanLoader::new();
    let started = std::time::Instant::now();
    loader.request(dir.clone());
    let mut first_word = None;
    loop {
        let mut done = false;
        for outcome in loader.poll() {
            match outcome {
                ScanOutcome::Progress { .. } if first_word.is_none() => {
                    first_word = Some(started.elapsed().as_secs_f64() * 1000.0);
                }
                ScanOutcome::Done { .. } => done = true,
                _ => {}
            }
        }
        if done {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    println!(
        "  first word to the picker    {:>8.1} ms",
        first_word.unwrap_or(f64::NAN)
    );
    println!(
        "  request to gallery          {:>8.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Timing check against a realistic 24 MP frame.
///
/// Ignored by default because it builds a large fixture. Run it with:
/// `cargo test -p pickture-kernel --release --test pipeline -- --ignored --nocapture`
#[test]
#[ignore]
fn decode_budget_on_a_24mp_frame() {
    let dir = fixture_dir("budget");
    let path = write_jpeg(&dir, "big.jpg", &ramp(6000, 4000));
    let size_mb = std::fs::metadata(&path).unwrap().len() as f64 / 1_048_576.0;
    println!("\nfixture: 6000x4000, {size_mb:.1} MB on disk");

    let time = |label: &str, f: &dyn Fn()| {
        // One warm-up so the file is in the OS cache, then three timed runs.
        f();
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            f();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("  {label:<28} {best:>8.1} ms");
        best
    };

    let thumb = time("thumbnail (300px box)", &|| {
        image_io::decode_thumbnail(&path, 300, 300).unwrap();
    });
    let preview = time("preview (2048px)", &|| {
        image_io::decode_preview(&path, 2048).unwrap();
    });
    let proxy = time("enhance proxy (1600px)", &|| {
        image_io::decode_preview(&path, 1600).unwrap();
    });
    let full = time("full decode", &|| {
        image_io::decode_full(&path).unwrap();
    });

    println!(
        "\n  preview is {:.2}x the cost of a full decode",
        preview / full.max(0.001)
    );
    println!(
        "  a 10-frame prefetch costs ~{:.0} ms of background work\n",
        preview * 10.0
    );

    // The numbers that matter: a thumbnail must be cheap enough to fill a strip
    // of hundreds, and a preview cheap enough that a prefetch keeps ahead of a
    // person holding the arrow key.
    assert!(
        thumb < full,
        "thumbnail should be cheaper than a full decode"
    );
    assert!(
        proxy <= preview * 1.5,
        "proxy should not cost more than a preview"
    );
    assert!(
        preview < full * 1.2,
        "preview at {preview:.0} ms is not worth it against a {full:.0} ms full decode"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unreadable_files_report_rather_than_panic() {
    let dir = fixture_dir("broken");
    let path = dir.join("truncated.jpg");
    // A JPEG header and nothing else.
    std::fs::write(&path, [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).unwrap();

    assert!(image_io::decode_thumbnail(&path, 300, 300).is_err());
    assert!(image_io::decode_preview(&path, 1024).is_err());
    assert!(image_io::decode_full(&path).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}
