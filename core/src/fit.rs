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
}
