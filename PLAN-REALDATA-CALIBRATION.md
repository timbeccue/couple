# PLAN-REALDATA-CALIBRATION — re-freezing the [TUNE] verdict lines on the owner's real sessions

> **STATUS: PROPOSAL — AWAITING OWNER DATA + OWNER SIGN-OFF.**
> Nothing in this document is enacted. No threshold, gate line, or verdict
> constant has been changed. The SPEC-CHANGE entries in §6 are DRAFTS and MUST
> NOT be appended to PROGRESS.md until Tim has run the sweep, reviewed the
> numbers, and signed off (SPEC §0 process). What HAS shipped alongside this
> document is measurement-only: fail-loud realdata test skips and
> `scripts/realdata-sweep.sh`.

## 1. Problem (finding F4, 2026-07-11)

The v2 verdict thresholds ship `[TUNE]` (SPEC-V2 §V2.9 / flag V2-F2): they were
derived from v1 synthetic/single-pair evidence, while real true-pairs are
reported to run ~5 ms fit RMS / ~90 ppm drift and still be *correct* — v1-derived
lines would flag them, so the TO APPROVE / TO FIX sort is uncalibrated. At the
same time the real-data safety net was dormant: the SPEC §9.4 frozen-pair
regression and §9.5 hand-sync gate only run when `COUPLE_REALDATA_DIR` is set,
and until this change the suites skipped SILENTLY GREEN without it.
Different-take false matches (SPEC-V2 §3.5) — the feature's realistic hazard —
are uncharacterized on real material.

## 2. What ships now (measurement only)

1. **Fail-loud skips.** The five realdata tests are `#[ignore]`d with the reason
   string `SKIPPED (no realdata): …`, so every default
   `cargo test --release --workspace` prints a visible per-test marker instead
   of a lying green. Run explicitly without a valid dir, they PANIC
   ("REALDATA MISSING").
2. **One command for Tim:**

       COUPLE_REALDATA_DIR=/path/to/your/sessions scripts/realdata-sweep.sh

   Layout expectation: `COUPLE_REALDATA_DIR` is a parent folder; every
   directory under it (depth ≤ 3) that directly holds ≥ 2 media files is
   treated as one session. If the canonical schumann pair
   (`schumann-video.mp4` + `schumann-audio.wav`, plus `peaks/*.json`) sits in
   the root, the frozen §9.4 regression suites run too. Re-runs are warm
   (default cache). `scripts/realdata-sweep.sh --self-check` proves the tool on
   a synthesized session without any real data.

## 3. What comes back

`realdata-report.md` (+ `.json`, both gitignored at the repo root):
- §1 sessions scanned and row-state counts;
- §2 per-couple sync measurements (offset/drift/fit RMS/windows/fine NCC) and
  true-couple distributions (min/median/p95/max) against each current line,
  with "N/M correct couples would fail this line" counts;
- §3 identity evidence: selected vs non-selected spectral cells, legacy
  z-margin distribution vs Z_MARGIN, and an explicit red list of non-selected
  cells that classify `match` — the §3.5 different-take hazard, each to be
  verified by ear;
- §4 coverage vs COVERAGE_FULL and triangle residuals vs TRIANGLE_TOL_MS;
- §5 frozen-suite pass/fail; §6 hand-sync material presence; §7 pointer here.

## 4. How the thresholds would then be set (mirrors CALIBRATION.md §6–7)

The methodology is the one that froze fingerprint profile 2
(`fixtures/matcher-corpus/CALIBRATION.md` §6–7); it transfers line-for-line:

1. **Partition first, tune second.** Split the sessions: a *calibration* split
   (tune here), the schumann frozen pair as *validation* (measure, never tune),
   and ≥ 25 % of sessions RESERVED UNSEEN as a *fresh holdout* — never paired,
   scored, or read while choosing values. Any split read while diagnosing is
   contaminated and cannot gate (the 2022orch lesson, §6.5b/§6.6).
2. **Two-sided margins, negative-exclusion + true-pair floor.** Each new value
   must sit clearly above the worst measured negative AND clearly below the
   weakest measured true pair (profile 2 example: count 500 ≈ 4.4× worst
   negative, ≈ 3.4× under weakest true pair). For the sync-verdict lines:
   `synced_max_rms_ms` from the true-couple fit-RMS p95 with headroom vs any
   wrong-take RMS observed; `Z_MARGIN` from the separation between selected and
   runner-up scores on multi-take sessions; `COVERAGE_FULL` from measured
   true-couple coverage.
3. **Principle-derived where data is silent.** If the corpus lacks a regime
   (e.g. no same-piece different-take conflicts appear), the value stays
   conservative and the gap is RECORDED, not guessed (§6.3 "calibration
   discipline"; V2-F2's `ambiguous`-over-coin-flip posture stands).
4. **Freeze + value-lock.** New values land as a versioned freeze with a
   value-lock test (the `profile_v2_value_lock` pattern) and per-session frozen
   expectations following the §9.4 F2 freeze procedure (measure → owner
   verifies by ear → freeze ± tolerance).
5. **Score the holdout ONCE.** The fresh-holdout run is the release gate; 0
   confident-false remains the floor. Determinism: two runs must produce
   byte-identical report JSON for the same inputs.

## 5. The lines under review (READ-ONLY inventory; none changed)

| line | value today | where |
|---|---|---|
| `synced_max_rms_ms` | 1.0 | `core/src/sync.rs` `SyncParams::default()` [TUNE] |
| `synced_fine_ncc` | 0.5 | same [TUNE] |
| `synced_min_windows` | 6 | same |
| `suggest_resample_ms` | 2.0 | same [DECIDED form, TUNE value] |
| `z_min` | 8.5 | `core/src/sync.rs` + `core/src/resolve.rs` [VALIDATED] |
| `z_margin` (Z_MARGIN) | 6.0 | `core/src/resolve.rs` `ResolveParams::default()` [TUNE, V2-F2] |
| `triangle_tol_ms` | 250.0 | same [SPEC, TUNE] |
| `coverage_full` | 0.98 | same [SPEC, TUNE] |
| `rms_trust_floor_ms` / `rms_ratio` | 9.0 / 3.0 | same (legacy tie-breaker) |
| fingerprint profile 2 | (frozen) | `core/src/fingerprint/params.rs` — NOT in scope here; owns its own CALIBRATION.md process |

Owner anecdote to be CONFIRMED by the sweep (unverified until measured): real
true-pairs ~5 ms RMS / ~90 ppm drift — which would fail `synced_max_rms_ms=1.0`
and is the concrete reason this calibration exists (SPEC-V2 §11, V2-F2).

## 6. DRAFTED SPEC-CHANGE entries — NOT ENACTED, DO NOT APPEND

> Each entry below is a template. `<…>` values come from realdata-report.md.
> Only after Tim signs off does a session copy the filled-in entries to
> PROGRESS.md and change the constants (with value-lock tests + shell-mirror
> checks in the same commit).

    SPEC-CHANGE (DRAFT): §5.6/Appendix A sync-verdict lines re-frozen on the
    owner corpus (realdata-report.md <DATE>, N=<couples>): synced_max_rms_ms
    1.0 → <value; derivation: true-couple RMS p95 <p95> with <headroom>×
    margin, worst observed wrong-take RMS <value>>; synced_fine_ncc 0.5 →
    <value or unchanged>; suggest_resample_ms 2.0 → <value or unchanged>.
    Two-sided margins per CALIBRATION.md §6.3 discipline. Owner signed off
    <DATE>. Value-locked in core/src/sync.rs tests.

    SPEC-CHANGE (DRAFT): SPEC-V2 §9 pairing constants re-frozen on the first
    real multi-take corpus (V2-F2 closure): Z_MARGIN 6.0 → <value; measured
    selected-vs-runner-up separation min <v>, worst same-piece different-take
    z <v>>; COVERAGE_FULL 0.98 → <value>; TRIANGLE_TOL_MS 250 → <value or
    unchanged, residual p95 <v> ms>. Different-take hazard characterized:
    <K> cross-cell match classifications over <M> cells, disposition noted.
    Owner signed off <DATE>.

    SPEC-CHANGE (DRAFT): §9.4 frozen real-pair regression extended: sessions
    <list> frozen per the F2 procedure (measured offset/drift written to
    <session>/expected.json, ownerVerified=true after ear check, asserted at
    ±<tol> ms thereafter). Owner signed off <DATE>.

## 7. §9.5 hand-sync gate activation (when material exists)

Protocol unchanged from SPEC §9.5: Tim drops hand-synced exports under
`<COUPLE_REALDATA_DIR>/handsync/<name>/`; the sweep reports their presence
(report §6). Activation = a follow-up task wiring the §9.5 measure_local
median |Δ| ≤ 10 ms check into the sweep. Not automated here because no
material exists to validate against.

## 8. Owner sign-off checklist

1. Run the one command (§2). 2. Confirm every couple in report §2 is actually
   the right pairing (by ear where in doubt) — the distributions are only
   meaningful over verified-true couples. 3. Review report §3's red list, if
   any. 4. Approve/adjust the derived values per §4. 5. A session then enacts
   §6 (constants + value-locks + PROGRESS entries + doc updates) — never
   before this step.
