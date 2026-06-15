//! `PairingParams` — the single core-owned parameter contract for role-agnostic
//! pairing and the fine waveform-lock verifier (PLAN-AUDIO-MATCHING-PREWORK §6).
//!
//! Before this module, the verifier thresholds were copied into `SyncParams`,
//! `cli/src/pair.rs`, and `Resolver.swift`, and the copies had already drifted
//! (CLI `verify_density_per_s = 0.20` vs core `0.5`). Here the Rust core owns every
//! pairing/verifier default and the match predicate; CLI and Swift forward overrides
//! to [`PairingParams::set`] and read the core result. `SyncParams` keeps ONLY the
//! editor/full-sync configuration (offset search + drift pass), so the proven
//! `auto_sync_tracks` path is untouched (§3 invariant 2).
//!
//! Determinism: every field is plain data; the verifier is a pure function of the two
//! pyramids + coarse lag + these params (§3 invariant 3).

use crate::error::{CoupleError, Result};
use crate::fingerprint::FingerprintMatchParams;
use serde::{Deserialize, Serialize};

/// Which recording-identity matcher a pairing operation runs (PLAN-SPECTRAL-FINGERPRINT-MATCHING
/// §5.2 / §16.1). The DEFAULT is now [`MatcherMode::FingerprintPrimary`] — boundary 13 flipped it
/// after the §15.2 holdout gates passed and the owner APPROVED the switch (CALIBRATION.md §7.6;
/// the frozen profile is [`FingerprintMatchParams::profile_v2`]). A DEVELOPER OVERRIDE back to
/// [`Self::LegacyEnvelope`] is retained for ≥ 1 release (§16.1): the CLI `--matcher legacy` flag
/// and the Swift / env toggle still force the proven envelope resolver. Fingerprint matching (the
/// §9–§10 pipeline) runs ONLY in [`Self::FingerprintShadow`] / [`Self::FingerprintPrimary`]; in
/// `LegacyEnvelope` no fingerprint is ever built (§5.2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum MatcherMode {
    /// The proven amplitude-envelope matcher (`pair_correlate`) is the identity mechanism. The
    /// boundary-13 DEVELOPER OVERRIDE (§16.1) — no longer the default; fingerprints are never
    /// built in this mode.
    LegacyEnvelope,
    /// The spectral-landmark matcher runs and produces `SpectralLandmarkV1` evidence, but the
    /// legacy `PairCorrelation` MAY be attached alongside for shadow comparison. For calibration
    /// and parity measurement — not the resolver's authority.
    FingerprintShadow,
    /// The spectral-landmark matcher is the authority; the legacy field MAY be `None`. Requires a
    /// calibrated threshold profile (`threshold_profile_version != 0`, §6) — validation rejects
    /// profile 0 here. The DEFAULT since boundary 13 (§16.1), carrying [`FingerprintMatchParams::profile_v2`].
    #[default]
    FingerprintPrimary,
}

/// Core-owned pairing + fine-verifier parameters. `serde(default)` lets a partial JSON
/// override (the FFI `*_params` symbols, CLI `--params`) fill the rest from
/// [`Default`], so a caller only states what it changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PairingParams {
    // ---- Legacy envelope proposal / localization (pair.rs) ----
    /// Absolute pairing overlap floor, seconds. Phase 2 (§7.1) converts this to
    /// stage-1 bins for `pair_correlate`; the fine verifier already uses it as its
    /// hard floor.
    pub min_overlap_s: f64,
    /// Stage-1 z at/above which a cell is a legacy candidate (`z >= legacy_z_min`).
    /// The default legacy resolver gate; separate from the resolver's own `z_min`.
    pub legacy_z_min: f64,
    /// Loose stage-A resemblance floor (NCC) — rejects only obvious garbage; the fine
    /// lock disposes. Phase 2 (§7.2) makes core compute `prefilter_candidate` from it.
    pub refine_ncc_min: f64,
    /// Refinement stage spans, ms (stages 2–5). Strictly decreasing and positive.
    pub stage_spans_ms: [f64; 4],
    /// Parabolic sub-bin clamp (§5.3).
    pub parabolic_clamp: f64,

    // ---- Fine verifier (verify.rs) ----
    /// Window span ≤ this fraction of the overlap (so ≥ 2 disjoint windows fit).
    pub verify_win_frac: f64,
    /// Floor on the verify half-window, seconds.
    pub verify_half_win_min_s: f64,
    /// Ceiling on the verify half-window, seconds (was `SyncParams.half_win_s`).
    pub verify_half_win_max_s: f64,
    /// Windows per second of overlap (density, not a fixed count).
    pub verify_density_per_s: f64,
    /// Always attempt at least this many windows (a fit minimum).
    pub verify_min_windows_floor: usize,
    /// Long-clip cost ceiling on window count.
    pub verify_max_windows: usize,
    /// Per-window lag-search base span, ms (was `SyncParams.drift_base_span_ms`).
    pub verify_base_span_ms: f64,
    /// Per-window lag-search distance scaling, ppm (was `drift_span_scale_ppm`).
    pub verify_span_scale_ppm: f64,
    /// Per-window lag-search cap, ms; tighter than the editor `drift_span_cap_ms`.
    pub verify_span_cap_ms: f64,
    /// Per-window NCC floor for COUNTING a window as locked (kept separate from the
    /// editor-sync `ncc_window_min`: a degraded mic locks the correct lag at modest NCC).
    pub verify_ncc_window_min: f64,
    /// Small absolute floor on inlier windows for a match.
    pub verify_min_locked: u32,
    /// Fraction of measurable windows that must agree for a match.
    pub verify_lock_fraction_min: f64,
    /// Trust floor on lock RMS, ms, for a match.
    pub verify_rms_max_ms: f64,
    /// `[TUNE]` absolute residual floor for robust-fit outlier rejection (Phase 3 §8.2);
    /// surfaced here rather than hidden inside the fitter.
    pub verify_outlier_floor_ms: f64,
    /// Ridge weight in s² (added to sxx = Σ(t−t̄)², itself s²; see `fit::FitReg`).
    pub drift_lambda_s2: f64,
    /// Generous physical drift bound, ppm; out-of-bound ⇒ clamped ⇒ not a match.
    pub drift_max_ppm: f64,

    // ---- Work limits (safety ceilings, never a top-K ranking policy) ----
    /// Hard ceiling on verify pairs attempted per video. A guard on resource use, not a
    /// candidate selector — exceeding it becomes honest ambiguity (Phase 4 §9), never a
    /// silent eviction.
    pub max_verify_pairs_per_video: usize,
    /// Hard ceiling on verify pairs attempted per resolve.
    pub max_verify_pairs_total: usize,

    // ---- Spectral-landmark matcher selection (PLAN-SPECTRAL-FINGERPRINT-MATCHING §5.2) ----
    /// Which matcher this pairing operation runs. DEFAULT [`MatcherMode::LegacyEnvelope`]
    /// (invariant: no default switch until boundary 13). Only the fingerprint modes trigger the
    /// §9–§10 spectral pipeline; `LegacyEnvelope` never builds a fingerprint (§5.2).
    pub matcher_mode: MatcherMode,
    /// Runtime classification thresholds for the spectral-landmark matcher (§6). Consumed only in
    /// the fingerprint modes; the frozen analysis FORMAT lives in `FingerprintFormatV1` and is NOT
    /// reachable through this JSON (a v1 artifact cannot be reshaped by pairing params).
    pub fingerprint_match: FingerprintMatchParams,

    // ---- Cross-device graph consistency (§5.2 / §13) — DEFINED here, CONSUMED in boundary 11 ----
    /// Offset agreement tolerance for composed graph paths, ms (§13.3). Not consumed yet.
    pub graph_offset_tol_ms: f64,
    /// Drift agreement tolerance for composed graph paths, ppm (§13.3). Not consumed yet.
    pub graph_drift_tol_ppm: f64,
    /// Minimum direct-edge support ratio for a graph promotion (`ceil(min_support · ratio)`,
    /// §13.3). Not consumed yet.
    pub graph_min_direct_support_ratio: f64,
    /// Independent composed `Match` paths a graph promotion/demotion requires (§13.3). Not
    /// consumed yet.
    pub graph_required_independent_paths: usize,
}

impl Default for PairingParams {
    fn default() -> Self {
        Self {
            min_overlap_s: 8.0,
            legacy_z_min: 8.5,
            refine_ncc_min: 0.15,
            stage_spans_ms: [500.0, 80.0, 14.0, 3.0],
            parabolic_clamp: 0.6,
            verify_win_frac: 0.5,
            verify_half_win_min_s: 2.5,
            verify_half_win_max_s: 9.0,
            verify_density_per_s: 0.5,
            verify_min_windows_floor: 3,
            verify_max_windows: 24,
            verify_base_span_ms: 120.0,
            verify_span_scale_ppm: 150.0,
            verify_span_cap_ms: 500.0,
            verify_ncc_window_min: 0.25,
            verify_min_locked: 4,
            verify_lock_fraction_min: 0.66,
            // Trust floor on lock RMS. Calibrated on real data: true pairs run up to
            // ~9.5 ms (Test 1 bach172↔ZOOM0033), so 9.0 false-rejected them; 12.0 admits
            // the true pair while the drift bound + lock_fraction reject impostors.
            verify_rms_max_ms: 12.0,
            verify_outlier_floor_ms: 2.0,
            drift_lambda_s2: 50.0,
            // Classifier-only drift bound (read solely by verify_pair via fit_ls_reg).
            // 600 clears real true pairs (+44…−255 ppm) with > 2× margin while a coherent
            // different-recording impostor that wants ~1500+ ppm clamps and is rejected.
            drift_max_ppm: 600.0,
            max_verify_pairs_per_video: 32,
            max_verify_pairs_total: 4096,
            // BOUNDARY 13 (§16.1): the default matcher is now FINGERPRINT-PRIMARY, carrying the
            // FROZEN, holdout-checked profile 2 (NOT the uncalibrated profile-0 default — Primary
            // validation rejects profile 0, §6). The owner approved the flip after the §15.2 gates
            // passed (CALIBRATION.md §7.6). The `--matcher legacy` flag + Swift/env toggle force
            // LegacyEnvelope as a developer override for ≥ 1 release (§16.1).
            matcher_mode: MatcherMode::FingerprintPrimary,
            fingerprint_match: FingerprintMatchParams::profile_v2(),
            // §5.2 initial graph values (DEFINED, not consumed until boundary 11).
            graph_offset_tol_ms: 100.0,
            graph_drift_tol_ppm: 150.0,
            graph_min_direct_support_ratio: 0.75,
            graph_required_independent_paths: 2,
        }
    }
}

impl PairingParams {
    /// Validate once before any work (every FFI and CLI entry calls this). Rejects the
    /// degenerate configurations listed in PLAN §6.2 so a malformed override fails loudly
    /// rather than silently producing junk measurements.
    pub fn validate(&self) -> Result<()> {
        let finite = [
            ("minOverlapS", self.min_overlap_s),
            ("legacyZMin", self.legacy_z_min),
            ("refineNccMin", self.refine_ncc_min),
            ("parabolicClamp", self.parabolic_clamp),
            ("verifyWinFrac", self.verify_win_frac),
            ("verifyHalfWinMinS", self.verify_half_win_min_s),
            ("verifyHalfWinMaxS", self.verify_half_win_max_s),
            ("verifyDensityPerS", self.verify_density_per_s),
            ("verifyBaseSpanMs", self.verify_base_span_ms),
            ("verifySpanScalePpm", self.verify_span_scale_ppm),
            ("verifySpanCapMs", self.verify_span_cap_ms),
            ("verifyNccWindowMin", self.verify_ncc_window_min),
            ("verifyLockFractionMin", self.verify_lock_fraction_min),
            ("verifyRmsMaxMs", self.verify_rms_max_ms),
            ("verifyOutlierFloorMs", self.verify_outlier_floor_ms),
            ("driftLambdaS2", self.drift_lambda_s2),
            ("driftMaxPpm", self.drift_max_ppm),
        ];
        for (name, v) in finite {
            if !v.is_finite() {
                return Err(CoupleError::invalid(format!("PairingParams.{name} must be finite")));
            }
        }
        for (i, &s) in self.stage_spans_ms.iter().enumerate() {
            if !s.is_finite() {
                return Err(CoupleError::invalid(format!("PairingParams.stageSpansMs[{i}] must be finite")));
            }
        }

        if self.min_overlap_s <= 0.0 {
            return Err(CoupleError::invalid("PairingParams.minOverlapS must be > 0"));
        }

        // Fractions in [0, 1].
        for (name, v) in [
            ("verifyWinFrac", self.verify_win_frac),
            ("verifyLockFractionMin", self.verify_lock_fraction_min),
            ("parabolicClamp", self.parabolic_clamp),
            ("refineNccMin", self.refine_ncc_min),
        ] {
            if !(0.0..=1.0).contains(&v) {
                return Err(CoupleError::invalid(format!("PairingParams.{name} must be in [0, 1]")));
            }
        }

        // Non-zero window counts and limits.
        for (name, v) in [
            ("verifyMinWindowsFloor", self.verify_min_windows_floor),
            ("verifyMaxWindows", self.verify_max_windows),
            ("maxVerifyPairsPerVideo", self.max_verify_pairs_per_video),
            ("maxVerifyPairsTotal", self.max_verify_pairs_total),
        ] {
            if v == 0 {
                return Err(CoupleError::invalid(format!("PairingParams.{name} must be > 0")));
            }
        }
        if self.verify_min_locked == 0 {
            return Err(CoupleError::invalid("PairingParams.verifyMinLocked must be > 0"));
        }

        if self.verify_max_windows < self.verify_min_windows_floor {
            return Err(CoupleError::invalid(
                "PairingParams.verifyMaxWindows must be >= verifyMinWindowsFloor",
            ));
        }
        if self.verify_half_win_max_s < self.verify_half_win_min_s {
            return Err(CoupleError::invalid(
                "PairingParams.verifyHalfWinMaxS must be >= verifyHalfWinMinS",
            ));
        }

        // No negative spans, RMS limits, ridge weights, drift bounds, half-windows, density.
        for (name, v) in [
            ("legacyZMin", self.legacy_z_min),
            ("verifyHalfWinMinS", self.verify_half_win_min_s),
            ("verifyHalfWinMaxS", self.verify_half_win_max_s),
            ("verifyDensityPerS", self.verify_density_per_s),
            ("verifyBaseSpanMs", self.verify_base_span_ms),
            ("verifySpanScalePpm", self.verify_span_scale_ppm),
            ("verifySpanCapMs", self.verify_span_cap_ms),
            ("verifyNccWindowMin", self.verify_ncc_window_min),
            ("verifyRmsMaxMs", self.verify_rms_max_ms),
            ("verifyOutlierFloorMs", self.verify_outlier_floor_ms),
            ("driftLambdaS2", self.drift_lambda_s2),
            ("driftMaxPpm", self.drift_max_ppm),
        ] {
            if v < 0.0 {
                return Err(CoupleError::invalid(format!("PairingParams.{name} must be >= 0")));
            }
        }

        // Refinement spans strictly decreasing and positive.
        for w in self.stage_spans_ms.windows(2) {
            if w[0] <= w[1] {
                return Err(CoupleError::invalid(
                    "PairingParams.stageSpansMs must be strictly decreasing",
                ));
            }
        }
        if self.stage_spans_ms.iter().any(|&s| s <= 0.0) {
            return Err(CoupleError::invalid("PairingParams.stageSpansMs must be positive"));
        }

        // ---- Spectral-landmark matcher fields (§5.2 / §6) ----
        // Graph tolerances: finite and non-negative (DEFINED here even though boundary 11 consumes
        // them — a malformed value must fail loudly NOW so it cannot reach the graph stage later).
        for (name, v) in [
            ("graphOffsetTolMs", self.graph_offset_tol_ms),
            ("graphDriftTolPpm", self.graph_drift_tol_ppm),
            ("graphMinDirectSupportRatio", self.graph_min_direct_support_ratio),
        ] {
            if !v.is_finite() {
                return Err(CoupleError::invalid(format!("PairingParams.{name} must be finite")));
            }
            if v < 0.0 {
                return Err(CoupleError::invalid(format!("PairingParams.{name} must be >= 0")));
            }
        }
        // The support RATIO is a fraction of `min_support`; > 1 would demand more direct support
        // than a match itself requires, which is nonsensical.
        if self.graph_min_direct_support_ratio > 1.0 {
            return Err(CoupleError::invalid(
                "PairingParams.graphMinDirectSupportRatio must be in [0, 1]",
            ));
        }
        if self.graph_required_independent_paths == 0 {
            return Err(CoupleError::invalid(
                "PairingParams.graphRequiredIndependentPaths must be > 0",
            ));
        }

        // The fingerprint runtime thresholds validate independently (their own finite/[0,1]/non-zero
        // rules, §6). Always checked so a bad override is rejected even if the mode is legacy.
        self.fingerprint_match.validate()?;

        // §6: `FingerprintPrimary` MUST reject an uncalibrated threshold profile (version 0 is
        // shadow-only). Because the default mode is LegacyEnvelope this never fires in normal
        // operation; it guards a deliberate (boundary-13) switch from running on uncalibrated
        // thresholds.
        if self.matcher_mode == MatcherMode::FingerprintPrimary
            && self.fingerprint_match.threshold_profile_version == 0
        {
            return Err(CoupleError::invalid(
                "PairingParams: FingerprintPrimary requires a calibrated thresholdProfileVersion \
                 (!= 0); profile 0 is shadow-only",
            ));
        }

        Ok(())
    }

    /// CLI/host `--params k=v` override for the scalar fields. Keys are kebab-case
    /// mirrors of the field names. The array field `stage-spans-ms` is not settable
    /// here (its four values are calibrated together). Unknown keys error.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let v: f64 = value
            .parse()
            .map_err(|_| CoupleError::invalid(format!("--params {key}: not a number")))?;
        match key {
            "min-overlap-s" => self.min_overlap_s = v,
            "legacy-z-min" => self.legacy_z_min = v,
            "refine-ncc-min" => self.refine_ncc_min = v,
            "parabolic-clamp" => self.parabolic_clamp = v,
            "verify-win-frac" => self.verify_win_frac = v,
            "verify-half-win-min-s" => self.verify_half_win_min_s = v,
            "verify-half-win-max-s" => self.verify_half_win_max_s = v,
            "verify-density-per-s" => self.verify_density_per_s = v,
            "verify-min-windows-floor" => self.verify_min_windows_floor = v as usize,
            "verify-max-windows" => self.verify_max_windows = v as usize,
            "verify-base-span-ms" => self.verify_base_span_ms = v,
            "verify-span-scale-ppm" => self.verify_span_scale_ppm = v,
            "verify-span-cap-ms" => self.verify_span_cap_ms = v,
            "verify-ncc-window-min" => self.verify_ncc_window_min = v,
            "verify-min-locked" => self.verify_min_locked = v as u32,
            "verify-lock-fraction-min" => self.verify_lock_fraction_min = v,
            "verify-rms-max-ms" => self.verify_rms_max_ms = v,
            "verify-outlier-floor-ms" => self.verify_outlier_floor_ms = v,
            "drift-lambda-s2" => self.drift_lambda_s2 = v,
            "drift-max-ppm" => self.drift_max_ppm = v,
            "max-verify-pairs-per-video" => self.max_verify_pairs_per_video = v as usize,
            "max-verify-pairs-total" => self.max_verify_pairs_total = v as usize,
            other => return Err(CoupleError::invalid(format!("unknown pairing param {other}"))),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // §6.8.1 — default params serialize and deserialize identically.
    #[test]
    fn default_round_trip() {
        let p = PairingParams::default();
        let json = serde_json::to_string(&p).unwrap();
        let back: PairingParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back, "default PairingParams must round-trip through JSON");
        p.validate().expect("defaults must validate");
    }

    // A partial override fills the rest from Default (serde(default)) — the FFI/CLI path.
    #[test]
    fn partial_override_fills_from_default() {
        let json = r#"{"verifyRmsMaxMs": 7.5}"#;
        let p: PairingParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.verify_rms_max_ms, 7.5);
        // Everything else is the default.
        assert_eq!(p.drift_max_ppm, PairingParams::default().drift_max_ppm);
        assert_eq!(p.min_overlap_s, PairingParams::default().min_overlap_s);
    }

    // set() routes kebab keys to the scalar fields; unknown keys error.
    #[test]
    fn set_scalar_and_unknown() {
        let mut p = PairingParams::default();
        p.set("verify-rms-max-ms", "9").unwrap();
        assert_eq!(p.verify_rms_max_ms, 9.0);
        p.set("verify-min-locked", "5").unwrap();
        assert_eq!(p.verify_min_locked, 5);
        assert!(p.set("stage-spans-ms", "1").is_err(), "array key not settable");
        assert!(p.set("bogus-key", "1").is_err());
        assert!(p.set("verify-rms-max-ms", "notnum").is_err());
    }

    // §6.8.2 — invalid-param table: each row must be rejected by validate().
    #[test]
    fn invalid_param_table() {
        type Mutate = fn(&mut PairingParams);
        let cases: &[(&str, Mutate)] = &[
            ("non-finite rms", |p| p.verify_rms_max_ms = f64::NAN),
            ("inf lambda", |p| p.drift_lambda_s2 = f64::INFINITY),
            ("min_overlap_s = 0", |p| p.min_overlap_s = 0.0),
            ("min_overlap_s < 0", |p| p.min_overlap_s = -1.0),
            ("win_frac > 1", |p| p.verify_win_frac = 1.5),
            ("win_frac < 0", |p| p.verify_win_frac = -0.1),
            ("lock_fraction > 1", |p| p.verify_lock_fraction_min = 2.0),
            ("zero windows floor", |p| p.verify_min_windows_floor = 0),
            ("zero max windows", |p| p.verify_max_windows = 0),
            ("zero per-video limit", |p| p.max_verify_pairs_per_video = 0),
            ("zero total limit", |p| p.max_verify_pairs_total = 0),
            ("zero min_locked", |p| p.verify_min_locked = 0),
            ("max < floor windows", |p| {
                p.verify_min_windows_floor = 10;
                p.verify_max_windows = 5;
            }),
            ("half_win max < min", |p| {
                p.verify_half_win_min_s = 9.0;
                p.verify_half_win_max_s = 2.0;
            }),
            ("negative span", |p| p.verify_base_span_ms = -1.0),
            ("negative rms limit", |p| p.verify_rms_max_ms = -1.0),
            ("negative ridge", |p| p.drift_lambda_s2 = -1.0),
            ("negative drift bound", |p| p.drift_max_ppm = -1.0),
            ("non-decreasing spans", |p| p.stage_spans_ms = [10.0, 20.0, 5.0, 1.0]),
            ("non-positive span entry", |p| p.stage_spans_ms = [500.0, 80.0, 14.0, 0.0]),
        ];
        for (label, mutate) in cases {
            let mut p = PairingParams::default();
            mutate(&mut p);
            assert!(p.validate().is_err(), "case '{label}' must be rejected by validate()");
        }
        // The unmodified defaults still pass (guards against an over-broad rule above).
        assert!(PairingParams::default().validate().is_ok());
    }

    // ---- §5.2: spectral-landmark matcher fields ----

    // BOUNDARY 13 (§16.1): the default matcher is now FingerprintPrimary carrying the frozen
    // profile 2. The new fields carry their §5.2 initial values and the whole struct round-trips.
    #[test]
    fn default_matcher_is_fingerprint_primary_and_new_fields_round_trip() {
        let p = PairingParams::default();
        assert_eq!(
            p.matcher_mode,
            MatcherMode::FingerprintPrimary,
            "boundary 13: default must be FingerprintPrimary (§16.1)"
        );
        // The default carries the FROZEN profile 2 (NOT profile 0 — Primary rejects profile 0, §6).
        assert_eq!(
            p.fingerprint_match,
            FingerprintMatchParams::profile_v2(),
            "default fingerprint thresholds are the frozen profile 2"
        );
        assert_eq!(p.fingerprint_match.threshold_profile_version, 2);
        assert_eq!(p.graph_offset_tol_ms, 100.0);
        assert_eq!(p.graph_drift_tol_ppm, 150.0);
        assert_eq!(p.graph_min_direct_support_ratio, 0.75);
        assert_eq!(p.graph_required_independent_paths, 2);

        let json = serde_json::to_string(&p).unwrap();
        // The matcher mode is on the wire as a camelCase string.
        assert!(json.contains("\"matcherMode\":\"fingerprintPrimary\""), "wire: {json}");
        assert!(json.contains("fingerprintMatch") && json.contains("graphOffsetTolMs"));
        let back: PairingParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back, "extended PairingParams must round-trip through JSON");
        p.validate().expect("extended defaults must validate");
    }

    // The DEVELOPER OVERRIDE (§16.1): explicitly selecting LegacyEnvelope still validates (with
    // ANY profile, including profile 0 — the legacy matcher does not consult the fingerprint
    // identity thresholds), so `--matcher legacy` / the Swift+env toggle can always force it.
    #[test]
    fn legacy_override_is_valid_with_any_profile() {
        let legacy =
            PairingParams { matcher_mode: MatcherMode::LegacyEnvelope, ..Default::default() };
        legacy.validate().expect("LegacyEnvelope override (profile 2 default) must validate");
        // Even with the uncalibrated profile 0 (the legacy path ignores the profile, §6).
        let legacy0 = PairingParams {
            matcher_mode: MatcherMode::LegacyEnvelope,
            fingerprint_match: FingerprintMatchParams::default(),
            ..Default::default()
        };
        legacy0.validate().expect("LegacyEnvelope override with profile 0 must validate");
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(json.contains("\"matcherMode\":\"legacyEnvelope\""), "override on the wire: {json}");
    }

    // BACK-COMPAT: a params JSON written BEFORE these fields existed (no matcherMode /
    // fingerprintMatch / graph*) still deserializes — every new field fills from Default via
    // serde(default). This protects the prework FFI default-json / Swift round-trips. After
    // boundary 13 the back-filled object therefore equals the NEW default (FingerprintPrimary +
    // profile 2), proving the wire is forward/back compatible across the flip.
    #[test]
    fn old_json_without_new_fields_still_deserializes() {
        // A representative pre-boundary-6 params object: only the original prework scalar fields.
        let old = r#"{
            "minOverlapS": 8.0, "legacyZMin": 8.5, "refineNccMin": 0.15,
            "stageSpansMs": [500.0, 80.0, 14.0, 3.0], "parabolicClamp": 0.6,
            "verifyWinFrac": 0.5, "verifyHalfWinMinS": 2.5, "verifyHalfWinMaxS": 9.0,
            "verifyDensityPerS": 0.5, "verifyMinWindowsFloor": 3, "verifyMaxWindows": 24,
            "verifyBaseSpanMs": 120.0, "verifySpanScalePpm": 150.0, "verifySpanCapMs": 500.0,
            "verifyNccWindowMin": 0.25, "verifyMinLocked": 4, "verifyLockFractionMin": 0.66,
            "verifyRmsMaxMs": 12.0, "verifyOutlierFloorMs": 2.0, "driftLambdaS2": 50.0,
            "driftMaxPpm": 600.0, "maxVerifyPairsPerVideo": 32, "maxVerifyPairsTotal": 4096
        }"#;
        let p: PairingParams = serde_json::from_str(old).unwrap();
        // The new fields are filled from Default, and the whole thing equals the (new) Default.
        assert_eq!(p, PairingParams::default(), "old JSON must fill new fields from Default");
        assert_eq!(p.matcher_mode, MatcherMode::FingerprintPrimary);
        p.validate().expect("back-filled params must validate");
    }

    // A partial override can SELECT a fingerprint mode and tune fingerprint thresholds; the rest
    // fills from Default (the shadow-calibration entry path).
    #[test]
    fn partial_override_selects_shadow_mode() {
        let json = r#"{"matcherMode":"fingerprintShadow","fingerprintMatch":{"minSupport":20}}"#;
        let p: PairingParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.matcher_mode, MatcherMode::FingerprintShadow);
        assert_eq!(p.fingerprint_match.min_support, 20);
        // Untouched fingerprint fields keep their defaults.
        assert_eq!(
            p.fingerprint_match.max_hypotheses,
            FingerprintMatchParams::default().max_hypotheses
        );
        // Profile 0 is fine in shadow mode.
        p.validate().expect("shadow mode with profile 0 must validate");
    }

    // Validation rejects bad graph values and a bad nested fingerprint value, and — critically —
    // FingerprintPrimary with the uncalibrated profile 0 (§6).
    #[test]
    fn validate_rejects_bad_graph_and_primary_profile_zero() {
        type Mutate = fn(&mut PairingParams);
        let cases: &[(&str, Mutate)] = &[
            ("nan graph offset", |p| p.graph_offset_tol_ms = f64::NAN),
            ("inf graph drift", |p| p.graph_drift_tol_ppm = f64::INFINITY),
            ("negative graph offset", |p| p.graph_offset_tol_ms = -1.0),
            ("ratio > 1", |p| p.graph_min_direct_support_ratio = 1.5),
            ("negative ratio", |p| p.graph_min_direct_support_ratio = -0.1),
            ("zero required paths", |p| p.graph_required_independent_paths = 0),
            // A bad nested fingerprint threshold must bubble up through fingerprint_match.validate().
            ("bad nested fingerprint", |p| p.fingerprint_match.min_support = 0),
            ("nested non-finite", |p| p.fingerprint_match.inlier_tolerance_ms = f64::NAN),
            // §6: Primary mode + uncalibrated profile 0 is rejected.
            ("primary + profile 0", |p| {
                p.matcher_mode = MatcherMode::FingerprintPrimary;
                p.fingerprint_match.threshold_profile_version = 0;
            }),
        ];
        for (label, mutate) in cases {
            let mut p = PairingParams::default();
            mutate(&mut p);
            assert!(p.validate().is_err(), "case '{label}' must be rejected by validate()");
        }

        // Primary mode WITH a calibrated profile (version 1) validates — proving the rejection is
        // specifically the uncalibrated-profile guard, not the mode itself.
        let mut primary_ok =
            PairingParams { matcher_mode: MatcherMode::FingerprintPrimary, ..Default::default() };
        primary_ok.fingerprint_match.threshold_profile_version = 1;
        primary_ok.validate().expect("primary + profile 1 must validate");

        // §15.1.3: the actual FROZEN profile_v1 table makes FingerprintPrimary valid end-to-end
        // (the boundary-12 deliverable — profile 1 exists, so Primary is no longer a profile-0-only
        // error). Boundary 13 ships the DEFAULT with profile 2 (a superset of this guarantee).
        let primary_frozen = PairingParams {
            matcher_mode: MatcherMode::FingerprintPrimary,
            fingerprint_match: crate::fingerprint::FingerprintMatchParams::profile_v1(),
            ..Default::default()
        };
        primary_frozen
            .validate()
            .expect("FingerprintPrimary with the frozen profile_v1 must validate");
        assert_eq!(primary_frozen.fingerprint_match.threshold_profile_version, 1);

        // And profile_v1 under Primary still REJECTS if someone resets the version to 0.
        let mut primary_reset = primary_frozen.clone();
        primary_reset.fingerprint_match.threshold_profile_version = 0;
        assert!(
            primary_reset.validate().is_err(),
            "resetting the frozen profile to version 0 must re-trip the primary guard"
        );

        // BOUNDARY 13: the unmodified default is FingerprintPrimary + profile 2 and validates (the
        // default is no longer profile-0-shadow; the Primary guard is satisfied by the frozen profile).
        assert!(PairingParams::default().validate().is_ok());
        assert_eq!(PairingParams::default().matcher_mode, MatcherMode::FingerprintPrimary);
        assert_eq!(PairingParams::default().fingerprint_match.threshold_profile_version, 2);
    }
}
