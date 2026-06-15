# Fresh holdout — the UNBIASED boundary-13 release gate (§15.1 "run holdout once")

PLAN-SPECTRAL-FINGERPRINT-MATCHING §15.1/§15.2. This is the **fresh, uncontaminated** hold-out
built for the boundary-13 default-switch decision, scored **exactly once** against the frozen
`FingerprintMatchParams::profile_v2`. It supersedes `../holdout/` (the original `2022orch` set),
which is now **contaminated and NON-GATING** (it was observed during the boundary-12 calibration,
the recall diagnosis, and the fix's sanity-checks — see `../holdout/README.md`).

## Hygiene (no leakage)

Every file here was **reserved-unseen** by the recall-fix work — present in NO calibration,
validation, or old-holdout manifest, never paired/scored/traced during tuning:

- `2022orch/IMG_9901.MOV` + `2022orch/ZOOM0004.WAV` — the single reserved camera+Zoom pair in the
  2022orch folder (the seven manifested 2022orch true pairs use the other MOV/WAV files).
- `hsBach/Gigue Recording.WAV`, `hsBach/Prelude Recording.WAV` — reserved hsBach recorders
  (calibration only used `bach 172.mov ↔ ZOOM0033.WAV`; these recorders were never paired).

No file here is shared with calibration (`hsBach test 1/2/3`: bach 172.mov / ZOOM0033.WAV /
33-shortened*), validation (`schumann`), or the contaminated old holdout (`2022orch` bach/berlioz/
dvorak/prokofiev set). Event-grouping is respected: the true pair and negatives are anchored on the
fresh `IMG_9901.MOV`, and the negatives draw their audio from a **different session** (2013 hsBach)
so a positive could never be confused with a negative within one event.

## Cases (n = 1 true pair + 2 negatives; all `allowedForTuning:false`)

| id | a | b | sameEvent | overlap | ground truth |
|---|---|---|---|---|---|
| `fresh_img9901_zoom0004` | IMG_9901.MOV | ZOOM0004.WAV | true | ~231 s | offset 1050.19 ms, drift -22.7 ppm (legacy full auto-sync, 12/12 windows) |
| `fresh_img9901_gigue_negative` | IMG_9901.MOV | Gigue Recording.WAV | false | — | cross-session non-match |
| `fresh_img9901_prelude_negative` | IMG_9901.MOV | Prelude Recording.WAV | false | — | cross-session non-match (high-corr / low-inlier safety case) |

## What this set CAN and CANNOT test (honesty, §15.1)

- **CAN** test, unbiased: zero-confident-false-match SAFETY (two fresh negatives, one demanding),
  fresh same-event RECALL (the true pair Matches), accepted-offset accuracy, determinism.
- **CANNOT** test, unbiased: the profile-2 **absolute identity path** (`min_abs_inlier_count = 500`,
  `min_abs_coverage = 0.90`). The fresh true pair groups **35537** correspondences — UNDER the 50000
  §9.2 sample cap — and Matches via the **FRACTION** path (inlierFrac 0.096 ≥ 0.08). It does NOT
  enter the long-capped regime, so it does not exercise the novel absolute-path thresholds. No other
  reserved file forms a no-leakage same-event pair long/dense enough to cap. The absolute path's only
  real-capped evidence remains the CONTAMINATED `2022orch` sanity run (corroborating, not unbiased).

## Run (scored once)

```bash
couple corpus metrics --manifest fixtures/matcher-corpus/holdout-fresh/manifest.json \
  --repo-root . --matcher fingerprint-primary --profile 2     # the gate
couple corpus metrics --manifest fixtures/matcher-corpus/holdout-fresh/manifest.json \
  --repo-root . --matcher legacy-fine-lock                     # recall-vs-legacy reference
```

Results + the GO/NO-GO/ESCALATE verdict are recorded in `../CALIBRATION.md §7`.
