# UX-DECISIONS.md

**Project:** Couple
**Purpose of this document:** Definitive UX handoff for the technical-spec session. The spec session has not seen the interactive mockups or the design conversation; this document stands alone. It records what was *decided*, what the engine must therefore *provide*, and what remains *open*. Where the document gives concrete numbers (window sizes, thresholds, timings), treat them as validated starting values, not sacred constants, unless marked DECIDED.

**Provenance, briefly:** The design went through three deliberately opposed interactive HTML mockups (editor-centric, queue-centric, inspector-style), then converged through three feedback iterations against **real recording data** — a 23.9-minute concert video (camera audio, AAC, heavily AGC-limited) and a 30-minute external recorder WAV. All sync algorithms described in §4 were implemented in the mockups and validated against that real pair: global offset −5354.74 ms recovered from a cold start, drift +3.07 ppm measured with 0.276 ms RMS across 12 windows, total compute ~0.6 s in single-threaded JavaScript. Engine requirements below are therefore demonstrated, not speculative.

**What the app is:** A focused macOS desktop utility that syncs one externally recorded audio file to one video file (replacing the camera audio), with automatic waveform cross-correlation, drift (sample-rate mismatch) measurement and optional correction, and a manual verification UI built around multiple simultaneous magnified waveform views ("loupes") pinned to different timestamps — so the user never scrolls or re-zooms to check alignment at the start, middle, and end of a recording.

**v1 scope (DECIDED):** Single-pair editor. A lightweight session queue for multiple takes per session is the defined v2 direction (§2.6), with only a minimal placeholder affordance in v1. The editor is the product; the queue is an accelerator once the editor is proven. The keys `j`/`k`/`Enter` are reserved in the keymap for v2 queue triage and must not be bound to anything else in v1.

---

## 1. CHOSEN DESIGN

### 1.1 Paradigm

A single-window **workbench**. The verification-first single-pair editor is the entire v1 product surface; a multi-pair session queue is a defined v2 direction (§2.6) with only a minimal placeholder in v1. The window reads top-to-bottom as the workflow: **Input → Sync → Export** across a compact strip at the top, with the **waveform stage** owning all remaining (and all flexible) space below.

The defining design element: a full-width **overview strip** showing both complete tracks, with colored, draggable **anchor pins**, connected by colored leader lines to a row of **loupes** — live magnified views of both waveforms overlaid at each anchor. Every alignment adjustment (keyboard nudge, drag, automatic fit) updates all loupes simultaneously in real time. The user verifies sync at three points at once instead of zoom-scroll-zooming through the file.

![Full window, post-auto-sync, side-by-side loupes](design/mockups/01-full-window-post-sync.png)
*(The two loupe layouts: [side-by-side](design/mockups/side-by-side-view.png), [stacked](design/mockups/stacked-view.png). All mockup screenshots are structural reference, not a pixel spec — see design/mockups/README.md.)*

### 1.2 Window layout (DECIDED)

Four horizontal bands, top to bottom:

1. **Title/top bar** (intrinsic height): app wordmark; a live **overlap chip** ("overlap 99% · 23:50", or amber "no alignment"/"no overlap"); right-aligned segmented control for loupe layout (**Side-by-side | Stacked**).
2. **Workflow strip** (intrinsic height; max content width ~1340 px, centered when the window is wider): three columns — Input (~222 px), Sync (flexible, ≥500 px), Export (~230 px). Panels size to their content; nothing in this strip stretches to fill vertical space.
3. **Waveform stage** (all remaining height, flexible): overview strip, connector layer, loupes.
4. **Footer** (intrinsic height): one line of keyboard-shortcut reminders plus a note that the lamps/preview run on real peak data.

Resize policy (DECIDED): the waveform stage is the only greedy region. Minimum window ~1000 × 560; below that the window scrolls rather than crushing the waveforms. Extra width goes entirely to the waveform stage (the workflow strip caps and centers); extra height goes entirely to the loupes. No region may ever produce dead filler space when the window grows.

### 1.3 Input panel

Two file rows — camera and external — each with a color identity dot (camera = teal, external = amber, used consistently everywhere in the app), the filename, and the decoded duration. Below a divider, a single **session row** showing the pair name (file basenames joined) and a "1 of 1" position chip, plus a dashed **"+ add pair"** affordance.

In v1, "+ add pair" opens a new pair, replacing the current one after an unsaved-changes confirmation. It is a real affordance, not a stub — its purpose is enabling multiple-take sessions without quitting the app — but it does not add a persistent queue or list view. That is v2. (See §2.6 and §6.11 for details.)

Design note carried from feedback: pair identity must not depend on filenames being descriptive — real-world inputs are often `MVI_0012.MP4` + `ZOOM0007.WAV`. Durations are always shown; the v2 queue adds video thumbnails for the same reason.

### 1.4 Sync panel (the editor's control center)

Left: a small **video preview** (16:9, ~264 px wide, intrinsic height — never letterboxed into leftover space) with a transport bar: play/pause, timecode, two **transient lamps** (camera-transient and external-audio-transient indicator dots; see Open Questions §6.2 for whether these survive once real playback exists), and one small colored **jump button per anchor** that starts playback ~0.8 s before that anchor.

Right, two rows:

- **Row 1 — readouts and the primary action.** Three groups: **Offset** (editable numeric, ms, 2 decimals, signed; a small sub-line shows the human-readable form "= −5m 21.355s" when |offset| ≥ 1 s; nudge buttons −10/−1/+1/+10 ms), **Drift** (editable numeric, ppm, 2 decimals; sub-line shows projected end-to-end ms over the overlap, or "off — measured X ppm" when correction is disabled; nudge buttons −1/−0.1/+0.1/+1 ppm; the whole group is visually disabled when drift-correction mode is None), and the **Auto-sync** button (the panel's visually primary element; shows live progress percentage while running) with a **fit status** sub-line beneath it: "fit RMS 0.28 ms · 10 windows", color-coded green < 0.7 ms, amber < 3 ms, red above; "not measured" before any sync.
- **Row 2 — drift sparkline.** A small (~46 px tall, horizontally flexible, gracefully compressible) plot: measured per-window local offsets as dots (opacity encodes measurement confidence) against camera time, with the **current mapping** (offset + drift line) drawn through them. This is deliberately peripheral — a glanceable "do my measurements agree with my line" check, not a hero element. It doubles as a fitting aid: when the line threads the dots, the mapping matches the measurements.

There is intentionally **no "fit" button** and **no per-loupe match percentage** (both existed in earlier iterations and were removed — see §2.4 and the rationale notes inline).

[SCREENSHOT not captured: sync panel close-up with drift mode None / greyed drift group. The sync and export panels are visible — with Resample selected — in design/mockups/01-full-window-post-sync.png; for the None state (greyed drift group, "X ms uncorrected" summary line) the text above is the only reference.]

### 1.5 Export panel

Top to bottom: **Verified** toggle (checkbox-style button, keyboard `v`); **Export** button (visually inert until Verified; clicking while unverified explains the gate); **Drift correction** segmented control — **Resample | None** — with a one-line helper ("corrects drift, resamples audio" / "audio untouched (offset only)"); a small summary grid: offset, drift (shows "X ms uncorrected" when mode is None and drift was measured), fit RMS, overlap %.

The drift-mode control is not a passive export flag (DECIDED, and important): selecting **None zeroes drift in the live mapping**, so the loupes and audition show exactly what an offset-only export produces — the far-end loupe's local Δ honestly displays the uncorrected drift. The measured ppm is retained and restored when switching back to Resample. Any manual drift edit while in None mode flips the mode to Resample (editing a value you've chosen not to apply is a contradiction the UI resolves automatically).

### 1.6 Waveform stage

**Overview strip** (~96 px, full width): two stacked lanes — camera (teal, top) and external (amber, bottom) — drawn over the **union** of both tracks' extents in video-time coordinates, so partial overlap is immediately visible as lanes that don't fully cover each other. The external lane is drawn through the current offset/drift mapping and therefore slides live as offset changes. The overlapping region gets a subtle background highlight. Minute tick marks with sparse labels run along the bottom. Lane labels "V · CAMERA" / "A · EXTERNAL" sit at the lane corners. A thin playhead line appears during playback. Two floating chips sit top-right: **"lock zoom"** (toggle, default on) and **"+ anchor"**.

**Anchor pins:** one per anchor, a small numbered flag in the anchor's identity color (a fixed palette of four hues distinct from teal/amber) with a stem line down through both lanes. Pins are draggable along the overview; on release the pin **snaps to the nearest strong transient** (±4 s search). Pin colors match the corresponding loupe's accent everywhere (flag, loupe border, leader line, jump button).

**Connector layer:** colored bezier leader lines from each pin down to the top-center of its loupe — **side-by-side layout only** (DECIDED: in stacked layout connectors crossed the content and are omitted entirely; the color-matched labels carry the association).

**Loupes:** 2–4 (default 3), in either a single row of equal columns (side-by-side) or full-width stacked rows (stacked; full width trades the at-a-glance connectors for maximum horizontal alignment precision — both layouts ship, side-by-side is the default; DECIDED). Each loupe:

- Header: identity dot · anchor time · **local Δ** ("local Δ +0.19 ms", color-coded green |Δ| < 0.7 ms, amber < 3 ms, red above, grey "—" when no confident measurement) · window size ("±120ms") · **auto** chip (lit when the window is auto-framed) · spacer · **⌖ snap** · **⇥ align** · **×** remove.
- Canvas: both tracks' min/max peak bands drawn **overlaid** at the same center line with additive ("screen") blending so overlap literally brightens; **stepped per-bin rendering** (DECIDED — honest pixels; smoothing/interpolation exists as a deferred render-style option, §5); **per-loupe local normalization** — each track independently scaled so its in-window peak fills ~86% of half-height (DECIDED, and necessary: the camera track is AGC-limited while the external preserves dynamics; no global gain makes them comparable in every passage, local gain does); a vertical center line in the anchor color; during playback, a playhead in the focused loupe.
- **Residual guidance arrow:** when the measured local Δ exceeds ~12% of the window, an amber arrow is drawn from center toward where the matching external content sits, labeled with the distance ("ext content → 38.6 ms") — the user always knows which way and how far, never lost off-screen.

![Stacked loupes layout](design/mockups/stacked-view.png)
*(Stacked layout reference. Not captured in any screenshot: the residual guidance arrow and the drift-mode-None loupe state — the text above is the only reference for those.)*

![Misaligned state — large offset, partial overlap](design/mockups/04-pre-sync-no-alignment.png)
*(Closest capture to the pre-sync marker: a badly misaligned mapping (offset ≈ −39 s, 87% overlap) with visibly disagreeing loupes. The true virgin state — offset 0, "no alignment" chip — was not captured; the §1.2 chip description and the §3 state definitions are authoritative.)*

---

## 2. INTERACTION PATTERNS

### 2.1 Alignment model (DECIDED — single source of truth for all math)

State: `offsetMs` (resolution 0.01 ms) and `driftPpm` (resolution 0.01 ppm). Mapping from video time *t* (seconds, camera timeline) to external-file time *u*:

```
u(t) = t · (1 + driftPpm·1e−6) + offsetMs/1000
```

The **local lag** at video time *t*, in ms: `L(t) = offsetMs + t · driftPpm·1e−3`. A per-window *measured* local lag λ(t) (engine-supplied, §4.3) has the same units/sign; the per-anchor residual displayed as "local Δ" is `λ(tᵢ) − L(tᵢ)`. Positive Δ means the matching external content currently appears to the *right* of the camera content by Δ ms.

Sign intuition: increasing offset shows later external content at the same video time, i.e. external waveform features move left.

### 2.2 Anchors

- **Auto-proposal** (on load and after every auto-sync): three anchors at the *sharpest attack* inside each of three zones of the overlap — [1%, 30%], [38%, 62%], [70%, 99%] of the overlap span (falls back to the camera duration when overlap is unknown/short). "Sharpest attack" = highest onset score from the engine's ranked onset list (§4.5). Validated on the real recording: all three picks were attacks out of near-silence (envelope ~20–60 → ~180–240 of 254).
- **Manual placement:** drag a pin along the overview (live loupe update while dragging); on release it snaps to the strongest transient within ±4 s. **⌖ snap** in a loupe header re-snaps in place. Rationale (DECIDED early and load-bearing): anchors on silence or sustained tones are unjudgeable — a held cello note is near-periodic and can alias a full cycle; transients are the only honest evidence.
- **Add/remove:** "+ anchor" adds the best unused onset in the overlap keeping ≥8% of-overlap separation, capped at 4; anchors keep chronological order; minimum 2. Each loupe header has ×.
- Moving or re-snapping an anchor invalidates and re-runs that anchor's local measurement.

### 2.3 Loupe windows: auto-framing and zoom lock

- Default window 240 ms (DECIDED after testing 40/80 ms against real 0.5 ms-resolution data — tighter defaults were both too blocky and contextless).
- **Auto-frame:** a loupe in auto mode sizes its window to `max(240 ms, 5 × |local Δ|)` (rounded to 10 ms, capped 2 s). Purpose: after a coarse sync, the far-end loupe shows the drift as a *visibly-sized* displacement of two recognizable shapes — never "so far out of frame I don't know the direction." Auto-framing re-applies on load, auto-sync, anchor move, and ⇥ align — **not** on every nudge (zoom changing mid-nudge is disorienting; DECIDED).
- **Zoom lock** (toggle on the overview, **default ON**; DECIDED): all loupes share one scale. Scroll-zoom on any loupe zooms all; an offset drag therefore moves identical pixel distances in every loupe. Under lock, auto-framing computes each loupe's desired window and applies the **maximum** to all. Manual scroll-zoom exits auto mode (for all loupes under lock, for one loupe unlocked); clicking the **auto** chip re-arms (all loupes under lock). Toggling lock on unifies windows to the current maximum.

### 2.4 The nudge/alignment workflow (the heart of the app)

Everything below updates **live, every frame**: all loupe canvases, the overview's external lane, both readouts, the sparkline's mapping line, and each loupe's displayed local Δ (recomputed against cached measurements; the *measurements themselves* are re-run on a ~450 ms debounce whenever the mapping has moved > 30 ms from a measurement's center, and on commit of typed values).

**Offset adjustments:**

- Plain **drag on any loupe canvas**: slides offset; ms-per-pixel = window/width, so drag sensitivity follows zoom. Direction: content follows the pointer.
- **Coarse drag on the overview's external lane** (lower half of the strip): offset at overview scale (seconds per pixel) — the ballpark/rescue gesture for grossly unaligned pairs.
- **Keyboard:** ←/→ = 1 ms; with ⇧ = 10 ms; with ⌥ = 0.1 ms.
- **Nudge buttons** ±10/±1 ms; **typed value** in the readout (Enter/blur commits).

**Drift adjustments:**

- **⇧-drag on a loupe** = *pivot drift drag* (DECIDED, a signature interaction): rotates the mapping line about the **reference anchor** — anchor 1 (the leftmost), or the rightmost when dragging anchor 1 itself — so the reference loupe's alignment is held fixed while the dragged loupe moves; intermediate loupes follow proportionally. Math: for pointer delta δ ms at anchor *i* with reference *r*: `Δslope = δ/(tᵢ−t_r)` ms/s; `driftPpm += Δslope·1000`; `offsetMs −= t_r·Δslope`. Guarded when |tᵢ−t_r| < 5 s.
- **⇥ align** (per loupe): one click zeroes that anchor's measured Δ using the same pivot math (plain offset shift when the anchor *is* the reference). Canonical two-click manual flow: align anchor 1, align anchor 3, anchor 2 confirms.
- **Keyboard:** [ / ] = 1 ppm; with ⌥ = 0.1 ppm. Buttons ±1/±0.1 ppm; typed value.
- Any drift edit while drift-correction mode is None flips mode to Resample (§1.5).

**Per-anchor evidence:** local Δ in ms with the green/amber/red thresholds of §1.6, the in-canvas guidance arrow, and the single **global** metric — fit RMS over the measured windows — under the Auto-sync button. Removed by explicit decision: a per-loupe "match %" (Pearson over visible pixels) — it changed with zoom and anchor position, i.e. moved when nothing meaningful changed; residuals in ms are the domain-true metric, with one global RMS for "overall fit."

### 2.5 Pair lifecycle: proposed → verified

1. **Load** both files → tracks decoded/peak-extracted → anchors proposed → **auto-sync runs automatically** (DECIDED: the app opens in an already-aligned state with the result clearly marked unverified; users who distrust the auto-sync result still have the full manual toolkit, and genuine failures still exercise the rescue flow). Drift-correction mode is auto-suggested on completion (§6.3).
2. **Inspect:** glance at three loupes (green Δs, visibly interleaved waveform texture), optionally nudge/pivot/align, optionally audition (§2.7), optionally watch the video preview at each anchor via the jump buttons. A toast on open reports offset, ppm, end-to-end ms, window count, RMS, and the suggested mode. Status is *unverified* until the user toggles Verified regardless of confidence.
3. **Verify:** toggle Verified (`v`). Export becomes active.
4. **Export** (§2.8).

Manual rescue path (auto-sync failed or distrusted): dismiss the failure toast → coarse-drag the external lane until the lanes roughly coincide → loupes start finding content → drag/nudge at anchor 1 → ⇧-drag or ⇥ align at the far anchor → verify.

### 2.6 Session and queue

**v1 (DECIDED):** single pair at a time. "+ add pair" in the Input panel opens a new pair, replacing the current one after an unsaved-changes check. Auto-sync runs on load (§2.5). "1 of N" position chip counts pairs opened in the current session (informational only — no persistent list in v1).

**v2 direction (documented so the spec can leave room; NOT v1 scope):** a left-rail session list in the style of the queue-centric mockup. Drag a folder of files in; the app proposes video+audio pairings via recording timestamps from metadata, verified or supplemented by cross-correlation peak across candidate pairs (envelopes already computed at import, so all-pairs comparison is cheap for ~3–6 takes). Each row shows a thumbnail, both filenames, duration, an auto-sync **status chip** (synced + residual ms / low-confidence / failed), a confidence bar, and an **inline verified checkbox** so high-confidence pairs can be cleared without opening the editor. Keyboard triage: `j`/`k` move through rows, `v` verifies and advances to the next unverified, Enter opens the editor. "Needs attention first" sort floats failures up. "Export all verified (n)" batch button. Explicitly NOT multicam timeline reconstruction.

### 2.7 Audition (DECIDED)

Three monitor modes, selectable in the editor:

- **Pan L/R** — camera audio hard left, external hard right; gross errors audible as inter-ear echo.
- **Overlay** — both tracks summed to mono at matched loudness; the user listens for flange/comb-filter/doubling artifacts, which sweep and vanish as alignment improves. (The musician's natural test.)
- **Null** — external polarity-inverted and summed with camera at matched loudness; the user tunes for *minimum* residual. Important caveat to surface in UI copy: with different microphones/positions/AGC the null is a relative dip, never silence.

Playback starts from the focused anchor (or any anchor via the jump buttons), shows playheads in the overview and focused loupe, and **always applies the current offset sample-accurately**. Drift is applied as a *rendered* correction: the audition region is (re)resampled after a drift change is committed — **not** live varispeed while dragging (DECIDED: "listenable after I've applied a drift adjustment" is the requirement; this materially simplifies the engine). When drift-correction mode is None, audition plays offset-only — matching what export will produce.

`space` toggles play/pause. Loudness matching for Overlay/Null uses per-track gain derived from the engine's level analysis.

### 2.8 Export

Gated on Verified. Produces, per the "standard, few core options" decision:

1. **Muxed video** (primary): video stream copied untouched into the same container family as the input; audio replaced by the aligned external track, trimmed/padded to the video timeline, offset applied sample-accurately, resampled only when drift mode is Resample. Option (single checkbox, default **off** — Open Question §6.7): retain the original camera audio as a second, default-muted track.
2. **Aligned WAV** (secondary): the external audio rendered to the video's timeline (same trim/offset/drift treatment) at the source sample rate — for users finishing in an NLE/DAW.

Codec/container details (PCM vs AAC defaults per container, bitrate) are delegated to the spec session with the guidance: PCM where the container allows (MOV), high-bitrate AAC otherwise; one visible choice at most.

### 2.9 Complete keyboard and pointer reference (as currently decided)

| Input | Action |
|---|---|
| ← / → | Offset −/+ 1 ms |
| ⇧ ← / ⇧ → | Offset −/+ 10 ms |
| ⌥ ← / ⌥ → | Offset −/+ 0.1 ms |
| [ / ] | Drift −/+ 1 ppm |
| ⌥ [ / ⌥ ] | Drift −/+ 0.1 ppm |
| v | Toggle Verified |
| space | Play / pause audition and preview |
| drag loupe | Offset (ms/px = window/width) |
| ⇧-drag loupe | Drift, pivoting on reference anchor (§2.4) |
| scroll on loupe | Zoom (all loupes when zoom-locked) |
| drag external lane in overview | Coarse offset (s/px scale) |
| drag anchor pin | Move anchor; snaps to nearest transient on release |
| Enter (in numeric field) | Commit typed offset/drift |
| j / k / ⏎ | *Reserved* for v2 queue triage (row nav / open) |

Typed-field focus suppresses the global shortcuts. All shortcuts also exist as visible controls; the footer shows the cheat line.

---

## 3. STATES (described in words; deliberately not mocked)

**Empty / launch.** No pair loaded. A single load surface: drop zone + two pickers (camera/video, external/audio), each slot showing its status independently. In the native app this should accept the *media files themselves* (video + WAV), not pre-extracted peak data — peak extraction is the engine's job (§4.1). User can do nothing else until both slots fill; the rest of the window may be hidden or inert.

**Import in progress.** Per-slot progress ("decoding audio… / building peaks…"). Cancellable per slot. Auto-sync begins automatically once both slots complete.

**Auto-sync running on load.** Auto-sync button shows progress ("Searching 43%…", then "Measuring drift…"); alignment inputs disabled; cancellable (Esc or clicking the button again), returning to the unaligned unverified state.

**Success.** Toast with the numbers; mode suggested; anchors proposed and green; fit status green. State is *unverified* — user must toggle Verified.

**Low-confidence result.** Heuristics in §4.9. UI: overlap chip or fit status in amber; toast says the result needs review; anchors with weak local measurements show grey "—" Δ; the user's tools are exactly the normal ones (move anchors to better transients, audition, manual nudge). Verified is never auto-set; low confidence simply means the human checks harder.

**Failed sync.** No statistically plausible correlation peak (or overlap below minimum). The mapping is left at zero (no half-applied guess); a toast explains, and the manual rescue path (§2.5) is the recovery. Drift reported "not measurable" when fewer than 3 windows pass the confidence gate.

**Unpaired / mismatched files.** v1: one slot filled — the load surface simply waits, with the empty slot highlighted. A same-kind file dropped twice replaces with confirmation.

**No overlap (user-induced).** If manual offset pushes the tracks apart, the overlap chip turns amber "no overlap", loupes show one-sided silence, measurements all read "—". Nothing breaks; dragging back recovers.

---

## 4. CORE ENGINE REQUIREMENTS (all validated in the mockups against real data unless noted)

### 4.1 Peak pyramid / waveform data

- Engine ingests the actual media (video container + audio file), decodes audio as **mono, 48 kHz, float**, and builds a min/max **peak pyramid** per track. Levels used by the UI: **24, 96, 480, 2400, 9600, 48000 samples/bin** (0.5 ms → 1 s). Values stored as int8 −127..127 with a per-track `normalization` (track absolute peak).
- The UI's universal read API: `band(t0, t1, nColumns) → {mins[], maxs[]}` — per-column min/max over an arbitrary time range, selecting the **coarsest level still yielding ≥ nColumns bins**, aggregating when bins exceed columns. Loupes call it at ~0.05–2 ms/px; the overview at ~1–2 s/px. Must be cheap enough to call for every loupe on every animation frame (the mockups redrew 3–4 loupes + overview at 60 fps during drags).
- **Resolution requirement discovered on real data:** the 24 samples/bin (0.5 ms) floor is visibly blocky below ~±60 ms windows. Provide **a finer base level (≤ 8 samples/bin, ≈0.17 ms)** *or* a raw-sample window fetch near anchors for deep zoom. (Open Question §6.5 covers which.)
- **Format:** the mockups transported peaks as JSON (~22–24 MB/track, ~4× binary size, multi-second parse). The native engine should use a binary representation internally; JSON interchange is not a requirement.
- Per-track **level statistics** for display gain: the UI's overview gain rule is `1.44 / p95(active envelope bins)` where active = bins > 12% of the track max at the 9600 level (plain percentiles fail: long silent tails skew them — observed). Loupes additionally re-normalize locally per view (§1.6); the engine just needs to expose envelope stats or the raw data to compute them.

### 4.2 Global auto-sync (offset search)

Hierarchical normalized cross-correlation over **log-compressed envelopes** (`log1p(max−min)` per bin). This design and all parameters below are **validated against real data** and supersede any earlier algorithmic suggestions:

- Stage 1: 9600 spb (0.2 s bins), full lag range ±(both durations), minimum overlap 20 s. **Selection by significance score `z = ncc·√(overlapBins)`, not raw NCC** — empirically mandatory: on the real pair, raw NCC picked a spurious 42-second-overlap peak at the lag boundary (ncc 0.83, z 12) over the true 99.6%-overlap alignment (ncc 0.88, z 75). Log compression further strengthened the true peak.
- Refinement stages at 2400/480/96/24 spb over shrinking spans (±500/±80/±14/±3 ms), plain NCC, **parabolic sub-bin interpolation** at each stage.
- Measured performance: ~0.6 s single-threaded JS for a 24 min × 30 min pair; native will be faster. **Progress callbacks** needed during stage 1 only; the whole operation must be **cancellable**.
- Outputs: offsetMs, coarse and fine confidence (NCC values) for status heuristics.

### 4.3 Windowed local-lag measurement (powers local Δ, drift, sparkline, ⇥ align)

`measureLocal(tc, halfWin, centerMs, spanMs) → {lagMs, ncc} | null`: NCC of a window of camera envelope (~16–20 s) against the external, searching lags `center ± span` at 96 spb then refining ±3 ms at 24 spb, parabolic sub-bin. Real-data accuracy: window-to-window RMS 0.276 ms (sub-bin). Called: per anchor (span ±80 ms, centered on current L(t)) on anchor move and on a ~450 ms debounce after mapping changes > 30 ms; and for the **drift pass**: 12 windows spread over the overlap, **reject ncc ≤ 0.4**, least-squares `(t, λ)` → offset + slope (`ppm = slope·1000`), requiring ≥ 3 accepted windows. Each call must run in low single-digit ms (it's invoked interactively).

### 4.4 Least-squares fit

`fitLS(points) → {offMs, ppm, rmsMs}` over `(t, lagMs)` pairs. Used by auto-sync's drift pass; RMS feeds the global fit-status readout, recomputed in UI against the *current* mapping whenever offset/drift change.

### 4.5 Onset ranking (anchor proposal and snap)

From the 480 spb envelope: score = (20 ms rise)² / (preceding level + 0.15·max), threshold rise > 10% of max, skip 1 s at each file edge, 0.3 s non-max suppression, ranked list (cap ~4000). Queries: best onset in [a, b] (zone proposal); best within ±4 s of t (snap). Validated to pick attacks-from-silence over loud-passage wiggles on the real recording.

### 4.6 Mutable alignment state (the document the UI reads/writes)

```
{ offsetMs (0.01 ms resolution), driftPpm (0.01 ppm),
  driftMode: 'resample' | 'none', savedDriftPpm,
  anchors: [seconds], frames: [{winMs, auto}], lockZoom,
  perAnchorMeasurements: [{lagMs, ncc} | null],
  driftWindows: [{t, lagMs, ncc}],
  verified: bool, loupeLayout: 'grid' | 'stack' }
```

Every mutation is observable by the renderer the same frame. (Persistence across launches: Open Question §6.8.)

### 4.7 Playback / audition interface

- Decode and play the **actual video** with the external audio mapped through current offset (sample-accurate) — replacing the mockup's placeholder canvas. Playback is **shell-side** (AVPlayer/AVFoundation on macOS); the engine supplies only the aligned audio per the established core/shell playback boundary. Seek to arbitrary video time; per-anchor cue points.
- Three monitor matrices: pan L/R; mono overlay sum with per-track loudness matching; null (polarity-invert external + sum, loudness-matched).
- Drift handling: **offline/streamed re-render of the audition region after a drift commit** — realtime varispeed is explicitly *not* required (DECIDED). When driftMode = none, play offset-only.
- Optional but cheap and useful if real playback is delayed in development: envelope values + onset events at a given time (this powered the mockup's meters/lamps).

### 4.8 Export pipeline

Per §2.8: video stream copy; aligned-audio render (trim/pad to video timeline, sample-accurate offset, optional resample for drift — the resampler must be transparent-quality since the no-resample mode exists specifically to protect audio quality); optional second (camera) audio track; standalone WAV render. Progress + cancellation for the export job.

### 4.9 Status heuristics (initial values — tune against more material in Session C)

Observed on the validated run: true alignment had coarse NCC 0.88 / fine (24 spb) NCC 0.67 / 10 of 12 drift windows accepted / RMS 0.28 ms. These values come from one recording pair; expect tuning once the real-data test corpus grows. Suggested chips: **synced** = fine NCC ≥ 0.5 and ≥ 6 windows and RMS < 1 ms; **low-confidence** = found but below that; **failed** = no peak with z above noise floor or overlap < 20 s.

**Drift-correction auto-suggest (DECIDED heuristic):** suggest Resample when the measured end-to-end drift (|driftPpm| × overlapSeconds / 1000) exceeds 2 ms; suggest None otherwise. This decides on the directly measured quantity rather than a proxy like clip duration. The 2 ms threshold is an open question (§6.3); the *form* of the heuristic is decided.

---

## 5. EXPLICITLY DEFERRED (do not re-litigate in the spec; these belong to the native implementation)

- All visual styling: exact palette values (the *semantic* color mapping of §1.3/§1.6 is decided), typography, spacing, panel chrome, dark/light themes, app icon.
- Animation/transition treatments; toast vs native notification styling.
- The HTML loader card — the native app uses standard open/drag-drop flows (§3 describes the states, not the chrome).
- Waveform **render style options**: stepped per-bin is the decided default; interpolated contours and max-contour outlines were prototyped and may return as a user preference. Not a v1 feature.
- Exact pixel dimensions in §1 (informative, not normative) — except the resize *policy* and minimum-size principle, which are decided.
- Mock-specific scaffolding that must not leak into the spec as features: the procedural fake video scene, simulated meters, JSON peak interchange.

---

## 6. OPEN QUESTIONS (decide before or during spec; nothing below was silently resolved)

1. **App name:** Couple (DECIDED — working title "Resync" retired).
2. **Transient lamps:** mockup scaffolding for "does sync *look* right" — once real A/V playback exists, keep them as a feature beside the preview, or drop? UI copy should note that human A/V-skew perception bottoms out around 40–80 ms, so the video preview alone catches only gross errors.
3. **Drift-correction auto-suggest threshold:** currently end-to-end > 2 ms → Resample. Plausibly should be frame-relative (e.g. half a frame duration) or content-dependent (music tolerates more than dialogue).
4. **Status thresholds** (§4.9): initial values are observational from one recording pair; need tuning across more material.
5. **Sub-ms zoom data:** finer pyramid base level (≤ 8 spb) vs on-demand raw-sample fetch near anchors — pick one (or both) in the spec.
6. **Pivot reference:** always anchor 1 (current) vs user-selectable pivot anchor.
7. **Export defaults:** keep-camera-audio-as-second-track default (currently off); codec defaults per container; whether the WAV export is always offered or opt-in.
8. **Persistence:** session/alignment state across launches (document format? auto-save? "recent pairs"?) — never discussed.
9. **Anchor cap:** 4 was a mockup convenience; confirm or raise.
10. **Nudge step sizes:** 1 / 10 / 0.1 ms and 1 / 0.1 ppm felt right in testing but were never explicitly ratified.
11. **"+ add pair" v1 behavior (DECIDED):** replace current pair with unsaved-changes check. Does "1 of N" chip persist across app launches or reset to 1 of 1 on relaunch? Related to §6.8 (persistence).
12. **Auto-run auto-sync on load (DECIDED):** yes — app opens already-aligned, result marked unverified.

---

*End of document.*
