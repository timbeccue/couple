# PLAN - Audio matching prework and architecture hardening

Status: implementation plan. This plan prepares the existing envelope matcher and its
cross-language plumbing for the spectral-landmark matcher described in
`PLAN-SPECTRAL-FINGERPRINT-MATCHING.md`. Complete this plan first.

Repository root:
`/Users/tim/code/claude_apps/couple`

Written 2026-06-14 against the working tree that contains the uncommitted
`PLAN-MATCH-CLASSIFIER.md` and `PLAN-RMS-TIEBREAKER.md` implementation. Those changes
are the baseline. Do not discard, rewrite, or "clean up" unrelated user changes.

---

## 1. Objective

Make the pairing subsystem safe to extend without changing the proven editor-sync,
audition, or export behavior.

This plan has five concrete outcomes:

1. Core owns every matcher and verifier threshold. CLI and Swift stop maintaining
   independent copies.
2. The advertised 8-second verification floor is reachable. The 200 ms envelope search
   no longer rejects 8-20 second tracks as structurally unscannable.
3. Verification windows are unique, edge-safe, and fitted robustly.
4. Candidate limiting never silently removes the correct answer.
5. A deterministic calibration/validation corpus and parity gates exist before a new
   matcher is introduced.

This is preparatory work. It MUST NOT add spectral fingerprints, change export mappings,
change editor auto-sync thresholds, or make the experimental verifier the default.

---

## 2. Existing defects this plan must remove

### 2.1 Unreachable short-overlap path

`core/src/pair.rs` rejects either track when it contains fewer than
`SyncParams.min_overlap_bins` at 9600 samples/bin. The current default is 100 bins,
which is 20 seconds. `verify_min_overlap_s` is 8 seconds, but `verify_pair` is never
reached for an 8-20 second pair.

Required result: `insufficient_overlap` means "less than the pairing hard floor," not
"less than the legacy z calibration length."

### 2.2 Parameter ownership is split and inconsistent

The same verifier values are currently copied into:

- `core/src/sync.rs`
- `cli/src/pair.rs`
- `mac/Sources/Couple/Resolver.swift`

At least one copy has already drifted: CLI uses `verify_density_per_s = 0.20`, while
core and Swift use `0.5`.

The FFI pairing calls use `Default::default()` internally, so Swift-side fields do not
actually configure core.

Required result: the Rust core is the only source of defaults and match predicates.

### 2.3 Verifier windows can duplicate evidence

`core/src/verify.rs` creates evenly spaced nominal centers and clamps each center
independently. Several centers can collapse to the same edge location. `locked` and
`expected` then count repeated measurements as independent evidence.

Required result: center placement computes the feasible interval first and emits unique
centers inside it.

### 2.4 The verifier fit is sensitive to bad windows

The verifier currently feeds every locked point into a ridge least-squares line. One or
two false local locks can bias both offset and drift.

Required result: deterministic residual-based outlier rejection wraps the existing
ridge fit. Do not introduce randomized behavior.

### 2.5 Top-K truncation is a hidden false-negative path

Both resolvers cap candidates before fine verification. If the real candidate is below
the cap, later stages cannot recover it.

Required result: a resource limit may stop work explicitly, but it MUST NOT silently
convert omitted candidates into "no match."

---

## 3. Non-negotiable invariants

The implementer MUST preserve all of these:

1. The canonical timeline and lag sign remain unchanged:
   content at time `t` in A occurs at `t + lag_ms / 1000` in B.
2. `auto_sync_tracks`, editor drift fitting, session state, audition, and export continue
   using `SyncParams` and retain existing defaults.
3. Pairing remains deterministic: identical inputs and params produce bit-identical
   serialized measurements on the same machine.
4. Old C ABI symbols remain available and retain default behavior. New parameterized
   symbols are additive.
5. Existing cache entries remain readable during this plan.
6. The feature flag controlling the experimental verifier remains OFF by default.
7. Resolver uncertainty remains honest: no candidate omission or fit failure may cause
   a confident selection.
8. CLI and Swift must use the same core match predicate. There must be no Swift rewrite
   of `is_match`.
9. Do not add timestamps to candidate ordering. Metadata remains non-vetoing.
10. Do not change `PairCorrelation.z` semantics during this plan.

---

## 4. Target module layout after prework

Add:

```text
core/src/pairing_params.rs   PairingParams, defaults, validation, CLI-style overrides
core/src/robust_fit.rs       deterministic verifier outlier rejection
core/tests/pairing_prework.rs
fixtures/matcher-corpus/     generated-case manifests and ground-truth schema
```

Keep these responsibilities:

```text
core/src/sync.rs             editor/full-sync configuration and alignment
core/src/pair.rs             legacy envelope candidate localization
core/src/verify.rs           per-pair fine verification
core/src/fit.rs              primitive least-squares/ridge math
core/src/ctx.rs              batched track-handle operations
cli/src/pair.rs              row resolution and CLI serialization
mac/.../Resolver.swift       row resolution and user-overridable shell behavior
```

Do not merge the resolver into core in this plan. The fingerprint implementation will
introduce a method-neutral evidence contract without forcing a resolver rewrite first.

---

## 5. Phase 0 - Freeze and characterize the baseline

### 5.1 Record the dirty baseline

Before editing:

```bash
git status --short
git diff --stat
```

Do not reset or stash merely to obtain a clean tree. The current uncommitted classifier
work is expected.

### 5.2 Run the baseline gates

Run:

```bash
cargo test -p couple-core --lib --release
cargo test -p couple-core --tests --release
cargo test -p couple-cli --test pair_gates --release
cd mac && swift build
```

Record failures that existed before implementation. Do not weaken a gate to make this
plan pass.

### 5.3 Add a behavior snapshot test

In `core/tests/pairing_prework.rs`, generate a deterministic 30-second true pair and an
unrelated pair using the existing fixture helpers. Serialize `PairCorrelation` for each
twice and assert byte equality.

This test protects determinism while later phases move parameters and window placement.

Exit gate:

- Baseline commands are recorded.
- Determinism snapshot passes.
- No production behavior has changed.

---

## 6. Phase 1 - Give pairing its own core-owned parameters

### 6.1 Create `PairingParams`

Create `core/src/pairing_params.rs`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PairingParams {
    // Legacy envelope proposal/localization.
    pub min_overlap_s: f64,
    pub legacy_z_min: f64,
    pub refine_ncc_min: f64,
    pub stage_spans_ms: [f64; 4],
    pub parabolic_clamp: f64,

    // Fine verifier.
    pub verify_win_frac: f64,
    pub verify_half_win_min_s: f64,
    pub verify_half_win_max_s: f64,
    pub verify_density_per_s: f64,
    pub verify_min_windows_floor: usize,
    pub verify_max_windows: usize,
    pub verify_base_span_ms: f64,
    pub verify_span_scale_ppm: f64,
    pub verify_span_cap_ms: f64,
    pub verify_ncc_window_min: f64,
    pub verify_min_locked: u32,
    pub verify_lock_fraction_min: f64,
    pub verify_rms_max_ms: f64,
    pub verify_outlier_floor_ms: f64,
    pub drift_lambda_s2: f64,
    pub drift_max_ppm: f64,

    // Work limits. Limits control resource use, never correctness silently.
    pub max_verify_pairs_per_video: usize,
    pub max_verify_pairs_total: usize,
}
```

Default values:

```text
min_overlap_s                   8.0
legacy_z_min                    8.5
refine_ncc_min                  0.15
stage_spans_ms                  [500.0, 80.0, 14.0, 3.0]
parabolic_clamp                 0.6
verify_win_frac                 0.5
verify_half_win_min_s           2.5
verify_half_win_max_s           9.0
verify_density_per_s            0.5
verify_min_windows_floor        3
verify_max_windows              24
verify_base_span_ms             120.0
verify_span_scale_ppm           150.0
verify_span_cap_ms              500.0
verify_ncc_window_min           0.25
verify_min_locked               4
verify_lock_fraction_min        0.66
verify_rms_max_ms               12.0
verify_outlier_floor_ms         2.0
drift_lambda_s2                 50.0
drift_max_ppm                   600.0
max_verify_pairs_per_video      32
max_verify_pairs_total          4096
```

The new limits are safety ceilings, not a top-K ranking policy.

### 6.2 Validate parameters once

Add:

```rust
impl PairingParams {
    pub fn validate(&self) -> Result<()>;
    pub fn set(&mut self, key: &str, value: &str) -> Result<()>;
}
```

`validate` MUST reject:

- non-finite numeric values;
- `min_overlap_s <= 0`;
- fractions outside `[0, 1]`;
- zero window counts or limits;
- `verify_max_windows < verify_min_windows_floor`;
- `verify_half_win_max_s < verify_half_win_min_s`;
- negative spans, RMS limits, ridge weights, or drift bounds;
- a refinement span array that is not strictly decreasing and positive.

Every FFI and CLI entry validates before doing work.

### 6.3 Remove classifier fields from `SyncParams`

Move all fields beginning with `verify_`, plus classifier-only
`drift_lambda_s2` and `drift_max_ppm`, out of `SyncParams`.

Do not remove editor-sync fields such as `ncc_window_min`, `drift_windows`,
`half_win_s`, `drift_span_cap_ms`, or `z_min`.

After this change:

- `auto_sync_tracks` and `offset_only_sync` still accept `&SyncParams`;
- `pair_correlate`, `verify_pair`, `pair_fit_rms`, `pair_candidates`,
  `pair_fit_batch`, and `verify_batch` accept `&PairingParams`.

Within `verify_pair`, replace the former reads of `half_win_s`,
`drift_base_span_ms`, and `drift_span_scale_ppm` with
`verify_half_win_max_s`, `verify_base_span_ms`, and `verify_span_scale_ppm`.

### 6.4 Core owns the predicate

Extend `VerifyFit`:

```rust
pub struct VerifyFit {
    // existing fields
    pub inliers: u32,
    pub residual_p95_ms: f64,
    pub temporal_coverage: f64,
    pub matched: bool,
}
```

`matched` is computed by core after the robust fit:

```rust
matched =
    inliers >= params.verify_min_locked
    && lock_fraction >= params.verify_lock_fraction_min
    && rms_ms <= params.verify_rms_max_ms
    && !clamped;
```

Keep `verify::is_match` as a small helper returning `fit.matched`. Do not duplicate the
predicate in CLI or Swift.

### 6.5 CLI changes

In `cli/src/pair.rs`:

- Keep only resolver-owned fields in `PairParams`: feature flag, z margin, triangle
  tolerance, coverage threshold, disjoint tolerance, and RMS escalation policy.
- Store a `PairingParams` value or construct one from core defaults.
- Delete copied verifier defaults.
- `--params` matching keys delegate to `PairingParams::set`.
- Use `fit.matched`; do not call a locally mirrored predicate.

The current default-off behavior remains:

```text
verify_active == false -> legacy z gate
verify_active == true  -> core prefilter + core VerifyFit.matched
```

### 6.6 Swift changes

In `mac/Sources/Couple/Resolver.swift`:

- Remove copied verifier numeric thresholds.
- Keep `verifyClassifierActive` and resolver-owned constants.
- Replace the Swift `isMatch` implementation with `fit.matched`.

In `mac/Sources/CoupleKit/Wire.swift`, add the new `VerifyFit` fields.

### 6.7 Add parameterized FFI symbols

Keep all old symbols. Add:

```c
char *cpl_pair_correlate_params(
    uint64_t ctx, uint64_t track_a, uint64_t track_b,
    const char *params_json);

char *cpl_pair_candidates_params(
    uint64_t ctx,
    const uint64_t *videos, size_t n_videos,
    const uint64_t *audios, size_t n_audios,
    const char *params_json);

char *cpl_pair_fit_batch_params(
    uint64_t ctx, const char *pairs_json, const char *params_json);

char *cpl_verify_batch_params(
    uint64_t ctx, const char *pairs_json, const char *params_json);

char *cpl_pairing_params_default_json(void);
```

Rules:

- null or empty `params_json` means `PairingParams::default()`;
- malformed or invalid JSON is an `InvalidArgument`;
- old symbols call the same internal implementation with defaults;
- no ABI version bump is required because this is additive.

Update `CoupleKit.Engine` to use parameterized symbols only when an override is supplied.
Normal app operation passes null and therefore uses core defaults.

### 6.8 Tests

Add:

1. Rust round-trip test: default params serialize and deserialize identically.
2. Invalid-param table test.
3. Old FFI symbol output equals new symbol with null params.
4. New symbol with an override visibly changes the requested parameterized behavior.
5. Swift decodes core default JSON and `VerifyFit.matched`.
6. A source search gate fails if verifier default literals reappear in
   `Resolver.swift` or `cli/src/pair.rs`.

Exit gate:

- Core is the sole owner of verifier defaults and predicates.
- Old ABI calls still work.
- CLI and app use identical defaults.

---

## 7. Phase 2 - Make short overlaps mechanically reachable

### 7.1 Convert seconds to stage-1 bins

In `core/src/pair.rs`:

```rust
let min_overlap_bins =
    ((params.min_overlap_s * SAMPLE_RATE as f64) / 9600.0).ceil() as u64;
let min_overlap_bins = min_overlap_bins.max(2);
```

Use this value for both duration checks and `stage1_scan`.

With the default, 8 seconds becomes 40 bins.

### 7.2 Separate localization from legacy acceptance

The stage refinement ladder currently runs only when `z >= z_min`. Change it to run
when:

```text
not insufficient_overlap AND best_ncc >= refine_ncc_min
```

`PairCorrelation.z` and the default legacy resolver gate remain unchanged.

Add these fields to `PairCorrelation`:

```rust
pub legacy_candidate: bool,   // z >= legacy_z_min
pub prefilter_candidate: bool // ncc >= refine_ncc_min
```

Both are core-computed. Shells must consume these booleans rather than repeat thresholds.

For a cell with sufficient duration but no plausible NCC:

- `insufficient_overlap = false`;
- `prefilter_candidate = false`;
- `legacy_candidate` reflects z and will normally be false;
- refinement may remain skipped.

### 7.3 Preserve full-sync semantics

Do not alter `SyncParams.min_overlap_bins` or editor/full-sync behavior in this phase.
Only role-agnostic pairing uses the new second-based floor.

### 7.4 Tests

Add generated true pairs with 7.9, 8.0, 12.0, 19.9, and 20.0 seconds of overlap.

Assert:

- 7.9 seconds is insufficient;
- 8.0 seconds and above are scannable;
- a genuine 8-20 second pair obtains a refined lag when NCC clears the prefilter;
- legacy candidacy remains governed by z and therefore does not silently change the
  default-off resolver;
- lag error remains within the existing offset tolerance.

Exit gate:

- The verifier can receive 8-20 second candidates when its feature flag is enabled.
- Default legacy row decisions remain unchanged on existing gates.

---

## 8. Phase 3 - Make verifier evidence independent and robust

### 8.1 Replace per-center clamping

In `core/src/verify.rs`, extract:

```rust
fn verification_centers(
    dur_a: f64,
    dur_b: f64,
    off_s: f64,
    overlap_t0: f64,
    overlap_t1: f64,
    half_win_s: f64,
    max_span_ms: f64,
    desired: usize,
) -> Vec<f64>;
```

Algorithm:

1. Compute one global feasible center interval using the maximum scan margin:

   ```text
   margin_s = max_span_ms / 1000
   lo = max(overlap_t0, half_win_s, half_win_s - off_s + margin_s)
   hi = min(overlap_t1, dur_a - half_win_s,
            dur_b - off_s - half_win_s - margin_s)
   ```

2. If `hi < lo`, return an empty vector.
3. If `desired == 1` or `hi - lo < 1e-9`, return one midpoint.
4. Otherwise emit exactly `desired` linearly spaced points including `lo` and `hi`.
5. Deduplicate adjacent values whose difference is below one 96-spb bin.
6. Never clamp individual nominal centers.

The verifier reports an unmatched fit when too few unique centers remain. It must not
manufacture evidence.

### 8.2 Add deterministic robust fitting

Create `core/src/robust_fit.rs`.

```rust
pub struct RobustFit {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
    pub residual_p95_ms: f64,
    pub clamped: bool,
    pub inlier_indices: Vec<usize>,
}

pub fn fit_line_mad(
    points: &[(f64, f64)],
    lambda_s2: f64,
    max_ppm: f64,
    absolute_floor_ms: f64,
) -> Option<RobustFit>;
```

Use this exact deterministic procedure:

1. Fit all points with `fit_ls_reg`.
2. Compute absolute residuals.
3. Compute residual median `m`.
4. Compute `MAD = median(abs(residual - m))`.
5. Set:

   ```text
   sigma = 1.4826 * MAD
   threshold = max(absolute_floor_ms, 3.0 * sigma)
   ```

6. Keep points with absolute residual `<= threshold`.
7. Refit retained points.
8. Repeat steps 2-7 once.
9. Require at least two inliers with non-degenerate time spread.
10. Compute RMS and nearest-rank p95 from final inliers.

Use `absolute_floor_ms = 2.0` initially. Mark it `[TUNE]` in `PairingParams` as
`verify_outlier_floor_ms`; do not hide the value inside the fitter.

No RNG, hash iteration, or wall clock may affect the result.

### 8.3 Add temporal coverage

For final inlier centers:

```text
temporal_coverage =
    (max_inlier_t - min_inlier_t) / max(overlap_s, epsilon)
```

This is telemetry in prework. Do not add it to the match predicate until corpus evidence
supports a threshold.

### 8.4 Correct lock accounting

Definitions:

- `expected`: unique placed centers where both tracks are measurable;
- `locked`: expected centers whose local result clears the NCC floor;
- `inliers`: locked centers retained by robust fitting;
- `lock_fraction`: `locked / expected`, preserving the existing meaning;
- `matched`: uses `inliers` for the minimum-count condition, not `locked`.

`PairFit.windows` becomes the robust inlier count. Document the changed meaning.

### 8.5 Tests

Add tests for:

1. No duplicate centers on short contained clips.
2. Reversing the input point order produces identical robust fit fields.
3. One large outlier does not move a clean line beyond 0.5 ms or 1 ppm.
4. A majority of scattered points does not incorrectly produce `matched = true`.
5. All-equal center times fail rather than return zero RMS.
6. p95 calculation for 2, 3, and 20 points.
7. Existing clean-pair verifier values remain within documented tolerances.

Exit gate:

- Duplicate evidence is impossible.
- Verifier fit is deterministic and resistant to isolated false locks.

---

## 9. Phase 4 - Remove silent candidate eviction

### 9.1 Batch all eligible candidates

When the verifier feature is active:

- collect every `prefilter_candidate`;
- verify in chunks to bound temporary allocation;
- do not sort and truncate before verification.

Use `max_verify_pairs_per_video` and `max_verify_pairs_total` only as explicit resource
guards.

### 9.2 Explicit overflow behavior

If a limit is exceeded:

- verify no arbitrary subset;
- return or construct an explicit `candidateOverflow` condition;
- resolve the affected row as `ambiguous`, never `noMatch` or `confident`;
- include the eligible count and configured limit in debug/CLI detail;
- keep all candidate names visible to the user.

Add an optional `candidate_overflow` field to CLI and Swift `ResolutionDetail`. Do not
persist it.

### 9.3 Tests

Create a generated row with at least eight plausible candidates and put the true pair
after position six.

Assert:

- normal limits verify the true pair;
- a deliberately low limit yields explicit ambiguity;
- changing filenames does not decide which candidates are silently retained, because
  none are silently retained.

Exit gate:

- Candidate limits can degrade to honest ambiguity but cannot create a false negative.

---

## 10. Phase 5 - Establish the matcher corpus

### 10.1 Corpus structure

Add:

```text
fixtures/matcher-corpus/
  README.md
  schema.json
  synthetic/
    cases.json
  calibration/
    manifest.json
  validation/
    manifest.json
  holdout/
    manifest.json
```

Media may remain gitignored. Manifests and ground truth must be committed.

Each case records:

```json
{
  "id": "short_true_agc_12s",
  "group": "synthetic",
  "a": "relative/or/generated/source",
  "b": "relative/or/generated/source",
  "sameEvent": true,
  "expectedOffsetMs": 1250.0,
  "expectedDriftPpm": 80.0,
  "minimumOverlapS": 12.0,
  "tags": ["short", "agc"],
  "allowedForTuning": true
}
```

Holdout cases MUST have `allowedForTuning: false`.

### 10.2 Required synthetic families

Generate deterministic cases for:

1. Same event, clean.
2. Same event with AGC/limiting.
3. Same event with telephone-band filtering.
4. Same event with reverberation.
5. Same event at SNR 0, 5, and 10 dB.
6. Same event at 8, 12, 20, 60, and 600 seconds.
7. Same event at drift -600, -100, 0, +100, and +600 ppm.
8. Same macro-envelope but independently generated fine content.
9. Repeated musical phrase with several plausible offsets.
10. Sustained periodic tone.
11. Mostly silence with one transient region.
12. Dropout in one recorder.
13. Recorder restart/disjoint coverage.
14. Multiple simultaneous recorders.
15. More than six plausible candidates.

### 10.3 Real-data split

Place existing difficult real examples into calibration or validation, never both.
Reserve at least 25% of available independent recording events for holdout.

Different clips from the same recording event belong to the same split. Otherwise the
holdout leaks event-specific acoustics.

### 10.4 Metrics command

Add a CLI command or test utility that reports:

```text
true positives
false positives
false negatives
ambiguous true pairs
median/p95 offset error
median/p95 drift error
per-tag breakdown
runtime and peak candidate count
```

It must emit JSON suitable for comparing implementations.

### 10.5 Prework acceptance thresholds

The legacy matcher is not expected to pass future fingerprint goals. Prework gates are:

- no regression on existing true-pair fixtures;
- no new confident false match;
- all 8-20 second pairs reach the verifier when enabled;
- overflow becomes ambiguity;
- CLI and Swift use identical core measurements and match booleans.

Do not tune thresholds on holdout.

---

## 11. Phase 6 - Cross-stack parity and cleanup

### 11.1 Delete obsolete duplication

After all callers use core-owned parameters:

- delete copied verifier constants from CLI and Swift;
- delete the Swift predicate;
- delete comments claiming Swift parameters are forwarded when they are not;
- update `PLAN-MATCH-CLASSIFIER.md` status notes or `PROGRESS.md` with the completed
  migration. Do not rewrite historical records.

### 11.2 Add parity fixtures

Commit one small candidate-matrix JSON fixture produced by core. Test that:

- Rust CLI decodes and resolves it;
- CoupleKit decodes every field;
- Swift's feature-on filtering uses `legacyCandidate`, `prefilterCandidate`, and
  `VerifyFit.matched`, not local numeric gates.

### 11.3 Final commands

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
swift test --package-path mac
swift build --package-path mac
```

Also run the matcher-corpus metrics command twice and compare its JSON byte-for-byte.

---

## 12. Definition of done

This prework is complete only when:

1. `PairingParams` is core-owned and validated.
2. FFI can accept pairing params, while old symbols remain compatible.
3. CLI and Swift contain no duplicate match thresholds or predicates.
4. An 8-second pair is scannable and can reach fine verification.
5. Verification centers are unique.
6. Robust fit telemetry and `matched` are core-computed.
7. No candidate is silently dropped because of a top-K cap.
8. Corpus manifests and metrics exist.
9. Existing sync, editor, audition, export, stability, and performance gates pass.
10. The spectral implementation plan can rely on one parameter contract and one
    match-classification authority.

---

## 13. Explicitly deferred to the fingerprint plan

Do not implement these during prework:

- STFT or spectral peaks;
- fingerprint cache files;
- landmark hash indexes;
- affine RANSAC over landmark correspondences;
- BWF/timecode priors in decisions;
- a method-neutral `PairEvidence` API;
- graph-wide cycle consistency;
- making the new matcher the default.
