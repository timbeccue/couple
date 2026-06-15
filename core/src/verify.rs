//! Match classification by fine waveform-lock (PLAN-MATCH-CLASSIFIER §2.3).
//!
//! `z`/NCC PROPOSES (find the lag + a cheap pre-filter); the fine waveform-lock here
//! DISPOSES (the classifier). This module owns the per-pair *match decision* and is kept
//! separate from `sync.rs` (the offset/drift *measurement* pipeline) so it can be imported
//! without dragging the full-sync surface. The editor-sync path is untouched.

use crate::measure;
use crate::pairing_params::PairingParams;
use crate::pyramid::{PyramidReader, SAMPLE_RATE};
use crate::robust_fit::{fit_line_mad, RobustFit};

const SR: f64 = SAMPLE_RATE as f64;
/// Coarse level `measure_local` gates first (96 spb); the measurability probe uses it.
const COARSE_SPB: u32 = 96;

/// Fine waveform-lock verification of a candidate pair at a known coarse lag.
///
/// Pure & deterministic (§5.8): a function of the two pyramids + `coarse_lag_ms` +
/// `params` alone — no clock, no track ids, no hashmap-iteration order. The lag
/// convention matches `pair_correlate` / `SyncResult.offset_ms` (`core/src/pair.rs:18-19`):
/// content at time `t` in `p_a` occurs at `t + coarse_lag_ms/1000` in `p_b`.
///
/// Core computes the match decision (`matched`) here so CLI and Swift never re-implement
/// the predicate (PLAN-AUDIO-MATCHING-PREWORK §6.4). Phase 3 (§8) makes `inliers` the
/// robust-fit survivor count (a MAD-threshold subset of `locked`) and recomputes
/// `off_ms`/`ppm`/`rms_ms`/`residual_p95_ms`/`temporal_coverage` over those survivors, so
/// one or two false local locks cannot bias the offset/drift (§2.4).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyFit {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
    /// Drift hit the slope bound (a rejection signal).
    pub clamped: bool,
    /// Windows that locked (`measure_local` Some AND ncc > `verify_ncc_window_min`).
    pub locked: u32,
    /// MEASURABLE windows = placed windows where BOTH tracks clear the variance floor.
    pub expected: u32,
    /// `locked / expected` (0 when `expected == 0`). Unchanged meaning across Phase 3 (§8.4).
    pub lock_fraction: f64,
    pub overlap_s: f64,
    /// Locked windows retained by the robust fit (the §8.2 MAD-threshold survivors). The
    /// minimum-count condition of `matched` uses THIS, not `locked` (§8.4).
    pub inliers: u32,
    /// Nearest-rank p95 of |residual| of the INLIERS to the fitted line, ms (+∞ when no fit).
    pub residual_p95_ms: f64,
    /// `(max_inlier_t − min_inlier_t) / overlap_s` over the robust inliers — telemetry only
    /// until §8.3 corpus evidence thresholds it (NOT in the match predicate during prework).
    pub temporal_coverage: f64,
    /// Core-computed match decision (§6.4). The single classification authority.
    pub matched: bool,
}

/// The classifier predicate: is this candidate a REAL match? The decision is computed by
/// core inside [`verify_pair`] and stored as [`VerifyFit::matched`]; this helper just
/// surfaces it so CLI and Swift share ONE predicate (PLAN-AUDIO-MATCHING-PREWORK §6.4).
pub fn is_match(fit: &VerifyFit) -> bool {
    fit.matched
}

/// Fine waveform-lock verification. Returns `None` ONLY when the overlap is below the
/// absolute hard floor (`verify_min_overlap_s`) — the caller treats that as "no candidate
/// at all". A too-few-locked or scattered pair returns a `VerifyFit` that fails
/// [`is_match`] (e.g. `locked < 2` ⇒ `rms_ms = +∞`), never `None`.
pub fn verify_pair(
    p_a: &PyramidReader,
    p_b: &PyramidReader,
    coarse_lag_ms: f64,
    params: &PairingParams,
) -> Option<VerifyFit> {
    let dur_a = p_a.duration_s();
    let dur_b = p_b.duration_s();
    let off_s = coarse_lag_ms / 1000.0;
    let t0 = (-off_s).max(0.0);
    let t1 = (dur_b - off_s).min(dur_a);
    let overlap = t1 - t0;
    if overlap < params.min_overlap_s {
        return None; // absolute hard floor — no candidate possible
    }

    // Adaptive half-window: as wide as fits (≤ half the overlap by verify_win_frac),
    // floored, never wider than verify_half_win_max_s. On a 20 s overlap with
    // verify_win_frac=0.5 ⇒ ~5 s half-win (10 s windows) — short enough that
    // measure_local locks (vs the 9 s half-win / 18 s window that can't).
    let hw = (overlap * params.verify_win_frac * 0.5)
        .clamp(params.verify_half_win_min_s, params.verify_half_win_max_s);

    // Adaptive count: density per second, floored, capped (long-clip cost ceiling).
    let n = ((overlap * params.verify_density_per_s).round() as usize)
        .clamp(params.verify_min_windows_floor, params.verify_max_windows);

    // Per-window lag search span: distance-scaled like drift_pass (sync.rs:495) but
    // capped tighter (the slope bound forbids far excursions ⇒ faster, fewer spurious
    // locks).
    let t_mid = (t0 + t1) / 2.0;
    let base = params.verify_base_span_ms;
    let scale = params.verify_span_scale_ppm;
    let cap = params.verify_span_cap_ms;
    let span_at = move |tc: f64| (base + scale * 1e-3 * (tc - t_mid).abs()).min(cap);

    // §8.1: place UNIQUE centers inside ONE global feasible interval (using the max scan
    // margin), instead of evenly spacing then clamping each center independently — which
    // could collapse several centers onto the same edge location and count one measurement
    // as independent evidence (§2.3). `max_span_ms` (the span at the worst-case far edge)
    // sizes the interval so EVERY placed window's scan stays inside both tracks. A center is
    // never clamped: if the interval is too small to hold `n` unique points, fewer come back
    // and the fit reports an unmatched result (it must not manufacture evidence).
    let max_span_ms = span_at(t0).max(span_at(t1));
    let centers = verification_centers(dur_a, dur_b, off_s, t0, t1, hw, max_span_ms, n);

    let bin_s = COARSE_SPB as f64 / SR;
    let n_cam = p_a.n_bins(COARSE_SPB) as i64;
    let n_ext = p_b.n_bins(COARSE_SPB) as i64;

    let mut expected: u32 = 0;
    // (center index, (t_s, measured lag_ms)) so a robust inlier maps back to its center time.
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
        // slope-bounded robust fit (below) separate a tight genuine lock from a coherent
        // impostor trend (which clamps on drift_max_ppm). (A window that edge-rejects is
        // None ⇒ not locked but still counted in `expected` if it was both-tracks measurable.)
        if let Some(m) = measure::measure_local(p_a, p_b, tc, hw, coarse_lag_ms, span) {
            if m.ncc > params.verify_ncc_window_min {
                locked_pts.push((tc, m.lag_ms));
            }
        }
    }

    // §8.4 lock accounting: `expected` = unique measurable centers; `locked` = those clearing
    // the NCC floor; `lock_fraction = locked / expected` (UNCHANGED meaning). `inliers` (below)
    // is the robust-fit survivor subset of `locked`.
    let locked = locked_pts.len() as u32;
    let lock_fraction = if expected == 0 {
        0.0
    } else {
        locked as f64 / expected as f64
    };

    // §8.2 deterministic robust fit over the locked points: drop the windows the residuals
    // themselves flag as outliers (a MAD threshold, floored at verify_outlier_floor_ms), then
    // refit. `None` ⇒ fewer than 2 trustworthy inliers / degenerate t-spread ⇒ untrustworthy
    // lock ⇒ +∞ rms so `matched` is false. We never return `None` from verify_pair here (that
    // is reserved for below-hard-floor overlap, handled above) — an unmatched VerifyFit instead.
    let robust: Option<RobustFit> = fit_line_mad(
        &locked_pts,
        params.drift_lambda_s2,
        params.drift_max_ppm,
        params.verify_outlier_floor_ms,
    );

    let (off_ms, ppm, rms_ms, clamped, inliers, residual_p95_ms, temporal_coverage) = match &robust
    {
        Some(r) => {
            // §8.3 temporal coverage over the ROBUST INLIER centers (telemetry only).
            let inlier_ts: Vec<f64> = r.inlier_indices.iter().map(|&i| locked_pts[i].0).collect();
            (
                r.off_ms,
                r.ppm,
                r.rms_ms,
                r.clamped,
                r.inlier_indices.len() as u32,
                r.residual_p95_ms,
                temporal_coverage_of_times(&inlier_ts, overlap),
            )
        }
        None => (coarse_lag_ms, 0.0, f64::INFINITY, false, 0, f64::INFINITY, 0.0),
    };

    // Core owns the match decision (§6.4): CLI and Swift consume `matched`, never a copy.
    // §8.4: the minimum-count condition uses `inliers` (robust survivors), NOT `locked`.
    let matched = inliers >= params.verify_min_locked
        && lock_fraction >= params.verify_lock_fraction_min
        && rms_ms <= params.verify_rms_max_ms
        && !clamped;

    Some(VerifyFit {
        off_ms,
        ppm,
        rms_ms,
        clamped,
        locked,
        expected,
        lock_fraction,
        overlap_s: overlap,
        inliers,
        residual_p95_ms,
        temporal_coverage,
        matched,
    })
}

/// Place at most `desired` UNIQUE verification-window centers inside the single global
/// feasible interval (PLAN-AUDIO-MATCHING-PREWORK §8.1). The interval is computed ONCE from
/// the maximum scan margin so every placed window's lag scan stays inside both tracks; a
/// center is NEVER clamped individually (which §2.3 showed can collapse several centers onto
/// one edge and double-count evidence). When the interval is too small to hold `desired`
/// unique points, fewer come back — the caller then reports an unmatched fit rather than
/// manufacturing evidence.
///
/// `dur_a`/`dur_b`: track durations (s). `off_s`: A→B lag (s). `overlap_t0`/`overlap_t1`: the
/// overlap interval on A's timeline. `half_win_s`: the verify half-window. `max_span_ms`: the
/// widest per-window lag-scan span (its margin sizes the feasible interval). `desired`: target
/// count. Pure, deterministic, allocation-light.
// The 8-argument signature is mandated verbatim by PLAN §8.1; these are the independent
// geometric inputs of the feasible-interval formula (no natural struct to fold them into).
#[allow(clippy::too_many_arguments)]
fn verification_centers(
    dur_a: f64,
    dur_b: f64,
    off_s: f64,
    overlap_t0: f64,
    overlap_t1: f64,
    half_win_s: f64,
    max_span_ms: f64,
    desired: usize,
) -> Vec<f64> {
    // Step 1: ONE feasible interval from the maximum scan margin (§8.1 formulas verbatim).
    let margin_s = max_span_ms / 1000.0;
    let lo = overlap_t0
        .max(half_win_s)
        .max(half_win_s - off_s + margin_s);
    let hi = (overlap_t1)
        .min(dur_a - half_win_s)
        .min(dur_b - off_s - half_win_s - margin_s);

    // Step 2: infeasible interval ⇒ no centers.
    if hi < lo {
        return Vec::new();
    }
    // Step 3: a single requested point, or a near-degenerate interval ⇒ one midpoint.
    if desired <= 1 || hi - lo < 1e-9 {
        return vec![0.5 * (lo + hi)];
    }

    // Step 4: exactly `desired` linearly spaced points INCLUDING both endpoints lo and hi.
    let last = (desired - 1) as f64;
    let mut centers: Vec<f64> = (0..desired)
        .map(|k| lo + (k as f64 / last) * (hi - lo))
        .collect();

    // Step 5: dedup adjacent values closer than one 96-spb bin (the finest grid a center can
    // resolve), so two near-identical centers never count as independent evidence.
    let bin_s = COARSE_SPB as f64 / SR;
    centers.dedup_by(|next, prev| (*next - *prev).abs() < bin_s);
    centers
}

/// `(max_t − min_t) / max(overlap_s, ε)` over the inlier center times — temporal spread of
/// the lock (§8.3 telemetry). 0 when fewer than 2 inliers. (The p95 of |residual| now lives
/// with the robust fit in `robust_fit.rs`, computed over the same inlier set.)
fn temporal_coverage_of_times(times: &[f64], overlap_s: f64) -> f64 {
    if times.len() < 2 {
        return 0.0;
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &t in times {
        lo = lo.min(t);
        hi = hi.max(t);
    }
    (hi - lo) / overlap_s.max(1e-9)
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

        let params = PairingParams::default();
        let v = verify_pair(&p_cam, &p_ext, lag_ms, &params).expect("overlap clears floor");
        assert!(v.locked >= 4, "locked {} should be >= 4", v.locked);
        assert!(
            v.lock_fraction >= 0.9,
            "clean short pair lock_fraction {} should be high",
            v.lock_fraction
        );
        assert!(v.rms_ms < 5.0, "clean short pair rms {} ms should be small", v.rms_ms);
        assert!(!v.clamped, "clean pair drift should be in bound");
        assert!(is_match(&v), "clean 20 s same-master pair must be a match: {v:?}");
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

        let params = PairingParams::default();
        let v = verify_pair(&p_cam, &p_ext, lag_ms, &params).expect("overlap clears floor");
        assert!(
            !is_match(&v),
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

        let params = PairingParams::default();
        let v = verify_pair(&p_cam, &p_ext, lag_ms, &params).expect("overlap clears floor");
        assert!(
            !is_match(&v),
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

        let a = verify_pair(&p_cam, &p_ext, lag_ms, &PairingParams::default());
        let b = verify_pair(&p_cam, &p_ext, lag_ms, &PairingParams::default());
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
        assert!(verify_pair(&p_cam, &p_ext, 0.0, &PairingParams::default()).is_none());
    }

    /// One 96-spb bin in seconds — the §8.1 dedup granularity for adjacent centers.
    const BIN_S: f64 = COARSE_SPB as f64 / SR;

    // §8.5(1) — no duplicate centers on a short contained clip. A ~12 s external fully inside
    // a longer track, with the default density asking for several windows, must yield STRICTLY
    // increasing (hence unique) centers all inside the feasible interval — never the collapsed
    // edge duplicates of the old per-center clamping (§2.3). Asserted on the §8.1 placement fn
    // directly so the uniqueness property is checked at its source.
    #[test]
    fn no_duplicate_centers_short_contained() {
        // A is 12 s; B is 30 s and contains A's overlap. off = 3 s (A-content at t → t+3 in B).
        let (dur_a, dur_b, off_s) = (12.0_f64, 30.0_f64, 3.0_f64);
        let (t0, t1) = ((-off_s).max(0.0), (dur_b - off_s).min(dur_a)); // [0, 12]
        let p = PairingParams::default();
        let hw = ((t1 - t0) * p.verify_win_frac * 0.5)
            .clamp(p.verify_half_win_min_s, p.verify_half_win_max_s);
        let desired = 8usize;
        let centers = verification_centers(dur_a, dur_b, off_s, t0, t1, hw, p.verify_base_span_ms, desired);

        // Strictly increasing ⇒ no two centers equal AND none within a 96-spb bin of its
        // neighbor (the §8.1 dedup floor): each measurement is independent evidence.
        assert!(centers.len() >= 2, "a 12 s contained clip must place ≥ 2 unique centers: {centers:?}");
        for w in centers.windows(2) {
            assert!(
                w[1] - w[0] >= BIN_S,
                "adjacent centers {:.4} and {:.4} are within one 96-spb bin ⇒ duplicate evidence",
                w[0], w[1]
            );
        }
        // Every center lies inside both tracks' scan-feasible region (never clamped onto an edge).
        for &c in &centers {
            assert!(c >= hw - 1e-9 && c <= dur_a - hw + 1e-9, "center {c:.4} escapes [hw, dur_a-hw]");
        }
    }

    // §8.1 branches: an INFEASIBLE interval (half-window wider than the overlap allows) returns
    // EMPTY — the verifier then reports an unmatched fit rather than manufacturing a center.
    #[test]
    fn centers_empty_when_infeasible() {
        // 9 s overlap but a 9 s half-window (18 s window) cannot fit ⇒ hi < lo ⇒ no centers.
        let centers = verification_centers(9.0, 9.0, 0.0, 0.0, 9.0, 9.0, 120.0, 6);
        assert!(centers.is_empty(), "an 18 s window in a 9 s overlap must place no centers: {centers:?}");
    }

    // §8.1 branches: `desired == 1` and a near-degenerate interval both collapse to ONE
    // midpoint (not two coincident endpoints).
    #[test]
    fn centers_single_and_degenerate_midpoint() {
        // desired == 1 ⇒ one midpoint of the feasible interval.
        let one = verification_centers(30.0, 30.0, 0.0, 0.0, 30.0, 5.0, 120.0, 1);
        assert_eq!(one.len(), 1, "desired==1 ⇒ exactly one center: {one:?}");
        // A feasible interval narrower than 1e-9 ⇒ one midpoint even when several are desired.
        // Construct lo≈hi: overlap [a, a+ε] after margins. Use a tiny overlap window.
        let lo_hi = verification_centers(10.0, 10.0, 0.0, 5.0, 5.0 + 1e-12, 5.0, 0.0, 8);
        assert_eq!(lo_hi.len(), 1, "a near-degenerate interval ⇒ one midpoint, not duplicates: {lo_hi:?}");
    }

    // §8.1 step 4: a comfortably feasible interval emits EXACTLY `desired` centers, INCLUDING
    // both endpoints lo and hi (so the windows span the whole locking region).
    #[test]
    fn centers_include_endpoints() {
        let (dur_a, dur_b, off_s) = (60.0_f64, 60.0_f64, 0.0_f64);
        let (t0, t1) = (0.0_f64, 60.0_f64);
        let hw = 5.0_f64;
        let margin_s = 0.120_f64; // 120 ms base span
        let lo = t0.max(hw).max(hw - off_s + margin_s);
        let hi = t1.min(dur_a - hw).min(dur_b - off_s - hw - margin_s);
        let centers = verification_centers(dur_a, dur_b, off_s, t0, t1, hw, 120.0, 6);
        assert_eq!(centers.len(), 6, "a wide interval emits exactly `desired` centers: {centers:?}");
        assert!((centers[0] - lo).abs() < 1e-9, "first center is lo");
        assert!((centers[centers.len() - 1] - hi).abs() < 1e-9, "last center is hi");
    }
}
