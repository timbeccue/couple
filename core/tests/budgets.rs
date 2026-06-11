//! §9.6 performance budgets on a 2 h synthetic pair (Apple Silicon, release).
//! Ignored by default (multi-minute generation); run via:
//!   cargo test --release -p couple-core --test budgets -- --ignored --nocapture
//! The fixture lands in fixtures/pair2h (gitignored) and is reused across runs.

mod common;

use couple_core::fixture::{generate, FixtureSpec};
use couple_core::job::{noop_progress, CancelToken, ProgressEmitter};
use couple_core::TrackRole;
use std::time::Instant;

#[test]
#[ignore]
fn budgets_2h_pair() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/pair2h");
    let ffmpeg = couple_core::ffmpeg::resolve_tool("ffmpeg", None).unwrap();
    let spec = FixtureSpec {
        offset_ms: -300_000.0, // external starts 5 min into the camera
        ppm: 12.0,
        dur_cam_s: 7200.0,
        dur_ext_s: 7300.0,
        snr_db: Some(15.0),
        gain_db: -6.0,
        seed: 4242,
        ..Default::default()
    };
    if !root.join("truth.json").is_file() {
        eprintln!("generating 2 h fixture pair (one-time, takes a while)…");
        let t = Instant::now();
        generate(&root, &spec, &ffmpeg).unwrap();
        eprintln!("fixture gen: {:.1} s", t.elapsed().as_secs_f64());
    }

    let ctx = common::test_ctx(&root.join("cache"));

    // Import budget: ≤ 90 s per 2 h file (fresh decode, not a cache hit).
    let _ = std::fs::remove_dir_all(root.join("cache"));
    let t = Instant::now();
    let cam_id = common::import(&ctx, &root.join("camera.wav"), TrackRole::Camera).unwrap();
    let cam_import_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let ext_id = common::import(&ctx, &root.join("external.wav"), TrackRole::External).unwrap();
    let ext_import_s = t.elapsed().as_secs_f64();
    eprintln!("import camera: {cam_import_s:.1} s, external: {ext_import_s:.1} s (budget 90 s)");
    assert!(cam_import_s <= 90.0 && ext_import_s <= 90.0);

    let cam = ctx.track(cam_id).unwrap();
    let ext = ctx.track(ext_id).unwrap();

    // Stage-1 budget: ≤ 3 s at 2 h × 2 h.
    let a = couple_core::envelope::envelope_full(&cam.pyramid, 9600);
    let b = couple_core::envelope::envelope_full(&ext.pyramid, 9600);
    let t = Instant::now();
    let s1 = couple_core::sync::stage1_scan(&a, &b, 100, 0.6).unwrap();
    let stage1_s = t.elapsed().as_secs_f64();
    eprintln!(
        "stage 1: {stage1_s:.3} s (budget 3 s), z = {:.1}, lag = {:.1} ms",
        s1.best_z,
        s1.best_lag as f64 * 200.0
    );
    assert!(stage1_s <= 3.0);
    assert!((s1.best_lag as f64 * 200.0 - spec.offset_ms).abs() <= 200.0);

    // Full offset pipeline (stages 1–5): well inside the 10 s auto-sync budget;
    // the drift pass is asserted in the Phase-3 budget run.
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let t = Instant::now();
    let r = couple_core::sync::offset_only_sync(
        &cam.pyramid,
        &ext.pyramid,
        &couple_core::sync::SyncParams::default(),
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    let sync_s = t.elapsed().as_secs_f64();
    eprintln!(
        "offset pipeline: {sync_s:.2} s, offset = {:.3} ms (truth −300000, drift present)",
        r.offset_ms
    );
    assert!(sync_s <= 10.0);
    // 12 ppm over ~2 h skews a single global offset; just sanity-check the ballpark.
    assert!((r.offset_ms - spec.offset_ms).abs() <= 100.0);

    // Full auto-sync budget (§9.6): ≤ 10 s at 2 h × 2 h including the drift pass.
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let t = Instant::now();
    let r = couple_core::sync::auto_sync_tracks(
        &cam.pyramid,
        &ext.pyramid,
        &couple_core::sync::SyncParams::default(),
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    let full_s = t.elapsed().as_secs_f64();
    eprintln!(
        "full auto-sync: {full_s:.2} s (budget 10 s) — drift {:.2} ppm (truth 12), rms {:?}",
        r.drift_ppm, r.fit_rms_ms
    );
    assert!(full_s <= 10.0);
    assert!(r.drift_measurable && (r.drift_ppm - 12.0).abs() <= 0.5);
    let l_meas = r.offset_ms + 3600.0 * r.drift_ppm * 1e-3;
    let l_true = -300_000.0 + 3600.0 * 12.0 * 1e-3;
    assert!((l_meas - l_true).abs() <= 1.0, "midpoint mapping err {:.3} ms", l_meas - l_true);

    // band() budget: ≤ 50 µs typical (median over 1000 random calls).
    use rand::{Rng, SeedableRng};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(9);
    let mut times = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let t0 = rng.gen_range(0.0..7000.0);
        let t1 = t0 + rng.gen_range(0.05..200.0);
        let cols = rng.gen_range(100..=2048);
        let t = Instant::now();
        let band = couple_core::band(&cam.pyramid, t0, t1, cols).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1e6);
        std::hint::black_box(band);
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    eprintln!("band(): median {median:.1} µs over 1000 calls (budget 50 µs typical)");
    assert!(median <= 50.0);
}

/// §9.6 export budget: 2 h render ≤ 90 s (+ stream-copy I/O, absent for WAV).
/// Resample mode = the CPU-worst path. Uses the pair2h fixture.
#[test]
#[ignore]
fn export_budget_2h() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/pair2h");
    assert!(
        root.join("truth.json").is_file(),
        "run budgets_2h_pair first to generate fixtures/pair2h"
    );
    let out = root.join("aligned_2h.wav");
    let _ = std::fs::remove_file(&out);
    let mapping = couple_core::export::ExportMapping {
        offset_ms: -300_000.0,
        drift_ppm: 12.0,
        resample: true,
    };
    let opts = couple_core::export::ExportOptions {
        kind: couple_core::export::ExportKind::Wav,
        output_path: out.clone(),
        keep_camera_audio: false,
        container: couple_core::export::Container::Auto,
        faststart: false,
        overwrite: true,
        allow_silence: false,
    };
    let ffmpeg = couple_core::ffmpeg::resolve_tool("ffmpeg", None).unwrap();
    let ffprobe = couple_core::ffmpeg::resolve_tool("ffprobe", None).unwrap();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let t = Instant::now();
    let report = couple_core::export::export(
        &ffmpeg,
        &ffprobe,
        &root.join("camera.wav"),
        &root.join("external.wav"),
        &mapping,
        &opts,
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    let secs = t.elapsed().as_secs_f64();
    eprintln!("2 h resample-mode WAV export: {secs:.1} s (budget 90 s), {} frames", report.frames);
    assert!(secs <= 90.0);
    let _ = std::fs::remove_file(&out);
}

/// §9.3 drift ladder, 1800 s row: ≤ ±0.2 ppm. Slow (fixture gen ~1 min), hence ignored.
#[test]
#[ignore]
fn drift_ladder_1800s_slow() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/pair1800");
    let ffmpeg = couple_core::ffmpeg::resolve_tool("ffmpeg", None).unwrap();
    let spec = FixtureSpec {
        offset_ms: -4000.0,
        ppm: 30.0,
        dur_cam_s: 1800.0,
        dur_ext_s: 1810.0,
        snr_db: Some(15.0),
        gain_db: -6.0,
        seed: 1800,
        ..Default::default()
    };
    if !root.join("truth.json").is_file() {
        generate(&root, &spec, &ffmpeg).unwrap();
    }
    let ctx = common::test_ctx(&root.join("cache"));
    let cam_id = common::import(&ctx, &root.join("camera.wav"), TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &root.join("external.wav"), TrackRole::External).unwrap();
    let cam = ctx.track(cam_id).unwrap();
    let ext = ctx.track(ext_id).unwrap();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let r = couple_core::sync::auto_sync_tracks(
        &cam.pyramid,
        &ext.pyramid,
        &couple_core::sync::SyncParams::default(),
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    eprintln!("1800 s row: drift {:.4} ppm (truth 30), rms {:?}", r.drift_ppm, r.fit_rms_ms);
    assert!(r.drift_measurable);
    assert!((r.drift_ppm - 30.0).abs() <= 0.2, "ppm err {:.4}", r.drift_ppm - 30.0);
}
