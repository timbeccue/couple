# PLAN - Spectral-landmark audio matching

Status: implementation plan. Prerequisite:
`PLAN-AUDIO-MATCHING-PREWORK.md` is complete and all of its gates pass.

Repository root:
`/Users/tim/code/claude_apps/couple`

Written 2026-06-14.

---

## 1. Objective

Replace broadband amplitude-envelope correlation as the primary recording-identity
mechanism with a hybrid matcher:

1. Sparse spectral landmarks identify recordings of the same acoustic event.
2. Landmark time correspondences robustly estimate offset and clock scale.
3. Metadata contributes hypotheses but never vetoes audio evidence.
4. The existing envelope/raw-audio machinery performs final sub-millisecond refinement.
5. Existing resolver coverage behavior, editor sync, session state, audition, and export
   remain intact.

The new matcher must improve:

- 8-20 second genuine clips;
- same-piece/different-take discrimination;
- repeated passages and periodic content;
- recordings with different microphones, AGC, filtering, or reverberation;
- large candidate pools;
- moderate clock drift.

The system must continue preferring `ambiguous` over a confident wrong pairing.

---

## 2. Architectural decision

This is not a replacement for the entire sync engine.

Replace:

```text
candidate identity and coarse time-model discovery
```

Retain:

```text
canonical PCM import
peak pyramid
local envelope/raw refinement
coverage calculation
queue/session/editor behavior
drift-mode application
audition
export
```

The new primary path is:

```text
import PCM
  -> cached spectral landmarks
  -> batch landmark lookup
  -> robust affine consensus
  -> method-neutral PairEvidence
  -> existing fine local refinement
  -> existing row coverage/resolution
  -> optional graph consistency check
```

The legacy envelope matcher remains available as:

- a fallback when fingerprints cannot be built;
- a diagnostic comparison path;
- the final local refinement seed when landmark evidence is accepted.

---

## 3. Non-negotiable invariants

1. Lag convention remains:
   `t_b = t_a * (1 + drift_ppm * 1e-6) + lag_ms / 1000`.
2. The matcher is deterministic. No nondeterministic RANSAC or hash iteration.
3. Metadata may add or rank a hypothesis but MUST NOT create a match without audio
   support and MUST NOT veto strong audio evidence.
4. Different performances of the same composition are non-matches unless landmark
   correspondences establish the same captured event.
5. Chroma, semantic embeddings, speech embeddings, and music-recognition labels are out
   of scope. They answer content similarity, not recording identity.
6. No cloud service or network dependency.
7. Old cache entries remain usable. Missing fingerprints are derived from cached PCM.
8. Existing `PairCorrelation` and old FFI symbols remain during rollout.
9. The new matcher begins in shadow mode and is enabled only after holdout gates pass.
10. Fine alignment remains owned by the existing sync/measure code.

---

## 4. Target module and artifact layout

Add:

```text
core/src/fingerprint/
  mod.rs
  params.rs
  builder.rs
  peaks.rs
  landmarks.rs
  file.rs
  index.rs
  match_pair.rs
  robust_affine.rs
  metadata_hint.rs

core/src/matching.rs
core/src/match_graph.rs

core/tests/fingerprint_builder.rs
core/tests/fingerprint_file.rs
core/tests/fingerprint_match.rs
core/tests/fingerprint_robustness.rs
core/tests/match_graph.rs
cli/tests/fingerprint_gates.rs
```

Add one derived cache artifact per track:

```text
fingerprint-v1.cpf
```

Do not put fingerprints into `pyramid.cpk`. They have independent format/version
lifecycle and should be rebuildable from `pcm48f32.raw`.

---

## 5. Public core data contracts

### 5.1 Method-neutral evidence

Create `core/src/matching.rs`.

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchMethod {
    SpectralLandmarkV1,
    EnvelopeV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchClassification {
    Match,
    NoMatch,
    Ambiguous,
    InsufficientOverlap,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairEvidence {
    pub method: MatchMethod,
    pub classification: MatchClassification,

    // Mapping from A time to B time.
    pub lag_ms: f64,
    pub drift_ppm: f64,

    // Method-neutral ranking and uniqueness. Higher is better.
    // Scores rank candidates produced by the same matcher version; they are not
    // user-visible probabilities.
    pub score: f64,
    pub runner_up_score: f64,
    pub score_margin: f64,

    // Consensus quality.
    pub support: u32,
    pub candidate_correspondences: u32,
    pub inlier_fraction: f64,
    pub temporal_coverage: f64,
    pub residual_rms_ms: Option<f64>,
    pub residual_p95_ms: Option<f64>,
    pub overlap_s: f64,

    // Fine refinement of the accepted affine hypothesis.
    pub refined_lag_ms: Option<f64>,
    pub refined_drift_ppm: Option<f64>,
    pub refined_rms_ms: Option<f64>,

    // Optional legacy telemetry for shadow comparison only.
    pub legacy: Option<crate::pair::PairCorrelation>,
}
```

Rules:

- `lag_ms` and `drift_ppm` come from landmark consensus.
- Resolvers use `classification`, `score`, and the refined mapping.
- Resolvers MUST NOT reinterpret fingerprint support thresholds.
- `score` is finite. Failed/unavailable measurements use `0.0`.
- Non-applicable residuals serialize as null through the optional fields. Do not
  serialize NaN or Infinity.

### 5.2 Matcher mode

Extend `PairingParams`:

```rust
pub enum MatcherMode {
    LegacyEnvelope,
    FingerprintShadow,
    FingerprintPrimary,
}

pub struct PairingParams {
    // prework fields
    pub matcher_mode: MatcherMode,
    pub fingerprint_match: FingerprintMatchParams,
    pub graph_offset_tol_ms: f64,
    pub graph_drift_tol_ppm: f64,
    pub graph_min_direct_support_ratio: f64,
    pub graph_required_independent_paths: usize,
}
```

Initial graph values:

```text
graph_offset_tol_ms                  100.0
graph_drift_tol_ppm                  150.0
graph_min_direct_support_ratio       0.75
graph_required_independent_paths     2
```

`PairingParams::default()` remains `LegacyEnvelope` until Phase 10 gates pass.
Development and calibration explicitly request `FingerprintShadow` through the CLI,
debug environment, or parameter JSON. Do not make shadow computation an accidental
cost of every normal pairing operation.

### 5.3 New core operations

Add:

```rust
impl Ctx {
    pub fn match_pair(
        &self,
        a: TrackId,
        b: TrackId,
        params: &PairingParams,
    ) -> Result<PairEvidence>;

    pub fn match_candidates(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        params: &PairingParams,
    ) -> Result<Vec<Vec<PairEvidence>>>;
}
```

`match_candidates` MUST build one audio-side inverted index for the batch. It must not
call a naive independent pair matcher for every matrix cell.

---

## 6. Fingerprint parameters

Create `core/src/fingerprint/params.rs`.

Separate format-defining values from runtime classification thresholds. A loaded track
holds one `fingerprint-v1.cpf`, so production code MUST NOT allow the v1 analysis format
to be changed through ordinary pairing parameter JSON.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintFormatV1 {
    pub analysis_sample_rate: u32,
    pub fft_size: usize,
    pub hop_size: usize,
    pub min_frequency_hz: f64,
    pub max_frequency_hz: f64,
    pub peak_time_radius_frames: usize,
    pub peak_freq_radius_bins: usize,
    pub peak_min_prominence_db: f64,
    pub max_peaks_per_frame: usize,
    pub max_peaks_per_second: usize,
    pub frequency_quantization_bins: u32,
    pub delta_time_quantization_frames: u32,
    pub target_min_s: f64,
    pub target_max_s: f64,
    pub fanout: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FingerprintMatchParams {
    pub max_occurrences_per_hash_per_track: usize,
    pub max_occurrence_product: usize,
    pub max_correspondences_per_pair: usize, // routine §9.2 SAMPLE cap (score the sample normally)
    pub hard_correspondence_ceiling: usize,  // genuine §9.3 resource ceiling (>> sample cap) ⇒ Ambiguous
    pub max_abs_drift_ppm: f64,
    pub inlier_tolerance_ms: f64,
    pub min_support: u32,
    pub min_inlier_fraction: f64,            // §10.6 identity gate — FRACTION path
    pub min_abs_inlier_count: u32,           // §10.6 identity gate — ABSOLUTE path COUNT (u32::MAX = off)
    pub min_abs_coverage: f64,               // §10.6 identity gate — ABSOLUTE path COVERAGE
    pub min_temporal_coverage: f64,
    pub max_residual_p95_ms: f64,
    pub min_score_margin: f64,
    pub max_hypotheses: usize,
    pub threshold_profile_version: u32,
}
```

`FingerprintFormatV1::fixed()` returns the exact format values below. Changing one
requires a `FingerprintFormatV2`, `fingerprint-v2.cpf`, and a new `MatchMethod` variant.

Format values:

```text
analysis_sample_rate                   12000
fft_size                              1024
hop_size                              256
min_frequency_hz                      100
max_frequency_hz                      5500
peak_time_radius_frames               2
peak_freq_radius_bins                 3
peak_min_prominence_db                6
max_peaks_per_frame                   5
max_peaks_per_second                  30
frequency_quantization_bins           2
delta_time_quantization_frames        2
target_min_s                          0.5
target_max_s                          4.0
fanout                                5
```

Initial runtime matching values, all `[TUNE]`:

```text
max_occurrences_per_hash_per_track    64
max_occurrence_product                4096
max_correspondences_per_pair          50000
hard_correspondence_ceiling           50000        (profile 0: == sample cap ⇒ a capped pair trips the ceiling ⇒ Ambiguous, reproducing boundary 12)
max_abs_drift_ppm                     2000
inlier_tolerance_ms                   50
min_support                           12
min_inlier_fraction                   0.15
min_abs_inlier_count                  u32::MAX     (profile 0: absolute identity path INERT)
min_abs_coverage                      0.35
min_temporal_coverage                 0.35
max_residual_p95_ms                   45
min_score_margin                      0.0
max_hypotheses                        1024
threshold_profile_version             0
```

Profile version 0 means uncalibrated and is valid only in shadow mode.
`FingerprintPrimary` parameter validation MUST reject profile version 0. Calibration
freezes the first accepted thresholds as profile version 1.

**Threshold profile 2 (the boundary-12 recall fix).** The boundary-12 corpus proved the §9.2/§9.3
conflation + the density-fragile fraction gate collapse recall on long real recordings (§9.3, §10.6).
Profile 2 is the re-calibrated freeze; it differs from profile 1 in exactly three identity/resource
fields (everything else held — see `fixtures/matcher-corpus/CALIBRATION.md`):

```text
hard_correspondence_ceiling           5000000  (genuine §9.3 ceiling; 100x the §9.2 sample cap, ~4.5x above the ~1.1M real max)
min_abs_inlier_count                  500      (absolute §10.6 path; ~4.4x above the worst calibration negative's 114, ~3.4x below the weakest true long pair's 1707)
min_abs_coverage                      0.90     (absolute §10.6 path; above every negative's coverage, below true long pairs' 0.97-0.99)
threshold_profile_version             2
```

Profile 2 keeps `FingerprintPrimary` valid. Switching the DEFAULT matcher to `FingerprintPrimary`
remains gated on a FRESH real holdout (the boundary-12 holdout is now contaminated by the diagnosis);
that is a later boundary, not part of this revision.

Do not tune runtime values on holdout.

---

## 7. Phase 1 - Fingerprint signal builder

### 7.1 Downsample deterministically

Input is canonical mono 48 kHz PCM.

Downsample to 12 kHz with two fixed 2:1 half-band FIR stages:

- stage 1: 48 kHz to 24 kHz;
- stage 2: 24 kHz to 12 kHz;
- each stage is a symmetric 63-tap half-band Kaiser filter, beta 8.6;
- exploit zero-valued half-band coefficients so the implementation performs only the
  non-zero multiplies;
- passband extends through at least 90% of the output Nyquist frequency;
- coefficients are checked-in constants generated by one documented script or build
  helper and protected by a coefficient golden test;
- maintain filter state across `push` calls;
- account for cumulative group delay consistently when assigning fingerprint frame
  times.

Do not use ffmpeg for this derived analysis. Do not alter cached PCM.

### 7.2 STFT

Use:

- 1024-sample Hann window at 12 kHz;
- 256-sample hop;
- `realfft`;
- power spectrum converted to `10 * log10(max(power, epsilon))`;
- bins restricted to 100-5500 Hz.

`FingerprintBuilder::push(&[f32])` must be streaming and chunk-boundary invariant.

Test the same PCM pushed:

- all at once;
- one sample at a time;
- random deterministic chunk sizes.

The resulting landmarks must be byte-identical.

### 7.3 Spectral peak selection

A peak is eligible when:

1. it is a maximum in the configured time/frequency neighborhood;
2. equal maxima select exactly one peak by lower frequency bin, then earlier frame;
3. it exceeds the local frequency-neighborhood median by
   `peak_min_prominence_db`;
4. it is inside the configured frequency range.

After eligibility:

- keep the strongest `max_peaks_per_frame`;
- enforce `max_peaks_per_second` with a deterministic sliding one-second cap;
- sort retained peaks by time, then frequency.

Do not normalize using whole-track maximum. Peak selection must tolerate sections with
different loudness and AGC.

### 7.4 Landmark generation

For each anchor peak:

- consider target peaks between `target_min_s` and `target_max_s` later;
- choose up to `fanout` targets by strongest magnitude;
- tie-break by earlier time, then lower frequency;
- emit one landmark per anchor-target pair.

Hash payload:

```text
anchor frequency bin: 9 bits
target frequency bin: 9 bits
delta-time hop count: 8 bits
reserved/version:      6 bits
```

Before packing:

```text
anchor frequency index = fft_bin / frequency_quantization_bins
target frequency index = fft_bin / frequency_quantization_bins
delta-time index        = hop_delta / delta_time_quantization_frames
```

Use integer floor division. Quantization makes hashes tolerant of small clock-rate and
peak-bin shifts. Validate that maximum packed values fit their bit widths. Set all
reserved/version bits to zero for v1.

If the configured ranges do not fit, fail parameter validation rather than truncate.

Store:

```rust
pub struct Landmark {
    pub hash: u32,
    pub anchor_frame: u32,
}
```

Sort by `(hash, anchor_frame)` before serialization.

### 7.5 Builder tests

Required tests:

1. chunk-boundary invariance;
2. gain invariance over at least +/-24 dB;
3. deterministic peak tie-breaking;
4. no peaks from digital silence;
5. bounded landmark density on sustained tones;
6. stable landmarks under AGC and moderate filtering;
7. a different random event has negligible exact-hash overlap;
8. two excerpts of the same master share hashes at the correct offset.

---

## 8. Phase 2 - Versioned fingerprint cache

### 8.1 File format

Implement `core/src/fingerprint/file.rs`.

Header, little-endian:

```text
magic                 4 bytes  "CPF1"
format_version        u32      1
analysis_sample_rate  u32
fft_size              u32
hop_size              u32
min_frequency_millihz u32
max_frequency_millihz u32
params_hash           [u8; 16]
duration_frames       u64      canonical 48 kHz frames
landmark_count        u64
header_blake3_32      u32
```

Body:

```text
landmark_count repetitions of:
  hash          u32
  anchor_frame  u32
```

Footer:

```text
body_blake3_128 [u8; 16]
```

Reject:

- wrong magic/version;
- incompatible params hash;
- unsorted landmarks;
- truncated body;
- checksum mismatch;
- anchor frames beyond duration;
- unreasonable landmark counts before allocation.

`header_blake3_32` is the little-endian value of the first four BLAKE3 digest bytes over
all preceding header bytes. No CRC dependency is required.

### 8.2 Params hash

Hash every format-defining analysis and landmark parameter in an explicitly ordered
binary encoding. Do not hash JSON text.

Classification thresholds are not format-defining and do not invalidate the artifact.

### 8.3 Cache lifecycle

Add `CacheEntry::fingerprint_path_v1()`.

Behavior:

1. New imports generate the fingerprint in the existing decode pass.
2. Existing valid cache entries without a fingerprint remain valid.
3. On first fingerprint request, build from the mmap PCM cache.
4. Write to a temporary file and atomically rename.
5. Concurrent requests for the same track build at most once in-process using
   `OnceLock` or an equivalent per-track synchronization primitive.
6. A corrupt fingerprint is deleted/rebuilt without deleting PCM or pyramid artifacts.
7. Cache GC counts fingerprint bytes automatically because it removes the whole entry.

Add to `TrackData`:

```rust
fingerprint: OnceLock<Arc<FingerprintReader>>
```

Expose:

```rust
pub fn fingerprint_v1(&self) -> Result<Arc<FingerprintReader>>;
```

### 8.4 Cache tests

Test:

- build/write/mmap round trip;
- old cache without fingerprint lazily upgrades;
- corrupt file rebuild;
- incompatible params hash rebuild;
- two concurrent callers receive the same artifact;
- any builder or write error leaves no `.tmp` file;
- existing `CACHE_META_VERSION` entries remain readable.

---

## 9. Phase 3 - Landmark lookup and correspondence generation

### 9.1 Batch audio index

Implement `core/src/fingerprint/index.rs`.

For one `match_candidates(videos, audios)` call, build:

```rust
hash -> sorted list of (audio_index, anchor_frame)
```

Use a sorted vector/range index or deterministic ordered map. Do not rely on randomized
hash-map iteration for result order.

Suppress uninformative hashes:

- discard a hash for a track when occurrences exceed
  `max_occurrences_per_hash_per_track`;
- discard a video-hash/audio-hash group when occurrence product exceeds
  `max_occurrence_product`;
- record discarded counts as debug telemetry.

### 9.2 Correspondences

For every shared hash:

```rust
pub struct Correspondence {
    pub t_a_s: f64,
    pub t_b_s: f64,
    pub weight: f64,
}
```

Weight:

```text
weight = 1 / sqrt(occurrences_in_a * occurrences_in_b)
```

Sort correspondences by `(t_a_s, t_b_s)`.

Before applying the correspondence cap, group records with the same
`(a_anchor_frame, b_anchor_frame)`. Emit one correspondence per frame pair with:

```text
weight = min(1.0, sum of contributing hash weights)
```

This prevents one dense anchor fanout from pretending to be several independent timing
observations.

If count exceeds `max_correspondences_per_pair`, deterministically retain a
time-stratified sample:

1. divide A duration into 128 equal bins;
2. allocate the cap proportionally, with at least one from non-empty bins;
3. within a bin retain highest weight;
4. tie-break by time.

Never take the first N records, which biases early audio.

### 9.3 No-evidence classifications

- overlap below `PairingParams.min_overlap_s`:
  `InsufficientOverlap`;
- fingerprint unavailable but legacy matcher works:
  method `EnvelopeV1`;
- fingerprints available with no affine hypothesis:
  `NoMatch`;
- best support below `min_support` but at least `ceil(min_support / 2)`, with a
  non-equivalent runner-up and `score_margin < min_score_margin`:
  `Ambiguous`;
- all other below-support cases:
  `NoMatch`;
- **the routine §9.2 SAMPLE cap fired** (`grouped > max_correspondences_per_pair`): this is a
  SAMPLING event, NOT a resource ceiling. Retain the representative 128-bin subset and SCORE IT
  NORMALLY (map the affine class exactly like an uncapped pair). The sample cap MUST NOT by itself
  force `Ambiguous`;
- **the genuine RESOURCE CEILING exceeded** (`grouped > hard_correspondence_ceiling`, a threshold FAR
  above the §9.2 sample cap, or no representative sample could be formed):
  `Ambiguous`, with telemetry, never `NoMatch`.

> **Boundary-12 recall fix (§9.2/§9.3 separation).** The original spec conflated the routine §9.2
> sample cap with the §9.3 resource ceiling — forcing `Ambiguous` whenever the sample cap fired. On
> EVERY long dense real pair (> `max_correspondences_per_pair` grouped correspondences) this discarded
> a clean, all-gates-passing consensus, collapsing recall (calibration §3.3: 6 of 7 holdout true pairs
> were `Ambiguous` solely because of this). The two are now distinct signals: the §9.2 sample is
> scored normally; only `hard_correspondence_ceiling` (a genuinely-exhausted pair) forces `Ambiguous`.
> See §10.6 for the companion density-robust identity gate.

---

## 10. Phase 4 - Deterministic robust affine consensus

### 10.1 Model

Fit:

```text
t_b = intercept_s + scale * t_a
lag_ms = intercept_s * 1000
drift_ppm = (scale - 1) * 1_000_000
```

Create `core/src/fingerprint/robust_affine.rs`.

### 10.2 Hypothesis generation

Generate hypotheses deterministically:

1. Add scale-1 hypotheses from the 16 highest smoothed offset-histogram peaks.
   Histogram bin width is 100 ms; smooth with a fixed +/-2-bin triangular kernel.
2. Add two-point hypotheses from correspondences selected by this deterministic
   low-discrepancy sequence:

   ```text
   for k = 1, 2, ...:
       i = floor(van_der_corput(k, base=2) * n)
       j = floor(van_der_corput(k, base=3) * n)
   ```

   Skip `i == j`, duplicate index pairs, and pairs failing the time-separation rule.
   Examine at most `8 * max_hypotheses` sequence positions.
3. Reject point pairs separated by less than 5 seconds in A.
4. Reject non-positive scale or drift outside `max_abs_drift_ppm`.
5. Deduplicate hypotheses quantized to 10 ms intercept and 10 ppm scale.
6. Stop at `max_hypotheses`.

Do not add RNG-based sampling.

### 10.3 Hypothesis scoring

For every hypothesis:

```text
residual_ms = 1000 * abs(t_b - (intercept + scale * t_a))
inlier      = residual_ms <= inlier_tolerance_ms
```

Score:

```text
weighted_support
  * sqrt(max(temporal_coverage, 0))
  * max(inlier_fraction, epsilon)
  / (1 + residual_p95_ms / inlier_tolerance_ms)
```

Where:

- weighted support is the sum of correspondence weights;
- temporal coverage is the inlier A-time span divided by candidate overlap;
- inlier fraction is weighted inlier support divided by total weighted support.

Tie-break in this exact order:

1. higher score;
2. higher unweighted support;
3. higher temporal coverage;
4. lower p95 residual;
5. smaller absolute drift;
6. smaller absolute lag;
7. lower lag.

### 10.4 Refinement

For the best hypothesis:

1. fit weighted least squares to inliers;
2. apply Huber IRLS for three fixed iterations;
3. Huber delta equals `inlier_tolerance_ms / 2`;
4. reject points beyond the full inlier tolerance after each iteration;
5. enforce the drift bound;
6. compute final support, RMS, p95, fraction, and temporal coverage.

No convergence-based variable iteration count. Fixed iteration count preserves
deterministic cost and output.

For each IRLS iteration:

```text
huber_weight = 1                        when abs(residual_ms) <= delta
             = delta / abs(residual_ms) otherwise
final_weight = correspondence_weight * huber_weight
```

Compute weighted means, weighted covariance, slope, and intercept directly in f64.
Reject weighted time variance below `1e-12`.

### 10.5 Runner-up uniqueness

Find a runner-up after excluding hypotheses equivalent to the winner:

```text
abs(lag difference) < 250 ms
AND
abs(drift difference) < 50 ppm
```

Store:

```text
runner_up_score = 0 when no non-equivalent runner-up exists
score_margin = (best_score - runner_up_score) / max(best_score, epsilon)
```

`score_margin` is therefore a dimensionless uniqueness measure in `[0, 1]`, not an
absolute score difference.

Repeated content commonly produces two strong non-equivalent hypotheses. Such a pair
must classify `Ambiguous` until uniqueness clears the calibrated threshold.

### 10.6 Initial classification

Classify `Match` only when all are true:

```text
support >= min_support
identity_ok
temporal_coverage >= min_temporal_coverage
residual_p95_ms <= max_residual_p95_ms
score_margin >= min_score_margin
abs(drift_ppm) <= max_abs_drift_ppm
```

where the **density-robust identity gate** is an OR of two complementary paths:

```text
identity_ok =
       (inlier_fraction >= min_inlier_fraction)
    OR (inlier_count >= min_abs_inlier_count AND temporal_coverage >= min_abs_coverage)
```

If either residual option is absent, classification cannot be `Match`.

> **Boundary-12 recall fix (the §10.6-sanctioned plan revision).** The single inlier-FRACTION
> predicate is density-fragile: dense orchestral audio yields many spurious correspondences, so a
> true line's inlier FRACTION is low (calibration measured 0.063–0.088 on real long pairs) even when
> its inlier COUNT (1707–3014), coverage (0.97–0.99), p95 (31–43 ms), support, drift and margin are
> all strong and stable. RAISING the §9.2 cap is strictly worse (the §9.2 sample keeps the
> highest-weight correspondences, so the CAPPED fraction is BETTER than the uncapped one). §10.6
> explicitly permits this: *"If the corpus proves a duration function is necessary, revise this plan
> and add the function plus boundary tests before coding it."* The corpus proved it — but the function
> is **density-robust, not duration-specific** (no `if duration > X`): an absolute COUNT + COVERAGE
> path that fires precisely when dense audio depresses the fraction. The 0-confident-false-match safety
> survives every hazard: it still requires a refined AFFINE CONSENSUS (no classification from a raw
> hash count, §20), a HIGH inlier COUNT (≫ any two-point coincidence — the no-two-point-zero-RMS guard
> stays), HIGH coverage (blocks a localized coincidence / duplicate-tower), and the UNCHANGED
> p95/drift/support/margin gates (repeated content still depresses the margin ⇒ `Ambiguous`). The lone
> calibration negative is separated from every true pair by ~70× in inlier count (23 vs ≥ 1707).

The two new `[TUNE]` thresholds (§6):

```text
min_abs_inlier_count   absolute inlier COUNT for the absolute identity path (u32::MAX = path off)
min_abs_coverage       absolute temporal COVERAGE for the absolute identity path
```

At threshold profile 0 the absolute path is INERT (`min_abs_inlier_count = u32::MAX`), so the OR
collapses to the fraction path — shadow behaviour is byte-for-byte the boundary-12 path. The frozen
profile-2 values are in §6 (and `fixtures/matcher-corpus/CALIBRATION.md`). Beyond this density
function, version 2 still has NO duration-specific threshold exceptions; do not scatter
duration-specific `if` statements through the matcher.

### 10.7 Robust-affine tests

Test:

1. exact affine line;
2. +/-2000 ppm bounds;
3. 50% random outliers;
4. duplicated correspondences do not inflate support after deduplication;
5. repeated-offset runner-up causes ambiguity;
6. input ordering does not change output bits;
7. correspondence cap preserves a late true segment;
8. no two-point zero-RMS shortcut can classify a match;
9. p95 and temporal coverage gates reject a localized coincidence.

---

## 11. Phase 5 - Fine refinement with existing machinery

### 11.1 Seed local measurement

For a landmark `Match` or strong `Ambiguous` hypothesis:

1. Use affine lag at selected anchor times:

   ```text
   predicted_lag_ms(t) = lag_ms + t * drift_ppm * 1e-3
   ```

2. Select 6-16 unique anchors across the inlier-covered span.
3. Call the existing `measure_local` around each predicted lag with a narrow span.
4. Fit these local measurements using the robust verifier fitter from the prework plan.

Do not call the old global envelope scan again merely to rediscover the offset.

### 11.2 Refined result

Populate:

- `refined_lag_ms`;
- `refined_drift_ppm`;
- `refined_rms_ms`.

Use the refined mapping for coverage and later full sync.

If fine refinement fails:

- retain strong landmark identity evidence;
- classify `Ambiguous`, not `NoMatch`, unless corpus evidence proves landmark-only
  acceptance safe;
- expose a reason in debug telemetry.

### 11.3 Full sync integration

Add an overload or seed parameter so full sync can begin from accepted fingerprint
offset/drift rather than rerunning stage 1.

Do not change:

- final status thresholds;
- session write-on-success behavior;
- editor-visible drift windows;
- export mapping math.

### 11.4 Accuracy gates

For accepted true pairs:

- refined offset error <= 1 ms for overlap >= 20 s;
- <= 3 ms for 8-20 s clips;
- drift error follows existing duration-dependent sync tolerances;
- exported audio re-sync regression remains within existing bounds.

---

## 12. Phase 6 - Metadata hypotheses

### 12.1 Parse hints

Create `metadata_hint.rs`.

Sources:

1. BWF `time_reference_samples / source_sample_rate`;
2. BWF date/time when complete;
3. container creation time;
4. same-device ordering hints from stable device tags.

Represent:

```rust
pub struct TimeHint {
    pub lag_ms: f64,
    pub tolerance_ms: f64,
    pub source: TimeHintSource,
    pub reliability: HintReliability,
}
```

### 12.2 Reliability rules

- BWF sample reference with compatible dates: medium reliability.
- Creation-time-only cross-device hint: low reliability.
- Implausible date disagreement: keep as debug data, do not use for ranking.
- Same-device monotonic ordering: may order equal audio-evidence candidates, never
  accept or reject one.

### 12.3 Use

Metadata may:

- add an affine hypothesis to the robust-consensus candidate list;
- break an exact audio-score tie;
- improve debug explanations.

Metadata may not:

- restrict landmark lookup to a time window;
- lower audio support thresholds;
- turn `NoMatch` into `Match`;
- override a better audio hypothesis.

Tests must include the known camera/Zoom 11.6-year disagreement and prove it cannot
harm the correct audio match.

---

## 13. Phase 7 - Cross-device graph consistency

### 13.1 Scope

After per-pair evidence is available, build a graph:

- node: imported track;
- directed edge: accepted or ambiguous affine mapping;
- edge weight: evidence score and uncertainty.

Implement in `core/src/match_graph.rs`.

### 13.2 Cycle residual

For triangle A -> B -> C -> A, compose affine maps and measure:

- offset residual at the start, midpoint, and end of common coverage;
- scale/drift residual.

Do not compare only one scalar lag when drift is non-zero.

For map `A -> B` represented by `(scale_ab, intercept_ab)` and `B -> C` by
`(scale_bc, intercept_bc)`, compose exactly:

```text
scale_ac     = scale_bc * scale_ab
intercept_ac = scale_bc * intercept_ab + intercept_bc
```

The inverse of `A -> B` is:

```text
scale_ba     = 1 / scale_ab
intercept_ba = -intercept_ab / scale_ab
```

Reject non-positive or non-finite scales before graph construction.

### 13.3 Graph action

Promotion is allowed only when all are true:

1. The direct edge is `Ambiguous`, not `NoMatch` or `Unavailable`.
2. Direct support is at least
   `ceil(min_support * graph_min_direct_support_ratio)`.
3. At least `graph_required_independent_paths` distinct intermediate nodes provide
   two-edge paths whose component edges are both `Match`.
4. Every composed path agrees with the direct edge at the start, midpoint, and end of
   common coverage within `graph_offset_tol_ms`.
5. Composed drift agrees within `graph_drift_tol_ppm`.
6. The independent composed paths also agree with one another under the same limits.

Demote a direct `Match` to `Ambiguous` only when at least the required number of
independent composed `Match` paths agree with one another, conflict with the direct edge,
and each path's bottleneck score is at least the direct edge's score.

Graph consistency MUST NOT promote an edge with zero direct landmark support. It may
also identify simultaneous alternate recorders when their mutually composed maps close.

### 13.4 Resolver migration

Expose graph-adjusted `PairEvidence` to CLI and Swift. Keep coverage clustering and user
override in the shell.

Replace the existing audio-audio triangle's use of raw `z` with affine evidence and
composed-map residuals.

Tests:

1. three simultaneous recorders close;
2. one wrong-take edge fails the cycle;
3. continuous recorder used by several videos remains valid;
4. disjoint recorder restart is not mistaken for a cycle conflict;
5. drift composition remains correct over long durations.

---

## 14. Phase 8 - FFI, CLI, and Swift integration

### 14.1 Additive FFI

Add:

```c
char *cpl_match_pair(
    uint64_t ctx, uint64_t track_a, uint64_t track_b,
    const char *pairing_params_json);

char *cpl_match_candidates(
    uint64_t ctx,
    const uint64_t *videos, size_t n_videos,
    const uint64_t *audios, size_t n_audios,
    const char *pairing_params_json);
```

Return `PairEvidence` JSON.

Do not remove `cpl_pair_correlate` or `cpl_pair_candidates`.

### 14.2 CoupleKit

Add wire enums and `PairEvidence`.

Add:

```swift
public func matchPair(...) -> PairEvidence?
public func matchCandidates(...) -> [[PairEvidence]]?
```

The app resolver uses:

- `classification`;
- refined lag/drift when present;
- `score` for deterministic ordering;
- `scoreMargin` and quality metrics only for display/debugging.

It must not duplicate the fingerprint acceptance formula.

### 14.3 CLI

Extend `couple pair`:

```text
--matcher legacy|shadow|fingerprint
--matcher-debug
```

JSON output in shadow mode includes both legacy and fingerprint matrices, but resolved
rows remain legacy until primary mode is selected.

Add a concise table showing:

```text
method, class, lag, ppm, support, inlier%, coverage%, p95, score, margin
```

### 14.4 UI

Do not expose algorithm jargon in normal queue rows.

User-facing states remain:

- confident;
- partial;
- split;
- 2 sources;
- ambiguous;
- no match.

Detailed evidence may appear in the existing diagnostic disclosure.

---

## 15. Phase 9 - Shadow-mode calibration

### 15.1 Required sequence

1. Run legacy and fingerprint matchers on calibration.
2. Tune only on calibration.
3. Freeze thresholds.
4. Run validation and fix algorithmic defects, not thresholds opportunistically.
5. Run holdout once for the release decision.

If thresholds change after seeing holdout, create a new holdout set before claiming an
unbiased result.

### 15.2 Minimum release gates

On validation and holdout:

- zero confident false matches;
- false-negative rate materially below legacy for 8-20 second true clips;
- all same-piece/different-take cases are correct or ambiguous, never confidently wrong;
- all repeated-content multi-hypothesis cases are correct or ambiguous;
- accepted mappings meet Phase 5 accuracy;
- metadata corruption cannot change a strong correct audio result;
- deterministic output across repeated runs;
- candidate matrix runtime remains within the product budget.

Suggested quantitative goals, subject to corpus size:

```text
confident false-match rate       0%
true-pair recall >=20 s          >= 99%
true-pair recall 8-20 s          >= 95%
same-piece wrong acceptance      0%
accepted offset p95 >=20 s       <= 1 ms
accepted offset p95 8-20 s       <= 3 ms
```

Ambiguity is not counted as a false match, but track its rate separately.

### 15.3 Performance budgets

Measure:

- fingerprint generation time per hour of audio;
- artifact bytes per hour;
- index build time;
- candidate lookup time;
- affine fitting time;
- full matrix time;
- peak memory.

Starting budgets:

```text
fingerprint generation       <= 10 s per hour of audio on target Mac
artifact size                <= 10 MB per hour
6x6 warm-cache matrix        <= 5 s
20x20 warm-cache matrix      <= 15 s
peak matching memory         <= 512 MB
```

If a budget is missed, optimize indexing and correspondence caps before weakening
identity thresholds.

---

## 16. Phase 10 - Default switch and fallback policy

### 16.1 Release switch

Only after holdout gates pass:

```text
MatcherMode::FingerprintPrimary
```

becomes the app default.

Keep a developer override to legacy for at least one release cycle.

### 16.2 Fallback

Fallback to `EnvelopeV1` only for:

- artifact build/read failure;
- unsupported internal fingerprint version;
- explicit developer override.

Do not fall back merely because fingerprints report `NoMatch`; doing so would allow the
weaker matcher to override a deliberate fingerprint rejection.

When fallback occurs, method and reason must be visible in debug output.

### 16.3 Cleanup after one release

After telemetry/corpus review:

- remove shadow computation from normal operation;
- retain legacy matcher APIs and diagnostics;
- retain cache rebuild compatibility;
- update `SPEC-V2.md` through the repository's SPEC-CHANGE process;
- append implementation results and calibrated values to `PROGRESS.md`.

---

## 17. Test matrix

Every implementation phase must keep all existing workspace tests green.

New fingerprint tests must cover:

| Family | Required result |
|---|---|
| Exact same PCM excerpt | Match, correct affine map |
| Gain/AGC difference | Match |
| EQ/band-limit difference | Match or honest ambiguity at extreme degradation |
| Reverberation | Match within corpus threshold |
| SNR degradation | Calibrated recall, never wrong confident |
| 8/12/20 s excerpts | Reach matcher; expected recall |
| Different take, same score | NoMatch or Ambiguous |
| Repeated phrase | Unique truth or Ambiguous |
| Sustained tone | No confident match from periodicity alone |
| Silence | NoMatch |
| Dropout | Match if remaining temporal coverage is sufficient |
| +/- drift bounds | Correct map inside bound; reject/ambiguous outside |
| Recorder restart | Correct partial/disjoint behavior |
| Multiple simultaneous recorders | Multiple real matches |
| Bad metadata | Audio result unchanged |
| Corrupt fingerprint | Rebuild, no crash |
| Candidate overflow | Ambiguous, never silent omission |

---

## 18. Implementation order and commit boundaries

Use these reviewable boundaries:

1. Fingerprint params, downsampler, STFT, peak tests.
2. Landmark generation and binary artifact.
3. Cache integration and lazy upgrade.
4. Batch index and correspondence generation.
5. Robust affine consensus and tests.
6. `PairEvidence` core API.
7. Existing fine-refinement integration.
8. FFI/CoupleKit/CLI shadow mode.
9. Swift resolver shadow telemetry.
10. Metadata hypotheses.
11. Graph consistency.
12. Corpus calibration and threshold freeze.
13. Fingerprint-primary switch.

Do not combine cache format, affine math, FFI migration, and default switching into one
change. Each boundary must pass the relevant tests before proceeding.

---

## 19. Definition of done

The implementation is complete only when:

1. Every imported track can provide a versioned cached fingerprint.
2. Existing caches lazily gain fingerprints without re-decoding source media.
3. Batch lookup uses one audio index rather than V x A independent scans.
4. Pair identity comes from robust spectral-landmark consensus.
5. Offset and drift are refined with the existing local alignment machinery.
6. Runner-up uniqueness and temporal coverage prevent periodic coincidences from being
   confidently accepted.
7. Metadata remains advisory and non-vetoing.
8. Graph consistency uses affine composition.
9. CLI and Swift consume core `PairEvidence` without reproducing acceptance thresholds.
10. Holdout has zero confident false matches and meets agreed recall/accuracy budgets.
11. The app defaults to fingerprints only after those gates pass.
12. Existing editor, audition, session, export, ABI compatibility, determinism, and
    stability gates remain green.

---

## 20. Prohibited shortcuts

An implementer must not:

- call envelope correlation "spectral verification";
- classify from raw shared-hash count without affine consensus;
- accept a two-correspondence zero-residual fit;
- tune against holdout;
- use timestamps as a hard search window;
- silently cap candidates;
- serialize NaN/Infinity into JSON;
- replace the existing final alignment path without evidence;
- use semantic audio embeddings to distinguish recordings;
- delete old cache entries merely because a fingerprint is missing;
- switch the default before shadow and holdout gates pass.
