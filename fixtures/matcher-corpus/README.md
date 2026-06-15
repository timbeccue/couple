# Matcher corpus

PLAN-AUDIO-MATCHING-PREWORK §10. A deterministic calibration / validation corpus and a
byte-stable metrics block for the pairing matcher, established **before** the spectral
fingerprint matcher (PLAN-SPECTRAL-FINGERPRINT-MATCHING.md) is introduced. The legacy envelope
matcher is not expected to pass future fingerprint goals; this corpus is the shared yardstick
both implementations run against.

## Layout

```
fixtures/matcher-corpus/
  README.md            ← this file
  schema.json          ← the case/manifest JSON schema (§10.1)
  synthetic/
    cases.json         ← the 15 §10.2 synthetic families (generated, committed)
  calibration/
    manifest.json      ← real difficult examples — tuning ALLOWED
  validation/
    manifest.json      ← real held-back examples — tuning ALLOWED, kept for honest validation
  holdout/
    manifest.json      ← real examples — tuning FORBIDDEN (every case allowedForTuning=false)
```

**Media MAY be gitignored; manifests + ground truth MUST be committed** (and are — see the
`.gitignore` matcher-corpus carve-out). Synthetic media is *regenerated* from each case's
committed `gen` block; real media lives under `example_data/` (gitignored, large).

## Case shape (§10.1)

Each case is exactly:

```json
{
  "id": "clean_30s",
  "group": "synthetic",
  "a": "synthetic/clean_30s/camera.wav",
  "b": "synthetic/clean_30s/external.wav",
  "sameEvent": true,
  "expectedOffsetMs": 1500.0,
  "expectedDriftPpm": 0.0,
  "minimumOverlapS": 30.0,
  "tags": ["clean"],
  "allowedForTuning": true
}
```

Synthetic cases additionally carry a `gen` block (the generation parameters). Holdout cases
**must** have `allowedForTuning: false` — `CorpusManifest::validate()` rejects a holdout
manifest otherwise.

## Synthetic families (§10.2)

`synthetic/cases.json` is produced by `couple corpus gen-synthetic` and covers all 15 required
families: clean; AGC/limiting; telephone-band; reverb; SNR 0/5/10 dB; durations 8/12/20/60/600 s;
drift −600/−100/0/+100/+600 ppm; same-macro-envelope-but-independent-fine-content; repeated
phrase with several plausible offsets; sustained periodic tone; mostly silence with one transient;
dropout; recorder restart / disjoint; multiple simultaneous recorders; and >6 plausible
candidates. Everything is reproducible from the committed parameters — `gen-synthetic` is a pure
function (no clock, no RNG), so re-running it reproduces `cases.json` byte-for-byte.

Two synthetic proxies are honest stand-ins (recorded in PROGRESS.md): **telephone-band** and
**reverb** use the available band-limit + AGC knobs rather than a tunable narrowband filter / a
convolver, because a genuinely narrowband or reverberant variant would need a non-defaulted field
on the shared `FixtureSpec` and editing ~26 proven test fixtures. They carry the `telephone` /
`reverb` tags for intent; truly band-limited / reverberant real media lands with the fingerprint
plan's own corpus.

## Real-data split (§10.3 — no event leakage)

Different clips of the **same recording event** stay in the **same split** (otherwise the holdout
leaks event-specific acoustics). The events here:

| event       | split        | why |
|-------------|--------------|-----|
| `hsBach`    | calibration  | the documented difficult bach 172 / ZOOM0033 material (test 1/2/3) — the short-excerpt and no-match cases the classifier was built for |
| `schumann`  | validation   | the frozen canonical pair (offset −5536.639 ms, +3.57 ppm) — held for honest validation, not threshold tuning |
| `2022orch`  | holdout      | a distinct multi-piece session (berlioz / dvorak / prokofiev / bach) — RESERVED, never tuned on |

That reserves one of three recording events (≈33% ≥ the required 25%) entirely for holdout. The
`bach 169–172` material appears in both `difficult-match/` and `multifile-session/hsBach/`; it is
ONE event (`hsBach`) and lives only in calibration — it is never split across splits.

**Thresholds are never tuned on holdout** (§10.5). The holdout manifest exists to be scored once,
after tuning, to detect over-fitting.

## Metrics command (§10.4)

```bash
# Score a manifest with the legacy z-gate verdict (default):
couple corpus metrics --manifest fixtures/matcher-corpus/synthetic/cases.json \
                      --repo-root fixtures/matcher-corpus

# …or with the fine waveform-lock classifier verdict:
couple corpus metrics --manifest fixtures/matcher-corpus/synthetic/cases.json \
                      --repo-root fixtures/matcher-corpus --verify-active
```

The metrics JSON is written to **stdout** and is **byte-identical across runs** (the §11.3 gate):
it contains only deterministic quantities — TP / FP / FN, ambiguous-true-pairs, confident false
matches, median/p95 offset & drift error, per-tag breakdown, and the peak candidate count (the max
prefilter candidates any single video accumulates — a corpus property). The **non-deterministic
runtime** is printed to **stderr**, never serialized, so two runs compare byte-for-byte.

Real-data cases whose (gitignored) media is absent are skipped from scoring but still validated
structurally (the split rule + holdout invariant are enforced by `validate()` regardless), so the
manifests + splitting rule are enforceable even on a checkout without the media.

## Regenerating

```bash
# Re-derive the committed synthetic manifest (byte-identical to what is checked in):
couple corpus gen-synthetic --out fixtures/matcher-corpus/synthetic/cases.json
```
