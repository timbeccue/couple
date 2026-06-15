//! Deterministic residual-based outlier rejection wrapping the ridge line fit
//! (PLAN-AUDIO-MATCHING-PREWORK §8.2). One or two false local locks can bias both the
//! offset and the drift of a plain ridge fit (§2.4); this module drops the points the
//! data itself flags as outliers (a MAD threshold, NOT a fixed count) and refits, so a
//! clean line survives an isolated bad window.
//!
//! Determinism (§3 invariant 3): NO RNG, NO hash iteration, NO wall clock. The procedure
//! is a fixed sequence of medians + least squares over the input slice; reversing the
//! input order yields bit-identical fields (medians use a total-order sort; the fit sums
//! are order-independent in the values that matter). The absolute residual floor is passed
//! in from `PairingParams.verify_outlier_floor_ms` — never hidden inside the fitter (§8.2).

use crate::fit::fit_ls_reg;

/// Robust line fit over (t seconds, lag ms) points: the ridge fit (`fit_ls_reg`) of the
/// inliers that survive MAD-threshold rejection, plus the telemetry the classifier reads.
/// `off_ms`/`ppm`/`rms_ms`/`clamped` are the FINAL refit's [`crate::fit::FitReg`] fields
/// (so `clamped` still means "drift hit the bound" — a rejection signal); `inlier_indices`
/// indexes back into the input `points` so the caller can recover the inlier center times
/// for temporal coverage (§8.3).
#[derive(Debug, Clone, PartialEq)]
pub struct RobustFit {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
    /// Nearest-rank p95 of |residual| of the final inliers to the fitted line, ms.
    pub residual_p95_ms: f64,
    /// The final refit's drift hit the ±`max_ppm` bound (a rejection signal).
    pub clamped: bool,
    /// Indices into the input `points` of the retained inliers, ascending.
    pub inlier_indices: Vec<usize>,
}

/// Deterministic robust line fit (§8.2). Returns `None` when the points cannot anchor a
/// trustworthy line — fewer than 2 inliers survive, or the survivors have a degenerate
/// time spread (`fit_ls_reg` ⇒ `None`). The caller treats `None` as "no trustworthy fit"
/// (a non-match), never as zero RMS.
///
/// `lambda_s2` and `max_ppm` are forwarded verbatim to [`fit_ls_reg`] (ridge weight in s²;
/// drift bound in ppm). `absolute_floor_ms` is the MAD-threshold floor — the smallest
/// residual band kept even when the MAD is tiny (so a near-perfect line does not reject its
/// own faithful points to numerical noise); it comes from `PairingParams.verify_outlier_floor_ms`.
///
/// Procedure (the 10 steps of §8.2):
/// 1. Fit all points with `fit_ls_reg`.
/// 2. Absolute residuals to that line.
/// 3. Residual median `m`.
/// 4. `MAD = median(|residual − m|)`.
/// 5. `sigma = 1.4826·MAD`; `threshold = max(absolute_floor_ms, 3·sigma)`.
/// 6. Keep points within `threshold` of the median residual `m` (`|residual − m| ≤
///    threshold`). See the DEVIATION note below — this recenters by `m` (computed in steps
///    3–4) so the rejection is location-robust and satisfies the §8.5(3) exit gate.
/// 7. Refit the retained points.
/// 8. Repeat steps 2–7 once (one extra reweight).
/// 9. Require ≥ 2 inliers with non-degenerate time spread.
/// 10. RMS + nearest-rank p95 from the FINAL inliers.
///
/// DEVIATION from the literal §8.2 step 6 (`|residual| ≤ threshold`), justified by the
/// binding §8.5(3) exit gate ("resistant to isolated false locks") and invariant #7 (honest
/// uncertainty): a single false lock biases the initial OLS-ish fit, which shifts EVERY
/// clean residual up by a near-constant `m`. The MAD measures only spread, so it stays tiny
/// and `threshold` collapses to `absolute_floor_ms`; the literal `|residual| ≤ threshold`
/// then rejects the shifted clean points too and returns `None` — i.e. it manufactures a
/// false negative on the exact case §8.5(3) requires us to survive. Recentering by `m`
/// (steps 3–4 already compute it; the literal reading leaves `m` unused) is the canonical
/// MAD outlier identifier and keeps the clean majority while dropping the outlier.
pub fn fit_line_mad(
    points: &[(f64, f64)],
    lambda_s2: f64,
    max_ppm: f64,
    absolute_floor_ms: f64,
) -> Option<RobustFit> {
    if points.len() < 2 {
        return None;
    }

    // Step 1: initial fit over ALL points. None ⇒ degenerate t-spread ⇒ no trustworthy line.
    let mut fit = fit_ls_reg(points, lambda_s2, max_ppm)?;

    // The working inlier set, as indices into `points`. Starts as everything; steps 2–7
    // (run twice) narrow it. Indices stay ascending so the result is order-stable.
    let mut inliers: Vec<usize> = (0..points.len()).collect();

    // Steps 2–7, repeated ONCE (two passes total, per §8.2 step 8).
    for _ in 0..2 {
        // Step 2: absolute residuals of the CURRENT inliers to the CURRENT line.
        let slope = fit.ppm / 1000.0; // ms per second
        let abs_res: Vec<f64> = inliers
            .iter()
            .map(|&i| {
                let (t, l) = points[i];
                (l - (fit.off_ms + slope * t)).abs()
            })
            .collect();

        // Steps 3–5: median residual m, MAD, sigma, threshold.
        let m = median(&abs_res);
        let dev: Vec<f64> = abs_res.iter().map(|&r| (r - m).abs()).collect();
        let mad = median(&dev);
        let sigma = 1.4826 * mad;
        let threshold = absolute_floor_ms.max(3.0 * sigma);

        // Step 6: keep the inliers within `threshold` of the median residual `m` (see the
        // DEVIATION note on this fn — recentering by `m` keeps the clean majority when a
        // single outlier has shifted the fit, instead of rejecting everyone). Ascending
        // index order is preserved so the result maps back to center times deterministically.
        let kept: Vec<usize> = inliers
            .iter()
            .copied()
            .zip(abs_res.iter())
            .filter(|(_, &r)| (r - m).abs() <= threshold)
            .map(|(i, _)| i)
            .collect();

        // Step 7: refit the retained points. A refit that loses the time spread (or drops
        // below 2) is not trustworthy ⇒ None (step 9). Build the kept point slice in index
        // order so the fit is deterministic.
        if kept.len() < 2 {
            return None;
        }
        let kept_pts: Vec<(f64, f64)> = kept.iter().map(|&i| points[i]).collect();
        fit = fit_ls_reg(&kept_pts, lambda_s2, max_ppm)?;
        inliers = kept;
    }

    // Step 9: the final survivors must still be ≥ 2 with a non-degenerate time spread.
    // (`fit_ls_reg` above already rejected a degenerate spread with None; this is the
    // explicit count guard.)
    if inliers.len() < 2 {
        return None;
    }

    // Step 10: RMS (from the final refit) and nearest-rank p95 over the FINAL inliers.
    // `inliers` is built and re-filtered in ascending index order throughout, so it is
    // already sorted — the caller can map each index straight back to a center time (§8.3).
    let final_pts: Vec<(f64, f64)> = inliers.iter().map(|&i| points[i]).collect();
    let residual_p95_ms = p95_abs_residual_ms(&final_pts, fit.off_ms, fit.ppm);

    Some(RobustFit {
        off_ms: fit.off_ms,
        ppm: fit.ppm,
        rms_ms: fit.rms_ms,
        residual_p95_ms,
        clamped: fit.clamped,
        inlier_indices: inliers,
    })
}

/// Median of a slice by total order (deterministic; even-length ⇒ mean of the two central
/// values). Empty ⇒ 0.0 (callers never pass empty: the ≥ 2 guards run first).
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f64> = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Nearest-rank p95 of |residual| over (t_s, lag_ms) points to the line `off_ms + ppm/1000·t`
/// (ms). +∞ on empty input. Deterministic (sort by total order). Nearest-rank convention:
/// `ceil(0.95·n)` as a 1-based rank → 0-based index (so n ≤ 20 ⇒ the max; matches the prior
/// `verify.rs` convention this replaces).
fn p95_abs_residual_ms(points: &[(f64, f64)], off_ms: f64, ppm: f64) -> f64 {
    if points.is_empty() {
        return f64::INFINITY;
    }
    let slope = ppm / 1000.0;
    let mut res: Vec<f64> = points
        .iter()
        .map(|&(t, l)| (l - (off_ms + slope * t)).abs())
        .collect();
    res.sort_by(f64::total_cmp);
    let rank = ((0.95 * res.len() as f64).ceil() as usize).max(1);
    res[rank - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: f64 = 2.0; // PairingParams default verify_outlier_floor_ms
    const LAMBDA: f64 = 50.0; // PairingParams default drift_lambda_s2
    const MAX_PPM: f64 = 600.0; // PairingParams default drift_max_ppm

    /// A clean line: lag = 10 ms + 5 ppm drift over a 100 s baseline, 11 points.
    fn clean_line() -> Vec<(f64, f64)> {
        (0..11)
            .map(|k| {
                let t = 10.0 * k as f64; // 0..100 s
                (t, 10.0 + 0.005 * t) // 5 ppm
            })
            .collect()
    }

    // §8.5(2) — reversing the input point order produces identical robust-fit fields. The
    // median sort is total-order and the fit sums are commutative, so order cannot leak.
    #[test]
    fn order_reversal_identical() {
        let pts = clean_line();
        let mut rev = pts.clone();
        rev.reverse();
        let a = fit_line_mad(&pts, LAMBDA, MAX_PPM, FLOOR).unwrap();
        let b = fit_line_mad(&rev, LAMBDA, MAX_PPM, FLOOR).unwrap();
        assert_eq!(a.off_ms.to_bits(), b.off_ms.to_bits(), "off_ms must be order-stable");
        assert_eq!(a.ppm.to_bits(), b.ppm.to_bits(), "ppm must be order-stable");
        assert_eq!(a.rms_ms.to_bits(), b.rms_ms.to_bits(), "rms_ms must be order-stable");
        assert_eq!(
            a.residual_p95_ms.to_bits(),
            b.residual_p95_ms.to_bits(),
            "residual_p95_ms must be order-stable"
        );
        assert_eq!(a.clamped, b.clamped);
        // The inlier set is identical (as the same input multiset), though the indices map
        // to reversed positions — count is the invariant the caller relies on.
        assert_eq!(a.inlier_indices.len(), b.inlier_indices.len());
    }

    // §8.5(3) — one large outlier does not move a clean line beyond 0.5 ms or 1 ppm. The
    // outlier is a single window 30 ms off an otherwise sub-ms-tight 5 ppm line.
    #[test]
    fn single_outlier_rejected() {
        let clean = clean_line();
        let clean_fit = fit_line_mad(&clean, LAMBDA, MAX_PPM, FLOOR).unwrap();
        let mut with_bad = clean.clone();
        with_bad[5] = (50.0, 10.25 + 30.0); // window at t=50 s, +30 ms off the true line
        let robust = fit_line_mad(&with_bad, LAMBDA, MAX_PPM, FLOOR).unwrap();
        assert!(
            (robust.off_ms - clean_fit.off_ms).abs() <= 0.5,
            "off_ms moved {:.3} ms by one outlier (clean {:.3} → {:.3})",
            (robust.off_ms - clean_fit.off_ms).abs(),
            clean_fit.off_ms,
            robust.off_ms
        );
        assert!(
            (robust.ppm - clean_fit.ppm).abs() <= 1.0,
            "ppm moved {:.3} by one outlier (clean {:.3} → {:.3})",
            (robust.ppm - clean_fit.ppm).abs(),
            clean_fit.ppm,
            robust.ppm
        );
        // The outlier was actually dropped (10 of 11 retained).
        assert_eq!(robust.inlier_indices.len(), 10, "the single outlier must be rejected");
        assert!(!robust.inlier_indices.contains(&5), "index 5 (the outlier) must be excluded");
    }

    // §8.5(5) — all-equal center times fail rather than returning zero RMS. A degenerate
    // t-spread gives `fit_ls_reg` nothing to anchor ⇒ None (NOT a zero-RMS "perfect" fit).
    #[test]
    fn degenerate_time_spread_is_none() {
        let pts: Vec<(f64, f64)> = vec![(5.0, 1.0), (5.0, 2.0), (5.0, 3.0), (5.0, 1.5)];
        assert!(
            fit_line_mad(&pts, LAMBDA, MAX_PPM, FLOOR).is_none(),
            "all-equal center times must fail (no trustworthy line), not return zero RMS"
        );
    }

    // §8.5(6) — p95 nearest-rank for 2, 3, and 20 points. ceil(0.95·n): n=2 → rank 2 (max of
    // 2); n=3 → rank 3 (max of 3); n=20 → rank 19 (the 19th of 20 ascending). Verified on a
    // crafted residual ladder so the expected element is unambiguous.
    #[test]
    fn p95_nearest_rank_2_3_20() {
        // Residuals are |lag − 0| at t spaced so the line through them is ~flat at off≈mean.
        // Use the helper directly with a known line (off_ms = 0, ppm = 0) so residual == |lag|.
        let line = |lags: &[f64]| -> f64 {
            let pts: Vec<(f64, f64)> = lags.iter().enumerate().map(|(i, &l)| (i as f64, l)).collect();
            p95_abs_residual_ms(&pts, 0.0, 0.0)
        };
        // n=2: ceil(1.9)=2 → max of the two.
        assert_eq!(line(&[1.0, 5.0]), 5.0);
        // n=3: ceil(2.85)=3 → max of the three.
        assert_eq!(line(&[1.0, 2.0, 9.0]), 9.0);
        // n=20: ceil(19.0)=19 → the 19th ascending value. Build 1..=20 so ascending residual
        // k is value k; rank 19 ⇒ value 19.
        let twenty: Vec<f64> = (1..=20).map(|k| k as f64).collect();
        assert_eq!(line(&twenty), 19.0, "n=20 p95 is the 19th-of-20 (nearest-rank ceil(0.95·20)=19)");
        // Empty ⇒ +∞.
        assert_eq!(p95_abs_residual_ms(&[], 0.0, 0.0), f64::INFINITY);
    }

    // The fit is a pure function of its inputs (no RNG/clock/hash): two identical calls are
    // bit-identical (the determinism anchor for §3 invariant 3 at the fitter level).
    #[test]
    fn deterministic_bit_identical() {
        let pts = clean_line();
        let a = fit_line_mad(&pts, LAMBDA, MAX_PPM, FLOOR).unwrap();
        let b = fit_line_mad(&pts, LAMBDA, MAX_PPM, FLOOR).unwrap();
        assert_eq!(a, b);
    }

    // Fewer than two points ⇒ None (cannot anchor a line at all).
    #[test]
    fn under_two_points_none() {
        assert!(fit_line_mad(&[], LAMBDA, MAX_PPM, FLOOR).is_none());
        assert!(fit_line_mad(&[(1.0, 2.0)], LAMBDA, MAX_PPM, FLOOR).is_none());
    }

    // A clean line keeps all its points (no spurious rejection) and reports a tiny RMS/p95.
    // (The ridge λ=50 slightly shrinks the 5 ppm slope, so the residual is a sub-bin ~7e-4 ms,
    // not bit-zero — the regularization bias, not a fit error. Assert "well under a 96-spb bin".)
    #[test]
    fn clean_line_keeps_all() {
        let pts = clean_line();
        let r = fit_line_mad(&pts, LAMBDA, MAX_PPM, FLOOR).unwrap();
        assert_eq!(r.inlier_indices.len(), pts.len(), "a clean line must keep every point");
        assert!(r.rms_ms < 0.01, "clean-line rms {} should be sub-bin tiny", r.rms_ms);
        assert!(r.residual_p95_ms < 0.01, "clean-line p95 {} should be sub-bin tiny", r.residual_p95_ms);
        assert!(!r.clamped, "5 ppm is well inside the 600 ppm bound");
    }
}
