//! Fingerprint parameters (PLAN-SPECTRAL-FINGERPRINT-MATCHING §6).
//!
//! Two structs with deliberately separated lifecycles:
//!
//! - [`FingerprintFormatV1`] is the FORMAT-DEFINING analysis contract (analysis rate,
//!   FFT/hop, peak geometry, landmark quantization). One loaded track holds one
//!   `fingerprint-v1.cpf` built under these values, so production code MUST NOT let the v1
//!   format be changed through ordinary pairing JSON — the constructor is
//!   [`FingerprintFormatV1::fixed`], whose returned table is the only legal v1 format.
//!   `serde(default)` (filling absent JSON fields from `fixed()`) plus the lock test mean a
//!   round-tripped artifact reproduces `fixed()`; changing a value requires a
//!   `FingerprintFormatV2`, `fingerprint-v2.cpf`, and a new `MatchMethod` variant (§6).
//! - [`FingerprintMatchParams`] is RUNTIME classification thresholds (all `[TUNE]`). These
//!   rank/accept candidates and may move under calibration WITHOUT invalidating any cached
//!   artifact. DEFINED here; no matching logic consumes it yet (that arrives in later
//!   boundaries). `threshold_profile_version = 0` means uncalibrated / shadow-only.
//!
//! Determinism (§3 invariant 2): every field is plain data; nothing here reads RNG, a hash
//! iteration order, or a wall clock.

use crate::error::{CoupleError, Result};
use serde::{Deserialize, Serialize};

/// Golden value of [`FingerprintFormatV1::params_hash`] for the frozen [`FingerprintFormatV1::fixed`]
/// format (§8.2): `BLAKE3("CPFPARAMS\x01" ‖ ordered fixed() fields)[..16]`. Pinned so the
/// hash ENCODING (field order, widths, the domain tag) cannot drift silently — a deliberate v2
/// change must regenerate this and the CPF1 golden artifact together. The file format uses this
/// to reject an artifact whose stored params hash does not match the format it loads under.
pub const FIXED_PARAMS_HASH: [u8; 16] = [
    0x42, 0x7e, 0x9e, 0x29, 0x44, 0x6c, 0x2e, 0x4c, 0x8a, 0x9f, 0xb7, 0x8f, 0x34, 0x64, 0x6e, 0x10,
];

/// The format-defining analysis + landmark parameters for fingerprint v1. Do NOT mutate
/// these through ordinary pairing JSON — see the module docs and [`Self::fixed`].
///
/// `serde(default)` means a partial/round-tripped JSON fills any absent field from
/// [`Self::fixed`]; combined with the `fixed()` lock test, a v1 artifact always deserializes
/// back to the canonical table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FingerprintFormatV1 {
    /// Analysis sample rate after the two half-band decimation stages, Hz (48k→24k→12k).
    pub analysis_sample_rate: u32,
    /// STFT window length in analysis-rate samples (§7.2).
    pub fft_size: usize,
    /// STFT hop in analysis-rate samples (§7.2).
    pub hop_size: usize,
    /// Low edge of the analyzed band, Hz (bins below this are ignored, §7.2).
    pub min_frequency_hz: f64,
    /// High edge of the analyzed band, Hz.
    pub max_frequency_hz: f64,
    /// Peak neighborhood half-extent in frames (time radius, §7.3).
    pub peak_time_radius_frames: usize,
    /// Peak neighborhood half-extent in FFT bins (frequency radius, §7.3).
    pub peak_freq_radius_bins: usize,
    /// Prominence a peak must clear above its local frequency-neighborhood median, dB (§7.3).
    pub peak_min_prominence_db: f64,
    /// Cap on peaks kept per STFT frame, strongest first (§7.3).
    pub max_peaks_per_frame: usize,
    /// Cap on peaks kept per rolling one-second window (§7.3).
    pub max_peaks_per_second: usize,
    /// FFT bins per landmark frequency-quantization step (floor division, §7.4).
    pub frequency_quantization_bins: u32,
    /// Frames per landmark delta-time quantization step (floor division, §7.4).
    pub delta_time_quantization_frames: u32,
    /// Earliest anchor→target spacing for a landmark, seconds (§7.4).
    pub target_min_s: f64,
    /// Latest anchor→target spacing for a landmark, seconds (§7.4).
    pub target_max_s: f64,
    /// Targets per anchor (landmark fan-out, §7.4).
    pub fanout: usize,
}

impl Default for FingerprintFormatV1 {
    /// `Default` IS [`Self::fixed`]: the only legal v1 format. This makes `serde(default)`
    /// fill absent JSON fields from the canonical table rather than from arbitrary zeros.
    fn default() -> Self {
        Self::fixed()
    }
}

impl FingerprintFormatV1 {
    /// The exact, frozen v1 format table (§6). These are the ONLY legal v1 values; the
    /// `fixed()` lock test pins every one. Changing any requires a v2 format + a new
    /// `fingerprint-v2.cpf` + a new `MatchMethod` variant — never an in-place edit.
    pub const fn fixed() -> Self {
        Self {
            analysis_sample_rate: 12_000,
            fft_size: 1024,
            hop_size: 256,
            min_frequency_hz: 100.0,
            max_frequency_hz: 5500.0,
            peak_time_radius_frames: 2,
            peak_freq_radius_bins: 3,
            peak_min_prominence_db: 6.0,
            max_peaks_per_frame: 5,
            max_peaks_per_second: 30,
            frequency_quantization_bins: 2,
            delta_time_quantization_frames: 2,
            target_min_s: 0.5,
            target_max_s: 4.0,
            fanout: 5,
        }
    }

    /// Seconds per analysis sample at the analysis rate.
    pub fn sample_period_s(&self) -> f64 {
        1.0 / self.analysis_sample_rate as f64
    }

    /// The 16-byte format params hash (§8.2). BLAKE3 over an EXPLICITLY ORDERED BINARY encoding
    /// of every FORMAT-DEFINING analysis + landmark parameter — never JSON text (whose key order
    /// and float formatting are not guaranteed stable). The first 16 bytes of the digest are the
    /// `params_hash` written into the CPF1 header; a reader rejects an artifact whose stored hash
    /// differs from the hash of the format it loads under (§8.1).
    ///
    /// Encoding (each field appended in this exact order; integers little-endian, floats as their
    /// IEEE-754 bit pattern little-endian so the byte form is exact and reproducible):
    ///
    /// ```text
    /// "CPFPARAMS\x01"            domain/version tag (guards against cross-purpose hashing)
    /// analysis_sample_rate      u32
    /// fft_size                  u64   (usize widened to a fixed width for portability)
    /// hop_size                  u64
    /// min_frequency_hz          f64 bits
    /// max_frequency_hz          f64 bits
    /// peak_time_radius_frames   u64
    /// peak_freq_radius_bins     u64
    /// peak_min_prominence_db    f64 bits
    /// max_peaks_per_frame       u64
    /// max_peaks_per_second      u64
    /// frequency_quantization_bins   u32
    /// delta_time_quantization_frames u32
    /// target_min_s              f64 bits
    /// target_max_s              f64 bits
    /// fanout                    u64
    /// ```
    ///
    /// CLASSIFICATION thresholds ([`FingerprintMatchParams`]) are deliberately ABSENT: they are
    /// not format-defining and must not invalidate a cached artifact (§8.2). Adding/reordering a
    /// field here, or changing any value, changes the hash — which is exactly the v2-format
    /// signal. The byte layout is pinned by `params_hash_*` tests.
    pub fn params_hash(&self) -> [u8; 16] {
        let mut h = blake3::Hasher::new();
        h.update(b"CPFPARAMS\x01");
        h.update(&self.analysis_sample_rate.to_le_bytes());
        h.update(&(self.fft_size as u64).to_le_bytes());
        h.update(&(self.hop_size as u64).to_le_bytes());
        h.update(&self.min_frequency_hz.to_bits().to_le_bytes());
        h.update(&self.max_frequency_hz.to_bits().to_le_bytes());
        h.update(&(self.peak_time_radius_frames as u64).to_le_bytes());
        h.update(&(self.peak_freq_radius_bins as u64).to_le_bytes());
        h.update(&self.peak_min_prominence_db.to_bits().to_le_bytes());
        h.update(&(self.max_peaks_per_frame as u64).to_le_bytes());
        h.update(&(self.max_peaks_per_second as u64).to_le_bytes());
        h.update(&self.frequency_quantization_bins.to_le_bytes());
        h.update(&self.delta_time_quantization_frames.to_le_bytes());
        h.update(&self.target_min_s.to_bits().to_le_bytes());
        h.update(&self.target_max_s.to_bits().to_le_bytes());
        h.update(&(self.fanout as u64).to_le_bytes());
        let digest = h.finalize();
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest.as_bytes()[..16]);
        out
    }

    /// Seconds between successive STFT frame centers (`hop_size / analysis_sample_rate`).
    pub fn frame_period_s(&self) -> f64 {
        self.hop_size as f64 / self.analysis_sample_rate as f64
    }

    /// Hz per FFT bin (`analysis_sample_rate / fft_size`).
    pub fn bin_hz(&self) -> f64 {
        self.analysis_sample_rate as f64 / self.fft_size as f64
    }

    /// Lowest FFT bin index inside `[min_frequency_hz, max_frequency_hz]` (inclusive),
    /// rounding the low edge UP so a partial bin below the band is excluded.
    pub fn min_bin(&self) -> usize {
        (self.min_frequency_hz / self.bin_hz()).ceil() as usize
    }

    /// Highest FFT bin index inside the band (inclusive), rounding the high edge DOWN, and
    /// never past the real-FFT Nyquist bin `fft_size / 2`.
    pub fn max_bin(&self) -> usize {
        let nyquist = self.fft_size / 2;
        ((self.max_frequency_hz / self.bin_hz()).floor() as usize).min(nyquist)
    }

    /// Validate the format is internally consistent and its quantized indices fit the v1
    /// landmark bit-widths (§7.4 "fail validation rather than truncate"). Even though this
    /// boundary does not pack hashes, an infeasible format must fail loudly NOW so a later
    /// boundary never silently truncates a bin index.
    ///
    /// v1 landmark hash bit budget (§7.4): anchor freq 9 bits, target freq 9 bits, delta-time
    /// 8 bits, reserved/version 6 bits.
    pub fn validate(&self) -> Result<()> {
        // Finiteness of the float fields (§20: no NaN/Inf anywhere in the contract).
        for (name, v) in [
            ("minFrequencyHz", self.min_frequency_hz),
            ("maxFrequencyHz", self.max_frequency_hz),
            ("peakMinProminenceDb", self.peak_min_prominence_db),
            ("targetMinS", self.target_min_s),
            ("targetMaxS", self.target_max_s),
        ] {
            if !v.is_finite() {
                return Err(CoupleError::invalid(format!("FingerprintFormatV1.{name} must be finite")));
            }
        }

        // Non-zero structural sizes.
        for (name, v) in [
            ("analysisSampleRate", self.analysis_sample_rate as usize),
            ("fftSize", self.fft_size),
            ("hopSize", self.hop_size),
            ("maxPeaksPerFrame", self.max_peaks_per_frame),
            ("maxPeaksPerSecond", self.max_peaks_per_second),
            ("fanout", self.fanout),
        ] {
            if v == 0 {
                return Err(CoupleError::invalid(format!("FingerprintFormatV1.{name} must be > 0")));
            }
        }
        if self.frequency_quantization_bins == 0 {
            return Err(CoupleError::invalid("FingerprintFormatV1.frequencyQuantizationBins must be > 0"));
        }
        if self.delta_time_quantization_frames == 0 {
            return Err(CoupleError::invalid(
                "FingerprintFormatV1.deltaTimeQuantizationFrames must be > 0",
            ));
        }

        // FFT geometry: power-of-two window (realfft is happiest there and the format is
        // frozen at 1024), hop ≤ window (frames must advance ≤ one window, and a hop larger
        // than the window would skip samples — a silent data loss).
        if !self.fft_size.is_power_of_two() {
            return Err(CoupleError::invalid("FingerprintFormatV1.fftSize must be a power of two"));
        }
        if self.hop_size > self.fft_size {
            return Err(CoupleError::invalid("FingerprintFormatV1.hopSize must be <= fftSize"));
        }

        // Frequency band: 0 < min < max ≤ Nyquist.
        if self.min_frequency_hz <= 0.0 {
            return Err(CoupleError::invalid("FingerprintFormatV1.minFrequencyHz must be > 0"));
        }
        if self.max_frequency_hz <= self.min_frequency_hz {
            return Err(CoupleError::invalid(
                "FingerprintFormatV1.maxFrequencyHz must be > minFrequencyHz",
            ));
        }
        let nyquist_hz = self.analysis_sample_rate as f64 / 2.0;
        if self.max_frequency_hz > nyquist_hz {
            return Err(CoupleError::invalid(format!(
                "FingerprintFormatV1.maxFrequencyHz ({}) exceeds analysis Nyquist ({nyquist_hz})",
                self.max_frequency_hz
            )));
        }
        // The band must contain at least one usable bin AND leave room for the freq-radius
        // neighborhood, else peak selection has nothing to compare against.
        if self.max_bin() <= self.min_bin() {
            return Err(CoupleError::invalid(
                "FingerprintFormatV1 frequency band spans no FFT bins",
            ));
        }
        if self.max_bin() - self.min_bin() < 2 * self.peak_freq_radius_bins {
            return Err(CoupleError::invalid(
                "FingerprintFormatV1 band too narrow for the peak frequency radius",
            ));
        }

        // Landmark timing window: 0 < target_min < target_max, and the window must be
        // expressible in whole frames.
        if self.target_min_s <= 0.0 {
            return Err(CoupleError::invalid("FingerprintFormatV1.targetMinS must be > 0"));
        }
        if self.target_max_s <= self.target_min_s {
            return Err(CoupleError::invalid(
                "FingerprintFormatV1.targetMaxS must be > targetMinS",
            ));
        }

        // §7.4 bit-width feasibility. The quantized indices must fit their hash fields, or a
        // later boundary would silently truncate — reject here instead.
        //
        // Frequency index = max_bin / frequency_quantization_bins; it must fit BOTH the 9-bit
        // anchor and the 9-bit target field. Delta-time index = (target_max − target_min in
        // frames) / delta_time_quantization_frames; it must fit the 8-bit field.
        const FREQ_BITS: u32 = 9;
        const DELTA_BITS: u32 = 8;
        let max_freq_index = self.max_bin() as u32 / self.frequency_quantization_bins;
        if max_freq_index >= (1u32 << FREQ_BITS) {
            return Err(CoupleError::invalid(format!(
                "FingerprintFormatV1 quantized frequency index {max_freq_index} exceeds {FREQ_BITS}-bit field"
            )));
        }
        let max_delta_frames =
            ((self.target_max_s - 0.0) / self.frame_period_s()).floor() as u32;
        let max_delta_index = max_delta_frames / self.delta_time_quantization_frames;
        if max_delta_index >= (1u32 << DELTA_BITS) {
            return Err(CoupleError::invalid(format!(
                "FingerprintFormatV1 quantized delta-time index {max_delta_index} exceeds {DELTA_BITS}-bit field"
            )));
        }

        Ok(())
    }
}

/// Runtime classification thresholds (all `[TUNE]`, §6). These rank and accept candidates;
/// they are NOT format-defining and do not invalidate any cached fingerprint. No matching
/// logic consumes this yet — it is defined for the later affine-consensus boundaries.
///
/// `serde(default)` lets a partial JSON override fill the rest from [`Default`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FingerprintMatchParams {
    /// Drop a hash for a track when its occurrence count exceeds this (an uninformative,
    /// over-frequent hash — §9.1).
    pub max_occurrences_per_hash_per_track: usize,
    /// Drop a shared-hash group when `occ_a * occ_b` exceeds this (§9.1).
    pub max_occurrence_product: usize,
    /// ROUTINE time-stratified SAMPLE cap on correspondences carried into consensus for one pair
    /// (§9.2). When the grouped count exceeds this, a representative 128-bin subset is retained and
    /// scored NORMALLY — this is a sampling cap, NOT a "give up / Ambiguous" resource ceiling (that
    /// is [`Self::hard_correspondence_ceiling`], §9.3).
    pub max_correspondences_per_pair: usize,
    /// HARD resource ceiling on the GROUPED correspondence count for one pair (§9.3). A pair whose
    /// grouped correspondences exceed this is a genuinely-exhausted "resource ceiling" ⇒ the
    /// method-neutral classification is forced `Ambiguous` (never `NoMatch`, §20). MUST be
    /// `>= max_correspondences_per_pair` (validated). Profile 2 sets it FAR above
    /// [`Self::max_correspondences_per_pair`] so the ROUTINE §9.2 sample (then scored NORMALLY) does
    /// NOT trip it — only a pathological pair does. Profile 0/1 set it EQUAL to the §9.2 sample cap,
    /// so a capped pair also trips the ceiling and is forced Ambiguous — reproducing the boundary-12
    /// conflated behaviour byte-for-byte (the §9.2/§9.3 SEPARATION is the recall fix this field gates).
    pub hard_correspondence_ceiling: usize,
    /// Physical drift bound for an accepted affine map, ppm (§10.2).
    pub max_abs_drift_ppm: f64,
    /// Residual under which a correspondence counts as an inlier, ms (§10.3).
    pub inlier_tolerance_ms: f64,
    /// Minimum weighted support for a `Match` (§10.6).
    pub min_support: u32,
    /// Minimum inlier fraction for a `Match` (§10.6) — the FRACTION path of the §10.6 identity gate.
    pub min_inlier_fraction: f64,
    /// Minimum ABSOLUTE inlier COUNT for the density-robust §10.6 identity path. On long dense
    /// orchestral pairs the true-line inlier FRACTION is depressed by spurious correspondences, but
    /// the inlier COUNT is large and stable; the identity gate passes when the fraction path OR this
    /// absolute path (`inlier_count >= min_abs_inlier_count AND temporal_coverage >=
    /// min_abs_coverage`) holds (§10.6). Set EFFECTIVELY INERT at profile 0 (`u32::MAX`) so shadow
    /// behaves exactly as boundary 12 (only the fraction path can fire).
    pub min_abs_inlier_count: u32,
    /// Minimum temporal coverage for the ABSOLUTE identity path (§10.6). The absolute path requires
    /// HIGH coverage so a localized coincidence / duplicate-tower (high count, low coverage) is still
    /// rejected; profile 0 holds it at the profile-0 `min_temporal_coverage` (the absolute path is
    /// gated off by `min_abs_inlier_count = u32::MAX` there anyway).
    pub min_abs_coverage: f64,
    /// Minimum temporal coverage for a `Match` (§10.6) — applies to BOTH identity paths.
    pub min_temporal_coverage: f64,
    /// Maximum p95 inlier residual for a `Match`, ms (§10.6).
    pub max_residual_p95_ms: f64,
    /// Minimum uniqueness margin in `[0, 1]` for a `Match` (§10.5/§10.6).
    pub min_score_margin: f64,
    /// Hypothesis-count ceiling for consensus (§10.2).
    pub max_hypotheses: usize,
    /// Calibrated threshold-profile version. `0` = uncalibrated / shadow-only; a later
    /// boundary will reject version 0 under `FingerprintPrimary`. Calibration freezes the
    /// first accepted thresholds as version 1 (§6).
    pub threshold_profile_version: u32,
}

impl Default for FingerprintMatchParams {
    /// The initial `[TUNE]` runtime values (§6). Profile version 0 = uncalibrated.
    fn default() -> Self {
        Self {
            max_occurrences_per_hash_per_track: 64,
            max_occurrence_product: 4096,
            max_correspondences_per_pair: 50_000,
            // SHADOW-SAFE INERT: equal to the §9.2 sample cap, so a capped pair (grouped > 50000)
            // ALSO trips the resource ceiling ⇒ forced Ambiguous — reproducing the boundary-12
            // conflated behaviour BYTE-FOR-BYTE under profile 0. The §9.2/§9.3 SEPARATION (scoring the
            // sample normally) is a profile-2 feature, gated by raising this ceiling far above the cap.
            hard_correspondence_ceiling: 50_000,
            max_abs_drift_ppm: 2000.0,
            inlier_tolerance_ms: 50.0,
            min_support: 12,
            min_inlier_fraction: 0.15,
            // INERT at profile 0: u32::MAX means the absolute identity path can never trigger, so the
            // §10.6 OR collapses to the FRACTION path alone — byte-identical to boundary-12 shadow.
            min_abs_inlier_count: u32::MAX,
            min_abs_coverage: 0.35,
            min_temporal_coverage: 0.35,
            max_residual_p95_ms: 45.0,
            min_score_margin: 0.0,
            max_hypotheses: 1024,
            threshold_profile_version: 0,
        }
    }
}

impl FingerprintMatchParams {
    /// The FROZEN calibrated **profile version 1** thresholds (PLAN-SPECTRAL-FINGERPRINT-MATCHING
    /// §6 / §15.1.3). These are the FIRST accepted thresholds: tuned ONLY on the calibration split
    /// (the real `hsBach` difficult-match material + the synthetic families flagged
    /// `allowedForTuning:true`), frozen here, then measured unchanged on validation and holdout.
    /// `threshold_profile_version = 1` makes `FingerprintPrimary` VALID (the profile-0 default is
    /// shadow-only and is rejected under primary — see [`PairingParams::validate`]).
    ///
    /// The profile-0 [`Default`] values are deliberately KEPT for back-compat / shadow runs; this
    /// is a SEPARATE committed table. The calibration rationale for each value is documented in
    /// `fixtures/matcher-corpus/CALIBRATION.md` and pinned by `profile_v1_value_lock`. Re-tuning
    /// (a future corpus) must bump the version and regenerate that report — never edit in place
    /// after observing holdout (§20).
    ///
    /// Resource caps (`max_occurrences_per_hash_per_track`, `max_occurrence_product`,
    /// `max_correspondences_per_pair`, `max_hypotheses`) are NOT identity thresholds and are held at
    /// the profile-0 values: calibration confirmed they bound work without ever being the blocker on
    /// a true pair, and §15.3 says to optimize indexing/caps before weakening identity gates — none
    /// needed weakening, so the caps are unchanged.
    pub fn profile_v1() -> Self {
        Self {
            // --- Resource caps (bound work; §15.3 says OPTIMIZE caps before weakening identity gates). ---
            // ALL held at the profile-0 values. Calibration confirmed none was the blocker on a real
            // true pair (real test1 carried 9821 correspondences / 801 inliers without hitting them).
            // The CORRESPONDENCE cap deserves a note: a VALIDATION experiment showed the very long
            // schumann pair (1400 s, ~1.1 M raw correspondences) is `Ambiguous` BECAUSE the §9.3
            // resource-ceiling rule forces Ambiguous once the cap fires — and RAISING the cap does NOT
            // rescue it: at 200_000 it is still capped (Ambiguous), and UNCAPPED its true-line inlier
            // fraction collapses to 0.043 (the repetitive orchestral content yields ~1 M mostly-OUTLIER
            // correspondences), which falls below the identity gate ⇒ NoMatch. So no cap value turns
            // schumann into a confident Match; 50_000 is retained because it gives the HEALTHIEST
            // capped inlier fraction (0.138) and the lowest memory. This is a documented recall
            // limitation (the refined lag is nonetheless within 0.2 ms of truth — the ALIGNMENT is
            // correct, only the CLASSIFICATION stays Ambiguous), not a gate that weakening identity
            // thresholds could fix without risking the negatives. See CALIBRATION.md.
            max_occurrences_per_hash_per_track: 64,
            max_occurrence_product: 4096,
            max_correspondences_per_pair: 50_000,
            // Profile 1 predates the §9.3 conflation fix: it kept the boundary-12 behaviour where the
            // §9.2 sample cap ALSO forced Ambiguous. To preserve profile 1 byte-for-byte as a frozen
            // historical record, the hard ceiling EQUALS the §9.2 sample cap here, so a capped pair
            // trips the resource ceiling exactly as boundary 12 did (the §9.3 separation + the absolute
            // identity path are profile-2 features). See profile_v2().
            hard_correspondence_ceiling: 50_000,
            max_hypotheses: 1024,

            // --- Identity thresholds (the calibrated §10.6 gate levers). ---
            // Physical clock-rate bound. Held at 2000 ppm: the drift corpus spans ±600 ppm and the
            // real material is sub-400 ppm, so 2000 ppm admits every genuine map with margin while
            // still rejecting a nonphysical fit. The accepted real pairs sat at +32 / −305 / +364 ppm
            // (legacy reference), all well inside.
            max_abs_drift_ppm: 2000.0,
            // Inlier residual tolerance for the coarse consensus (ms). 50 ms ≈ 2.3 STFT frames at the
            // 21.3 ms frame period: wide enough that the quantized landmark times of a true map land
            // inside (real true pairs reached coverage 0.86–0.99 of their overlap at this tolerance),
            // tight enough that an unrelated coincidence does not accumulate inliers. SUB-MILLISECOND
            // offset accuracy is delivered by boundary-7 refinement, not this coarse tolerance.
            inlier_tolerance_ms: 50.0,
            // Minimum WEIGHTED inlier support for a Match. Held at 12. CALIBRATION EVIDENCE: weighted
            // support does NOT separate true from false here (the real negative test1_171 carried
            // support 114, ABOVE several true pairs), so support is not the discriminator — the inlier
            // FRACTION is. Held at 12 so the 8 s real excerpt (support 110) and the synthetic short
            // clips clear it; raising it would not have rejected the high-support negative anyway.
            min_support: 12,
            // Minimum WEIGHTED inlier fraction — the PRIMARY calibrated discriminator. LOWERED
            // 0.15 → 0.08. CALIBRATION EVIDENCE: on the real difficult-match material the three TRUE
            // pairs landed at inlier fractions 0.104 / 0.125 / 0.159 (lots of correspondences from
            // repetitive concert audio, only ~10–16 % inliers to the true line), while the three true
            // NEGATIVES (different bach takes vs the same ZOOM) sat at 0.004 / 0.003 / 0.013. There is
            // a clean ~8× gap: 0.08 admits every real true pair (the profile-0 0.15 REJECTED two of
            // them despite correct offsets + 0.97+ coverage) and still rejects every negative with a
            // 6× margin. Synthetic true pairs sit far above 0.08, so no synthetic recall is lost.
            min_inlier_fraction: 0.08,
            // The absolute identity path is a profile-2 feature; profile 1 keeps it INERT (the
            // historical record). u32::MAX ⇒ the OR collapses to the fraction path, exactly as the
            // frozen boundary-12 profile-1 behaviour.
            min_abs_inlier_count: u32::MAX,
            min_abs_coverage: 0.35,
            // Minimum temporal coverage (inlier A-span / overlap). Held at 0.35. The real true pairs
            // covered 0.86–0.99 of their overlap; the real negatives that had any line covered
            // 0.48–0.83 but were already rejected by the fraction gate. 0.35 still blocks the
            // localized-coincidence / duplicate-tower hazards (§10.7), and the dropout / mostly-silence
            // synthetic families whose live region itself spans ≳⅓ of the clip still pass.
            min_temporal_coverage: 0.35,
            // Maximum p95 inlier residual (ms). Held at 45 — NOT tightened. CALIBRATION EVIDENCE: the
            // real true pairs' coarse-consensus p95 was 43.4 / 40.5 / 36.9 ms; tightening to 40 ms
            // would have REJECTED the full-length true pair (p95 43.4). 45 ms passes all real true
            // pairs and the synthetic ones (p95 ≈ 0.04 ms) while still rejecting a diffuse fit.
            max_residual_p95_ms: 45.0,
            // Minimum uniqueness margin in [0,1]. Held at 0.0 — NOT raised. CALIBRATION EVIDENCE: the
            // real true pairs had LOW margins (0.037 / 0.046 / 0.001) because repetitive concert music
            // produces a near-equivalent runner-up, while some negatives had HIGHER margins
            // (0.087 / 0.223 / 0.352). Margin therefore does NOT separate true from false on this
            // material; raising it would have rejected the real true pairs. The repeated-content /
            // periodic synthetic hazards are held correct by the coverage + fraction gates, not margin.
            min_score_margin: 0.0,

            // The calibrated profile version. Makes FingerprintPrimary valid (§6).
            threshold_profile_version: 1,
        }
    }

    /// The FROZEN calibrated **profile version 2** (PLAN-SPECTRAL-FINGERPRINT-MATCHING §6 / §10.6 /
    /// §15). This is the boundary-12-DISCOVERED **recall fix** for long, dense, real recordings: a
    /// capped-but-all-gates-passing dense pair must classify `Match`, not be forced `Ambiguous` by the
    /// routine §9.2 sample cap (the §9.3 conflation fix), and the §10.6 identity gate gains a
    /// density-robust ABSOLUTE path so a true line whose inlier FRACTION is depressed by spurious
    /// dense correspondences — yet whose inlier COUNT, coverage, p95, drift, support and margin are all
    /// strong — is accepted (§10.6 sanctions this revision: "if the corpus proves a duration function
    /// is necessary, revise this plan"; here the function is density-robust, not duration-specific).
    ///
    /// Differences from [`Self::profile_v1`] (everything else IDENTICAL):
    /// - `hard_correspondence_ceiling`: a GENUINE resource ceiling FAR above the §9.2 sample cap (so
    ///   the routine sample is scored normally, only a pathological pair is forced Ambiguous, §9.3);
    /// - `min_abs_inlier_count` / `min_abs_coverage`: the absolute identity path's two thresholds,
    ///   CALIBRATION-derived to exclude the lone calibration negative (inlier count ~23) by a wide
    ///   margin while admitting every true pair (counts ≥ 1707, coverage ≥ ~0.9);
    /// - `threshold_profile_version = 2`.
    ///
    /// Every threshold value + its calibration / principle rationale is documented in
    /// `fixtures/matcher-corpus/CALIBRATION.md` and pinned by `profile_v2_value_lock`. Re-tuning (a
    /// future corpus) must bump the version and regenerate that report — never edit in place after
    /// observing the holdout (§20).
    pub fn profile_v2() -> Self {
        Self {
            // The §9.3 conflation fix: a genuine resource ceiling FAR above the §9.2 sample cap
            // (50000) AND comfortably above the largest GROUPED count any real concert pair produces.
            // PRINCIPLE-DERIVED (not tuned to a single pair): consensus only ever sees ≤ 50000
            // correspondences (the §9.2 sample), so this ceiling is purely a defensive bound on the
            // GROUPING memory for a pathological/degenerate input (e.g. a hash-collision storm). The
            // independent diagnosis measured the largest real grouped count at ~1.1 M (schumann,
            // 1400 s); 5_000_000 is 100× the sample cap and ~4.5× above that real maximum, so it never
            // fires on plausible real material (schumann's SAMPLED consensus is a clean all-gates Match
            // and is now scored normally) yet still caps a truly runaway grouping. Verified on
            // calibration: no real pair trips it (CALIBRATION.md §2.1).
            hard_correspondence_ceiling: 5_000_000,
            // --- The density-robust ABSOLUTE identity path (CALIBRATION-derived, §10.6). ---
            // The absolute path is the SAFEGUARD for a CAPPED long pair whose SAMPLED inlier fraction
            // still dips below min_inlier_fraction (0.08). It must EXCLUDE every negative by a wide
            // margin on BOTH clauses while admitting a true long line.
            //
            // min_abs_inlier_count = 500. CALIBRATION EVIDENCE (the unweighted inlier COUNT exposed as
            // PairEvidence.support): the three calibration NEGATIVES carry counts 13 / 10 / 114 (the
            // worst, test1_171, is 114); the independent diagnosis's holdout negative carried just 23.
            // The TRUE long pairs the path must rescue carry counts ≥ 1707 (diagnosis) — and the
            // calibration full-length true pair already shows count 801. 500 sits ~4.4× above the worst
            // calibration negative (114) and ~3.4× below the weakest diagnosed true long pair (1707):
            // a wide two-sided margin on the stable COUNT statistic (where FRACTION fails on dense
            // audio). The calibration true pairs with counts < 500 (267, 110) do NOT need this path —
            // they pass via the FRACTION gate (0.125 / 0.159 ≥ 0.08); the absolute path is a fallback,
            // never a relaxation.
            min_abs_inlier_count: 500,
            // min_abs_coverage = 0.90. CALIBRATION EVIDENCE: the calibration negatives that carried any
            // line covered 0.478 / 0.796 / 0.829 of the overlap; a true long pair spans nearly all of
            // it (calibration full-length 0.992; schumann 0.989; diagnosis long pairs 0.97–0.99). 0.90
            // sits above every negative's coverage and below every true long pair's, so the absolute
            // path additionally rejects a localized coincidence / duplicate-tower (high count, low
            // coverage). The general min_temporal_coverage (0.35) still applies to BOTH paths; this is
            // the stricter absolute-path floor. (PRINCIPLE: a true long line covers its overlap; a
            // coincidence does not.)
            min_abs_coverage: 0.90,

            // --- Everything below is IDENTICAL to profile_v1 (held by calibration, §3 of CALIBRATION.md). ---
            max_occurrences_per_hash_per_track: 64,
            max_occurrence_product: 4096,
            max_correspondences_per_pair: 50_000,
            max_hypotheses: 1024,
            max_abs_drift_ppm: 2000.0,
            inlier_tolerance_ms: 50.0,
            min_support: 12,
            min_inlier_fraction: 0.08,
            min_temporal_coverage: 0.35,
            max_residual_p95_ms: 45.0,
            min_score_margin: 0.0,

            // The re-calibrated profile version. Keeps FingerprintPrimary valid (§6).
            threshold_profile_version: 2,
        }
    }

    /// Validate the runtime thresholds. Rejects non-finite values, out-of-`[0,1]` fractions,
    /// and zero counts/limits. (The profile-version-0 rejection for `FingerprintPrimary` is a
    /// LATER boundary; here version 0 is a valid shadow-mode value.)
    pub fn validate(&self) -> Result<()> {
        for (name, v) in [
            ("maxAbsDriftPpm", self.max_abs_drift_ppm),
            ("inlierToleranceMs", self.inlier_tolerance_ms),
            ("minInlierFraction", self.min_inlier_fraction),
            ("minAbsCoverage", self.min_abs_coverage),
            ("minTemporalCoverage", self.min_temporal_coverage),
            ("maxResidualP95Ms", self.max_residual_p95_ms),
            ("minScoreMargin", self.min_score_margin),
        ] {
            if !v.is_finite() {
                return Err(CoupleError::invalid(format!("FingerprintMatchParams.{name} must be finite")));
            }
        }
        for (name, v) in [
            ("minInlierFraction", self.min_inlier_fraction),
            ("minAbsCoverage", self.min_abs_coverage),
            ("minTemporalCoverage", self.min_temporal_coverage),
            ("minScoreMargin", self.min_score_margin),
        ] {
            if !(0.0..=1.0).contains(&v) {
                return Err(CoupleError::invalid(format!(
                    "FingerprintMatchParams.{name} must be in [0, 1]"
                )));
            }
        }
        for (name, v) in [
            ("maxOccurrencesPerHashPerTrack", self.max_occurrences_per_hash_per_track),
            ("maxOccurrenceProduct", self.max_occurrence_product),
            ("maxCorrespondencesPerPair", self.max_correspondences_per_pair),
            ("hardCorrespondenceCeiling", self.hard_correspondence_ceiling),
            ("maxHypotheses", self.max_hypotheses),
        ] {
            if v == 0 {
                return Err(CoupleError::invalid(format!("FingerprintMatchParams.{name} must be > 0")));
            }
        }
        // §9.3: the HARD resource ceiling must sit at or above the ROUTINE §9.2 sample cap, else the
        // ordering is incoherent (a "ceiling" below the sample cap would force Ambiguous on the very
        // samples §9.2 says to score normally). `min_abs_inlier_count` has no upper bound — u32::MAX
        // is its INERT sentinel (the absolute identity path off), a deliberately valid value.
        if self.hard_correspondence_ceiling < self.max_correspondences_per_pair {
            return Err(CoupleError::invalid(
                "FingerprintMatchParams.hardCorrespondenceCeiling must be >= maxCorrespondencesPerPair",
            ));
        }
        if self.min_support == 0 {
            return Err(CoupleError::invalid("FingerprintMatchParams.minSupport must be > 0"));
        }
        for (name, v) in [
            ("maxAbsDriftPpm", self.max_abs_drift_ppm),
            ("inlierToleranceMs", self.inlier_tolerance_ms),
            ("maxResidualP95Ms", self.max_residual_p95_ms),
        ] {
            if v < 0.0 {
                return Err(CoupleError::invalid(format!("FingerprintMatchParams.{name} must be >= 0")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // §6 lock: `fixed()` returns EXACTLY the frozen table. A change here is a v2-format
    // decision, not an edit — this test makes that explicit and catches an accidental drift.
    #[test]
    fn fixed_format_value_lock() {
        let f = FingerprintFormatV1::fixed();
        assert_eq!(f.analysis_sample_rate, 12_000);
        assert_eq!(f.fft_size, 1024);
        assert_eq!(f.hop_size, 256);
        assert_eq!(f.min_frequency_hz, 100.0);
        assert_eq!(f.max_frequency_hz, 5500.0);
        assert_eq!(f.peak_time_radius_frames, 2);
        assert_eq!(f.peak_freq_radius_bins, 3);
        assert_eq!(f.peak_min_prominence_db, 6.0);
        assert_eq!(f.max_peaks_per_frame, 5);
        assert_eq!(f.max_peaks_per_second, 30);
        assert_eq!(f.frequency_quantization_bins, 2);
        assert_eq!(f.delta_time_quantization_frames, 2);
        assert_eq!(f.target_min_s, 0.5);
        assert_eq!(f.target_max_s, 4.0);
        assert_eq!(f.fanout, 5);
        // Default IS fixed() (so serde(default) fills from the canonical table).
        assert_eq!(FingerprintFormatV1::default(), f);
        // The frozen format validates.
        f.validate().expect("the fixed v1 format must validate");
    }

    // Derived quantities of the fixed format, so a later boundary can rely on these exactly.
    #[test]
    fn fixed_format_derived_quantities() {
        let f = FingerprintFormatV1::fixed();
        assert_eq!(f.bin_hz(), 12_000.0 / 1024.0); // ≈ 11.71875 Hz/bin
        assert_eq!(f.frame_period_s(), 256.0 / 12_000.0); // ≈ 21.3333 ms/frame
                                                          // 100 Hz → ceil(100/11.71875) = 9; 5500 Hz → floor(5500/11.71875)=469, ≤ Nyquist 512.
        assert_eq!(f.min_bin(), 9);
        assert_eq!(f.max_bin(), 469);
    }

    // §6: both structs round-trip through JSON identically (the artifact-header and
    // params-JSON contract). Also proves serde(camelCase) is wired.
    #[test]
    fn params_round_trip() {
        let fmt = FingerprintFormatV1::fixed();
        let j = serde_json::to_string(&fmt).unwrap();
        let back: FingerprintFormatV1 = serde_json::from_str(&j).unwrap();
        assert_eq!(fmt, back, "FingerprintFormatV1 must round-trip through JSON");
        // camelCase is on the wire.
        assert!(j.contains("analysisSampleRate") && j.contains("frequencyQuantizationBins"));

        let mp = FingerprintMatchParams::default();
        let j2 = serde_json::to_string(&mp).unwrap();
        let back2: FingerprintMatchParams = serde_json::from_str(&j2).unwrap();
        assert_eq!(mp, back2, "FingerprintMatchParams must round-trip through JSON");
        mp.validate().expect("default match params must validate");
    }

    // serde(default) on the FORMAT: an absent field is filled from fixed(), so a partial
    // (e.g. forward-compatible) JSON cannot silently zero a structural size. This is the
    // mechanism that keeps "ordinary JSON cannot mutate the v1 format into something illegal."
    #[test]
    fn format_partial_json_fills_from_fixed() {
        // Only state max_frequency_hz; everything else must come from fixed().
        let p: FingerprintFormatV1 = serde_json::from_str(r#"{"maxFrequencyHz": 5500.0}"#).unwrap();
        assert_eq!(p, FingerprintFormatV1::fixed(), "absent fields fill from fixed()");
    }

    // §15.1.3 FREEZE — the calibrated profile-1 VALUE LOCK. These are the FIRST accepted
    // thresholds, tuned only on calibration and frozen. A change here is a re-calibration decision
    // (bump the version + regenerate CALIBRATION.md), not an edit — this test makes that explicit.
    #[test]
    fn profile_v1_value_lock() {
        let p = FingerprintMatchParams::profile_v1();
        // The calibrated version is exactly 1 (this is what makes FingerprintPrimary valid, §6).
        assert_eq!(p.threshold_profile_version, 1, "profile_v1 must be version 1");
        // Resource caps — all held at the profile-0 values (not identity thresholds, §15.3).
        assert_eq!(p.max_occurrences_per_hash_per_track, 64);
        assert_eq!(p.max_occurrence_product, 4096);
        assert_eq!(p.max_correspondences_per_pair, 50_000);
        assert_eq!(p.max_hypotheses, 1024);
        // Identity thresholds — the calibrated §10.6 gate levers (frozen values).
        assert_eq!(p.max_abs_drift_ppm, 2000.0);
        assert_eq!(p.inlier_tolerance_ms, 50.0);
        assert_eq!(p.min_support, 12);
        assert_eq!(p.min_inlier_fraction, 0.08);
        assert_eq!(p.min_temporal_coverage, 0.35);
        assert_eq!(p.max_residual_p95_ms, 45.0);
        assert_eq!(p.min_score_margin, 0.0);
        // The density-robust absolute path is a profile-2 feature; profile 1 keeps it INERT so it is
        // byte-for-byte the historical boundary-12 profile (the OR collapses to the fraction path).
        assert_eq!(p.min_abs_inlier_count, u32::MAX, "profile_v1 leaves the absolute path inert");
        assert_eq!(
            p.hard_correspondence_ceiling, 50_000,
            "profile_v1 keeps the §9.2/§9.3 conflation: ceiling == sample cap ⇒ capped ⇒ Ambiguous"
        );
        // The frozen profile validates.
        p.validate().expect("the frozen profile_v1 must validate");
        // The profile-0 DEFAULT is deliberately KEPT for shadow/back-compat (a SEPARATE table).
        assert_eq!(
            FingerprintMatchParams::default().threshold_profile_version,
            0,
            "the profile-0 default is retained for shadow mode"
        );
        assert_ne!(
            FingerprintMatchParams::default(),
            p,
            "profile_v1 differs from the profile-0 default (it is calibrated)"
        );
    }

    // §15 RE-FREEZE — the calibrated profile-2 VALUE LOCK (the boundary-12-discovered recall fix). A
    // change here is a re-calibration decision (bump the version + regenerate CALIBRATION.md), not an
    // edit — this test makes that explicit and catches accidental drift.
    #[test]
    fn profile_v2_value_lock() {
        let p = FingerprintMatchParams::profile_v2();
        assert_eq!(p.threshold_profile_version, 2, "profile_v2 must be version 2");
        // The recall-fix levers (the ONLY differences from profile_v1).
        assert_eq!(p.hard_correspondence_ceiling, 5_000_000, "genuine resource ceiling, far above any real grouped count");
        assert_eq!(p.min_abs_inlier_count, 500, "absolute identity path count threshold (calibration-derived)");
        assert_eq!(p.min_abs_coverage, 0.90, "absolute identity path coverage threshold");
        // Everything else IDENTICAL to profile_v1 (held by calibration).
        assert_eq!(p.max_occurrences_per_hash_per_track, 64);
        assert_eq!(p.max_occurrence_product, 4096);
        assert_eq!(p.max_correspondences_per_pair, 50_000);
        assert_eq!(p.max_hypotheses, 1024);
        assert_eq!(p.max_abs_drift_ppm, 2000.0);
        assert_eq!(p.inlier_tolerance_ms, 50.0);
        assert_eq!(p.min_support, 12);
        assert_eq!(p.min_inlier_fraction, 0.08);
        assert_eq!(p.min_temporal_coverage, 0.35);
        assert_eq!(p.max_residual_p95_ms, 45.0);
        assert_eq!(p.min_score_margin, 0.0);
        // The frozen profile validates.
        p.validate().expect("the frozen profile_v2 must validate");
        // profile_v2 differs from profile_v1 in EXACTLY the three recall-fix fields + the version.
        let v1 = FingerprintMatchParams::profile_v1();
        assert_ne!(v1, p, "profile_v2 differs from profile_v1 (it is re-calibrated)");
        let v1_to_v2 = FingerprintMatchParams {
            hard_correspondence_ceiling: 5_000_000,
            min_abs_inlier_count: 500,
            min_abs_coverage: 0.90,
            threshold_profile_version: 2,
            ..v1
        };
        assert_eq!(v1_to_v2, p, "profile_v2 is profile_v1 + exactly {{ceiling, abs_count, abs_coverage, version}}");
    }

    // serde(default) on the MATCH params: a partial override fills the rest from Default.
    #[test]
    fn match_params_partial_override() {
        let p: FingerprintMatchParams = serde_json::from_str(r#"{"minSupport": 20}"#).unwrap();
        assert_eq!(p.min_support, 20);
        assert_eq!(p.max_hypotheses, FingerprintMatchParams::default().max_hypotheses);
        assert_eq!(p.threshold_profile_version, 0);
    }

    // §7.4 validation table: each infeasible/inconsistent FORMAT config must be rejected by
    // validate() (the "fail validation rather than truncate" rule, applied to format
    // feasibility even before hash packing exists).
    #[test]
    fn format_validation_table() {
        type Mutate = fn(&mut FingerprintFormatV1);
        let cases: &[(&str, Mutate)] = &[
            ("non-finite min freq", |f| f.min_frequency_hz = f64::NAN),
            ("inf max freq", |f| f.max_frequency_hz = f64::INFINITY),
            ("non-finite prominence", |f| f.peak_min_prominence_db = f64::NAN),
            ("zero analysis rate", |f| f.analysis_sample_rate = 0),
            ("zero fft size", |f| f.fft_size = 0),
            ("zero hop", |f| f.hop_size = 0),
            ("zero fanout", |f| f.fanout = 0),
            ("zero freq quant", |f| f.frequency_quantization_bins = 0),
            ("zero dt quant", |f| f.delta_time_quantization_frames = 0),
            ("non-pow2 fft", |f| f.fft_size = 1000),
            ("hop > fft", |f| f.hop_size = 2048),
            ("min freq <= 0", |f| f.min_frequency_hz = 0.0),
            ("max <= min freq", |f| {
                f.min_frequency_hz = 2000.0;
                f.max_frequency_hz = 1000.0;
            }),
            ("max freq > Nyquist", |f| f.max_frequency_hz = 7000.0),
            ("band too narrow for radius", |f| {
                // A band only one bin wide cannot host the ±3-bin neighborhood.
                f.min_frequency_hz = 1000.0;
                f.max_frequency_hz = 1011.0; // < 2 bins of 11.72 Hz
            }),
            ("target_min <= 0", |f| f.target_min_s = 0.0),
            ("target_max <= target_min", |f| {
                f.target_min_s = 4.0;
                f.target_max_s = 1.0;
            }),
            // §7.4 bit-width: a coarse-enough band + fine quantization overflows the 9-bit
            // frequency field. With analysis_rate raised so Nyquist admits many bins and
            // frequency_quantization_bins = 1, max_bin (≈ huge) / 1 exceeds 511.
            ("freq index overflows 9 bits", |f| {
                f.analysis_sample_rate = 24_000; // Nyquist 12 kHz
                f.max_frequency_hz = 12_000.0; // max_bin ≈ 512
                f.frequency_quantization_bins = 1; // index ≈ 512 ≥ 512
            }),
            // §7.4 bit-width: a long target window with fine delta quantization overflows the
            // 8-bit delta field. target_max 60 s ⇒ ~2812 frames / 2 ⇒ index ≫ 255.
            ("delta index overflows 8 bits", |f| {
                f.target_max_s = 60.0;
            }),
        ];
        for (label, mutate) in cases {
            let mut f = FingerprintFormatV1::fixed();
            mutate(&mut f);
            assert!(f.validate().is_err(), "format case '{label}' must be rejected by validate()");
        }
        // The frozen format still passes (guards against an over-broad rule above).
        assert!(FingerprintFormatV1::fixed().validate().is_ok());
    }

    // Runtime-threshold validation table: non-finite, out-of-[0,1], and zero-count rejects.
    #[test]
    fn match_params_validation_table() {
        type Mutate = fn(&mut FingerprintMatchParams);
        let cases: &[(&str, Mutate)] = &[
            ("nan drift", |p| p.max_abs_drift_ppm = f64::NAN),
            ("inf tol", |p| p.inlier_tolerance_ms = f64::INFINITY),
            ("inlier frac > 1", |p| p.min_inlier_fraction = 1.5),
            ("inlier frac < 0", |p| p.min_inlier_fraction = -0.1),
            ("coverage > 1", |p| p.min_temporal_coverage = 2.0),
            ("score margin > 1", |p| p.min_score_margin = 1.1),
            ("zero occ/hash", |p| p.max_occurrences_per_hash_per_track = 0),
            ("zero occ product", |p| p.max_occurrence_product = 0),
            ("zero correspondences", |p| p.max_correspondences_per_pair = 0),
            ("zero hypotheses", |p| p.max_hypotheses = 0),
            ("zero support", |p| p.min_support = 0),
            ("negative drift bound", |p| p.max_abs_drift_ppm = -1.0),
            ("negative tol", |p| p.inlier_tolerance_ms = -1.0),
            ("negative p95", |p| p.max_residual_p95_ms = -1.0),
            // The new §10.6 / §9.3 fields.
            ("abs coverage > 1", |p| p.min_abs_coverage = 1.5),
            ("abs coverage < 0", |p| p.min_abs_coverage = -0.1),
            ("nan abs coverage", |p| p.min_abs_coverage = f64::NAN),
            ("zero hard ceiling", |p| p.hard_correspondence_ceiling = 0),
            // The hard resource ceiling must not sit BELOW the routine §9.2 sample cap.
            ("hard ceiling < sample cap", |p| {
                p.max_correspondences_per_pair = 50_000;
                p.hard_correspondence_ceiling = 10_000;
            }),
        ];
        for (label, mutate) in cases {
            let mut p = FingerprintMatchParams::default();
            mutate(&mut p);
            assert!(p.validate().is_err(), "match-params case '{label}' must be rejected");
        }
        assert!(FingerprintMatchParams::default().validate().is_ok());
        // Profile version 0 is a VALID shadow-mode value here (the Primary rejection is a
        // later boundary) — assert it does not trip validation.
        let v0 = FingerprintMatchParams { threshold_profile_version: 0, ..Default::default() };
        assert!(v0.validate().is_ok(), "profile version 0 is valid in shadow mode");
    }

    // §20: no NaN/Infinity may reach JSON. serde_json does NOT error on a non-finite f64 — it
    // silently emits `null` (which would then fail to round-trip back to f64). The guard is
    // therefore `validate()`, which every entry point calls BEFORE serializing: a non-finite
    // field is rejected before it can be written. This test pins both halves of that contract:
    // (a) raw serde would lossily null a non-finite field (so we must never skip validation),
    // and (b) validate() rejects it.
    #[test]
    fn validation_blocks_non_finite_before_serialization() {
        let mut f = FingerprintFormatV1::fixed();
        f.max_frequency_hz = f64::NAN;
        // (a) Raw serde lossily emits null for the NaN — proof the guard cannot be skipped.
        let raw = serde_json::to_string(&f).expect("serde tolerates NaN by emitting null");
        assert!(raw.contains("null"), "raw serde nulls a NaN field: {raw}");
        // (b) validate() is the guard that prevents the non-finite from ever reaching serde.
        assert!(f.validate().is_err(), "validate() must reject a NaN format field");

        let p = FingerprintMatchParams { inlier_tolerance_ms: f64::INFINITY, ..Default::default() };
        let raw2 = serde_json::to_string(&p).expect("serde tolerates Inf by emitting null");
        assert!(raw2.contains("null"), "raw serde nulls an Inf field: {raw2}");
        assert!(p.validate().is_err(), "validate() must reject an Inf match-param field");
    }

    // §8.2 params hash: deterministic across calls (the §3 invariant-2 anchor for the hash).
    #[test]
    fn params_hash_is_stable() {
        let f = FingerprintFormatV1::fixed();
        assert_eq!(f.params_hash(), f.params_hash(), "params_hash must be deterministic");
    }

    // §8.2: the params hash CHANGES iff a FORMAT-DEFINING value changes, and does NOT depend on
    // classification thresholds. We mutate one format field at a time and require a different
    // hash; FingerprintMatchParams is not even an input, so it cannot move the hash.
    #[test]
    fn params_hash_changes_only_on_format_fields() {
        let base = FingerprintFormatV1::fixed().params_hash();

        type Mutate = fn(&mut FingerprintFormatV1);
        // Each mutation keeps the format VALID (so the change is the only variable), and must
        // change the hash. (We avoid mutations that would fail validate(); the hash is defined
        // for any format, but using valid neighbors keeps the intent clear.)
        let cases: &[(&str, Mutate)] = &[
            ("hop_size", |f| f.hop_size = 128),
            ("fft_size", |f| f.fft_size = 2048),
            ("min_frequency_hz", |f| f.min_frequency_hz = 120.0),
            ("max_frequency_hz", |f| f.max_frequency_hz = 5000.0),
            ("peak_time_radius_frames", |f| f.peak_time_radius_frames = 3),
            ("peak_freq_radius_bins", |f| f.peak_freq_radius_bins = 2),
            ("peak_min_prominence_db", |f| f.peak_min_prominence_db = 8.0),
            ("max_peaks_per_frame", |f| f.max_peaks_per_frame = 4),
            ("max_peaks_per_second", |f| f.max_peaks_per_second = 25),
            ("frequency_quantization_bins", |f| f.frequency_quantization_bins = 4),
            ("delta_time_quantization_frames", |f| f.delta_time_quantization_frames = 4),
            ("target_min_s", |f| f.target_min_s = 0.4),
            ("target_max_s", |f| f.target_max_s = 3.5),
            ("fanout", |f| f.fanout = 4),
        ];
        for (label, mutate) in cases {
            let mut f = FingerprintFormatV1::fixed();
            mutate(&mut f);
            assert_ne!(base, f.params_hash(), "changing {label} must change the params hash");
        }

        // analysis_sample_rate too (keep the band valid by lowering max_frequency_hz under the
        // new Nyquist so the format stays sane — though the hash is defined regardless).
        let mut f = FingerprintFormatV1::fixed();
        f.analysis_sample_rate = 16_000;
        assert_ne!(base, f.params_hash(), "changing analysis_sample_rate must change the hash");

        // Classification thresholds are NOT an input to the format hash — there is no API to feed
        // them in, which is the structural guarantee. We assert the fixed()-format hash is stable
        // regardless of any match-param value a caller might hold alongside it.
        let _mp = FingerprintMatchParams { min_support: 999, ..Default::default() };
        assert_eq!(base, FingerprintFormatV1::fixed().params_hash());
    }

    // Capture the actual fixed()-format params hash as a committed golden byte vector. This is
    // the single source of truth other format/file tests reference; if the encoding changes
    // deliberately (a v2 decision), regenerate this one constant.
    #[test]
    fn params_hash_golden_value() {
        let h = FingerprintFormatV1::fixed().params_hash();
        // Pinned bytes of BLAKE3("CPFPARAMS\x01" ‖ ordered fixed() fields)[..16].
        assert_eq!(
            h,
            super::FIXED_PARAMS_HASH,
            "fixed() params hash drifted; if this was an intentional v2-format change, regenerate \
             FIXED_PARAMS_HASH and the CPF1 golden artifact"
        );
    }
}
