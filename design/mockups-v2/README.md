# Couple v2 — queue-centric mockup (Session G)

Interactive HTML mockup for the SPEC-V2 queue redesign, in the lineage of
`design/mockups/` (real data baked in; behavior decisions get ratified here,
then written into UX-DECISIONS.md as the V2 section at convergence).

## Run

```
cd <repo root>
python3 -m http.server 8741
open http://localhost:8741/design/mockups-v2/index.html
```

http is required (the mockup fetches `session-data.json` and `assets/`).

## What's real

Rows 1–4 are the owner's real session (`example_data/multifile-session`),
resolved by the Phase-7 engine — states, candidates, margins, triangle checks,
offsets, drift, fit windows, waveforms, and video frames all come from
`couple pair` / `couple sync` / `couple peaks` / ffmpeg. Regenerate with:

```
python3 design/mockups-v2/generate-session-data.py
```

Three labeled "demo —" rows cover the §3.3 states the real session didn't
produce (split across files / 2 sources / no match). Demo waveforms and
footage are procedural and marked as such.

Notable real numbers (also V2-F2 calibration evidence, recorded 2026-06-12):
bach 170 resolves `ambiguous` (margin 3.3 < 6.0) with ZOOM0031+ZOOM0027
measuring *simultaneous* (triangle Δ 1.6 ms) while Gigue Recording is a
different take; cross-take decoys reach z 14–19 (floor 8.5); real fit RMS runs
4.5–5.3 ms — red under v1's 0.7/3 ms thresholds — and drift hits 90 ppm
(2013-era recorder). The v1 status thresholds need re-calibration for this
class of material; carried as an open item for the UX-DECISIONS-V2 writeup.

## Design intent settled so far (owner directions, rounds 1–3)

These are binding intent even where the mockup only gestures at them:

- **Everything visible at once; no "places".** The editor is not a separate
  screen: ⏎ (or "⚙ sync tools") *expands* the sync tools in place and esc
  collapses them. There is no "back to queue" — the queue, hero, overview and
  export rail never go away.
- **Persistent tool bar pinned to the bottom** of the center column (round 7):
  always-visible toggles (⚙ sync tools, ◫ loupes) that are same-place
  on/off switches, plus Approve at the right end. Expanding sync tools reveals
  the offset/drift readouts inline in the bar — one level, not a nested
  disclosure (§11.1 variant A = these readouts appear; variant B = never).
  Known limit: when sync tools + loupes + the alternates box are all open on a
  short window the stack can overflow; Session H should let the workspace
  above the bar scroll so the bar stays pinned.
- **Workflow sections, not sorting.** The queue is three fixed groups —
  QUEUED FOR EXPORT / TO VERIFY / TO FIX — and couples migrate between them
  live as they're verified or repaired. (Names still under review.)
- **Quality = a single status dot, on the verdict model.** Pairings are
  **good / questionable / none** (green dot / red dot / hollow grey — hues
  deliberately distinct from the UI's verified-green/amber). Rationale
  (owner + data): fit is bimodal for same-source audio; the only real middle
  ground is the same-piece-different-take hazard, which means exactly "a human
  must pick" — so it IS the questionable bucket. Coverage is a fact about the
  recording, never a quality penalty (partial rows with good fit are green).
  The dot carries verdict only; a word rides along **only when action is
  needed** ("pick one" / "forced" / "no audio"). Match strength is deliberately
  not shown on rows (bimodal ⇒ noise) — raw z stays in the tooltip. The one
  place relative strength helps is choosing between same-piece takes, so the
  **candidate picker keeps a relative strength bar** (same verdict colors).
  Workflow sections mirror the verdicts: green → TO APPROVE, red/grey → TO FIX,
  approved → QUEUED FOR EXPORT; empty sections stay visible (dimmed).
  [TUNE] the verdict thresholds against the owner corpus (true pairs on the
  2013 recorder run ~5 ms RMS — v1's green/amber lines don't transfer).
  (Rounds 1–4 tried a dense tick meter; dropped as false precision — the data
  is categorical, not a 0–100% scale.)
- **Candidates are never destroyed.** Every candidate stays listed; switching
  selection is always one click reversible; manual choices pin (📌) and
  re-pairing runs never override them (§3.6).
- **Verify = queue for export, clickable everywhere**: row checkbox, editor
  button, and `v`.
- **Loupes are summoned, never resident**: anchor-pin click (focuses that
  anchor's loupe), `l`, or the fit-status line; esc dismisses. Pins at the
  bottom of the overview with stems running up to colored leader lines.
- **Media pool replaces the unused tray**: every imported file with a live
  usage indicator ("take 2" / "alt · take 5" / "unused"), hover highlights
  the couples using it, audio gets a low-key inline preview (real app:
  built-in preview or Quick Look), drag-to-re-pair stays.
- **Export names**: template default, per-row inline override marked
  `custom` with one-click return to the template. Renaming edits the stem
  only — the extension is owned by the format choice (v1's Auto/MOV/MP4/WAV
  selector survives: one global default in the export rail, per-couple
  override as a click-to-cycle tag on the export row; keep-camera-audio
  checkbox carries over).
- **Media pool**: collapsible ("MEDIA POOL · n"), "+ import" button (the
  whole window stays a drop target per §2.1), two-line items so a horizontal
  scrollbar is impossible; coverage micro-bar under queue rows is quiet
  (thin, dim, hidden entirely at full coverage).
- **Overview domain**: clamped to the video timeline ±15% with an "audio
  continues ▶" hint, instead of v1's full union (continuous-recorder files
  would crush the lanes).

Deliberately *not* implemented in the mockup (intent recorded, Session H must
honor):

- The video preview must never distort: AVPlayerLayer letterboxing, aspect is
  inviolable under any layout change.
- Zoom lock: the chip is shown; behavior is v1 UX §2.3 unchanged.
- Audition, monitor modes, drift re-render: v1 engine behavior, unchanged.

Out of scope for v2 (own phase + SPEC-CHANGE, owner agreed): export
size/resolution targeting (re-encode pipeline; conflicts with the v1
stream-copy contract).

## Files

- `index.html` — the mockup (self-contained; vanilla JS/canvas)
- `session-data.json` + `assets/` — baked real-session data (regenerable)
- `generate-session-data.py` — the bake script
- `screenshots/` — per-round captures for the record
