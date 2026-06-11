//! Phase 2 gates: fixture truth tests (§9.1), FFT/brute equivalence (§5.2), §9.3
//! offset + partial-overlap + hostile-noise rows, spurious-peak regression.

mod common;

use couple_core::fixture::{generate, FixtureSpec};
use couple_core::job::{noop_progress, CancelToken, ProgressEmitter};
use couple_core::sync::*;
use couple_core::TrackRole;
use std::path::Path;
use std::sync::Arc;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg required")
}

/// Generate, import both files, run the offset-only pipeline.
fn run_sync(dir: &Path, spec: &FixtureSpec, params: &SyncParams) -> SyncResult {
    let truth = generate(dir, spec, &ffmpeg()).expect("fixture gen");
    sync_files(
        dir,
        &dir.join(&truth.camera_file),
        &dir.join(&truth.external_file),
        params,
    )
}

fn sync_files(dir: &Path, cam: &Path, ext: &Path, params: &SyncParams) -> SyncResult {
    let ctx = common::test_ctx(&dir.join("cache"));
    let cam_id = common::import(&ctx, cam, TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, ext, TrackRole::External).unwrap();
    let cam = ctx.track(cam_id).unwrap();
    let ext = ctx.track(ext_id).unwrap();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    offset_only_sync(&cam.pyramid, &ext.pyramid, params, &mut prog, &CancelToken::new()).unwrap()
}

fn assert_offset(r: &SyncResult, truth_ms: f64, tol_ms: f64, label: &str) {
    assert!(
        matches!(r.status, SyncStatus::Synced | SyncStatus::LowConfidence),
        "{label}: failed: {:?}",
        r.status
    );
    let err = r.offset_ms - truth_ms;
    assert!(
        err.abs() <= tol_ms,
        "{label}: offset {} vs truth {} (err {:.3} ms > {tol_ms})",
        r.offset_ms,
        truth_ms,
        err
    );
}

// ---------- §9.1 fixture truth-table tests (FIRST, per the session plan) ----------

#[test]
fn fixture_truth_files_and_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -3000.0,
        dur_cam_s: 30.0,
        dur_ext_s: 40.0,
        snr_db: Some(20.0),
        seed: 21,
        ..Default::default()
    };
    let truth = generate(dir.path(), &spec, &ffmpeg()).unwrap();
    assert_eq!(truth.camera_file, "camera.wav");
    // External starts 3 s into the camera; overlap to camera end.
    assert!((truth.overlap_t0 - 3.0).abs() < 1e-9);
    assert!((truth.overlap_t1 - 30.0).abs() < 1e-9);

    // Probed durations match the spec.
    let ctx = common::test_ctx(&dir.path().join("cache"));
    let cam = ctx.probe(&dir.path().join("camera.wav")).unwrap();
    let ext = ctx.probe(&dir.path().join("external.wav")).unwrap();
    assert!((cam.duration_s - 30.0).abs() < 0.01, "cam dur {}", cam.duration_s);
    assert!((ext.duration_s - 40.0).abs() < 0.01, "ext dur {}", ext.duration_s);
    assert_eq!(ext.audio[0].bits_per_sample, Some(24));

    // truth.json round-trips.
    let parsed: couple_core::fixture::FixtureTruth =
        serde_json::from_slice(&std::fs::read(dir.path().join("truth.json")).unwrap()).unwrap();
    assert_eq!(parsed.spec.seed, 21);
}

#[test]
fn fixture_is_deterministic_per_seed() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        dur_cam_s: 10.0,
        dur_ext_s: 12.0,
        seed: 33,
        ..Default::default()
    };
    generate(d1.path(), &spec, &ffmpeg()).unwrap();
    generate(d2.path(), &spec, &ffmpeg()).unwrap();
    for f in ["camera.wav", "external.wav"] {
        let a = std::fs::read(d1.path().join(f)).unwrap();
        let b = std::fs::read(d2.path().join(f)).unwrap();
        assert_eq!(a, b, "{f} not deterministic");
    }
}

/// Ground-truth self-check: camera samples must correlate with external samples read
/// through u(t) far better than at a wrong offset (validates the warp direction/sign).
#[test]
fn fixture_warp_matches_truth_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 2500.0,
        ppm: 0.0,
        dur_cam_s: 30.0,
        dur_ext_s: 40.0,
        snr_db: None,
        agc: false,
        gain_db: 0.0,
        seed: 4,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let ctx = common::test_ctx(&dir.path().join("cache"));
    let cam_id = common::import(&ctx, &dir.path().join("camera.wav"), TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &dir.path().join("external.wav"), TrackRole::External).unwrap();
    let cam = ctx.track(cam_id).unwrap();
    let ext = ctx.track(ext_id).unwrap();

    let n = 48_000 * 4;
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    // camera t ∈ [10, 14) ↔ external u = t + 2.5
    cam.raw_window(10.0, 14.0, &mut a).unwrap();
    ext.raw_window(12.5, 16.5, &mut b).unwrap();
    let r_true = pearson(&a, &b);
    ext.raw_window(12.0, 16.0, &mut b).unwrap();
    let r_wrong = pearson(&a, &b);
    assert!(r_true > 0.9, "true-mapping correlation {r_true}");
    assert!(r_wrong < 0.3, "wrong-offset correlation {r_wrong}");
}

fn pearson(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let ma = a.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mb = b.iter().map(|&v| v as f64).sum::<f64>() / n;
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for i in 0..a.len() {
        let x = a[i] as f64 - ma;
        let y = b[i] as f64 - mb;
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    sab / (saa * sbb).sqrt().max(1e-30)
}

// ---------- §5.2: FFT path == brute force (winner identical, z within 1e−6) ----------

#[test]
fn fft_cross_equals_brute_force() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(2024);
    for (na, nb) in [(300usize, 400usize), (101, 1000), (777, 333)] {
        let a: Vec<f64> = (0..na).map(|_| rng.gen_range(0.0..5.5)).collect();
        let b: Vec<f64> = (0..nb).map(|_| rng.gen_range(0.0..5.5)).collect();
        let s_fft = stage1_scan(&a, &b, 50, 0.6).unwrap();

        // Brute-force z over the same lag range.
        let mut best: Option<(i64, f64)> = None;
        for lag in (50 - na as i64)..=(nb as i64 - 50) {
            let cross = cross_brute(&a, &b, lag);
            let i0 = (-lag).max(0) as usize;
            let i1 = ((na as i64).min(nb as i64 - lag).max(0)) as usize;
            let n = (i1.max(i0) - i0) as f64;
            if n < 50.0 {
                continue;
            }
            let sa: f64 = a[i0..i1].iter().sum();
            let saa: f64 = a[i0..i1].iter().map(|x| x * x).sum();
            let bs = &b[(i0 as i64 + lag) as usize..(i1 as i64 + lag) as usize];
            let sb: f64 = bs.iter().sum();
            let sbb: f64 = bs.iter().map(|x| x * x).sum();
            let var_a = saa - sa * sa / n;
            let var_b = sbb - sb * sb / n;
            if var_a < 1e-9 || var_b < 1e-9 {
                continue;
            }
            let z = (cross - sa * sb / n) / (var_a * var_b).sqrt() * n.sqrt();
            if best.map(|(_, bz)| z > bz).unwrap_or(true) {
                best = Some((lag, z));
            }
        }
        let (brute_lag, brute_z) = best.unwrap();
        assert_eq!(s_fft.best_lag, brute_lag, "winner differs ({na}x{nb})");
        assert!(
            (s_fft.best_z - brute_z).abs() < 1e-6,
            "z differs: {} vs {brute_z}",
            s_fft.best_z
        );
    }
}

// ---------- §9.3 offset rows (ppm = 0 → ≤ 0.5 ms) ----------

#[test]
fn offset_accuracy_basic() {
    let dir = tempfile::tempdir().unwrap();
    let p = SyncParams::default();
    for (i, offset_ms) in [-3123.456, 0.0, 12_345.6789].into_iter().enumerate() {
        let spec = FixtureSpec {
            offset_ms,
            dur_cam_s: 90.0,
            dur_ext_s: 120.0,
            snr_db: Some(15.0),
            gain_db: -10.0,
            seed: 100 + i as u64,
            ..Default::default()
        };
        let sub = dir.path().join(format!("c{i}"));
        let r = run_sync(&sub, &spec, &p);
        assert_offset(&r, offset_ms, 0.5, &format!("case {i}"));
    }
}

/// Gain delta at the spec ceiling (24 dB) + AAC round-trip degradation.
#[test]
fn offset_accuracy_hostile_gain_and_aac() {
    let dir = tempfile::tempdir().unwrap();
    let p = SyncParams::default();
    let spec = FixtureSpec {
        offset_ms: -7777.7,
        dur_cam_s: 90.0,
        dur_ext_s: 100.0,
        snr_db: Some(10.0),
        gain_db: 24.0,
        aac_roundtrip: true,
        seed: 55,
        ..Default::default()
    };
    let r = run_sync(dir.path(), &spec, &p);
    assert_offset(&r, -7777.7, 0.5, "gain+aac");
}

/// §9.3 nominal-rate mix rows: 44.1 ↔ 48 and 96 ↔ 48 behave identically.
#[test]
fn offset_accuracy_rate_mix() {
    let dir = tempfile::tempdir().unwrap();
    let p = SyncParams::default();
    for (i, rate_ext) in [44_100u32, 96_000].into_iter().enumerate() {
        let spec = FixtureSpec {
            offset_ms: 4321.0,
            dur_cam_s: 90.0,
            dur_ext_s: 120.0,
            rate_ext,
            snr_db: Some(15.0),
            seed: 200 + i as u64,
            ..Default::default()
        };
        let sub = dir.path().join(format!("r{rate_ext}"));
        let r = run_sync(&sub, &spec, &p);
        assert_offset(&r, 4321.0, 0.5, &format!("rate {rate_ext}"));
    }
}

/// Camera muxed into mp4 with mpeg4 video + an -itsoffset epoch: the canonical-timeline
/// rule must keep the measured offset truth-accurate (F1 locked at the sync level).
#[test]
fn offset_accuracy_with_video_and_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let p = SyncParams::default();
    let spec = FixtureSpec {
        offset_ms: -2000.0,
        dur_cam_s: 60.0,
        dur_ext_s: 80.0,
        snr_db: Some(15.0),
        with_video: true,
        itsoffset_s: 0.18,
        seed: 61,
        ..Default::default()
    };
    let truth = generate(dir.path(), &spec, &ffmpeg()).unwrap();
    assert_eq!(truth.camera_file, "camera_epoch.mp4");
    let ctx = common::test_ctx(&dir.path().join("cache"));
    let info = ctx.probe(&dir.path().join(&truth.camera_file)).unwrap();
    assert!(info.video.is_some());
    // AAC priming (1024 samples ≈ 21.3 ms) shifts the muxed start_time below the
    // authored 0.18 s — the same ±25 ms ambiguity §1.3 documents for the real pair.
    let epoch = info.audio[0].start_time_s;
    assert!((0.13..=0.20).contains(&epoch), "epoch {epoch}");

    let r = sync_files(
        dir.path(),
        &dir.path().join(&truth.camera_file),
        &dir.path().join(&truth.external_file),
        &p,
    );
    // The epoch shifts camera content +180 ms on the canonical timeline; the same
    // event is heard 180 ms later in video time, so u(t) needs offset − 180 ms.
    // Tolerance covers the AAC priming/edit-list ambiguity (§1.3: ±25 ms class).
    let expected = -2000.0 - 0.18 * 1000.0;
    assert_offset(&r, expected, 25.0, "video+epoch");
}

// ---------- §9.3 partial overlap + hostile noise ----------

#[test]
fn partial_overlap_rows() {
    let dir = tempfile::tempdir().unwrap();
    let p = SyncParams::default();

    // 25 s overlap (20 ≤ ov < 60): offset within 2 ms.
    let delta = 1.0;
    let spec = FixtureSpec {
        offset_ms: -(120.0 - 25.0) * delta * 1000.0,
        dur_cam_s: 120.0,
        dur_ext_s: 90.0,
        snr_db: Some(10.0),
        seed: 71,
        ..Default::default()
    };
    let r = run_sync(&dir.path().join("ov25"), &spec, &p);
    assert_offset(&r, spec.offset_ms, 2.0, "overlap 25 s");

    // 12 s overlap: MUST fail, never a confident result.
    let spec = FixtureSpec {
        offset_ms: -(120.0 - 12.0) * delta * 1000.0,
        dur_cam_s: 120.0,
        dur_ext_s: 90.0,
        snr_db: Some(10.0),
        seed: 72,
        ..Default::default()
    };
    let r = run_sync(&dir.path().join("ov12"), &spec, &p);
    match r.status {
        SyncStatus::Failed(_) => {}
        _ => {
            // If a peak was found anyway it must not be a confident wrong answer.
            panic!("12 s overlap must fail, got {:?} at {}", r.status, r.offset_ms);
        }
    }

    // Duration-impossible: external shorter than the 20 s minimum.
    let spec = FixtureSpec {
        offset_ms: 0.0,
        dur_cam_s: 60.0,
        dur_ext_s: 15.0,
        snr_db: Some(10.0),
        seed: 73,
        ..Default::default()
    };
    let r = run_sync(&dir.path().join("short"), &spec, &p);
    assert!(
        matches!(
            r.status,
            SyncStatus::Failed(FailReason::InsufficientOverlap { .. })
        ),
        "expected InsufficientOverlap, got {:?}",
        r.status
    );
}

/// §9.3 hostile noise (SNR 0): correct, LowConfidence, or Failed — never
/// confident-and-wrong (the z-gate test).
#[test]
fn hostile_noise_never_confident_and_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let p = SyncParams::default();
    for seed in [81u64, 82, 83] {
        let spec = FixtureSpec {
            offset_ms: 5000.0,
            dur_cam_s: 90.0,
            dur_ext_s: 120.0,
            snr_db: Some(0.0),
            seed,
            ..Default::default()
        };
        let sub = dir.path().join(format!("n{seed}"));
        let r = run_sync(&sub, &spec, &p);
        if matches!(r.status, SyncStatus::Synced) {
            let err = (r.offset_ms - 5000.0).abs();
            assert!(err <= 2.0, "seed {seed}: confident and wrong by {err} ms");
        }
    }
}

// ---------- spurious-peak regression (§9.3, UX §4.2) ----------

/// External gets a verbatim copy of the camera's head appended at its end. At the
/// boundary lag the copy correlates ~perfectly over a small overlap, so raw-NCC argmax
/// picks the decoy; z = ncc·√n must pick the true alignment.
#[test]
fn spurious_peak_z_selection_wins() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 0.0,
        dur_cam_s: 150.0,
        dur_ext_s: 160.0,
        snr_db: Some(8.0), // noisy enough that true NCC < 1
        seed: 90,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();

    // Append camera[0..25 s] to the external (camera.wav is 16-bit mono 48 kHz).
    let cam_path = dir.path().join("camera.wav");
    let ext_path = dir.path().join("external.wav");
    let decoy_path = dir.path().join("external_decoy.wav");
    let cam: Vec<f32> = {
        let mut r = hound::WavReader::open(&cam_path).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    };
    let ext: Vec<f32> = {
        let mut r = hound::WavReader::open(&ext_path).unwrap();
        r.samples::<i32>().map(|s| s.unwrap() as f32 / 8_388_608.0).collect()
    };
    let mut decoy = ext.clone();
    decoy.extend_from_slice(&cam[..48_000 * 25]);
    common::write_wav_mono_48k(&decoy_path, &decoy);

    let ctx = common::test_ctx(&dir.path().join("cache"));
    let cam_id = common::import(&ctx, &cam_path, TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &decoy_path, TrackRole::External).unwrap();
    let cam_t = ctx.track(cam_id).unwrap();
    let ext_t = ctx.track(ext_id).unwrap();
    let a = couple_core::envelope::envelope_full(&cam_t.pyramid, 9600);
    let b = couple_core::envelope::envelope_full(&ext_t.pyramid, 9600);

    let (raw_lag, raw_ncc) = stage1_raw_ncc_argmax(&a, &b, 100).unwrap();
    let s1 = stage1_scan(&a, &b, 100, 0.6).unwrap();

    // Raw NCC picks the decoy at the boundary (~160 s = 800 bins).
    assert!(
        (raw_lag - 800).abs() <= 2,
        "decoy construction failed: raw argmax at lag {raw_lag} (ncc {raw_ncc:.3})"
    );
    // z-selection picks the true alignment (lag ~0).
    assert!(
        s1.best_lag.abs() <= 1,
        "z-selection failed: picked lag {} (z {:.1})",
        s1.best_lag,
        s1.best_z
    );
    assert!(s1.best_z > 8.5, "true peak z {:.2}", s1.best_z);
}
