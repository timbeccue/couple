//! Windowed local-lag measurement (§5.4) — powers local Δ, the drift pass, sparkline,
//! and ⇥ align. Budget ≤ 5 ms per call at the default ±80 ms span.

use crate::envelope;
use crate::pyramid::{PyramidReader, SAMPLE_RATE};
use crate::session::LocalMeasurement;

const SR: f64 = SAMPLE_RATE as f64;

/// NCC of a fixed window `a` against `b_super[shift .. shift+n]` for integer bin shifts.
/// Returns (best_shift_index, ncc_values). Skips shifts whose window leaves b_super.
fn scan(a: &[f64], b_super: &[f64], shifts: std::ops::RangeInclusive<i64>, base: i64) -> Vec<f64> {
    let n = a.len();
    let mean_a = a.iter().sum::<f64>() / n as f64;
    let var_a: f64 = a.iter().map(|x| (x - mean_a) * (x - mean_a)).sum();
    if var_a < 1e-9 {
        return Vec::new(); // §5.4 variance gate: silence
    }
    // prefix sums over b_super
    let mut pb = vec![0f64; b_super.len() + 1];
    let mut pbb = vec![0f64; b_super.len() + 1];
    for (i, &x) in b_super.iter().enumerate() {
        pb[i + 1] = pb[i] + x;
        pbb[i + 1] = pbb[i] + x * x;
    }
    shifts
        .map(|s| {
            let o = s - base;
            if o < 0 || (o as usize + n) > b_super.len() {
                return f64::NEG_INFINITY;
            }
            let o = o as usize;
            let sb = pb[o + n] - pb[o];
            let sbb = pbb[o + n] - pbb[o];
            let var_b = sbb - sb * sb / n as f64;
            if var_b < 1e-9 {
                return f64::NEG_INFINITY;
            }
            let bw = &b_super[o..o + n];
            let mut cross = 0f64;
            for i in 0..n {
                cross += a[i] * bw[i];
            }
            let num = cross - mean_a * sb; // = cross − n·mean_a·mean_b
            num / (var_a * var_b).sqrt()
        })
        .collect()
}

fn parabolic(ym: f64, y0: f64, yp: f64) -> f64 {
    let denom = ym - 2.0 * y0 + yp;
    if denom.abs() < 1e-12 {
        return 0.0;
    }
    (0.5 * (ym - yp) / denom).clamp(-0.6, 0.6)
}

/// One scan pass at a pyramid level: camera window [tc−hw, tc+hw], external lags over
/// `center_ms ± span_ms` in bin steps. Returns (lag_ms, ncc) of the integer-bin winner
/// plus its neighbors for sub-bin refinement, or None per the §5.4 gates.
fn scan_level(
    cam: &PyramidReader,
    ext: &PyramidReader,
    spb: u32,
    tc: f64,
    half_win_s: f64,
    center_ms: f64,
    span_ms: f64,
    sub_bin: bool,
    reject_edges: bool,
) -> Option<(f64, f64)> {
    let bin_s = spb as f64 / SR;
    let bin_ms = bin_s * 1000.0;
    let n_cam = cam.n_bins(spb) as i64;
    let n_ext = ext.n_bins(spb) as i64;

    let c0 = (((tc - half_win_s) / bin_s).floor() as i64).max(0);
    let c1 = (((tc + half_win_s) / bin_s).ceil() as i64).min(n_cam);
    if c1 - c0 < 100 {
        return None;
    }
    let a = envelope::envelope(cam, spb, c0 as u64, c1 as u64);

    let l_min = ((center_ms - span_ms) / bin_ms).round() as i64;
    let l_max = ((center_ms + span_ms) / bin_ms).round() as i64;
    // external superwindow covering all candidate shifts of the camera window
    let e0 = (c0 + l_min).max(0);
    let e1 = (c1 + l_max).min(n_ext);
    if e1 - e0 < c1 - c0 {
        // external side cannot fully contain the window anywhere near this center
        if e1 - e0 < 100 {
            return None;
        }
    }
    let b = envelope::envelope(ext, spb, e0 as u64, e1 as u64);
    let nccs = scan(&a, &b, l_min..=l_max, e0 - c0);
    if nccs.is_empty() {
        return None;
    }
    let bi = nccs
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .map(|(i, _)| i)?;
    if !nccs[bi].is_finite() {
        return None;
    }
    // §5.4 "degenerate peak": a winner at the span edge — or hugging the region where
    // the external window would leave the track (−∞ neighbors) — means the true peak's
    // support is truncated; the measured lag would be junk, so refuse.
    if reject_edges
        && (bi == 0
            || bi == nccs.len() - 1
            || !nccs[bi - 1].is_finite()
            || !nccs[bi + 1].is_finite())
    {
        return None;
    }
    let lag_bins = l_min + bi as i64;
    let delta = if sub_bin && bi > 0 && bi + 1 < nccs.len() && nccs[bi - 1].is_finite() && nccs[bi + 1].is_finite() {
        parabolic(nccs[bi - 1], nccs[bi], nccs[bi + 1])
    } else {
        0.0
    };
    Some(((lag_bins as f64 + delta) * bin_ms, nccs[bi]))
}

/// §5.4 measure_local: coarse 96 spb scan over `center ± span` (2 ms steps), fine
/// ±3 ms at 24 spb (0.5 ms steps), parabolic sub-bin. None on silence/degenerate peak.
pub fn measure_local(
    cam: &PyramidReader,
    ext: &PyramidReader,
    tc: f64,
    half_win_s: f64,
    center_ms: f64,
    span_ms: f64,
) -> Option<LocalMeasurement> {
    let (coarse_lag, _) = scan_level(cam, ext, 96, tc, half_win_s, center_ms, span_ms, false, true)?;
    let (lag_ms, ncc) = scan_level(cam, ext, 24, tc, half_win_s, coarse_lag, 3.0, true, false)?;
    Some(LocalMeasurement { lag_ms, ncc })
}
