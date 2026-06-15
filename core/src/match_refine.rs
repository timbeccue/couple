//! Fine refinement of an accepted spectral-landmark affine hypothesis using the EXISTING local
//! alignment machinery (PLAN-SPECTRAL-FINGERPRINT-MATCHING §11.1 / §11.2).
//!
//! Boundary 5's [`crate::fingerprint::consensus`] estimates the affine map `(lag_ms, drift_ppm)`
//! from landmark *correspondences* at the 50 ms inlier tolerance — coarse, because a landmark
//! frame is ~21 ms (256 / 12 kHz) and the fit absorbs quantization. §11 refines that map to
//! sub-millisecond accuracy by re-using the proven raw-envelope local aligner, NOT by re-running
//! the global envelope scan (§11.1, §20 "replace the existing final alignment path without
//! evidence"). The pipeline is:
//!
//! 1. `predicted_lag_ms(t) = lag_ms + t · drift_ppm · 1e-3` — the affine lag at an A-time `t`
//!    (§11.1 #1; matches the §3.1 convention `t_b = t·(1+drift_ppm·1e-6) + lag_ms/1000`, since
//!    `predicted_lag_ms(t)/1000 = (t_b − t) = drift_ppm·1e-6·t + lag_ms/1000`).
//! 2. select 6–16 UNIQUE anchors spread across the inlier-covered A-span (§11.1 #2);
//! 3. call the EXISTING [`crate::measure::measure_local`] around each predicted lag with a NARROW
//!    span (§11.1 #3) — the same per-window NCC lag measurement `verify_pair` / the drift pass use;
//! 4. fit those local `(t, measured_lag_ms)` points with the prework deterministic MAD line fitter
//!    [`crate::robust_fit::fit_line_mad`] (§11.1 #4).
//!
//! The refined `(off_ms, ppm, rms_ms)` become `PairEvidence.refined_lag_ms / refined_drift_ppm /
//! refined_rms_ms` (§11.2). If refinement FAILS (too few anchors measurable, or the local
//! measurements do not fit a trustworthy line), the caller retains the landmark *identity* evidence
//! but classifies `Ambiguous` (NOT `NoMatch`) and records the [`RefineReason`] in debug telemetry
//! (§11.2) — landmark-only acceptance is not yet corpus-proven safe, so a refinement failure must
//! never present as a confident `Match`.
//!
//! Determinism (§3 invariant 2): anchors are placed by a closed-form linear spacing over the span,
//! `measure_local` is a pure function of the two pyramids, and `fit_line_mad` is RNG-free — so the
//! refined fields are byte-identical across runs, and `finite_*` scrubbing guarantees no
//! NaN/Infinity reaches `PairEvidence` (§20). This module touches NO editor-sync / audition /
//! session / export code (§3); the §11.3 full-sync seed lives in `sync.rs`.

use crate::pyramid::PyramidReader;
use crate::robust_fit::fit_line_mad;

/// Minimum anchors to attempt across the inlier span (§11.1 #2 lower bound).
const MIN_ANCHORS: usize = 6;
/// Maximum anchors to attempt across the inlier span (§11.1 #2 upper bound).
const MAX_ANCHORS: usize = 16;
/// Half-window (seconds) for each local `measure_local` probe. Matches the verifier's lower
/// `verify_half_win_min_s` regime: wide enough for a stable NCC lock, short enough to localize on
/// a short overlap. Held as a module constant (NOT a format/threshold param) because §11 refinement
/// is a fixed local measurement, not a tunable acceptance gate.
const REFINE_HALF_WIN_S: f64 = 2.5;
/// NARROW per-anchor lag-scan span (ms), §11.1 #3. The affine prediction is already within a few
/// landmark frames of the truth, so the local scan only needs to cover residual prediction error —
/// NOT rediscover the offset (which would be the prohibited global rescan, §20). 60 ms comfortably
/// brackets the ~21 ms landmark-frame quantization plus drift-prediction error across the span,
/// while keeping `measure_local` cheap and free of spurious far locks.
const REFINE_SCAN_SPAN_MS: f64 = 60.0;
/// A local probe is KEPT only when its NCC clears this floor — the same modest per-window floor the
/// verifier uses (`PairingParams::verify_ncc_window_min` default 0.25), so a degraded-mic short clip
/// still contributes its correct per-anchor lag. Held as a constant here (the refinement is a
/// fixed local measurement; the value mirrors the verifier default).
const REFINE_NCC_MIN: f64 = 0.25;
/// MAD-fit absolute residual floor (ms), mirrors `PairingParams::verify_outlier_floor_ms` (2.0) —
/// the smallest residual band kept when the MAD is tiny, so a near-perfect refined line does not
/// reject its own faithful anchors to numerical noise.
const REFINE_OUTLIER_FLOOR_MS: f64 = 2.0;
/// Ridge weight (s²) for the MAD line fit, mirrors `PairingParams::drift_lambda_s2` (50.0).
const REFINE_LAMBDA_S2: f64 = 50.0;
/// Drift bound (ppm) for the MAD line fit, mirrors `PairingParams::drift_max_ppm` (600.0). A refined
/// drift that clamps here is a rejection signal (the refit could not stay inside the physical bound).
const REFINE_MAX_PPM: f64 = 600.0;
/// Minimum anchors whose local probe must lock for a fit to be attempted (§11.1 #2 / robust-fit
/// `fit_line_mad` needs ≥ 2 with spread). Below this the refinement fails ⇒ `Ambiguous`.
const REFINE_MIN_LOCKED: usize = 4;

/// Why a §11 refinement did NOT produce refined fields — debug telemetry only (§11.2). The
/// successful case is [`RefineReason::Refined`]; the failure variants explain an `Ambiguous`
/// downgrade. NOT serialized into the `PairEvidence` wire JSON (it rides as a `#[serde(skip)]`
/// field so the FFI/Swift contract stays byte-identical until boundary 8 chooses how to surface
/// it), so a `Default` is required for round-trip deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefineReason {
    /// Refinement was not run for this cell (not a fingerprint `Match`/strong `Ambiguous`, or the
    /// consensus had no fittable hypothesis to refine). The default for every non-refined evidence.
    #[default]
    NotAttempted,
    /// Refinement succeeded: the `refined_*` fields are populated from the local-measurement fit.
    Refined,
    /// The consensus exposed no inlier A-span to place anchors across (no fittable hypothesis).
    NoInlierSpan,
    /// Fewer than [`REFINE_MIN_LOCKED`] anchors produced a usable local lock (e.g. quiet passages,
    /// or the predicted lag was off by more than the narrow scan span at every anchor).
    TooFewLocalLocks,
    /// The local measurements did not fit a trustworthy line (`fit_line_mad` ⇒ `None`: < 2 inliers
    /// after MAD rejection, or a degenerate anchor-time spread).
    LineFitFailed,
    /// The refined drift hit the ±bound (clamped) — an untrustworthy refined slope, treated as a
    /// refinement failure so it cannot present as a confident `Match`.
    DriftClamped,
}

/// The outcome of a §11 fine refinement: the refined affine mapping when it succeeded, plus the
/// telemetry reason. On failure the mapping fields are `None` and `reason` explains why.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefineOutcome {
    /// Refined lag (ms) — `None` on failure. Finite when `Some` (§20).
    pub refined_lag_ms: Option<f64>,
    /// Refined drift (ppm) — `None` on failure. Finite when `Some`.
    pub refined_drift_ppm: Option<f64>,
    /// Refined RMS of the local-measurement residuals (ms) — `None` on failure. Finite when `Some`.
    pub refined_rms_ms: Option<f64>,
    /// Debug telemetry: success, or why refinement failed (§11.2).
    pub reason: RefineReason,
}

impl RefineOutcome {
    /// A refinement-failure outcome with the given reason (no refined fields). `pub(crate)` so the
    /// `matching` policy tests can synthesize a specific failure reason for the orchestrator.
    pub(crate) fn failed(reason: RefineReason) -> Self {
        Self { refined_lag_ms: None, refined_drift_ppm: None, refined_rms_ms: None, reason }
    }

    /// The "refinement not run" outcome ([`RefineReason::NotAttempted`], no refined fields). For a
    /// caller that cannot refine (e.g. the pure `match_candidates_fingerprint` unit tests, which
    /// have hand-built readers but no pyramids): returning this from the refine closure leaves the
    /// cell exactly as the §9.3 mapping produced it.
    pub fn not_attempted() -> Self {
        Self::failed(RefineReason::NotAttempted)
    }

    /// `true` when the refined fields were populated (a successful §11 refinement).
    pub fn is_refined(&self) -> bool {
        self.reason == RefineReason::Refined
    }
}

/// Fine-refine an accepted affine hypothesis against the two pyramids (§11.1 / §11.2). `p_a` is the
/// A-side (video) pyramid, `p_b` the B-side (audio); `lag_ms` / `drift_ppm` are the consensus map;
/// `inlier_t_a_min` / `inlier_t_a_max` bound the inlier-covered A-span the anchors spread across
/// (from [`crate::fingerprint::AffineConsensus`]). Anchor feasibility is derived from the two
/// durations + this inlier span (already inside the candidate overlap), so no overlap scalar is
/// needed.
///
/// Reuses the EXISTING [`crate::measure::measure_local`] (the narrow-span local NCC lag measurement,
/// `core/src/measure.rs`) — it does NOT call the global envelope scan / `pair_correlate` again
/// (§11.1, §20). Returns a [`RefineOutcome`]; on success `refined_lag_ms` / `refined_drift_ppm` /
/// `refined_rms_ms` are the [`fit_line_mad`] result, scrubbed finite (§20).
pub fn refine_affine(
    p_a: &PyramidReader,
    p_b: &PyramidReader,
    lag_ms: f64,
    drift_ppm: f64,
    inlier_t_a_min: f64,
    inlier_t_a_max: f64,
) -> RefineOutcome {
    // A finite, ordered inlier span is required to place anchors (§11.1 #2). A non-finite or empty
    // span means there was no fittable hypothesis to refine.
    if !inlier_t_a_min.is_finite() || !inlier_t_a_max.is_finite() || inlier_t_a_max <= inlier_t_a_min
    {
        return RefineOutcome::failed(RefineReason::NoInlierSpan);
    }
    if !lag_ms.is_finite() || !drift_ppm.is_finite() {
        return RefineOutcome::failed(RefineReason::NoInlierSpan);
    }

    let dur_a = p_a.duration_s();
    let dur_b = p_b.duration_s();
    let off_s = lag_ms / 1000.0;

    // Restrict the anchor span to the region where a half-window probe fits inside BOTH tracks at
    // the predicted lag (so `measure_local` never truncates a window at a file edge and locks a junk
    // lag — the same edge-safety the verifier/drift-pass enforce). margin covers the narrow scan.
    let margin_s = REFINE_SCAN_SPAN_MS / 1000.0;
    let lo_feasible = REFINE_HALF_WIN_S.max(REFINE_HALF_WIN_S - off_s + margin_s);
    let hi_feasible = (dur_a - REFINE_HALF_WIN_S).min(dur_b - off_s - REFINE_HALF_WIN_S - margin_s);
    let lo = inlier_t_a_min.max(lo_feasible);
    let hi = inlier_t_a_max.min(hi_feasible);
    if hi <= lo {
        // The inlier span does not overlap the feasible probe region (too short a track for the
        // half-window). Cannot place anchors ⇒ refinement fails (the identity evidence is retained
        // by the caller as Ambiguous).
        return RefineOutcome::failed(RefineReason::TooFewLocalLocks);
    }

    // §11.1 #2: choose the anchor count from the available span — denser spans get more anchors, up
    // to MAX_ANCHORS, never fewer than MIN_ANCHORS (clamped to what fits). One anchor per ~3 s of
    // span keeps probes independent without over-sampling a short clip.
    let span = hi - lo;
    let desired = ((span / 3.0).floor() as usize + 1).clamp(MIN_ANCHORS, MAX_ANCHORS);
    let anchors = anchor_times(lo, hi, desired);

    // §11.1 #3: probe each anchor with the EXISTING measure_local around the PREDICTED lag, NARROW
    // span. Keep the anchor's (t, measured_lag_ms) when the probe locks above the NCC floor.
    let mut points: Vec<(f64, f64)> = Vec::with_capacity(anchors.len());
    for &t in &anchors {
        let predicted_lag_ms = lag_ms + t * drift_ppm * 1e-3;
        if let Some(m) =
            crate::measure::measure_local(p_a, p_b, t, REFINE_HALF_WIN_S, predicted_lag_ms, REFINE_SCAN_SPAN_MS)
        {
            if m.ncc > REFINE_NCC_MIN && m.lag_ms.is_finite() {
                points.push((t, m.lag_ms));
            }
        }
    }

    if points.len() < REFINE_MIN_LOCKED {
        return RefineOutcome::failed(RefineReason::TooFewLocalLocks);
    }

    // §11.1 #4: fit the local measurements with the prework deterministic MAD line fitter. None ⇒
    // no trustworthy line (< 2 inliers after rejection / degenerate spread) ⇒ refinement failed.
    let Some(fit) = fit_line_mad(&points, REFINE_LAMBDA_S2, REFINE_MAX_PPM, REFINE_OUTLIER_FLOOR_MS)
    else {
        return RefineOutcome::failed(RefineReason::LineFitFailed);
    };
    // A clamped refined drift is an untrustworthy slope (the local locks wanted a drift beyond the
    // physical bound) — treat as a failure so it cannot become a confident Match.
    if fit.clamped {
        return RefineOutcome::failed(RefineReason::DriftClamped);
    }

    // §11.2 success: populate the refined fields, scrubbed finite (§20). off_ms is the refined lag;
    // ppm is the refined drift; rms_ms is the local-measurement residual RMS.
    let refined_lag_ms = finite(fit.off_ms);
    let refined_drift_ppm = finite(fit.ppm);
    let refined_rms_ms = finite(fit.rms_ms);
    match (refined_lag_ms, refined_drift_ppm, refined_rms_ms) {
        (Some(l), Some(d), Some(r)) => RefineOutcome {
            refined_lag_ms: Some(l),
            refined_drift_ppm: Some(d),
            refined_rms_ms: Some(r),
            reason: RefineReason::Refined,
        },
        // A non-finite slipping through (defensive) ⇒ treat as a fit failure, never serialize NaN.
        _ => RefineOutcome::failed(RefineReason::LineFitFailed),
    }
}

/// `desired` linearly-spaced anchor times across `[lo, hi]` INCLUDING both endpoints, deduplicated so
/// two anchors never collapse onto (almost) the same time (§11.1 #2 "unique anchors"). `desired ≥ 1`,
/// `hi > lo`. Mirrors the verifier's `verification_centers` spacing/dedup discipline.
fn anchor_times(lo: f64, hi: f64, desired: usize) -> Vec<f64> {
    if desired <= 1 || hi - lo < 1e-9 {
        return vec![0.5 * (lo + hi)];
    }
    let last = (desired - 1) as f64;
    let mut out: Vec<f64> = (0..desired).map(|k| lo + (k as f64 / last) * (hi - lo)).collect();
    // Dedup adjacent anchors closer than 1 ms in A-time (far finer than a probe can resolve) so each
    // is independent evidence for the fit.
    out.dedup_by(|next, prev| (*next - *prev).abs() < 1e-3);
    out
}

/// `Some(v)` when `v` is finite, else `None` — the §20 scrub so no NaN/Infinity reaches a refined
/// `PairEvidence` field.
fn finite(v: f64) -> Option<f64> {
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyramid::{PyramidBuilder, PyramidReader, SAMPLE_RATE};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    const SR: f64 = SAMPLE_RATE as f64;
    const PLAN_LO: f64 = -3.0;
    const PLAN_HI: f64 = 95.0;

    /// Burst-plan master keyed to the ABSOLUTE grid index (so slices of the same seed agree where
    /// they overlap) — the validated generator shared by `verify.rs` / `sync.rs` tests.
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
        let base = (t0 * SR).round() as i64;
        (0..n)
            .map(|k| {
                let idx = base + k as i64;
                let t = idx as f64 / SR;
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

    // A clean same-master pair: seed the refiner with a DELIBERATELY coarse offset (the affine
    // consensus is only accurate to the inlier tolerance) and confirm §11 pulls it back onto the
    // true lag within a millisecond, with refined drift ≈ 0 and a tiny RMS.
    #[test]
    fn refines_coarse_offset_to_sub_ms() {
        let dir = tempfile::tempdir().unwrap();
        let dur_cam = 40.0;
        let dur_ext = 50.0;
        let true_lag_ms = 1500.0;
        let off_s = true_lag_ms / 1000.0;
        let cam = with_noise(master(7, 0.0, (dur_cam * SR) as usize), 7, 0.08);
        let ext = master(7, -off_s, (dur_ext * SR) as usize);
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");

        // Seed offset is 18 ms off the truth (within a landmark-frame quantum), drift seeded 0.
        let seed_lag = true_lag_ms + 18.0;
        let out = refine_affine(&p_cam, &p_ext, seed_lag, 0.0, 1.0, 38.0);
        assert!(out.is_refined(), "clean pair must refine: {out:?}");
        let refined = out.refined_lag_ms.unwrap();
        assert!(
            (refined - true_lag_ms).abs() <= 1.0,
            "refined lag {refined} must be within 1 ms of {true_lag_ms} (seed was {seed_lag})"
        );
        assert!(out.refined_drift_ppm.unwrap().abs() < 30.0, "clean pair drift ≈ 0");
        assert!(out.refined_rms_ms.unwrap() < 5.0, "refined rms small");
    }

    // Determinism (§3 invariant 2 / §20): two refinements on the same pyramids + seed are
    // byte-identical, and no NaN/Infinity appears in the refined fields.
    #[test]
    fn deterministic_and_finite() {
        let dir = tempfile::tempdir().unwrap();
        let off_s = 0.8;
        let cam = with_noise(master(42, 0.0, (40.0 * SR) as usize), 42, 0.05);
        let ext = master(42, -off_s, (50.0 * SR) as usize);
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");

        let a = refine_affine(&p_cam, &p_ext, 800.0, 0.0, 1.0, 38.0);
        let b = refine_affine(&p_cam, &p_ext, 800.0, 0.0, 1.0, 38.0);
        assert_eq!(a, b, "refine_affine must be deterministic");
        let l = a.refined_lag_ms.unwrap();
        assert!(l.is_finite(), "no NaN/Inf in refined lag");
        assert!(a.refined_drift_ppm.unwrap().is_finite() && a.refined_rms_ms.unwrap().is_finite());
    }

    // A degenerate/empty inlier span ⇒ NoInlierSpan (no anchors to place), refined fields None.
    #[test]
    fn empty_inlier_span_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cam = master(7, 0.0, (30.0 * SR) as usize);
        let ext = master(7, 0.0, (30.0 * SR) as usize);
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");
        let out = refine_affine(&p_cam, &p_ext, 0.0, 0.0, 5.0, 5.0); // min == max
        assert_eq!(out.reason, RefineReason::NoInlierSpan);
        assert!(out.refined_lag_ms.is_none());
    }

    // A mis-seeded UNRELATED pair: the local probes either fail to lock or scatter, so refinement
    // fails (TooFewLocalLocks or LineFitFailed) — it must NOT fabricate a confident refined line.
    #[test]
    fn unrelated_pair_refinement_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cam = master(7, 0.0, (40.0 * SR) as usize);
        let ext = master(999, -0.8, (50.0 * SR) as usize); // independent master
        let p_cam = pyramid_of(&cam, &dir, "cam.cpk");
        let p_ext = pyramid_of(&ext, &dir, "ext.cpk");
        let out = refine_affine(&p_cam, &p_ext, 800.0, 0.0, 1.0, 38.0);
        assert!(!out.is_refined(), "unrelated pair must not refine: {out:?}");
        assert!(
            matches!(out.reason, RefineReason::TooFewLocalLocks | RefineReason::LineFitFailed | RefineReason::DriftClamped),
            "expected a failure reason, got {:?}",
            out.reason
        );
    }

    // anchor_times: unique, sorted, span the interval including endpoints; single point ⇒ midpoint.
    #[test]
    fn anchor_times_unique_and_spanning() {
        let a = anchor_times(2.0, 20.0, 8);
        assert_eq!(a.len(), 8);
        assert!((a[0] - 2.0).abs() < 1e-9 && (a[7] - 20.0).abs() < 1e-9, "includes endpoints");
        for w in a.windows(2) {
            assert!(w[1] - w[0] >= 1e-3, "anchors are unique/spread: {a:?}");
        }
        let one = anchor_times(5.0, 5.0 + 1e-12, 8);
        assert_eq!(one.len(), 1, "near-degenerate ⇒ one midpoint");
    }
}
