//! Least-squares fit over (t_s, lag_ms) points (§4.4, UX §4.4). Pure function.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
}

/// `points`: (t seconds, lag ms). Slope is ms/s; ppm = slope·1000.
/// None if fewer than 2 points or degenerate spread.
pub fn fit_ls(points: &[(f64, f64)]) -> Option<Fit> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let mean_t = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_l = points.iter().map(|p| p.1).sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for &(t, l) in points {
        sxx += (t - mean_t) * (t - mean_t);
        sxy += (t - mean_t) * (l - mean_l);
    }
    if sxx < 1e-12 {
        return None;
    }
    let slope = sxy / sxx; // ms per second
    let off_ms = mean_l - slope * mean_t; // intercept at t = 0
    let mut ss = 0.0;
    for &(t, l) in points {
        let r = l - (off_ms + slope * t);
        ss += r * r;
    }
    Some(Fit {
        off_ms,
        ppm: slope * 1000.0,
        rms_ms: (ss / n).sqrt(),
    })
}

/// Slope-regularized line fit over (t_s, lag_ms): minimizes
///   Σ (lag − off − slope·t)²  +  λ · slope²        (intercept unpenalized)
/// then hard-clamps slope to ±max_ppm/1000 (ms per s). `λ` is chosen so a short, sparse
/// overlap is pulled flat (the prior dominates) while a long overlap's data still refines
/// a small slope. `rms_ms` is the residual of the CLAMPED line (the honest lock error
/// under the physical drift bound). None on < 2 points or degenerate t-spread.
///
/// UNITS: `lambda_s2` is in SECONDS² — it is added directly to `sxx = Σ(t−t̄)²` (s²),
/// so it MUST share sxx's units. (Despite the penalty being written λ·slope² with slope
/// in ms/s, the centered closed-form solution is `sxy/(sxx+λ)`, where λ lands in sxx's
/// units.) With `λ = 0, max_ppm = ∞`, `fit_ls_reg` is identical to `fit_ls`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitReg {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
    pub clamped: bool,
}

/// `points`: (t seconds, lag ms). See [`FitReg`] for the units warning on `lambda_s2`.
/// `max_ppm` bounds |slope| to `±max_ppm/1000` ms/s (out-of-bound ⇒ `clamped == true`).
pub fn fit_ls_reg(points: &[(f64, f64)], lambda_s2: f64, max_ppm: f64) -> Option<FitReg> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let mean_t = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_l = points.iter().map(|p| p.1).sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for &(t, l) in points {
        sxx += (t - mean_t) * (t - mean_t);
        sxy += (t - mean_t) * (l - mean_l);
    }
    if sxx < 1e-12 {
        return None;
    }
    // Ridge on slope (centered design ⇒ intercept stays mean_l): slope = sxy/(sxx+λ).
    // λ is in s² (comparable to sxx). For sparse short overlaps sxx is small so λ
    // dominates ⇒ slope→0; for long overlaps sxx ≫ λ ⇒ slope≈OLS.
    let mut slope = sxy / (sxx + lambda_s2); // ms per second
    let mut clamped = false;
    let max_slope = max_ppm / 1000.0; // ms per s
    if slope > max_slope {
        slope = max_slope;
        clamped = true;
    }
    if slope < -max_slope {
        slope = -max_slope;
        clamped = true;
    }
    let off_ms = mean_l - slope * mean_t; // intercept at t = 0 on the (clamped) line
    let mut ss = 0.0;
    for &(t, l) in points {
        let r = l - (off_ms + slope * t);
        ss += r * r;
    }
    Some(FitReg {
        off_ms,
        ppm: slope * 1000.0,
        rms_ms: (ss / n).sqrt(),
        clamped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_line() {
        // lag = 10 ms + 3 ppm drift → slope 0.003 ms/s
        let pts: Vec<(f64, f64)> = (0..12)
            .map(|k| {
                let t = 100.0 * k as f64;
                (t, 10.0 + 0.003 * t)
            })
            .collect();
        let f = fit_ls(&pts).unwrap();
        assert!((f.off_ms - 10.0).abs() < 1e-9);
        assert!((f.ppm - 3.0).abs() < 1e-9);
        assert!(f.rms_ms < 1e-9);
    }

    #[test]
    fn degenerate() {
        assert!(fit_ls(&[(1.0, 2.0)]).is_none());
        assert!(fit_ls(&[(1.0, 2.0), (1.0, 3.0)]).is_none());
    }

    // λ=0, max_ppm=∞ ⇒ fit_ls_reg is the unregularized OLS line, identical to fit_ls.
    #[test]
    fn reg_zero_lambda_unbounded_equals_fit_ls() {
        let pts: Vec<(f64, f64)> = (0..12)
            .map(|k| {
                let t = 100.0 * k as f64;
                (t, 10.0 + 0.003 * t)
            })
            .collect();
        let f = fit_ls(&pts).unwrap();
        let r = fit_ls_reg(&pts, 0.0, f64::INFINITY).unwrap();
        assert!(!r.clamped);
        assert_eq!(f.off_ms.to_bits(), r.off_ms.to_bits());
        assert_eq!(f.ppm.to_bits(), r.ppm.to_bits());
        assert_eq!(f.rms_ms.to_bits(), r.rms_ms.to_bits());
    }

    // Regularization pulls a sparse, short-baseline fit toward flat: a strong λ relative
    // to the tiny sxx of a short span shrinks the recovered slope well below OLS.
    #[test]
    fn reg_pulls_sparse_short_fit_flat() {
        // 3 points over a 4 s baseline with a steep 50 ppm slope (0.05 ms/s). sxx is
        // ~O(10 s²), so a λ of 200 s² dominates and flattens the slope toward 0.
        let pts: Vec<(f64, f64)> = vec![(0.0, 0.0), (2.0, 0.10), (4.0, 0.20)];
        let ols = fit_ls_reg(&pts, 0.0, f64::INFINITY).unwrap();
        let reg = fit_ls_reg(&pts, 200.0, f64::INFINITY).unwrap();
        assert!(!reg.clamped);
        assert!(
            reg.ppm.abs() < ols.ppm.abs() * 0.5,
            "regularized slope {} ppm should be much flatter than OLS {} ppm",
            reg.ppm,
            ols.ppm
        );
    }

    // The clamp marks an out-of-bound slope and reports rms to the clamped (not OLS) line.
    #[test]
    fn reg_clamp_flags_out_of_bound_slope() {
        // A near-perfect line at 50 ppm (0.05 ms/s); cap to 10 ppm (0.01 ms/s).
        let pts: Vec<(f64, f64)> = (0..10)
            .map(|k| {
                let t = 10.0 * k as f64;
                (t, 0.05 * t)
            })
            .collect();
        let r = fit_ls_reg(&pts, 0.0, 10.0).unwrap();
        assert!(r.clamped, "50 ppm slope clamped to a 10 ppm bound must flag");
        assert!((r.ppm.abs() - 10.0).abs() < 1e-9, "slope clamped to the bound");
        // Residual to the clamped line is large (the data wanted 50 ppm), a rejection signal.
        assert!(r.rms_ms > 1.0, "clamped-line rms {} should be large", r.rms_ms);
    }
}
