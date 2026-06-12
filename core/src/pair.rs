//! Role-agnostic pair correlation (SPEC-V2 §3.2/§4) — the stage-1 envelope search
//! plus the §5.3 refinement ladder between ANY two imported tracks. Core stays
//! mechanical: measurements only. Coverage intervals, resolution states, triangle
//! closure and margins live in the shell/CLI layer where the user can override them.
//!
//! Role-freedom note (Session F verification): stage-1 and the refinement stages
//! operate purely on pyramid envelopes; the epoch rule (§1.2/§7.2) is baked into the
//! pyramid at import, and `TrackRole` is carried only in `TrackMeta`. Nothing in the
//! §5.2/§5.3 path assumes camera/external roles — no SPEC-CHANGE required.

use crate::envelope;
use crate::pyramid::{PyramidReader, SAMPLE_RATE};
use crate::sync::{self, SyncParams};
use serde::{Deserialize, Serialize};

/// One stage-1 measurement between tracks A and B.
///
/// Lag convention (identical to `SyncResult.offset_ms` when A = video, B = audio):
/// content at time `t` in A occurs at `t + lag_ms/1000` in B (drift ignored).
/// Deterministic: same inputs → bit-identical values on the same machine (§5.8),
/// which the v2 stability gate (SPEC-V2 §7.6) relies on. No timing fields here —
/// resolved rows must be byte-stable across runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairCorrelation {
    /// Stage-1 selection score z = NCC·√n (0.0 when no scannable lag).
    pub z: f64,
    /// Refined lag when `refined`, else the stage-1 coarse lag.
    pub lag_ms: f64,
    /// Stage-1 winner NCC.
    pub ncc: f64,
    /// Stage-5 NCC; None when the peak was below floor and refinement was skipped.
    pub fine_ncc: Option<f64>,
    pub overlap_bins: u64,
    /// Best z at distance > 2 bins from the winner (§5.3).
    pub runner_up_z: f64,
    /// Stages 2–5 ran (z ≥ z_min); below-floor cells keep the coarse lag for
    /// corpus calibration but are never refined.
    pub refined: bool,
    /// Tracks cannot overlap ≥ MIN_OVERLAP_BINS at any lag.
    pub insufficient_overlap: bool,
}

impl PairCorrelation {
    fn empty(insufficient: bool) -> Self {
        Self {
            z: 0.0,
            lag_ms: 0.0,
            ncc: 0.0,
            fine_ncc: None,
            overlap_bins: 0,
            runner_up_z: 0.0,
            refined: false,
            insufficient_overlap: insufficient,
        }
    }
}

/// Stage-1 z-selection between any two tracks, refined through §5.3 stages 2–5 when
/// the peak clears `params.z_min`. Pure measurement — never errors on a poor match.
pub fn pair_correlate(p_a: &PyramidReader, p_b: &PyramidReader, params: &SyncParams) -> PairCorrelation {
    let a = envelope::envelope_full(p_a, 9600);
    let b = envelope::envelope_full(p_b, 9600);
    let bin_ms = 9600.0 / SAMPLE_RATE as f64 * 1000.0; // 200 ms
    if (a.len() as u64) < params.min_overlap_bins || (b.len() as u64) < params.min_overlap_bins {
        return PairCorrelation::empty(true);
    }
    let Some(s1) = sync::stage1_scan(&a, &b, params.min_overlap_bins, params.parabolic_clamp) else {
        return PairCorrelation::empty(true);
    };
    let coarse_lag_ms = (s1.best_lag as f64 + s1.delta) * bin_ms;
    let mut out = PairCorrelation {
        z: s1.best_z,
        lag_ms: coarse_lag_ms,
        ncc: s1.best_ncc,
        fine_ncc: None,
        overlap_bins: s1.best_n,
        runner_up_z: s1.runner_up_z,
        refined: false,
        insufficient_overlap: false,
    };
    if s1.best_z < params.z_min {
        return out;
    }
    // §5.3 stages 2–5: 2400/480/96/24 spb over ±500/±80/±14/±3 ms, max NCC.
    let mut lag_ms = coarse_lag_ms;
    let mut fine_ncc = s1.best_ncc;
    for (&spb, &span) in [2400u32, 480, 96, 24].iter().zip(&params.stage_spans_ms) {
        if let Some((l, n)) = sync::refine_stage(p_a, p_b, spb, lag_ms, span, params.parabolic_clamp) {
            lag_ms = l;
            fine_ncc = n;
        }
    }
    out.lag_ms = lag_ms;
    out.fine_ncc = Some(fine_ncc);
    out.refined = true;
    out
}
