//! Sync pipeline (§5): global offset search (stages 1–5), drift pass (Phase 3),
//! status heuristics. All lag math in f64; FFT scoring path bit-stable (§5.8).

use crate::envelope;
use crate::error::{CoupleError, Result};
use crate::job::{CancelToken, Phase, ProgressEmitter};
use crate::pyramid::{PyramidReader, SAMPLE_RATE};
use crate::session::DriftMode;
use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncParams {
    /// Stage-1 minimum overlap, bins @ 9600 spb (= 20 s) [DECIDED]
    pub min_overlap_bins: u64,
    /// Failed threshold on stage-1 best z [TUNE]
    pub z_min: f64,
    /// Drift-window acceptance NCC [VALIDATED]
    pub ncc_window_min: f64,
    /// Synced gate [TUNE]
    pub synced_fine_ncc: f64,
    pub synced_min_windows: usize,
    pub synced_max_rms_ms: f64,
    /// Drift-mode suggestion threshold, ms end-to-end [DECIDED form, TUNE value]
    pub suggest_resample_ms: f64,
    /// Drift pass (§5.5)
    pub drift_windows: usize,
    pub half_win_s: f64,
    pub drift_base_span_ms: f64,
    pub drift_span_scale_ppm: f64,
    pub drift_span_cap_ms: f64,
    pub refit_ppm_threshold: f64,
    /// Refinement stage spans, ms [VALIDATED]
    pub stage_spans_ms: [f64; 4],
    /// Parabolic sub-bin clamp [SPEC]
    pub parabolic_clamp: f64,
}

impl Default for SyncParams {
    fn default() -> Self {
        Self {
            min_overlap_bins: 100,
            // SPEC-CHANGE (2026-06-11): Appendix A said 20 [TUNE], but z at the §9.3
            // partial-overlap floor (100 bins) cannot exceed √100 = 10, so 20 would make
            // that row unsatisfiable. 8.5 sits between the measured unrelated-content
            // ceiling (≤ 7.5) and the weakest must-pass true peak (9.9 at 20 s, SNR 10).
            z_min: 8.5,
            ncc_window_min: 0.4,
            synced_fine_ncc: 0.5,
            synced_min_windows: 6,
            synced_max_rms_ms: 1.0,
            suggest_resample_ms: 2.0,
            drift_windows: 12,
            half_win_s: 9.0,
            drift_base_span_ms: 120.0,
            drift_span_scale_ppm: 150.0,
            drift_span_cap_ms: 2000.0,
            refit_ppm_threshold: 20.0,
            stage_spans_ms: [500.0, 80.0, 14.0, 3.0],
            parabolic_clamp: 0.6,
        }
    }
}

impl SyncParams {
    /// CLI `--params k=v` override.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let v: f64 = value
            .parse()
            .map_err(|_| CoupleError::invalid(format!("--params {key}: not a number")))?;
        match key {
            "min-overlap-bins" => self.min_overlap_bins = v as u64,
            "z-min" => self.z_min = v,
            "ncc-window-min" => self.ncc_window_min = v,
            "synced-fine-ncc" => self.synced_fine_ncc = v,
            "synced-min-windows" => self.synced_min_windows = v as usize,
            "synced-max-rms-ms" => self.synced_max_rms_ms = v,
            "suggest-resample-ms" => self.suggest_resample_ms = v,
            "drift-windows" => self.drift_windows = v as usize,
            "half-win-s" => self.half_win_s = v,
            "drift-base-span-ms" => self.drift_base_span_ms = v,
            "drift-span-cap-ms" => self.drift_span_cap_ms = v,
            "refit-ppm-threshold" => self.refit_ppm_threshold = v,
            other => return Err(CoupleError::invalid(format!("unknown param {other}"))),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagePeak {
    pub lag_ms: f64,
    pub ncc: f64,
    pub z: f64,
    pub overlap_bins: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftWindow {
    pub t: f64,
    pub lag_ms: f64,
    pub ncc: f64,
    pub accepted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlapInfo {
    pub t0: f64,
    pub t1: f64,
    pub seconds: f64,
    pub fraction_of_video: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum FailReason {
    NoPlausiblePeak { best_z: f64 },
    InsufficientOverlap { overlap_s: f64 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "status", content = "fail", rename_all = "camelCase")]
pub enum SyncStatus {
    Synced,
    LowConfidence,
    Failed(FailReason),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub offset_ms: f64,
    pub drift_ppm: f64,
    pub drift_measurable: bool,
    pub coarse: StagePeak,
    pub runner_up_z: f64,
    pub fine_ncc: f64,
    pub windows: Vec<DriftWindow>,
    pub fit_rms_ms: Option<f64>,
    pub end_to_end_drift_ms: f64,
    pub overlap: OverlapInfo,
    #[serde(flatten)]
    pub status: SyncStatus,
    pub suggested_drift_mode: DriftMode,
    pub compute_ms: f64,
}

// ---------- NCC machinery (§5.2) ----------

/// Per-lag Pearson NCC of `a` vs `b[·+ℓ]` for all ℓ in [min_lag, max_lag], using prefix
/// sums + a caller-supplied cross-term vector (FFT or brute force). Returns (ncc, n) rows.
struct LagScorer {
    pa: Vec<f64>,
    paa: Vec<f64>,
    pb: Vec<f64>,
    pbb: Vec<f64>,
    na: usize,
    nb: usize,
}

impl LagScorer {
    fn new(a: &[f64], b: &[f64]) -> Self {
        let prefix = |v: &[f64], sq: bool| {
            let mut p = vec![0f64; v.len() + 1];
            for (i, &x) in v.iter().enumerate() {
                p[i + 1] = p[i] + if sq { x * x } else { x };
            }
            p
        };
        Self {
            pa: prefix(a, false),
            paa: prefix(a, true),
            pb: prefix(b, false),
            pbb: prefix(b, true),
            na: a.len(),
            nb: b.len(),
        }
    }

    fn overlap(&self, lag: i64) -> (usize, usize) {
        let i0 = (-lag).max(0) as usize;
        let i1 = (self.na as i64).min(self.nb as i64 - lag).max(0) as usize;
        (i0, i1.max(i0))
    }

    /// Pearson NCC at integer lag given the raw cross sum Σ a[i]·b[i+ℓ] over the overlap.
    fn ncc_at(&self, lag: i64, cross: f64) -> Option<(f64, u64)> {
        let (i0, i1) = self.overlap(lag);
        let n = (i1 - i0) as f64;
        if i1 <= i0 {
            return None;
        }
        let sa = self.pa[i1] - self.pa[i0];
        let saa = self.paa[i1] - self.paa[i0];
        let j0 = (i0 as i64 + lag) as usize;
        let j1 = (i1 as i64 + lag) as usize;
        let sb = self.pb[j1] - self.pb[j0];
        let sbb = self.pbb[j1] - self.pbb[j0];
        let var_a = saa - sa * sa / n;
        let var_b = sbb - sb * sb / n;
        // §5.8 guard: σ < 1e−9 → NCC undefined → skip lag.
        if var_a < 1e-9 || var_b < 1e-9 {
            return None;
        }
        let num = cross - sa * sb / n;
        Some((num / (var_a * var_b).sqrt(), (i1 - i0) as u64))
    }
}

/// Cross-correlation Σ a[i]·b[i+ℓ] for ℓ ∈ [−(Na−1), Nb−1] via FFT linear correlation
/// (real FFT, zero-padded ≥ Na+Nb−1) [SPEC F6]. Index: result[ℓ + Na − 1].
fn cross_fft(a: &[f64], b: &[f64]) -> Vec<f64> {
    let na = a.len();
    let nb = b.len();
    let full = na + nb - 1;
    let n = full.next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);

    // conv(a_rev, b)[k] = Σ a[i] b[k − (Na−1) + i] → cross(ℓ = k − Na + 1)
    let mut fa = vec![0f64; n];
    for (i, &v) in a.iter().enumerate() {
        fa[na - 1 - i] = v;
    }
    let mut fb = vec![0f64; n];
    fb[..nb].copy_from_slice(b);

    let mut sa = fft.make_output_vec();
    let mut sb = fft.make_output_vec();
    fft.process(&mut fa, &mut sa).unwrap();
    fft.process(&mut fb, &mut sb).unwrap();
    for (x, y) in sa.iter_mut().zip(&sb) {
        *x *= *y;
    }
    let mut out = ifft.make_output_vec();
    ifft.process(&mut sa, &mut out).unwrap();
    let scale = 1.0 / n as f64;
    out.truncate(full);
    for v in &mut out {
        *v *= scale;
    }
    out
}

/// Brute-force cross term (test oracle for the FFT path, §5.2 equivalence requirement).
pub fn cross_brute(a: &[f64], b: &[f64], lag: i64) -> f64 {
    let i0 = (-lag).max(0) as usize;
    let i1 = (a.len() as i64).min(b.len() as i64 - lag).max(0) as usize;
    let mut s = 0.0;
    for i in i0..i1.max(i0) {
        s += a[i] * b[(i as i64 + lag) as usize];
    }
    s
}

/// Stage-1 scan output (z-selection): winner + runner-up z outside ±2 bins.
pub struct Stage1 {
    pub best_lag: i64,
    pub best_ncc: f64,
    pub best_z: f64,
    pub best_n: u64,
    pub runner_up_z: f64,
    pub delta: f64, // parabolic sub-bin refinement on z
}

/// z(ℓ) = NCC(ℓ)·√n(ℓ), n ≥ min_overlap [DECIDED]. Sequential argmax → deterministic.
pub fn stage1_scan(a: &[f64], b: &[f64], min_overlap: u64, clamp: f64) -> Option<Stage1> {
    let scorer = LagScorer::new(a, b);
    let cross = cross_fft(a, b);
    let na = a.len() as i64;
    let nb = b.len() as i64;
    let lag_min = min_overlap as i64 - na;
    let lag_max = nb - min_overlap as i64;
    if lag_min > lag_max {
        return None;
    }
    let mut zs: Vec<f64> = Vec::with_capacity((lag_max - lag_min + 1) as usize);
    let mut best: Option<(i64, f64, f64, u64)> = None;
    for lag in lag_min..=lag_max {
        let c = cross[(lag + na - 1) as usize];
        let z = match scorer.ncc_at(lag, c) {
            Some((ncc, n)) if n >= min_overlap => {
                let z = ncc * (n as f64).sqrt();
                if best.map(|(_, _, bz, _)| z > bz).unwrap_or(true) {
                    best = Some((lag, ncc, z, n));
                }
                z
            }
            _ => f64::NEG_INFINITY,
        };
        zs.push(z);
    }
    let (best_lag, best_ncc, best_z, best_n) = best?;
    let bi = (best_lag - lag_min) as usize;

    let mut runner_up_z = f64::NEG_INFINITY;
    for (i, &z) in zs.iter().enumerate() {
        if (i as i64 - bi as i64).abs() > 2 && z > runner_up_z {
            runner_up_z = z;
        }
    }

    // Parabolic sub-bin on the selection score; skipped at span edges (§5.3).
    let delta = if bi > 0 && bi + 1 < zs.len() && zs[bi - 1].is_finite() && zs[bi + 1].is_finite() {
        parabolic(zs[bi - 1], zs[bi], zs[bi + 1], clamp)
    } else {
        0.0
    };
    Some(Stage1 {
        best_lag,
        best_ncc,
        best_z,
        best_n,
        runner_up_z,
        delta,
    })
}

/// Diagnostic/regression helper: stage-1 argmax by RAW NCC (the selection rule §5.2
/// rejects). The spurious-peak regression asserts this picks a boundary lag where
/// z-selection picks truth (UX §4.2 finding).
pub fn stage1_raw_ncc_argmax(a: &[f64], b: &[f64], min_overlap: u64) -> Option<(i64, f64)> {
    let scorer = LagScorer::new(a, b);
    let cross = cross_fft(a, b);
    let na = a.len() as i64;
    let nb = b.len() as i64;
    let mut best: Option<(i64, f64)> = None;
    for lag in (min_overlap as i64 - na)..=(nb - min_overlap as i64) {
        if let Some((ncc, n)) = scorer.ncc_at(lag, cross[(lag + na - 1) as usize]) {
            if n >= min_overlap && best.map(|(_, bn)| ncc > bn).unwrap_or(true) {
                best = Some((lag, ncc));
            }
        }
    }
    best
}

/// δ = 0.5·(y₋ − y₊)/(y₋ − 2y₀ + y₊), clamped (§5.3).
fn parabolic(ym: f64, y0: f64, yp: f64, clamp: f64) -> f64 {
    let denom = ym - 2.0 * y0 + yp;
    if denom.abs() < 1e-12 {
        return 0.0;
    }
    (0.5 * (ym - yp) / denom).clamp(-clamp, clamp)
}

/// One refinement stage (§5.3 stages 2–5): scan integer bin lags in `center ± span`,
/// max NCC, parabolic sub-bin. Returns (refined lag in ms, winner NCC).
fn refine_stage(
    p_cam: &PyramidReader,
    p_ext: &PyramidReader,
    spb: u32,
    center_ms: f64,
    span_ms: f64,
    clamp: f64,
) -> Option<(f64, f64)> {
    let bin_ms = spb as f64 / SAMPLE_RATE as f64 * 1000.0;
    let a = envelope::envelope_full(p_cam, spb);
    let b = envelope::envelope_full(p_ext, spb);
    let scorer = LagScorer::new(&a, &b);
    let center = (center_ms / bin_ms).round() as i64;
    let span = (span_ms / bin_ms).round() as i64;
    let lags: Vec<i64> = (center - span..=center + span).collect();
    use rayon::prelude::*;
    let nccs: Vec<f64> = lags
        .par_iter()
        .map(|&lag| {
            let c = cross_brute(&a, &b, lag);
            scorer.ncc_at(lag, c).map(|(ncc, _)| ncc).unwrap_or(f64::NEG_INFINITY)
        })
        .collect();
    let bi = nccs
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .map(|(i, _)| i)?;
    if !nccs[bi].is_finite() {
        return None;
    }
    let delta = if bi > 0 && bi + 1 < nccs.len() && nccs[bi - 1].is_finite() && nccs[bi + 1].is_finite()
    {
        parabolic(nccs[bi - 1], nccs[bi], nccs[bi + 1], clamp)
    } else {
        0.0
    };
    Some(((lags[bi] as f64 + delta) * bin_ms, nccs[bi]))
}

// ---------- the global offset search (§5.3) ----------

pub struct OffsetSearch {
    pub coarse: StagePeak,
    pub runner_up_z: f64,
    pub offset_stage_ms: f64,
    pub fine_ncc: f64,
}

pub fn offset_search(
    p_cam: &PyramidReader,
    p_ext: &PyramidReader,
    params: &SyncParams,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<std::result::Result<OffsetSearch, FailReason>> {
    // Stage 1 @ 9600 spb (0.2 s bins), full ±(dur_cam + dur_ext), z-selection.
    prog.emit(Phase::SyncCoarse, 0.05);
    let a = envelope::envelope_full(p_cam, 9600);
    let b = envelope::envelope_full(p_ext, 9600);
    let bin_ms = 9600.0 / SAMPLE_RATE as f64 * 1000.0; // 200 ms
    if (a.len() as u64) < params.min_overlap_bins || (b.len() as u64) < params.min_overlap_bins {
        let overlap_s = a.len().min(b.len()) as f64 * bin_ms / 1000.0;
        return Ok(Err(FailReason::InsufficientOverlap { overlap_s }));
    }
    cancel.check()?;
    prog.emit(Phase::SyncCoarse, 0.2);
    let Some(s1) = stage1_scan(&a, &b, params.min_overlap_bins, params.parabolic_clamp) else {
        return Ok(Err(FailReason::InsufficientOverlap {
            overlap_s: a.len().min(b.len()) as f64 * bin_ms / 1000.0,
        }));
    };
    prog.emit(Phase::SyncCoarse, 1.0);
    cancel.check()?;

    let coarse = StagePeak {
        lag_ms: (s1.best_lag as f64 + s1.delta) * bin_ms,
        ncc: s1.best_ncc,
        z: s1.best_z,
        overlap_bins: s1.best_n,
    };
    if s1.best_z < params.z_min {
        return Ok(Err(FailReason::NoPlausiblePeak { best_z: s1.best_z }));
    }

    // Stages 2–5 (§5.3): 2400/480/96/24 spb over ±500/±80/±14/±3 ms, max NCC.
    let mut lag_ms = coarse.lag_ms;
    let mut fine_ncc = coarse.ncc;
    for (i, (&spb, &span)) in [2400u32, 480, 96, 24]
        .iter()
        .zip(&params.stage_spans_ms)
        .enumerate()
    {
        cancel.check()?;
        if let Some((l, n)) = refine_stage(p_cam, p_ext, spb, lag_ms, span, params.parabolic_clamp)
        {
            lag_ms = l;
            fine_ncc = n;
        }
        prog.emit(Phase::SyncRefine, (i + 1) as f64 / 4.0);
    }

    Ok(Ok(OffsetSearch {
        coarse,
        runner_up_z: s1.runner_up_z,
        offset_stage_ms: lag_ms,
        fine_ncc,
    }))
}

// ---------- drift pass + full pipeline (§5.5/§5.6) ----------

struct DriftFit {
    windows: Vec<DriftWindow>,
    accepted: usize,
    fit: Option<crate::fit::Fit>,
}

/// §5.5: 12 windows over the overlap, measure_local each with distance-scaled spans,
/// accept ncc > NCC_WINDOW_MIN, least-squares fit; one refit iteration when |ppm| > 20.
fn drift_pass(
    p_cam: &PyramidReader,
    p_ext: &PyramidReader,
    offset_stage_ms: f64,
    params: &SyncParams,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<DriftFit> {
    let dur_cam = p_cam.duration_s();
    let dur_ext = p_ext.duration_s();
    let off_s = offset_stage_ms / 1000.0;
    let t0 = (-off_s).max(0.0);
    let t1 = (dur_ext - off_s).min(dur_cam);
    let hw = params.half_win_s;
    let k_n = params.drift_windows;
    let t_mid = (t0 + t1) / 2.0;

    // Clamp centers so the 18 s window stays inside both tracks; when the tracks are
    // too short for that, leave centers as-is and let measure_local's gates refuse.
    let lo = hw.max(hw - off_s);
    let hi = (dur_cam - hw).min(dur_ext - off_s - hw);
    let centers: Vec<f64> = (0..k_n)
        .map(|k| {
            let t = t0 + (k as f64 + 0.5) / k_n as f64 * (t1 - t0);
            if lo < hi {
                t.clamp(lo, hi)
            } else {
                t
            }
        })
        .collect();

    let measure_all = |center_of: &dyn Fn(f64) -> f64,
                       span_of: &dyn Fn(f64) -> f64,
                       prog: &mut ProgressEmitter,
                       cancel: &CancelToken|
     -> Result<Vec<DriftWindow>> {
        let mut out = Vec::with_capacity(k_n);
        for (k, &tc) in centers.iter().enumerate() {
            cancel.check()?;
            let m = crate::measure::measure_local(
                p_cam,
                p_ext,
                tc,
                hw,
                center_of(tc),
                span_of(tc),
            );
            let (lag_ms, ncc) = m.map(|m| (m.lag_ms, m.ncc)).unwrap_or((0.0, f64::NEG_INFINITY));
            out.push(DriftWindow {
                t: tc,
                lag_ms,
                ncc: if ncc.is_finite() { ncc } else { 0.0 },
                accepted: ncc > params.ncc_window_min,
            });
            prog.emit(Phase::SyncWindows, (k + 1) as f64 / (k_n + 1) as f64);
        }
        Ok(out)
    };

    // First pass: centers on the stage offset, spans scaled by distance from overlap
    // midpoint (F6: covers ±150 ppm true drift at long range), cap ±2 s.
    let base = params.drift_base_span_ms;
    let scale = params.drift_span_scale_ppm;
    let cap = params.drift_span_cap_ms;
    let mut windows = measure_all(
        &|_tc| offset_stage_ms,
        &|tc: f64| (base + scale * 1e-3 * (tc - t_mid).abs()).min(cap),
        prog,
        cancel,
    )?;
    let mut accepted_pts: Vec<(f64, f64)> = windows
        .iter()
        .filter(|w| w.accepted)
        .map(|w| (w.t, w.lag_ms))
        .collect();
    let mut fit = if accepted_pts.len() >= 3 {
        crate::fit::fit_ls(&accepted_pts)
    } else {
        None
    };

    // §5.5 iteration [SHOULD]: at high ppm, re-measure with centers on the fitted
    // mapping (recovers windows the first pass missed at long range), then refit.
    if let Some(f) = fit {
        if f.ppm.abs() > params.refit_ppm_threshold {
            windows = measure_all(
                &|tc: f64| f.off_ms + tc * f.ppm * 1e-3,
                &|_tc| base,
                prog,
                cancel,
            )?;
            accepted_pts = windows
                .iter()
                .filter(|w| w.accepted)
                .map(|w| (w.t, w.lag_ms))
                .collect();
            if accepted_pts.len() >= 3 {
                fit = crate::fit::fit_ls(&accepted_pts);
            }
        }
    }

    prog.emit(Phase::SyncWindows, 1.0);
    Ok(DriftFit {
        accepted: windows.iter().filter(|w| w.accepted).count(),
        windows,
        fit: if accepted_pts.len() >= 3 { fit } else { None },
    })
}

/// Full auto-sync (§5): offset search + drift pass + status/suggestion heuristics.
pub fn auto_sync_tracks(
    p_cam: &PyramidReader,
    p_ext: &PyramidReader,
    params: &SyncParams,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<SyncResult> {
    let started = std::time::Instant::now();
    let dur_cam = p_cam.duration_s();
    let dur_ext = p_ext.duration_s();
    let search = match offset_search(p_cam, p_ext, params, prog, cancel)? {
        Err(reason) => {
            let mut r = offset_only_sync(p_cam, p_ext, params, prog, cancel)?;
            r.status = SyncStatus::Failed(reason);
            return Ok(r);
        }
        Ok(s) => s,
    };

    let drift = drift_pass(p_cam, p_ext, search.offset_stage_ms, params, prog, cancel)?;
    prog.emit(Phase::SyncFit, 1.0);

    // §5.5: the fit replaces the stage offset when drift is measurable.
    let (offset_ms, drift_ppm, drift_measurable, fit_rms_ms) = match drift.fit {
        Some(f) => (f.off_ms, f.ppm, true, Some(f.rms_ms)),
        None => (search.offset_stage_ms, 0.0, false, None),
    };
    let overlap = overlap_info(dur_cam, dur_ext, offset_ms, drift_ppm);
    let end_to_end_drift_ms = drift_ppm.abs() * overlap.seconds / 1000.0;
    let suggested = if end_to_end_drift_ms > params.suggest_resample_ms {
        DriftMode::Resample
    } else {
        DriftMode::None
    };
    // §5.6 status gates.
    let status = if search.fine_ncc >= params.synced_fine_ncc
        && drift.accepted >= params.synced_min_windows
        && fit_rms_ms.map(|r| r < params.synced_max_rms_ms).unwrap_or(false)
    {
        SyncStatus::Synced
    } else {
        SyncStatus::LowConfidence
    };

    Ok(SyncResult {
        offset_ms,
        drift_ppm,
        drift_measurable,
        coarse: search.coarse,
        runner_up_z: search.runner_up_z,
        fine_ncc: search.fine_ncc,
        windows: drift.windows,
        fit_rms_ms,
        end_to_end_drift_ms,
        overlap,
        status,
        suggested_drift_mode: suggested,
        compute_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Offset-only auto-sync (`couple sync --offset-only`, Phase 2). The full pipeline
/// with the drift pass is `auto_sync_tracks` (Phase 3).
pub fn offset_only_sync(
    p_cam: &PyramidReader,
    p_ext: &PyramidReader,
    params: &SyncParams,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<SyncResult> {
    let started = std::time::Instant::now();
    let dur_cam = p_cam.duration_s();
    let dur_ext = p_ext.duration_s();
    let search = offset_search(p_cam, p_ext, params, prog, cancel)?;
    prog.emit(Phase::SyncFit, 1.0);
    let result = match search {
        Err(reason) => {
            let coarse_zero = StagePeak {
                lag_ms: 0.0,
                ncc: 0.0,
                z: match reason {
                    FailReason::NoPlausiblePeak { best_z } => best_z,
                    _ => 0.0,
                },
                overlap_bins: 0,
            };
            SyncResult {
                offset_ms: 0.0,
                drift_ppm: 0.0,
                drift_measurable: false,
                coarse: coarse_zero,
                runner_up_z: 0.0,
                fine_ncc: 0.0,
                windows: Vec::new(),
                fit_rms_ms: None,
                end_to_end_drift_ms: 0.0,
                overlap: overlap_info(dur_cam, dur_ext, 0.0, 0.0),
                status: SyncStatus::Failed(reason),
                suggested_drift_mode: DriftMode::None,
                compute_ms: started.elapsed().as_secs_f64() * 1000.0,
            }
        }
        Ok(s) => {
            // Offset-only: no drift pass, so the §5.6 window criteria can't hold;
            // status reflects the fine NCC alone.
            let status = if s.fine_ncc >= params.synced_fine_ncc {
                SyncStatus::Synced
            } else {
                SyncStatus::LowConfidence
            };
            SyncResult {
                offset_ms: s.offset_stage_ms,
                drift_ppm: 0.0,
                drift_measurable: false,
                coarse: s.coarse,
                runner_up_z: s.runner_up_z,
                fine_ncc: s.fine_ncc,
                windows: Vec::new(),
                fit_rms_ms: None,
                end_to_end_drift_ms: 0.0,
                overlap: overlap_info(dur_cam, dur_ext, s.offset_stage_ms, 0.0),
                status,
                suggested_drift_mode: DriftMode::None,
                compute_ms: started.elapsed().as_secs_f64() * 1000.0,
            }
        }
    };
    Ok(result)
}

/// Overlap info on the video timeline for a final mapping (§5.6 context).
pub fn overlap_info(dur_cam: f64, dur_ext: f64, offset_ms: f64, drift_ppm: f64) -> OverlapInfo {
    let delta = 1.0 + drift_ppm * 1e-6;
    let off_s = offset_ms / 1000.0;
    let t0 = ((0.0 - off_s) / delta).max(0.0);
    let t1 = ((dur_ext - off_s) / delta).min(dur_cam);
    let seconds = (t1 - t0).max(0.0);
    OverlapInfo {
        t0,
        t1,
        seconds,
        fraction_of_video: if dur_cam > 0.0 { seconds / dur_cam } else { 0.0 },
    }
}
