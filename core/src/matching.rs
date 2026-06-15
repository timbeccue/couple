//! Method-neutral pair evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §5).
//!
//! This module is the boundary-6 cut: the *public core data contract* a resolver consumes
//! regardless of which matcher produced it, plus the orchestration that turns either the legacy
//! envelope matcher (`pair_correlate`) or the spectral-landmark pipeline (boundary 4's index +
//! boundary 5's [`consensus`](crate::fingerprint::consensus)) into one [`PairEvidence`].
//!
//! ## Scope (EXACTLY §5)
//!
//! - §5.1 [`MatchMethod`], [`MatchClassification`], [`PairEvidence`] — the evidence types. Every
//!   numeric field is FINITE; non-applicable residuals are `Option` and serialize to JSON `null`;
//!   nothing here ever serializes NaN/Infinity (§5.1, §20).
//! - §5.2 lives in [`crate::pairing_params`] ([`MatcherMode`](crate::pairing_params::MatcherMode)
//!   + the new `PairingParams` fields).
//! - §5.3 the new `Ctx` operations `match_pair` / `match_candidates` live in [`crate::ctx`] and
//!   call the pure orchestrators here ([`evidence_from_legacy`], [`evidence_from_consensus`]).
//!
//! The `refined_*` fields are populated by BOUNDARY 7 (§11 fine refinement); here they stay
//! `None`. This module calls NO existing local aligner / `verify_pair`, adds NO FFI symbol, and
//! does NOT consume the `graph_*` params — those are later boundaries. The default resolver path
//! is untouched: these are NEW additive APIs.
//!
//! ## §9.3 no-evidence / below-support classification mapping
//!
//! The legacy wrapper classifies by the legacy gate ([`PairCorrelation::legacy_candidate`]). The
//! spectral wrapper maps boundary 5's [`AffineConsensus`] to a [`MatchClassification`] EXACTLY per
//! §9.3 (see [`evidence_from_consensus`]):
//!
//! - overlap below `min_overlap_s` ⇒ [`MatchClassification::InsufficientOverlap`] (handled by the
//!   caller before consensus runs);
//! - fingerprint unavailable but the legacy matcher works ⇒ a `EnvelopeV1` fallback (the caller
//!   produces it via [`evidence_from_legacy`]);
//! - fingerprints present, NO affine hypothesis ⇒ [`MatchClassification::NoMatch`];
//! - all six §10.6 gates pass ⇒ [`MatchClassification::Match`];
//! - below `min_support` but `>= ceil(min_support / 2)`, with a non-equivalent runner-up AND
//!   `score_margin < min_score_margin` ⇒ [`MatchClassification::Ambiguous`];
//! - any OTHER below-support / failed-gate case ⇒ [`MatchClassification::NoMatch`];
//! - the GENUINE resource ceiling (grouped correspondences exceeded `hard_correspondence_ceiling`,
//!   FAR above the routine §9.2 sample cap) was hit ⇒ [`MatchClassification::Ambiguous`] with
//!   telemetry, NEVER `NoMatch` (§9.3, §20 "never silently cap"). The ROUTINE §9.2 sample cap is NOT
//!   a resource ceiling: its representative subset is scored NORMALLY (the §9.2/§9.3 separation).

use crate::fingerprint::{
    consensus_with_hint, correspondences_for_pair, AffineClass, AffineConsensus, AudioHashIndex,
    CorrespondenceStats, FingerprintFormatV1, FingerprintReader, TimeHint,
};
use crate::pair::PairCorrelation;
use crate::pairing_params::PairingParams;
use serde::{Deserialize, Serialize};

/// The matcher that produced a [`PairEvidence`] (§5.1). A resolver may display it, but MUST gate
/// on [`PairEvidence::classification`], not on the method.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchMethod {
    /// Spectral-landmark consensus (boundary 4 correspondences → boundary 5 affine consensus).
    SpectralLandmarkV1,
    /// The legacy amplitude-envelope matcher (`pair_correlate`).
    EnvelopeV1,
}

/// The method-neutral classification of a candidate pair (§5.1). This is the ONLY field a
/// resolver keys on; fingerprint support thresholds must never be reinterpreted downstream (§5.1).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchClassification {
    /// A confident same-recording match with an accepted affine/lag mapping.
    Match,
    /// Confidently not the same recording (or no fittable affine hypothesis).
    NoMatch,
    /// Real evidence exists but is not confidently unique (repeated content) or a resource ceiling
    /// was hit — honest uncertainty, preferred over a confident wrong pairing (§1/§9.3).
    Ambiguous,
    /// The two tracks cannot overlap by the pairing floor (`min_overlap_s`) — no candidate
    /// possible (§9.3).
    InsufficientOverlap,
    /// The matcher could not run (e.g. a fingerprint could not be built and no legacy fallback was
    /// available). Distinct from `NoMatch`: absence of evidence, not evidence of absence (§5.1).
    Unavailable,
}

/// Method-neutral evidence for one candidate (video, audio) pair (§5.1). The resolver consumes
/// `classification`, `score` (deterministic ordering of same-matcher candidates), and the refined
/// mapping; the remaining fields are quality telemetry.
///
/// FINITENESS (§5.1, §20): `lag_ms`, `drift_ppm`, `score`, `runner_up_score`, `score_margin`,
/// `inlier_fraction`, `temporal_coverage`, and `overlap_s` are always finite (`score` is `0.0`
/// for a failed/unavailable measurement). The residual and refined fields are `Option`: `None`
/// serializes to JSON `null`, so NaN/Infinity is never written. Every constructor here upholds
/// this; callers must not poke a non-finite value in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairEvidence {
    /// Which matcher produced this evidence.
    pub method: MatchMethod,
    /// The method-neutral verdict — the resolver's sole decision input.
    pub classification: MatchClassification,

    // ---- Mapping from A (video) time to B (audio) time. ----
    /// Lag in milliseconds: content at A-time `t` occurs at `t·(1 + drift_ppm·1e-6) + lag_ms/1000`
    /// in B (the §3.1 convention). From landmark consensus in the spectral path; from the legacy
    /// coarse/refined lag in the envelope path.
    pub lag_ms: f64,
    /// Clock drift in ppm. From landmark consensus (spectral); `0.0` in the legacy path (the
    /// legacy stage-1 matcher does not estimate drift — boundary 7's refinement owns fine drift).
    pub drift_ppm: f64,

    // ---- Method-neutral ranking and uniqueness (higher is better; NOT a probability). ----
    /// Ranking score for candidates of the SAME matcher version. Finite; `0.0` when failed.
    pub score: f64,
    /// Best non-equivalent runner-up score (`0.0` when none).
    pub runner_up_score: f64,
    /// Dimensionless uniqueness margin in `[0, 1]` (`(score − runner_up)/max(score, eps)`).
    pub score_margin: f64,

    // ---- Consensus quality. ----
    /// Unweighted inlier/support count (spectral: final affine inliers; legacy: locked windows or
    /// 0 — the legacy stage-1 path has no fine inliers, those arrive with boundary 7).
    pub support: u32,
    /// Candidate correspondences fed to consensus (spectral); `0` for the legacy path.
    pub candidate_correspondences: u32,
    /// Weighted inlier fraction (spectral); `0.0` for the legacy path.
    pub inlier_fraction: f64,
    /// Temporal coverage of the inliers over the overlap (spectral); `0.0` for the legacy path.
    pub temporal_coverage: f64,
    /// RMS of inlier residuals to the mapping, ms. `None` when not applicable (no fittable
    /// hypothesis, or the legacy path) ⇒ JSON `null`.
    pub residual_rms_ms: Option<f64>,
    /// Nearest-rank p95 of inlier |residual|, ms. `None` when not applicable ⇒ JSON `null`.
    pub residual_p95_ms: Option<f64>,
    /// Candidate overlap in seconds this evidence was scored against (always finite, `>= 0`).
    pub overlap_s: f64,

    // ---- Fine refinement of the accepted affine hypothesis (BOUNDARY 7, §11). ----
    /// Refined lag, ms — `None` until boundary 7 populates it. JSON `null` here.
    pub refined_lag_ms: Option<f64>,
    /// Refined drift, ppm — `None` until boundary 7. JSON `null` here.
    pub refined_drift_ppm: Option<f64>,
    /// Refined RMS, ms — `None` until boundary 7. JSON `null` here.
    pub refined_rms_ms: Option<f64>,

    // ---- Optional legacy telemetry for shadow comparison only. ----
    /// The legacy `PairCorrelation`, attached in `FingerprintShadow` for comparison; `None` in
    /// `FingerprintPrimary` (and in the legacy wrapper, which IS the legacy result — there the
    /// method already says `EnvelopeV1`).
    pub legacy: Option<PairCorrelation>,

    // ---- §11.2 refinement telemetry (BOUNDARY 7). NOT serialized — debug only. ----
    /// Why the §11 fine refinement did or did not populate `refined_*` (debug telemetry, §11.2).
    /// `#[serde(skip)]` so the FFI/Swift wire JSON is BYTE-IDENTICAL to boundary 6 (boundary 8
    /// chooses how to surface it); it deserializes to [`RefineReason::NotAttempted`] via `Default`,
    /// so a round-trip of any non-refined evidence still compares equal. The fingerprint
    /// `match_pair` path sets it; the legacy / no-evidence constructors leave it `NotAttempted`.
    #[serde(skip)]
    pub refine_reason: crate::match_refine::RefineReason,
}

impl PairEvidence {
    /// The `InsufficientOverlap` evidence (§9.3): the two tracks cannot overlap by the pairing
    /// floor at any lag. `method` records which matcher was selected so a shadow caller still sees
    /// the intended path; all measurements are zero/`None` (finite, no NaN/Inf — §20).
    pub fn insufficient_overlap(method: MatchMethod, overlap_s: f64) -> Self {
        Self {
            method,
            classification: MatchClassification::InsufficientOverlap,
            lag_ms: 0.0,
            drift_ppm: 0.0,
            score: 0.0,
            runner_up_score: 0.0,
            score_margin: 0.0,
            support: 0,
            candidate_correspondences: 0,
            inlier_fraction: 0.0,
            temporal_coverage: 0.0,
            residual_rms_ms: None,
            residual_p95_ms: None,
            // Report the (sub-floor) overlap we measured, clamped finite & non-negative.
            overlap_s: finite_nonneg(overlap_s),
            refined_lag_ms: None,
            refined_drift_ppm: None,
            refined_rms_ms: None,
            legacy: None,
            refine_reason: crate::match_refine::RefineReason::NotAttempted,
        }
    }

    /// The `Unavailable` evidence (§5.1): the requested matcher could not run and no fallback was
    /// available (e.g. a fingerprint build failed AND the legacy matcher reported insufficient
    /// overlap / produced nothing usable). All-zero/`None`, finite.
    pub fn unavailable(method: MatchMethod, overlap_s: f64) -> Self {
        Self {
            method,
            classification: MatchClassification::Unavailable,
            lag_ms: 0.0,
            drift_ppm: 0.0,
            score: 0.0,
            runner_up_score: 0.0,
            score_margin: 0.0,
            support: 0,
            candidate_correspondences: 0,
            inlier_fraction: 0.0,
            temporal_coverage: 0.0,
            residual_rms_ms: None,
            residual_p95_ms: None,
            overlap_s: finite_nonneg(overlap_s),
            refined_lag_ms: None,
            refined_drift_ppm: None,
            refined_rms_ms: None,
            legacy: None,
            refine_reason: crate::match_refine::RefineReason::NotAttempted,
        }
    }
}

/// Clamp a value to a finite, non-negative number (used for `overlap_s`). A non-finite input
/// becomes `0.0`, so no NaN/Infinity can reach a serialized `PairEvidence` (§20).
fn finite_nonneg(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

/// Build `EnvelopeV1` evidence from a legacy [`PairCorrelation`] (§5.3 legacy wrapper / §9.3
/// `EnvelopeV1` fallback). The legacy matcher is method-neutral here: its `legacy_candidate`
/// boolean (`z >= legacy_z_min`, the unchanged default resolver gate — invariant 10) decides
/// `Match` vs `NoMatch`, and `insufficient_overlap` maps to `InsufficientOverlap`. The legacy
/// stage-1 result carries no drift, no fine inliers, and no residuals; those stay zero/`None` and
/// arrive with boundary 7.
///
/// `overlap_s` is the candidate overlap (the caller computes it from the track durations); it is
/// reported for telemetry and is independent of the legacy gate. `attach_legacy` controls whether
/// the raw `PairCorrelation` is embedded for shadow comparison (true in `FingerprintShadow` when a
/// fallback is taken; in a pure-legacy operation the method already says `EnvelopeV1`, so we do
/// NOT duplicate it into `legacy`).
///
/// `score` is the legacy selection score `z` (finite by construction; `0.0` when no scannable
/// lag) — a deterministic same-matcher ranking, not a probability.
pub fn evidence_from_legacy(
    corr: &PairCorrelation,
    overlap_s: f64,
    attach_legacy: bool,
) -> PairEvidence {
    if corr.insufficient_overlap {
        let mut ev = PairEvidence::insufficient_overlap(MatchMethod::EnvelopeV1, overlap_s);
        if attach_legacy {
            ev.legacy = Some(corr.clone());
        }
        return ev;
    }

    // The default legacy resolver gate is `legacy_candidate` (z >= legacy_z_min), computed by core
    // (invariant 8/10). A legacy candidate ⇒ Match; otherwise NoMatch. The legacy stage-1 path has
    // no notion of Ambiguous on its own — uncertainty in the legacy resolver is a shell-level
    // coverage/triangle decision, not a core stage-1 verdict — so the method-neutral wrapper
    // reports the binary legacy gate.
    let classification = if corr.legacy_candidate {
        MatchClassification::Match
    } else {
        MatchClassification::NoMatch
    };

    PairEvidence {
        method: MatchMethod::EnvelopeV1,
        classification,
        lag_ms: finite_or_zero(corr.lag_ms),
        drift_ppm: 0.0,
        // z is the legacy selection score; finite by construction.
        score: finite_or_zero(corr.z),
        runner_up_score: finite_or_zero(corr.runner_up_z),
        // Legacy has no uniqueness margin in the spectral sense; 0.0 keeps the field finite.
        score_margin: 0.0,
        support: 0,
        candidate_correspondences: 0,
        inlier_fraction: 0.0,
        temporal_coverage: 0.0,
        residual_rms_ms: None,
        residual_p95_ms: None,
        overlap_s: finite_nonneg(overlap_s),
        refined_lag_ms: None,
        refined_drift_ppm: None,
        refined_rms_ms: None,
        legacy: if attach_legacy { Some(corr.clone()) } else { None },
        refine_reason: crate::match_refine::RefineReason::NotAttempted,
    }
}

/// Build `SpectralLandmarkV1` evidence from a boundary-5 [`AffineConsensus`] (§5.3 spectral path),
/// applying the §9.3 no-evidence / below-support classification mapping.
///
/// `consensus` is the output of [`crate::fingerprint::consensus`] over the pair's correspondences;
/// `corr_stats` is the boundary-4 [`CorrespondenceStats`] for the same pair (its `capped` flag is
/// the resource-ceiling signal); `params` supplies `min_support` and `min_score_margin`;
/// `legacy` is an optional `PairCorrelation` to embed for shadow comparison.
///
/// Classification (EXACTLY §9.3):
///
/// 1. The GENUINE resource ceiling fired (`corr_stats.hard_ceiling_exceeded`: grouped
///    correspondences exceeded `hard_correspondence_ceiling`, a threshold FAR above the routine §9.2
///    sample cap) ⇒ [`MatchClassification::Ambiguous`] regardless of the affine verdict — a
///    truly-exhausted pair must never present as a confident `Match` or a `NoMatch` (§9.3, §20).
///    The ROUTINE §9.2 sample cap (`corr_stats.capped`) is NOT this case: its representative subset is
///    scored NORMALLY in step 2 (separating "§9.2 sample taken ⇒ score normally" from "§9.3 resource
///    ceiling ⇒ Ambiguous" is the boundary-12 recall fix). (Telemetry: both flags are in `corr_stats`.)
/// 2. Else map the affine class (the §9.2 sample, if any, is scored here like an uncapped pair):
///    - [`AffineClass::Match`] ⇒ `Match`.
///    - [`AffineClass::NoMatch`] (no fittable hypothesis) ⇒ `NoMatch`.
///    - [`AffineClass::Ambiguous`] (a fittable line failed a §10.6 gate) ⇒ the §9.3 below-support
///      refinement: `Ambiguous` only when `ceil(min_support/2) <= support < min_support` AND a
///      non-equivalent runner-up exists (`runner_up_score > 0`) AND `score_margin <
///      min_score_margin`; any other failing case is `NoMatch`.
pub fn evidence_from_consensus(
    consensus: &AffineConsensus,
    corr_stats: &CorrespondenceStats,
    params: &PairingParams,
    legacy: Option<PairCorrelation>,
) -> PairEvidence {
    let fp = &params.fingerprint_match;
    // Weighted support gates `Match` in §10.6; the §9.3 below-support BAND is on the same
    // weighted quantity. `min_support` is a u32; the lower band edge is ceil(min_support/2).
    let support = consensus.support;
    let min_support = fp.min_support as f64;
    let half_support = (fp.min_support.div_ceil(2)) as f64;

    let classification = if corr_stats.hard_ceiling_exceeded {
        // §9.3 GENUINE resource ceiling exceeded (grouped > hard_correspondence_ceiling, FAR above the
        // §9.2 sample cap) ⇒ Ambiguous, never NoMatch (telemetry in corr_stats, §20). NOTE: the
        // ROUTINE §9.2 sample cap (`corr_stats.capped`) does NOT land here — its representative subset
        // is scored NORMALLY below. Conflating the two forced every long dense pair to Ambiguous (the
        // boundary-12 recall bug); they are now separate signals.
        MatchClassification::Ambiguous
    } else {
        match consensus.classification {
            AffineClass::Match => MatchClassification::Match,
            // No fittable hypothesis at all ⇒ NoMatch (fingerprints present, no affine line).
            AffineClass::NoMatch => MatchClassification::NoMatch,
            AffineClass::Ambiguous => {
                // A fittable line exists but a §10.6 gate failed. §9.3 below-support branch:
                // Ambiguous iff in the [ceil(min_support/2), min_support) band with a
                // non-equivalent runner-up that depresses the margin below min_score_margin.
                let in_band = support < min_support && support >= half_support;
                let has_runner_up = consensus.runner_up_score > 0.0;
                let margin_below = consensus.score_margin < fp.min_score_margin;
                if in_band && has_runner_up && margin_below {
                    MatchClassification::Ambiguous
                } else {
                    MatchClassification::NoMatch
                }
            }
        }
    };

    // The affine `support` is an unweighted count clamped to u32; weighted `support` (the gate
    // lever) is exposed via `score`/quality only. We report the unweighted `inlier_count` as the
    // method-neutral `support` field (§5.1 "support u32").
    PairEvidence {
        method: MatchMethod::SpectralLandmarkV1,
        classification,
        lag_ms: finite_or_zero(consensus.lag_ms),
        drift_ppm: finite_or_zero(consensus.drift_ppm),
        score: finite_or_zero(consensus.score),
        runner_up_score: finite_or_zero(consensus.runner_up_score),
        score_margin: finite_or_zero(consensus.score_margin),
        support: consensus.inlier_count,
        candidate_correspondences: consensus.candidate_correspondences,
        inlier_fraction: finite_or_zero(consensus.inlier_fraction),
        temporal_coverage: finite_or_zero(consensus.temporal_coverage),
        // The consensus residual Options already encode "no fittable hypothesis ⇒ None"; pass them
        // through but scrub any non-finite to None so JSON never carries NaN/Inf (§20).
        residual_rms_ms: finite_opt(consensus.residual_rms_ms),
        residual_p95_ms: finite_opt(consensus.residual_p95_ms),
        overlap_s: finite_nonneg(consensus.overlap_s),
        refined_lag_ms: None,
        refined_drift_ppm: None,
        refined_rms_ms: None,
        legacy,
        // Boundary 7 sets this when it refines the cell in the Ctx fingerprint path; the pure
        // consensus wrapper itself does not run the local aligner (it has no pyramids).
        refine_reason: crate::match_refine::RefineReason::NotAttempted,
    }
}

/// Pass a finite value through, mapping any non-finite to `0.0`. Used on the always-present scalar
/// fields so a (defensive) NaN/Infinity from an upstream edge can never serialize (§20). Upstream
/// boundaries already guarantee finiteness; this is belt-and-suspenders at the contract boundary.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Pass a finite `Some` through; map a `None` OR a non-finite `Some` to `None`. This is how an
/// "absent residual" reaches JSON `null` and how a stray non-finite can never become a serialized
/// NaN/Infinity (§5.1, §20).
fn finite_opt(v: Option<f64>) -> Option<f64> {
    match v {
        Some(x) if x.is_finite() => Some(x),
        _ => None,
    }
}

/// The candidate overlap in seconds for two tracks of durations `dur_a` / `dur_b`, used for the
/// §9.3 `InsufficientOverlap` gate and as the boundary-5 temporal-coverage denominator.
///
/// This is the LAG-INDEPENDENT, maximally-aligned content overlap `min(dur_a, dur_b)`: it answers
/// "could these two possibly overlap by the pairing floor at all" — exactly the question the
/// legacy `pair_correlate` `insufficient_overlap` also answers (which compares each track's length
/// to the floor, independent of lag). A timestamp is NEVER used as a search prior (§20); this is a
/// duration arithmetic only. Always finite & non-negative.
pub fn candidate_overlap_s(dur_a: f64, dur_b: f64) -> f64 {
    finite_nonneg(dur_a.min(dur_b))
}

/// Per-cell fingerprint availability for a batch (§9.3). A track contributes its `FingerprintReader`
/// when one could be built/loaded; `None` means the fingerprint is UNAVAILABLE and the cell falls
/// back to the legacy matcher (§9.3 "fingerprint unavailable but legacy matcher works ⇒
/// EnvelopeV1").
pub type FingerprintSlot<'a> = Option<&'a FingerprintReader>;

/// Telemetry for one batch `match_candidates` run, proving the §5.3 one-index design. `index_builds`
/// counts how many times the audio-side [`AudioHashIndex`] was constructed for the whole batch: it
/// MUST be exactly 1 for a fingerprint batch (and 0 for a pure-legacy batch, which builds no
/// fingerprints — §5.2). A naive per-cell matcher would make this `videos.len() · audios.len()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MatchBatchStats {
    /// Number of audio-side inverted indexes built for the whole batch. `1` for a fingerprint
    /// batch with at least one fingerprint-available audio; `0` for legacy / no-fingerprint.
    pub index_builds: u32,
}

/// Pure batch orchestrator for the fingerprint modes (§5.3). Builds EXACTLY ONE audio-side
/// [`AudioHashIndex`] for the whole batch, then fills the `videos × audios` evidence matrix from
/// it. This is the function `Ctx::match_candidates` delegates to so the one-index property is a
/// structural fact (the build is OUTSIDE both loops), and so the orchestration is unit-testable on
/// hand-built readers without the PCM/ffmpeg pipeline.
///
/// Inputs per side: `(FingerprintSlot, duration_s)`. A `None` slot ⇒ that track's cells fall back
/// to the legacy matcher. `legacy` provides the legacy [`PairCorrelation`] for cell `(vi, ai)` —
/// computed lazily by the caller only when needed (a fallback, or shadow attachment), so a normal
/// fingerprint cell pays no legacy cost. `shadow` (true for [`MatcherMode::FingerprintShadow`])
/// controls whether the legacy correlation is embedded into `PairEvidence.legacy` for comparison.
///
/// `refine` is the BOUNDARY-7 (§11) fine-refinement hook: for a spectral cell `(vi, ai)` that the
/// consensus classified `Match` or a fittable (strong) `Ambiguous`, the orchestrator calls
/// `refine(vi, ai, lag_ms, drift_ppm, inlier_t_a_min, inlier_t_a_max)` and the caller runs the
/// existing local aligner (`match_refine::refine_affine`) over the two PYRAMIDS it holds — the pure
/// orchestrator has no pyramids, so the measurement is injected, exactly like `legacy`. The returned
/// [`RefineOutcome`] populates `refined_*`; a refinement FAILURE on a `Match` downgrades it to
/// `Ambiguous` (§11.2 — landmark-only acceptance is not yet corpus-proven safe). A caller with no
/// pyramids (the unit tests) passes a closure returning `RefineOutcome::failed(NotAttempted)` to
/// skip refinement.
///
/// `hint` is the BOUNDARY-10 (§12) metadata hook: for a spectral cell it returns the per-pair
/// [`TimeHint`] (parsed from the two tracks' container/BWF metadata by the caller, which holds the
/// `MediaInfo`; the pure orchestrator does not). It is threaded into [`consensus_with_hint`] where it
/// MAY seed one scale-1 hypothesis and break an exact audio tie (§12.3) — but NEVER restricts the
/// landmark lookup (the correspondences are computed BEFORE the hint is consulted), lowers a
/// threshold, or overrides a better audio hypothesis. A caller with no metadata (the unit tests)
/// passes `|_, _| None`, making the cell byte-identical to the no-hint consensus.
///
/// Returns the matrix and [`MatchBatchStats`] (with `index_builds == 1` whenever at least one
/// audio has a fingerprint; `0` when none do, i.e. every cell is a legacy fallback).
// The parameter count is intrinsic to the design: the pure orchestrator owns the batch index +
// §9.3 mapping, and the three per-cell DEPENDENCIES it cannot have itself (the legacy correlation,
// the §11 pyramid refinement, and the §12 metadata hint) are INJECTED as closures so the function
// is unit-testable without the PCM/ffmpeg pipeline. Bundling them into a struct would only obscure
// that. (`Ctx::match_pair`/`match_candidates` are the two production callers.)
#[allow(clippy::too_many_arguments)]
pub fn match_candidates_fingerprint<F, R, H>(
    videos: &[(FingerprintSlot, f64)],
    audios: &[(FingerprintSlot, f64)],
    fmt: &FingerprintFormatV1,
    params: &PairingParams,
    shadow: bool,
    mut legacy: F,
    mut refine: R,
    mut hint: H,
) -> (Vec<Vec<PairEvidence>>, MatchBatchStats)
where
    F: FnMut(usize, usize) -> Option<PairCorrelation>,
    R: FnMut(usize, usize, f64, f64, f64, f64) -> crate::match_refine::RefineOutcome,
    H: FnMut(usize, usize) -> Option<TimeHint>,
{
    let fp_params = &params.fingerprint_match;
    let mut stats = MatchBatchStats::default();

    // Build the audio-side index ONCE for the batch (§5.3 / §9.1). Only fingerprint-available
    // audios are indexed; the index's `audio_index` is the position within `present_audio_readers`,
    // which we map back to the original column via `present_audio_cols`.
    let mut present_audio_readers: Vec<&FingerprintReader> = Vec::new();
    let mut present_audio_cols: Vec<usize> = Vec::new();
    for (ai, (slot, _)) in audios.iter().enumerate() {
        if let Some(reader) = slot {
            present_audio_cols.push(ai);
            present_audio_readers.push(reader);
        }
    }
    // The ONE index build for the whole V×A batch (or none if no audio has a fingerprint).
    let index = if present_audio_readers.is_empty() {
        None
    } else {
        stats.index_builds = 1;
        let (idx, _idx_stats) = AudioHashIndex::build(&present_audio_readers, fp_params);
        Some(idx)
    };
    // Inverse map: original audio column -> its `audio_index` within the index (if present).
    let mut audio_col_to_index: Vec<Option<u32>> = vec![None; audios.len()];
    for (k, &col) in present_audio_cols.iter().enumerate() {
        audio_col_to_index[col] = Some(k as u32);
    }

    let mut matrix: Vec<Vec<PairEvidence>> = Vec::with_capacity(videos.len());
    for (vi, (v_slot, v_dur)) in videos.iter().enumerate() {
        let mut row: Vec<PairEvidence> = Vec::with_capacity(audios.len());
        for (ai, (_a_slot, a_dur)) in audios.iter().enumerate() {
            let overlap = candidate_overlap_s(*v_dur, *a_dur);

            // §9.3 #1: below the pairing floor ⇒ InsufficientOverlap, before any matcher runs.
            if overlap < params.min_overlap_s {
                row.push(PairEvidence::insufficient_overlap(MatchMethod::SpectralLandmarkV1, overlap));
                continue;
            }

            // §9.3 #2: a fingerprint is UNAVAILABLE (this video OR this audio) ⇒ legacy fallback.
            let (Some(video_fp), Some(audio_idx), Some(idx)) =
                (v_slot.as_ref(), audio_col_to_index[ai], index.as_ref())
            else {
                row.push(legacy_fallback(&mut legacy, vi, ai, overlap, shadow));
                continue;
            };

            // Fingerprints present for both sides: correspondences against the ONE batch index,
            // then boundary-5 consensus, then the §9.3 mapping. §20/§12.3: the landmark lookup +
            // correspondences are computed HERE, BEFORE the metadata hint is consulted — the hint
            // never restricts the index or the correspondence set, it only enters consensus as one
            // more candidate hypothesis + an exact-tie order key.
            let pc = correspondences_for_pair(video_fp, idx, audio_idx, fmt, fp_params);
            let cell_hint = hint(vi, ai);
            let cons = consensus_with_hint(&pc.correspondences, overlap, fp_params, cell_hint.as_ref());
            // In shadow mode attach the legacy correlation (if the caller can supply one) for
            // comparison; in primary mode leave it None (§5.3). The legacy closure is only invoked
            // when shadow is on, so a primary fingerprint cell pays no legacy cost.
            let legacy_attach = if shadow { legacy(vi, ai) } else { None };
            let mut ev = evidence_from_consensus(&cons, &pc.stats, params, legacy_attach);
            // BOUNDARY 7 (§11.1/§11.2): fine-refine the accepted hypothesis using the existing local
            // aligner. The §11 refinement is meaningful only when the consensus produced a fittable
            // line (residuals present ⇒ an inlier A-span to anchor across), and only for a Match or a
            // strong (fittable) Ambiguous — the §9.3 cases below do not refine:
            //   - InsufficientOverlap / a capped resource ceiling: handled above / not a hypothesis;
            //   - a plain NoMatch (no fittable line): nothing to refine.
            apply_refinement(&mut ev, &cons, vi, ai, &mut refine);
            row.push(ev);
        }
        matrix.push(row);
    }

    (matrix, stats)
}

/// Apply the §11 fine refinement to a spectral cell's evidence (BOUNDARY 7). Eligibility:
///
/// - the evidence is `Match` or `Ambiguous` (a `NoMatch` / `InsufficientOverlap` / `Unavailable`
///   has no accepted hypothesis to refine);
/// - the consensus exposed an inlier A-span (`inlier_t_a_min/max` Some) — i.e. a fittable line.
///
/// For an eligible cell the `refine` closure runs the existing local aligner over the pyramids; on
/// SUCCESS the `refined_*` fields are populated from its fit (§11.2) and `refine_reason = Refined`.
/// On FAILURE the landmark identity evidence is retained but a `Match` is downgraded to `Ambiguous`
/// (§11.2 — a refinement failure must never be a confident Match); an already-`Ambiguous` cell keeps
/// its class. The failure reason is recorded in `refine_reason` (debug telemetry). A non-eligible
/// cell is left exactly as the §9.3 mapping produced it, with `refine_reason = NotAttempted`.
fn apply_refinement<R>(
    ev: &mut PairEvidence,
    cons: &AffineConsensus,
    vi: usize,
    ai: usize,
    refine: &mut R,
) where
    R: FnMut(usize, usize, f64, f64, f64, f64) -> crate::match_refine::RefineOutcome,
{
    use crate::match_refine::RefineReason;

    // Only Match / Ambiguous carry an accepted (or candidate) hypothesis worth refining.
    let eligible = matches!(
        ev.classification,
        MatchClassification::Match | MatchClassification::Ambiguous
    );
    let (Some(t_min), Some(t_max)) = (cons.inlier_t_a_min, cons.inlier_t_a_max) else {
        // No fittable line (a NoMatch consensus, or a resource-ceiling Ambiguous with no model) ⇒
        // nothing to refine; leave the evidence and reason untouched (NotAttempted).
        return;
    };
    if !eligible {
        return;
    }

    let outcome = refine(vi, ai, cons.lag_ms, cons.drift_ppm, t_min, t_max);
    ev.refine_reason = outcome.reason;
    if outcome.is_refined() {
        // §11.2: the refined mapping replaces the coarse consensus map for downstream coverage /
        // full sync. Fields are already finite (refine_affine scrubs); pass through.
        ev.refined_lag_ms = outcome.refined_lag_ms;
        ev.refined_drift_ppm = outcome.refined_drift_ppm;
        ev.refined_rms_ms = outcome.refined_rms_ms;
    } else if outcome.reason != RefineReason::NotAttempted
        && ev.classification == MatchClassification::Match
    {
        // §11.2: a refinement that was ATTEMPTED and FAILED must NOT remain a confident Match.
        // Retain the landmark identity evidence (lag/support/etc) but downgrade to Ambiguous; the
        // reason is in refine_reason. (An already-Ambiguous cell stays Ambiguous with refined_*
        // None.) A `NotAttempted` outcome — the refinement was deliberately skipped, e.g. a caller
        // with no pyramids — is NOT a failure and leaves the classification intact.
        ev.classification = MatchClassification::Ambiguous;
    }
}

/// Produce the §9.3 `EnvelopeV1` fallback evidence for a cell whose fingerprint is unavailable. The
/// caller's `legacy(vi, ai)` supplies the `PairCorrelation`; if it cannot (the legacy matcher
/// itself produced nothing), the cell is `Unavailable` (absence of evidence, not a `NoMatch`). The
/// raw correlation is attached for transparency — a fallback always records the legacy result it
/// used, since the spectral path could not run (the method already reads `EnvelopeV1`).
fn legacy_fallback<F>(
    legacy: &mut F,
    vi: usize,
    ai: usize,
    overlap: f64,
    _shadow: bool,
) -> PairEvidence
where
    F: FnMut(usize, usize) -> Option<PairCorrelation>,
{
    match legacy(vi, ai) {
        // A fallback always attaches the legacy correlation it used: it IS the evidence here, and
        // the method already says EnvelopeV1, so the embed is the transparency record (§9.3 +
        // §16.2 "method and reason must be visible").
        Some(corr) => evidence_from_legacy(&corr, overlap, true),
        // The legacy matcher could not produce a correlation either ⇒ the matcher was unavailable.
        None => PairEvidence::unavailable(MatchMethod::EnvelopeV1, overlap),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::landmarks::Landmark;
    use crate::fingerprint::AffineModel;
    use crate::pairing_params::MatcherMode;

    // ---- helpers ----

    /// Build a `FingerprintReader` directly from a landmark list (the reader re-sorts) for a
    /// duration, bypassing the signal pipeline — same approach as the `fingerprint::index` unit
    /// tests, so the batch orchestrator can be driven on hand-crafted hashes/frames without ffmpeg.
    fn reader(mut lms: Vec<Landmark>, duration_frames: u64) -> FingerprintReader {
        lms.sort();
        let fmt = FingerprintFormatV1::fixed();
        FingerprintReader {
            analysis_sample_rate: fmt.analysis_sample_rate,
            fft_size: fmt.fft_size as u32,
            hop_size: fmt.hop_size as u32,
            min_frequency_millihz: (fmt.min_frequency_hz * 1000.0).round() as u32,
            max_frequency_millihz: (fmt.max_frequency_hz * 1000.0).round() as u32,
            params_hash: fmt.params_hash(),
            duration_frames,
            landmarks: lms,
        }
    }

    fn lm(hash: u32, anchor_frame: u32) -> Landmark {
        Landmark { hash, anchor_frame }
    }

    /// A pair of readers that share a rich set of hashes on a clean integer-frame line (B is OFF
    /// frames earlier than A), so boundary-5 consensus has a decisive line to fit. Returns
    /// (video, audio). `n` distinct hashes anchored frames `[OFF, OFF+n)` in A and `[0, n)` in B.
    fn same_master_pair(off: u32, n: u32) -> (FingerprintReader, FingerprintReader) {
        let mut v = Vec::new();
        let mut a = Vec::new();
        for f in off..off + n {
            let h = 5000 + f;
            v.push(lm(h, f));
            a.push(lm(h, f - off));
        }
        // Durations comfortably exceed the anchors so no anchor is out of range.
        (reader(v, (off + n + 10) as u64), reader(a, (n + 10) as u64))
    }

    fn legacy_corr(z: f64, lag_ms: f64, legacy_candidate: bool) -> PairCorrelation {
        // Build a PairCorrelation directly (all public fields) for the legacy wrapper tests.
        PairCorrelation {
            z,
            lag_ms,
            ncc: 0.5,
            fine_ncc: Some(0.5),
            overlap_bins: 100,
            runner_up_z: 1.0,
            refined: true,
            insufficient_overlap: false,
            legacy_candidate,
            prefilter_candidate: true,
        }
    }

    fn consensus_match() -> AffineConsensus {
        AffineConsensus {
            classification: AffineClass::Match,
            model: AffineModel { intercept_s: 1.25, scale: 1.000_08 },
            lag_ms: 1250.0,
            drift_ppm: 80.0,
            score: 5.0,
            runner_up_score: 0.0,
            score_margin: 1.0,
            support: 50.0,
            inlier_count: 50,
            candidate_correspondences: 60,
            inlier_fraction: 0.9,
            temporal_coverage: 0.8,
            residual_rms_ms: Some(0.4),
            residual_p95_ms: Some(0.9),
            overlap_s: 40.0,
            inlier_t_a_min: Some(0.5),
            inlier_t_a_max: Some(39.5),
        }
    }

    /// `CorrespondenceStats` with the ROUTINE §9.2 sample cap flag (`capped`) set as requested and
    /// the GENUINE §9.3 resource ceiling (`hard_ceiling_exceeded`) OFF. Used to prove that the
    /// routine sample cap alone is scored NORMALLY (no Ambiguous override) — the §9.2/§9.3 separation.
    fn stats(capped: bool) -> CorrespondenceStats {
        CorrespondenceStats { capped, hard_ceiling_exceeded: false, ..Default::default() }
    }

    /// `CorrespondenceStats` for the GENUINE §9.3 resource ceiling: `hard_ceiling_exceeded` ON (and
    /// `capped` ON too, since a pair past the hard ceiling is necessarily past the §9.2 sample cap).
    fn stats_hard_ceiling() -> CorrespondenceStats {
        CorrespondenceStats { capped: true, hard_ceiling_exceeded: true, ..Default::default() }
    }

    /// A JSON string must contain NO NaN/Infinity token (serde_json emits `null` for non-finite
    /// f64, which would also be wrong here — we assert neither path occurs for a clean evidence).
    fn assert_no_nan_inf(json: &str) {
        for bad in ["NaN", "Infinity", "inf", "nan"] {
            assert!(!json.contains(bad), "evidence JSON must not contain {bad:?}: {json}");
        }
    }

    // ---- §5.1 serde round-trip + None residual → null + no NaN/Inf + finite score ----

    #[test]
    fn evidence_round_trip_and_null_residuals() {
        // A Match with residuals present.
        let ev = evidence_from_consensus(&consensus_match(), &stats(false), &PairingParams::default(), None);
        let json = serde_json::to_string(&ev).unwrap();
        assert_no_nan_inf(&json);
        // camelCase on the wire; residuals serialized as numbers (present).
        assert!(json.contains("\"method\":\"spectralLandmarkV1\""), "wire: {json}");
        assert!(json.contains("\"classification\":\"match\""));
        assert!(json.contains("\"residualRmsMs\":0.4"));
        // refined_* are None ⇒ null.
        assert!(json.contains("\"refinedLagMs\":null"), "refined fields null: {json}");
        assert!(json.contains("\"refinedDriftPpm\":null"));
        assert!(json.contains("\"refinedRmsMs\":null"));
        let back: PairEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back, "PairEvidence must round-trip through JSON");
    }

    #[test]
    fn none_residuals_serialize_as_null_and_score_finite_when_unavailable() {
        // Unavailable evidence: score 0.0 (finite), residuals null.
        let ev = PairEvidence::unavailable(MatchMethod::SpectralLandmarkV1, 12.0);
        assert_eq!(ev.score, 0.0);
        assert!(ev.score.is_finite());
        let json = serde_json::to_string(&ev).unwrap();
        assert_no_nan_inf(&json);
        assert!(json.contains("\"classification\":\"unavailable\""));
        assert!(json.contains("\"residualRmsMs\":null") && json.contains("\"residualP95Ms\":null"));
        assert!(json.contains("\"legacy\":null"));
        let back: PairEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    // A NoMatch consensus has None residuals (boundary 5 returns them None); evidence must keep
    // them None ⇒ null, and never synthesize a residual.
    #[test]
    fn no_match_consensus_keeps_null_residuals() {
        let mut c = consensus_match();
        c.classification = AffineClass::NoMatch;
        c.residual_rms_ms = None;
        c.residual_p95_ms = None;
        c.score = 0.0;
        let ev = evidence_from_consensus(&c, &stats(false), &PairingParams::default(), None);
        assert_eq!(ev.classification, MatchClassification::NoMatch);
        assert_eq!(ev.residual_rms_ms, None);
        assert_eq!(ev.residual_p95_ms, None);
        let json = serde_json::to_string(&ev).unwrap();
        assert_no_nan_inf(&json);
    }

    // Defensive: a non-finite leaking into the consensus is scrubbed to 0.0 / None before serde,
    // so no NaN/Infinity ever reaches JSON (§20). (Upstream guarantees finiteness; this proves the
    // contract boundary also enforces it.)
    #[test]
    fn non_finite_inputs_are_scrubbed() {
        let mut c = consensus_match();
        c.lag_ms = f64::NAN;
        c.score = f64::INFINITY;
        c.inlier_fraction = f64::NAN;
        c.residual_rms_ms = Some(f64::INFINITY);
        let ev = evidence_from_consensus(&c, &stats(false), &PairingParams::default(), None);
        assert_eq!(ev.lag_ms, 0.0);
        assert_eq!(ev.score, 0.0);
        assert_eq!(ev.inlier_fraction, 0.0);
        assert_eq!(ev.residual_rms_ms, None, "non-finite residual scrubbed to None");
        let json = serde_json::to_string(&ev).unwrap();
        assert_no_nan_inf(&json);
    }

    // ---- §5.3 legacy wrapper ----

    #[test]
    fn legacy_wrapper_match_and_no_match() {
        // legacy_candidate true ⇒ Match; lag/score passed through; method EnvelopeV1.
        let ev = evidence_from_legacy(&legacy_corr(9.0, 1500.0, true), 30.0, false);
        assert_eq!(ev.method, MatchMethod::EnvelopeV1);
        assert_eq!(ev.classification, MatchClassification::Match);
        assert_eq!(ev.lag_ms, 1500.0);
        assert_eq!(ev.score, 9.0);
        assert_eq!(ev.drift_ppm, 0.0, "legacy stage-1 carries no drift");
        assert_eq!(ev.legacy, None, "pure-legacy does not duplicate into legacy field");

        // legacy_candidate false ⇒ NoMatch.
        let ev2 = evidence_from_legacy(&legacy_corr(2.0, 100.0, false), 30.0, false);
        assert_eq!(ev2.classification, MatchClassification::NoMatch);
    }

    #[test]
    fn legacy_wrapper_insufficient_overlap_and_attach() {
        let mut corr = legacy_corr(0.0, 0.0, false);
        corr.insufficient_overlap = true;
        // attach_legacy true ⇒ embed the PairCorrelation for shadow comparison.
        let ev = evidence_from_legacy(&corr, 5.0, true);
        assert_eq!(ev.classification, MatchClassification::InsufficientOverlap);
        assert!(ev.legacy.is_some(), "shadow fallback attaches the legacy correlation");
        assert_eq!(ev.overlap_s, 5.0);
    }

    // ---- §9.3 classification mapping over the affine consensus ----

    #[test]
    fn consensus_match_maps_to_match() {
        let ev = evidence_from_consensus(&consensus_match(), &stats(false), &PairingParams::default(), None);
        assert_eq!(ev.classification, MatchClassification::Match);
        assert_eq!(ev.lag_ms, 1250.0);
        assert!((ev.drift_ppm - 80.0).abs() < 1e-9);
        assert_eq!(ev.support, 50);
        assert_eq!(ev.candidate_correspondences, 60);
    }

    #[test]
    fn consensus_no_hypothesis_maps_to_no_match() {
        // AffineClass::NoMatch (no fittable hypothesis) ⇒ NoMatch, residuals None.
        let mut c = consensus_match();
        c.classification = AffineClass::NoMatch;
        c.support = 0.0;
        c.inlier_count = 0;
        c.score = 0.0;
        c.residual_rms_ms = None;
        c.residual_p95_ms = None;
        let ev = evidence_from_consensus(&c, &stats(false), &PairingParams::default(), None);
        assert_eq!(ev.classification, MatchClassification::NoMatch);
    }

    #[test]
    fn consensus_below_support_with_runner_up_is_ambiguous() {
        // In the [ceil(min_support/2), min_support) band with a non-equivalent runner-up and a
        // margin below min_score_margin ⇒ Ambiguous (§9.3 repeated-content case).
        let mut params = PairingParams::default();
        params.fingerprint_match.min_support = 12; // band [6, 12)
        params.fingerprint_match.min_score_margin = 0.5;
        let mut c = consensus_match();
        c.classification = AffineClass::Ambiguous;
        c.support = 8.0; // in band
        c.inlier_count = 8;
        c.runner_up_score = 4.0; // a real non-equivalent rival
        c.score = 5.0;
        c.score_margin = 0.2; // < 0.5 ⇒ below the margin
        let ev = evidence_from_consensus(&c, &stats(false), &params, None);
        assert_eq!(ev.classification, MatchClassification::Ambiguous);
    }

    #[test]
    fn consensus_below_support_without_runner_up_is_no_match() {
        // Same band but NO non-equivalent runner-up ⇒ NoMatch (not Ambiguous).
        let mut params = PairingParams::default();
        params.fingerprint_match.min_support = 12;
        params.fingerprint_match.min_score_margin = 0.5;
        let mut c = consensus_match();
        c.classification = AffineClass::Ambiguous;
        c.support = 8.0;
        c.inlier_count = 8;
        c.runner_up_score = 0.0; // no rival
        c.score = 5.0;
        c.score_margin = 1.0;
        let ev = evidence_from_consensus(&c, &stats(false), &params, None);
        assert_eq!(ev.classification, MatchClassification::NoMatch);
    }

    #[test]
    fn consensus_far_below_support_is_no_match() {
        // Below ceil(min_support/2) (out of the band) ⇒ NoMatch even with a runner-up.
        let mut params = PairingParams::default();
        params.fingerprint_match.min_support = 12; // band lower edge 6
        params.fingerprint_match.min_score_margin = 0.5;
        let mut c = consensus_match();
        c.classification = AffineClass::Ambiguous;
        c.support = 3.0; // below 6 ⇒ out of band
        c.inlier_count = 3;
        c.runner_up_score = 4.0;
        c.score = 5.0;
        c.score_margin = 0.1;
        let ev = evidence_from_consensus(&c, &stats(false), &params, None);
        assert_eq!(ev.classification, MatchClassification::NoMatch);
    }

    // ---- §9.3 / §20: separation of the routine §9.2 sample cap from the genuine resource ceiling ----

    // BOUNDARY TEST (1) — the conflation regression guard. A dense pair whose ROUTINE §9.2 sample cap
    // fired (`capped`) but whose SAMPLED consensus is a clean all-gates-passing Match must classify
    // `Match`, NOT be forced Ambiguous. This is the boundary-12 recall bug fixed: `capped` alone is a
    // SAMPLING signal, scored normally — only the genuine ceiling forces Ambiguous.
    #[test]
    fn routine_sample_cap_is_scored_normally_not_ambiguous() {
        // A capped-but-all-gates-passing dense pair ⇒ Match (the §9.2 sample's consensus is a Match).
        let ev = evidence_from_consensus(&consensus_match(), &stats(true), &PairingParams::default(), None);
        assert_eq!(
            ev.classification,
            MatchClassification::Match,
            "a capped (§9.2 sampled) pair with a Match consensus must Match, NOT be forced Ambiguous"
        );

        // And a capped sample whose consensus is a plain NoMatch is a NORMAL NoMatch (the routine cap
        // does not upgrade it to Ambiguous — that override is reserved for the genuine ceiling).
        let mut c = consensus_match();
        c.classification = AffineClass::NoMatch;
        c.support = 0.0;
        c.inlier_count = 0;
        let ev2 = evidence_from_consensus(&c, &stats(true), &PairingParams::default(), None);
        assert_eq!(ev2.classification, MatchClassification::NoMatch, "a capped NoMatch stays NoMatch");
    }

    // BOUNDARY TEST (4) — the genuine resource ceiling still ⇒ Ambiguous, NEVER NoMatch (§9.3/§20).
    #[test]
    fn genuine_resource_ceiling_forces_ambiguous_never_no_match() {
        // Even when the affine verdict is NoMatch, the GENUINE ceiling (hard_ceiling_exceeded) ⇒
        // Ambiguous (telemetry in corr_stats).
        let mut c = consensus_match();
        c.classification = AffineClass::NoMatch;
        c.support = 0.0;
        c.inlier_count = 0;
        let ev = evidence_from_consensus(&c, &stats_hard_ceiling(), &PairingParams::default(), None);
        assert_eq!(
            ev.classification,
            MatchClassification::Ambiguous,
            "a hit GENUINE resource ceiling must be Ambiguous, never NoMatch (§20)"
        );

        // And it also overrides a would-be Match (a truly-exhausted input is not confidently unique).
        let ev2 = evidence_from_consensus(&consensus_match(), &stats_hard_ceiling(), &PairingParams::default(), None);
        assert_eq!(ev2.classification, MatchClassification::Ambiguous);
    }

    // ---- BOUNDARY 7 (§11.2): apply_refinement policy over the orchestrator ----

    use crate::match_refine::{RefineOutcome, RefineReason};

    // A SpectralLandmarkV1 Match cell whose refinement SUCCEEDS gets refined_* populated and stays a
    // Match; the refine closure is invoked with the consensus lag/drift + the inlier A-span (so the
    // §11.1 anchor span is threaded through from the consensus, not the global scan).
    #[test]
    fn refinement_success_populates_refined_and_keeps_match() {
        let mut params =
            PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
        params.fingerprint_match.min_support = 12;
        params.fingerprint_match.min_temporal_coverage = 0.1;
        params.min_overlap_s = 1.0;
        let fmt = FingerprintFormatV1::fixed();
        let (v0, a0) = same_master_pair(120, 200); // a clean Match cell
        let v0_dur = v0.duration_frames as f64 * fmt.frame_period_s();
        let a0_dur = a0.duration_frames as f64 * fmt.frame_period_s();
        let videos = [(Some(&v0), v0_dur)];
        let audios = [(Some(&a0), a0_dur)];

        // The refine closure asserts it received a fittable inlier span (t_max > t_min, finite) and
        // returns a SUCCESS with a refined lag 1 ms off the consensus lag.
        let mut refine_calls = 0;
        let (matrix, _) = match_candidates_fingerprint(
            &videos,
            &audios,
            &fmt,
            &params,
            true,
            |_, _| None,
            |_, _, lag_ms, drift_ppm, t_min, t_max| {
                refine_calls += 1;
                assert!(t_max > t_min && t_min.is_finite() && t_max.is_finite(), "fittable span threaded");
                assert!(lag_ms.is_finite() && drift_ppm.is_finite());
                RefineOutcome {
                    refined_lag_ms: Some(lag_ms + 1.0),
                    refined_drift_ppm: Some(drift_ppm),
                    refined_rms_ms: Some(0.3),
                    reason: RefineReason::Refined,
                }
            },
            |_, _| None,
        );
        let cell = &matrix[0][0];
        assert_eq!(cell.classification, MatchClassification::Match, "a refined Match stays a Match");
        assert_eq!(cell.refine_reason, RefineReason::Refined);
        assert!(cell.refined_lag_ms.is_some(), "refined_lag_ms populated on success");
        assert_eq!(cell.refined_rms_ms, Some(0.3));
        assert_eq!(refine_calls, 1, "refinement runs exactly once for the one Match cell");
    }

    // §11.2 FAILURE policy: a Match cell whose refinement FAILS (an attempted failure reason) is
    // DOWNGRADED to Ambiguous (NOT NoMatch), keeps refined_* = None, and records the telemetry
    // reason. It must NOT remain a confident Match.
    #[test]
    fn refinement_failure_downgrades_match_to_ambiguous() {
        let mut params =
            PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
        params.fingerprint_match.min_support = 12;
        params.fingerprint_match.min_temporal_coverage = 0.1;
        params.min_overlap_s = 1.0;
        let fmt = FingerprintFormatV1::fixed();
        let (v0, a0) = same_master_pair(120, 200);
        let v0_dur = v0.duration_frames as f64 * fmt.frame_period_s();
        let a0_dur = a0.duration_frames as f64 * fmt.frame_period_s();
        let videos = [(Some(&v0), v0_dur)];
        let audios = [(Some(&a0), a0_dur)];

        // Sanity: with NO refinement the cell is a confident Match (the thing we will downgrade).
        let (base, _) = match_candidates_fingerprint(
            &videos, &audios, &fmt, &params, true, |_, _| None,
            |_, _, _, _, _, _| RefineOutcome::not_attempted(),
            |_, _| None,
        );
        assert_eq!(base[0][0].classification, MatchClassification::Match, "baseline is a Match");

        // Now the refinement FAILS (the local measurements did not fit a line).
        let (matrix, _) = match_candidates_fingerprint(
            &videos, &audios, &fmt, &params, true, |_, _| None,
            |_, _, _, _, _, _| RefineOutcome::failed(RefineReason::LineFitFailed),
            |_, _| None,
        );
        let cell = &matrix[0][0];
        assert_eq!(
            cell.classification,
            MatchClassification::Ambiguous,
            "a refinement failure on a Match ⇒ Ambiguous, never NoMatch (§11.2)"
        );
        assert_ne!(cell.classification, MatchClassification::NoMatch, "must NOT be NoMatch");
        assert_eq!(cell.refined_lag_ms, None, "failed refinement leaves refined_* None");
        assert_eq!(cell.refine_reason, RefineReason::LineFitFailed, "telemetry reason recorded");
        // The landmark identity evidence is RETAINED (lag/support unchanged from the consensus).
        assert_eq!(cell.lag_ms, base[0][0].lag_ms, "identity evidence retained");
        assert_eq!(cell.support, base[0][0].support);
    }

    // A NoMatch cell is never eligible for refinement: the refine closure is NOT called and the
    // class/refined_* are untouched (refine_reason stays NotAttempted).
    #[test]
    fn refinement_not_attempted_for_no_match_cell() {
        let params =
            PairingParams { matcher_mode: MatcherMode::FingerprintShadow, min_overlap_s: 1.0, ..Default::default() };
        let fmt = FingerprintFormatV1::fixed();
        // Disjoint hash spaces ⇒ NoMatch.
        let v0 = reader((0..200u32).map(|k| lm(1_000 + k, k)).collect(), 220);
        let a0 = reader((0..200u32).map(|k| lm(80_000 + k, k)).collect(), 220);
        let v0_dur = v0.duration_frames as f64 * fmt.frame_period_s();
        let a0_dur = a0.duration_frames as f64 * fmt.frame_period_s();
        let videos = [(Some(&v0), v0_dur)];
        let audios = [(Some(&a0), a0_dur)];
        let (matrix, _) = match_candidates_fingerprint(
            &videos, &audios, &fmt, &params, true, |_, _| None,
            |_, _, _, _, _, _| panic!("refinement must NOT run for a NoMatch cell"),
            |_, _| None,
        );
        assert_eq!(matrix[0][0].classification, MatchClassification::NoMatch);
        assert_eq!(matrix[0][0].refine_reason, RefineReason::NotAttempted);
        assert_eq!(matrix[0][0].refined_lag_ms, None);
    }

    // ---- shadow legacy attachment on the spectral path ----

    #[test]
    fn shadow_attaches_legacy_correlation() {
        let legacy = legacy_corr(9.0, 1250.0, true);
        let ev = evidence_from_consensus(
            &consensus_match(),
            &stats(false),
            &PairingParams::default(),
            Some(legacy.clone()),
        );
        assert_eq!(ev.method, MatchMethod::SpectralLandmarkV1);
        assert!(ev.legacy.is_some(), "shadow mode attaches the legacy correlation for comparison");
        let json = serde_json::to_string(&ev).unwrap();
        assert_no_nan_inf(&json);
        // Round-trips with the embedded legacy correlation.
        let back: PairEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    // ---- §5.3 batch orchestrator: ONE index, V×A matrix, fallback, overlap ----

    // A 2×3 fingerprint batch builds EXACTLY ONE audio index (not per-cell), returns a 2×3 matrix,
    // and recovers each true pair as a SpectralLandmarkV1 Match with the correct lag sign.
    #[test]
    fn batch_builds_one_index_and_matches_true_pairs() {
        let mut params =
            PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
        // Lower the support floor so the modest synthetic landmark sets clear Match (the gate
        // values are calibrated for real audio; these are clean hand-built lines).
        params.fingerprint_match.min_support = 12;
        params.fingerprint_match.min_temporal_coverage = 0.1;
        let fmt = FingerprintFormatV1::fixed();

        // Two videos, three audios. video0 ↔ audio1 share a master at +120 frames; video1 ↔ audio2
        // share a master at +60. The remaining cells are unrelated (disjoint hash spaces).
        let (v0, a1) = same_master_pair(120, 200);
        let (v1, a2) = same_master_pair(60, 200);
        // An unrelated audio0 with its own hash space.
        let a0 = reader((0..200u32).map(|k| lm(99_000 + k, k)).collect(), 220);
        // video durations long enough; reuse v0/v1's durations.
        let v0_dur = v0.duration_frames as f64 * fmt.frame_period_s();
        let v1_dur = v1.duration_frames as f64 * fmt.frame_period_s();
        let a0_dur = a0.duration_frames as f64 * fmt.frame_period_s();
        let a1_dur = a1.duration_frames as f64 * fmt.frame_period_s();
        let a2_dur = a2.duration_frames as f64 * fmt.frame_period_s();
        // Make overlaps comfortably exceed the 8 s floor: scale durations up by using frames that
        // map to >8 s. 220 frames ≈ 4.7 s, under the floor — so set min_overlap_s low for the test.
        params.min_overlap_s = 1.0;

        let videos = [(Some(&v0), v0_dur), (Some(&v1), v1_dur)];
        let audios = [(Some(&a0), a0_dur), (Some(&a1), a1_dur), (Some(&a2), a2_dur)];

        let (matrix, stats) =
            match_candidates_fingerprint(&videos, &audios, &fmt, &params, true, |_, _| None, |_, _, _, _, _, _| {
                crate::match_refine::RefineOutcome::not_attempted()
            }, |_, _| None);

        // THE one-index proof: a 2×3 batch builds the audio index exactly once.
        assert_eq!(stats.index_builds, 1, "batch must build ONE audio index, not per-cell");
        assert_eq!(matrix.len(), 2, "two video rows");
        assert!(matrix.iter().all(|r| r.len() == 3), "three audio columns each");

        // The true pairs are Matches with the correct lag sign (B earlier ⇒ negative lag).
        assert_eq!(matrix[0][1].classification, MatchClassification::Match, "v0↔a1 true pair");
        assert!(matrix[0][1].lag_ms < 0.0, "B started later in content ⇒ negative lag");
        assert_eq!(matrix[0][1].method, MatchMethod::SpectralLandmarkV1);
        assert_eq!(matrix[1][2].classification, MatchClassification::Match, "v1↔a2 true pair");

        // An unrelated cell (v0 vs a0, disjoint hashes) is NoMatch.
        assert_eq!(matrix[0][0].classification, MatchClassification::NoMatch, "v0↔a0 unrelated");

        // Determinism: a second run is byte-identical per cell.
        let (matrix2, _) =
            match_candidates_fingerprint(&videos, &audios, &fmt, &params, true, |_, _| None, |_, _, _, _, _, _| {
                crate::match_refine::RefineOutcome::not_attempted()
            }, |_, _| None);
        for (r1, r2) in matrix.iter().zip(&matrix2) {
            for (e1, e2) in r1.iter().zip(r2) {
                assert_eq!(
                    serde_json::to_string(e1).unwrap(),
                    serde_json::to_string(e2).unwrap(),
                    "match_candidates must be deterministic"
                );
            }
        }
    }

    // A cell whose fingerprint is unavailable falls back to the legacy matcher (EnvelopeV1). With a
    // legacy correlation supplied ⇒ EnvelopeV1 evidence; with none ⇒ Unavailable. The index is
    // still built only for the fingerprint-available audios.
    #[test]
    fn batch_fingerprint_unavailable_falls_back_to_legacy() {
        let params = PairingParams {
            matcher_mode: MatcherMode::FingerprintShadow,
            min_overlap_s: 1.0,
            ..Default::default()
        };
        let fmt = FingerprintFormatV1::fixed();

        let (v0, a0) = same_master_pair(50, 200);
        let v0_dur = v0.duration_frames as f64 * fmt.frame_period_s();
        let a0_dur = a0.duration_frames as f64 * fmt.frame_period_s();

        // video0 has NO fingerprint (None) ⇒ its only cell must fall back.
        let videos: [(FingerprintSlot, f64); 1] = [(None, v0_dur)];
        let audios = [(Some(&a0), a0_dur)];

        // Provide a legacy correlation ⇒ EnvelopeV1 fallback evidence.
        let legacy_corr = legacy_corr(9.0, 1234.0, true);
        let (matrix, stats) = match_candidates_fingerprint(
            &videos,
            &audios,
            &fmt,
            &params,
            true,
            |_, _| Some(legacy_corr.clone()),
            |_, _, _, _, _, _| crate::match_refine::RefineOutcome::not_attempted(),
            |_, _| None,
        );
        // One audio has a fingerprint, so the index is still built once (even though the only video
        // lacks one and every cell falls back).
        assert_eq!(stats.index_builds, 1);
        assert_eq!(matrix[0][0].method, MatchMethod::EnvelopeV1, "fallback to legacy method");
        assert_eq!(matrix[0][0].classification, MatchClassification::Match, "legacy_candidate ⇒ Match");
        assert!(matrix[0][0].legacy.is_some(), "fallback records the legacy correlation it used");

        // With NO legacy correlation available ⇒ Unavailable (absence of evidence, not NoMatch).
        let (matrix2, _) =
            match_candidates_fingerprint(&videos, &audios, &fmt, &params, true, |_, _| None, |_, _, _, _, _, _| {
                crate::match_refine::RefineOutcome::not_attempted()
            }, |_, _| None);
        assert_eq!(matrix2[0][0].classification, MatchClassification::Unavailable);
    }

    // A sub-`min_overlap_s` cell is InsufficientOverlap and never runs a matcher.
    #[test]
    fn batch_insufficient_overlap_cell() {
        // The default min_overlap_s (8 s) is the floor we exercise here.
        let params =
            PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
        assert_eq!(params.min_overlap_s, 8.0, "test relies on the 8 s default floor");
        let fmt = FingerprintFormatV1::fixed();

        let (v0, a0) = same_master_pair(50, 200); // ~4.7 s durations, under the 8 s floor
        let v0_dur = v0.duration_frames as f64 * fmt.frame_period_s();
        let a0_dur = a0.duration_frames as f64 * fmt.frame_period_s();
        assert!(v0_dur.min(a0_dur) < 8.0, "test setup: overlap below floor");

        let videos = [(Some(&v0), v0_dur)];
        let audios = [(Some(&a0), a0_dur)];
        // The legacy closure must NEVER be called for an InsufficientOverlap cell. Neither must the
        // refine closure (no accepted hypothesis exists to refine for a sub-floor cell).
        let (matrix, _) = match_candidates_fingerprint(
            &videos,
            &audios,
            &fmt,
            &params,
            true,
            |_, _| panic!("legacy matcher must not run for a sub-floor overlap cell"),
            |_, _, _, _, _, _| panic!("refinement must not run for a sub-floor overlap cell"),
            |_, _| panic!("metadata hint must not be consulted for a sub-floor overlap cell"),
        );
        assert_eq!(matrix[0][0].classification, MatchClassification::InsufficientOverlap);
        assert_eq!(matrix[0][0].overlap_s, candidate_overlap_s(v0_dur, a0_dur));
    }

    // A pure-legacy-fallback batch (NO audio has a fingerprint) builds ZERO indexes.
    #[test]
    fn batch_no_fingerprints_builds_zero_indexes() {
        let params = PairingParams {
            matcher_mode: MatcherMode::FingerprintShadow,
            min_overlap_s: 1.0,
            ..Default::default()
        };
        let fmt = FingerprintFormatV1::fixed();
        let videos: [(FingerprintSlot, f64); 1] = [(None, 30.0)];
        let audios: [(FingerprintSlot, f64); 1] = [(None, 30.0)];
        let (matrix, stats) = match_candidates_fingerprint(
            &videos,
            &audios,
            &fmt,
            &params,
            true,
            |_, _| Some(legacy_corr(9.0, 100.0, true)),
            |_, _, _, _, _, _| crate::match_refine::RefineOutcome::not_attempted(),
            |_, _| None,
        );
        assert_eq!(stats.index_builds, 0, "no fingerprints ⇒ no index built");
        assert_eq!(matrix[0][0].method, MatchMethod::EnvelopeV1);
    }
}
