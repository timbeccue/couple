# PLAN — Drift-fit RMS as an escalation tie-breaker for ambiguous pairings

Status: design plan. Two phases. Phase 1 is observe-only (no decision change). Phase 2
gates behind tunable params. Written 2026-06-13.

All file paths absolute. Every code claim below was verified against the tree at the
commit on `main` (Phase 9 / Session H).

---

## 1. Problem and chosen design

### 1.1 The defect

The resolver chooses which audio pairs with each video from a single stage-1 score
`z = NCC · √n` — NCC of the 200 ms amplitude envelope times √(overlap-bins). It is
formed at `/Users/tim/code/claude_apps/couple/core/src/sync.rs:288`
(`let z = ncc * (n as f64).sqrt();`) inside `stage1_scan`, surfaced on
`PairCorrelation.z` (`/Users/tim/code/claude_apps/couple/core/src/pair.rs:26`).

When two candidate audios overlap the same video span and the §3.4 simultaneity /
triangle-closure check decides they are NOT simultaneous (= different takes), the §3.5
margin rule runs:

- Swift resolver: `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Resolver.swift:240-248`
- CLI mirror:    `/Users/tim/code/claude_apps/couple/cli/src/pair.rs:516-528`

```
let margin = zBest - zSecond
if zSecond < zMin || margin >= zMargin { select best }
else { ambiguous }            // no selection — user must pick (V2-F2 honesty)
```

For different takes of the **same musical piece**, the 200 ms envelopes are nearly
identical, both score high `z`, `margin` collapses below `zMargin` (6.0), and the row is
marked `ambiguous` even when one take is obviously the correct one. `z` is structurally
blind here: it only measures coarse envelope shape, which two takes of the same score
share.

### 1.2 The signal that separates them

The post-pairing least-squares drift fit RMS does separate them cleanly. `fit_ls`
(`/Users/tim/code/claude_apps/couple/core/src/fit.rs:15-43`) fits `(t_s, lag_ms)` points
to `off + slope·t` and returns `rms_ms = sqrt(Σ residual² / n)`
(`fit.rs:41`). The correct take locks a clean offset+drift line end-to-end across the
whole overlap (real true-pairs run ≈5 ms per the comment at
`Resolver.swift:18`); a wrong take scatters (20+ ms). RMS measures fine **waveform** lock
across the take — the true identity signal that envelope `z` misses.

The same RMS is already computed two places, proving the math is trusted:
- core full sync: `auto_sync_tracks` → `drift_pass` → `fit_ls`, surfaced as
  `SyncResult.fit_rms_ms` (`/Users/tim/code/claude_apps/couple/core/src/sync.rs:610-613`,
  `639`);
- Swift live editor: `PairSession.fitStatus` recomputes `rms = sqrt(Σ residual²/n)` over
  `driftWindows` against the current mapping
  (`/Users/tim/code/claude_apps/couple/mac/Sources/Couple/PairSession.swift:172-177`).

### 1.3 Chosen design (conservative, two-phase)

Use **per-candidate drift-fit RMS as an escalation tie-breaker** that only rescues rows
that are `ambiguous` today — never overriding a clear `z` winner, never coin-flipping,
never firing on the simultaneous (`2 sources`) path.

- **Phase 1 — MEASURE & SURFACE (no behavior change).** For candidates in a *contested*
  cluster (2+ candidates overlapping one span, not simultaneous), compute per-candidate
  drift-fit RMS plus the window/anchor count behind it. Carry it into
  `ResolutionDetail` / `RowCandidate`, surface it informationally in the candidate
  list/detail. Decisions are byte-for-byte unchanged. Purpose: let the owner validate the
  ≈5-vs-20+ separation across many real sessions before it gates anything.

- **Phase 2 — GATE (behavior change, behind tunable params).** In the non-simultaneous
  conflict branch, before falling back to `ambiguous`, consult RMS: if the best
  candidate's RMS is below a generous trust floor (≈8–10 ms — deliberately not 5) AND
  decisively better than the runner-up by a *ratio* (`rms_second / rms_best ≥ ≈3`), select
  it (confident/partial per coverage) instead of `ambiguous`; else keep `ambiguous`. New
  `[TUNE]`/`[SPEC]` params, with guards: minimum window/anchor count, comparable coverage
  only, never on the simultaneous path.

The two phases are independently implementable and reviewable. Phase 1 ships and bakes;
Phase 2 turns on later once thresholds are validated.

---

## 2. Critical constraint: the byte-stability gate

### 2.1 What the gate hashes

The resolved-row stability contract is asserted by **`gate6_stability`**
(`/Users/tim/code/claude_apps/couple/cli/tests/pair_gates.rs:287-328`). It runs
`couple pair --json`, adds one unrelated audio file, re-runs, and asserts:

```rust
for v in ["video1.mp4", "video2.mp4"] {
    let a = serde_json::to_string(row(&r1, v)).unwrap();
    let b = serde_json::to_string(row(&r2, v)).unwrap();
    assert_eq!(a, b, "row {v} changed after an unrelated file was added");
}
```

i.e. the entire serialized `Row` JSON (`cli/src/pair.rs:174-184`: `video, state, chip,
selected, alternates, resolution`) must be byte-identical. `Row.selected`/`alternates`
are `RowCandidate { audio, z, lagMs, coverage }` (`cli/src/pair.rs:111-118`);
`resolution` is `ResolutionDetail { margin, clusters, triangles, note }`
(`cli/src/pair.rs:159-172`).

The deeper invariant is documented at `core/src/pair.rs:20-22`:

> Deterministic: same inputs → bit-identical values on the same machine (§5.8) … No
> timing fields here — resolved rows must be byte-stable across runs.

and the CLI header `cli/src/pair.rs:13-17` ("resolved rows contain no run-varying
fields"). RMS is **timing-derived** and (because the drift pass measures windows and fits
a line) is exactly the kind of value the gate exists to keep out of the stable surface.

### 2.2 How Phase 1 avoids regressing it

Two hard rules, both enforced by construction:

1. **The Rust core `couple pair` JSON does NOT gain an RMS field.** Phase 1 does not touch
   `cli/src/pair.rs` `Row` / `RowCandidate` / `ResolutionDetail`, nor the
   `MatrixCell`/`PairReport` serialization. The new RMS lives only in (a) a new,
   separately-callable FFI entry consumed by the Swift resolver, and (b) Swift-side
   non-persisted view fields. `gate6_stability` runs against the CLI binary, which never
   sees RMS, so it cannot regress.

2. **RMS never enters Swift persistence.** `QueueStore.SavedQueue.Couple`
   (`/Users/tim/code/claude_apps/couple/mac/Sources/Couple/QueueStore.swift:20-27`)
   persists only `videoKey, selectedAudioKey, pinned, forced, approved, stemOverride` — no
   candidate metrics at all. We add NO field there. RMS is recomputed on resolve, lives on
   the in-memory `RowCandidate`, and is dropped on save/restore. (Even today `z`/`lagMs`
   are not persisted per-candidate, so this is consistent with the existing contract.)

3. **RMS is excluded from any equality used for stability.** The Swift `RowCandidate`'s new
   `fitRmsMs`/`fitWindows` are optional and informational. If a stability-style assertion
   is ever added Swift-side, it must compare the persisted projection, not the live
   candidate — but no such Swift assertion exists today.

Net: the value the §7.6 gate hashes is the CLI `Row` JSON, and we keep RMS entirely out of
it. (See §6 for an optional belt-and-suspenders core test that asserts the CLI surface
still carries no RMS key.)

### 2.3 A subtlety the implementer must respect (determinism of the RMS itself)

Phase 2 *does* let RMS change a decision, so in Phase 2 the RMS path must itself be
deterministic and stability-safe:

- The new core RMS routine must be a pure function of the two pyramids + the coarse lag +
  fixed params (no clock, no track ids, no iteration over a hashmap). `drift_pass`'s window
  centers are derived deterministically from durations and the offset
  (`sync.rs:501-513`), and `measure_local` is deterministic; reuse that pattern exactly.
- Because Phase 2 can flip an `ambiguous` row to a `selected` row, the *decision* still
  obeys §5.8 (same inputs → same output) — but the new param defaults in Phase 2 must be
  chosen so the CLI gate fixtures keep their current verdicts (see §5.4). Phase 2 keeps RMS
  out of the serialized `Row` exactly as Phase 1 does; only the `state`/`selected` change,
  and only on fixtures we re-baseline deliberately.

---

## 3. Cost model and reuse (where the measurement comes from)

### 3.1 What's available today

- Stage-1 candidate matrix runs on pyramids directly, no session needed:
  `Ctx::pair_candidates` (`/Users/tim/code/claude_apps/couple/core/src/ctx.rs:235-252`)
  maps `pair_correlate(&v.pyramid, &a.pyramid, params)` in a Rayon parallel loop. Each cell
  yields a `PairCorrelation` with the refined `lag_ms` (the §5.3 stage ladder), which is
  the coarse offset we need to seed a drift fit.
- The multi-window measurement + fit machinery already exists but is **welded into the full
  sync** and is private: `drift_pass` (`/Users/tim/code/claude_apps/couple/core/src/sync.rs:474-584`)
  walks 12 window centers across the overlap, calls
  `measure::measure_local` (`/Users/tim/code/claude_apps/couple/core/src/measure.rs:130-141`)
  per window, filters `accepted` (`ncc > ncc_window_min`), and runs
  `fit::fit_ls` over the accepted `(t, lag_ms)` points.
- `measure_local` itself is `pub` and pyramid-based (no session):
  `measure_local(cam_pyramid, ext_pyramid, tc, half_win_s, center_ms, span_ms)`.

So the reusable primitive we want — "given two pyramids and a coarse lag, return a drift
fit (off, ppm, rms) over windows across the overlap" — is `drift_pass` minus the
progress/cancel/refit-iteration plumbing.

### 3.2 The cost constraint

The resolver only has `z`/`lag`/`coverage` at decision time. A per-candidate RMS needs a
12-window measure + fit per candidate (≈ the cost of one drift pass). That is real work
(each `measure_local` is budgeted ≤5 ms; 12 windows ≈ tens of ms per candidate). So:

- **RMS is computed ONLY for contested clusters** — a cluster with ≥2 candidates on one
  overlapping span that is NOT simultaneous. The common single-candidate path
  (`Resolver.swift:199-203`, `cli/src/pair.rs:463-472`) and the simultaneous `2 sources`
  path never compute it.
- Within a contested cluster, in Phase 1 we may compute it for all cluster candidates (so
  the owner sees the full separation); in Phase 2 the gate only needs best vs runner-up, so
  top-2 is sufficient (open question §7).
- Reuse the existing parallel discipline: compute the cluster's per-candidate RMS with
  Rayon, mirroring `pair_candidates`.

### 3.3 New core primitive (shared by both phases)

Add a lightweight, pure fit-RMS routine in core. Proposed home:
`/Users/tim/code/claude_apps/couple/core/src/sync.rs` (next to `drift_pass`, reusing its
private helpers) or a small new module `core/src/pairfit.rs`. Signature sketch:

```rust
/// Drift fit over windows across the [p_a, p_b] overlap at a known coarse lag.
/// Deterministic; no session, no clock. Returns None when < MIN points lock.
pub struct PairFit { pub off_ms: f64, pub ppm: f64, pub rms_ms: f64, pub windows: u32 }

pub fn pair_fit_rms(
    p_a: &PyramidReader,
    p_b: &PyramidReader,
    coarse_lag_ms: f64,
    params: &SyncParams,
) -> Option<PairFit>;
```

Implementation: lift the body of `drift_pass` (centers from `sync.rs:501-513`, the
`measure_all` first pass at `sync.rs:545`, accept filter + `fit_ls` at `sync.rs:546-555`)
into this function. Drop the high-ppm refit iteration (`sync.rs:559-576`) for the
tie-breaker — it costs a second 12-window pass and isn't needed to tell a 5 ms fit from a
20 ms one (keep it as an open question §7 if owner wants the extra fidelity). Carry
`windows = accepted_pts.len()`. To keep `drift_pass` honest, optionally refactor it to call
`pair_fit_rms` for its first pass (pure cleanup, not required).

The lag convention matches: `pair_correlate`'s `lag_ms` is "content at t in A occurs at
t + lag/1000 in B" (`core/src/pair.rs:18-19`), identical to `SyncResult.offset_ms`, which
is what `drift_pass` consumes as `offset_stage_ms`. So feeding a candidate's
`PairCorrelation.lag_ms` straight in is correct.

---

## 4. Phase 1 — full data path (MEASURE & SURFACE)

Goal: contested-cluster candidates carry a fit RMS, shown informationally; zero decision
change; stability gate untouched.

### 4.1 Core (Rust) — new measurement

File: `/Users/tim/code/claude_apps/couple/core/src/sync.rs` (or new `core/src/pairfit.rs`,
added to `core/src/lib.rs`).
- Add `PairFit` + `pair_fit_rms(p_a, p_b, coarse_lag_ms, params)` per §3.3.
- Add a batch helper on `Ctx` mirroring `pair_candidates`, so the shell asks for many
  cluster fits in one parallel call. File:
  `/Users/tim/code/claude_apps/couple/core/src/ctx.rs` (near `pair_candidates`,
  `ctx.rs:235`):

```rust
/// Per-(video, audio) drift-fit RMS for a set of contested candidate pairs.
/// `pairs`: (video_id, audio_id, coarse_lag_ms). Parallel; deterministic.
pub fn pair_fit_batch(
    &self,
    pairs: &[(TrackId, TrackId, f64)],
    params: &SyncParams,
) -> Result<Vec<Option<PairFit>>>;
```

This keeps the resolver's call count to one FFI round trip per resolve, not one per
candidate.

### 4.2 FFI (Rust C-ABI) — new entry, additive

File: `/Users/tim/code/claude_apps/couple/ffi/src/lib.rs` (alongside `cpl_pair_candidates`
at `ffi/src/lib.rs:799`). Add:

```c
// JSON in: [[video_id, audio_id, coarse_lag_ms], …]; JSON out: [PairFit|null, …].
cpl_pair_fit_batch(ctx, pairs_json) -> *mut c_char
```

Follow the existing pattern (`cpl_fit_ls_json` at `ffi/src/lib.rs:604` parses a JSON
points array; `cpl_pair_candidates` at `ffi/src/lib.rs:799` does the id-array form). JSON
in/out so no C struct/ABI bump — `cpl_abi_version` stays 1 (`ffi/src/lib.rs:842`). Default
`SyncParams` is fine (the only field that matters, `z_min`, doesn't affect the drift
windows).

The C declaration must also be added to the bridging header / modulemap that exposes
`CCouple` to Swift. Locate it via the symbols already declared for
`cpl_pair_candidates`/`cpl_fit_ls_json` (the `CCouple` module that `Engine.swift` imports)
and add the new prototype there.

### 4.3 CoupleKit (Swift Wire + Engine) — new value type + call

File: `/Users/tim/code/claude_apps/couple/mac/Sources/CoupleKit/Wire.swift`
- Add a Codable mirror (the wire is camelCase verbatim, like the existing `Fit` at
  `Wire.swift:237-242`):

```swift
public struct PairFit: Codable, Sendable {
    public var offMs: Double
    public var ppm: Double
    public var rmsMs: Double
    public var windows: UInt32
}
```

File: `/Users/tim/code/claude_apps/couple/mac/Sources/CoupleKit/Engine.swift`
- Add a wrapper next to `pairCandidates` (`Engine.swift:133`):

```swift
/// Drift-fit RMS for contested candidate pairs (Phase-1 tie-breaker telemetry).
/// `pairs`: (video, audio, coarseLagMs). Result aligns to input order; element
/// nil when too few windows locked. nil only on a core fault.
public func pairFitBatch(_ pairs: [(TrackHandle, TrackHandle, Double)]) -> [PairFit?]?
```

Encode `[[v.id, a.id, lag], …]` to JSON, call `cpl_pair_fit_batch`, decode
`[PairFit?].self`. (Use `decodeJSON`; the array elements are nullable like
`perAnchorMeasurements`.)

### 4.4 Resolver (Swift) — carry RMS on candidates in contested clusters only

File: `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Resolver.swift`

1. Extend `RowCandidate` (`Resolver.swift:77-84`) with optional, informational fields:

```swift
var fitRmsMs: Double? = nil   // Phase-1 telemetry; contested clusters only
var fitWindows: Int? = nil
```

   Default `nil` so every existing construction site (`mk` at `Resolver.swift:176`, the
   `repair` placeholder at `AppModel.swift:271`, the restore path at `AppModel.swift:456`)
   compiles unchanged and non-contested candidates simply carry `nil`.

2. Optionally mirror onto `ResolutionDetail` (`Resolver.swift:97-102`) a small summary for
   the banner, e.g. `var fitBestRmsMs: Double?; var fitSecondRmsMs: Double?` — purely so the
   ambiguous banner can show "best fit 4.2 ms vs 21.8 ms" (see §4.5). Keep it optional.

3. In `resolveRow`, in the conflict (`else { ambiguous = true }`) branch
   (`Resolver.swift:237-248`): when a cluster is contested (`!allSimultaneous`), gather the
   `(video, audio, lagMs)` for that cluster's candidates and request fits. Because
   `resolveRow` already has `engine`, call `engine.pairFitBatch(...)` here. Store each
   returned `rmsMs`/`windows` onto the corresponding `RowCandidate` when the alternates
   list is built (`Resolver.swift:253` for the ambiguous list; also the winners' alternates
   at `Resolver.swift:270` if we want RMS visible on selected rows too).

   IMPORTANT (Phase 1 = no decision change): compute and *attach* RMS, but do **not** read
   it in any `if`. The `ambiguous = true` line stays. Verdicts are byte-identical.

   Performance: gather contested pairs across the whole resolve and issue ONE
   `pairFitBatch` after the cluster loop, rather than per-cluster, to keep it a single
   parallel core call. (Restructure `resolveRow`/`resolve` so contested
   `(vi, ai, lagMs)` tuples are collected, fitted once in `resolve`
   (`Resolver.swift:138-163`), then stitched back by key.) This also keeps the off-main
   `Task.detached` in `AppModel.resolveQueue` (`AppModel.swift:140-142`) doing the heavy
   work, which it already is.

4. Note: `resolveRow` is currently `private static` and pure; adding the fit call keeps it
   pure-ish (engine is already passed). The CLI `cli/src/pair.rs` resolver is **not**
   touched in Phase 1 — the CLI byte-stable surface gains nothing, by design (§2.2).

### 4.5 Model + UI (Swift) — surface informationally

File: `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Couple.swift`
- `CoupleRow.candidates` is `[RowCandidate]` (`Couple.swift:35`, built at
  `Couple.swift:66`) and `mergeResolution` (`AppModel.swift:170-185`) already copies fresh
  candidates wholesale for unresolved couples, so the new optional fields flow through with
  no change. (For pinned/approved couples only genuinely-new alternates are appended —
  fine; their RMS is telemetry only.)

File: `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Views/CenterColumn.swift`
- `candidateRow` (`CenterColumn.swift:262-296`) renders a relative strength bar + coverage,
  with z/lag in the `.help(...)` tooltip (`CenterColumn.swift:295`). Phase-1 surfacing:
  append RMS to that tooltip, e.g.
  `String(format: "z %.1f · lag %.0f ms%@", cand.z, cand.lagMs, cand.fitRmsMs.map { String(format: " · fit %.1f ms (%d win)", $0, cand.fitWindows ?? 0) } ?? "")`.
- The ambiguous banner `reason` (`CenterColumn.swift:247-258`) can add a line when the two
  best contested candidates have fits, e.g. "fit separates them: 4.2 ms vs 21.8 ms" — this
  is exactly the validation signal the owner needs to eyeball across sessions. Keep it
  read-only.

File (optional, parity): `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/DebugDrive.swift`
- `DebugDrive` prints alternates (`DebugDrive.swift:315`). Add RMS to that line so headless
  driver runs log the separation for corpus collection.

### 4.6 Phase 1 stability handling (explicit)

- CLI `couple pair` JSON: **unchanged** → `gate6_stability` cannot regress.
- `QueueStore`: **no new field** → restore is byte-identical; RMS recomputed on resolve.
- New FFI is additive; `cpl_abi_version` unchanged.
- `RowCandidate.fitRmsMs`/`fitWindows` are non-persisted view state.

---

## 5. Phase 2 — GATE (consult RMS before falling back to ambiguous)

Goal: rescue genuinely-decidable contested rows using RMS, behind new tunable params,
without overriding a clear `z` winner and without coin-flipping.

### 5.1 New params

File: `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Resolver.swift`, struct
`PairParams` (`Resolver.swift:19-25`). Add, following the `[TUNE]`/`[SPEC]` convention:

```swift
var rmsTrustFloorMs = 9.0   // [TUNE] generous: real true-pairs ~5 ms (V2-F2/§V2.9), not 5
var rmsRatio        = 3.0   // [TUNE] runner-up must be ≥ this × best to be decisive
var minFitWindows   = 6     // [TUNE/SPEC] mirrors syncMinWindows; fewer ⇒ don't trust RMS
```

(`minFitWindows = 6` matches the existing `synced_min_windows: 6` gate at
`core/src/sync.rs:51`, so "trustworthy fit" means the same thing here as in full sync.)

If/when the CLI `couple pair` should also gate on RMS (so headless == shell — desirable for
parity but a separate decision, §7), add the mirror in
`/Users/tim/code/claude_apps/couple/cli/src/pair.rs` `PairParams`
(`cli/src/pair.rs:31-44`) and its `set()` (`cli/src/pair.rs:58-73`), plus `pair_fit_rms`
wiring in `resolve_row` (`cli/src/pair.rs:516-528`). **Phase 2's default behavior must keep
the existing CLI gates green** — see §5.4. If parity is deferred, Phase 2 changes only the
Swift resolver and the CLI gate is untouched (safest; recommended first cut).

### 5.2 The gate predicate

In the conflict branch (`Resolver.swift:237-248`), replace the bare
`else { ambiguous = true }` with an RMS escalation that runs ONLY when the existing
z-margin rule did not already decide:

```swift
} else {
    // §3.5 z-margin first (unchanged): decisive z still wins outright.
    if zSecond < params.zMin || margin >= params.zMargin {
        winners.append((cluster[0], false))
    } else if let pick = rmsEscalate(cluster, c, fits, params) {
        // Phase-2 tie-breaker: a clean end-to-end fit identifies the take.
        winners.append((pick, false))
    } else {
        ambiguous = true   // unchanged fallback (V2-F2 honesty)
    }
}
```

`rmsEscalate` (new helper) returns the index in `c` to select, or `nil` to stay
`ambiguous`. Predicate, over the cluster's best-by-z and runner-up-by-z (already
`cluster[0]`, `cluster[1]`):

```
let best   = fit[cluster[0]], second = fit[cluster[1]]
guard let rb = best?.rmsMs, let r2 = second?.rmsMs,
      (best?.windows ?? 0) >= minFitWindows,
      (second?.windows ?? 0) >= minFitWindows           // both must have locked enough windows
else { return nil }                                     // can't trust → ambiguous
guard rb <= rmsTrustFloorMs else { return nil }         // best must itself be a clean lock
guard r2 >= rb * rmsRatio   else { return nil }         // and decisively better than runner-up
return cluster[0]                                        // select the best-z candidate (whose fit is clean)
```

Notes:
- We select `cluster[0]` (best z). RMS is used to *confirm* the z-leader is the real take,
  not to re-rank. This deliberately avoids "RMS overrides a clear z winner": when z already
  decides (`margin >= zMargin`) we never reach here; when z is ambiguous we only proceed if
  the best-z candidate also has the clean fit AND the runner-up's fit is decisively worse.
  If the best-z candidate's fit is NOT the clean one (rb > floor, or r2 < rb·ratio), we stay
  `ambiguous` — we never flip to a different candidate on RMS alone (open question §7
  asks whether the owner wants the stronger "pick whichever has the clean fit" form).
- Coverage comparability guard: only escalate when `cluster[0]` and `cluster[1]` cover
  comparable spans (they already overlap by construction of the cluster; additionally
  require `cov.seconds` within, say, 20% — reuse `covs` already computed at
  `Resolver.swift:207`). A short partial-overlap fit isn't comparable to a full one.
- Never fires on the simultaneous path: that path appends to `winners` at
  `Resolver.swift:235-236` and never enters this `else`.
- Resulting state: the selected row goes through the normal winner selection
  (`Resolver.swift:258-280`) → `confident` if `coverage.fraction >= coverageFull` else
  `partial` (or `split` across disjoint clusters). No new state needed.

### 5.3 Determinism / no-coin-flip

- The predicate is a pure function of fits + params; ties that don't clear the ratio resolve
  to `ambiguous`, never to an arbitrary pick. This honors the header contract at
  `Resolver.swift:1-14` and `cli/src/pair.rs:7-11`.
- `pair_fit_rms` is deterministic (§2.3), so the decision is reproducible (§5.8).

### 5.4 Keeping existing gates green in Phase 2

If the CLI resolver gains the gate, these fixtures must be re-checked (and only
`gate8`/`margin_rule_prefers_ambiguous` are expected to be sensitive):

- `gate8_honesty_floor` (`pair_gates.rs:373-419`): synthetic same-plan takes classify as
  *simultaneous* (the comment at `pair_gates.rs:405-411` records this), so they never reach
  the conflict branch and the RMS gate never runs — gate unaffected. The acceptance is "true
  take or ambiguous, never confident-decoy"; RMS can only ever pick the true take, so it
  cannot newly fail this gate.
- `margin_rule_prefers_ambiguous` (`pair_gates.rs:427-458`): forces the conflict branch via
  `triangle-tol-ms=0.000001` + `z-margin=1000`. With Phase-2 defaults the RMS gate could now
  rescue it. Two options: (a) keep this test asserting `ambiguous` by also pinning the new
  params to never-fire values (`--params rms-trust-floor-ms=0` so `rb <= 0` is false), which
  documents that the *z*-path still yields ambiguous; or (b) re-baseline it knowing RMS now
  selects — but synthetic same-plan takes have near-identical fits, so the ratio guard will
  *not* clear and it stays `ambiguous` anyway. Verify empirically; prefer (a) to keep the
  test's intent (z-margin honesty) pure.
- Gates 1–7 don't enter the contested+non-simultaneous branch (they're confident / split /
  twoSources / noMatch), so they're unaffected.

If the CLI is NOT changed in Phase 2 (Swift-only gate), none of the Rust gates can regress;
only Swift resolver tests (new, §6) cover the gate.

---

## 6. Test & gate plan

### 6.1 Gates that could regress (name them)

- `gate6_stability` (`cli/tests/pair_gates.rs:287`): the byte-stability gate. Phase 1: not
  at risk (CLI JSON unchanged). Phase 2: at risk only if RMS is serialized into the CLI
  `Row` — which we explicitly do not do; the gate's fixture (one related external + one
  unrelated add) doesn't hit a contested cluster anyway.
- `gate8_honesty_floor` (`pair_gates.rs:373`): Phase 2 only; analysis in §5.4 — expected
  unaffected.
- `margin_rule_prefers_ambiguous` (`pair_gates.rs:427`): Phase 2 only; pin new params to
  no-fire (§5.4) so it keeps asserting the z-path ambiguity.
- `realdata_pair_confident_row` (`pair_gates.rs:463`): single confident row, no contest;
  unaffected.

### 6.2 New tests — Rust core (Phase 1 primitive)

File: `/Users/tim/code/claude_apps/couple/core/src/sync.rs` `#[cfg(test)]` (or new
`pairfit.rs` tests), and `core/src/fit.rs` already has `exact_line`/`degenerate`
(`fit.rs:49-68`) to lean on:
- `pair_fit_rms` over a clean synthetic pair (reuse `fixture::generate_session` as
  `pair_gates.rs:5` does, or two pyramids built in-test) returns `rms_ms` < a few ms and
  `windows >= 6`.
- `pair_fit_rms` over a deliberately mis-lagged / wrong-take pair returns either `None`
  (too few windows lock) or a large `rms_ms` (≫ trust floor). This is the core assertion
  that RMS separates takes — the thing synthetic fixtures *can* show even though they can't
  fake same-piece-different-take envelopes (`pair_gates.rs:369-371`).
- Determinism: two calls give bit-identical `PairFit` (mirrors §5.8).

File: `cli/tests/pair_gates.rs` — optionally a `gate9_fit_separation` that runs the
existing `gate8` decoy fixture and asserts (via a new `--json` debug field OR via a direct
core test, not via the stable `Row`) that the true take's fit RMS ≪ the decoy's. Prefer the
**core** test to avoid adding any field to the stable CLI surface.

### 6.3 New tests — Swift resolver (both phases)

There is no existing Swift resolver test target visible; add one (e.g.
`mac/Tests/CoupleTests/ResolverTests.swift`) or, if the project has no XCTest target yet,
drive via `DebugDrive`. Cases:
- Phase 1: a contested non-simultaneous cluster produces candidates whose `fitRmsMs` is
  populated, and the row state is **identical** to the pre-change baseline (assert no
  decision change). Use injected/mocked `PairFit` values so the test doesn't need real
  media (factor `rmsEscalate` to take the fits array, §5.2, so it's unit-testable without
  the engine).
- Phase 2, rescue: best-z candidate `rms 4 ms`, runner-up `rms 22 ms`, both `windows ≥ 6`,
  comparable coverage, z-margin < `zMargin` → row becomes `confident`/`partial`, selected =
  best-z. 
- Phase 2, hold ambiguous: best-z `rms 6 ms`, runner-up `rms 9 ms` (ratio 1.5 < 3) →
  stays `ambiguous`.
- Phase 2, hold ambiguous: best-z `rms 12 ms` (> floor) even if runner-up is 40 ms →
  stays `ambiguous` (best itself isn't a clean lock).
- Phase 2, guard: runner-up has `windows < minFitWindows` → stays `ambiguous` (can't
  trust).
- Phase 2, never on simultaneous: a `2 sources` cluster ignores RMS entirely.

### 6.4 Keeping the stability gate green — procedure

1. Run `cargo test -p couple-cli --release` (the gates need `--release`,
   `pair_gates.rs:2-3`) after Phase 1 and confirm `gate6_stability` and all others pass
   unchanged.
2. Confirm the `couple pair --json` output schema is byte-identical before/after Phase 1
   (diff a sample report). It must be, since no Rust resolver/report type changed.
3. Phase 2: re-run the full gate suite; for any fixture that newly hits the RMS gate,
   apply §5.4 (pin no-fire params or re-baseline with justification logged in PROGRESS.md as
   a V2-F2 calibration entry, matching the project's convention at `pair_gates.rs:116-117`).

---

## 7. Risks & open questions for the human

1. **Exact thresholds.** `rmsTrustFloorMs ≈ 9`, `rmsRatio ≈ 3`, `minFitWindows = 6` are
   starting guesses. The brief says true-pairs run ≈5 ms; the floor is deliberately
   generous (8–10) to avoid rejecting a slightly-noisy true take, and the *ratio* (not an
   absolute gap) is what makes "decisively better" scale-free. Phase 1 exists precisely to
   let you read real best-vs-second RMS pairs across sessions and set these. Decision
   needed: confirm the three numbers (or a range to A/B).

2. **All candidates vs top-K for the fit.** Phase 1 can fit every contested candidate (full
   visibility, more cost) or just top-2 by z (enough for the gate). The cost is ≈ one drift
   pass per fitted candidate, only on contested clusters. Recommendation: top-2 for the gate
   path, but compute all in Phase 1 while validating, then drop to top-2 in Phase 2.
   Decision needed: which.

3. **"Confirm the z-leader" vs "pick the clean fit".** The §5.2 predicate only *confirms*
   the best-z candidate (won't flip to a lower-z candidate on RMS alone — conservative). A
   stronger variant would select whichever candidate has the clean fit even if it's the
   z-runner-up. The strong form rescues more rows but is a bigger trust step. Recommendation:
   ship the conservative form first; revisit after Phase-1 data. Decision needed: which form.

4. **CLI parity (headless gate).** Should `couple pair` also gate on RMS so the CLI and the
   shell agree? Pro: one source of truth, gates cover it. Con: it touches the byte-stable
   surface's *decisions* (not its fields) and forces re-baselining of
   `margin_rule_prefers_ambiguous`. Recommendation: Swift-only in Phase 2's first cut; add
   CLI parity as a fast-follow once thresholds are frozen. Decision needed: now or later.

5. **UI placement.** Phase 1 puts RMS in the candidate tooltip + (optionally) the ambiguous
   banner ("fit separates them: 4.2 vs 21.8 ms"). The candidate row today deliberately hides
   raw z (bimodal) behind a strength bar (`CenterColumn.swift:260-261`). RMS is arguably
   *more* legible than z (lower = better, ms units) — should it get a visible glyph on
   contested rows rather than only a tooltip? Recommendation: tooltip + banner for Phase 1
   (low commitment); promote to a visible affordance only if it proves trustworthy. Decision
   needed: tooltip-only vs visible.

6. **Cost ceiling.** Gate 7 budgets the 6×6 matrix ≤ 5 s (`pair_gates.rs:330-358`). Fit RMS
   runs only on contested clusters, so a typical session adds little, but a pathological
   session with many same-piece takes could add tens of ms × candidates. If that matters,
   cap contested-cluster fitting (e.g. top-3) and/or reuse the single-pass (no refit)
   `pair_fit_rms`. Open: do we want an explicit budget/cap?

---

## 8. File-by-file change list (for implementation agents)

### Phase 1 (observe-only; independently shippable)

Core (Rust):
- `/Users/tim/code/claude_apps/couple/core/src/sync.rs` — add `PairFit` + `pair_fit_rms`
  (lift `drift_pass` first-pass body, §3.3); optional refactor of `drift_pass` to reuse it.
  Add `#[cfg(test)]` cases (§6.2).
- `/Users/tim/code/claude_apps/couple/core/src/lib.rs` — register a new module if
  `pairfit.rs` is used instead of `sync.rs`.
- `/Users/tim/code/claude_apps/couple/core/src/ctx.rs` — add `pair_fit_batch` (§4.1),
  mirroring `pair_candidates` (`ctx.rs:235`).

FFI:
- `/Users/tim/code/claude_apps/couple/ffi/src/lib.rs` — add `cpl_pair_fit_batch`
  (JSON-in/JSON-out, additive; §4.2), next to `cpl_pair_candidates` (`ffi/src/lib.rs:799`).
- The `CCouple` bridging header / modulemap (the one declaring `cpl_pair_candidates`) — add
  the new prototype.

CoupleKit (Swift):
- `/Users/tim/code/claude_apps/couple/mac/Sources/CoupleKit/Wire.swift` — add `PairFit`
  Codable (§4.3).
- `/Users/tim/code/claude_apps/couple/mac/Sources/CoupleKit/Engine.swift` — add
  `pairFitBatch` (§4.3).

App (Swift):
- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Resolver.swift` — add optional
  `fitRmsMs`/`fitWindows` to `RowCandidate` (`Resolver.swift:77`); optional summary on
  `ResolutionDetail`; in `resolve`/`resolveRow` collect contested `(vi, ai, lagMs)`, call
  `engine.pairFitBatch` once, attach RMS to candidates — **no `if` reads RMS** (§4.4).
- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Views/CenterColumn.swift` — append
  RMS to the candidate `.help` tooltip (`CenterColumn.swift:295`) and optionally the
  ambiguous banner `reason` (`CenterColumn.swift:247`).
- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/DebugDrive.swift` (optional) —
  log RMS on the alternates line (`DebugDrive.swift:315`).
- No change to `QueueStore.swift`, `cli/src/pair.rs`, or any `couple pair` JSON type.

Tests:
- Core fit tests (§6.2); Swift resolver "no decision change + RMS populated" test (§6.3).

### Phase 2 (gate; independently reviewable, builds on Phase 1)

App (Swift):
- `/Users/tim/code/claude_apps/couple/mac/Sources/Couple/Resolver.swift` — add
  `rmsTrustFloorMs`/`rmsRatio`/`minFitWindows` to `PairParams` (`Resolver.swift:19`); add
  `rmsEscalate` helper; wire it into the conflict branch (`Resolver.swift:237-248`) per
  §5.2; add the coverage-comparability guard using `covs`.
- Swift resolver gate tests (§6.3: rescue / hold-ambiguous / guard / never-simultaneous).

CLI parity (OPTIONAL — only if §7.4 says "now"):
- `/Users/tim/code/claude_apps/couple/cli/src/pair.rs` — mirror params in `PairParams`
  (`cli/src/pair.rs:31`) + `set()` (`cli/src/pair.rs:58`); add `pair_fit_rms` call and the
  gate in `resolve_row` (`cli/src/pair.rs:516-528`); ensure RMS stays OUT of the serialized
  `Row`.
- `/Users/tim/code/claude_apps/couple/cli/tests/pair_gates.rs` — pin no-fire params in
  `margin_rule_prefers_ambiguous` (or re-baseline, §5.4); confirm `gate6_stability`,
  `gate8_honesty_floor` green.

Docs:
- `PROGRESS.md` — log thresholds chosen + any fixture re-baseline as a V2-F2 calibration
  entry (project convention).

---

## 9. Reviewer addendum (verified + refinements)

Reviewed 2026-06-13. Independently re-read the three load-bearing sites; all confirmed:
- `gate6_stability` (`cli/tests/pair_gates.rs:321-324`) serializes each `Row` to JSON and
  asserts byte-equality — Phase-1's "keep RMS out of the CLI `Row`" makes it untouchable. ✓
- gate8's synthetic same-plan takes are classified **simultaneous**
  (`pair_gates.rs:405-411`), so they never reach the conflict branch — Phase-2's RMS gate
  cannot regress the honesty floor. `margin_rule_prefers_ambiguous` is the only
  Phase-2-sensitive gate, and its two externals are the *same plan* → near-identical fits →
  the ratio guard won't clear, so it stays `ambiguous` even if the gate runs. ✓
- `drift_pass` (`core/src/sync.rs:474-555`) seeds from `offset_stage_ms` == candidate
  `lag_ms` (same convention, `pair.rs:18-19`); lifting [centers + first-pass `measure_all`
  + accept-filter + `fit_ls`] into a pure `pair_fit_rms` is faithful. ✓

Refinements to fold into implementation:

- **R1 — Memoize fits.** Resolves run on every edit; a persistent contested cluster would
  refit each resolve. `pair_fit_rms` is pure/deterministic → cache by
  `(videoKey, audioKey, round(lagMs))`, invalidate on pool change. Bounds cost under rapid
  editing, not just per single resolve.
- **R2 — Verify candidate equality isn't decision-gated** before adding fields. If
  `RowCandidate`/`CoupleRow` are `Equatable`/`Hashable` and used to diff-gate UI refreshes
  or dedupe (Set/`==`), new optional fields shift equality. Confirm `mergeResolution` and
  any `==`/Set use treat them as value-carried-through. Phase-1 "no behavior change" must
  include no spurious or *blocked* UI refresh.
- **R3 — Testing convention.** Prefer the existing `DebugDrive.check(...)` pattern
  (`DebugDrive.swift:526`) over standing up a new XCTest target (none exists). Factor
  `rmsEscalate(fits:params:) -> Int?` as a pure free function so the predicate unit-tests
  without media/engine: rescue / hold-on-thin-ratio / hold-on-floor / window-guard /
  never-simultaneous.
- **R4 — Single-pass RMS is a *conservative* estimate** for genuinely high-drift true
  takes (the dropped refit at `sync.rs:557-576` is what recovers far-edge windows under
  real drift). Acceptable — it only ever fails safe (stays `ambiguous`) — but keep the
  refit reachable behind a param if Phase-1 data shows true takes hovering at the
  window-count floor.
- **R5 — Make conservative↔strong a one-line switch** in `rmsEscalate` (see below).

On open-Q3 (the real behavioral call): the **conservative** form only rescues when the
correct take *also* won the z coin-flip — but the premise is that z can't separate
same-piece takes, so which one edges z is near-random. Expect conservative to rescue only
~half the reported cases. The **strong** form (pick the single candidate below the trust
floor when exactly one is, decisively better than *all* others by ratio, enough windows,
comparable coverage) matches the owner's data ("correct <5 ms, others 20+") and the thesis
that RMS is the better identity signal; its strict guards keep it safe. Recommendation:
validate with Phase 1, target the **strong** form for Phase 2.
