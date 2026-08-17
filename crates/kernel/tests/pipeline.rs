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
