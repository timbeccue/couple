# Original holdout (`2022orch`) — CONTAMINATED, NON-GATING

PLAN-SPECTRAL-FINGERPRINT-MATCHING §15.1. This split (`manifest.json`, the seven `2022orch`
camera+Zoom true pairs + one berlioz↔dvorak negative) was the boundary-12 release holdout. It is
**no longer an unbiased release signal** and **MUST NOT gate the boundary-13 default switch**.

## Why it is contaminated

It was OBSERVED outside its one-shot release run:
- read during the boundary-12 holdout decision (CALIBRATION.md §3.3),
- read again during the long-recording recall **diagnosis**,
- read a third time as the recall-fix **sanity check** (CALIBRATION.md §6.5b).

Per §15.1 ("if thresholds change after seeing holdout, create a new holdout set before claiming an
unbiased result"), its profile-2 result (7/7 true pairs Match, 0 confident-false) is **corroborating
only** — it confirms the recall fix and is the sole *real-capped* exercise of the profile-2 absolute
identity path, but it cannot be the unbiased gate.

## The unbiased gate

See `../holdout-fresh/` — a fresh, reserved-unseen hold-out (IMG_9901/ZOOM0004 true pair + two
cross-session negatives) scored once against `profile_v2`. The GO/NO-GO/ESCALATE decision lives in
`../CALIBRATION.md §7`.

This directory is retained for history and as a non-gating corroboration run. `manifest.json` keeps
`allowedForTuning:false` on every case (the split rule still holds), but its numbers are NOT the
release gate.
