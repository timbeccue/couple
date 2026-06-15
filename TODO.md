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

## TODO-001: Remove imported media or reset the session

**Status:** Ready  
**Type:** Feature  
**Area:** Media pool, queue lifecycle, persistence

### Motivation

Imported files currently remain in the session. A user needs both a fast way to
return to a blank session and a precise way to remove accidental or obsolete
imports without restarting the app.

### Current behavior

The media pool owns imported track handles and couples reference those entries.
There is no user-facing removal flow. Removing an entry therefore requires more
than deleting a row: dependent sessions, candidate lists, approvals, audition,
queued sync work, and persisted queue state can all reference it.

### Proposed solution

Add standard media-pool selection using click, Command-click, and Shift-click.
When one or more entries are selected, show a trash button in the media-pool
header and support the Delete key. Also add a `Clear Session...` action for a
complete reset.

Removal must affect only the open Couple session. It must never delete the source
media from disk. Releasing a track handle may make it eligible for normal cache
garbage collection, but removal should not synchronously purge the cache.

Use a confirmation dialog only when removal has consequences:

- Removing a video removes its couple row.
- Removing selected audio clears that source from affected couples, removes it
  from candidate/alternate lists, cancels their active work, clears approval and
  export state, and re-resolves them against the remaining audio.
- Removing an unused file needs no warning beyond normal undo/confirmation
  conventions.
- `Clear Session...` always confirms with counts of media files, couples, and
  approved exports that will be cleared.

This is preferable to always deleting a whole couple when audio is removed. The
video remains a valid unit of work and may resolve to another source.

### Acceptance criteria

- Multiple media entries can be selected and removed in one action.
- The confirmation names the number of affected couples and approved rows.
- Active preview, audition, import, resolve, and sync work touching removed
  entries stops cleanly.
- No couple, candidate, selection, or persisted queue record retains a removed
  content key.
- Removing selected audio leaves the video row unresolved or re-resolved rather
  than deleting the row.
- `Clear Session...` returns the app to the initial empty state.
- Source files and previously exported files are untouched.
- The cleared state is persisted and does not reappear on relaunch.

---

## TODO-002: Make media-pool audio previews pauseable and seekable

**Status:** Done  
**Type:** Feature  
**Area:** Media pool audio preview

### Motivation

Users often need to identify a long recording by listening somewhere in the
middle. A play/stop button forces them to restart from the beginning and gives no
feedback about position.

### Current behavior

Each audio row has a play/stop toggle backed by a shared `AVPlayer`. Playback
state is stored locally by each row, so starting a different preview can also
leave the previous row's icon stale.

### Proposed solution

Replace the stop behavior with play/pause. Centralize observable preview state in
one controller that owns the active media-pool entry, playback state, current
time, and duration.

The active preview row should expand by one compact line containing a seek bar
and elapsed/total time. Keep the extra control hidden on inactive rows to avoid
cluttering the pool. Pausing preserves position; pressing play resumes. Starting
another file transfers the active state and resets the previous row's icon.

### Acceptance criteria

- The play icon becomes pause while that file is playing.
- Pause preserves the current position and play resumes from it.
- The active row shows a seek bar that can scrub while playing or paused.
- Time labels update during playback without causing the list to jump.
- Starting another preview updates both rows correctly and stops the old audio.
- Reaching the end returns the item to a non-playing state and seeks to the
  beginning for the next play.
- Removing the active file or clearing the session stops and releases preview
  playback.

### Implementation notes

New `PoolPreviewController.swift` replaces the old per-row `playing` flag + the
stop-only `PoolPreview.shared` singleton. The transport is split for testability:
`PoolPreviewState` is a PURE `Equatable` value carrying the entire observable
surface (`activeKey`, `isPlaying`, `currentTime`, `duration`, `isScrubbing`); its
transitions (`toggle`/`scrubBegin`/`scrubTo`/`scrubEnd`/`tick`/`setDuration`/
`reachedEnd`/`stop`) are pure functions returning a `PoolPreviewAction`
(`.load`/`.play`/`.pause`/`.seek`/`.none`). `PoolPreviewController` is the thin
`@MainActor @Observable` shell that owns ONE `AVPlayer`, applies those actions,
and feeds back a ~10 Hz `addPeriodicTimeObserver` tick + the
`AVPlayerItemDidPlayToEndTime` notification. `AppModel` owns the controller, so
`activeKey` is the single source of truth: `QueueRail.PoolPreviewButton` shows
play⇄pause derived from controller state (switching files flips the old row's
icon immediately), and only the active row expands by one compact line —
`PoolPreviewScrubber` (mini `Slider` + mm:ss elapsed/total, amber tint, chip
styling). Pause preserves position; play resumes from it; a different file does a
`.load` that releases the prior item and starts at 0; natural end rewinds to 0
and goes non-playing. Scrub works playing or paused — `isScrubbing` suppresses
tick overwrites so the periodic feed never fights the drag, and `scrubTo` seeks
immediately. Observers are removed against the exact player/item before any swap
or teardown (no stale row retention). Explicit cleanup is `controller.stop()`,
invoked from the app teardown lifecycle (`AppModel`'s
`NSApplication.willTerminateNotification` handler, before `saveAll()`); the same
method is the seam a future per-file removal (TODO-001) calls before freeing a
pool handle — TODO-001 itself is NOT implemented here. Verified: `swift build`
clean (only pre-existing `VideoThumbnail` Sendable warnings, untouched);
`swift test` 16 passed / 0 failed; the state machine has deterministic automated
coverage via a new media-free `DebugDrive` hook (`COUPLE_PREVIEW_TEST=1`,
mirroring the TODO-006 `COUPLE_LAYOUT_TEST` pattern) asserting every transition
incl. play/pause/resume, scrub-while-playing, scrub-while-paused, file-switch,
tick-vs-scrub no-fight, natural-end rewind, and stop/cleanup — 25 passed /
0 failed. `git diff --check` clean.

---

## TODO-003: Show useful media metadata without crowding the pool

**Status:** Done  
**Type:** Feature  
**Area:** Media pool

### Motivation

Filenames are often opaque. Small amounts of metadata help identify recordings
and catch obvious mistakes, but displaying every technical field in every row
would make the media pool harder to scan.

### Current behavior

Duration is available and appears inside some usage strings. Creation time is
stored, and probe/import metadata contains additional technical information, but
the pool does not present a consistent metadata hierarchy.

### Proposed solution

Keep the always-visible row compact:

- primary line: filename;
- secondary line: duration followed by usage (`unused`, `alternate`, or the
  number of couples using it).

Do not add sample rate, codec, path, and other details to every row. Put richer
metadata in a hover tooltip or compact info popover:

- all files: role, duration, creation date when available, file size, and path;
- audio: codec/format, sample rate, channel count, and bit depth when known;
- video: container/codec, dimensions, frame rate when known, and embedded audio
  format.

This progressive disclosure gives duration high visibility while keeping
diagnostic information available on demand.

### Acceptance criteria

- Every imported row displays duration in a consistent location.
- Usage remains visible without repeating long lists of couple filenames in the
  normal row.
- Detailed metadata is accessible without navigating away from the session.
- Missing metadata is omitted or shown as unknown without malformed separators.
- Long filenames and narrow rails remain readable.
- Metadata comes from structured probe/file APIs rather than filename parsing.

### Implementation notes

Progressive disclosure landed without growing the always-visible row. The probe
`MediaInfo` (already produced once during import) is now RETAINED on
`MediaPool.Entry` (`info`), and the on-disk byte size is captured at import via
filesystem resource values (`MediaPool.fileSize` → `URL.resourceValues(.fileSizeKey)`),
so the popover re-renders from retained data and NEVER re-probes when it opens.
Restore stays compatible with no queue-schema change: restore re-imports through
`pool.importDropped`, which re-probes, so `info`/`fileSizeBytes` repopulate
naturally (the persisted queue schema is untouched).

`PoolEntryRow`'s second line is now `Fmt.hmmss(duration) · <concise usage>`
joined only when both parts exist (no dangling `· ` on a zero-duration file).
Audio usage is a state/COUNT only — `unused`, `alternate for N couples`, or
`used by N couples` — the long couple-filename list was removed from the normal
row. An unobtrusive `info.circle` affordance (`PoolInfoButton`) sits beside the
TODO-002 preview button on every row and opens a compact `.popover`
(`MediaMetaPopover`, 280 pt, independent of rail width) instead of making the row
taller. The popover shows, omitting any genuinely-missing field cleanly:
all files — role, duration, creation date, size, full path; audio —
container/codec, sample rate, channels, bit depth; video — container/codec,
dimensions, frame rate (AVG fps preferred, nominal fallback), and a compact
embedded-audio summary (one numbered line per stream, only known parts joined so
a stream missing bit depth has no trailing separator).

The display/formatting/projection logic is a PURE `MediaMetaCard` in
`CoupleKit/MediaMeta.swift` (so the `CoupleKitTests` target gives it real
coverage; the app view only renders its rows). It uses `ByteCountFormatter` for
size and a fixed-locale (`en_US_POSIX`) ISO-8601→readable date formatter; a row
exists IFF its value is genuinely known (no `unknown`/`—`/blank placeholders).
TODO-002's preview transport (`PoolPreviewButton`/`PoolPreviewScrubber`/
`isActivePreview`/`model.preview.*`) is fully preserved — the info button is
purely additive (verified: 0 preview lines removed in the diff; a live capture
shows the play button beside the new info button on the active audio row).

Verified: `swift build` clean (only the pre-existing `VideoThumbnail` Sendable
warnings, untouched); `swift test` 25 passed / 0 failed (was 16; +9 media-free
projection tests covering full audio, full video, multiple embedded audio,
missing optionals/unknown bit depth/no creation time, unparseable date,
no-probe-info, and the sample-rate/channels/fps/container-codec formatters).
A live `DebugDrive` capture of the Schumann pair confirmed the compact two-line
rows (`23:55 · confident`, `29:59 · used by 1 couple`) and the info+preview
buttons coexisting. `git diff --check` clean. Metadata the probe genuinely cannot
provide is omitted rather than faked (e.g. a container with no creation
timestamp, a stream with no `bitsPerSample`, or a stripped-audio video — which
the pool already surfaces as `skipped`, never reaching a row).

---

## TODO-005: Add a unified long-running activity indicator

**Status:** Ready  
**Type:** Feature  
**Area:** Import, analysis, pairing, sync, export

### Motivation

Several operations perform meaningful background work, but feedback is scattered
across a header spinner, local text, and per-couple states. Users need a clear
answer to "is the app working, and what is it doing?"

### Current behavior

Core jobs expose useful phase/fraction updates for import, sync, and export.
Media import stores a text label, queue resolution exposes only a Boolean, and
background couple syncs are intentionally ambient. There is no single,
consistent activity surface.

### Proposed solution

Add a compact global activity strip near the session header. Show it only after
an operation lasts roughly 200-300 ms so quick tasks do not flash. Use a
determinate bar when the work reports a trustworthy fraction and an indeterminate
bar otherwise. Keep row-level progress where it adds useful detail, but let the
global strip summarize the current foreground task and queued work.

Recommended text states:

- `Scanning files...`
- `Inspecting 3 of 12: filename.mov`
- `Importing 3 of 12: filename.mov`
- `Decoding audio - 42%`
- `Building waveform - 68%`
- `Analyzing transients - 91%`
- `Comparing 24 video/audio pairs...`
- `Resolving matches...`
- `Updating matches after import...`
- `Syncing couple 2 of 7: video.mov`
- `Searching for alignment - 43%`
- `Refining alignment...`
- `Measuring drift...`
- `Preparing export...`
- `Exporting 2 of 5 - 61%`
- `Finishing export...`

Only claim a distinct analysis phase if the underlying job can report it
honestly. Otherwise combine it with `Building waveform` or `Finishing import`
rather than showing fabricated precision.

### Acceptance criteria

- Import, analysis/extraction, candidate pairing, queued auto-sync, and export
  all produce visible activity when they take meaningful time.
- Text identifies both the operation and the current file/couple when useful.
- Determinate progress is monotonic within a phase.
- Indeterminate work does not display a fake percentage.
- Multiple queued operations show an item count or queue position.
- The indicator clears on success, cancellation, and failure.
- Failure transitions to an actionable error instead of leaving a stuck bar.
- Existing cancellation controls remain available for cancellable jobs.

---

## TODO-009: Keep match quality independent from manual source selection

**Status:** Ready  
**Type:** Behavior fix  
**Area:** Pairing verdicts, resolve banner, queue colors

### Motivation

Selecting an audio source is a user decision, not evidence that the source is a
high-quality match. Turning the row green immediately overstates confidence and
conflates pairing quality with approval/queue membership.

### Current behavior

Unless a pairing is explicitly marked `forced`, `CoupleRow.verdict` returns
`good` whenever `selectedAudioKey` is non-nil. An ambiguous row can therefore
turn green solely because the user clicked a candidate.

### Proposed solution

Model these concepts separately:

- **selection:** which audio the row currently uses and whether it was automatic
  or manual;
- **match quality:** confident, questionable, or no supported match, derived from
  resolver/classifier evidence for that specific video/audio pair;
- **approval:** whether the user queued the couple for export.

A manual choice should remain questionable when the resolver could not
distinguish it or when it is below the evidence floor. A full re-analysis may
promote it to confident only if the pairing classifier now independently clears
the confident threshold. Successful alignment or a low fit RMS alone must not
erase source ambiguity; alignment quality can stay green in its own fit display.

### Acceptance criteria

- Clicking a candidate does not by itself change an amber/questionable verdict
  to green.
- Green means the selected video/audio pair independently meets the confident
  matching criteria.
- Below-floor forced pairs remain visibly questionable.
- Pairing verdict color, alignment fit, and approval state use distinct UI
  signals and cannot overwrite one another.
- Manual selection still creates a usable session and permits deliberate
  approval/export.
- Persistence restores selection provenance and match quality correctly.
- Tests cover ambiguous manual selection, confident automatic selection,
  below-floor forcing, and evidence-based promotion after re-analysis.

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

Give the overview its own visible time range, independent of loupe window sizes:

- pinch over the overview to zoom around the pointer;
- horizontal trackpad scrolling pans the visible range;
- provide a small `Fit` action to restore the full union;
- support an accessible mouse/keyboard equivalent, such as
  Command-scroll and Command-0;
- keep anchors, overlap shading, playhead, ticks, seeking, and coarse alignment
  gestures mapped through the visible range.

Do not couple overview zoom to `lock zoom`; that control describes loupe scale.
Clamp the viewport to meaningful track extents and enforce a minimum span so it
cannot collapse to an unusable range.

### Acceptance criteria

- Users can zoom in, zoom out, pan, and return to the full-duration fit.
- Zoom is centered on the pointer/playhead rather than always on the midpoint.
- Anchor dragging, playhead seeking, and external-lane offset dragging remain
  accurate while zoomed.
- The visible time range is obvious from tick labels.
- Loupe zoom and overview zoom do not unexpectedly change each other.
- Changing couples initializes a sensible fitted view.

---

## TODO-011: Keep automatically proposed anchors inside true overlap

**Status:** Ready  
**Type:** Behavior fix  
**Area:** Anchor proposal

### Motivation

An anchor outside the shared media interval cannot show corresponding content
from both tracks and therefore cannot provide honest alignment evidence.

### Current behavior

Anchor proposal uses the overlap only when it is longer than 30 seconds.
Otherwise it falls back to the full camera duration, which can place automatic
anchors where the external source has no content.

### Proposed solution

Automatic anchors must always lie within the current video/external overlap.
Adapt the number and spacing of anchors to short overlaps rather than falling
back outside them:

- use three zoned anchors when the overlap can support them;
- use two well-separated anchors for a shorter but still measurable overlap;
- use no automatic anchors when there is no usable overlap, and surface the
  manual rescue state instead.

This constraint applies to automatic proposal and automatic `+ anchor`
selection. It does not prevent a deliberate manual anchor move, though the UI
should make an out-of-overlap anchor visibly non-evidentiary.

### Acceptance criteria

- Every automatically proposed anchor is inside the current overlap.
- Automatic `+ anchor` never chooses an onset outside the overlap.
- Short overlaps produce a valid reduced anchor set or none; they never fall
  back to camera-only regions.
- No-overlap state avoids invalid measurements and explains why anchors cannot
  be proposed.
- Tests cover full, partial, short, and zero overlap.

---

## TODO-013: Allow drag-to-pair over the central workspace

**Status:** Ready  
**Type:** Feature  
**Area:** Manual pairing, drag and drop

### Motivation

Dragging an audio file onto the selected video's main workspace is a natural way
to force or replace a pairing. A small queue-row-only drop target is easy to miss,
especially when the selected row is already represented prominently in the
center.

### Current behavior

Audio files from the media pool can already be dropped onto a queue row to call
the manual repair path. The video preview, resolve banner, waveform area, and sync
tools are not drop targets.

### Proposed solution

Make the selected couple's central content region one coherent audio drop target,
covering the video preview, resolve banner, waveform row, and sync-tools area.
During a valid drag, highlight the region and show copy such as
`Pair selected video with audio.wav`.

For an unresolved couple, dropping immediately selects and pins the audio, builds
the session, and runs full sync/classification. If the selected couple already
has a source, ask for confirmation before replacing it when the row is approved
or has user edits; otherwise make replacement a direct, reversible action.

Use the same underlying repair operation as the existing queue-row target so the
two gestures cannot diverge. Match color must follow TODO-009: a forced selection
does not become green merely because the drop succeeded.

### Acceptance criteria

- Dragging an audio pool entry over the selected center workspace shows a clear
  valid-drop highlight.
- Dropping on any major central subregion invokes the same repair path.
- No selected couple, a non-audio payload, or an unavailable video rejects the
  drop without changing state.
- The new source is selected/pinned, the session is rebuilt, and sync runs.
- Replacing an approved or edited pairing requires confirmation and clears or
  preserves downstream state according to the confirmed action.
- Queue-row and center-workspace drops produce identical pairing results.
- Pairing verdict remains evidence-based as specified by TODO-009.

---

## TODO-014: Make anchor dragging direct and add a precision magnifier

**Status:** Ready  
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
