//! Phase 3 gates: §9.3 drift / local-lag / corrected-residual rows, measure_local
//! budget (§9.6), determinism (§9.7). The 1800 s drift row lives in slow_budgets.

mod common;

use couple_core::fixture::{generate, FixtureSpec};
use couple_core::job::{noop_progress, CancelToken, ProgressEmitter};
use couple_core::sync::*;
use couple_core::TrackRole;
use std::path::Path;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg required")
}

struct Pair {
    cam: std::sync::Arc<couple_core::track::TrackData>,
    ext: std::sync::Arc<couple_core::track::TrackData>,
}

fn build_pair(dir: &Path, spec: &FixtureSpec) -> Pair {
    let truth = generate(dir, spec, &ffmpeg()).expect("fixture gen");
    let ctx = common::test_ctx(&dir.join("cache"));
    let cam_id = common::import(&ctx, &dir.join(&truth.camera_file), TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &dir.join(&truth.external_file), TrackRole::External).unwrap();
    Pair {
        cam: ctx.track(cam_id).unwrap(),
        ext: ctx.track(ext_id).unwrap(),
    }
}

fn full_sync(p: &Pair, params: &SyncParams) -> SyncResult {
    let mut prog = ProgressEmitter::new(0, noop_progress());
    auto_sync_tracks(&p.cam.pyramid, &p.ext.pyramid, params, &mut prog, &CancelToken::new())
        .unwrap()
}

/// One §9.3 drift-ladder row: build a pair with the given overlap, assert ppm tolerance,
/// midpoint mapping error ≤ 1 ms, local-lag RMS ≤ 0.3 ms (ncc ≥ 0.6 windows), and the
/// ≤ 2 ms corrected residual at every accepted window.
fn drift_row(dir: &Path, overlap_s: f64, ppm_true: f64, ppm_tol: f64, seed: u64) {
    let spec = FixtureSpec {
        // External spans the whole camera plus slack; overlap == dur_cam.
        offset_ms: -4000.0,
        ppm: ppm_true,
        dur_cam_s: overlap_s,
        dur_ext_s: overlap_s + 10.0,
        snr_db: Some(15.0),
        gain_db: -6.0,
        seed,
        ..Default::default()
    };
    let pair = build_pair(dir, &spec);
    let r = full_sync(&pair, &SyncParams::default());

    assert!(
        matches!(r.status, SyncStatus::Synced | SyncStatus::LowConfidence),
        "overlap {overlap_s}: {:?}",
        r.status
    );
    assert!(r.drift_measurable, "overlap {overlap_s}: drift not measurable");
    let ppm_err = r.drift_ppm - ppm_true;
    assert!(
        ppm_err.abs() <= ppm_tol,
        "overlap {overlap_s}: ppm {} vs {ppm_true} (err {ppm_err:.3} > {ppm_tol})",
        r.drift_ppm
    );

    // Mapping error at the overlap midpoint ≤ 1.0 ms.
    let t_mid = (r.overlap.t0 + r.overlap.t1) / 2.0;
    let l_meas = r.offset_ms + t_mid * r.drift_ppm * 1e-3;
    let l_true = spec.offset_ms + t_mid * ppm_true * 1e-3;
    assert!(
        (l_meas - l_true).abs() <= 1.0,
        "overlap {overlap_s}: midpoint mapping error {:.3} ms",
        l_meas - l_true
    );

    // Local lag per accepted window (ncc ≥ 0.6): RMS vs the TRUE mapping ≤ 0.3 ms.
    let good: Vec<&DriftWindow> = r.windows.iter().filter(|w| w.accepted && w.ncc >= 0.6).collect();
    if !good.is_empty() {
        let ss: f64 = good
            .iter()
            .map(|w| {
                let resid = w.lag_ms - (spec.offset_ms + w.t * ppm_true * 1e-3);
                resid * resid
            })
            .sum();
        let rms = (ss / good.len() as f64).sqrt();
        assert!(rms <= 0.3, "overlap {overlap_s}: local-lag RMS {rms:.4} ms");
    }

    // Corrected end-to-end residual: ≤ 2 ms at every accepted window vs the fit.
    for w in r.windows.iter().filter(|w| w.accepted) {
        let resid = w.lag_ms - (r.offset_ms + w.t * r.drift_ppm * 1e-3);
        assert!(
            resid.abs() <= 2.0,
            "overlap {overlap_s}: window at {:.1}s residual {resid:.3} ms",
            w.t
        );
    }
}

#[test]
fn drift_ladder_600s() {
    let dir = tempfile::tempdir().unwrap();
    drift_row(dir.path(), 600.0, 30.0, 0.5, 301);
}

#[test]
fn drift_ladder_180s() {
    let dir = tempfile::tempdir().unwrap();
    drift_row(dir.path(), 180.0, 60.0, 2.0, 302);
}

#[test]
fn drift_ladder_60s() {
    let dir = tempfile::tempdir().unwrap();
    drift_row(dir.path(), 60.0, 12.0, 5.0, 303);
}

/// ppm at the spec ceiling (100) — mapping error at midpoint ≤ 1 ms.
#[test]
fn drift_100ppm() {
    let dir = tempfile::tempdir().unwrap();
    drift_row(dir.path(), 300.0, 100.0, 2.0, 304);
}

/// Overlap < 60 s: not-measurable is fine; a confidently wrong value is not.
#[test]
fn short_overlap_drift_honesty() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -1000.0,
        ppm: 40.0,
        dur_cam_s: 45.0,
        dur_ext_s: 55.0,
        snr_db: Some(12.0),
        seed: 310,
        ..Default::default()
    };
    let pair = build_pair(dir.path(), &spec);
    let r = full_sync(&pair, &SyncParams::default());
    if r.drift_measurable && matches!(r.status, SyncStatus::Synced) {
        assert!(
            (r.drift_ppm - 40.0).abs() <= 5.0,
            "confident wrong drift: {} ppm",
            r.drift_ppm
        );
    }
}

/// measure_local accuracy directly (not via the drift pass): RMS ≤ 0.3 ms across
/// spread points, and the ≤ 5 ms §9.6 budget.
#[test]
fn measure_local_accuracy_and_budget() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 2345.6,
        ppm: 0.0,
        dur_cam_s: 120.0,
        dur_ext_s: 130.0,
        snr_db: Some(15.0),
        seed: 320,
        ..Default::default()
    };
    let pair = build_pair(dir.path(), &spec);

    let mut residuals = Vec::new();
    let mut times_us = Vec::new();
    for k in 0..10 {
        let tc = 15.0 + k as f64 * 10.0;
        let t = std::time::Instant::now();
        let m = couple_core::measure::measure_local(
            &pair.cam.pyramid,
            &pair.ext.pyramid,
            tc,
            9.0,
            2345.6,
            80.0,
        );
        times_us.push(t.elapsed().as_secs_f64() * 1e6);
        if let Some(m) = m {
            if m.ncc >= 0.6 {
                residuals.push(m.lag_ms - 2345.6);
            }
        }
    }
    assert!(residuals.len() >= 6, "too few valid measurements: {}", residuals.len());
    let rms = (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt();
    assert!(rms <= 0.3, "measure_local RMS {rms:.4} ms");

    times_us.sort_by(f64::total_cmp);
    let median_ms = times_us[times_us.len() / 2] / 1000.0;
    assert!(median_ms <= 5.0, "measure_local median {median_ms:.2} ms (budget 5 ms)");
}

/// §9.7 determinism: same inputs → bit-identical SyncResult on the same machine.
#[test]
fn sync_result_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -777.0,
        ppm: 15.0,
        dur_cam_s: 120.0,
        dur_ext_s: 140.0,
        snr_db: Some(12.0),
        seed: 330,
        ..Default::default()
    };
    let pair = build_pair(dir.path(), &spec);
    let mut a = full_sync(&pair, &SyncParams::default());
    let mut b = full_sync(&pair, &SyncParams::default());
    // compute_ms is wall-clock, not part of the determinism contract
    a.compute_ms = 0.0;
    b.compute_ms = 0.0;
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

/// Session integration (§4.4): success writes state with suggestion semantics.
#[test]
fn auto_sync_writes_session_state() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -2500.0,
        ppm: 25.0,
        dur_cam_s: 300.0,
        dur_ext_s: 320.0,
        snr_db: Some(15.0),
        seed: 340,
        ..Default::default()
    };
    let truth = generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let ctx = common::test_ctx(&dir.path().join("cache"));
    let cam_id = common::import(&ctx, &dir.path().join(&truth.camera_file), TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &dir.path().join(&truth.external_file), TrackRole::External).unwrap();
    let sid = ctx.session_new(cam_id, ext_id).unwrap();
    let r = ctx
        .auto_sync_blocking(sid, &SyncParams::default(), noop_progress(), &CancelToken::new(), 0)
        .unwrap();
    assert!(matches!(r.status, SyncStatus::Synced));

    let s = ctx.session(sid).unwrap();
    let state = s.snapshot();
    assert_eq!(state.offset_ms, r.offset_ms);
    assert!(!state.verified);
    // 25 ppm over 300 s ≈ 7.5 ms end-to-end → resample suggested and live.
    assert_eq!(state.drift_mode, couple_core::session::DriftMode::Resample);
    assert!((state.drift_ppm - r.drift_ppm).abs() < 1e-12);
    assert!(!state.drift_windows.is_empty());

    // measure_local through the session surface agrees with the mapping.
    let m = ctx
        .measure_local(sid, 150.0, 9.0, s.local_lag_ms(150.0), 80.0)
        .unwrap()
        .expect("measurement");
    let true_lag = spec.offset_ms + 150.0 * spec.ppm * 1e-3;
    assert!((m.lag_ms - true_lag).abs() <= 1.0, "session lag {} vs {true_lag}", m.lag_ms);
}

/// UX §1.5 drift-mode semantics in core (None zeroes live mapping, edit flips back).
#[test]
fn drift_mode_none_semantics() {
    use couple_core::session::{DriftMode, Session};
    let s = Session::new(1, 10, 11);
    s.set_offset_ms(-100.0);
    s.set_drift_ppm_live(8.0);
    s.set_drift_mode(DriftMode::None);
    assert_eq!(s.drift_ppm(), 0.0);
    let snap = s.snapshot();
    assert_eq!(snap.saved_drift_ppm, 8.0);
    assert_eq!(snap.drift_mode, DriftMode::None);
    // restore
    s.set_drift_mode(DriftMode::Resample);
    assert_eq!(s.drift_ppm(), 8.0);
    // any drift write while none flips to resample
    s.set_drift_mode(DriftMode::None);
    s.set_drift_ppm_live(3.0);
    assert_eq!(s.snapshot().drift_mode, DriftMode::Resample);
    assert_eq!(s.drift_ppm(), 3.0);
}
