//! Deterministic robust affine consensus over landmark correspondences
//! (PLAN-SPECTRAL-FINGERPRINT-MATCHING §10).
//!
//! Boundary-4 (`index`) hands this module a [`Correspondence`] slice — same spectral landmark
//! observed at `t_a_s` in A and `t_b_s` in B, with a distinctiveness `weight` — plus the
//! candidate overlap in seconds. This module fits the affine time model
//!
//! ```text
//! t_b = intercept_s + scale · t_a        (§10.1)
//! lag_ms   = intercept_s · 1000
//! drift_ppm = (scale − 1) · 1e6
//! ```
//!
//! and returns the WINNING hypothesis, its consensus statistics, the `score_margin` uniqueness
//! measure, and an INITIAL `Match`/`Ambiguous`/`NoMatch` affine classification (§10.6). It makes
//! NO `PairEvidence` (boundary 6), calls NO existing aligner / `verify_pair` (the §10.4 Huber IRLS
//! is this consensus's OWN refinement over the correspondences — boundary 7 is the §11 local
//! refinement), and touches nothing in editor-sync / audition / session / export.
//!
//! ## Determinism (§3 invariant 2, §20)
//!
//! Every byte of the output is a pure function of the (correspondence multiset, overlap, params):
//!
//! - Hypotheses come from a SMOOTHED offset histogram (scale-1) and a fixed van-der-Corput
//!   low-discrepancy index sequence (two-point) — NO RNG, NO hash-iteration order, NO wall clock
//!   (§10.2). The one `HashMap` (histogram binning) only COUNTS; the peaks are read back from a
//!   sorted `Vec`, so map order never reaches a hypothesis.
//! - Scoring is a closed-form pass over the slice; the winner is chosen by the EXACT 7-level
//!   tie-break ladder of §10.3, every level a `total_cmp` / integer compare (no float-eq ambiguity).
//! - Refinement runs a FIXED three iterations of Huber IRLS (§10.4) — never a convergence-based
//!   variable count — with the weighted-time-variance floor (`1e-12`) guarding the slope division so
//!   no NaN/Infinity can be produced.
//! - Sorting that the output depends on is total-order; input REORDERING does not change the output
//!   bits (test 6).

use super::index::Correspondence;
use super::metadata_hint::TimeHint;
use super::params::FingerprintMatchParams;

/// A tiny positive epsilon used in the score denominators and the `score_margin` denominator
/// (§10.3 `max(inlier_fraction, eps)`, §10.5 `max(best_score, eps)`). Small enough not to perturb
/// any real score, large enough to keep every division finite (§20).
const SCORE_EPS: f64 = 1e-9;

/// Finite ceiling for temporal coverage. Coverage is `inlier_span / overlap` — meaningfully in
/// `[0, 1]` for a real overlap (the span cannot exceed the overlap), so a value above 1 only
/// arises from a degenerate/under-floored overlap. Clamping to a large FINITE ceiling keeps the
/// `sqrt(coverage)` score factor and the §10.6 coverage gate finite (no +Infinity, §20) without
/// affecting any realistic case.
const COVERAGE_CEIL: f64 = 1e6;

/// Temporal coverage `inlier_span / overlap`, clamped to `[0, COVERAGE_CEIL]` so it is always
/// finite even when `overlap` was floored to a tiny value (§20). `inlier_span ≥ 0`, `overlap > 0`.
fn temporal_coverage(inlier_span: f64, overlap_eff: f64) -> f64 {
    (inlier_span.max(0.0) / overlap_eff).clamp(0.0, COVERAGE_CEIL)
}

/// The affine time model `t_b = intercept_s + scale · t_a` (§10.1), in the plan's two reported
/// units. `scale` is dimensionless (≈ 1); `intercept_s` is seconds. A hypothesis is a candidate
/// affine map; the refined winner is also expressed here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineModel {
    /// Affine intercept, seconds: B-time when A-time is 0.
    pub intercept_s: f64,
    /// Affine scale (B-seconds per A-second), dimensionless, ≈ 1 for low drift.
    pub scale: f64,
}

impl AffineModel {
    /// Lag in milliseconds (`intercept_s · 1000`, §10.1).
    pub fn lag_ms(&self) -> f64 {
        self.intercept_s * 1000.0
    }

    /// Drift in ppm (`(scale − 1) · 1e6`, §10.1).
    pub fn drift_ppm(&self) -> f64 {
        (self.scale - 1.0) * 1_000_000.0
    }

    /// Predicted B-time for an A-time under this model.
    fn predict(&self, t_a_s: f64) -> f64 {
        self.intercept_s + self.scale * t_a_s
    }
}

/// The initial affine classification (§10.6). This is the consensus's OWN verdict over the
/// correspondences; the method-neutral `MatchClassification` and the no-evidence cases (§9.3) are
/// wired in boundary 6 — this enum stays inside the fingerprint module for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffineClass {
    /// All six §10.6 predicates hold.
    Match,
    /// A fittable hypothesis exists but the §10.6 predicate fails (commonly: a non-equivalent
    /// runner-up keeps `score_margin` below the threshold — repeated content).
    Ambiguous,
    /// No fittable hypothesis at all (too few correspondences, degenerate spread, or every
    /// hypothesis rejected by the drift bound / variance floor).
    NoMatch,
}

/// The boundary-5 consensus result: the refined winning affine map, its consensus statistics, the
/// runner-up uniqueness, and the initial affine classification. All numeric fields are FINITE
/// (§20): residual measures are `Option`, `None` when no fittable hypothesis exists, so a `Match`
/// can never be declared from an absent residual (§10.6).
#[derive(Debug, Clone, PartialEq)]
pub struct AffineConsensus {
    /// The initial §10.6 classification.
    pub classification: AffineClass,
    /// The refined winning affine map (§10.4). When `classification == NoMatch` and nothing was
    /// fittable, this is the identity-ish best-effort `intercept 0 / scale 1` placeholder and the
    /// statistics below are all zero / `None` — callers gate on `classification`, never read the
    /// map of a `NoMatch`.
    pub model: AffineModel,
    /// `lag_ms` of [`Self::model`] (`intercept_s · 1000`), surfaced for convenience/telemetry.
    pub lag_ms: f64,
    /// `drift_ppm` of [`Self::model`] (`(scale − 1)·1e6`).
    pub drift_ppm: f64,
    /// Winning hypothesis score (§10.3), finite, `0.0` when nothing was fittable.
    pub score: f64,
    /// Best non-equivalent runner-up score (§10.5), `0.0` when none exists.
    pub runner_up_score: f64,
    /// Dimensionless uniqueness margin `(best − runner_up)/max(best, eps)` ∈ `[0, 1]` (§10.5).
    pub score_margin: f64,
    /// Weighted inlier support after refinement (Σ correspondence weights of the final inliers).
    pub support: f64,
    /// UNWEIGHTED count of final inliers (the tie-break / §10.6 `min_support` lever is the weighted
    /// `support`; this count is reported for telemetry and the no-evidence wiring in boundary 6).
    pub inlier_count: u32,
    /// Total candidate correspondences fed to consensus (the slice length).
    pub candidate_correspondences: u32,
    /// Weighted inlier fraction (weighted inlier support / total weighted support), §10.3.
    pub inlier_fraction: f64,
    /// Temporal coverage = inlier A-time span / candidate overlap (§10.3), clamped finite.
    pub temporal_coverage: f64,
    /// RMS of inlier residuals to the refined line, ms. `None` when no fittable hypothesis (§10.6).
    pub residual_rms_ms: Option<f64>,
    /// Nearest-rank p95 of inlier |residual|, ms. `None` when no fittable hypothesis (§10.6).
    pub residual_p95_ms: Option<f64>,
    /// The candidate overlap in seconds this consensus was scored against (echoed for telemetry).
    pub overlap_s: f64,
    /// Minimum inlier A-time (seconds) of the refined fit — the LOW edge of the inlier-covered
    /// A-span. `None` when no fittable hypothesis. BOUNDARY 7 (§11.1) selects its local-measurement
    /// anchors across `[inlier_t_a_min, inlier_t_a_max]` so the fine refinement only probes the
    /// region the consensus actually supports.
    pub inlier_t_a_min: Option<f64>,
    /// Maximum inlier A-time (seconds) of the refined fit — the HIGH edge of the inlier-covered
    /// A-span (§11.1). `None` when no fittable hypothesis.
    pub inlier_t_a_max: Option<f64>,
}

/// One scored hypothesis: its affine map and the §10.3 statistics, used both to pick the winner
/// (tie-break ladder) and to find the §10.5 runner-up.
#[derive(Debug, Clone, Copy)]
struct ScoredHypothesis {
    model: AffineModel,
    /// §10.3 score (finite).
    score: f64,
    /// UNWEIGHTED inlier count (tie-break level 2).
    unweighted_support: u32,
    /// Temporal coverage (tie-break level 3).
    temporal_coverage: f64,
    /// p95 inlier residual, ms (tie-break level 4).
    residual_p95_ms: f64,
    /// Whether this hypothesis is CONSISTENT with the (orderable) metadata hint (§12.3b): its lag is
    /// within the hint's tolerance and its scale is ≈ 1 (the hint is scale-1). Used ONLY as a
    /// tie-break rung AFTER the four audio-quality rungs (score/support/coverage/p95) and BEFORE the
    /// purely-geometric lag/drift rungs — so it breaks an EXACT audio tie toward the metadata-
    /// consistent candidate, yet a strictly better audio score always wins (it never reaches this
    /// rung). `false` for every hypothesis when no orderable hint is present ⇒ the rung is a no-op
    /// and the legacy ladder is byte-for-byte unchanged.
    metadata_consistent: bool,
}

/// Run the deterministic robust affine consensus over `correspondences` against `overlap_s`
/// (§10), with NO metadata hint. `correspondences` need NOT be pre-sorted — the output is
/// input-order-invariant (test 6); in practice boundary 4 already sorts them by `(t_a_s, t_b_s)`.
/// `overlap_s` is the candidate overlap (the temporal-coverage denominator); a
/// non-positive/non-finite overlap is floored so no division produces NaN/Infinity (§20).
///
/// Returns the [`AffineConsensus`]: winning refined map, statistics, `score_margin`, and the
/// §10.6 initial classification. With too few correspondences, a degenerate time spread, or every
/// hypothesis rejected, the classification is [`AffineClass::NoMatch`] with zero/`None`
/// statistics (never a fabricated zero-RMS match — §20, test 8).
///
/// This is the thin no-hint entry point: `consensus(cs, ov, p) == consensus_with_hint(cs, ov, p,
/// None)`, byte-for-byte. The pure-consensus call sites and §10.7 gates use it unchanged.
pub fn consensus(
    correspondences: &[Correspondence],
    overlap_s: f64,
    params: &FingerprintMatchParams,
) -> AffineConsensus {
    consensus_with_hint(correspondences, overlap_s, params, None)
}

/// Run the deterministic robust affine consensus (§10) with an OPTIONAL metadata [`TimeHint`]
/// (§12.3). The hint is ADVISORY and NON-VETOING:
///
/// - a RANKABLE hint ([`HintReliability::can_rank`](super::metadata_hint::HintReliability::can_rank))
///   seeds ONE additional scale-1 hypothesis at the hint's lag into the SAME §10.2 candidate list,
///   scored by the SAME §10.3 audio formula — so it can only WIN by earning audio inliers (§12.3a);
/// - an ORDERABLE hint
///   ([`HintReliability::can_order`](super::metadata_hint::HintReliability::can_order)) marks every
///   hypothesis whose lag sits within the hint tolerance (and scale ≈ 1) as `metadata_consistent`,
///   which the winner tie-break consults ONLY to break an EXACT audio-score tie (§12.3b).
///
/// Metadata NEVER restricts the landmark lookup / correspondence set (that is fixed before this call
/// — §20), NEVER lowers a §10.6 gate, and NEVER overrides a better audio hypothesis: the hypothesis
/// set, scoring, refinement, runner-up, and classification are otherwise IDENTICAL to the no-hint
/// path. With `hint == None` (or a non-rankable/non-orderable hint) the result is byte-for-byte the
/// legacy [`consensus`] output (the seeded hypothesis is absent and `metadata_consistent` is `false`
/// everywhere, so its tie-break rung is inert).
pub fn consensus_with_hint(
    correspondences: &[Correspondence],
    overlap_s: f64,
    params: &FingerprintMatchParams,
    hint: Option<&TimeHint>,
) -> AffineConsensus {
    let candidate_correspondences = correspondences.len().min(u32::MAX as usize) as u32;
    // Overlap floor: temporal coverage divides by this, so it must be strictly positive & finite.
    // 1e-9 (not f64::MIN_POSITIVE) so that even a large inlier span / overlap stays a FINITE ratio
    // (a subnormal divisor would overflow to +Infinity); coverage is then clamped finite below.
    let overlap_eff = if overlap_s.is_finite() && overlap_s > 0.0 {
        overlap_s
    } else {
        1e-9
    };

    // A consensus needs at least two points to anchor a line; with fewer, there is no fittable
    // hypothesis at all (§10.4 weighted LS needs spread). Return an explicit NoMatch.
    if correspondences.len() < 2 {
        return no_match(candidate_correspondences, overlap_s);
    }

    // INPUT-ORDER INVARIANCE (§3 invariant 2, test 6). Float addition is NOT associative, so every
    // downstream sum (total weight, histogram bins, the weighted-fit accumulators) would otherwise
    // depend on the slice's order and the output bits would change under a reordering. Canonicalize
    // ONCE here — sort a local copy by (t_a_s, t_b_s, weight) via total order — and drive ALL of
    // hypothesis generation, scoring, and refinement from this canonical slice. Two reorderings of
    // the same multiset now produce bit-identical sums, hence bit-identical output.
    let mut cs: Vec<Correspondence> = correspondences.to_vec();
    cs.sort_by(|a, b| {
        a.t_a_s
            .total_cmp(&b.t_a_s)
            .then(a.t_b_s.total_cmp(&b.t_b_s))
            .then(a.weight.total_cmp(&b.weight))
    });
    let correspondences: &[Correspondence] = &cs;

    // Total weighted support over ALL correspondences — the §10.3 inlier-fraction denominator. A
    // weight is in (0, 1] (boundary 4), so this is finite and > 0 for a non-empty slice; guard
    // anyway so the fraction never divides by zero. Summed in canonical order (above).
    let total_weight: f64 = correspondences.iter().map(|c| c.weight).sum();
    let total_weight = if total_weight > 0.0 { total_weight } else { SCORE_EPS };

    // §10.2 — generate the deterministic hypothesis set (scale-1 histogram peaks + van-der-Corput
    // two-point pairs), already drift-/scale-validated and deduplicated, capped at max_hypotheses.
    // §12.3a: a RANKABLE metadata hint seeds ONE additional scale-1 hypothesis at its lag (added
    // FIRST so it survives the max_hypotheses cap; it is deduped against the audio hypotheses on the
    // same 10 ms × 10 ppm grid, so it never double-counts an audio line at the same offset).
    let rank_hint = hint.filter(|h| h.reliability.can_rank() && h.lag_ms.is_finite());
    let hypotheses = generate_hypotheses(correspondences, params, rank_hint);
    if hypotheses.is_empty() {
        return no_match(candidate_correspondences, overlap_s);
    }

    // §12.3b: an ORDERABLE hint marks hypotheses CONSISTENT with the metadata lag (lag within the
    // hint tolerance AND scale ≈ 1, since the hint is scale-1). This flag is consulted ONLY by the
    // tie-break ladder, AFTER the four audio-quality rungs — it can never beat a better audio score.
    let order_hint = hint.filter(|h| h.reliability.can_order() && h.lag_ms.is_finite());

    // §10.3 — score every hypothesis. Keep them all (we need the full set for the §10.5 runner-up).
    let mut scored: Vec<ScoredHypothesis> = Vec::with_capacity(hypotheses.len());
    for model in &hypotheses {
        let mut s = score_hypothesis(*model, correspondences, total_weight, overlap_eff, params);
        s.metadata_consistent = order_hint.is_some_and(|h| metadata_consistent(*model, h));
        scored.push(s);
    }

    // Pick the winner by the EXACT §10.3 7-level tie-break ladder.
    let best_idx = pick_winner(&scored);
    let best = scored[best_idx];

    // §10.4 — refine the winner: weighted LS to its inliers, then 3 fixed Huber-IRLS iterations.
    // None ⇒ the winner had no trustworthy refined line (degenerate spread / variance floor);
    // treat as no fittable hypothesis.
    let refined = match refine(best.model, correspondences, overlap_eff, params) {
        Some(r) => r,
        None => return no_match(candidate_correspondences, overlap_s),
    };

    // §10.5 — runner-up uniqueness over hypotheses NOT equivalent to the winner.
    let runner_up_score = best_runner_up_score(&scored, best_idx, best.model);
    let score_margin = score_margin(best.score, runner_up_score);

    // §10.6 — initial classification from the REFINED statistics + the uniqueness margin.
    let classification = classify(&refined, score_margin, params);

    AffineConsensus {
        classification,
        model: refined.model,
        lag_ms: refined.model.lag_ms(),
        drift_ppm: refined.model.drift_ppm(),
        score: best.score,
        runner_up_score,
        score_margin,
        support: refined.support,
        inlier_count: refined.inlier_count,
        candidate_correspondences,
        inlier_fraction: refined.inlier_fraction,
        temporal_coverage: refined.temporal_coverage,
        residual_rms_ms: Some(refined.residual_rms_ms),
        residual_p95_ms: Some(refined.residual_p95_ms),
        overlap_s,
        inlier_t_a_min: Some(refined.inlier_t_a_min),
        inlier_t_a_max: Some(refined.inlier_t_a_max),
    }
}

/// The explicit "no fittable hypothesis" result: classification `NoMatch`, identity-ish model, zero
/// statistics, and ABSENT residuals (so §10.6 can never read them into a Match — §20, test 8).
fn no_match(candidate_correspondences: u32, overlap_s: f64) -> AffineConsensus {
    AffineConsensus {
        classification: AffineClass::NoMatch,
        model: AffineModel { intercept_s: 0.0, scale: 1.0 },
        lag_ms: 0.0,
        drift_ppm: 0.0,
        score: 0.0,
        runner_up_score: 0.0,
        score_margin: 0.0,
        support: 0.0,
        inlier_count: 0,
        candidate_correspondences,
        inlier_fraction: 0.0,
        temporal_coverage: 0.0,
        residual_rms_ms: None,
        residual_p95_ms: None,
        overlap_s,
        inlier_t_a_min: None,
        inlier_t_a_max: None,
    }
}

// ===================================================================================================
// §10.2 — deterministic hypothesis generation
// ===================================================================================================

/// Minimum A-time separation (seconds) between the two points of a two-point hypothesis (§10.2 #3).
const MIN_TWO_POINT_SEP_S: f64 = 5.0;
/// Offset-histogram bin width (seconds): 100 ms (§10.2 #1).
const HIST_BIN_S: f64 = 0.1;
/// Number of smoothed-histogram peaks to seed scale-1 hypotheses from (§10.2 #1).
const HIST_PEAKS: usize = 16;
/// Triangular smoothing half-width in bins (±2-bin kernel, §10.2 #1).
const HIST_SMOOTH_RADIUS: i64 = 2;
/// Intercept dedup quantum: 10 ms (§10.2 #5).
const DEDUP_INTERCEPT_S: f64 = 0.010;
/// Scale dedup quantum: 10 ppm in scale units (§10.2 #5).
const DEDUP_SCALE: f64 = 10.0 / 1_000_000.0;

/// Generate the deterministic hypothesis set (§10.2): scale-1 offset-histogram peaks, then
/// van-der-Corput two-point pairs, each validated (positive scale, |drift| ≤ bound, ≥ 5 s A-time
/// separation) and deduplicated to a 10 ms × 10 ppm grid, capped at `max_hypotheses`.
///
/// `rank_hint` (§12.3a): when `Some`, ONE additional scale-1 hypothesis at the hint's lag is added
/// FIRST (so the `max_hypotheses` cap never drops it) through the SAME validation + dedup path — so
/// it is a normal candidate that competes on the §10.3 audio score and is collapsed into an audio
/// hypothesis at the same offset rather than double-counted. A non-finite or out-of-bound hint lag
/// simply fails `push_hypothesis` and contributes nothing (it can never harm the audio set).
fn generate_hypotheses(
    correspondences: &[Correspondence],
    params: &FingerprintMatchParams,
    rank_hint: Option<&TimeHint>,
) -> Vec<AffineModel> {
    let mut out: Vec<AffineModel> = Vec::new();
    // Dedup keys: quantized (intercept, scale). A `Vec<(i64,i64)>` kept sorted gives a deterministic
    // membership test without any hash-map iteration entering the output (§10.2 #5).
    let mut seen: Vec<(i64, i64)> = Vec::new();
    let max_hypotheses = params.max_hypotheses;

    // ---- §12.3a: the metadata-seeded scale-1 hypothesis (added first; same validation + dedup) ----
    if let Some(h) = rank_hint {
        let model = AffineModel { intercept_s: h.lag_ms / 1000.0, scale: 1.0 };
        push_hypothesis(model, params, &mut out, &mut seen, max_hypotheses);
    }

    // ---- §10.2 #1: scale-1 hypotheses from the smoothed offset histogram ----
    for intercept_s in histogram_offset_peaks(correspondences) {
        if out.len() >= max_hypotheses {
            return out;
        }
        let model = AffineModel { intercept_s, scale: 1.0 };
        push_hypothesis(model, params, &mut out, &mut seen, max_hypotheses);
    }

    // ---- §10.2 #2: two-point hypotheses from the van-der-Corput low-discrepancy sequence ----
    let n = correspondences.len();
    if n >= 2 {
        // Examine at most 8·max_hypotheses sequence positions (§10.2 #2). A separate dedup of the
        // INDEX pairs avoids re-deriving the same model from a repeated (i, j) (§10.2 #2).
        let max_positions = max_hypotheses.saturating_mul(8);
        let mut seen_pairs: Vec<(u32, u32)> = Vec::new();
        for k in 1..=max_positions {
            if out.len() >= max_hypotheses {
                break;
            }
            let i = (van_der_corput(k as u64, 2) * n as f64) as usize;
            let j = (van_der_corput(k as u64, 3) * n as f64) as usize;
            // floor can land exactly on n for a degenerate frac==1.0 edge; clamp into range.
            let i = i.min(n - 1);
            let j = j.min(n - 1);
            if i == j {
                continue;
            }
            // Deduplicate INDEX pairs (unordered): (i, j) and (j, i) describe the same two points.
            let key = if i < j { (i as u32, j as u32) } else { (j as u32, i as u32) };
            if let Err(pos) = seen_pairs.binary_search(&key) {
                seen_pairs.insert(pos, key);
            } else {
                continue;
            }
            let a = correspondences[i];
            let b = correspondences[j];
            // §10.2 #3: reject point pairs < 5 s apart in A.
            if (a.t_a_s - b.t_a_s).abs() < MIN_TWO_POINT_SEP_S {
                continue;
            }
            // Two-point affine fit: scale = Δt_b/Δt_a; intercept = t_b − scale·t_a.
            let dt_a = b.t_a_s - a.t_a_s;
            if dt_a.abs() < f64::MIN_POSITIVE {
                continue; // guarded by the 5 s rule already, but never divide by ~0 (§20).
            }
            let scale = (b.t_b_s - a.t_b_s) / dt_a;
            let intercept_s = a.t_b_s - scale * a.t_a_s;
            let model = AffineModel { intercept_s, scale };
            push_hypothesis(model, params, &mut out, &mut seen, max_hypotheses);
        }
    }

    out
}

/// Validate a hypothesis (§10.2 #4 positive scale + |drift| ≤ bound; finite fields) and, if it is
/// new on the 10 ms × 10 ppm dedup grid (§10.2 #5), push it. Stops contributing once `out` reaches
/// `max_hypotheses` (§10.2 #6 — the caller also breaks, this is belt-and-suspenders).
fn push_hypothesis(
    model: AffineModel,
    params: &FingerprintMatchParams,
    out: &mut Vec<AffineModel>,
    seen: &mut Vec<(i64, i64)>,
    max_hypotheses: usize,
) {
    if out.len() >= max_hypotheses {
        return;
    }
    if !model.intercept_s.is_finite() || !model.scale.is_finite() {
        return;
    }
    // §10.2 #4: reject non-positive scale or out-of-bound drift.
    if model.scale <= 0.0 {
        return;
    }
    if model.drift_ppm().abs() > params.max_abs_drift_ppm {
        return;
    }
    // §10.2 #5: dedup on a 10 ms intercept × 10 ppm scale grid. Quantize by ROUND so two hypotheses
    // within the quantum collapse; the key is integer so membership is exact and order-stable.
    let key = (
        quantize(model.intercept_s, DEDUP_INTERCEPT_S),
        quantize(model.scale, DEDUP_SCALE),
    );
    match seen.binary_search(&key) {
        Ok(_) => {} // already present
        Err(pos) => {
            seen.insert(pos, key);
            out.push(model);
        }
    }
}

/// Round `v` to the nearest multiple of `q` and return the integer multiple index (the dedup grid
/// cell, §10.2 #5). `q > 0`; finite `v` ⇒ finite key.
fn quantize(v: f64, q: f64) -> i64 {
    (v / q).round() as i64
}

/// The intercepts of the up-to-16 highest SMOOTHED offset-histogram peaks (§10.2 #1). The histogram
/// is over `t_b − t_a` (the scale-1 offset) in 100 ms bins, smoothed with a fixed ±2-bin triangular
/// kernel; peaks are the highest smoothed bins, each mapped back to its bin-center offset (seconds).
///
/// Determinism: bins are accumulated in a `HashMap<i64, f64>` keyed by the integer bin index, but
/// the peaks are read from a SORTED `Vec` (by smoothed weight desc, then bin index) — map iteration
/// order never reaches the result.
fn histogram_offset_peaks(correspondences: &[Correspondence]) -> Vec<f64> {
    use std::collections::HashMap;

    // Accumulate WEIGHTED counts per 100 ms offset bin. Weighting by correspondence weight makes a
    // distinctive cluster outweigh a dense uninformative one — consistent with §10.3's weighting.
    let mut raw: HashMap<i64, f64> = HashMap::new();
    for c in correspondences {
        let offset = c.t_b_s - c.t_a_s;
        if !offset.is_finite() {
            continue;
        }
        let bin = (offset / HIST_BIN_S).floor() as i64;
        *raw.entry(bin).or_insert(0.0) += c.weight;
    }
    if raw.is_empty() {
        return Vec::new();
    }

    // Materialize the occupied bins in SORTED bin order (so smoothing and peak selection are
    // independent of map iteration). The triangular ±2-bin kernel has weights [1,2,3,2,1].
    let mut bins: Vec<(i64, f64)> = raw.into_iter().collect();
    bins.sort_by_key(|(b, _)| *b);
    // Fast lookup of a raw bin's weight by index for the convolution (binary search on the sorted
    // bins). Smoothing only needs the occupied bins ± the radius; empty bins contribute 0.
    let smoothed: Vec<(i64, f64)> = bins
        .iter()
        .map(|&(b, _)| {
            let mut acc = 0.0;
            for d in -HIST_SMOOTH_RADIUS..=HIST_SMOOTH_RADIUS {
                let w = (HIST_SMOOTH_RADIUS + 1 - d.abs()) as f64; // 1,2,3,2,1
                if let Ok(pos) = bins.binary_search_by_key(&(b + d), |(bb, _)| *bb) {
                    acc += w * bins[pos].1;
                }
            }
            (b, acc)
        })
        .collect();

    // The HIST_PEAKS highest smoothed bins (by weight desc, then lower bin index for determinism),
    // each mapped to its bin-CENTER offset in seconds.
    let mut ranked = smoothed;
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(HIST_PEAKS);
    ranked
        .into_iter()
        .map(|(bin, _)| (bin as f64 + 0.5) * HIST_BIN_S)
        .collect()
}

/// The van der Corput low-discrepancy value of index `k` in the given `base` (§10.2 #2). Returns a
/// value in `[0, 1)`: reflect `k`'s base-`base` digits about the radix point. Deterministic and
/// allocation-free; `k ≥ 1` in our use.
fn van_der_corput(mut k: u64, base: u64) -> f64 {
    debug_assert!(base >= 2);
    let mut result = 0.0;
    let mut denom = 1.0;
    while k > 0 {
        denom *= base as f64;
        result += (k % base) as f64 / denom;
        k /= base;
    }
    result
}

// ===================================================================================================
// §10.3 — hypothesis scoring + tie-break
// ===================================================================================================

/// Score one hypothesis against the correspondences (§10.3). Computes inlier weighted support,
/// temporal coverage (inlier A-span / overlap), inlier fraction (weighted inlier / total weighted),
/// the inlier p95 residual, and the composite score; returns them packed for the tie-break ladder.
fn score_hypothesis(
    model: AffineModel,
    correspondences: &[Correspondence],
    total_weight: f64,
    overlap_eff: f64,
    params: &FingerprintMatchParams,
) -> ScoredHypothesis {
    let tol = params.inlier_tolerance_ms;
    let mut weighted_support = 0.0;
    let mut unweighted_support: u32 = 0;
    let mut min_t_a = f64::INFINITY;
    let mut max_t_a = f64::NEG_INFINITY;
    // Inlier |residual| values (ms) for the p95.
    let mut inlier_res: Vec<f64> = Vec::new();

    for c in correspondences {
        let residual_ms = 1000.0 * (c.t_b_s - model.predict(c.t_a_s)).abs();
        if residual_ms <= tol {
            weighted_support += c.weight;
            unweighted_support += 1;
            if c.t_a_s < min_t_a {
                min_t_a = c.t_a_s;
            }
            if c.t_a_s > max_t_a {
                max_t_a = c.t_a_s;
            }
            inlier_res.push(residual_ms);
        }
    }

    if unweighted_support == 0 {
        // No inliers ⇒ a zero score (it cannot win against any hypothesis that has support; if NONE
        // has support the winner is still well-defined and refinement will reject it as NoMatch).
        return ScoredHypothesis {
            model,
            score: 0.0,
            unweighted_support: 0,
            temporal_coverage: 0.0,
            residual_p95_ms: f64::INFINITY,
            // Set by the caller (§12.3b) from the orderable hint; default false (the no-hint path
            // leaves it false everywhere, keeping the legacy tie-break inert).
            metadata_consistent: false,
        };
    }

    // Temporal coverage = inlier A-span / candidate overlap (§10.3), clamped finite (§20).
    let inlier_span = max_t_a - min_t_a;
    let temporal_coverage = temporal_coverage(inlier_span, overlap_eff);
    // Inlier fraction = weighted inlier support / total weighted support (§10.3). total_weight > 0.
    let inlier_fraction = weighted_support / total_weight;
    let residual_p95_ms = nearest_rank_p95(&mut inlier_res);

    // Score = weighted_support · sqrt(max(coverage, 0)) · max(inlier_fraction, eps)
    //         / (1 + residual_p95_ms / inlier_tolerance_ms)              (§10.3)
    // Every factor is finite: coverage ≥ 0 ⇒ sqrt finite; tol guarded below; p95 finite (≥1 inlier).
    let coverage_term = temporal_coverage.max(0.0).sqrt();
    let fraction_term = inlier_fraction.max(SCORE_EPS);
    let tol_denom = if tol > 0.0 { tol } else { SCORE_EPS };
    let denom = 1.0 + residual_p95_ms / tol_denom;
    let score = weighted_support * coverage_term * fraction_term / denom;

    ScoredHypothesis {
        model,
        score,
        unweighted_support,
        temporal_coverage,
        residual_p95_ms,
        // Set by the caller (§12.3b); default false so the no-hint path is byte-identical.
        metadata_consistent: false,
    }
}

/// Whether `model` is CONSISTENT with the metadata `hint` (§12.3b): its lag is within the hint's
/// tolerance AND its scale is ≈ 1 (the hint is a scale-1, drift-free prior). Pure compare; used only
/// to flag a hypothesis for the EXACT-tie ordering rung, never for any gate.
fn metadata_consistent(model: AffineModel, hint: &TimeHint) -> bool {
    let tol = if hint.tolerance_ms.is_finite() && hint.tolerance_ms > 0.0 {
        hint.tolerance_ms
    } else {
        0.0
    };
    // |drift| ≤ 50 ppm reuses the §10.5 equivalence drift window: the hint is scale-1, so a
    // hypothesis with materially different drift is a different time model, not "the metadata line".
    (model.lag_ms() - hint.lag_ms).abs() <= tol && model.drift_ppm().abs() < 50.0
}

/// Choose the winning hypothesis index by the EXACT §10.3 7-level tie-break ladder:
/// (1) higher score, (2) higher unweighted support, (3) higher temporal coverage, (4) lower p95
/// residual, (5) smaller |drift|, (6) smaller |lag|, (7) lower lag. Every comparison is total-order;
/// the final `index` tie-break makes the choice fully deterministic even for identical hypotheses.
fn pick_winner(scored: &[ScoredHypothesis]) -> usize {
    let mut best = 0usize;
    for i in 1..scored.len() {
        if hypothesis_better(&scored[i], &scored[best]) {
            best = i;
        }
    }
    best
}

/// `true` when `a` should rank ABOVE `b` under the §10.3 ladder. The ladder is a strict ordering;
/// the caller only swaps on strictly-better, so identical scores keep the earlier (lower-index)
/// hypothesis — a deterministic final tie-break.
///
/// §12.3b inserts ONE rung — metadata consistency — AFTER the four AUDIO-QUALITY rungs (score,
/// support, coverage, p95) and BEFORE the purely-geometric lag/drift rungs. Consequences:
///
/// - it can ONLY decide a case where the four audio rungs are ALL exactly equal — an EXACT
///   audio-score tie — so a strictly better audio hypothesis ALWAYS wins before this rung is reached
///   (it never overrides a better audio line, §12.3 "may not override a better audio hypothesis");
/// - with no orderable hint, `metadata_consistent` is `false` for every hypothesis, the rung is a
///   no-op, and the ladder is byte-for-byte the legacy §10.3 ladder.
fn hypothesis_better(a: &ScoredHypothesis, b: &ScoredHypothesis) -> bool {
    // 1) higher score
    match a.score.total_cmp(&b.score) {
        std::cmp::Ordering::Greater => return true,
        std::cmp::Ordering::Less => return false,
        std::cmp::Ordering::Equal => {}
    }
    // 2) higher unweighted support
    match a.unweighted_support.cmp(&b.unweighted_support) {
        std::cmp::Ordering::Greater => return true,
        std::cmp::Ordering::Less => return false,
        std::cmp::Ordering::Equal => {}
    }
    // 3) higher temporal coverage
    match a.temporal_coverage.total_cmp(&b.temporal_coverage) {
        std::cmp::Ordering::Greater => return true,
        std::cmp::Ordering::Less => return false,
        std::cmp::Ordering::Equal => {}
    }
    // 4) lower p95 residual
    match a.residual_p95_ms.total_cmp(&b.residual_p95_ms) {
        std::cmp::Ordering::Less => return true,
        std::cmp::Ordering::Greater => return false,
        std::cmp::Ordering::Equal => {}
    }
    // 4.5) §12.3b: ONLY on an EXACT audio tie (rungs 1–4 all equal), prefer the metadata-consistent
    // candidate. This breaks the tie toward the metadata line without ever beating a better audio
    // score. Inert (both false) when there is no orderable hint.
    match a.metadata_consistent.cmp(&b.metadata_consistent) {
        std::cmp::Ordering::Greater => return true, // a consistent, b not
        std::cmp::Ordering::Less => return false,   // b consistent, a not
        std::cmp::Ordering::Equal => {}
    }
    // 5) smaller |drift|
    match a.model.drift_ppm().abs().total_cmp(&b.model.drift_ppm().abs()) {
        std::cmp::Ordering::Less => return true,
        std::cmp::Ordering::Greater => return false,
        std::cmp::Ordering::Equal => {}
    }
    // 6) smaller |lag|
    match a.model.lag_ms().abs().total_cmp(&b.model.lag_ms().abs()) {
        std::cmp::Ordering::Less => return true,
        std::cmp::Ordering::Greater => return false,
        std::cmp::Ordering::Equal => {}
    }
    // 7) lower lag (signed)
    matches!(
        a.model.lag_ms().total_cmp(&b.model.lag_ms()),
        std::cmp::Ordering::Less
    )
}

// ===================================================================================================
// §10.5 — runner-up uniqueness
// ===================================================================================================

/// Two affine maps are EQUIVALENT (§10.5) when `|lag diff| < 250 ms` AND `|drift diff| < 50 ppm`.
fn equivalent(a: AffineModel, b: AffineModel) -> bool {
    (a.lag_ms() - b.lag_ms()).abs() < 250.0 && (a.drift_ppm() - b.drift_ppm()).abs() < 50.0
}

/// The best score among hypotheses NOT equivalent to the winner (§10.5), or `0.0` if none exists.
/// (Repeated content yields a strong second cluster here, depressing the margin → Ambiguous.)
fn best_runner_up_score(
    scored: &[ScoredHypothesis],
    best_idx: usize,
    winner: AffineModel,
) -> f64 {
    let mut runner = 0.0_f64;
    for (i, s) in scored.iter().enumerate() {
        if i == best_idx {
            continue;
        }
        if equivalent(s.model, winner) {
            continue;
        }
        if s.score > runner {
            runner = s.score;
        }
    }
    runner
}

/// `score_margin = (best − runner_up) / max(best, eps)` ∈ `[0, 1]` (§10.5): a dimensionless
/// uniqueness measure, NOT an absolute difference. `runner_up == 0` (no non-equivalent rival) ⇒
/// margin 1. Clamped to `[0, 1]` so a finite, well-defined value always reaches the predicate.
fn score_margin(best: f64, runner_up: f64) -> f64 {
    let denom = best.max(SCORE_EPS);
    ((best - runner_up) / denom).clamp(0.0, 1.0)
}

// ===================================================================================================
// §10.4 — Huber IRLS refinement
// ===================================================================================================

/// Huber delta as a fraction of the inlier tolerance (`inlier_tolerance_ms / 2`, §10.4 #3).
const HUBER_DELTA_FRAC: f64 = 0.5;
/// Fixed number of IRLS iterations (§10.4 #2 — NO convergence-based variable count).
const IRLS_ITERS: usize = 3;
/// Weighted time-variance floor below which the slope division is rejected (§10.4 — no NaN/Inf).
const TIME_VAR_FLOOR: f64 = 1e-12;

/// The refined statistics of the winning hypothesis after §10.4 weighted-LS + 3× Huber IRLS.
struct Refined {
    model: AffineModel,
    support: f64,
    inlier_count: u32,
    inlier_fraction: f64,
    temporal_coverage: f64,
    residual_rms_ms: f64,
    residual_p95_ms: f64,
    /// Min/max inlier A-time (seconds) — the inlier-covered A-span boundary 7 (§11.1) probes.
    inlier_t_a_min: f64,
    inlier_t_a_max: f64,
}

/// Refine the best hypothesis (§10.4): seed inliers at the FULL tolerance around `seed`, fit a
/// weighted line to them, then run THREE fixed Huber-IRLS iterations — each recomputing Huber
/// weights, refitting, and re-selecting inliers within the FULL tolerance — enforcing the drift
/// bound throughout. Returns `None` when no trustworthy line exists (fewer than 2 inliers, or the
/// weighted time variance falls below `1e-12`, §10.4 / §20). Final statistics are computed from the
/// final inlier set against the final line.
fn refine(
    seed: AffineModel,
    correspondences: &[Correspondence],
    overlap_eff: f64,
    params: &FingerprintMatchParams,
) -> Option<Refined> {
    let tol = params.inlier_tolerance_ms;
    let delta = (tol * HUBER_DELTA_FRAC).max(SCORE_EPS); // > 0 so delta/|r| is finite.

    // Total weighted support over ALL correspondences (the inlier-fraction denominator), guarded.
    let total_weight: f64 = correspondences.iter().map(|c| c.weight).sum::<f64>().max(SCORE_EPS);

    // Initial inliers: full tolerance around the seed model. Indices into `correspondences`.
    let mut inliers: Vec<usize> = correspondences
        .iter()
        .enumerate()
        .filter(|(_, c)| 1000.0 * (c.t_b_s - seed.predict(c.t_a_s)).abs() <= tol)
        .map(|(i, _)| i)
        .collect();
    if inliers.len() < 2 {
        return None;
    }

    // §10.4 #1: weighted LS to the inliers (correspondence weights only) to start the line.
    let mut model = weighted_line_fit(
        inliers.iter().map(|&i| {
            let c = correspondences[i];
            (c.t_a_s, c.t_b_s, c.weight)
        }),
        params,
    )?;

    // §10.4 #2: THREE fixed Huber-IRLS iterations.
    for _ in 0..IRLS_ITERS {
        // final_weight = correspondence_weight · huber_weight, with
        // huber_weight = 1 if |r| ≤ delta else delta/|r|              (§10.4)
        let points = inliers.iter().map(|&i| {
            let c = correspondences[i];
            let r = 1000.0 * (c.t_b_s - model.predict(c.t_a_s)).abs();
            let huber = if r <= delta { 1.0 } else { delta / r };
            (c.t_a_s, c.t_b_s, c.weight * huber)
        });
        model = weighted_line_fit(points, params)?;

        // §10.4 #4: re-select inliers within the FULL tolerance after the refit.
        inliers = correspondences
            .iter()
            .enumerate()
            .filter(|(_, c)| 1000.0 * (c.t_b_s - model.predict(c.t_a_s)).abs() <= tol)
            .map(|(i, _)| i)
            .collect();
        if inliers.len() < 2 {
            return None;
        }
    }

    // §10.4 #6: final statistics from the FINAL inliers against the FINAL line. The drift bound was
    // enforced inside every fit; assert it once more defensively (a refit cannot exceed it).
    if model.drift_ppm().abs() > params.max_abs_drift_ppm {
        return None;
    }

    let mut weighted_support = 0.0;
    let mut min_t_a = f64::INFINITY;
    let mut max_t_a = f64::NEG_INFINITY;
    let mut abs_res: Vec<f64> = Vec::with_capacity(inliers.len());
    let mut sq_sum = 0.0;
    for &i in &inliers {
        let c = correspondences[i];
        let r = 1000.0 * (c.t_b_s - model.predict(c.t_a_s));
        weighted_support += c.weight;
        if c.t_a_s < min_t_a {
            min_t_a = c.t_a_s;
        }
        if c.t_a_s > max_t_a {
            max_t_a = c.t_a_s;
        }
        abs_res.push(r.abs());
        sq_sum += r * r;
    }
    let inlier_count = inliers.len() as u32;
    let residual_rms_ms = (sq_sum / inliers.len() as f64).sqrt();
    let residual_p95_ms = nearest_rank_p95(&mut abs_res);
    let inlier_span = max_t_a - min_t_a;
    let temporal_coverage = temporal_coverage(inlier_span, overlap_eff);
    let inlier_fraction = weighted_support / total_weight;

    Some(Refined {
        model,
        support: weighted_support,
        inlier_count,
        inlier_fraction,
        temporal_coverage,
        residual_rms_ms,
        residual_p95_ms,
        inlier_t_a_min: min_t_a,
        inlier_t_a_max: max_t_a,
    })
}

/// Weighted least-squares line fit `t_b = intercept + scale·t_a` over `(t_a, t_b, weight)` points,
/// computing weighted means / covariance / slope / intercept directly in f64 (§10.4). Enforces the
/// drift bound by CLAMPING `scale` to the `±max_abs_drift_ppm` band (so a refit never escapes the
/// physical bound). Returns `None` when the weighted time variance is below `1e-12` (degenerate
/// spread — no slope is trustworthy, and dividing by it would yield NaN/Infinity, §10.4 / §20), or
/// when the total weight is non-positive.
fn weighted_line_fit(
    points: impl Iterator<Item = (f64, f64, f64)>,
    params: &FingerprintMatchParams,
) -> Option<AffineModel> {
    let mut sw = 0.0; // Σ w
    let mut swa = 0.0; // Σ w·t_a
    let mut swb = 0.0; // Σ w·t_b
    let mut swaa = 0.0; // Σ w·t_a²
    let mut swab = 0.0; // Σ w·t_a·t_b
    let mut count = 0usize;
    for (ta, tb, w) in points {
        sw += w;
        swa += w * ta;
        swb += w * tb;
        swaa += w * ta * ta;
        swab += w * ta * tb;
        count += 1;
    }
    if count < 2 || sw <= 0.0 {
        return None;
    }
    let mean_a = swa / sw;
    let mean_b = swb / sw;
    // Weighted variance of t_a and weighted covariance of (t_a, t_b), via the centered forms.
    let var_a = swaa / sw - mean_a * mean_a;
    let cov_ab = swab / sw - mean_a * mean_b;
    // §10.4 / §20: reject a weighted time variance below the floor (or non-finite) — no division by
    // ~0 / NaN. Written as an explicit finite-and-above-floor test so a NaN `var_a` also fails here.
    if !var_a.is_finite() || var_a <= TIME_VAR_FLOOR {
        return None;
    }
    let mut scale = cov_ab / var_a;
    // Enforce the drift bound by clamping scale into the ±max_abs_drift_ppm band (scale units).
    let max_scale_dev = params.max_abs_drift_ppm / 1_000_000.0;
    let lo = 1.0 - max_scale_dev;
    let hi = 1.0 + max_scale_dev;
    if scale < lo {
        scale = lo;
    } else if scale > hi {
        scale = hi;
    }
    let intercept_s = mean_b - scale * mean_a;
    if !intercept_s.is_finite() || !scale.is_finite() {
        return None;
    }
    Some(AffineModel { intercept_s, scale })
}

// ===================================================================================================
// §10.6 — initial classification
// ===================================================================================================

/// The initial affine classification (§10.6). `Match` ONLY when ALL gates hold:
/// `support ≥ min_support`, `identity_ok` (the density-robust OR below),
/// `temporal_coverage ≥ min_temporal_coverage`, `residual_p95_ms ≤ max_residual_p95_ms`,
/// `score_margin ≥ min_score_margin`, `|drift_ppm| ≤ max_abs_drift_ppm`. A fittable-but-failing
/// hypothesis is `Ambiguous` (a real affine line exists; it just is not confidently unique/clean
/// enough). The residual is always present here (we have a refined fit), so the §10.6 "absent
/// residual ⇒ cannot be Match" rule is structurally enforced by [`consensus`] returning `NoMatch`
/// (with `None` residuals) whenever no line was fittable.
///
/// ## The density-robust §10.6 identity gate (the §10.6-sanctioned plan revision)
///
/// The single inlier-FRACTION predicate is density-fragile: on long, dense, REAL orchestral audio a
/// true line's inlier FRACTION is depressed by the many spurious correspondences dense audio yields,
/// even though the inlier COUNT, coverage, p95, drift, support and margin are all strong and stable.
/// §10.6 explicitly sanctions replacing this with a density function if the corpus proves it
/// necessary (the boundary-12 corpus did). The identity gate is now an OR of two complementary paths:
///
/// ```text
/// identity_ok = (inlier_fraction >= min_inlier_fraction)
///            OR (inlier_count >= min_abs_inlier_count AND temporal_coverage >= min_abs_coverage)
/// ```
///
/// The ABSOLUTE path preserves the 0-confident-false-match safety (§20): it still requires a refined
/// AFFINE CONSENSUS (this `classify` only runs on a fitted line — no classification from raw hash
/// count, §20), a HIGH inlier COUNT (`min_abs_inlier_count` ≫ any coincidence — the no-two-point-zero
/// -RMS guard in [`consensus`] still holds), HIGH temporal coverage (`min_abs_coverage` blocks a
/// localized coincidence / duplicate-tower), and the UNCHANGED p95 / drift / support / margin gates
/// (repeated content still depresses the margin ⇒ Ambiguous). At profile 0 `min_abs_inlier_count =
/// u32::MAX`, so the absolute path can never fire and the OR collapses to the fraction path —
/// byte-for-byte the boundary-12 shadow behaviour.
fn classify(refined: &Refined, score_margin: f64, params: &FingerprintMatchParams) -> AffineClass {
    let support_ok = refined.support >= params.min_support as f64;
    // §10.6 density-robust identity gate: the FRACTION path OR the density-robust ABSOLUTE path.
    let fraction_ok = refined.inlier_fraction >= params.min_inlier_fraction;
    let absolute_ok = refined.inlier_count >= params.min_abs_inlier_count
        && refined.temporal_coverage >= params.min_abs_coverage;
    let identity_ok = fraction_ok || absolute_ok;
    let coverage_ok = refined.temporal_coverage >= params.min_temporal_coverage;
    let p95_ok = refined.residual_p95_ms <= params.max_residual_p95_ms;
    let margin_ok = score_margin >= params.min_score_margin;
    let drift_ok = refined.model.drift_ppm().abs() <= params.max_abs_drift_ppm;

    if support_ok && identity_ok && coverage_ok && p95_ok && margin_ok && drift_ok {
        AffineClass::Match
    } else {
        AffineClass::Ambiguous
    }
}

// ===================================================================================================
// shared numeric helpers
// ===================================================================================================

/// Nearest-rank p95 of a slice of non-negative |residual| values (ms). Sorts in place by total
/// order (deterministic); `rank = ceil(0.95·n)` 1-based → 0-based index (so n ≤ 20 ⇒ the max),
/// matching the project-wide convention in `robust_fit::p95_abs_residual_ms`. Empty ⇒ `+∞` (callers
/// only pass a non-empty inlier set; a `+∞` here would simply fail the p95 gate, never a Match).
fn nearest_rank_p95(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return f64::INFINITY;
    }
    values.sort_by(f64::total_cmp);
    let rank = ((0.95 * values.len() as f64).ceil() as usize).max(1);
    values[rank - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> FingerprintMatchParams {
        FingerprintMatchParams::default()
    }

    fn corr(t_a_s: f64, t_b_s: f64, weight: f64) -> Correspondence {
        Correspondence { t_a_s, t_b_s, weight }
    }

    /// Deterministic serialization of a consensus's load-bearing numeric fields, for the
    /// input-order-invariance gate (test 6). Every f64 by its exact bit pattern.
    fn serialize_consensus(c: &AffineConsensus) -> Vec<u8> {
        let mut out = Vec::new();
        let mut push_f = |x: f64| out.extend_from_slice(&x.to_bits().to_le_bytes());
        push_f(c.model.intercept_s);
        push_f(c.model.scale);
        push_f(c.lag_ms);
        push_f(c.drift_ppm);
        push_f(c.score);
        push_f(c.runner_up_score);
        push_f(c.score_margin);
        push_f(c.support);
        push_f(c.inlier_fraction);
        push_f(c.temporal_coverage);
        push_f(c.residual_rms_ms.unwrap_or(f64::NAN));
        push_f(c.residual_p95_ms.unwrap_or(f64::NAN));
        out.extend_from_slice(&c.inlier_count.to_le_bytes());
        out.extend_from_slice(&c.candidate_correspondences.to_le_bytes());
        out.push(match c.classification {
            AffineClass::Match => 0,
            AffineClass::Ambiguous => 1,
            AffineClass::NoMatch => 2,
        });
        out
    }

    /// A dense clean affine line: `t_b = intercept_s + scale·t_a` sampled over `[0, span_s]` at
    /// `n` points, every correspondence weight 1.0. Realistic support for the §10.6 gates.
    fn line_correspondences(
        intercept_s: f64,
        scale: f64,
        span_s: f64,
        n: usize,
    ) -> Vec<Correspondence> {
        (0..n)
            .map(|k| {
                let t_a = span_s * k as f64 / (n.max(2) - 1) as f64;
                corr(t_a, intercept_s + scale * t_a, 1.0)
            })
            .collect()
    }

    /// Deterministic pseudo-random in [0,1) from a u64 seed via a splitmix64 step. NOT used for any
    /// production decision — only to GENERATE test outliers reproducibly (no RNG crate, bit-stable).
    fn det_rand(state: &mut u64) -> f64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // 53-bit mantissa → [0,1).
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    // ---- §10.7 test 1: exact affine line recovers intercept/scale ----
    #[test]
    fn t1_exact_line_recovers_intercept_and_scale() {
        // lag 1250 ms (intercept 1.25 s), drift +80 ppm (scale 1.00008), 40 s span, 200 points.
        let intercept = 1.25;
        let scale = 1.000_08;
        let cs = line_correspondences(intercept, scale, 40.0, 200);
        let r = consensus(&cs, 40.0, &params());
        assert_eq!(r.classification, AffineClass::Match, "a clean rich line must Match");
        assert!((r.model.intercept_s - intercept).abs() < 1e-6, "intercept {} ≠ {}", r.model.intercept_s, intercept);
        assert!((r.model.scale - scale).abs() < 1e-9, "scale {} ≠ {}", r.model.scale, scale);
        assert!((r.lag_ms - 1250.0).abs() < 1e-3, "lag {} ms", r.lag_ms);
        assert!((r.drift_ppm - 80.0).abs() < 1e-2, "drift {} ppm", r.drift_ppm);
        assert!(r.residual_rms_ms.unwrap() < 1e-3, "clean line rms tiny");
        // The margin is finite and in [0, 1] (the §10.6 gate is `>= min_score_margin = 0` here, so
        // a clean line Matches). NOTE: a small-drift line legitimately has a STRONG non-equivalent
        // rival — the scale-1 (drift-0) hypothesis fits within tolerance over 40 s (max deviation
        // 80 ppm · 40 s = 3.2 ms ≪ 50 ms) yet differs by 80 ppm > the 50 ppm equivalence window —
        // so the margin is NOT ~1, and asserting that would be wrong. Uniqueness here comes from
        // the §10.6 gate threshold, exercised in t5.
        assert!(r.score_margin.is_finite() && (0.0..=1.0).contains(&r.score_margin), "margin in [0,1]: {}", r.score_margin);
    }

    // ---- §10.7 test 2: ±2000 ppm drift bounds ----
    #[test]
    fn t2_drift_bounds_plus_minus_2000ppm() {
        // Just INSIDE the bound: +1900 ppm and −1900 ppm lines must be recovered and Match.
        for &ppm in &[1900.0_f64, -1900.0_f64] {
            let scale = 1.0 + ppm / 1_000_000.0;
            let cs = line_correspondences(0.5, scale, 60.0, 300);
            let r = consensus(&cs, 60.0, &params());
            assert_eq!(r.classification, AffineClass::Match, "{ppm} ppm inside bound must Match");
            assert!((r.drift_ppm - ppm).abs() < 1.0, "recovered drift {} ≠ {}", r.drift_ppm, ppm);
        }

        // OUTSIDE the bound: a true +3000 ppm line. No hypothesis with |drift| ≤ 2000 can be a
        // clean fit; the result must NOT be a confident Match (clamped fit ⇒ huge residual ⇒
        // Ambiguous/NoMatch, never Match).
        let scale = 1.0 + 3000.0 / 1_000_000.0;
        let cs = line_correspondences(0.5, scale, 60.0, 300);
        let r = consensus(&cs, 60.0, &params());
        assert_ne!(r.classification, AffineClass::Match, "a 3000 ppm line exceeds the 2000 ppm bound");
        // And whatever drift is reported respects the bound (clamped).
        assert!(r.drift_ppm.abs() <= params().max_abs_drift_ppm + 1e-6, "reported drift within bound");
    }

    // ---- §10.7 test 3: 50% random outliers still recover the line ----
    #[test]
    fn t3_fifty_percent_outliers_recovers_line() {
        // 150 clean inliers on a 30 s, lag 200 ms, +50 ppm line; 150 deterministically-scattered
        // outliers (offsets up to ±2 s — far outside the 50 ms tolerance). The consensus must still
        // find the true line within a millisecond and inlier-classify the clean half.
        let intercept = 0.2;
        let scale = 1.000_05;
        let mut cs = line_correspondences(intercept, scale, 30.0, 150);
        let mut state = 0xCAFE_F00D_1234_5678u64;
        for k in 0..150 {
            let t_a = 30.0 * det_rand(&mut state);
            // A gross offset (±2 s) off the true line.
            let off = (det_rand(&mut state) - 0.5) * 4.0;
            cs.push(corr(t_a, intercept + scale * t_a + off, 1.0));
            let _ = k;
        }
        let r = consensus(&cs, 30.0, &params());
        assert_eq!(r.classification, AffineClass::Match, "the clean half must carry a Match");
        // The LINE is recovered: the lag/intercept (the dominant, robust quantity) lands within a
        // millisecond despite the 50% gross outliers. (Drift precision at the 50 ms inlier tolerance
        // is coarse — a few outliers that happen to fall within tolerance bias the slope; fine drift
        // is the §11 refinement's job in boundary 7. Here we only require drift to stay within the
        // physical bound, which the clamp guarantees.)
        assert!((r.model.intercept_s - intercept).abs() < 1e-3, "intercept {} ≈ {}", r.model.intercept_s, intercept);
        assert!(r.drift_ppm.abs() <= params().max_abs_drift_ppm, "drift {} within bound", r.drift_ppm);
        // Support reflects ~150 clean inliers (a handful of outliers can also fall inside the 50 ms
        // band by chance), not the 300 total — the gross half is rejected.
        assert!(r.inlier_count >= 140 && r.inlier_count <= 175, "inliers {} ≈ 150 (clean half)", r.inlier_count);
        assert!(r.residual_p95_ms.unwrap() <= 50.0, "inlier p95 within tolerance");
    }

    // ---- §10.7 test 4: duplicated correspondences do NOT inflate support after dedup ----
    //
    // Boundary 4 (§9.2) already collapses a `(a_frame, b_frame)` fan-out to ONE correspondence
    // BEFORE consensus, so the stream reaching §10 is deduplicated; consensus deliberately does not
    // re-dedup (that is boundary 4's contract). The §10-level guarantee this test pins is that a
    // localized duplicate TOWER cannot manufacture a confident Match: the span-based temporal-
    // coverage gate is DUPLICATE-INVARIANT, so stacking one point many times changes neither the
    // recovered line nor the coverage, and a tower with no temporal spread fails §10.6 (cf. test 9).
    #[test]
    fn t4_duplicates_do_not_inflate_support() {
        // Honest line: 40 distinct points, weight 1.0, over 30 s.
        let cs_clean = line_correspondences(0.1, 1.0, 30.0, 40);
        let r_clean = consensus(&cs_clean, 30.0, &params());
        assert_eq!(r_clean.classification, AffineClass::Match);

        // Stack ONE of those points 1000× (a degenerate duplicate fan at a single time).
        let mut cs_dup = cs_clean.clone();
        let dup_point = cs_clean[20];
        for _ in 0..1000 {
            cs_dup.push(dup_point);
        }
        let r_dup = consensus(&cs_dup, 30.0, &params());
        // The line is unchanged: a duplicate tower at one time does not move intercept or drift.
        assert_eq!(r_dup.classification, AffineClass::Match);
        assert!((r_dup.model.intercept_s - 0.1).abs() < 1e-6, "duplicates do not move the line");
        // KEY anti-inflation invariant: temporal coverage is IDENTICAL with and without the tower —
        // the duplicates add no A-span, so they cannot manufacture the coverage a real match needs.
        assert!(
            (r_dup.temporal_coverage - r_clean.temporal_coverage).abs() < 1e-9,
            "duplicates at one time must not change temporal coverage ({} vs {})",
            r_dup.temporal_coverage,
            r_clean.temporal_coverage
        );
        // The duplicates ARE present in the slice (consensus does not re-dedup — boundary 4 does);
        // the protection is structural via coverage, not by silently dropping them here.
        assert!(r_dup.inlier_count >= 40 + 1000, "consensus does not re-dedup; boundary 4 owns that");

        // Direct proof of the structural guarantee: a PURE duplicate tower (no distinct points, no
        // spread) — 2000 copies of two points 5 s apart — has huge weighted support yet near-zero
        // coverage over a long overlap, so it is NOT a Match regardless of how many copies pile up.
        let mut tower = Vec::new();
        for _ in 0..1000 {
            tower.push(corr(10.0, 10.05, 1.0));
            tower.push(corr(10.2, 10.25, 1.0)); // 0.2 s apart ⇒ tiny span
        }
        let rt = consensus(&tower, 120.0, &params());
        assert_ne!(rt.classification, AffineClass::Match, "a duplicate tower with no span cannot Match");
        assert!(rt.temporal_coverage < params().min_temporal_coverage, "tower coverage {} below gate", rt.temporal_coverage);
    }

    // ---- §10.7 test 5: repeated-offset runner-up causes ambiguity ----
    #[test]
    fn t5_repeated_offset_runner_up_is_ambiguous() {
        // Two EQUALLY strong, well-separated clean clusters at DIFFERENT lags (repeated content):
        // cluster 1 at lag 0 ms, cluster 2 at lag +5000 ms (5 s — far beyond the 250 ms equivalence
        // window). Each is a full clean line over a 30 s A-span. With min_score_margin raised above
        // 0, the non-equivalent rival depresses the margin ⇒ Ambiguous, NOT a confident Match.
        let span = 30.0;
        let n = 80;
        let mut cs = Vec::new();
        for k in 0..n {
            let t_a = span * k as f64 / (n - 1) as f64;
            cs.push(corr(t_a, t_a + 0.0, 1.0)); // cluster 1: lag 0
            cs.push(corr(t_a, t_a + 5.0, 1.0)); // cluster 2: lag +5 s
        }
        let mut p = params();
        p.min_score_margin = 0.2; // a realistic uniqueness gate (profile 0 default is 0.0)
        let r = consensus(&cs, span, &p);
        assert_eq!(
            r.classification,
            AffineClass::Ambiguous,
            "two equally strong non-equivalent lags must be Ambiguous, margin={}",
            r.score_margin
        );
        // There IS a strong non-equivalent runner-up, so the margin is well below 1.
        assert!(r.runner_up_score > 0.0, "a non-equivalent rival exists");
        assert!(r.score_margin < 0.2, "margin {} below the gate", r.score_margin);

        // Sanity: with the default margin gate of 0.0 the same content is a Match on the winning
        // cluster (margin ≥ 0 always) — proving the AMBIGUITY is driven by the uniqueness gate, the
        // §10.5/§10.6 mechanism, not by a fit failure.
        let r0 = consensus(&cs, span, &params());
        assert_eq!(r0.classification, AffineClass::Match, "margin gate 0 ⇒ winner Matches");
    }

    // ---- §10.7 test 6: input ordering does NOT change output bits ----
    #[test]
    fn t6_input_order_invariant_output_bits() {
        // A line WITH outliers (so hypothesis generation, scoring, refinement, AND the runner-up all
        // exercise order-sensitivity if any existed). Serialize the consensus, then shuffle the
        // slice deterministically and require a byte-identical consensus.
        let intercept = 0.333;
        let scale = 1.000_12;
        let mut cs = line_correspondences(intercept, scale, 45.0, 120);
        let mut state = 0x1357_9BDF_2468_ACE0u64;
        for _ in 0..120 {
            let t_a = 45.0 * det_rand(&mut state);
            let off = (det_rand(&mut state) - 0.5) * 3.0;
            // Varied weights too, so a weight-order dependence would show.
            let w = 0.25 + 0.75 * det_rand(&mut state);
            cs.push(corr(t_a, intercept + scale * t_a + off, w));
        }
        let baseline = consensus(&cs, 45.0, &params());
        let base_bytes = serialize_consensus(&baseline);

        // Deterministic Fisher–Yates shuffle (det_rand) — a true REORDERING of the same multiset.
        let mut shuffled = cs.clone();
        let mut st2 = 0xF0E1_D2C3_B4A5_9687u64;
        for i in (1..shuffled.len()).rev() {
            let j = (det_rand(&mut st2) * (i as f64 + 1.0)) as usize;
            shuffled.swap(i, j.min(i));
        }
        // Confirm it is actually a different order (not a no-op) but the same set.
        assert_ne!(
            cs.iter().map(|c| c.t_a_s.to_bits()).collect::<Vec<_>>(),
            shuffled.iter().map(|c| c.t_a_s.to_bits()).collect::<Vec<_>>(),
            "the shuffle must reorder the slice"
        );
        let shuffled_bytes = serialize_consensus(&consensus(&shuffled, 45.0, &params()));
        assert_eq!(base_bytes, shuffled_bytes, "consensus output must be byte-identical under reordering");

        // And a second run on the SAME order is identical too (pure determinism).
        assert_eq!(base_bytes, serialize_consensus(&consensus(&cs, 45.0, &params())));
    }

    // ---- §10.7 test 7: correspondence cap preserves a late true segment (line still found) ----
    //
    // Model the boundary-4 cap's effect: most correspondences sit early, with a sparse-but-real late
    // segment on the SAME line. Even though the early region dominates the count, the consensus must
    // still recover the true line (the two-point van-der-Corput sampler reaches the late points, and
    // the histogram offset is shared by the whole line) AND its coverage must reflect the late span.
    #[test]
    fn t7_cap_preserves_late_segment() {
        let intercept = 0.75;
        let scale = 1.0001; // +100 ppm
        // Dense early block [0, 10] s (100 points) + sparse late block [50, 60] s (10 points), same line.
        let mut cs = line_correspondences(intercept, scale, 10.0, 100);
        let late = {
            let mut v = Vec::new();
            for k in 0..10 {
                let t_a = 50.0 + 10.0 * k as f64 / 9.0;
                v.push(corr(t_a, intercept + scale * t_a, 1.0));
            }
            v
        };
        cs.extend(late);
        let overlap = 60.0;
        let r = consensus(&cs, overlap, &params());
        assert_eq!(r.classification, AffineClass::Match, "the line spanning to the late segment must Match");
        assert!((r.model.intercept_s - intercept).abs() < 1e-5, "intercept recovered");
        assert!((r.drift_ppm - 100.0).abs() < 1.0, "drift recovered with the late lever");
        // Coverage spans ~[0,60] s / 60 s overlap ⇒ near 1.0 (the late segment is included as an
        // inlier; a fit that dropped the late points would show coverage ≈ 10/60).
        assert!(r.temporal_coverage > 0.9, "late segment included ⇒ coverage {} ≈ 1", r.temporal_coverage);
    }

    // ---- §10.7 test 8: no two-point zero-RMS shortcut can classify a Match ----
    #[test]
    fn t8_no_two_point_zero_rms_match() {
        // EXACTLY two correspondences, > 5 s apart, on a perfect line (zero residual). A two-point
        // fit has zero RMS by construction — the §20 prohibited shortcut. It must NOT be a Match:
        // support (weighted 2) is far below min_support (12), and coverage over a real overlap is
        // tiny. Assert NoMatch/Ambiguous, never Match, and that the residual being ~0 did not help.
        let cs = vec![corr(1.0, 1.5, 1.0), corr(20.0, 20.5, 1.0)];
        let r = consensus(&cs, 100.0, &params());
        assert_ne!(r.classification, AffineClass::Match, "a 2-point zero-RMS fit must NOT Match");
        // The fit IS found (it is a real, if unsupported, line) — so residuals are present and ~0,
        // proving the rejection is by SUPPORT/coverage, not by a missing fit.
        if let Some(rms) = r.residual_rms_ms {
            assert!(rms < 1e-6, "the two-point fit is exact (rms ~0) — yet still not a Match");
        }
        assert!((r.support - 2.0).abs() < 1e-9, "weighted support is just 2.0, well under min_support 12");

        // Even with min_support LOWERED to 2, the coverage gate (span 19 s / overlap 100 s = 0.19 <
        // min_temporal_coverage 0.35) still blocks the Match — two points cannot both clear support
        // AND span the overlap. This is the structural §20 guarantee.
        let mut p = params();
        p.min_support = 2;
        let r2 = consensus(&cs, 100.0, &p);
        assert_ne!(r2.classification, AffineClass::Match, "even at min_support 2, coverage blocks a 2-point Match");
        assert!(r2.temporal_coverage < params().min_temporal_coverage, "coverage {} below the gate", r2.temporal_coverage);
    }

    // ---- §10.7 test 9: p95 and temporal-coverage gates reject a localized coincidence ----
    #[test]
    fn t9_p95_and_coverage_reject_localized_coincidence() {
        // A TIGHT cluster of many correspondences over a tiny A-span (e.g. a 0.3 s window) at one
        // offset — a localized coincidence with high support and low residual but NO temporal
        // reach. Over a long overlap its coverage is tiny ⇒ must NOT Match (the coverage gate).
        let mut cs = Vec::new();
        for k in 0..60 {
            let t_a = 10.0 + 0.3 * k as f64 / 59.0; // [10.0, 10.3] s — a 0.3 s span
            cs.push(corr(t_a, t_a + 0.05, 1.0)); // lag 50 ms, exact ⇒ low residual
        }
        let overlap = 120.0; // span 0.3 s / 120 s ⇒ coverage 0.0025 ≪ 0.35
        let r = consensus(&cs, overlap, &params());
        assert_ne!(r.classification, AffineClass::Match, "a localized tight cluster must NOT Match");
        assert!(r.support >= params().min_support as f64, "support is high ({}) — not the blocker", r.support);
        assert!(r.temporal_coverage < params().min_temporal_coverage, "coverage {} blocks the Match", r.temporal_coverage);

        // Separately exercise the p95 gate: a cluster that DOES span the overlap but whose residuals
        // straddle the p95 limit must fail on p95 even though support and coverage pass. The scatter
        // is SIGN-ALTERNATING and time-SYMMETRIC so a least-squares refit cannot absorb it into the
        // line (the residuals survive any intercept/slope shift) — the p95 stays ≈47 ms > 45 ms cap,
        // while every point is still within the 50 ms inlier tolerance (so all count toward p95).
        let mut cs2 = Vec::new();
        let n = 100;
        for k in 0..n {
            let t_a = 60.0 * k as f64 / (n - 1) as f64;
            // Alternating ±47 ms residual about the line; symmetric in time and sign so the fit is
            // not tilted/shifted and the |residuals| remain ≈47 ms after refinement.
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            let resid_ms = 47.0 * sign;
            cs2.push(corr(t_a, t_a + 0.05 + resid_ms / 1000.0, 1.0));
        }
        let r2 = consensus(&cs2, 60.0, &params());
        // Coverage and support pass here; the p95 (≈47 ms > max_residual_p95_ms 45) is the blocker.
        assert!(r2.temporal_coverage >= params().min_temporal_coverage, "this cluster spans the overlap (cov {})", r2.temporal_coverage);
        assert!(r2.support >= params().min_support as f64, "support passes ({})", r2.support);
        assert!(
            r2.residual_p95_ms.unwrap() > params().max_residual_p95_ms,
            "p95 {} must exceed the gate to block the Match",
            r2.residual_p95_ms.unwrap()
        );
        assert_ne!(r2.classification, AffineClass::Match, "p95 over the cap must block the Match");
    }

    // ===========================================================================================
    // §10.6 density-robust identity gate (the boundary-12 recall fix): the ABSOLUTE path
    // ===========================================================================================

    /// A profile-2-like params for the density tests: the absolute identity path ENABLED at a low
    /// count threshold (so a constructible test line can trip it) with a high absolute-coverage floor,
    /// and a raised `min_inlier_fraction` so the FRACTION path does NOT mask the absolute path under
    /// test. `min_temporal_coverage` stays at the default 0.35.
    fn params_abs(min_abs_inlier_count: u32, min_abs_coverage: f64) -> FingerprintMatchParams {
        FingerprintMatchParams {
            min_abs_inlier_count,
            min_abs_coverage,
            // Force the fraction path to FAIL on the dense test line so the OR is decided by the
            // absolute path alone (the construction below has a low true-line fraction).
            min_inlier_fraction: 0.95,
            ..FingerprintMatchParams::default()
        }
    }

    /// A dense line that spans the whole overlap (so coverage ≈ 1) but whose inlier FRACTION is LOW,
    /// modelling long dense orchestral audio: `n_line` clean inliers on the true line over `[0, span]`
    /// PLUS `n_noise` deterministically-scattered gross outliers (offsets ±2 s, far outside the 50 ms
    /// tolerance) at times also spanning `[0, span]`. The true line has a high inlier COUNT (`n_line`)
    /// and high coverage, but the weighted inlier FRACTION ≈ n_line / (n_line + n_noise) is low.
    fn low_fraction_dense_line(span_s: f64, n_line: usize, n_noise: usize) -> Vec<Correspondence> {
        let intercept = 0.05; // 50 ms lag
        let mut cs = line_correspondences(intercept, 1.0, span_s, n_line);
        let mut state = 0xD15E_A5ED_1234_5678u64;
        for _ in 0..n_noise {
            let t_a = span_s * det_rand(&mut state);
            let off = (det_rand(&mut state) - 0.5) * 4.0; // ±2 s gross offset
            cs.push(corr(t_a, intercept + t_a + off, 1.0));
        }
        cs
    }

    // BOUNDARY TEST (2) — the absolute-consensus path accepts a low-FRACTION / high-COUNT /
    // high-COVERAGE fit. The FRACTION path fails (fraction ≪ 0.95), but the dense true line has a high
    // inlier COUNT and spans the overlap, so the ABSOLUTE path carries the Match. Crucially this only
    // happens through a REFINED affine consensus (no classification from a raw count), and the other
    // gates (p95/drift/support/margin) still hold.
    #[test]
    fn absolute_path_accepts_low_fraction_high_count_high_coverage() {
        let span = 60.0;
        let cs = low_fraction_dense_line(span, 300, 1500); // ~300 inliers among ~1800 ⇒ fraction ~0.17
        let p = params_abs(/*min_abs_inlier_count*/ 200, /*min_abs_coverage*/ 0.85);
        let r = consensus(&cs, span, &p);
        // Sanity: the FRACTION path is genuinely failing here (so the absolute path is what matters).
        assert!(r.inlier_fraction < 0.95, "the dense true line has a LOW fraction ({})", r.inlier_fraction);
        assert!(r.inlier_count >= 200, "high inlier COUNT ({}) clears the absolute threshold", r.inlier_count);
        assert!(r.temporal_coverage >= 0.85, "the line spans the overlap (cov {})", r.temporal_coverage);
        assert_eq!(
            r.classification,
            AffineClass::Match,
            "a low-fraction but high-count, high-coverage REFINED line must Match via the absolute path"
        );
        // With the absolute path DISABLED (profile-0 inert: count threshold u32::MAX) the SAME line is
        // Ambiguous — proving the Match came specifically from the absolute path, not a relaxed gate.
        let p_off = FingerprintMatchParams { min_inlier_fraction: 0.95, ..FingerprintMatchParams::default() };
        assert_eq!(p_off.min_abs_inlier_count, u32::MAX, "profile-0 default leaves the absolute path inert");
        let r_off = consensus(&cs, span, &p_off);
        assert_eq!(r_off.classification, AffineClass::Ambiguous, "absolute path inert ⇒ low fraction ⇒ Ambiguous");
    }

    // BOUNDARY TEST (3) — a genuine negative (LOW count AND LOW fraction) stays NoMatch even with the
    // absolute path enabled. A handful of scattered correspondences with no coherent dense line: the
    // fraction path fails AND the absolute count is far below the threshold ⇒ neither path fires.
    #[test]
    fn negative_low_count_low_fraction_stays_no_match() {
        // Deterministic scatter with NO line: random t_b per t_a over 60 s, only ~40 points total.
        let mut cs = Vec::new();
        let mut st = 0xBADD_CAFE_0042u64;
        for _ in 0..40 {
            let t_a = 60.0 * det_rand(&mut st);
            let t_b = 60.0 * det_rand(&mut st); // unrelated
            cs.push(corr(t_a, t_b, 1.0));
        }
        // Absolute path enabled with a LOW count threshold (50) — the negative's inlier count is still
        // far below it, and the fraction is low ⇒ NoMatch/Ambiguous, never Match.
        let p = params_abs(50, 0.85);
        let r = consensus(&cs, 60.0, &p);
        assert_ne!(r.classification, AffineClass::Match, "a low-count low-fraction scatter must NOT Match");
        assert!(r.inlier_count < 50, "inlier count ({}) is below the absolute threshold", r.inlier_count);
    }

    // BOUNDARY TEST (5) — a LOCALIZED coincidence (high COUNT but LOW coverage) is rejected by the
    // COVERAGE clause of the absolute path. Many correspondences packed into a tiny A-span over a long
    // overlap: the inlier count is high (clears min_abs_inlier_count) but coverage is tiny (fails BOTH
    // min_abs_coverage AND the general min_temporal_coverage), so the absolute path CANNOT rescue it.
    #[test]
    fn absolute_path_rejects_localized_high_count_low_coverage() {
        // 400 inliers packed into a 0.5 s A-span at lag 50 ms, over a 120 s overlap ⇒ coverage ≈ 0.004.
        let mut cs = Vec::new();
        for k in 0..400 {
            let t_a = 10.0 + 0.5 * k as f64 / 399.0; // [10.0, 10.5] s — a 0.5 s span
            cs.push(corr(t_a, t_a + 0.05, 1.0));
        }
        let overlap = 120.0;
        // Absolute path enabled with a count threshold (200) the cluster CLEARS, and a high coverage
        // floor (0.90) it does NOT — the coverage clause must block it.
        let p = params_abs(200, 0.90);
        let r = consensus(&cs, overlap, &p);
        assert!(r.inlier_count >= 200, "the localized cluster has a HIGH count ({}) — not the blocker", r.inlier_count);
        assert!(r.temporal_coverage < 0.90, "coverage ({}) fails the absolute-path floor", r.temporal_coverage);
        assert!(r.temporal_coverage < params().min_temporal_coverage, "and fails the general coverage gate too");
        assert_ne!(
            r.classification,
            AffineClass::Match,
            "high count but low coverage must be rejected by the coverage clause of the absolute path"
        );
    }

    // ---- determinism / no-NaN edge guards (§20) ----

    // No correspondences, one correspondence, empty/degenerate inputs ⇒ explicit NoMatch with
    // ABSENT residuals and finite zero statistics (never a panic, never NaN/Infinity).
    #[test]
    fn edge_cases_no_match_finite() {
        let p = params();
        for cs in [
            Vec::<Correspondence>::new(),
            vec![corr(1.0, 1.0, 1.0)],
            // Two points < 5 s apart ⇒ no two-point hypothesis; the histogram seeds scale-1 at
            // their shared offset, but coverage/support make this a non-match — still finite.
            vec![corr(1.0, 1.5, 1.0), corr(1.1, 1.6, 1.0)],
        ] {
            let r = consensus(&cs, 10.0, &p);
            assert!(r.lag_ms.is_finite() && r.drift_ppm.is_finite());
            assert!(r.score.is_finite() && r.score_margin.is_finite());
            assert!(r.support.is_finite() && r.temporal_coverage.is_finite() && r.inlier_fraction.is_finite());
        }
        // Empty ⇒ NoMatch with None residuals.
        let r = consensus(&[], 10.0, &p);
        assert_eq!(r.classification, AffineClass::NoMatch);
        assert!(r.residual_rms_ms.is_none() && r.residual_p95_ms.is_none());
    }

    // A non-positive / non-finite overlap is floored so temporal coverage stays finite (§20).
    #[test]
    fn non_positive_overlap_is_finite() {
        let cs = line_correspondences(0.2, 1.0, 30.0, 50);
        for &ov in &[0.0_f64, -5.0, f64::NAN, f64::INFINITY] {
            let r = consensus(&cs, ov, &params());
            assert!(r.temporal_coverage.is_finite(), "coverage finite for overlap {ov}");
            assert!(r.score.is_finite());
        }
    }

    // The van der Corput generator: known low-discrepancy values (base 2 and 3) and range [0,1).
    #[test]
    fn van_der_corput_known_values() {
        // base 2: 1→0.5, 2→0.25, 3→0.75, 4→0.125
        assert!((van_der_corput(1, 2) - 0.5).abs() < 1e-12);
        assert!((van_der_corput(2, 2) - 0.25).abs() < 1e-12);
        assert!((van_der_corput(3, 2) - 0.75).abs() < 1e-12);
        assert!((van_der_corput(4, 2) - 0.125).abs() < 1e-12);
        // base 3: 1→1/3, 2→2/3, 3→1/9
        assert!((van_der_corput(1, 3) - 1.0 / 3.0).abs() < 1e-12);
        assert!((van_der_corput(2, 3) - 2.0 / 3.0).abs() < 1e-12);
        assert!((van_der_corput(3, 3) - 1.0 / 9.0).abs() < 1e-12);
        for k in 1..1000u64 {
            let v = van_der_corput(k, 2);
            assert!((0.0..1.0).contains(&v), "vdc in [0,1): {v}");
        }
    }

    // The weighted line fit rejects a degenerate time spread (variance floor) instead of dividing by
    // ~0 — the §10.4/§20 anti-NaN guard at the fit level.
    #[test]
    fn weighted_fit_rejects_degenerate_spread() {
        // All points at the same t_a ⇒ zero time variance ⇒ None (no NaN slope).
        let pts = [(5.0, 1.0, 1.0), (5.0, 2.0, 1.0), (5.0, 3.0, 1.0)];
        assert!(weighted_line_fit(pts.into_iter(), &params()).is_none());
        // A real spread fits.
        let pts2 = [(0.0, 0.0, 1.0), (10.0, 10.0, 1.0)];
        let m = weighted_line_fit(pts2.into_iter(), &params()).unwrap();
        assert!((m.scale - 1.0).abs() < 1e-9 && m.intercept_s.abs() < 1e-9);
    }

    // The §10.5 equivalence window and score_margin formula on hand values.
    #[test]
    fn score_margin_and_equivalence() {
        let a = AffineModel { intercept_s: 0.0, scale: 1.0 };
        // Within 250 ms lag and 50 ppm drift ⇒ equivalent.
        let b = AffineModel { intercept_s: 0.2, scale: 1.000_04 }; // 200 ms, 40 ppm
        assert!(equivalent(a, b));
        // Beyond either ⇒ not equivalent.
        let c = AffineModel { intercept_s: 0.3, scale: 1.0 }; // 300 ms lag
        assert!(!equivalent(a, c));
        let d = AffineModel { intercept_s: 0.0, scale: 1.000_06 }; // 60 ppm
        assert!(!equivalent(a, d));
        // Margin: no rival ⇒ 1; equal rival ⇒ 0; half ⇒ 0.5.
        assert!((score_margin(10.0, 0.0) - 1.0).abs() < 1e-12);
        assert!((score_margin(10.0, 10.0) - 0.0).abs() < 1e-12);
        assert!((score_margin(10.0, 5.0) - 0.5).abs() < 1e-12);
        // best == 0 ⇒ finite (eps denom), clamped to [0,1].
        assert!(score_margin(0.0, 0.0).is_finite());
    }

    // ===========================================================================================
    // §12.3 — metadata hypotheses: the NON-VETOING contract at the consensus level
    // ===========================================================================================

    use super::super::metadata_hint::{HintReliability, TimeHint, TimeHintSource};

    /// A rankable (Medium) scale-1 hint at `lag_ms` with a tight (sample-ref) tolerance.
    fn medium_hint(lag_ms: f64) -> TimeHint {
        TimeHint {
            lag_ms,
            tolerance_ms: 2.0,
            source: TimeHintSource::BwfSampleReference,
            reliability: HintReliability::Medium,
        }
    }

    /// A bit-exact serialization of the consensus (reuses the test-6 helper) for determinism checks.
    fn ser(c: &AffineConsensus) -> Vec<u8> {
        serialize_consensus(c)
    }

    // ---- §12.3a: a rankable hint adds a hypothesis but does NOT override a BETTER audio line ----
    //
    // A clean audio line at lag +200 ms with rich support, plus a WRONG metadata hint pointing at
    // lag +9000 ms. The hint seeds a scale-1 hypothesis at +9000, but it earns ~zero audio inliers
    // (no correspondences sit there) and loses to the audio line. The winning lag is the AUDIO line.
    #[test]
    fn metadata_does_not_override_better_audio() {
        let intercept = 0.2; // +200 ms true audio line
        let cs = line_correspondences(intercept, 1.0, 40.0, 200);
        let wrong_hint = medium_hint(9000.0); // points 8.8 s away — wildly wrong

        let with = consensus_with_hint(&cs, 40.0, &params(), Some(&wrong_hint));
        let without = consensus(&cs, 40.0, &params());

        assert_eq!(with.classification, AffineClass::Match, "the audio line still Matches");
        assert!((with.lag_ms - 200.0).abs() < 1.0, "winner is the AUDIO lag, not the hint: {}", with.lag_ms);
        // The wrong hint changed NOTHING about the result (the audio line dominates on score).
        assert_eq!(ser(&with), ser(&without), "a wrong rankable hint must not perturb a strong audio match");
    }

    // ---- §12.3a: the seeded hypothesis only WINS by earning audio inliers ----
    //
    // When the metadata hint POINTS AT the true audio line (correct prior), the result is the same
    // correct Match at the same lag/drift — the hint did not "create" the match, the audio inliers
    // did. The hint may legitimately SHARPEN the candidate (the seeded scale-1 hypothesis sits
    // exactly on the line, whereas the histogram bin-center is offset by < 1 bin), which can only
    // RAISE the §10.3 score — never change the accepted model or relax a gate. So we assert the
    // load-bearing outputs (classification + refined model) are unchanged and the score did not drop.
    #[test]
    fn correct_hint_only_helps_never_harms() {
        let cs = line_correspondences(0.2, 1.0, 40.0, 200);
        let good_hint = medium_hint(200.0); // agrees with the audio line
        let with = consensus_with_hint(&cs, 40.0, &params(), Some(&good_hint));
        let without = consensus(&cs, 40.0, &params());
        // Same verdict, same accepted mapping (the refined line is the true 200 ms / 0 ppm line both
        // ways — refinement recovers it regardless of which scale-1 seed won).
        assert_eq!(with.classification, AffineClass::Match);
        assert_eq!(without.classification, AffineClass::Match);
        assert!((with.lag_ms - without.lag_ms).abs() < 1e-6, "same accepted lag {} vs {}", with.lag_ms, without.lag_ms);
        assert!((with.drift_ppm - without.drift_ppm).abs() < 1e-6, "same accepted drift");
        assert!((with.lag_ms - 200.0).abs() < 1.0, "the audio line, ~200 ms");
        // A correct prior can only help the ranking score (sharper seed ⇒ higher score), never lower
        // it — and it never weakens acceptance (still a Match by the same gates).
        assert!(with.score >= without.score - 1e-12, "a correct hint must not LOWER the score: {} vs {}", with.score, without.score);
    }

    // ---- §12.3: metadata cannot turn NoMatch → Match ----
    //
    // An UNRELATED pair: scattered correspondences with no coherent line. A metadata hint (even a
    // confident-looking Medium one) cannot manufacture a match — the seeded scale-1 hypothesis finds
    // no audio inliers and the §10.6 gates still reject. Result: NOT a Match, with or without the hint.
    #[test]
    fn metadata_cannot_turn_no_match_into_match() {
        // Deterministic scatter over 40 s with NO line (random t_b per t_a, gross spread).
        let mut cs = Vec::new();
        let mut st = 0xA11C_E5_BADu64;
        for _ in 0..200 {
            let t_a = 40.0 * det_rand(&mut st);
            let t_b = 40.0 * det_rand(&mut st); // unrelated
            cs.push(corr(t_a, t_b, 1.0));
        }
        let hint = medium_hint(1234.0);
        let without = consensus(&cs, 40.0, &params());
        let with = consensus_with_hint(&cs, 40.0, &params(), Some(&hint));
        assert_ne!(without.classification, AffineClass::Match, "unrelated pair is not a Match");
        assert_ne!(
            with.classification,
            AffineClass::Match,
            "a metadata hint must NOT turn an unrelated NoMatch/Ambiguous into a Match"
        );
    }

    // ---- §12.3b: metadata breaks an EXACT audio-score tie (and ONLY an exact tie) ----
    //
    // Construct TWO audio-equivalent candidate clusters at DIFFERENT lags that produce bit-identical
    // audio scores/support/coverage/p95 (symmetric construction), so rungs 1–4 are an EXACT tie. A
    // metadata hint at ONE of the two lags must order THAT cluster's hypothesis first ⇒ the winner is
    // the metadata-consistent lag. Then prove the SAME hint can NOT flip a NON-exact tie (one cluster
    // strictly stronger ⇒ better audio score always wins).
    #[test]
    fn metadata_breaks_exact_tie_only() {
        // Two clusters: lag −3000 ms and lag +3000 ms, each a clean line over the SAME A-times with
        // the SAME number of points and weights ⇒ identical audio quality (an exact tie on rungs
        // 1–4). 60 points each over [0, 40] s; well separated (> 250 ms) so they are non-equivalent.
        let span = 40.0;
        let n = 60;
        let lag_lo = -3.0; // s
        let lag_hi = 3.0; // s
        let mut cs = Vec::new();
        for k in 0..n {
            let t_a = span * k as f64 / (n - 1) as f64;
            cs.push(corr(t_a, t_a + lag_lo, 1.0));
            cs.push(corr(t_a, t_a + lag_hi, 1.0));
        }

        // No hint: the deterministic ladder picks one of the two by the geometric rungs (lower |lag|
        // then lower lag ⇒ here both |lag| equal at 3000, so rung 7 "lower lag" picks −3000).
        let base = consensus(&cs, span, &params());
        assert_eq!(base.classification, AffineClass::Match);
        assert!((base.lag_ms - (-3000.0)).abs() < 1.0, "no-hint tie resolves to −3000 by rung 7, got {}", base.lag_ms);

        // Hint at +3000 ms (the OTHER cluster): it must now win the EXACT tie ⇒ winner lag +3000.
        let hint_hi = medium_hint(3000.0);
        let with_hi = consensus_with_hint(&cs, span, &params(), Some(&hint_hi));
        assert_eq!(with_hi.classification, AffineClass::Match);
        assert!(
            (with_hi.lag_ms - 3000.0).abs() < 1.0,
            "the metadata-consistent cluster (+3000) must win the EXACT audio tie, got {}",
            with_hi.lag_ms
        );

        // Symmetry: a hint at −3000 keeps −3000 (consistent with the no-hint pick here, but now it is
        // metadata-DRIVEN, proving the rung fires both ways).
        let hint_lo = medium_hint(-3000.0);
        let with_lo = consensus_with_hint(&cs, span, &params(), Some(&hint_lo));
        assert!((with_lo.lag_ms - (-3000.0)).abs() < 1.0, "hint at −3000 selects −3000");

        // ---- NON-exact tie: make the +3000 cluster STRICTLY STRONGER (more points) and aim the hint
        // at the WEAKER −3000 cluster. A better audio score must always win ⇒ winner stays +3000. ----
        let mut cs2 = Vec::new();
        for k in 0..n {
            let t_a = span * k as f64 / (n - 1) as f64;
            cs2.push(corr(t_a, t_a + lag_lo, 1.0)); // weaker cluster: n points
        }
        for k in 0..(2 * n) {
            let t_a = span * k as f64 / (2 * n - 1) as f64;
            cs2.push(corr(t_a, t_a + lag_hi, 1.0)); // stronger cluster: 2n points ⇒ higher score
        }
        let hint_weak = medium_hint(-3000.0); // points at the WEAKER cluster
        let with_weak = consensus_with_hint(&cs2, span, &params(), Some(&hint_weak));
        assert!(
            (with_weak.lag_ms - 3000.0).abs() < 1.0,
            "a BETTER audio cluster (+3000) must win despite a hint at the weaker −3000: got {}",
            with_weak.lag_ms
        );
    }

    // ---- §12.3: a DebugOnly / non-orderable hint changes NOTHING (the 11.6-year case at consensus)
    //
    // The 11.6-year disagreement parses to HintReliability::DebugOnly upstream; at the consensus level
    // a DebugOnly hint is filtered out of BOTH the rank set and the order set, so the result is
    // byte-identical to no hint. We also show that even if such a wildly-wrong lag WERE rankable, it
    // would lose (zero audio inliers) — the structural guarantee, not just the filter.
    #[test]
    fn debug_only_hint_is_inert_and_wrong_lag_loses() {
        let cs = line_correspondences(0.2, 1.0, 40.0, 200); // true line at +200 ms
        let eleven_six_years_ms = 11.6 * 365.25 * 24.0 * 3600.0 * 1000.0;

        // DebugOnly ⇒ neither can_rank nor can_order ⇒ inert. Byte-identical to no hint.
        let debug_hint = TimeHint {
            lag_ms: eleven_six_years_ms,
            tolerance_ms: 1000.0,
            source: TimeHintSource::ContainerCreationTime,
            reliability: HintReliability::DebugOnly,
        };
        let without = consensus(&cs, 40.0, &params());
        let with_debug = consensus_with_hint(&cs, 40.0, &params(), Some(&debug_hint));
        assert_eq!(ser(&without), ser(&with_debug), "a DebugOnly hint must be completely inert");
        assert!((with_debug.lag_ms - 200.0).abs() < 1.0, "audio line unchanged: {}", with_debug.lag_ms);

        // Structural proof: even FORCING the 11.6-year lag to Medium (rankable), it earns no inliers
        // and the audio line still wins — the bad lag CANNOT harm the correct match.
        let forced = medium_hint(eleven_six_years_ms);
        let with_forced = consensus_with_hint(&cs, 40.0, &params(), Some(&forced));
        assert_eq!(with_forced.classification, AffineClass::Match, "audio still Matches");
        assert!((with_forced.lag_ms - 200.0).abs() < 1.0, "winner is still the audio lag, not 11.6 y");
        assert_eq!(ser(&with_forced), ser(&without), "even a forced wrong rankable hint perturbs nothing");
    }

    // ---- determinism: a hinted consensus is byte-identical across runs and input reorderings ----
    #[test]
    fn hinted_consensus_is_deterministic() {
        let mut cs = line_correspondences(0.333, 1.000_05, 45.0, 120);
        let mut st = 0x0BAD_C0DE_1234u64;
        for _ in 0..80 {
            let t_a = 45.0 * det_rand(&mut st);
            let off = (det_rand(&mut st) - 0.5) * 3.0;
            cs.push(corr(t_a, 0.333 + 1.000_05 * t_a + off, 0.5 + 0.5 * det_rand(&mut st)));
        }
        let hint = medium_hint(333.0);
        let a = ser(&consensus_with_hint(&cs, 45.0, &params(), Some(&hint)));
        // Same input, again.
        assert_eq!(a, ser(&consensus_with_hint(&cs, 45.0, &params(), Some(&hint))));
        // Reordered input ⇒ still identical (the canonical sort + the hint are order-invariant).
        let mut shuffled = cs.clone();
        let mut st2 = 0xFACE_FEED_4242u64;
        for i in (1..shuffled.len()).rev() {
            let j = (det_rand(&mut st2) * (i as f64 + 1.0)) as usize;
            shuffled.swap(i, j.min(i));
        }
        assert_eq!(a, ser(&consensus_with_hint(&shuffled, 45.0, &params(), Some(&hint))));
        // And every field finite (no NaN/Inf) under the hint.
        let r = consensus_with_hint(&cs, 45.0, &params(), Some(&hint));
        assert!(r.lag_ms.is_finite() && r.drift_ppm.is_finite() && r.score.is_finite());
        assert!(r.score_margin.is_finite() && r.temporal_coverage.is_finite());
    }

    // ---- a NON-finite hint lag is rejected by push_hypothesis and cannot reach the result ----
    #[test]
    fn non_finite_hint_lag_is_ignored() {
        let cs = line_correspondences(0.2, 1.0, 40.0, 200);
        let nan_hint = medium_hint(f64::NAN);
        // consensus_with_hint pre-filters non-finite hints; even if it reached generation,
        // push_hypothesis rejects a non-finite intercept. Result == no-hint.
        let with = consensus_with_hint(&cs, 40.0, &params(), Some(&nan_hint));
        let without = consensus(&cs, 40.0, &params());
        assert_eq!(ser(&with), ser(&without), "a non-finite hint lag must be ignored (no NaN reaches output)");
    }
}
