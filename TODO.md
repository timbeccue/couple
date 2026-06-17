# Couple Product Backlog

This file separates product thinking from implementation. Adding an item here does
not authorize implementing the whole backlog, and an implementation task should
name the specific item or items it intends to complete.

## Instructions for agents

### Before adding an item

1. Read this entire file, the relevant specs/decision logs, and enough of the
   current implementation to understand existing behavior.
2. Check whether the proposal duplicates, overlaps, depends on, or contradicts an
   existing backlog item or an established product decision.
3. Identify the underlying motivation, not only the requested UI mechanism.
   Consider whether a simpler or more general solution would satisfy it better.
4. Make the item self-contained enough that a different agent can implement it
   without access to the conversation that produced it. Include:
   - the user problem and motivation;
   - relevant current behavior;
   - the recommended solution and important interaction details;
   - concrete acceptance criteria;
   - dependencies or affected product decisions.
5. If the proposal is ambiguous, internally inconsistent, risky, or weaker than a
   plausible alternative, stop and discuss it with the user before adding it.
   Agent product feedback is wanted; do not silently preserve a poor idea.
6. If an addition overlaps or conflicts with an existing item, ask the user
   whether to merge, replace, or keep both before editing this file.

Do not add a vague placeholder with unresolved questions merely to capture it.
Resolve the questions first, then record the resulting decision.

### When implementing an item

1. Confirm the item is still compatible with the current code and authoritative
   specs. This backlog records intent but does not silently supersede
   `SPEC-V2.md`, `UX-DECISIONS.md`, or later decision records.
2. Work only on the requested item(s). Do not treat adjacent backlog entries as
   bundled scope unless the user explicitly groups them.
3. Set the item to `In progress` before substantial work. Set it to `Done` only
   after implementation and appropriate verification are complete.
4. If implementation changes an established decision, update the authoritative
   document and append any required `SPEC-CHANGE` entry to `PROGRESS.md`.
5. Preserve the motivation and final acceptance criteria. Add concise
   implementation notes or links rather than replacing the original reasoning.
6. If new information invalidates the proposed solution, return the item to
   product discussion instead of forcing the recorded design into the code.

### Status values

- `Ready`: sufficiently defined for a separate implementation task.
- `In progress`: actively being implemented.
- `Blocked`: cannot proceed until a named dependency or decision is resolved.
- `Deferred`: intentionally retained but not currently planned.
- `Done`: implemented and verified.
- `Rejected`: considered and intentionally declined, with the reason retained.

---

## TODO-010: Zoom and pan the overview waveforms

**Status:** Ready  
**Type:** Feature  
**Area:** Overview waveform

### Motivation

The full-duration overview is useful for orientation but can compress long
recordings so heavily that nearby events, the playhead, and anchor placement are
difficult to inspect.

### Current behavior

The overview always fits the union of both tracks. Loupe zoom is independent and
does not change the overview timeline.

### Proposed solution

Give the overview its own visible time range, independent of loupe window sizes.
Match the established loupe interaction: vertical scrolling over the overview
zooms in or out around the pointer location. No modifier key should be required.

Horizontal trackpad scrolling pans the visible range. Provide a small `Fit`
action and a keyboard equivalent such as Command-0 to restore the full union.
Keep anchors, overlap shading, playhead, ticks, seeking, and coarse alignment
gestures mapped through the visible range.

Do not couple overview zoom to `lock zoom`; that control describes loupe scale.
Clamp the viewport to meaningful track extents and enforce a minimum span so it
cannot collapse to an unusable range. Route scroll events by pointer location so
overview scrolling changes only the overview and loupe scrolling retains its
existing behavior.

### Acceptance criteria

- Users can zoom in, zoom out, pan, and return to the full-duration fit.
- Vertical scrolling over the overview changes its zoom level with the same
  direction and general sensitivity as vertical scrolling over a loupe.
- Horizontal scrolling over the overview pans without changing zoom.
- Zoom is centered on the pointer/playhead rather than always on the midpoint.
- Anchor dragging, playhead seeking, and external-lane offset dragging remain
  accurate while zoomed.
- The visible time range is obvious from tick labels.
- Loupe zoom and overview zoom do not unexpectedly change each other.
- Changing couples initializes a sensible fitted view.

---

## TODO-014: Make anchor dragging direct and add a precision magnifier

**Status:** In progress — Part 1 (direct 1:1 drag) Done; Part 2 (precision magnifier) Deferred  
**Type:** Behavior fix and interaction feature  
**Area:** Overview waveform, anchor placement

### Motivation

Moving an anchor on the full-duration waveform should feel as immediate and
predictable as dragging the playhead. The current attempt to make slow movement
more precise instead makes the anchor lag behind the pointer and change
sensitivity during a drag.

Long recordings still need a precise placement aid. The overview compresses too
much time to show nearby waveform detail, so direct 1:1 dragging alone does not
give enough visual information to choose an exact moment.

### Current behavior

Anchor dragging applies a velocity-adaptive gain to each incremental pointer
delta: slow movement advances the anchor less than the pointer, while fast
movement approaches 1:1. This hidden sensitivity change feels glitchy and differs
from overview playhead dragging, which maps the current pointer position directly
to timeline time.

The overview has no temporary local-detail surface during anchor placement.
Users must open the loupes or rely on the heavily compressed full waveform.

### Proposed solution

Treat this as one two-part interaction:

1. Remove velocity-adaptive anchor movement. During a drag, map the pointer's
   current x-position directly through the displayed overview timeline, using
   the same absolute-position principle as playhead dragging. The pin should stay
   under the pointer, use unrounded time values, remain where released, and keep
   the existing explicit loupe-header snap action. Do not restore snap-on-release.
2. While an anchor is being dragged, disclose a temporary magnified waveform
   window over the overview around the active pointer/anchor. It should expose a
   substantially shorter time span at higher waveform resolution, preserve both
   camera and external lanes, and make the exact anchor time unmistakable. The
   window should slide along the overview as the drag moves and close when the
   drag ends or is cancelled.

The precision surface should behave as a local timeline lens, not as pointer
acceleration. Opening it must not move the anchor. Within the lens, pointer
position maps directly to the time visibly beneath it; if the lens recenters or
pans near an edge, the transition must preserve the anchor under the pointer
without a jump, dead zone, or hysteresis.

Design the final presentation in the context of the app rather than copying the
Photos trim control literally. Explore the lens width, zoom span, edge behavior,
lane labels, time readout, border/accent treatment, and how it avoids the
overview chips and monitor controls. Prefer the app's existing panel, chip,
anchor-color, camera-teal, and external-amber language. The active anchor color
should identify the lens without turning the whole surface into an unrelated
yellow trim box. Animation may soften disclosure, but must not animate the
pointer-to-time mapping during a drag.

This is complementary to TODO-010. This item provides transient, anchor-specific
precision without changing the overview's persistent visible range; TODO-010
provides deliberate zoom, pan, and fit controls for broader inspection.

### Acceptance criteria

- Slow and fast anchor drags both place the pin at the time represented by the
  current pointer position; no velocity gain, acceleration, or incremental-delta
  sensitivity remains.
- The anchor stays visually attached to the pointer and does not jump at drag
  start, while moving, when the precision window shifts, or on release.
- Releasing the pointer leaves the anchor exactly where placed and does not snap
  it to a transient.
- A magnified local waveform window appears only for the active anchor drag,
  follows the relevant overview region, and disappears cleanly on commit or
  cancellation.
- The precision window shows both waveform lanes, a clear active-anchor marker,
  and enough local time context or labeling to understand the magnified scale.
- Pointer-to-time mapping inside the precision interaction is direct and
  deterministic. Repeating the same drag position produces the same anchor time
  regardless of pointer speed or path.
- The lens remains usable near the start and end of media and near the left and
  right edges of the overview without clipping essential content or causing
  time-mapping discontinuities.
- The overlay does not obscure or capture the lane monitor controls, overview
  chips, other anchor actions, or the Option-drag coarse-offset gesture outside
  the active anchor drag.
- The final visual design is reviewed at narrow and normal center-column widths,
  with short and long recordings, all anchor colors, partial overlap, and both
  light waveform activity and dense transients.
- Focus, loupe disclosure on click, explicit snap, anchor measurement
  invalidation, and TODO-010's future zoomed-overview coordinate mapping remain
  compatible.

### Progress

**Part 1 — direct 1:1 anchor drag (Done, verified).** The velocity-adaptive gain is
removed. `AnchorPin`'s drag previously accumulated pointer deltas (`lastDragX`) and
scaled each by `min(1.0, 0.25 + abs(dx)/12)` — a hidden mid-drag sensitivity change.
It now maps the pointer's CURRENT x straight to a time through the displayed overview
range, the same absolute-position principle as the overview playhead scrub, so a given
pointer x always yields the same anchor time regardless of speed or path. Both paths go
through ONE shared mapping, `OverviewTimeline.time(atX:width:visible:)` /
`x(forTime:…)` (WaveformStage.swift) — no parallel coordinate system; pin/connector x
positions invert the same function. The pin's `DragGesture` now reads the shared
`"overview"` coordinate space (matching the playhead) and passes the displayed range +
lane width in. The target is unrounded and clamped only by the existing
`PairSession.moveAnchor` playable clamp (`0…cam.durationS`) — no second clamp; the pin
stays exactly where released. The explicit ⌖ loupe-header snap (`snapAnchor`) is
untouched, and so are the Option-drag coarse-offset gesture, Shift behaviors, playhead
seeking, tap-to-focus/loupe disclosure, and anchor-measurement invalidation
(`releaseAnchor` → re-measure unchanged). Because the mapping reads the displayed
`visible` range rather than a hardcoded full duration, it is compatible with TODO-010's
future zoomed/panned overview range. Verified: `swift build` clean (only the
pre-existing `VideoThumbnail`/`QueueRail` `NSImage`-Sendable + `libcouple.a` macOS-
version warnings); `swift test` 92 passed (unchanged — `CoupleKitTests` doesn't link
the app target). New coverage is an env-gated app-target DebugDrive hook
(`COUPLE_OVERVIEW_TEST=1`, mirroring `COUPLE_LAYOUT_TEST`): 16 checks that the x→time
mapping is direct (endpoints + midpoint), monotonic in x, clamped off-lane, x↔time
round-trips, the SAME final x yields the SAME time across slow/fast/wandering paths
(the anti-regression for the removed gain), correct on a non-zero zoomed range, and
divide-by-zero-safe — 16 passed. `git diff --check` clean. Touched only
WaveformStage.swift + DebugDrive.swift. Human click-through to confirm: slow drag (pin
tracks pointer, no lag), fast drag (no acceleration, same landing as slow at the same
spot), drag start (no jump to pointer or away from it), release (stays put, no snap),
explicit ⌖ snap still jumps to the nearest transient, and Option-drag on the external
lane still slides the coarse offset rather than moving an anchor.

**Part 2 — precision magnifier (Deferred).** The transient magnified local-detail lens
over the overview during an anchor drag is intentionally retained but not yet planned;
it needs an owner design pass (lens width, zoom span, edge/recenter behavior, lane
labels, time readout, accent/border treatment, and overview-chip/monitor-control
avoidance) per the item's design notes before implementation. Part 1 stands on its own
and does not depend on it.
