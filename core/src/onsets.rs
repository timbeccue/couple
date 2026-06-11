//! Onset ranking from the 480 spb level (§5.7; UX §4.5). Linear span domain, not log.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Onset {
    pub t: f64,
    pub score: f64,
}

pub struct OnsetParams {
    pub rise_bins: usize,      // "20 ms rise" = 2 bins at 480 spb
    pub prec_from: usize,      // preceding window: bins i-30 .. i-3 (280 ms)
    pub prec_to: usize,
    pub floor_frac: f64,       // 0.15 · s_max in the score denominator
    pub rise_gate_frac: f64,   // eligible: rise > 0.10 · s_max
    pub nms_s: f64,            // 0.3 s non-max suppression
    pub edge_skip_s: f64,      // 1 s from either file edge
    pub cap: usize,            // 4000
}

impl Default for OnsetParams {
    fn default() -> Self {
        Self {
            rise_bins: 2,
            prec_from: 30,
            prec_to: 3,
            floor_frac: 0.15,
            rise_gate_frac: 0.10,
            nms_s: 0.3,
            edge_skip_s: 1.0,
            cap: 4000,
        }
    }
}

/// `spans`: per-bin span (max−min) in the int domain 0..254 at 480 spb.
/// Returns the ranked list (score-descending), capped.
pub fn rank_onsets(spans: &[u8], spb: u32, sample_rate: u32, p: &OnsetParams) -> Vec<Onset> {
    let n = spans.len();
    let bin_s = spb as f64 / sample_rate as f64;
    let duration_s = n as f64 * bin_s;
    let s_max = spans.iter().copied().max().unwrap_or(0) as f64;
    if s_max <= 0.0 {
        return Vec::new();
    }
    let rise_gate = p.rise_gate_frac * s_max;
    let denom_floor = p.floor_frac * s_max;

    // Prefix sums for the preceding-level mean.
    let mut prefix = vec![0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + spans[i] as f64;
    }

    let mut cands: Vec<Onset> = Vec::new();
    for i in p.prec_from..n {
        let t = i as f64 * bin_s;
        if t < p.edge_skip_s || t > duration_s - p.edge_skip_s {
            continue;
        }
        let rise = spans[i] as f64 - spans[i - p.rise_bins] as f64;
        if rise <= rise_gate {
            continue;
        }
        let a = i - p.prec_from;
        let b = i - p.prec_to;
        let prec = (prefix[b + 1] - prefix[a]) / (b + 1 - a) as f64;
        let score = rise * rise / (prec + denom_floor);
        cands.push(Onset { t, score });
    }

    // Non-max suppression: within nms_s keep the max score. Score-descending greedy,
    // neighbor lookup via a time-sorted list + binary search.
    cands.sort_by(|x, y| y.score.total_cmp(&x.score));
    let mut kept: Vec<Onset> = Vec::new();
    let mut by_time: Vec<f64> = Vec::new();
    for c in cands {
        if kept.len() >= p.cap {
            break;
        }
        let i = by_time.partition_point(|&t| t < c.t);
        let clear = (i == 0 || c.t - by_time[i - 1] >= p.nms_s)
            && (i == by_time.len() || by_time[i] - c.t >= p.nms_s);
        if clear {
            by_time.insert(i, c.t);
            kept.push(c);
        }
    }
    kept
}

/// Highest-scoring onset with t in [t0, t1].
pub fn best_in(onsets: &[Onset], t0: f64, t1: f64) -> Option<Onset> {
    onsets
        .iter()
        .filter(|o| o.t >= t0 && o.t <= t1)
        .max_by(|a, b| a.score.total_cmp(&b.score))
        .copied()
}

/// Snap (UX §2.2): highest-scoring onset within ±half_span of t.
pub fn best_near(onsets: &[Onset], t: f64, half_span_s: f64) -> Option<Onset> {
    best_in(onsets, t - half_span_s, t + half_span_s)
}

/// Top `limit` onsets in [t0, t1] by score.
pub fn in_range(onsets: &[Onset], t0: f64, t1: f64, limit: usize) -> Vec<Onset> {
    let mut v: Vec<Onset> = onsets.iter().filter(|o| o.t >= t0 && o.t <= t1).copied().collect();
    v.sort_by(|x, y| y.score.total_cmp(&x.score));
    v.truncate(limit);
    v
}
