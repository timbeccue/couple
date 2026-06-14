# PLAN — Match classification by fine waveform-lock (coarse-find / fine-verify)

Status: design plan. Two phases. Phase 1 instruments the new fine-verify signal with
ZERO decision change. Phase 2 flips the decision (z demoted to pre-filter; fine-verify
becomes the classifier) behind tunable params with gate re-baselining. Written
2026-06-13.

All paths absolute. Every code claim verified against the working tree (which already
contains the **uncommitted** PLAN-RMS-TIEBREAKER work — that is the baseline this plan
builds on and partly subsumes). Builds on, and supersedes where noted,
`/Users/tim/code/claude_apps/couple/PLAN-RMS-TIEBREAKER.md`.

---

## 1. Goals and the converged design

### 1.1 Product goals (owner)

1. Pairing is **classification**, not "pick the closest." For each video, decide which
   audio files are REAL matches (same event) vs not. An objectively correct answer
   exists; multiple real matches may co-exist (duplicate recorders).
2. Hard requirement: separate a REAL match from a non-match that merely **resembles** —
   a different take of the same piece shares the loudness envelope; a short clip overlaps
   a sliver. Envelope similarity must not be mistaken for identity.
3. Performant. Current speed is fine; a little slower is tolerable; there is a hard budget
   gate (gate7, ≤ 5 s for the 6×6 matrix).

### 1.2 The defect (verified, not re-derived)

Candidacy today is a single coarse score **`z = NCC·√n`** — Pearson NCC of the 200 ms
amplitude **envelope** times √(overlap bins). Formed at
`/Users/tim/code/claude_apps/couple/core/src/sync.rs:288`
(`let z = ncc * (n as f64).sqrt();`), gated `z >= z_min` with `z_min = 8.5`
(`core/src/sync.rs:48`) and `min_overlap_bins = 100` (≈ 20 s @ 9600 spb,
`core/src/sync.rs:43`). The gate fires three places, all reading the same `z`:

- core offset search: `core/src/sync.rs:435` (`if s1.best_z < params.z_min`);
- `pair_correlate`: `core/src/pair.rs:82` (refinement skipped below floor);
- candidacy in both resolvers: `mac/Sources/Couple/Resolver.swift:173`
  (`!m.insufficientOverlap && m.z >= params.zMin`) and the CLI mirror
  `cli/src/pair.rs:311` (`!m.insufficient_overlap && m.z >= params.z_min`).

`z` **conflates resemblance with quantity**. The comment at `core/src/sync.rs:44-47`
documents that a 20 s clip cannot exceed `z = √100 = 10`, so `z_min = 8.5` was tuned to a
near-perfect synthetic short clip. Consequences, both confirmed:

- **The √n term structurally rejects short genuine clips.** A real 20 s clip with a
  realistic envelope NCC of 0.6–0.8 gives `z ≈ 6–8 < 8.5` → rejected before becoming a
  candidate. The *same audio* at 4 min (√1200 ≈ 35) sails through. The owner confirmed a
  known-good 20 s clip won't match even when manually placed.
- **The 18 s analysis window can't lock a 20 s clip.** Even past the gate, the fine pass
  uses `drift_windows = 12` fixed-count windows of `half_win_s = 9.0` (an ~18 s analysis
  window) — `drift_pass` `core/src/sync.rs:474-584`, the just-built `pair_fit_rms`
  `core/src/sync.rs:607-663`. On a 20 s clip only ~1 window center fits and it hugs the
  edges, so `measure::measure_local`'s degenerate-edge rejection
  (`core/src/measure.rs:108-118`) refuses it → < 3 points → `None`. This is *also* why
  manual placement of a short clip cannot lock.
- **The same blindness produces the opposite failure.** Because envelope `z` can't judge
  short overlaps, short *spurious* clips (a sliver that happens to resemble) get
  suggested. Reject-the-real and accept-the-fake are one root cause: **coarse envelope
  correlation cannot classify short overlaps.**

What CAN separate real from resembling is the **fine waveform lock**: does the actual
audio align *consistently* across the whole overlap. The RMS-tiebreaker work already
proved this for one narrow case (same-piece different-take ambiguity) via per-candidate
drift-fit RMS — but only as an escalation *after* the z-gate already admitted both
candidates. This plan generalizes that signal into the **primary classifier**.

### 1.3 Target design: coarse-find / fine-verify

> **`z`/NCC PROPOSES (find the lag + a cheap pre-filter); the fine waveform-lock DISPOSES
> (the classifier).**

**Stage A — coarse localize & loose pre-filter** (≈ today's `z`, cheap). The envelope
stage-1 scan finds the candidate lag. A *loose* resemblance pre-filter skips only pairs
with no plausible alignment. `min_overlap_bins` is lowered / auto-scaled so short clips
pass through. `z` loses its power to **reject** genuine short clips; its only jobs are
*where to look* and *skip obvious garbage*.

**Stage B — fine verify** (THE decision):

1. **Adaptive-width windows at a fixed density per second of overlap** (not fixed count).
   Width scales to fit (narrow on short clips), total capped on long clips. A 20 s overlap
   yields ~N independent points; 4 min yields ~12N. Fixes manual-placement-can't-lock and
   short-clip editor sync.
2. **Slope-bounded / drift-regularized fit.** Clock-rate drift is physically tiny and
   bounded — crystal ~tens-to-100 ppm; worst legit cases (NTSC pulldown, mislabeled sample
   rate) ~1000 ppm; NEVER absurd (150 % = 500 000 ppm). Regularize the line's slope toward
   0 within a generous tunable bound. This (a) makes few-point fits robust — a free 2-param
   line overfits sparse points (2 pts → 0 RMS; 4 scattered pts tilt to fake-low RMS),
   regularization prevents that and makes short-clip RMS *trustworthy*; (b) yields a cheap
   discriminator — out-of-bound slope ⇒ not a match; (c) lets per-window search spans be
   tighter (lag can't wander far ⇒ faster, fewer spurious locks). One fitter serves both
   short (prior dominates → flat) and long (data refines a small slope) — no code fork.
3. **Decision signal = lock-fraction + slope-bounded RMS + drift-in-bound.**
   - `lock_fraction = locked_windows / expected_windows(overlap)` — the owner's key
     insight: "20 dense low-RMS points (short clip, locked everything) beat 20 sparse
     low-RMS points (long clip, mostly didn't agree)." `expected` counts only MEASURABLE
     (non-silent) windows so quiet passages don't unfairly lower it.
   - Classify REAL MATCH iff `rms ≤ R` AND `lock_fraction ≥ F` AND `locked ≥ floor`
     (≈ 4) AND `|ppm| ≤ bound`. All tunable.

**Stage C — primary selection.** Among real matches, the existing §3.3 coverage / cluster
/ tie-break logic picks the primary; duplicates all legitimately pass (per-pair
classification, not winner-take-all). The §3.4 simultaneity and §3.5 margin logic stay.

---

## 2. The new CORE primitive(s)

### 2.1 Where it lives

A new module **`/Users/tim/code/claude_apps/couple/core/src/verify.rs`** (registered in
`core/src/lib.rs` next to `pub mod sync;`). Rationale for a new module over extending
`sync.rs`: the classifier is a distinct concern (per-pair *match decision*) from the
offset/drift *measurement* pipeline, it must be importable by `pair.rs`/CLI without
dragging the full-sync surface, and isolating it satisfies the overseer's "isolate the
classifier's fitter if a shared refactor is risky" guard. `sync.rs` keeps the
editor-sync path; `verify.rs` owns classification.

The slope-regularized fitter is added to **`core/src/fit.rs`** (alongside `fit_ls`) so
both `verify.rs` and, if desired later, `drift_pass` can call it. This **subsumes**
`pair_fit_rms`: `pair_fit_rms` (`core/src/sync.rs:607-663`) and its `Ctx::pair_fit_batch`
(`core/src/ctx.rs:259-275`) are reimplemented as thin shims over the new `verify` path (or
deleted once the resolver moves to the new entry — see §2.6), so there are not two
parallel fine-fit functions.

### 2.2 `fit.rs` — slope-bounded (ridge-on-slope) fit

`fit_ls` (`core/src/fit.rs:15-43`) fits a free 2-param line: `slope = sxy / sxx`
(`fit.rs:31`), `rms_ms` at `fit.rs:41`. Add a regularized variant that penalizes nonzero
slope toward 0 within a bound. This is ridge regression on the slope coefficient only
(intercept unpenalized), then a hard clamp to `±max_ppm`.

```rust
/// Slope-regularized line fit over (t_s, lag_ms): minimizes
///   Σ (lag − off − slope·t)²  +  λ · slope²        (intercept unpenalized)
/// then hard-clamps slope to ±max_ppm/1000 (ms per s). λ chosen so a short, sparse
/// overlap is pulled flat (prior dominates) while a long overlap's data still refines
/// a small slope. Returns rms_ms of residuals to the CLAMPED line (the honest lock
/// error under the physical drift bound). None on < 2 points or degenerate t-spread.
///
/// UNITS: `lambda_s2` is in SECONDS² — it is added directly to sxx = Σ(t−t̄)² (s²),
/// so it MUST share sxx's units. (Despite the penalty being written λ·slope² with
/// slope in ms/s, the closed-form centered solution is sxy/(sxx+λ), where λ lands in
/// the same units as sxx.) Name it `lambda_s2` (NOT `_ms2_per_s2`).
pub struct FitReg { pub off_ms: f64, pub ppm: f64, pub rms_ms: f64, pub clamped: bool }

pub fn fit_ls_reg(points: &[(f64, f64)], lambda_s2: f64, max_ppm: f64)
    -> Option<FitReg>;
```

Pseudocode (the regularization + clamp toward 0 within ±bound):

```
n = points.len();           if n < 2 { return None }
mean_t, mean_l = means
sxx = Σ (t − mean_t)²;       if sxx < 1e-12 { return None }   // degenerate spread
sxy = Σ (t − mean_t)(l − mean_l)
// Ridge on slope (centered design ⇒ intercept stays mean_l): slope = sxy/(sxx+λ).
// λ is in s² (comparable to sxx, also s²); for sparse short overlaps sxx is small so
// λ dominates ⇒ slope→0; for long overlaps sxx ≫ λ ⇒ slope≈OLS.
slope = sxy / (sxx + lambda)                 // lambda has units of s² (= sxx's units)
clamped = false
max_slope = max_ppm / 1000.0                 // ms per s
if slope >  max_slope { slope =  max_slope; clamped = true }
if slope < -max_slope { slope = -max_slope; clamped = true }
off = mean_l − slope·mean_t                  // intercept at t=0 on the (clamped) line
rms = sqrt( Σ (l − (off + slope·t))² / n )   // residuals to the final line
return FitReg{ off_ms: off, ppm: slope*1000, rms_ms: rms, clamped }
```

> IMPLEMENTER WARNING — the pseudocode `slope = sxy/(sxx+λ)` is CORRECT and `λ` is in
> **s²** (it is added to `sxx`, which is s²). Do NOT "fix" the name/units to `ms²/s²` to
> match the penalty's written form `λ·slope²`; the centered closed form puts λ in sxx's
> units. The earlier draft mislabeled this — the math was always right, only the label
> was wrong.

Notes: with `λ = 0, max_ppm = ∞`, `fit_ls_reg` is identical to `fit_ls` — so `fit_ls`
can be kept (tests `fit.rs:49-68` unchanged) and `fit_ls_reg` is the superset. Default
`λ` and `max_ppm` are §4.2 params (`drift_lambda_s2`, `drift_max_ppm`). The clamp makes
out-of-bound drift a *signal* (`clamped == true` and a high resulting `rms`), so a
spurious lock that wants a 5 % slope is rejected rather than fitted.

### 2.3 `verify.rs` — adaptive windows, lock-fraction, classifier

```rust
/// Fine waveform-lock verification of a candidate pair at a known coarse lag.
/// Pure & deterministic (§5.8): a function of the two pyramids + coarse_lag_ms +
/// params alone (no clock, no ids, no hashmap order). Lag convention matches
/// pair_correlate / SyncResult.offset_ms (core/src/pair.rs:18-19).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyFit {
    pub off_ms: f64,
    pub ppm: f64,
    pub rms_ms: f64,
    pub clamped: bool,        // drift hit the slope bound (a rejection signal)
    pub locked: u32,          // windows that locked (ncc > ncc_window_min, non-edge)
    pub expected: u32,        // MEASURABLE windows = placed windows minus silent ones
    pub lock_fraction: f64,   // locked / expected (0 when expected == 0)
    pub overlap_s: f64,
}

pub fn verify_pair(
    p_a: &PyramidReader,
    p_b: &PyramidReader,
    coarse_lag_ms: f64,
    params: &SyncParams,
) -> Option<VerifyFit>;        // None only when overlap is below the absolute hard floor

/// The classifier predicate: is this candidate a REAL match? Pure; all thresholds
/// from params (§4.2). Used by BOTH resolvers in Phase 2.
pub fn is_match(fit: &VerifyFit, params: &SyncParams) -> bool {
    fit.locked >= params.verify_min_locked
        && fit.lock_fraction >= params.verify_lock_fraction_min
        && fit.rms_ms <= params.verify_rms_max_ms
        && !fit.clamped                       // out-of-bound drift ⇒ not a match
}
```

**Adaptive window placement** (replaces the fixed `drift_windows = 12`). Density-per-second
with a width that fits and a total cap:

```
dur_a, dur_b = durations;  off_s = coarse_lag_ms/1000
t0 = max(0, −off_s);  t1 = min(dur_a, dur_b − off_s);  overlap = t1 − t0
if overlap < params.verify_min_overlap_s { return None }   // absolute hard floor (≈8 s)

// Width: as wide as fits, capped, never longer than the overlap allows a full window.
//   half_win = clamp( overlap * verify_win_frac/2 , verify_half_win_min_s , half_win_s )
// On a 20 s overlap with verify_win_frac=0.5 ⇒ ~5 s half-win (10 s windows) — short
// enough that measure_local locks (vs today's 9 s half-win / 18 s window that can't).
half_win = (overlap * params.verify_win_frac * 0.5)
            .clamp(params.verify_half_win_min_s, params.half_win_s)

// Count: density per second, min floor, hard cap (long-clip cost ceiling).
n = ((overlap * params.verify_density_per_s).round() as usize)
      .clamp(params.verify_min_windows_floor, params.verify_max_windows)

// Centers EVENLY spaced and clamped inside both tracks for every scanned lag, exactly
// like sync.rs:501-513 (so a window never truncates at a file edge). Span per window is
// the distance-scaled ±(base + scale·ppm) from sync.rs:495, but capped tighter because
// the slope bound forbids far lag excursions (verify_span_cap_ms < drift_span_cap_ms).
```

**locked / expected / lock_fraction.** For each placed window call
`measure::measure_local(p_a, p_b, tc, half_win, coarse_lag_ms, span)`:

- It returns `None` on **silence** (variance gate, `core/src/measure.rs:16-18`) OR on a
  **degenerate edge** peak (`core/src/measure.rs:108-118`). We must distinguish these for
  `expected`: a silent window is *not measurable* (don't count it against the fraction);
  an edge-reject or a low-NCC lock *is* measurable but didn't agree.
  - To get this cleanly, `verify_pair` first probes each window's **measurability** by the
    same variance test `measure_local` uses (σ² ≥ 1e-9 over the bins at the fine spb).
    Implement a tiny `pub(crate) fn window_measurable(p, spb, t0, t1) -> bool` in
    `measure.rs` (factored from the existing `scan` variance gate at `measure.rs:14-18`)
    so `verify.rs` and `measure_local` share ONE definition. (Cheap: one prefix-sum over
    the window.)
  - **Measurability must hold for BOTH tracks** (ruling, corrected). A window counts in
    `expected` only when the camera window AND the corresponding external window (the
    external span the camera window maps onto at `coarse_lag_ms`) are both above the
    variance floor. Camera-only would count a window where the *external* is silent — which
    can never lock — and so unfairly deflate `lock_fraction` for a genuine match over audio
    with quiet passages (exactly the case the lock-fraction metric must NOT penalize). So:
    `window_measurable(cam, spb, c0, c1) && window_measurable(ext, spb, e0, e1)`, where
    `[e0,e1)` is the external bin range covering the camera window shifted by the lag (the
    same superwindow `scan_level` builds at `measure.rs:84-95`).
  - `expected = count(BOTH tracks measurable at tc)`.
  - `locked = count(measure_local(tc) is Some AND its ncc > ncc_window_min)`.
    `measure_local` already applies the edge rejection internally and returns the fine NCC;
    a window that edge-rejects is `None` ⇒ not locked but still counted in `expected` if it
    was (both-tracks) measurable. `lock_fraction = locked / expected` (0 if `expected == 0`).
- Fit: collect the locked windows' `(t, lag_ms)` and call
  `fit::fit_ls_reg(&pts, params.drift_lambda_s2, params.drift_max_ppm)`. If `< 2` locked
  points, return a `VerifyFit` with `locked < 2`, `rms_ms = +∞`, `clamped = false` — so
  `is_match` fails it without a panic (do NOT return `None`; `None` is reserved for
  below-hard-floor overlap, which the caller treats as "no candidate at all").

**Subsuming `pair_fit_rms`.** The RMS-tiebreaker's `PairFit { off_ms, ppm, rms_ms,
windows }` is a strict projection of `VerifyFit` (`windows == locked`). Implementation
order (§9): (A) add `verify_pair`/`VerifyFit`; (B) reimplement `pair_fit_rms` as
`verify_pair(...).map(|v| PairFit{ off_ms:v.off_ms, ppm:v.ppm, rms_ms:v.rms_ms,
windows:v.locked })` so the existing Swift Phase-1/Phase-2 telemetry keeps working
unchanged during the transition; (C) once the resolver consumes `VerifyFit` directly
(§3, §4), delete `pair_fit_rms` and rename `pair_fit_batch` → `verify_batch`.

### 2.4 `Ctx` batch entry

Replace/evolve `Ctx::pair_fit_batch` (`core/src/ctx.rs:259-275`) with:

```rust
/// Per-(video, audio) fine verification for a set of candidate pairs. `pairs`:
/// (video_id, audio_id, coarse_lag_ms). Parallel (Rayon), deterministic (§5.8);
/// result aligns to input order, element None only below the hard overlap floor.
pub fn verify_batch(
    &self,
    pairs: &[(TrackId, TrackId, f64)],
    params: &SyncParams,
) -> Result<Vec<Option<crate::verify::VerifyFit>>>;
```

Same handle-resolution-up-front + `par_iter` discipline as `pair_candidates`
(`ctx.rs:243-251`) and the current `pair_fit_batch`. During the transition, keep
`pair_fit_batch` as a shim mapping `VerifyFit → PairFit` so the FFI/Swift layer compiles
until §B/§C land.

### 2.5 New SyncParams fields (§4.2 lists defaults+rationale)

Add to `SyncParams` (`core/src/sync.rs:14-38`, default block `:40-64`, and the `set()`
CLI override at `:66-89`). These are the classifier knobs; they do NOT alter the
editor-sync path (which reads `drift_windows`/`half_win_s`/etc. as today).

### 2.6 Determinism & byte-stability of the primitive

`verify_pair` is a pure function of pyramids+lag+params: window centers are arithmetic of
durations+offset (mirroring `sync.rs:501-513`); `measure_local` is deterministic;
`fit_ls_reg` is closed-form. No clock, no ids, no map iteration. This satisfies §5.8 and
is the precondition for letting it drive a deterministic decision (§3 byte-stability).

---

## 3. Constraint: byte-stability (gate6) — how both phases hold it

`gate6_stability` (`cli/tests/pair_gates.rs:287-328`) runs `couple pair --json`, adds an
unrelated audio, re-runs, and asserts each serialized `Row` JSON is **byte-identical**
(`pair_gates.rs:321-324`). The `Row` (`cli/src/pair.rs:174-184`) =
`{video, state, chip, selected, alternates, resolution}`; `RowCandidate` =
`{audio, z, lagMs, coverage}` (`cli/src/pair.rs:111-118`). The invariant is documented at
`core/src/pair.rs:20-22` ("No timing fields here — resolved rows must be byte-stable") and
`cli/src/pair.rs:13-17`.

Rules, enforced by construction (identical posture to the RMS-tiebreaker §2.2, now applied
to the richer signal):

1. **The new fine numbers (`lock_fraction`/`rms_ms`/`ppm`/`locked`/`expected`) NEVER enter
   the serialized CLI `Row`.** They are not added to `RowCandidate`, `ResolutionDetail`,
   `MatrixCell`, or `Row` in `cli/src/pair.rs`. Only the deterministic DECISION
   (`state`/`chip`/`selected`) may change, and only on fixtures re-baselined deliberately
   (§5). The Swift side likewise keeps them off `QueueStore` persistence
   (`mac/Sources/Couple/QueueStore.swift` persists only key/flags, never candidate
   metrics) — already true for the RMS-tiebreaker fields.
2. **Adding an unrelated non-matching audio must still leave a video's row byte-identical.**
   This requires the classifier to **deterministically REJECT** the unrelated file: with
   `is_match` false for it, the unrelated audio is not a candidate, contributes no
   alternate, and the row is unchanged. Because `verify_pair` is deterministic and the
   gate6 fixture's `newfile` (master_seed 777, `pair_gates.rs:306-310`) shares no content
   with the true master, its fine lock scatters (high `rms`, low `lock_fraction`) → not a
   match → row stable. **Phase 2 adds a Rust-core test asserting exactly this** (the unrelated
   file's `VerifyFit` fails `is_match`) so the stability guarantee is checked at the
   primitive, not just observed at the gate.
3. **Phase 1 changes no decision at all** (§4), so gate6 cannot move in Phase 1.

---

## 4. Phase 1 — instrument (no decision change)

Goal: surface the new signal everywhere — UI candidate detail + ambiguous banner + a
headless/debug readout — with the OLD `z`-decision still live, so the owner can SEE
real-from-fake separation on real files (including the 20 s clip) at zero regression risk.

### 4.1 What's already done (the RMS-tiebreaker baseline)

The uncommitted tree already wires a *narrow* version of this: `pair_fit_rms` →
`cpl_pair_fit_batch` (`ffi/src/lib.rs:831`) → `Engine.pairFitBatch`
(`mac/Sources/CoupleKit/Engine.swift:152`) → resolver telemetry
(`RowCandidate.fitRmsMs/fitWindows`, `Resolver.swift:97-98`; banner `fitBestRmsMs`,
`Resolver.swift:120-121`) → UI (`CenterColumn.swift:302-303` tooltip,
`:253` banner) + DebugDrive alternates log (`DebugDrive.swift:330`). Phase 1 here
**generalizes that pipe to the full `VerifyFit`** and **verifies the FULL candidate matrix
— every plausibly-overlapping V×A cell, REGARDLESS of the z-gate** (the critical fix
below), so the owner can eyeball the separation on the cases that matter (short genuine
clip vs short spurious — neither is a z-passing "candidate" today).

> CRITICAL (Phase-1 telemetry gap, corrected). The motivating 20 s clip is **rejected by
> `z`**, so it is NOT in `perVideo[vi]` (the z-passing candidate set). If Phase-1 verify
> ran only over z-passing candidates, the one case that motivated this entire work — the
> known-good 20 s clip — would never be verified or shown, and the owner could not
> validate the new signal on it before flipping Phase 2. **Therefore the Phase-1 debug
> readout (COUPLE_VERIFY_DUMP / `couple pair --verify-debug`) MUST iterate the full
> candidate matrix and run `verify_pair` on EVERY cell that plausibly overlaps (overlap ≥
> `verify_min_overlap_s`), with the flag OFF and the z-gate bypassed for the dump.** The
> matrix already holds every V×A cell (`per_video`/`MatrixCell` in the CLI; `matrix` in
> Swift), so the lag is in hand. UI surfacing of z-rejected cells is OPTIONAL; the
> full-matrix debug dump is MANDATORY — it is the owner's only window onto the 20 s clip's
> `lock_fraction`/`rms`/`ppm` while Phase 1 changes no decision.

### 4.2 New params (added in Phase 1, consumed only as telemetry until Phase 2)

In `SyncParams` (`core/src/sync.rs`), with `set()` keys; generous defaults + rationale:

| param | default | rationale |
|---|---|---|
| `verify_min_overlap_s` | 8.0 | absolute hard floor below which no candidate is possible; well under the 20 s clip we must admit. Replaces `min_overlap_bins`'s reject role for candidacy (§5). |
| `verify_win_frac` | 0.5 | window spans ≤ half the overlap so ≥ 2 disjoint windows always fit a short clip. |
| `verify_half_win_min_s` | 2.5 | floor on half-win; `measure_local` needs ≳ 200 ms of camera window (`measure.rs:79`), 2.5 s is comfortably feature-rich yet fits a 20 s clip. |
| `verify_density_per_s` | 0.20 | ~1 window / 5 s → 20 s overlap ⇒ ~4 windows; 4 min ⇒ ~48 (capped). Density, not fixed count (the owner's lock-fraction insight). |
| `verify_min_windows_floor` | 3 | always attempt ≥ 3 (fit minimum). |
| `verify_max_windows` | 24 | long-clip cost ceiling (2× today's 12; keeps gate7 budget, §5/§6). |
| `verify_max_candidates_per_video` | 6 | **cost cap on large pools** (ruling D3 / §6): verify at most this many cells per video, ranked by NCC. 6 is generous for legit duplicate recorders. Bounds total verify at O(V·K) regardless of pool size. |
| `verify_span_cap_ms` | 500.0 | per-window lag search cap; far tighter than `drift_span_cap_ms = 2000` because the slope bound forbids far excursions ⇒ faster + fewer spurious locks. |
| `drift_lambda_s2` | 50.0 | ridge weight in **s²** (added to sxx = Σ(t−t̄)², itself s² — see §2.2 units warning). At ~50 s² it gives a modest shrink on a ~20 s clip (whose sxx is O(10²–10³ s²), so λ is a fraction of sxx ⇒ slope softened, not erased) and is negligible on long clips / the 24-min pair (sxx O(10⁵–10⁶ s²) ≫ 50 ⇒ slope≈OLS). Tune in Phase 1 by reading `ppm` on the real 24-min pair (known +3.57 ppm, `pair_gates.rs:499`) — `drift_lambda_s2` must be small enough to still recover it. |
| `drift_max_ppm` | 2000.0 | generous physical bound (covers NTSC pulldown ~1000 ppm; rejects absurd). Out-of-bound ⇒ `clamped` ⇒ not a match. |
| `verify_min_locked` | 4 | small absolute floor on locked windows. |
| `verify_lock_fraction_min` | 0.66 | ≥ ⅔ of measurable windows must agree (the dense-short-clip-beats-sparse-long-clip rule). |
| `verify_rms_max_ms` | 9.0 | trust floor; real true-pairs run ≈ 5 ms (`Resolver.swift:18`), generous to 9. Reuses the RMS-tiebreaker's validated `rmsTrustFloorMs = 9.0`. |
| `verify_prefilter_ncc` | 0.15 | Stage-A loose resemblance floor (§5.1): rejects only obvious garbage; the fine lock disposes. |

These mirror into the shell params (§4.4) and the CLI `PairParams` (§5) so BOTH resolvers
share identical thresholds (CLI parity — the gates run through the CLI).

### 4.3 Core → FFI → CoupleKit (full data path)

- **Core:** `verify.rs` (`verify_pair`/`VerifyFit`/`is_match`), `fit.rs::fit_ls_reg`,
  `ctx.rs::verify_batch`, new `SyncParams` fields. `pair_fit_rms`/`pair_fit_batch` become
  `VerifyFit` shims (§2.3).
- **FFI:** evolve `cpl_pair_fit_batch` (`ffi/src/lib.rs:831-855`) to return the full
  `VerifyFit` array (add fields to the JSON; same JSON-in/JSON-out signature, **no ABI
  bump** — `cpl_abi_version` stays 1). Update the hand-maintained header
  `/Users/tim/code/claude_apps/couple/ffi/couple.h` (the block at `couple.h:266-275`) to
  describe the richer payload. Optionally rename to `cpl_verify_batch` with a one-line
  back-compat note; keeping the name minimizes churn.
- **CoupleKit:** extend `Wire.PairFit` (`mac/Sources/CoupleKit/Wire.swift:247-259`) to
  `VerifyFit` (add `clamped`, `locked`, `expected`, `lockFraction`, `overlapS`; keep
  `rmsMs`/`ppm`/`offMs`). `Engine.pairFitBatch` (`Engine.swift:152-160`) decodes the new
  type. (Rename to `verifyBatch` optional.)

### 4.4 Resolver telemetry (Swift) — compute for ALL candidates, decide nothing new

In `mac/Sources/Couple/Resolver.swift`:

- Carry the full fit on `RowCandidate` (extend the existing `fitRmsMs`/`fitWindows`
  `:97-98` with `lockFraction`, `expected`, `ppm`, `clamped` — all optional, all
  informational). They remain excluded from any equality/persistence (the type is not
  `Equatable`/`Hashable`; `QueueStore` ignores them) — same property the RMS-tiebreaker
  established.
- **Widen the fit request set.** Today `fitReqs` are appended only inside the contested,
  non-simultaneous branch (`Resolver.swift:391-395`). For Phase-1 *observation* in the
  resolver/UI path, **request a fit for every candidate in `perVideo[vi]`** (all z-passing
  cells), so every candidate the owner sees in a list carries its `VerifyFit`. Cost is
  bounded by the batch + the cap (§6); the common single-confident row is one extra fit.
  Attach results in `finalize` (`Resolver.swift:282-311`) exactly as today.
  - LIMITATION (and why the dump is separate): `perVideo[vi]` is the z-passing set, so the
    z-rejected 20 s clip is NOT in it — the resolver/UI path alone cannot show it in
    Phase 1. Observing the z-rejected clip is the job of the **mandatory full-matrix debug
    dump** (§4.1 CRITICAL, §4.5), which bypasses the z-gate. Surfacing z-rejected cells in
    the live UI is optional and deferred.
- **No `if` reads the fit in Phase 1.** The Phase-2 escalation block
  (`Resolver.swift:294-298` calling `escalate`) is the ONLY decision consumer and is
  GATED by a feature flag defaulted OFF in Phase 1 (§4.6). Verdicts are byte-identical to
  the pre-RMS-tiebreaker baseline.

### 4.5 Headless / debug readout

- **DebugDrive:** extend the alternates log (`DebugDrive.swift:330`) and add a dedicated
  `COUPLE_VERIFY_DUMP=1` path that, for a given session folder, prints per (video,audio):
  `z, lag, overlap_s, locked/expected, lock_fraction, rms_ms, ppm, clamped, is_match`.
  This is the owner's "see the new signal separate real-from-fake on real files (incl. the
  20 s clip)" surface, runnable without the GUI. **MANDATORY: it iterates the FULL matrix
  and verifies every cell with overlap ≥ `verify_min_overlap_s`, bypassing the z-gate** —
  otherwise the z-rejected 20 s clip is invisible (§4.1 CRITICAL).
- **CLI (read-only, no stable-surface change):** add `couple pair --verify-debug` that
  prints the same per-candidate verify table to **stderr** (never into the `--json`
  `Row`/`MatrixCell`). This keeps gate6 untouched (stderr is not serialized) while giving a
  headless readout that runs through the real pairing path. **It MUST cover the full V×A
  matrix, NOT just z-passing candidates**: after the matrix step (`cli/src/pair.rs:299`),
  build the verify request list from EVERY `MatrixCell` whose `m.overlap_bins` clears
  `verify_min_overlap_s` (z bypassed), call `ctx.verify_batch`, and `eprintln!` each row
  with its `is_match` verdict. (This is the dump's whole point — the row the owner needs to
  see, the rejected short clip, never clears the z-gate.)
- **UI:** tooltip already shows `fit … ms (… win)` (`CenterColumn.swift:302-303`); extend
  to `fit R ms · F% lock · D ppm` and the ambiguous banner (`:253`) to the full
  separation line. Keep read-only.

### 4.6 The flag

Add `verifyClassifierActive` (Swift `PairParams`, default **false** in Phase 1) and a CLI
`--params verify-active=0/1` (default 0). When false: candidacy and acceptance are exactly
today's `z`-gate; verify is telemetry only. When true (Phase 2): the new classifier
decides (§5). This single switch is the Phase-1↔Phase-2 boundary and the owner's
post-eyeball toggle.

---

## 5. Phase 2 — flip the decision (z demoted to pre-filter; fine-verify classifies)

Active only when `verify-active = 1`. Builds on Phase 1's instrumentation.

### 5.1 Stage A becomes a loose pre-filter

- **Candidacy no longer requires `z >= z_min`.** In both resolvers, the candidate test
  changes from `z >= zMin` to a *loose* pre-filter: keep a cell as a **verify candidate**
  when `!insufficientOverlap` AND it has a plausible lag (stage-1 found a peak) AND a cheap
  resemblance floor passes (`ncc >= verify_prefilter_ncc`, default ≈ 0.15 — only rejects
  obvious garbage; far below today's effective bar). `z` is no longer consulted to reject.
  - Swift: `Resolver.swift:173` filter → `!m.insufficientOverlap && m.ncc >=
    params.verifyPrefilterNcc` (under the flag).
  - CLI: `cli/src/pair.rs:311` `candidate = !m.insufficient_overlap && m.ncc >=
    params.verify_prefilter_ncc` (under the flag).
- **Lower `min_overlap_bins` for candidacy.** Stage-1 still needs *some* overlap to find a
  lag; drop the candidacy floor to `verify_min_overlap_s` (≈ 8 s). `min_overlap_bins`
  stays as the stage-1 scan's mechanical lower bound but is reduced (e.g. 40 bins ≈ 8 s) so
  short clips reach the fine stage. **Open decision §8 (D3):** how aggressive — 8 s vs
  even lower.
- The pre-filter's only jobs are *where to look* and *skip obvious garbage*; everything
  decisive moves to Stage B.

### 5.2 Stage B is the classifier

After the (now looser) candidate set is formed, **cap it per video, then** run
`verify_batch` over the capped set and keep only those with `is_match(fit, params) ==
true`. Concretely:

- **Per-video cost cap (ruling D3, bounds large pools).** The loose `ncc ≥ 0.15`
  pre-filter can admit O(V×A) cells, and fine verify is far costlier than the envelope
  matrix → on a big real session verify would dominate. So for each video, keep only the
  top `verify_max_candidates_per_video` (≈ 6) pre-filtered cells **ranked by NCC**
  (`PairCorrelation.ncc`, the overlap-INDEPENDENT resemblance), and verify only those.
  - Rank by **NCC, NOT z**. Ranking by `z = NCC·√n` would re-introduce the exact
    short-clip penalty this whole plan removes (a real 20 s clip's small √n would push it
    below a 4-min impostor's, evicting it from the top-K before verify ever runs). NCC is
    scale-free, so a clean short clip ranks on its resemblance, not its length.
  - This bounds total fine verify at **O(V·K)** regardless of pool size. K = 6 is generous
    for legitimate duplicate recorders (gate3/gate7 simultaneous cases have ≤ ~6 real
    sources). A pool with more than K plausible audios per video is pathological; the
    excess are near-certainly non-matches the cap drops before the costly stage.
  - Swift: after building `perVideo[vi]` under the loose filter, `sort by m.ncc desc` and
    `prefix(verify_max_candidates_per_video)` before issuing `verify_batch`. CLI: same
    truncation on `per_video[vi]` (`cli/src/pair.rs:318-320`) before the verify call.
- A cell is a **real match** iff `verify_pair` returns a `VerifyFit` with `locked ≥
  verify_min_locked` AND `lock_fraction ≥ verify_lock_fraction_min` AND `rms_ms ≤
  verify_rms_max_ms` AND `!clamped`.
- The set of real matches per video replaces today's `cands` (the z-passing list). Then the
  EXISTING §3.3 pipeline runs unchanged on that set: clustering (`cluster_intervals`),
  §3.4 simultaneity (`correlate_audio_pair` triangle), §3.5 margin, coverage selection.
  Duplicates (multiple real matches that are simultaneous) still resolve to `2 sources`;
  disjoint real matches → `split`; one real match → `confident`/`partial`.
- **Subsume the RMS escalation.** The Phase-2 RMS-tiebreaker (`escalate`/`rmsEscalate`,
  `Resolver.swift:474-546`, gated by `rmsTrustFloorMs`/`rmsRatio`/`minFitWindows`) is
  **folded into the classifier**: once acceptance is by `is_match`, a same-piece *wrong*
  take fails `is_match` (it scatters → high `rms`, low `lock_fraction`) and never becomes a
  match, so the margin rule rarely reaches an ambiguous contest in the first place. The
  `minFitWindows` gate is reconciled into `verify_min_locked` (same meaning: enough locked
  windows to trust the fit). Keep `escalate` as a thin compatibility path ONLY for the
  residual case where two *genuine* simultaneous-looking matches both pass `is_match` but
  the margin is thin and one has the cleaner lock — i.e. demote it from "rescue an
  ambiguous z-contest" to "break a tie among already-classified real matches." The
  conservative↔strong switch (`rmsStrongForm`, `Resolver.swift:508`) stays as-is.

### 5.3 CLI parity (required — gates run through the CLI)

Phase 2 MUST change the CLI resolver too (the RMS-tiebreaker deliberately deferred this;
this plan does not, because the overseer requires shared tunable params used by BOTH
resolvers and the gates exercise the CLI). In `cli/src/pair.rs`:

- Mirror the new params into `PairParams` (`cli/src/pair.rs:31-44`) + `set()`
  (`cli/src/pair.rs:58-73`) + `sync_params()` (`cli/src/pair.rs:374-379`, which currently
  only forwards `z_min` — forward the new verify fields too).
- Under `verify-active`, replace the candidacy filter (`:311`) and insert a `verify_batch`
  + `is_match` gate before `resolve_row` builds clusters (the real-match set replaces
  `per_video[vi]`). `resolve_row`'s body (`:408-575`) is otherwise unchanged.
- Keep all verify numbers OUT of the serialized `Row` (§3). Add the `--verify-debug`
  stderr dump (§4.5).

### 5.4 Rollout recommendation

**Ship Phase 2 behind the flag, default OFF; the owner flips `verify-active=1` after
eyeballing Phase 1 on real sessions (incl. the 20 s clip).** Rationale: the classifier
re-baselines several gates and changes real verdicts; the owner explicitly wants to SEE
the signal separate real-from-fake before it decides. Once validated, flip the default to
ON in a one-line change (the flag default) — no code path differences, so the flip is
low-risk and reversible. This is open decision §8 (D2); the recommendation is
flag-default-OFF for the first ship, flip-to-ON as a fast follow after owner sign-off.

---

## 6. Gate impact analysis + re-baselining (preserve each gate's INTENT)

Phase 1: **no gate moves** (no decision change; verify numbers never serialized).
Phase 2 (flag ON) analysis, per gate, with intent preserved:

- **gate6_stability** (`pair_gates.rs:287`) — INTENT: adding an unrelated file changes
  only what it can. Verify numbers stay out of `Row` (§3). The unrelated `newfile` (seed
  777) must **deterministically fail `is_match`** so it never becomes a candidate/alternate
  → rows byte-identical. Re-baseline: none expected; add the §3 core test asserting the
  unrelated pair fails `is_match`. **At risk only if** the loose pre-filter admits the
  unrelated file as a *verify candidate* whose fine lock then (correctly) rejects it — that
  is fine for `Row` stability (rejected ⇒ no alternate). Per ruling **D4**, the matrix
  `candidate` flag KEEPS its current meaning ("passed the pre-filter") and verify outcome
  is NEVER serialized — so `candidate` does not change shape and gate6 stays trivially
  safe. (We do NOT redefine `candidate` from `is_match`.)
- **gate7_matrix_budget** (`pair_gates.rs:330`, ≤ 5 s for 6×6) — INTENT: performance.
  Phase 2 adds a `verify_batch` over candidate cells. Cost model (§quantified below) keeps
  it green via: (1) the **per-video top-K-by-NCC cap** (`verify_max_candidates_per_video =
  6`, §5.2) bounds fine verify at O(V·K) — the dominant guard on any large pool, not just
  the 6×6; (2) `verify_max_windows` caps long clips at 24 (2× today's 12 but on a 6×6 of
  30 s clips most cells aren't candidates); (3) tighter `verify_span_cap_ms` (500 vs 2000)
  shrinks each `measure_local`. Re-baseline: keep the assert at 5000 ms but log the new
  `matrix+verify` timing; if the 6×6 all-simultaneous fixture pushes close, lower
  `verify_density_per_s`, `verify_max_windows`, or `verify_max_candidates_per_video`. Add a
  `verify` timing field to `TimingMs` **stderr/log only** (not serialized — keep it out of
  `Row`).
- **gate8_honesty_floor** (`pair_gates.rs:373`) — INTENT: never confidently select a
  decoy. The decoy (same plan, ±400 ms jitter) currently classifies *simultaneous*
  (`pair_gates.rs:405-411`) so it never hits the conflict branch. Under the classifier, the
  decoy is a *different take*: its fine lock should **fail `is_match`** (the ±400 ms
  micro-timing jitter destroys waveform agreement → high `rms`/low `lock_fraction`) so it
  is not a real match at all — a STRONGER honesty guarantee than today. Re-baseline: assert
  the decoy is NOT a match (not just "not confident") and that the true take is. **Risk:**
  if synthetic jitter is too coarse for the fine lock to reject (200 ms stage bins
  integrate it, per the comment), the decoy might still pass — in which case add a
  finer-jitter or different-master decoy fixture (§ new fixtures). The gate's intent
  (no confident decoy) is preserved because `is_match` can only ever admit the true take or
  honestly reject both.
- **margin_rule_prefers_ambiguous** (`pair_gates.rs:427`) — INTENT: prefer `ambiguous`
  over a coin-flip when z can't separate. It forces the conflict branch via
  `triangle-tol-ms=0.000001` + `z-margin=1000` on two same-master externals (extA/extB).
  Under the classifier BOTH externals are genuine same-master matches → both pass
  `is_match` with clean locks → they are simultaneous duplicates, NOT a conflict, so the
  row becomes `2 sources` (or stays ambiguous if the forced `triangle-tol≈0` still blocks
  simultaneity). Re-baseline options: (a) run this gate with `verify-active=0` so it keeps
  asserting the **z-path** ambiguity (the behavior it was written to protect) — cleanest,
  since the test's intent is the z-margin rule, which still exists as the pre-filter-era
  path; (b) re-baseline to `2 sources` with justification. **Recommend (a)**: pin
  `verify-active=0` for this specific test; add a NEW `verify_classifies_takes` test for
  the classifier path.
- **realdata_pair_confident_row** (`pair_gates.rs:463`) — INTENT: the canonical 24-min
  pair is one confident row at the frozen lag (−5536.639 ms ± 25). Under the classifier it
  must STILL be one confident real match. Risk: `drift_lambda_s2` must not over-flatten the
  real +3.57 ppm (`pair_gates.rs:499`); the `lag` assertion is on stage-1 `lagMs` (not the
  fitted intercept) so it is unaffected, but `is_match` must accept (clean lock, ~5 ms).
  Re-baseline: none if defaults are sane; this gate is the calibration anchor for
  `drift_lambda_s2`/`verify_rms_max_ms` (verify the real pair's `rms` and `lock_fraction`
  in Phase 1 before choosing defaults).
- **gates 1–5, 7-truth** — confident/split/twoSources/noMatch on clean fixtures; the
  classifier accepts the same true matches (clean locks) and the loose pre-filter doesn't
  newly admit unrelated content (it fails `is_match`). Re-baseline: re-run; expect green.
  Watch gate4 (`unrelated`/different-master, `pair_gates.rs:205`): the unrelated file must
  fail `is_match` so video2 stays `noMatch` and `unused` is unchanged.

**New fixtures to add** (reuse `fixture::generate_session`, `pair_gates.rs:5`):

1. **Short genuine clip** — a 20 s external cut from the same master as the video, full
   overlap. ASSERT: with `verify-active=1` the row is `confident`/`partial` selecting it
   (today it is rejected by `z`). This is THE regression the owner reported.
2. **Short spurious clip** — a 20 s external from a *different* master overlapping a sliver.
   ASSERT: not a match (`is_match` false) → `noMatch`/unused. (The opposite failure mode.)
3. **Same-master different-take** if feasible — extend the fixture generator's
   `plan_jitter_ms` decoy to a take the fine lock can reject; ASSERT the true take is the
   only match. (If synthetic jitter can't be made rejectable, document the limitation as
   gate8 already does, `pair_gates.rs:369-371`, and rely on the real corpus.)

### Cost delta (quantified)

Per `measure_local` is budgeted ≤ 5 ms (`core/src/measure.rs:2`). Today the fine pass is
≤ 12 windows ≈ ≤ 60 ms per fitted pair. Phase 2 fine-verify runs on the **capped**
candidate set: at most `verify_max_candidates_per_video` (= 6) cells per video (top-K by
NCC, §5.2), each ≤ `verify_max_windows` (= 24) windows with a tighter span (each window
cheaper than the ≤ 5 ms bound). **Total fine verify is therefore O(V · K · W) =
O(V · 6 · 24) — independent of audio-pool size A**, which is the key guard for large real
sessions (without the cap, the loose pre-filter could make it O(V·A·W) and dominate). On
the gate7 6×6 (all 30 s clips, all simultaneous → ≤ 6 verified cells/row × 6 rows = ≤ 36
cells, each ~6–24 windows): worst ≈ 36 × 24 × (cheaper-than-5 ms), Rayon-parallel across
cells → wall-clock well under the 5 s budget (the existing envelope matrix is still the
dominant cost). The 20 s short-clip case is ~4 windows — trivial. Net expected delta: tens
of ms on a typical session, hard-bounded by K and the window cap regardless of pool size;
quantify exactly in Phase 1 via the new `verify` timing log and gate7.

---

## 7. Editor-sync interaction (do NOT regress the validated full-sync path)

The live editor sync is `auto_sync_tracks` → `drift_pass` → `fit_ls`
(`core/src/sync.rs:666-725`, `:474-584`), surfaced as `SyncResult` and consumed by
`PairSession`. This plan **does not touch `drift_pass` or `auto_sync_tracks` destructively**:

- `verify.rs` is a separate module; `fit_ls` is left intact (tests `fit.rs:49-68` pass);
  `fit_ls_reg` is additive. The full-sync status gates (`sync.rs:701-708`, reading
  `drift.accepted`/`fit_rms_ms`/`synced_*`) are unchanged.
- **Shared benefit — DEFERRED follow-up (ruling D5), NOT in this plan's scope.** The
  adaptive-window + slope-bounded improvements would also make *short clips syncable in the
  editor*, but per D5 this plan is **classifier-only and does NOT touch `drift_pass`**. The
  potential future sharing (have `drift_pass` opt into the new placement/`fit_ls_reg` for
  short overlaps, behind a `SyncParams` opt-in defaulting to current behavior so the §9.4
  sync freeze does not move) is recorded here only as a candidate follow-up after the
  classifier ships and bakes. `verify.rs` stays fully standalone; isolate first, share
  later (overseer guard 4).

---

## 8. Risks & DECISIONS — overseer rulings (RECORDED 2026-06-13)

All six decisions are RULED. They are recorded here as the build contract; the residual
"risk" on each is what implementation must watch, not an open question.

- **D1 — Default thresholds: RULED — accept the proposed defaults as Phase-1-CALIBRATED
  STARTING points (NOT hard-frozen).** `verify_rms_max_ms = 9`, `verify_lock_fraction_min =
  0.66`, `verify_min_locked = 4`, `drift_max_ppm = 2000`, `drift_lambda_s2 = 50`,
  `verify_prefilter_ncc = 0.15`, `verify_min_overlap_s = 8`, `verify_max_candidates_per_video
  = 6` (units fixed: λ is s², §2.2). **Phase 1 MUST validate against the real 24-min pair
  (recover +3.57 ppm; sane `rms`/`lock_fraction`) AND the 20 s clip BEFORE Phase 2 is
  flipped on.** Do not hard-freeze the numbers now; the COUPLE_VERIFY_DUMP / `--verify-debug`
  readout is the calibration instrument. Risk to watch: `drift_lambda_s2` over-flattening
  the real drift, and `verify_lock_fraction_min` (the core real-vs-fake knob) being mis-set
  on quiet-passage audio (mitigated by both-tracks `expected`, §2.3).
- **D2 — Phase-2 default: RULED — ship `verify-active` flag-default-OFF; owner flips to ON
  after eyeballing Phase 1.** Flip-to-default-ON is a later one-line change once validated.
- **D3 — Pre-filter aggressiveness / min-overlap floor / cost cap: RULED.** Candidacy
  overlap floor = **8 s** (`verify_min_overlap_s`); loose pre-filter NCC floor = **0.15**
  (`verify_prefilter_ncc`); plus the **top-K-by-NCC per-video cap** (`verify_max_candidates_
  per_video = 6`, ranked by NCC not z, §5.2) to bound verify at O(V·K) on large pools. This
  is the combined large-pool cost guard.
- **D4 — `matrix.candidate` semantics: RULED — keep `candidate` = "passed the pre-filter".**
  Do NOT redefine it from `is_match`, and never serialize verify outcome. This keeps gate6
  trivially safe (the `candidate` flag's shape/meaning is unchanged for a given input).
- **D5 — Fitter scope: RULED — classifier-only.** `fit_ls_reg` lives in/serves `verify.rs`
  only. **Do NOT touch `drift_pass`**; the editor-sync sharing in §7 is a DEFERRED follow-up,
  explicitly out of this plan's scope (protects the §9.4 sync freeze).
- **D6 — Keep the RMS `escalate` path: RULED — keep it as a thin tie-break among
  already-classified real matches** (§5.2 demotion). Do not delete it now; the
  conservative↔strong switch (`rmsStrongForm`) stays.

---

## 9. File-by-file change list (3 independently buildable stages)

### Stage A — core engine + fitter + tests (independently buildable/reviewable)

- `/Users/tim/code/claude_apps/couple/core/src/fit.rs` — add `FitReg` + `fit_ls_reg`
  (§2.2); keep `fit_ls`. Tests: regularization pulls a sparse short fit flat; clamp marks
  out-of-bound slope; with `λ=0,max=∞` equals `fit_ls`.
- `/Users/tim/code/claude_apps/couple/core/src/verify.rs` — NEW. `VerifyFit`,
  `verify_pair` (adaptive windows, locked/expected/lock_fraction, `fit_ls_reg`), `is_match`
  (§2.3). Tests: clean short pair (20 s) → `is_match` true, high `lock_fraction`, low
  `rms`, `locked ≥ 4`; mis-lagged/different-master pair → `is_match` false (scatter or
  `clamped`); the gate6 unrelated pair (seed 777) → `is_match` false (stability anchor,
  §3); determinism bit-identical (mirror `sync.rs:973-989`).
- `/Users/tim/code/claude_apps/couple/core/src/measure.rs` — factor the variance gate
  (`measure.rs:14-18`) into `pub(crate) fn window_measurable(p, spb, b0, b1) -> bool`
  shared by `measure_local` and `verify_pair`; `verify_pair` calls it on BOTH the camera
  AND the external window (§2.3 ruling) so quiet-passage audio isn't penalized. No behavior
  change to `measure_local`.
- `/Users/tim/code/claude_apps/couple/core/src/sync.rs` — add the new `SyncParams` fields
  (§4.2) + `set()` keys; reimplement `pair_fit_rms` as a `verify_pair` shim, or mark it for
  deletion once §C lands (§2.3). Leave `drift_pass`/`auto_sync_tracks` untouched (§7).
- `/Users/tim/code/claude_apps/couple/core/src/ctx.rs` — add `verify_batch` (§2.4); keep
  `pair_fit_batch` as a shim during transition.
- `/Users/tim/code/claude_apps/couple/core/src/lib.rs` — `pub mod verify;`.

### Stage B — FFI / CoupleKit plumbing (depends on A)

- `/Users/tim/code/claude_apps/couple/ffi/src/lib.rs` — evolve `cpl_pair_fit_batch`
  (`:831`) to emit the full `VerifyFit` JSON (no ABI bump); optional rename
  `cpl_verify_batch`.
- `/Users/tim/code/claude_apps/couple/ffi/couple.h` — update the declaration/comment block
  (`couple.h:266-275`) for the richer payload.
- `/Users/tim/code/claude_apps/couple/mac/Sources/CoupleKit/Wire.swift` — extend
  `PairFit` (`:247-259`) to the full `VerifyFit` fields.
- `/Users/tim/code/claude_apps/couple/mac/Sources/CoupleKit/Engine.swift` — `pairFitBatch`
  (`:152-160`) decodes the new type (optional rename `verifyBatch`).

### Stage C — resolver / CLI integration + instrumentation + gate re-baselining (depends on B)

- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Resolver.swift` — extend
  `RowCandidate` telemetry fields (`:97-98`); add the `verifyClassifierActive` flag +
  verify params to `PairParams` (`:19-32`); Phase-1 widen `fitReqs` to all z-passing
  candidates (`:391-395`); Phase-2 (flag) replace candidacy filter (`:173`) with the loose
  pre-filter, **cap to top-K-by-NCC per video** (`verify_max_candidates_per_video`, §5.2),
  gate the real-match set by `is_match`, fold/keep `escalate` (§5.2). No persistence/
  equality change (gate-safe).
- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Views/CenterColumn.swift` —
  extend tooltip (`:302-303`) + banner (`:253`) to the full separation line (read-only).
- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/DebugDrive.swift` — extend
  alternates log (`:330`) + add `COUPLE_VERIFY_DUMP=1` per-candidate table that **verifies
  the FULL matrix, z-gate bypassed** (§4.1/§4.5 — so the z-rejected 20 s clip is shown);
  extend the `rmsTest()` cases (`:549+`) to the new predicate via injected `VerifyFit`.
- `/Users/tim/code/claude_apps/couple/cli/src/pair.rs` — mirror verify params into
  `PairParams` (`:31`) + `set()` (`:58`) + `sync_params()` (`:374`); under `verify-active`,
  replace the candidacy filter (`:311`), **cap to top-K-by-NCC per video** (§5.2), and
  insert `verify_batch` + `is_match` before `resolve_row` clustering; add `--verify-debug`
  stderr dump that **covers the full V×A matrix, z bypassed** (§4.5). Keep verify numbers
  OUT of `Row`/`MatrixCell` (§3).
- `/Users/tim/code/claude_apps/couple/cli/tests/pair_gates.rs` — pin `verify-active=0` on
  `margin_rule_prefers_ambiguous` (§6 option a); add `gate_short_genuine_clip`,
  `gate_short_spurious_clip`, `verify_classifies_takes`; re-run gate4/gate6/gate7/gate8 and
  re-baseline only with logged justification (PROGRESS.md V2-F2 convention,
  `pair_gates.rs:116-117`).
- `/Users/tim/code/claude_apps/couple/PROGRESS.md` — log chosen thresholds + any fixture
  re-baseline as a V2-F2 calibration entry.

### Sequencing

A is reviewable alone (pure core + tests, no behavior change downstream). B is additive
plumbing. C lands Phase 1 (telemetry, flag OFF) first — zero decision change, gate6/7/8
green — then Phase 2 (flag ON behavior + new fixtures + re-baselining) as a distinct
reviewable change behind the same flag.
