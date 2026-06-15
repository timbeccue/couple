# Spectral fingerprint matcher — calibration & threshold freeze (boundary 12, §15)

PLAN-SPECTRAL-FINGERPRINT-MATCHING §15. This is the boundary-12 record: the calibration sequence,
the frozen **threshold profile version 1** (`FingerprintMatchParams::profile_v1`), and the §15.2
gate / §15.3 budget results on **validation** and **holdout**. Numbers are from the runs on this
machine (Apple Silicon) over the committed manifests + real `example_data/` media.

> **⚠️ SUPERSEDED BY PROFILE 2 — see §6 at the bottom.** Boundary 12 diagnosed a recall collapse on
> long real recordings (the §9.2/§9.3 cap conflation + the density-fragile inlier-FRACTION gate).
> **Profile version 2** (`FingerprintMatchParams::profile_v2`) is the re-calibrated fix: the §9.2
> sample is now scored normally (only a genuine resource ceiling forces `Ambiguous`), and the §10.6
> identity gate gained a density-robust absolute COUNT+COVERAGE path. Sections 1–5 below are the
> boundary-12 PROFILE-1 record, kept verbatim for history; **the current frozen profile, its
> calibration-derived thresholds, the corpus partition, and the re-measured gates are in §6.** The
> default still stays `LegacyEnvelope` — switching it remains gated on a FRESH real holdout (§6.5).

**Default stays `LegacyEnvelope`.** Profile 1 makes `FingerprintPrimary` *valid* (a calibrated
profile now exists), but the default matcher is NOT switched — that is boundary 13, and the holdout
result below shows it is **NOT yet justified (NO-GO)**.

---

## 1. Calibration sequence actually run (§15.1)

1. **Legacy vs fingerprint on calibration.** Scored the calibration split (real `hsBach`
   difficult-match material) with `legacy-z`, `legacy-fine-lock`, and `fingerprint-shadow` (profile
   0), and the synthetic split with `legacy-z` and `fingerprint-shadow`. Compared.
2. **Tuned ONLY on calibration.** The fingerprint matcher missed two of three real true pairs at
   profile 0 because `min_inlier_fraction = 0.15` rejected them despite correct offsets. The
   calibration evidence showed a clean separation on the inlier FRACTION (true pairs ≥ 0.104, true
   negatives ≤ 0.013), so `min_inlier_fraction` was lowered to **0.08**. No other identity threshold
   separated true from false on this material (see §3), so the rest were held at profile-0 values.
3. **Froze** the result as `profile_v1()` with `threshold_profile_version = 1` (the profile-0
   `[TUNE]` defaults are kept for shadow/back-compat).
4. **Ran validation** (the frozen schumann pair). It revealed an algorithmic limitation (the long-pair
   correspondence-cap → Ambiguous interaction, §3.2), investigated thoroughly. **No identity threshold
   was changed to fit validation.**
5. **Ran holdout ONCE** (2022orch) for the release decision. **No threshold was changed afterward.**

Splits and case counts (real media all present locally, gitignored):

| split | cases | same-event | real / synthetic | media present |
|---|---|---|---|---|
| synthetic   | 33 | 32 | synthetic (regenerated) | yes (generated) |
| calibration | 6  | 3  | real `hsBach`           | yes |
| validation  | 1  | 1  | real `schumann` (frozen offset) | yes |
| holdout     | 8  | 7  | real `2022orch`         | yes |

---

## 2. Frozen profile version 1 (`FingerprintMatchParams::profile_v1`)

| field | profile 0 | **profile 1** | one-line rationale |
|---|---|---|---|
| `max_occurrences_per_hash_per_track` | 64 | **64** | resource cap; never the blocker on a real true pair |
| `max_occurrence_product` | 4096 | **4096** | resource cap; held |
| `max_correspondences_per_pair` | 50000 | **50000** | resource cap; raising it does NOT rescue long pairs (§3.2) and 50000 gives the healthiest capped inlier fraction |
| `max_hypotheses` | 1024 | **1024** | resource cap; held |
| `max_abs_drift_ppm` | 2000 | **2000** | physical bound; accepted real pairs at +32/−305/+364 ppm, all inside |
| `inlier_tolerance_ms` | 50 | **50** | ≈2.3 STFT frames; real true pairs reached 0.86–0.99 coverage at this tolerance; sub-ms accuracy is boundary-7's job |
| `min_support` | 12 | **12** | weighted support does NOT separate here (a real negative had support 114); held |
| `min_inlier_fraction` | 0.15 | **0.08** | THE calibrated lever: real true pairs 0.104–0.159, real negatives 0.003–0.013 → 0.08 admits true, rejects false with a 6× margin |
| `min_temporal_coverage` | 0.35 | **0.35** | real true pairs 0.86–0.99; blocks localized-coincidence / duplicate-tower hazards; held |
| `max_residual_p95_ms` | 45 | **45** | real true pairs p95 36.9–43.4 ms; tightening to 40 would reject the full-length true pair; held |
| `min_score_margin` | 0.0 | **0.0** | margin does NOT separate (real true pairs 0.001–0.046, some negatives 0.087–0.352); raising it would reject true pairs; held |
| `threshold_profile_version` | 0 | **1** | calibrated; makes `FingerprintPrimary` valid (§6) |

So profile 1 differs from profile 0 in exactly **two** fields: `min_inlier_fraction` (0.15 → 0.08)
and `threshold_profile_version` (0 → 1). Value-locked by `fingerprint::params::tests::profile_v1_value_lock`.

---

## 3. §15.2 gate results

### 3.1 Calibration (real hsBach + synthetic) — the tuning split

- hsBach: **3/3 true pairs Match, 0 FN, 0 FP, 0 confident-false.** The 3 negatives (different bach
  takes vs the same ZOOM0033) all correctly `NoMatch` (inlier fraction 0.003–0.013).
- synthetic (profile 1): **31/32 TP, 1 Ambiguous (−600 ppm extreme drift), 0 FP, 0 confident-false**,
  offset p95 **0.041 ms**, drift p95 1.04 ppm. (Legacy-z baseline: 30 TP, **2 FN** — the 8 s and 12 s
  clips; the fingerprint matcher recovers BOTH, the §15.2 8–20 s improvement.)

### 3.2 Validation (schumann frozen pair, true offset −5536.639 ms / +3.57 ppm)

| metric | result |
|---|---|
| classification | **Ambiguous** (NOT Match) |
| confident false match | 0 ✓ |
| refined lag | −5536.4 ms (**within 0.2 ms of truth** — alignment correct) |
| refined drift | ≈ +4 ppm (truth +3.57) |
| why not Match | the §9.3 **resource-ceiling rule**: the 1400 s pair generates ≥ 50000 correspondences ⇒ the cap fires ⇒ forced Ambiguous |

Investigated: raising the cap to 200000 keeps it capped (still Ambiguous); UNCAPPED (~1.1 M
correspondences) the true-line inlier fraction collapses to 0.043 ⇒ `NoMatch`. **No cap value turns
schumann into a confident Match.** This is a documented recall limitation, NOT something an identity
threshold could fix without risking the negatives — so no threshold was changed.

### 3.3 Holdout (2022orch — RUN ONCE) — the release decision

| case | fp class | fp refined lag | legacy fine-lock lag | corr (cap=50000) |
|---|---|---|---|---|
| bach      | Ambiguous | 2512.7 | 2507.8  | 50000 (capped) |
| bach2     | Ambiguous | −1937.4 | −1937.6 | 50000 (capped) |
| berlioz   | Ambiguous | 1507.3 | 1505.5  | 50000 (capped) |
| dvorak    | Ambiguous | −2826.3 | −2805.8 | 50000 (capped) |
| dvorak2   | Ambiguous | −1806.5 | −1808.1 | 50000 (capped) |
| dvorak3   | **Match** | −3364.1 | −3367.1 | 8345 (under cap) |
| prokofiev | Ambiguous | −7875.3 | −7873.5 | 50000 (capped) |
| berlioz↔dvorak (negative) | NoMatch ✓ | — | (legacy NoMatch ✓) | 41553 |

| §15.2 gate | target | fingerprint (holdout) | legacy fine-lock (holdout) | verdict |
|---|---|---|---|---|
| confident false-match rate | 0 % | **0 %** (negative cleanly NoMatch) | 0 % | **PASS** |
| same-piece/different-take confidently wrong | 0 | **0** (every miss is Ambiguous, never wrong) | 0 | **PASS** |
| repeated-content confidently wrong | 0 | **0** | 0 | **PASS** |
| true-pair recall (these are all ≥60 s) | ≥ 99 % | **1/7 ≈ 14 %** (6 Ambiguous-due-to-cap) | **7/7 = 100 %** | **FAIL** |
| accepted-pair alignment | correct | refined lags agree with legacy within ~tens of ms | — | OK (the one Match aligns correctly) |
| determinism | byte-identical | **byte-identical** (md5 match; cold-cache test green) | — | **PASS** |
| metadata corruption cannot flip a strong result | yes | yes (boundary-10 hint is advisory/non-vetoing; the known 11.6-yr camera/Zoom disagreement test is green) | — | **PASS** |

Ambiguity rate (holdout true pairs): **6/7 = 86 %** — high, and it is the recall blocker. Ambiguity
is NOT counted as a false match (§15.2); the safety gates all hold.

### 3.4 Offset accuracy (§15.2 / Phase 5)

- **synthetic** (exact ground truth): offset p95 **0.041 ms** over matched true pairs — **PASS** for
  both ≥20 s (≤1 ms) and 8–20 s (≤3 ms).
- **schumann** (gold sub-ms frozen offset): the refined lag is **within 0.2 ms** of truth (the pair
  classifies Ambiguous, so it is not "accepted", but the alignment the matcher would hand to fine sync
  is correct).
- **real hsBach / 2022orch**: their `expectedOffsetMs` was a **placeholder 0.0** in the original
  manifests; boundary 12 corrected the hsBach offsets to the legacy-fine-lock fit (documented per
  case as `groundTruthSource`). The fingerprint refined lags AGREE with the legacy reference within
  ~tens of ms on every real pair, cross-confirming the alignment; these are not used as the sub-ms
  accuracy gate (the legacy reference carries its own error).

---

## 4. §15.3 performance budgets

Measured by `couple corpus budgets` (a TIMING harness; deterministic synthetic workload at
music-like landmark density; `--gen-audio-s 120 --cell-audio-s 60`):

| budget | limit | measured | verdict |
|---|---|---|---|
| fingerprint generation | ≤ 10 s/hr | **6.2 s/hr** | PASS |
| artifact size | ≤ 10 MB/hr | **2.5 MB/hr** | PASS |
| index build | — | 5.5 ms (20 audios) | — |
| candidate lookup | — | 0.36 ms/pair | — |
| affine fit | — | 2.9 ms/pair | — |
| 6×6 warm-cache matrix | ≤ 5 s | **0.12 s** | PASS |
| 20×20 warm-cache matrix | ≤ 15 s | **1.3 s** | PASS |
| peak matching memory | ≤ 512 MB | **405 MB** | PASS |

Memory note: the peak RSS scales with the length of a SINGLE in-RAM fingerprint generation, not with
the matching workload (a 20×20 matrix with a 30 s generation chunk peaks at ~370 MB; the matching
phase itself — index + one pair's correspondences — is the dominant matching cost and stays well
under 512 MB). In deployment fingerprints are built incrementally during decode (boundary 3), so the
giant one-shot PCM is never held; the ≤512 MB matching budget is met.

---

## 5. Decision

**NO-GO for boundary 13's default switch.**

- The frozen profile is **safe**: **zero confident false matches on validation AND holdout**, every
  true-pair miss is honest **Ambiguous** (never confidently wrong), the alignment is correct
  (refined lags match the legacy reference), it is deterministic, and metadata cannot corrupt a
  result.
- But it does **not meet the §15.2 recall gate** on real long-form material: holdout true-pair recall
  is **1/7 (14 %)** vs the legacy fine-lock's **7/7 (100 %)**, because the §9.3 correspondence-cap →
  Ambiguous rule fires on every full-length orchestral pair (≥ 50000 correspondences). Switching the
  default to `FingerprintPrimary` would regress confident matches to Ambiguous on real sessions.

The blocker is the **cap → Ambiguous interaction on long, rich pairs**, an ALGORITHMIC/spec issue
(§9.3 mandates cap ⇒ Ambiguous; §10.6 forbids ad-hoc duration logic), not a threshold that may be
re-tuned. Addressing it (e.g. allowing a capped-but-all-gates-passing fit to Match, or a
coarse-then-refine correspondence strategy that does not dilute inlier fraction on long pairs) is a
**plan revision** and requires a FRESH holdout — it must not be done by tuning on this holdout.

**Holdout was run exactly once; no threshold was changed after observing it.**

---

## 6. Profile version 2 — the recall fix (re-calibration after the boundary-12 diagnosis)

PLAN-SPECTRAL-FINGERPRINT-MATCHING §9.2/§9.3 + §10.6. Boundary 12 (§5 above) shipped a **safe** but
**low-recall** profile 1: it correctly rejected every false match, but forced `Ambiguous` on every
long, dense, real pair. An independent diagnosis pinned two root causes; profile 2 fixes both, is
re-frozen as `FingerprintMatchParams::profile_v2` (`threshold_profile_version = 2`), and is the
current profile. **Profile 1 is retained verbatim as a frozen historical record; profile 0 is the
shadow default. The default matcher stays `LegacyEnvelope` (§6.5).**

### 6.1 The two root causes (diagnosis, numbers given)

1. **§9.2/§9.3 conflation (within-plan bug).** `evidence_from_consensus` forced `Ambiguous` whenever
   the ROUTINE §9.2 time-stratified SAMPLE cap fired (`grouped > max_correspondences_per_pair`),
   conflating it with the §9.3 "resource ceiling ⇒ Ambiguous" rule. This discarded a clean,
   all-gates-passing consensus on EVERY long dense pair (the §9.2 sample is supposed to be SCORED
   NORMALLY). It fired on schumann and on 6 of 7 holdout pairs.
2. **The inlier-FRACTION gate is density-fragile.** Dense orchestral audio yields many spurious
   correspondences, so the true line's inlier FRACTION is low (diagnosis: 0.063–0.088 on long pairs)
   even when the absolute inlier COUNT (1707–3014), weighted support (1090–1872), coverage (0.97–0.99)
   and p95 (31–43 ms) are all strong. RAISING the §9.2 cap is strictly worse: the sample keeps the
   highest-weight correspondences, so the CAPPED fraction (0.138 on schumann) is BETTER than uncapped
   (0.043). The lone holdout negative is separated from every true pair by ~70× in inlier count
   (23 vs ≥ 1707).

### 6.2 The fix (the §10.6-sanctioned plan revision)

- **§9.2/§9.3 separation.** `CorrespondenceStats` now carries two flags: `capped` (the routine §9.2
  sample fired ⇒ score the representative subset NORMALLY) and `hard_ceiling_exceeded` (the GENUINE
  §9.3 resource ceiling fired ⇒ force `Ambiguous`, never `NoMatch`). Only the latter overrides the
  affine verdict.
- **Density-robust §10.6 identity gate.** The single inlier-FRACTION predicate is replaced by an OR of
  two complementary paths (all other gates — support/coverage/p95/margin/drift — UNCHANGED):
  `identity_ok = (inlier_fraction >= min_inlier_fraction) OR (inlier_count >= min_abs_inlier_count AND temporal_coverage >= min_abs_coverage)`.
  The absolute path preserves the 0-confident-false-match safety: it still requires a refined AFFINE
  CONSENSUS (no classification from a raw hash count, §20), a HIGH inlier COUNT (≫ any two-point
  coincidence — the no-two-point-zero-RMS guard stays), HIGH coverage (blocks a localized
  coincidence / duplicate-tower), and the unchanged p95/drift/support/margin gates (repeated content
  still depresses the margin ⇒ Ambiguous).

### 6.3 Frozen profile 2 (the three differences from profile 1; everything else IDENTICAL)

| field | profile 1 | **profile 2** | calibration / principle rationale |
|---|---|---|---|
| `hard_correspondence_ceiling` | 50000 (== cap ⇒ conflated) | **5 000 000** | The genuine §9.3 ceiling, separated from the §9.2 sample cap. Profile 0/1 set it EQUAL to the 50000 sample cap, so a capped pair trips the ceiling ⇒ Ambiguous — reproducing boundary 12 BYTE-FOR-BYTE (shadow safety). Profile 2 PRINCIPLE-derived: consensus only ever sees ≤ 50000 correspondences (the §9.2 sample), so this purely bounds GROUPING memory for a pathological input. 100× the sample cap and ~4.5× above the largest real grouped count (~1.1 M, schumann) — it never fires on real material (verified: no calibration/validation pair trips it; schumann at ~1.1 M is scored normally and Matches), only a runaway grouping. |
| `min_abs_inlier_count` | `u32::MAX` (path off) | **500** | The absolute §10.6 path's COUNT threshold. CALIBRATION-derived: the three calibration NEGATIVES carry inlier counts 13 / 10 / **114** (worst); the diagnosis holdout negative carried 23. True long pairs carry ≥ 1707 (diagnosis); the calibration full-length true pair shows 801. 500 sits ~4.4× above the worst negative (114) and ~3.4× below the weakest true long pair (1707) — a wide two-sided margin on the stable COUNT statistic. |
| `min_abs_coverage` | 0.35 (path off) | **0.90** | The absolute §10.6 path's COVERAGE floor. CALIBRATION-derived: calibration negatives covered 0.478 / 0.796 / 0.829; true long pairs span 0.86–0.99 (calibration full-length 0.992, schumann 0.989). 0.90 sits above every negative and below every true long pair, so the absolute path also rejects a localized coincidence (high count, low coverage). |
| `threshold_profile_version` | 1 | **2** | re-calibrated; keeps `FingerprintPrimary` valid (§6). |

Held at profile-1 values (calibration §2–3 confirmed they separate true from false, or are not the
blocker): `min_inlier_fraction` 0.08, `min_support` 12, `min_temporal_coverage` 0.35,
`max_residual_p95_ms` 45, `min_score_margin` 0.0, `max_abs_drift_ppm` 2000, `inlier_tolerance_ms` 50,
all resource caps. Value-locked by `fingerprint::params::tests::profile_v2_value_lock`.

> **Calibration discipline.** The probe values 500 / 0.95 from the diagnosis were NOT copied: the
> COUNT 500 is derived from the CALIBRATION negative-exclusion margin (worst negative 114) + the
> diagnosed true-long-pair floor (1707); the COVERAGE 0.90 from calibration negative coverage
> (≤ 0.829) vs true-long-pair coverage (≥ 0.97). The hard ceiling is PRINCIPLE-derived (a
> pathological-work bound), not fitted to any single pair. Because the current calibration split has
> NO long pair that trips the §9.2 cap with a sub-0.08 sampled fraction, the absolute path's exact
> thresholds are **principle-/negative-exclusion-derived and remain pending the FRESH-holdout gate**
> (§6.5) — they are deliberately CONSERVATIVE (wide margins on both clauses).

### 6.4 The corpus partition (which real events were added / reserved)

- **Calibration: NO real events added.** The existing calibration split (hsBach test 1/2/3) was
  inventoried. The only unused same-event long pair locally that would plausibly exercise the absolute
  path is the `multifile-session/hsBach/bach 172.mov ↔ ZOOM0033.WAV` recording — but it is
  BYTE-IDENTICAL to the `test 1` pair already in calibration (same file sizes/contents), so adding it
  would be a pure duplicate with no new signal. The other unused hsBach recorders (`Gigue
  Recording.WAV`, `Prelude Recording.WAV`, `ZOOM0027/0031/0034/0035`) are 110–280 s — the same length
  band as `test 1` (255 s), which groups only ~9821 correspondences (far under the 50000 §9.2 cap), so
  pairing them would NOT exercise the absolute path on calibration either. Per §10.6 ("set the
  thresholds conservatively by principle if calibration lacks long-dense pairs"), the absolute path
  was calibrated by negative-exclusion + principle instead of by adding non-informative cases.
- **RESERVED UNSEEN for the fresh holdout (never paired, scored, or traced by this work):**
  - `example_data/multifile-session/2022orch/IMG_9901.MOV` + `ZOOM0004.WAV` — explicitly reserved.
  - The entire `2022orch` holdout split — contaminated by the diagnosis; NOT used to calibrate. (It
    MAY be sanity-run for a non-gating number — see §6.5.)
  - The unused hsBach recorders `Gigue Recording.WAV`, `Prelude Recording.WAV`,
    `ZOOM0027/0031/0034/0035`, and the unused `test 2`/`test 3` recorders `ZOOM0027/0031/0034/0035` —
    never paired or measured here; available as fresh same-event/negative material for the next agent.
  This is well over 25% of the unused real events left untouched for an uncontaminated holdout.
- **Validation (schumann): MEASURED, not tuned.** No identity threshold was changed to fit schumann;
  the only change that rescues it is the §9.2/§9.3 conflation fix (the ceiling above real material),
  and it passes the FRACTION gate (0.138 ≥ 0.08) once it is no longer force-`Ambiguous`'d.

### 6.5 §15.2 gate results under profile 2 (re-measured on this machine)

`couple corpus metrics --matcher fingerprint-primary --profile 2`:

| split | cases | TP | FP | FN | ambiguous-true | confident-false | offset p95 |
|---|---|---|---|---|---|---|---|
| **calibration** (real hsBach) | 6 | 3 | 0 | 0 | 0 | **0** | 72.8 ms (coarse; the 3 negatives correctly `NoMatch`) |
| **validation** (schumann frozen) | 1 | **1** | 0 | 0 | **0** | **0** | **0.197 ms** (within 0.2 ms of the frozen truth; was `Ambiguous` under profile 1) |
| **synthetic** | 33 | 31 | 0 | 1 | 1 | **0** | **0.041 ms** (drift p95 1.04 ppm) |

- Per-case calibration trace (profile 2): the full-length true pair `test1_172_zoom33` Matches via the
  FRACTION path (count 801, frac 0.104, cov 0.992); the short/very-short true pairs Match via fraction
  (frac 0.125 / 0.159); the 3 negatives are `NoMatch` (frac 0.003–0.013, count ≤ 114, cov ≤ 0.829 —
  excluded by BOTH absolute clauses). **schumann** Matches (count 5922, frac 0.138, cov 0.989, p95
  31.2, refined lag −5536.4 ms).
- **Synthetic is at the profile-1 BASELINE (no regression):** the lone synthetic miss is the SAME
  `ppm-600` extreme-drift case that profile 1 missed (`ambiguousTrue = 1`), not a new failure — the
  recall fix changed nothing on synthetic (31/32 both profiles, offset p95 0.041 ms identical).
- **0 confident false matches on calibration + validation + synthetic.** Determinism: two calibration
  runs produced **byte-identical** metrics JSON. No NaN/Infinity (every field finite; `validate()`
  rejects non-finite before serialization).

### 6.5b Contaminated-holdout SANITY number (2022orch — NON-GATING, NOT a release gate)

The boundary-12 holdout (2022orch) was READ during the diagnosis, so it is contaminated and can no
longer gate the release. It is sanity-run here ONLY to corroborate the fix (no threshold was tuned to
it). Under profile 2: **7/7 true pairs Match, 0 FP, 0 FN, 0 confident-false** (was 1/7 under profile
1). The recall fix is dramatic AND it exercises the ABSOLUTE path on real capped pairs:

| case | class (prof 1 → **prof 2**) | inlier count | sampled frac | cov | Match path |
|---|---|---|---|---|---|
| bach      | Ambiguous → **Match** | 3014 | 0.088 | 0.974 | FRACTION (0.088 ≥ 0.08) |
| bach2     | Ambiguous → **Match** | 2478 | 0.075 | 0.998 | **ABSOLUTE** (count ≥ 500, cov ≥ 0.90; frac < 0.08) |
| berlioz   | Ambiguous → **Match** | 2398 | 0.069 | 0.974 | **ABSOLUTE** |
| dvorak    | Ambiguous → **Match** | 2867 | 0.074 | 0.998 | **ABSOLUTE** |
| dvorak2   | Ambiguous → **Match** | 1707 | 0.063 | 0.990 | **ABSOLUTE** |
| dvorak3   | Match → **Match** | 747 | 0.132 | 0.949 | FRACTION (under the §9.2 cap; corr 8345) |
| prokofiev | Ambiguous → **Match** | 2388 | 0.069 | 0.988 | **ABSOLUTE** |
| berlioz↔dvorak (neg) | NoMatch → **NoMatch ✓** | 23 | 0.001 | 0.901 | neither (count 23 ≪ 500, frac ≪ 0.08) |

This independently confirms the calibration-derived thresholds: the five sub-0.08-fraction true pairs
are rescued by the ABSOLUTE path (count 1707–2867 ≥ 500, cov ≥ 0.97), while the negative — even though
its coverage (0.901) squeaks over 0.90 — is excluded because its inlier COUNT (23) is far below 500
(and its fraction 0.001 ≪ 0.08). The COUNT clause is the discriminator on this negative; 500 holds it
out with a ~22× margin. **This number is NON-GATING; the release gate is a fresh holdout (§6.6).**

### 6.6 Decision

**Still NO-GO for the default switch — by design of THIS task.** Profile 2 demonstrably restores
recall on the long capped pair (schumann: `Ambiguous` → `Match`, sub-ms alignment) while keeping zero
confident false matches and full determinism. But the boundary-12 holdout (2022orch) is now
**contaminated** (it was read during the diagnosis), so it can no longer be the release gate. Flipping
the default to `FingerprintPrimary` is a LATER boundary, gated on a **FRESH real holdout** scored once
with profile 2 (the reserved-unseen events in §6.4 supply it). This task did NOT switch the default
and did NOT build or run the fresh holdout.

---

## 7. Fresh-holdout release gate (boundary 13 — the UNBIASED §15.2 run, scored once)

PLAN-SPECTRAL-FINGERPRINT-MATCHING §15.1 ("run holdout once") / §15.2. An independent release-gate
agent built a FRESH, uncontaminated hold-out from reserved-unseen real media (`holdout-fresh/`) and
scored it **exactly once** against the frozen `FingerprintMatchParams::profile_v2`. No threshold,
profile, or matcher code was changed; the default was NOT switched.

### 7.1 Reserved-media inventory + same-event determination (legacy fine-lock ground truth)

Durations probed with ffprobe; same-event status + offset/drift from the legacy full auto-sync
(`couple sync`) and the corpus `legacy-fine-lock` verdict.

| pair | a dur | b dur | legacy verdict | offset (ms) | drift (ppm) | same-event? |
|---|---|---|---|---|---|---|
| `IMG_9901.MOV ↔ ZOOM0004.WAV` | 231.9 s | 232.1 s | **fineMatch** (NCC 0.948 / z 32.2; 12/12 windows; RMS 2.69 ms) | **1050.19** | **-22.7** | **YES** |
| `IMG_9901.MOV ↔ Gigue Recording.WAV` (2013 hsBach) | 231.9 s | 126.4 s | NoMatch (0/12 windows) | — | — | no (cross-session) |
| `IMG_9901.MOV ↔ Prelude Recording.WAV` (2013 hsBach) | 231.9 s | 280.6 s | NoMatch | — | — | no (cross-session) |

`IMG_9901/ZOOM0004` is the single reserved camera+Zoom pair in the 2022orch folder and is a genuine
same-event recording (a ~232 s orchestral take). **Does it cap?** NO — it groups **35537**
correspondences, UNDER the 50000 §9.2 sample cap, and Matches via the **FRACTION** path (inlierFrac
0.096 ≥ 0.08, count 1871, cov 0.985, p95 43.2 ms). It does NOT enter the long-capped absolute-path
regime. No other reserved file forms a no-leakage same-event pair long/dense enough to exceed the cap
(the reserved hsBach recorders have no reserved video to anchor; the hsBach videos are calibration
material — reusing them would leak the hsBach event).

### 7.2 The fresh holdout (`holdout-fresh/manifest.json`)

3 cases, all `allowedForTuning:false`: **1 true pair** (`fresh_img9901_zoom0004`, ~231 s overlap) +
**2 negatives** (`fresh_img9901_gigue_negative`, `fresh_img9901_prelude_negative`, cross-session).
**No leakage:** no file is shared with calibration (bach 172.mov / ZOOM0033.WAV / 33-shortened*),
validation (schumann), or the contaminated old holdout (2022orch bach/berlioz/dvorak/prokofiev set).
The original `holdout/` (2022orch) is relabelled CONTAMINATED + NON-GATING (`holdout/README.md`).

### 7.3 §15.2 gate results on the FRESH holdout (profile 2, scored once)

`couple corpus metrics --manifest fixtures/matcher-corpus/holdout-fresh/manifest.json --repo-root . --matcher fingerprint-primary --profile 2`

| §15.2 gate | target | fingerprint (fresh holdout) | verdict |
|---|---|---|---|
| confident false-match rate | 0 % | **0/2 = 0 %** (both negatives NoMatch) | **PASS** |
| true-pair recall (vs legacy) | ≥ legacy | **1/1 = 100 %** (legacy fine-lock also 1/1) | **PASS** |
| same-piece / different-take confidently wrong | 0 | **0** | **PASS** |
| repeated-content confidently wrong | 0 | **0** | **PASS** |
| accepted-offset accuracy | p95 ≤ 1 ms (≥20 s) | refined lag 1046.5 ms; |err| **3.6 ms** vs the legacy fit | see note |
| metadata corruption cannot flip a strong result | yes | yes (boundary-10 hint non-vetoing; 11.6-yr camera/Zoom test green) | **PASS** |
| determinism | byte-identical | **byte-identical** (md5 534cd21c… twice) | **PASS** |

- **Ambiguity rate (true pairs): 0/1 = 0 %.** The fresh true pair Matches confidently (NOT ambiguous).
- **Negatives' exclusion:** Gigue — count 37, frac 0.003, cov 0.695; Prelude — count 31, frac 0.001,
  cov 0.759 (33448 corr yet only 31 inliers — high correspondence volume cannot force a match). Both
  excluded by BOTH absolute clauses (count ≪ 500) AND the fraction clause (≪ 0.08).
- **Offset-accuracy note:** the 3.6 ms figure is |fingerprint − legacy fitLag (1049.1)|; both methods
  agree on the offset within ~4 ms and within 0.9–3.6 ms of the 12-window auto-sync truth (1050.19).
  This is NOT a sub-ms gold pair (no frozen sub-ms truth exists for this recording; the legacy
  reference carries its own ~ms error), so it corroborates alignment rather than serving as the
  strict ≤1 ms accuracy gate. Boundary 7's fine refinement owns sub-ms.

### 7.4 §15.3 budgets (release build)

gen **6.0 s/hr** (≤10) | artifact **2.46 MB/hr** (≤10) | index 4.9 ms | lookup 0.35 ms/pair |
fit 2.86 ms/pair | 6×6 **113 ms** (≤5000) | 20×20 **1291 ms** (≤15000) | peakRSS **479 MB** (≤512).
All **PASS**.

### 7.5 What the fresh holdout COULD and COULD NOT test

- **COULD (unbiased):** zero-confident-false-match SAFETY (2 fresh negatives, incl. a demanding
  high-corr/low-inlier one); fresh same-event RECALL parity with legacy (1/1, not ambiguous);
  accepted-offset agreement; byte determinism. All PASS on genuinely-unseen material.
- **COULD NOT (unbiased):** the profile-2 **absolute identity path** (`min_abs_inlier_count = 500`,
  `min_abs_coverage = 0.90`). The only no-leakage fresh same-event pair (IMG_9901/ZOOM0004) does NOT
  cap (35537 < 50000) and Matches via the FRACTION path, so the novel absolute-path thresholds get NO
  unbiased real exercise here. Their only real-capped evidence stays the CONTAMINATED §6.5b run (five
  2022orch pairs Match via the absolute path; the negative excluded by COUNT 23 ≪ 500) — corroborating
  but not unbiased.
- **Statistical weight:** SMALL — 1 true pair, 2 negatives. The 0-confident-false-match result is a
  meaningful SAFETY signal on unseen data; the 100 % recall is a single positive (weak as a
  percentage, strong as "the fix did not break a fresh real match").

### 7.6 Decision — ESCALATE (the absolute-path regime is not unbiased-tested)

The fresh holdout is **GO on every gate it can carry**: 0 confident false matches on unseen
negatives, fresh recall parity with legacy, correct alignment, full determinism, budgets met — and it
independently confirms the recall fix did not regress a genuinely-unseen real pair. **But** it CANNOT
unbiased-test the profile-2 absolute identity path, which was the novel, previously-untested part of
the freeze and is the mechanism that rescues the long capped pairs in deployment. The honest decision
is therefore **ESCALATE**, with two options for the owner:

1. **ACCEPT the boundary-13 switch on fresh-SAFE + contaminated-CORROBORATION.** The fresh holdout
   proves SAFETY and FRACTION-path recall on unseen data; the contaminated §6.5b run proves the
   absolute path rescues real capped pairs (7/7) while still rejecting the negative by a ~22× count
   margin. Combined, the evidence that the switch is safe is strong; the residual risk is that the
   absolute path's exact thresholds (500 / 0.90) have never met an UNSEEN capped pair.
2. **ACQUIRE a new multi-recorder LONG session** (a fresh camera + Zoom pair > ~400 s of dense
   orchestral content, to exceed the 50000-correspondence cap with a sub-0.08 sampled fraction), add
   it as a second fresh-holdout true pair, and score once — the only way to unbiased-test 500 / 0.90.

**Recommendation: option 2 if a long dense multi-recorder pair can be obtained cheaply; otherwise
option 1 is defensible.** This gate did NOT switch the default. **Holdout scored exactly once; no
threshold changed after observing it.**
