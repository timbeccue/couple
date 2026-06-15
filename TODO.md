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

**Status:** Done
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
header and support the Delete key.

Unused media should be removed immediately with no confirmation. If a selected
file participates in one or more couples, show one consequence-focused
confirmation before removal.

Removal must affect only the open Couple session. It must never delete the source
media from disk. Releasing a track handle may make it eligible for normal cache
garbage collection, but removal should not synchronously purge the cache.

After removing referenced media, regenerate every affected couple exactly as if
the current remaining files had been imported without the removed file:

- Removing a video removes its couple row.
- Removing audio discards affected rows' stale candidate data, selected source,
  pin/forced state, approval/export state, and pair session, then runs normal
  resolution over the remaining media.
- A video whose selected audio was removed remains as a newly resolved,
  ambiguous, or no-match row; removing audio does not delete the video row.
- Existing stability rules must not preserve a selection whose file no longer
  exists.

Also add a clearly visible `Clear Session...` button in the media-pool/session
controls. It confirms with counts of media files, couples, and approved exports,
then stops active work, releases every pooled track, clears queue/export state,
removes the persisted restore record, and returns to the initial import screen.

### Acceptance criteria

- Multiple media entries can be selected and removed in one action.
- Removing unused media happens immediately without a confirmation dialog.
- Removing referenced media confirms with the number of affected couples and
  approved rows.
- Active preview, audition, import, resolve, and sync work touching removed
  entries stops cleanly.
- No couple, candidate, selection, or persisted queue record retains a removed
  content key.
- Couples affected by audio removal equal the result of freshly resolving the
  remaining pool, with stale pair-specific state cleared.
- Removing selected audio leaves the video row newly resolved, ambiguous, or
  unmatched rather than deleting it.
- A visible `Clear Session...` button returns the app to the initial import
  screen and prevents the cleared queue from being offered for restore.
- Source files and previously exported files are untouched.
- The cleared state is persisted and does not reappear on relaunch.

### Implementation notes

Realizes the §V2.7 planned remove-media feature (the selection set was built for
it); doc updated there + a PROGRESS 2026-06-15 entry. No Rust/core change.

The "what should this removal do?" / "what are the clear counts?" decisions are a
PURE `CoupleKit/MediaRemovalPolicy` (style-matched to `RepairDropPolicy`/
`PoolSelection`): `plan(selectedKeys:poolKeys:couples:)` → `MediaRemovalPlan`
(removable keys after intersecting the live pool, `needsConfirmation`, and the
affected-couple / affected-approved counts) and `clearCounts(...)` →
`ClearSessionCounts`. A couple is AFFECTED only by its video (row removed) or its
SELECTED audio (row re-resolves) — a removed non-selected alternate is not a
consequence. So the trash button and the Delete key on the same selection decide
identically.

Removal == "imported without that file": `AppModel.performMediaRemoval` stops work
touching the removed keys (`preview.stop()` when the removed entry is the pool-preview
primary; `pair.teardown()` + `audition.teardown()` for affected couples), resets each
row whose selected audio vanished to a clean unresolved state (selection / pin /
forced / approval / provenance / stale candidates dropped), drops removed-video rows,
frees the pool handles (`MediaPool.remove` — releases the cache-GC pin, never a
synchronous purge; source untouched), then re-runs the SAME `resolveQueue()` the
importer uses so affected videos regenerate exactly as a fresh import would. Selection
+ persistence are pruned (`prunePoolSelection` + `saveQueue`) so no couple, candidate,
selection, or persisted queue record retains a removed key. Unused media skips the
dialog; a referenced removal shows ONE consequence confirmation (RootView). Clear
Session (`confirmClearSession`) stops work, releases every pooled track, clears queue/
export state, and removes the restore record (`QueueStore.clear()`), returning to the
LoadSurface; the empty-session `save` guard keeps a teardown save from recreating it.

Verified: pure `MediaRemovalPolicyTests` (9 — zero/partial/full reference, immediate
vs confirm, counts, removed-alternate-not-affected, stale-key dropping, cross-call-site
parity, clear counts). App-layer end-to-end via the env-gated `COUPLE_REMOVE_TEST`
DebugDrive hook over a real `COUPLE_PAIR_DIR` (audio removal re-resolves + drops the
key everywhere; video removal drops its row; Clear Session empties the model + removes
the restore record). `swift build` clean; `swift test` 69 passed (was 60); `git diff
--check` clean.

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

**UI relocated (2026-06-15, media-pool redesign):** the per-row play button + the
inline expanding scrubber were removed; this controller is now driven by the pool's
**sticky bottom player** following the single selected primary (the per-row
preview is superseded — see UX-DECISIONS §V2.7 and PROGRESS 2026-06-15). The
controller/state-machine CORE is retained and reused; it additionally gained a
load-paused path (SELECT = load without auto-play) and now plays VIDEO entries'
embedded audio. Still Done.

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

**UI relocated (2026-06-15, media-pool redesign):** the per-row `info.circle`
glyph was removed; the same `MediaMetaPopover` (rendering the retained
`MediaMetaCard`) is now opened from the **sticky bottom player's** info button,
re-targeted to the selected primary entry (per-row info is superseded — see
UX-DECISIONS §V2.7 and PROGRESS 2026-06-15). The projection CORE
(`MediaMetaCard` / `MediaMeta.swift`) is unchanged and still covered by
`MediaMetaTests`. Still Done.

---

## TODO-005: Add a unified long-running activity indicator

**Status:** Done
**Type:** Feature  
**Area:** Import, analysis, pairing, sync, export

### Motivation

Several operations perform meaningful background work, but feedback is scattered
across a header spinner, local text, and per-couple states. Users need a clear
answer to "is the app working, and what is it doing?"

### Current behavior

Core jobs expose useful phase/fraction updates for import, sync, and export.
Media import stores a phase label and percentage. Per-couple sync reports
`Searching`, `Refining`, `Measuring drift`, and `Fitting` phases. Batch export
shows batch and per-row progress.

Pairing is the weak spot. `AppModel` currently exposes only a `resolving`
Boolean, rendered as a tiny indeterminate spinner in the queue header. The
resolver runs off the main actor, but its graph-adjusted candidate-matrix call is
one synchronous core/FFI operation with no progress callback or cancellation.
That call may prepare fingerprints, build one batch index, compare the media
graph, refine candidate mappings, and then resolve queue rows, but the shell
cannot currently tell which phase or pair is active. A percentage displayed
today would therefore be fabricated.

### Proposed solution

Place a compact global activity strip directly beneath the counts/approval text
in the queue/session header, replacing the current resolver-only spinner. This is
the preferred location because import, pairing, queued sync, and export are
session-level work, and the header remains visible while either queue list
scrolls. Keep the existing inline import progress on the initial empty import
screen, since the queue header does not exist yet.

Alternatives considered:

- An app-wide strip above the footer would be visible everywhere, but it is far
  from the session controls and consumes valuable center/export width.
- Context-only indicators beside the media pool, selected couple, and export
  button are useful secondary details, but relying on them alone preserves the
  current fragmented experience.
- Toasts or overlays are inappropriate for continuous progress because they
  obscure work and are easy to miss.

Show the header strip only after an operation lasts roughly 200-300 ms so quick
tasks do not flash. Use a determinate bar when the underlying operation reports a
trustworthy fraction and an indeterminate bar otherwise. Existing local
indicators should remain when they add detail; the header is a summary rather
than their replacement.

For pairing, implement honest progress in two levels:

1. With the current API, show an indeterminate summary such as
   `Analyzing 12 files for matches...` or
   `Comparing 4 videos with 8 audio files...`, followed by
   `Resolving matches...`. Include file/pair counts but no percentage.
2. If pairing duration warrants finer feedback, add real progress reporting
   through core, FFI, and `CoupleKit` rather than simulating a Swift loop.
   Useful phases are `Preparing fingerprints (x/y)`, `Building match index`,
   `Comparing files (x/y)`, `Refining candidates (x/y)`, and
   `Resolving video rows (x/y)`. A cancellable job API would be preferable if
   this path becomes long enough to need determinate progress.

Recommended text states:

- `Scanning files...`
- `Inspecting 3 of 12: filename.mov`
- `Importing 3 of 12: filename.mov`
- `Decoding audio - 42%`
- `Building waveform - 68%`
- `Analyzing transients - 91%`
- `Analyzing 12 files for matches...`
- `Comparing 4 videos with 8 audio files...`
- `Preparing fingerprints - 7/12`
- `Refining candidates - 3/9`
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
- During a populated session, the summary activity strip lives in the
  queue/session header and remains visible while the queue or media pool scrolls.
- The initial import screen retains appropriate inline progress before the
  session header exists.
- Text identifies both the operation and the current file/couple when useful.
- Determinate progress is monotonic within a phase.
- Indeterminate work does not display a fake percentage.
- Pairing shows useful media/pair counts with indeterminate progress until the
  underlying API exposes real completed/total updates.
- Multiple queued operations show an item count or queue position.
- The indicator clears on success, cancellation, and failure.
- Failure transitions to an actionable error instead of leaving a stuck bar.
- Existing cancellation controls remain available for cancellable jobs.

### Implementation notes

The four scattered signals are unified into ONE honest strip beneath the
counts/approval text in the queue header (`QueueRail.ActivityStripView`),
REPLACING the resolver-only `resolving` spinner (consistent with §1.2; PROGRESS
2026-06-15 UX-CHANGE notes the unification). LoadSurface keeps its own inline
import progress (the header doesn't exist on the empty screen).

The "current signals → single summary (text + optional fraction)" decision is a
PURE `CoupleKit/ActivitySummary.swift` (`ActivitySummaryPolicy.summarize`,
mirroring MatchQuality/RepairDrop/MediaRemoval/AnchorProposal). It takes an
`ActivitySignals` snapshot + `elapsedMs` + a per-phase `phaseFloor` and returns
one line, or nil. HONESTY is enforced there: a determinate `fraction` is returned
ONLY for import (core job phase %) and export (per-row %); pairing is ALWAYS
indeterminate — counts only (`Comparing V videos with A audio files…` /
`Analyzing N files for matches…` → `Resolving matches…`), never a percentage,
no simulated loop (Level-2 core pairing progress deliberately NOT implemented).
Queued sync is summary-level indeterminate with position (`Syncing couple X of
N: video.mov`); the per-couple `syncActivity` line keeps the phase detail.
Determinate fractions are MONOTONIC within a phase (clamped up to `phaseFloor`)
and clamped to 0…1. A `.failure` is shown immediately (skips the debounce) as an
error with no bar; idle ⇒ nil (no stuck bar on success/cancellation/failure).
Priority when several run: failure ▸ export ▸ import ▸ pairing ▸ sync.

The app side is a thin `@Observable ActivityStrip` controller (Couple target)
mirroring PoolPreviewController-over-PoolPreviewState: it owns the ~250 ms
debounce TIMER + the per-phase floor and publishes `summary`, fed by
`activity.refresh()` from the EXISTING change points — MediaPool's new structured
`importActivity` (+ `onImportChange`), `AppModel.resolving`, the §5.2 sync
queue pump (new observable index/total/name), and `BatchExport.phase`/per-row
progress (+ `runningRowFraction`). No new jobs; no Rust/core change.

Verified: pure `ActivitySummaryTests` (11 — idle⇒nil, debounce window,
pairing⇒indeterminate+counts, import⇒determinate+phase text, queued sync
position, export determinate+preflight/finishing edges, failure⇒error not bar,
monotonic-within-phase clamp, 0…1 clamp, priority, hasActiveWork). Live
controller (debounce timer + floor carry-forward + failure→error→clear) via the
env-gated `COUPLE_ACTIVITY_TEST` DebugDrive hook (11 passed). `swift build`
clean; `swift test` 92 passed (was 81); `git diff --check` clean.

---

## TODO-009: Keep match quality independent from manual source selection

**Status:** Done
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

A manual choice remains questionable unless the selected video/audio pair
independently satisfies the same confident-match classifier used for an
automatic confident pairing. Re-run or reuse that pair's evidence after manual
selection; promote it to green only when that evidence clears the normal
confident threshold.

Manual choice, successful auto-sync, low alignment RMS, or user approval must
never promote match quality by themselves. Alignment quality can remain green in
its own fit display while pairing quality stays amber.

### Acceptance criteria

- Clicking a candidate does not by itself change an amber/questionable verdict
  to green.
- Green means the selected video/audio pair independently meets the confident
  matching criteria used for automatic confident matches; manual and automatic
  selections use the same evidence standard.
- Below-floor forced pairs remain visibly questionable.
- Pairing verdict color, alignment fit, and approval state use distinct UI
  signals and cannot overwrite one another.
- Manual selection still creates a usable session and permits deliberate
  approval/export.
- Persistence restores selection provenance and match quality correctly.
- Tests cover ambiguous manual selection, confident automatic selection,
  below-floor forcing, and evidence-based promotion after re-analysis.

### Implementation notes

Bug mechanism: `CoupleRow.verdict` returned `.good` for ANY non-nil
`selectedAudioKey` (after the `forced` check), so a manual pick via `repair`
(which set `selectedAudioKey` but left `state == .ambiguous`) flipped the dot
green from the click alone — conflating selection with match evidence.

Fix models the three signals separately. The verdict decision is now a PURE,
testable `MatchQualityPolicy.decide` in `CoupleKit/MatchQuality.swift` (mirroring
the `RowMergePolicy`/`PoolSelection` idiom) over `forced`, a new
`SelectionProvenance` (`.none`/`.automatic`/`.manual`), the resolver's own
`RowState.verdict`, and a manual pair's evidence verdict. Rules: `forced` ⇒
questionable; no selection or an AUTOMATIC selection ⇒ the resolver verdict (the
resolver already classified that pair); a MANUAL pick ⇒ green ONLY when that exact
pair independently cleared the SAME confident classifier, else questionable.
Approval and alignment RMS are not inputs. `CoupleRow` gained
`selectionProvenance` + `manualMatchConfident: Bool?` and maps the policy's
`MatchQuality` 1:1 to the UI `Verdict`.

Evidence reuses the EXISTING per-pair path (no new matching math, invariant 8):
`Resolver.manualPairIsConfident` returns core's `PairEvidence.classification ==
.match` via `matchPair` under the fingerprint-primary default, or the fine
verifier's `VerifyFit.matched` via `verifyBatch` (seeded with the `pairCorrelate`
coarse lag, since the legacy verifier only locks within ±span of the seed) under
`COUPLE_MATCHER=legacy`. `AppModel.repair` sets `.manual`/nil then computes
evidence off-main (the only thing that greens a manual pick — never the click,
sync, or approval); `mergeResolution` resets an adopted row to
`.automatic`/`.none` and clears stale evidence; restore round-trips both new
fields (added OPTIONAL to `SavedQueue.Couple`, schema still `couple-queue/1`,
back-compat: a pre-009 file infers provenance from `pinned` and recomputes a
manual pick's evidence via `reevaluateManualMatch`).

Verification: pure `MatchQualityTests` (6 tests) cover the four required cases
(ambiguous manual stays questionable, confident automatic green, below-floor
forced questionable, evidence-based promotion) plus no-selection fallback and
approval/fit independence. App-target end-to-end via DebugDrive
`COUPLE_QUALITY_TEST=1` (7 checks on the real `CoupleRow.verdict`, passing under
both matchers). `swift build` clean; `swift test` 51 passed (was 45);
`git diff --check` clean. Most likely a bug-fix toward the existing §V2.3
decision (which already says quality is evidence-based), so no spec/decision doc
change.

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

## TODO-011: Keep automatically proposed anchors inside true overlap

**Status:** Done
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

Automatic anchors must always lie within the current video/external overlap. The
threshold is based on usable overlap duration, not merely the camera clip's total
duration:

- For overlap of at least 30 seconds, retain the existing default of three
  zoned anchors selected from useful waveform events.
- For usable overlap shorter than 30 seconds, default to two anchors.
- For no usable overlap, propose no automatic anchors and surface the manual
  rescue state.

The two short-overlap anchors should be chosen for human readability, not placed
at arbitrary fractions or midpoints. Use the existing onset ranking as the
primary proxy for a clear shape, then prefer candidates that:

- contain a strong, isolated transient or distinctive local contour;
- contain non-silent, non-degenerate structure in both mapped waveform lanes;
- avoid flat silence and sustained/periodic passages that are visually
  ambiguous;
- are well separated in time, preferably one in the earlier part and one in the
  later part of the overlap.

Choose the best pair of usable candidates while maintaining at least roughly
25% of the overlap between them. If two non-degenerate candidate windows cannot
be found, propose no automatic anchors rather than inserting misleading
midpoints. This preserves the core invariant of either zero or 2-4 anchors.

This constraint applies to automatic proposal and automatic `+ anchor`
selection. It does not prevent a deliberate manual anchor move, though the UI
should make an out-of-overlap anchor visibly non-evidentiary.

### Acceptance criteria

- Every automatically proposed anchor is inside the current overlap.
- Automatic `+ anchor` never chooses an onset outside the overlap.
- Overlap of at least 30 seconds defaults to three zoned anchors.
- Usable overlap below 30 seconds defaults to two well-separated,
  human-readable waveform events.
- Short-overlap anchor selection rejects silence/degenerate windows and avoids
  arbitrary midpoint fallback.
- If two useful windows cannot be found, the state uses zero anchors rather than
  an invalid one-anchor or misleading two-anchor set.
- No-overlap state avoids invalid measurements and explains why anchors cannot
  be proposed.
- Tests cover full, partial, short, and zero overlap.

### Implementation notes

Bug mechanism: `PairSession.proposeAnchors()` set `span = 0...cam.durationS` and
replaced it with the overlap ONLY when `overlap != nil && len > 30`, so a ≤30 s or
absent overlap took the three zones over the FULL camera clip (placing anchors where
the external source has no content; `bestOnset(in:)` even fell back to a raw zone
MIDPOINT). `addAnchor()` shared the same `overlap ?? 0...cam.durationS` fallback.

Fix bounds all automatic paths to the overlap via a PURE
`CoupleKit/AnchorProposal` (mirrors `MatchQualityPolicy`/`RepairDropPolicy`/
`MediaRemovalPolicy`): `propose(overlap:candidates:)` → anchor times and
`addAnchorCandidate(overlap:existing:candidates:)` → one time/nil, over the overlap
span + ranked onset `Candidate`s (`t`, `score`, `usable`). It enforces the count on
USABLE OVERLAP: ≥30 s ⇒ 3 zoned ([1,30]/[38,62]/[70,99]% of overlap); <30 s ⇒ 2
separated (≥25% of overlap) non-degenerate; else 0; "+ anchor" never outside the
overlap; never exactly one. `PairSession` reuses the existing engine onset list
(`TrackHandle.onsets`) and marks `usable` from an `rms()` probe of BOTH lanes (camera
at the onset, external at `u(t)`, ≥ −55 dBFS) — no new matching math (invariant 8).
WaveformStage shows a "place anchors manually" chip when none were placed
(`anchorsUnavailable`); a deliberate MANUAL drag may still leave the overlap.

Verified: pure `AnchorProposalTests` (12 — full ⇒ 3 in-overlap; partial shorter than
camera ⇒ 3 still in-overlap; short ⇒ 2 separated; zero/degenerate/one-usable/bunched/
long-all-degenerate ⇒ 0 never 1; strongest-outside ignored; "+ anchor"
inside/separated/cap/nil; count-never-one sweep). Live path (onsets + per-lane RMS,
overlap shrunk <30 s and to zero) via env-gated `COUPLE_ANCHOR_TEST` DebugDrive hook
over a real `COUPLE_PAIR_DIR`. `swift build` clean; `swift test` 81 passed (was 69);
`git diff --check` clean. UX-DECISIONS §2.2 updated (removes the camera-duration
fallback) + PROGRESS 2026-06-15 SPEC-CHANGE. No Rust/core change.

---

## TODO-013: Allow drag-to-pair over the central workspace

**Status:** Done
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

Keep replacement rules deliberately simple:

- Dropping the already selected audio is a no-op.
- An unresolved couple accepts the drop immediately.
- An unapproved couple with a different source replaces it immediately without a
  confirmation dialog. The old audio remains in the pool and can be selected
  again.
- An approved couple asks for confirmation because replacement removes it from
  the export queue. The dialog states that approval/export status and the current
  pair session will be cleared. On confirmation, replace the source; on cancel,
  make no change.

Every accepted replacement selects and pins the audio, clears stale export
status, rebuilds the pair session, and runs full sync/classification. Pair-specific
autosaves for the previous video/audio combination may remain available if that
source is selected again; this action must not delete source files or cache data.

Use the same underlying repair operation as the existing queue-row target so the
two gestures cannot diverge, including the approved-row confirmation. Match
color must follow TODO-009: a forced selection does not become green merely
because the drop succeeded.

### Acceptance criteria

- Dragging an audio pool entry over the selected center workspace shows a clear
  valid-drop highlight.
- Dropping on any major central subregion invokes the same repair path.
- No selected couple, a non-audio payload, or an unavailable video rejects the
  drop without changing state.
- The new source is selected/pinned, the session is rebuilt, and sync runs.
- Replacing an unapproved source is immediate; dropping the current source is a
  no-op.
- Replacing an approved source requires confirmation and, when confirmed, clears
  approval and stale export status before rebuilding.
- Queue-row and center-workspace drops produce identical pairing results.
- Pairing verdict remains evidence-based as specified by TODO-009.

### Implementation notes

Realizes the §V2.6-DECIDED gesture (drag pool audio onto a couple) over the center
workspace, with NO doc change (an existing decision).

Pre-existing state characterized first: the queue-row drop
(`QueueRail.CoupleRowView`) called `model.repair(couple, audioKey:)` DIRECTLY, and
`AppModel.repair` had **no** approved-couple confirmation and did **not** clear
approval/export — so confirmation lived in NEITHER the view nor the model. TODO-013
therefore had to introduce the shared confirmation/clear path, not just reuse one.

Shared, non-divergent design (mirrors the RowMergePolicy/PoolSelection/MatchQuality
idiom). The "what should this drop do?" decision is a PURE
`CoupleKit/RepairDropPolicy.decide` → `RepairDropOutcome` (`noOp`/`accept`/
`needsConfirmation`/`reject`) over (selected-couple?, payload-is-audio?,
video-available?, current-vs-dropped key, approved). BOTH drop targets call ONE app
seam, `AppModel.requestRepairDrop(couple, audioKey:)`, which maps the outcome:
reject ⇒ false (no change); no-op ⇒ true; accept ⇒ `performRepairReplace`;
needs-confirmation ⇒ arm `pendingRepairConfirmation`. `performRepairReplace` clears
stale approval (`pair.toggleVerified` when approved — removes it from the export
queue) + `exportStatus = .idle`, then calls the SAME `repair` (selects+pins,
rebuilds the session, runs full sync + the TODO-009 evidence classifier). The
approved confirmation is one `.confirmationDialog` in `RootView` (shared ancestor),
stating that approval/export status and the current pair session will be cleared;
confirm ⇒ `confirmPendingRepair` (re-validates via the same policy), cancel ⇒
`cancelPendingRepair` (no change). The queue-row drop was switched from calling
`repair` directly to `requestRepairDrop`, so it now ALSO gets the no-op/confirm/clear
behavior — the two gestures are byte-identical by construction.

Center target: `CenterColumn` extracts its stack into `content` and attaches ONE
`CenterDropTarget` ViewModifier spanning the whole region (video preview + resolve
banner + waveform row + sync-tools/bottom bar). It is scoped to the selected couple
(nil couple ⇒ no `.dropDestination` attached ⇒ inert region). The valid-drag
highlight uses the app's ACCENT language (soft accent wash + accent stroke + a panel
chip), never a yellow box; copy reads `Pair selected video with <name>` using the
dragged file's display name via a lightweight `AppModel.poolDraggingKey` recorded at
drag start (the `.draggable` payload closure) — cosmetic only, the repair always
reads the dropped payload. Verdict stays evidence-based: `repair` never greens the
dot on drop (TODO-009 unchanged).

Verification: pure `RepairDropPolicyTests` (9 tests) cover same-audio no-op,
unresolved immediate, unapproved-different immediate, approved needs-confirmation,
and the three rejects (no couple / non-audio / unavailable video) + cross-call-site
input parity. App-layer end-to-end via DebugDrive `COUPLE_REPAIRDROP_TEST=1` (7
checks on the real `requestRepairDrop`/`payloadIsAudio`/confirmation state machine,
ran live: 7 passed). `swift build` clean (only the pre-existing `VideoThumbnail`
`NSImage`-Sendable warnings); `swift test` 60 passed (was 51); `git diff --check`
clean. This item touched no Rust/core.

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
