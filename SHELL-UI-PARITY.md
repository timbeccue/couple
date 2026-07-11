# SHELL-UI-PARITY — required affordances per surface

Why this exists (2026-07-04): the engine↔shell contract is fully specced
(SPEC §4/§13), but the SHELL UI surface was only implicitly specced by the
Swift views — porting the Electron shell showed that "read the Swift code" is
not a reliable transfer mechanism (surfaces got invented, dropped, or
restyled). This file is the CHECKLIST a new shell (or a change to an existing
one) is audited against. It intentionally lists WHAT must exist, not how it
looks — chrome stays shell-idiomatic (SPEC §13.3); semantics and presence are
required. Each line cites the authoritative implementation. Keep this file
current: adding/removing a user-facing affordance in any shell means updating
the matching line here IN THE SAME CHANGE (same rule as CLAUDE.md upkeep).

Legend: [source] = the reference implementation a port must read.

## Queue rail (left)
- Session header: app title, media counts ("N video · M audio"), "Approve to
  queue · a/b approved" status line, visible "Clear Session…" control (disabled
  mid-export and when the session is empty). [QueueRail.swift header]
- Three live sections QUEUED FOR EXPORT / TO APPROVE / TO FIX with counts;
  membership = approved / (¬approved ∧ verdict good) / rest. [AppModel.swift
  queued/toApprove/toFix]
- Couple row: evidence verdict dot (match-quality policy; pulsing while
  syncing), video identity, the SELECTED PAIRING visible, action word when
  action is needed ("pick one" / "no audio" / "forced"). [QueueRail.swift,
  Couple.swift verdict/actionWord]
- Pool-audio drag onto a couple row = manual re-pair via repair-drop policy,
  with hover affordance. [PoolController.swift / RepairDropPolicy]

## Media pool (rail bottom)
- Bottom-pinned box: header (count, trash, add), bounded list, STICKY footer
  preview player of constant height ("Select a file to preview" placeholder).
  [QueueRail.swift MediaPoolSection; 2026-06-15 UX decision]
- Rows: role identity (camera teal / external amber — fixed semantic colors,
  SPEC §13.3), video thumbnail (glyph fallback), duration, live usage line,
  ⓘ metadata popover (probe-derived only, never re-probed). [MediaMeta.swift]
- Finder multiselect: plain replaces / ⌘ toggles / ⇧ range; primary drives the
  preview. [PoolSelection.swift]
- Preview player: play/pause, waveform seek strip, elapsed/total (probe
  duration is authoritative), scrub never fights ticks. [PoolPreviewController.swift]
- Removal: Delete key + trash → media-removal policy plan (counts in the
  confirmation), teardown ordering, survivors re-resolve. [PoolController.swift]

## Center column (editor)
- Empty session: NO separate splash/launch screen — the full three-column shell
  renders from first launch, with the import surface (drop zone with hover
  highlight, "Choose files…" picker, one-line orientation to the rails,
  importing spinner) taking the hero slot; rails show their dimmed empty
  sections around it. [LoadSurface.swift, RootView.swift, CenterColumn.swift;
  UX-DECISIONS 2026-07-11]
- Video hero (preview-greedy layout): muted video slaved to the AUDITION CLOCK
  (audio is master; ≤1 frame skew), transport (play/pause, timecode, anchor
  jump chips with −0.8 s pre-roll, monitor mode Pan L/R / Overlay (Null
  excluded from the UI — CoupleKit MonitorMode.allCases; default Overlay),
  fullscreen).
  [AuditionController.swift, EditorComponents.swift]
- Resolve banner when unresolved/alternates: candidate list with evidence (z),
  re-pick = manual pin via forced-verdict policy. [CenterColumn.swift §V2.6]
- Waveform: ONE zoomable dual-lane strip (no separate minimap) owning anchor
  pins + playhead; Fit restores union; overlap chip; + anchor / lock-zoom /
  loupe-layout toolbar. [WaveformStage.swift OverviewStrip]
- Per-lane audio controls beside each lane: mute (remembers pre-mute level) +
  0–2 gain slider tinted to the lane; lane drawing scales with gain, dims when
  muted; same state as any other gain surface. [WaveformStage.swift
  LaneAudioControls + drawLane]
- Editing: drag external lane = offset, shift-drag = pivot drift (commit on
  release), camera lane pans, wheel zooms, ←→/[ ] nudge families, esc restores
  drag-start alignment; gated ONLY by inputsEnabled; editing never un-verifies.
  [WaveformStage.swift, InputRouter.swift]
- Anchors: auto-proposal + "+ anchor" via anchor policy, onset snap, drag to
  move; loupes on demand (◫): additive blend, local Δ readout, ⇥ align,
  pivot-about-anchor. [LoupeView.swift, AnchorProposal usage in PairSession.swift]
- FIT-QUALITY SPARKLINE always visible with the sync readouts: measured-span
  x-domain (m:ss corner labels), accepted lag points, rejected-window baseline
  ticks, fit line, caption `fit RMS <x> ms · <a>/<b> win[ · offset-only]`.
  [SparklineView.swift; PROGRESS 2026-07-01]
- Sync tools (⚙): offset/drift readouts + nudges, drift-mode toggle
  (resample/none), monitor + gains; Approve / re-sync always reachable.
  [CenterColumn.swift]

## Export rail (right)
- Destination picker (auto-tracks last import dir until user pins one), global
  Auto/MOV/MP4/WAV + keep-camera-audio (disabled for WAV), per-row editable
  stems (default `<stem>_synced`, reset affordance), Export(n) with per-row
  status, disk preflight, serial continue-past-failure, end toast with Reveal.
  [ExportRail.swift, BatchExport.swift]

## App-level
- Whole-window drop target + native file picker fallback; window is draggable
  (hidden titlebar needs an explicit drag region on non-mac frameworks).
- Keymap: j/k select, v approve (undoable), space/l play, esc, ⌘Z/⇧⌘Z, ⌘⌫
  clear session (policy counts in the confirm). [InputRouter.swift]
- Unified import→match→sync activity strip composed by the activity policy
  (shell holds clocks/debounce; never sticks at 100%). [ActivityStrip.swift]
- Persistence: couple-queue/1 + couple-session/1 in the platform app-data dir,
  autosave on the mac app's triggers, launch restore toast → saved alignments
  apply WITHOUT re-sync. [QueueStore.swift, SessionStore.swift]
- Toasts: skipped-files, batch summary; confirmation dialogs for destructive
  acts (replace approved pairing, removal, clear session).
