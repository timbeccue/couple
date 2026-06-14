//! Match classification by fine waveform-lock (PLAN-MATCH-CLASSIFIER §2.3).
//!
//! `z`/NCC PROPOSES (find the lag + a cheap pre-filter); the fine waveform-lock here
//! DISPOSES (the classifier). This module owns the per-pair *match decision* and is kept
//! separate from `sync.rs` (the offset/drift *measurement* pipeline) so it can be imported
//! without dragging the full-sync surface. The editor-sync path is untouched.

use crate::measure;
use crate::pyramid::{PyramidReader, SAMPLE_RATE};
use crate::sync::SyncParams;

const SR: f64 = SAMPLE_RATE as f64;
/// Coarse level `measure_local` gates first (96 spb); the measurability probe uses it.
const COARSE_SPB: u32 = 96;

/// Fine waveform-lock verification of a candidate pair at a known coarse lag.
///
/// Pure & deterministic (§5.8): a function of the two pyramids + `coarse_lag_ms` +
/// `params` alone — no clock, no track ids, no hashmap-iteration order. The lag
/// convention matches `pair_correlate` / `SyncResult.offset_ms` (`core/src/pair.rs:18-19`):
/// content at time `t` in `p_a` occurs at `t + coarse_lag_ms/1000` in `p_b`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyFit {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
    /// Drift hit the slope bound (a rejection signal).
    pub clamped: bool,
    /// Windows that locked (`measure_local` Some AND ncc > `ncc_window_min`).
    pub locked: u32,
    /// MEASURABLE windows = placed windows where BOTH tracks clear the variance floor.
    pub expected: u32,
    /// `locked / expected` (0 when `expected == 0`).
    pub lock_fraction: f64,
    pub overlap_s: f64,
}

/// The classifier predicate: is this candidate a REAL match? Pure; all thresholds from
/// `params` (§4.2). Used by BOTH resolvers in Phase 2.
pub fn is_match(fit: &VerifyFit, params: &SyncParams) -> bool {
    fit.locked >= params.verify_min_locked
        && fit.lock_fraction >= params.verify_lock_fraction_min
        && fit.rms_ms <= params.verify_rms_max_ms
        && !fit.clamped // out-of-bound drift ⇒ not a match
}

/// Fine waveform-lock verification. Returns `None` ONLY when the overlap is below the
/// absolute hard floor (`verify_min_overlap_s`) — the caller treats that as "no candidate
/// at all". A too-few-locked or scattered pair returns a `VerifyFit` that fails
/// [`is_match`] (e.g. `locked < 2` ⇒ `rms_ms = +∞`), never `None`.
pub fn verify_pair(
    p_a: &PyramidReader,
    p_b: &PyramidReader,
    coarse_lag_ms: f64,
    params: &SyncParams,
) -> Option<VerifyFit> {
    let dur_a = p_a.duration_s();
    let dur_b = p_b.duration_s();
    let off_s = coarse_lag_ms / 1000.0;
    let t0 = (-off_s).max(0.0);
    let t1 = (dur_b - off_s).min(dur_a);
    let overlap = t1 - t0;
    if overlap < params.verify_min_overlap_s {
        return None; // absolute hard floor — no candidate possible
    }

    // Adaptive half-window: as wide as fits (≤ half the overlap by verify_win_frac),
    // floored, never wider than the editor-sync half_win_s. On a 20 s overlap with
    // verify_win_frac=0.5 ⇒ ~5 s half-win (10 s windows) — short enough that
    // measure_local locks (vs the 9 s half-win / 18 s window that can't).
    let hw = (overlap * params.verify_win_frac * 0.5)
        .clamp(params.verify_half_win_min_s, params.half_win_s);

    // Adaptive count: density per second, floored, capped (long-clip cost ceiling).
    let n = ((overlap * params.verify_density_per_s).round() as usize)
        .clamp(params.verify_min_windows_floor, params.verify_max_windows);

    // Per-window lag search span: distance-scaled like drift_pass (sync.rs:495) but
    // capped tighter (the slope bound forbids far excursions ⇒ faster, fewer spurious
    // locks).
    let t_mid = (t0 + t1) / 2.0;
    let base = params.drift_base_span_ms;
    let scale = params.drift_span_scale_ppm;
    let cap = params.verify_span_cap_ms;
    let span_at = move |tc: f64| (base + scale * 1e-3 * (tc - t_mid).abs()).min(cap);

    // Centers EVENLY spaced and clamped inside both tracks for every scanned lag, exactly
    // like sync.rs:501-513 (so a window never truncates at a file edge).
    let centers: Vec<f64> = (0..n)
        .map(|k| {
            let t = t0 + (k as f64 + 0.5) / n as f64 * (t1 - t0);
            let margin = span_at(t) / 1000.0;
            let lo = hw.max(hw - off_s + margin);
            let hi = (dur_a - hw).min(dur_b - off_s - hw - margin);
            if lo < hi {
                t.clamp(lo, hi)
            } else {
                t
            }
        })
        .collect();

    let bin_s = COARSE_SPB as f64 / SR;
    let n_cam = p_a.n_bins(COARSE_SPB) as i64;
    let n_ext = p_b.n_bins(COARSE_SPB) as i64;

    let mut expected: u32 = 0;
    let mut locked_pts: Vec<(f64, f64)> = Vec::with_capacity(centers.len());

    for &tc in &centers {
        let span = span_at(tc);
        // Camera window bins [c0, c1) at the coarse level (mirrors measure.rs:77-78).
        let c0 = (((tc - hw) / bin_s).floor() as i64).max(0);
        let c1 = (((tc + hw) / bin_s).ceil() as i64).min(n_cam);
        if c1 <= c0 {
            continue;
        }
        // External superwindow [e0, e1) covering all candidate shifts of the camera
        // window at this lag — the same region scan_level builds (measure.rs:84-95).
        let bin_ms = bin_s * 1000.0;
        let l_min = ((coarse_lag_ms - span) / bin_ms).round() as i64;
        let l_max = ((coarse_lag_ms + span) / bin_ms).round() as i64;
        let e0 = (c0 + l_min).max(0);
        let e1 = (c1 + l_max).min(n_ext);
        if e1 <= e0 {
            continue;
        }

        // Measurability must hold for BOTH tracks (PLAN ruling): a window counts in
        // `expected` only when the camera window AND the external span it maps onto are
        // both non-silent — so quiet passages don't unfairly deflate lock_fraction.
        let measurable = measure::window_measurable(p_a, COARSE_SPB, c0 as u64, c1 as u64)
            && measure::window_measurable(p_b, COARSE_SPB, e0 as u64, e1 as u64);
        if measurable {
            expected += 1;
        }

        // locked: measure_local Some AND its fine NCC clears the CLASSIFIER window floor
        // (`verify_ncc_window_min`, separate from the editor-sync `ncc_window_min` per D5).
        // The floor is lower because a genuine degraded-mic short clip measures the
        // CORRECT per-window lag at modest NCC; counting those windows lets the
        // slope-bounded fit (below) separate a tight genuine lock from a coherent impostor
        // trend (which clamps on drift_max_ppm). (A window that edge-rejects is None ⇒ not
        // locked but still counted in `expected` if it was both-tracks measurable.)
        if let Some(m) = measure::measure_local(p_a, p_b, tc, hw, coarse_lag_ms, span) {
            if m.ncc > params.verify_ncc_window_min {
                locked_pts.push((tc, m.lag_ms));
            }
        }
    }

    let locked = locked_pts.len() as u32;
    let lock_fraction = if expected == 0 {
        0.0
    } else {
        locked as f64 / expected as f64
    };

    // < 2 locked points cannot anchor a line: report a VerifyFit that fails is_match
    // (rms = +∞), NOT None (None is reserved for below-hard-floor overlap).
    if locked_pts.len() < 2 {
        return Some(VerifyFit {
            off_ms: coarse_lag_ms,
            ppm: 0.0,
            rms_ms: f64::INFINITY,
            clamped: false,
            locked,
            expected,
            lock_fraction,
            overlap_s: overlap,
        });
    }

    // Slope-regularized fit over the locked points (short clips pulled flat; long clips
    // refine a small slope). fit_ls_reg returns None only on < 2 points / degenerate
    // t-spread; both leave the lock untrustworthy ⇒ +∞ rms so is_match fails it.
    match crate::fit::fit_ls_reg(&locked_pts, params.drift_lambda_s2, params.drift_max_ppm) {
        Some(f) => Some(VerifyFit {
            off_ms: f.off_ms,
            ppm: f.ppm,
            rms_ms: f.rms_ms,
            clamped: f.clamped,
            locked,
            expected,
            lock_fraction,
            overlap_s: overlap,
        }),
        None => Some(VerifyFit {
            off_ms: coarse_lag_ms,
            ppm: 0.0,
            rms_ms: f64::INFINITY,
            clamped: false,
            locked,
            expected,
            lock_fraction,
            overlap_s: overlap,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyramid::{PyramidBuilder, PyramidReader};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    const SRT: f64 = SAMPLE_RATE as f64;
    const PLAN_LO: f64 = 0.0;
    const PLAN_HI: f64 = 600.0;

    /// Burst-plan master keyed to the ABSOLUTE grid index (so slices of the same seed
    /// agree where they overlap). Mirrors the validated generator in sync.rs tests.
    fn master(seed: u64, t0: f64, n: usize) -> Vec<f32> {
        let mut rb = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0xb0_57));
        let mut bursts: Vec<(f64, f64, f64)> = Vec::new();
        let mut u = PLAN_LO;
        loop {
            u += -(rb.gen_range(1e-9..1.0f64)).ln() / 6.0;
            if u >= PLAN_HI {
                break;
            }
            bursts.push((u, rb.gen_range(0.010..0.060), rb.gen_range(0.4..1.0)));
        }
        let carrier = |idx: i64| {
            let mut z = (idx as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
        };
        let base = (t0 * SRT).round() as i64;
        (0..n)
            .map(|k| {
                let idx = base + k as i64;
                let t = idx as f64 / SRT;
                let mut env = 0.02f64;
                for &(o, tau, a) in &bursts {
                    if o <= t && t - o < 8.0 * tau {
                        let dt = t - o;
                        env += a * (1.0 - (-dt / 0.0005).exp()) * (-dt / tau).exp();
                    }
                }
                (carrier(idx) as f64 * env).clamp(-0.999, 0.999) as f32
            })
            .collect()
    }

    fn with_noise(mut samples: Vec<f32>, seed: u64, amp: f32) -> Vec<f32> {
        let mut r = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0x6e_6f_69_73));
        for s in &mut samples {
            *s = (*s + r.gen_range(-1.0f32..1.0) * amp).clamp(-0.999, 0.999);
        }
        samples
    }

    fn pyramid_of(samples: &[f32], dir: &tempfile::TempDir, name: &str) -> PyramidReader {
        let mut b = PyramidBuilder::new();
        b.push(samples);
        let fin = b.finalize();
        let path = dir.path().join(name);
        fin.write(&path).expect("write pyramid");
        PyramidReader::open(&path).expect("open pyramid")
    }

    // (a) A clean ~20 s same-master pair (the short clip the owner reported) ⇒ is_match
    // TRUE, high lock_fraction, low rms, locked ≥ 4.
    #[test]
    fn clean_short_pair_is_match() {
        let dir = tempfile::tempdir().unwrap();
        let dur_cam = 20.0; // the motivating 20 s clip
        let dur_ext = 30.0;
        let lag_ms = 1500.0; // cam content at t shows at t+1.5 s in ext
        let off_s = lag_ms / 1000.0;
        let cam = with_noise(master(7, 0.0, (dur_cam * SRT) as usize), 7, 0.10);
        let ext = master(7, -off_s, (dur_ext * SRT) as usize);
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");

        let params = SyncParams::default();
        let v = verify_pair(&p_cam, &p_ext, lag_ms, &params).expect("overlap clears floor");
        assert!(v.locked >= 4, "locked {} should be >= 4", v.locked);
        assert!(
            v.lock_fraction >= 0.9,
            "clean short pair lock_fraction {} should be high",
            v.lock_fraction
        );
        assert!(v.rms_ms < 5.0, "clean short pair rms {} ms should be small", v.rms_ms);
        assert!(!v.clamped, "clean pair drift should be in bound");
        assert!(is_match(&v, &params), "clean 20 s same-master pair must be a match: {v:?}");
    }

    // (b) A mis-lagged / different-master pair ⇒ is_match FALSE (scatter ⇒ high rms / low
    // lock_fraction, or clamped).
    #[test]
    fn different_master_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let dur = 30.0;
        let lag_ms = 1500.0;
        let off_s = lag_ms / 1000.0;
        let cam = master(7, 0.0, (dur * SRT) as usize);
        let ext = master(999, -off_s, (dur * SRT) as usize); // unrelated master
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");

        let params = SyncParams::default();
        let v = verify_pair(&p_cam, &p_ext, lag_ms, &params).expect("overlap clears floor");
        assert!(
            !is_match(&v, &params),
            "different-master pair must NOT be a match: {v:?}"
        );
    }

    // (c) The gate6 unrelated seed-777-style pair ⇒ is_match FALSE (the byte-stability
    // anchor: the unrelated newfile must deterministically fail the classifier so a
    // resolved row stays byte-identical, PLAN §3).
    #[test]
    fn gate6_unrelated_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let dur = 40.0;
        let lag_ms = 0.0;
        let cam = master(7, 0.0, (dur * SRT) as usize);
        let ext = master(777, 0.0, (dur * SRT) as usize); // gate6 newfile master_seed 777
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");

        let params = SyncParams::default();
        let v = verify_pair(&p_cam, &p_ext, lag_ms, &params).expect("overlap clears floor");
        assert!(
            !is_match(&v, &params),
            "gate6 unrelated (seed 777) must NOT be a match (stability anchor): {v:?}"
        );
    }

    // (d) Determinism: two calls are bit-identical on the same pyramids + lag + params.
    #[test]
    fn deterministic_bit_identical() {
        let dir = tempfile::tempdir().unwrap();
        let lag_ms = 800.0;
        let off_s = lag_ms / 1000.0;
        let cam = master(42, 0.0, (40.0 * SRT) as usize);
        let ext = master(42, -off_s, (50.0 * SRT) as usize);
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");

        let a = verify_pair(&p_cam, &p_ext, lag_ms, &SyncParams::default());
        let b = verify_pair(&p_cam, &p_ext, lag_ms, &SyncParams::default());
        assert_eq!(a, b, "verify_pair must be deterministic");
        let a = a.expect("clean pair clears floor");
        let b = b.unwrap();
        assert_eq!(a.off_ms.to_bits(), b.off_ms.to_bits());
        assert_eq!(a.rms_ms.to_bits(), b.rms_ms.to_bits());
        assert_eq!(a.lock_fraction.to_bits(), b.lock_fraction.to_bits());
    }

    // Below the hard overlap floor ⇒ None (no candidate at all).
    #[test]
    fn below_floor_is_none() {
        let dir = tempfile::tempdir().unwrap();
        // 5 s overlap < verify_min_overlap_s (8 s default).
        let cam = master(7, 0.0, (5.0 * SRT) as usize);
        let ext = master(7, 0.0, (5.0 * SRT) as usize);
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");
        assert!(verify_pair(&p_cam, &p_ext, 0.0, &SyncParams::default()).is_none());
    }
}
