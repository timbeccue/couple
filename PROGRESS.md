# PROGRESS.md — Couple

Append-only log. Each session adds a dated entry. Spec deviations are recorded as
`SPEC-CHANGE:` lines with rationale (see SPEC.md §0 for the protocol).

> Entries from 2026-06-11 through 2026-06-14 (planning, Sessions C–H, matching
> prework, spectral-fingerprint boundaries 1–13) are archived VERBATIM in
> `docs/archive/PROGRESS-2026-06-11-to-14.md`. Their `SPEC-CHANGE:` entries remain
> binding — search both files when checking the ledger.

## 2026-06-15 — Media-pool redesign (sticky bottom player + Finder selection)

Refactored the media pool's interaction model (the uncommitted TODO-002 preview +
TODO-003 metadata work) into a sticky bottom player driven by a Finder-style
selection set. Builds ON — does not rewrite — `PoolPreviewController` and
`MediaMetaPopover`; the matching/fingerprint work above is untouched.

- **UX-CHANGE (owner): per-row inline preview/info removed → sticky footer player.**
  Each pool row had a play icon, an info (ⓘ) glyph, and an inline seek bar that
  expanded the active row. Those per-row affordances are gone. Rows are now plain
  selectable items; a single **sticky transport bar pinned to the bottom of the
  pool box** (seek + elapsed/total + play/pause + ⓘ) follows the selection. The bar
  is always visible with a disabled "Select a file to preview" placeholder, so the
  box height is constant. (Supersedes the UX-DECISIONS §V2.7 per-row inline preview;
  documented there as the 2026-06-15 revision.)
- **UX-CHANGE (owner): pool selection model (TODO-001 foundation).** Pool clicks are
  Finder-style multi-select — plain replaces, ⌘ toggles one, ⇧ selects a contiguous
  range over the displayed order — while the sticky player follows a single
  **primary** (the last-clicked entry). The multi-select *set* is built so the
  future remove-media feature (TODO-001) reuses it verbatim; removal is NOT
  implemented now. The pure transition is `CoupleKit/PoolSelection.swift`
  (style-matched to `RowMergePolicy`), unit-tested under `swift test`
  (`PoolSelectionTests`, 12 cases) and mirrored live by the additive DebugDrive
  `COUPLE_POOLSEL_TEST` hook.
- **UX-CHANGE (owner): SELECT = load-paused.** Clicking a file loads it into the bar
  and shows its duration but does not auto-play; the user presses play. Implemented
  as an additive `PoolPreviewState.loadPaused` transition + a `play` flag threaded
  through the existing `.load` action (the toggle path keeps auto-playing); the
  controller's stop/observers/end-handling/teardown are unchanged.
- **UX-CHANGE (owner): video entries are previewable** — the bar plays the movie's
  embedded audio through the same `PoolPreviewController` (`entry.url` → AVPlayer).
- **UX-CHANGE (owner): independent scroll regions.** The couples list and the pool
  box now scroll independently (the pool items scroll inside a bounded region with
  the sticky bar as a non-scrolling footer); the old shared rail ScrollView +
  Spacer/minHeight bottom-pin trick is removed. Traffic-light clearance preserved.
- **Space stays the audition transport** (no pool key binding): pool play/pause is
  button-only this version, and no arrow-key pool navigation was added (the
  selection model is left ready for it). The global keyCode-49 handler in AppModel
  is unchanged.

TODO-002 / TODO-003 remain Done — their controller + metadata-projection cores are
retained and reused; only their UI was relocated into the sticky bar.

### Tests
`cd mac && swift build && swift test` — clean build (only the pre-existing
VideoThumbnail/libcouple.a warnings), **45 ✓** (was 34: +12 `PoolSelectionTests`,
covering plain/⌘/⇧, anchor stickiness, removing the primary → re-home or nil, and
off-list/empty edges). The preview load-paused state machine (app target, so not
reachable from the CoupleKit-only test bundle — same split as the existing
`COUPLE_PREVIEW_TEST`) is covered by `COUPLE_POOLSEL_TEST`.

## 2026-06-15 — Remove imported media + Clear Session (TODO-001)

Realized the planned remove-media feature that the §V2.7 selection set was built
for, plus a Clear Session reset. No Rust/core change.

### What shipped
- **Per-file removal.** When ≥1 pool entry is selected, a **trash button** appears
  in the media-pool header and the **Delete / forward-Delete key** removes the WHOLE
  selection in one action. **Unused** media (referenced by no couple) is removed
  IMMEDIATELY; a selection that touches a **referenced** file shows ONE consequence
  dialog stating the affected-couple + approved-row counts (never one per file).
  Removal affects only the open session and NEVER deletes source media from disk
  (freeing a track handle only makes its cache GC-eligible; no synchronous purge).
- **Removal == "imported without that file".** Rather than hand-editing couples, the
  app stops work touching the removed keys (`preview.stop()` if the removed entry is
  the pool-preview primary; `pair.teardown()` + `audition.teardown()` for affected
  couples), resets each row whose SELECTED audio vanished to a clean unresolved state
  (selection / pin / forced / approval / provenance / stale candidates dropped),
  drops removed-video rows, frees the pool handles, then re-runs the SAME
  `resolveQueue()` the importer uses — so affected videos regenerate exactly as a
  fresh import would (newly-resolved / ambiguous / no-match; a removed audio never
  deletes its video row). Selection/persistence are pruned (`prunePoolSelection`,
  `saveQueue`) so **no couple, candidate, selection, or persisted queue record
  retains a removed content key**. Removing the last file empties the queue and wipes
  the restore record.
- **Clear Session.** A clearly-visible **`Clear Session…`** button in the queue/
  session header confirms with media / couple / approved counts, then stops active
  work, releases every pooled track, clears queue + export state, **removes the
  persisted restore record** (`QueueStore.clear()`), and returns to the LoadSurface.
  The cleared state does not reappear on relaunch (the record is gone and the
  empty-session `save` guard never re-writes it). Disabled while a batch export runs.
  Source files and previously exported files are untouched.

### Design (mirrors the repo's pure-decision idiom)
The "what should this removal do?" / "what are the clear counts?" decisions are a
PURE `CoupleKit/MediaRemovalPolicy` (`plan` → `MediaRemovalPlan`; `clearCounts` →
`ClearSessionCounts`) over the selection set + live pool keys + the couples'
`(videoKey, selectedAudioKey, approved)` — so the Delete key and the trash button on
the same selection decide identically, and a couple is "affected" only by its video
or its SELECTED audio (a removed non-selected alternate is not a consequence). Both
confirmations live in `RootView` (shared ancestor), alongside the TODO-013 dialog.

### Docs
- UX-DECISIONS §V2.7 REVISED note updated: the selection set's "removal itself is not
  in this version" is superseded — removal now exists (this entry).

### Tests
`cd mac && swift build && swift test` — clean build (only the pre-existing
VideoThumbnail/libcouple.a warnings), **69 ✓** (was 60: +9 `MediaRemovalPolicyTests`
— zero/partial/full reference, immediate-vs-confirm, affected-couple/approved counts,
removed-alternate-not-affected, stale-key dropping, cross-call-site plan parity, and
clear-session counts). The stateful app behavior (removing an audio re-resolves and
no record retains the key; removing a video drops its row; Clear Session empties the
model + removes the restore record + a teardown save can't recreate it) is covered by
the env-gated `COUPLE_REMOVE_TEST` DebugDrive hook over a real `COUPLE_PAIR_DIR`
session (mirrors `COUPLE_QUALITY_TEST` / `COUPLE_REPAIRDROP_TEST`; needs the GUI
app-bundle runloop, so it is the owner's live walkthrough — the pure decision carries
the `swift test` coverage). `git diff --check` clean.

---

## 2026-06-15 — Keep automatically proposed anchors inside true overlap (TODO-011)

SPEC-CHANGE (2026-06-15, TODO-011): UX-DECISIONS §2.2 anchor auto-proposal no longer
"falls back to the camera duration when overlap is unknown/short." Automatic anchors
are now **always bounded to the current video/external overlap**, with the count
decided on **usable overlap duration**: overlap ≥ 30 s ⇒ three zoned anchors (the
unchanged [1,30]/[38,62]/[70,99]%-of-**overlap** behavior); 0 < usable overlap < 30 s
⇒ **two** well-separated non-degenerate anchors; no usable overlap (or no two
non-degenerate candidates) ⇒ **zero** anchors + the manual-rescue state. §2.2 updated
(Auto-proposal + Add/remove bullets).

### Bug mechanism (fixed)
`PairSession.proposeAnchors()` set `span = 0...cam.durationS` and only replaced it with
the overlap when `overlap != nil && len > 30`. So a ≤ 30 s **or** absent overlap took
the three zones over the **full camera clip**, placing anchors where the external
source has no content (and `bestOnset(in:)` fell back to a raw zone **midpoint** when
no onset was found). `addAnchor()` had the same `overlap ?? 0...cam.durationS`
fallback. Both could yield out-of-overlap automatic anchors.

### Fix (mirrors the repo's pure-decision idiom)
The "how many anchors, and where?" decision is a PURE
`CoupleKit/AnchorProposal` (`propose(overlap:candidates:)` → anchor times;
`addAnchorCandidate(overlap:existing:candidates:)` → one time or nil), over the
overlap span + ranked onset `Candidate`s (each `t`, `score`, and a `usable` flag =
non-silent/non-degenerate in BOTH mapped lanes). It enforces: ≥30 s ⇒ 3 zoned;
<30 s ⇒ 2 separated (≥25% of overlap) usable; else 0; "+ anchor" never outside the
overlap; **never exactly one** (every returned set is empty or 2–3). `PairSession`
builds the candidates by reusing the existing engine onset list
(`TrackHandle.onsets`) and marking `usable` from an `rms()` probe of the camera lane
at the onset and the external lane at the mapped time `u(t)` (≥ −55 dBFS = not
silence) — **no new matching math** (invariant 8). The waveform stage shows a
"place anchors manually" chip when automatic proposal placed none
(`PairSession.anchorsUnavailable`). A deliberate manual drag may still leave the
overlap (only the automatic paths are bounded).

### Tests
`cd mac && swift build && swift test` — clean build (only the pre-existing
VideoThumbnail/libcouple.a warnings), **81 ✓** (was 69: +12 `AnchorProposalTests` —
full overlap ⇒ 3 in-overlap; partial overlap shorter than the camera ⇒ 3 still in the
overlap, none past external content; short overlap ⇒ 2 separated; zero/degenerate ⇒ 0
never 1; one-usable-candidate ⇒ 0; bunched candidates ⇒ 0; long all-degenerate ⇒ 0;
strongest-outside-overlap ignored; "+ anchor" strongest-in-overlap/separated, never
outside, cap + nil; and a sweep proving the count is never exactly one). The live
`proposeAnchors`/`addAnchor` path (engine onsets + per-lane RMS usability, overlap
shrunk below 30 s and to zero) is covered by the env-gated `COUPLE_ANCHOR_TEST`
DebugDrive hook over a real `COUPLE_PAIR_DIR` session (mirrors `COUPLE_REMOVE_TEST` /
`COUPLE_QUALITY_TEST`; needs the GUI app-bundle runloop). `git diff --check` clean. No
Rust/core change.

## 2026-06-15 — TODO-005: unified long-running activity strip

Unified the four scattered progress signals (resolver spinner, import phase/%,
per-couple sync phases, batch export %) into ONE honest activity strip beneath the
counts/approval text in the queue header.

UX-CHANGE (2026-06-15, TODO-005): the resolver-only indeterminate spinner in the
queue header (`AppModel.resolving` → `QueueRail` `ProgressView`) is REPLACED by a
compact global activity strip (`QueueRail.ActivityStripView`). Consistent with
UX-DECISIONS §1.2 (session-level header band) — not a decision reversal, so no
UX-DECISIONS edit; this records the unification. LoadSurface keeps its own inline
import progress (the queue header doesn't exist on the empty screen).

The decision is a PURE `CoupleKit/ActivitySummary.swift`
(`ActivitySummaryPolicy.summarize` over an `ActivitySignals` snapshot + `elapsedMs`
+ a per-phase `phaseFloor`), mirroring MatchQuality/RepairDrop/MediaRemoval/
AnchorProposal. HONESTY is enforced there: determinate `fraction` ONLY for import
(core job phase %) and export (per-row %); PAIRING is ALWAYS indeterminate — counts
only (`Comparing V videos with A audio files…` / `Analyzing N files for matches…`
→ `Resolving matches…`), never a percentage, no simulated Swift loop (Level-2 core
pairing progress deliberately out of scope, no Rust/core change). Queued sync is
summary-level indeterminate with position (`Syncing couple X of N: video.mov`); the
per-couple `syncActivity` line keeps the Searching/Refining/Measuring/Fitting
detail. Determinate fractions are MONOTONIC within a phase (clamped up to the floor)
and clamped to 0…1; a failure surfaces immediately as an actionable error (no bar);
idle clears (no stuck bar on success/cancellation/failure). Priority when several
run: failure ▸ export ▸ import ▸ pairing ▸ sync.

App side: a thin `@Observable ActivityStrip` controller (Couple target,
PoolPreviewController-style) owns the ~250 ms debounce timer + the per-phase floor
and publishes `summary`, fed by `activity.refresh()` from the EXISTING change
points — MediaPool's new structured `importActivity` + `onImportChange`,
`AppModel.resolving`, the §5.2 sync-queue pump (new observable index/total/name),
and `BatchExport.phase`/per-row progress + `runningRowFraction`. No new jobs.

### Tests
`cd mac && swift build && swift test` — clean build (only the pre-existing
VideoThumbnail/libcouple.a warnings), **92 ✓** (was 81: +11 `ActivitySummaryTests`
— idle⇒nil, the ~250 ms debounce window, pairing⇒indeterminate+count text,
import⇒determinate+honest phase verb, queued-sync position text, export
determinate + preflight/finishing edges, failure⇒error not stuck bar,
monotonic-within-phase clamp, 0…1 clamp, priority order, hasActiveWork). The LIVE
controller (debounce timer + per-phase floor carried across `refresh()` + failure→
error→auto-clear + idle-clear) is covered by the env-gated `COUPLE_ACTIVITY_TEST`
DebugDrive hook (media-free; drives the real controller via a `debugSignalsOverride`
seam — 11 passed). `git diff --check` clean. No Rust/core change.

## 2026-07-01 — Boundary polish: forced verdict → core policy; dev harness fenced; log archived

Follow-through on the adopt-and-refactor decision (architecture assessment,
2026-07-01, owner sign-off) after fast-forwarding `main` to `core-shell-refactor`.

- **`forced` verdict relocated** (PLAN-CORE-SHELL-BOUNDARY boundary rule): the
  below-floor rule for a kept manual pairing (`0 < z < z_min`, plus the failed-lock
  promotion after sync) moves from an inline check in `AppModel.repair` to
  `couple_core::policy::repair_drop::forced` over the ADDITIVE
  `cpl_policy_repair_forced` (`cpl_abi_version` stays 1). Swift wrapper on
  `RepairDropPolicy`; the conservative error fallback is `forced=true`
  (questionable, never a silent green — mirrors MatchQualityPolicy). Behavior
  identical: the post-sync recompute with the same candidate z reduces to the old
  "set on failure, never clear". Core 11 / FFI 2 / Swift 109 tests green.
- **Dev harness fenced**: DebugDrive/DisplayDriver/FrameStats compile only under
  `COUPLE_DEVHARNESS` (dev.sh/build.sh pass it; make-app.sh does not — the shipped
  .app carries stubs). `#if DEBUG` was rejected: the §15 fps criterion is measured
  on `-c release`.
- **PROGRESS.md archived**: 2026-06-11..14 entries moved verbatim to
  `docs/archive/PROGRESS-2026-06-11-to-14.md`; SPEC-CHANGE ledger spans both files.

## 2026-07-01 — Shell refactors: AnchorPoint struct, AppModel façade split; corpus gate enforced

Remaining items from the adopt-and-refactor pass (same owner sign-off as above).

- **PairSession anchor state** → one `AnchorPoint` struct (`t`, `frame`, `measurement`,
  `measurementCenter`) instead of four hand-synchronized parallel arrays; every
  insert/remove/reorder site now moves one element (UX §2.2 chronological order kept).
  Core's persisted AlignmentState keeps its parallel-array wire shape — mapped at the
  pushColdState/restore boundary. Behavior-preserving.
- **AppModel decomposed** (1145 → 503 lines): three owned sub-controllers —
  `IngestController` (import→resolve orchestration, serialized sync queue, the
  activity-strip signal cluster with its synchronous-clear invariants verbatim),
  `PoolController` (Finder-style pool selection, drag state, removal / Clear Session),
  `InputRouter` (NSEvent key/scroll monitors + routing). AppModel stays the façade
  (couples/selection, approve/repair verbs, export settings, persistence) with thin
  forwarding accessors, so views / ActivityStrip / DebugDrive are unchanged.
- **Synthetic-corpus ROC gate enforced in-repo**: a new `#[ignore]`d release test
  scores the committed 33-case synthetic corpus under production defaults
  (FingerprintPrimary, frozen profile 2) and asserts the CALIBRATION.md §6 baseline —
  `confident_false_matches == 0` (§10.5 hard gate), 0 FP, recall ≥ 31/32. Verified:
  passes in 197 s. Run: `cargo test --release -p couple-core --test matching_evidence -- --ignored`.
- `policy/activity.rs` reviewed for decomposition (assessment follow-up): production
  code is ~558 lines of small per-line functions; the 1159-line count was mostly its
  test module. No change needed.

Gates: swift build (ship + COUPLE_DEVHARNESS variants), swift test 109, cargo ffi 18,
corpus gate 1 — all green.

## 2026-07-01 — Feature/hardening pass: TODO-010, TODO-015 undo, CI, media audit, policy properties

Owner-approved improvement pass (items 3/4/6/7/8 of the 2026-07-01 review).

- **TODO-010 done** — the overview strip gets its own visible range: scroll-to-zoom
  around the pointer, horizontal pan, Fit chip + ⌘0; every renderer/gesture maps
  through the visible range; independent of loupe zoom and `lockZoom`; not
  persisted (fresh fit per launch/couple-switch). Interactive feel still wants an
  owner click-through (headless coverage is the viewport math + COUPLE_OVERVIEW_TEST).
- **TODO-015 done (undo)** — ⌘Z/⇧⌘Z for approve, manual re-pair / candidate switch
  (an undone approved-replacement drop restores its approval), and export renames.
  Media removal / Clear Session keep confirmations by design (their inverse is a
  re-import). See TODO-015 for scope + acceptance.
- **CI** — .github/workflows/ci.yml: cargo test --release + both swift build
  variants + swift test on push/PR; the synthetic-corpus ROC gate weekly + on
  dispatch. Dormant until the repo gets a GitHub remote.
- **Real-media audit** — scripts/check-real-media.sh enumerates every gitignored
  recording the local-only suites need (example pair + all non-synthetic corpus
  cases). Currently: all 29 files missing on this machine — restore to re-arm the
  real-data gates.
- **Policy properties** — core/tests/policy_properties.rs: 37 proptest properties
  enforcing the policy modules' documented CONTRACT clauses + wire fixed-point
  stability for every camelCase envelope type.

Gates: swift build ship+devharness, swift test 109, core lib 255 + properties 37,
ffi 18 — all green.

## 2026-07-01 — Retract the real-media audit script (owner decision)

`scripts/check-real-media.sh` removed same-day: the gitignored recordings are owner
convenience material for playing with the app, not a dependency — no test requires
them (the corpus scorer counts absent media as "unscannable"; `realdata_sync` is
`COUPLE_REALDATA_DIR`-gated and SKIPs). An audit that exits 1 over optional files
demanded a state the repo never promised. Restoring media stays optional and
re-arms the extra real-data validation when present.

## 2026-07-01 — Media pool: rail-space growth, waveform seek strip, Space follows focus

Three owner-requested improvements to the queue rail's media pool (revising the
2026-06-15 sticky-player design; UX-DECISIONS §V2.7 updated in place):

- **UX-CHANGE (owner): the pool's bounded list may GROW.** The inner scroll was a
  fixed 168 pt cap even when the couples list left the rail half-empty. The cap is
  now the FLOOR: the list grows into rail height the couples are not using, bounded
  by the pool's own content (no blank inflation), and the couples keep first claim —
  a long couples list sees exactly the old layout. Pure clamp in
  `CoupleKit.PoolRailLayout.listHeight` (+3 tests); `QueueRail` feeds it live
  height measurements (viewport sum is invariant under redistribution, so the
  computation cannot oscillate).
- **UX-CHANGE (owner): the sticky bar's seek slider is now a waveform seek strip**
  (bar 26 → 34 pt). `PoolSeekStrip` draws the primary's whole-file min/max envelope
  via the same core `band()` query + active-P95 normalization as the overview
  lanes, role-colored (amber audio / camera video), played span bright, playhead
  bright-while-playing. The whole strip is a timeline surface — click or drag
  anywhere seeks through the retired slider's exact scrub transitions
  (scrubBegin/scrubTo/scrubEnd). A file whose handle/envelope is not ready shows an
  honest flat baseline, never a fabricated wave.
- **SPEC-CHANGE (owner): Space follows the pool's focus** — supersedes the §V2.7
  2026-06-15 note "pool play/pause is button-only; Space is unchanged". Rationale:
  with the sticky player in daily use, reaching for the mouse to pause a preview
  broke the keyboard triage flow; content-relative keys should drive the transport
  the user's eye is on. Rule: `PoolController.poolFocused` is set by a pool click
  (row select / sticky-bar transport) with a live primary, cleared by a USER couple
  selection (queue tap, export-list tap/rename, j/k, v-advance), the video
  transport button, collapsing the pool, or the primary vanishing. Automated
  selection (post-resolve auto-pick, DebugDrive) never steals a user-placed focus
  (`AppModel.select(_:byUser:)` distinguishes the two). InputRouter routes Space:
  pool-focused ⇒ toggle the sticky player; otherwise the audition — and starting
  the audition from Space pauses a still-playing pool preview, so the keyboard
  path never sounds two transports at once.

Gates: swift build + swift test (CoupleKit suite incl. 3 new PoolRailLayout tests)
green; core untouched.

## 2026-07-02 — SPEC-CHANGE: Space priority reworked after live use (owner)

The 2026-07-01 "Space follows the pool's focus" rule shipped and was immediately
revised from the owner's live testing: with focus-follows-last-click, the media
pool and the main view **compete for the spacebar on recency**, which reads as
flaky ("it will apply to the most recently used section only").

SPEC-CHANGE (owner): **Space always applies to the main view when it is
available.** If the selected couple has a session, Space drives the main audition
transport unconditionally; the sticky pool player gets Space only as a fallback
when no audition exists (nothing synced yet / no couple open). Starting the
audition from Space still pauses a still-playing pool preview first.

Implementation consequence: the whole `poolFocused` focus-tracking layer from
2026-07-01 (PoolController flag, `AppModel.select(_:byUser:)`, per-view focus
clears) is REMOVED — the new rule is a pure condition on existing state
(`pair.session != nil`, `poolPrimaryEntry`), so no focus state is needed. The
waveform seek strip and the pool rail-growth rule are unchanged. Gates: swift
build + swift test green.

## 2026-07-03 — Seeded auto-sync wired (§11.3): evidence placement + drag-to-rescue

Owner repro (difficult-match test 3): the v2 queue paired `33-shortened-more.wav`
with `bach 172.mov` correctly (spectral matcher), but the editor placed the audio
at 0:00 with "no plausible correlation peak" — and manual placement + re-sync
failed identically. Diagnosis: the editor's auto-sync discarded the row's accepted
fingerprint lag and reran the §5 stage-1 blind envelope scan (z 8.0 < z_min 8.5 on
this quiet 21 s clip). The core seeded-sync path (`SyncSeed`,
`auto_sync_tracks_seeded`) was built for exactly this in the §11.3 prework but
never called with `Some` outside tests (sync.rs's own "boundary 8+" wiring note);
this session wires it end to end:

- **`core/src/policy/sync_seed.rs`** (new policy + unit tests): decides
  evidence-seed vs manual-rescue-seed vs unseeded. Evidence requires candidate
  z > 0 (a manual re-pair placeholder is z 0/lag 0 and never seeds); manual
  rescue requires explicit re-sync + last sync Failed + non-zero current offset,
  and outranks evidence. Ambient syncs never read the current offset.
- **Core**: `Ctx::auto_sync_blocking` takes `Option<SyncSeed>` and calls
  `auto_sync_tracks_seeded` (None ⇒ byte-identical unseeded path, per its §11.3
  contract). Stale "nothing calls this" note in sync.rs updated.
- **FFI (additive)**: `cpl_autosync_start_seeded(ctx, session, params_json,
  seed_offset_ms, seed_drift_ppm, cb, user)` + `cpl_policy_sync_seed` (uniform
  policy envelope). `couple.h` regenerated.
- **CLI**: `couple sync --seed-lag-ms=<ms> [--seed-drift-ppm=<ppm>]`
  (conflicts with --offset-only). Verified on test 3: unseeded exit 5
  (noPlausiblePeak, unchanged); seeded ⇒ lowConfidence, offset −82038 ms,
  7/12 drift windows accepted, fit RMS 0.33 ms — mapping written, graph
  populated, honest "please review" verdict.
- **Swift**: `Session.autoSyncStart(seed:)` picks the seeded symbol;
  `CoupleKit/SyncSeedPolicy.swift` is the thin FFI wrapper (conservative
  fallback: unseeded); `PairSession.load` captures the selected candidate's
  lag/z/ppm as evidence and `runAutoSync` passes the policy's decision
  (explicit ≡ announce). All three load sites pass their selected candidate
  (resolver, undo-restore, repair — repair's fresh-drag placeholder is
  rejected by the policy).

UX ledger: UX-DECISIONS 2026-07-03 entry. Gates: cargo test --release
--workspace green (policy 9/9; sync_drift, ffi, cli suites); mac swift build
(both variants) + swift test green (116, incl. 4 new SyncSeedPolicy tests).

## 2026-07-03 (later) — Sync graph honesty: adaptive drift windows + slope guard

Owner feedback on the newly-working test-3 pair: the drift sparkline "is not
inspiring confidence" — two lonely dots. Investigation: (a) the sparkline's
x-domain was the FULL video, cramming a 21 s overlap's dots into ~8% of the
width; (b) §5.5's fixed 9 s half-window left a ~3 s feasible center span on a
21 s overlap, so all 12 windows measured the SAME audio — the "7 win" were seven
near-copies of one measurement.

SPEC-CHANGE (§5.5, two coupled changes; both default-equivalent for the
validated long material):

1. **Adaptive drift half-window**: effective half-window =
   `clamp(overlap · drift_win_frac/2, drift_half_win_min_s, half_win_s)`
   (defaults 0.5 / 2.5 s / 9 s — the §5.4 verify-window shrink rule). Overlap
   ≥ 36 s ⇒ 9 s exactly as before. New `--params drift-win-frac`,
   `drift-half-win-min-s`.
2. **Slope guard**: the least-squares DRIFT fit runs only when the accepted
   centers span ≥ `drift_min_fit_span_s` (15 s; `--params drift-min-fit-span-s`);
   below it the pass is intercept-only — mapping (stage offset, 0 ppm),
   `drift_measurable = false`. Found the hard way: with shrunk windows the real
   test-3 clip accepted a 2.5 s cluster and the unguarded fit extrapolated
   **−7245 ppm** across 4:15 (the old stacked windows were accidentally
   protected — identical lags ⇒ harmless slope). A ppm from a few seconds of
   baseline is noise amplified by track length; refusing it is the §5.5.6
   "drift not measurable" state working as designed. Overlaps ≥ ~32 s fit
   exactly as before (all drift-ladder rows unchanged).

Test: `sync_drift::short_overlap_drift_windows_spread` (21 s external vs 120 s
camera — the test-3 shape): ≥6 accepted windows spanning ≥7 s, NO measurable
drift claimed, offset within 3 ms. Real test 3 after the change: offset
−82035.7 ms, ppm 0, honest lowConfidence.

Shell (UX-DECISIONS addendum): sparkline x-domain = measured span with m:ss
corner labels (was: full video); attempted-but-rejected windows from the last
sync's telemetry drawn as baseline ticks ("measured here, no lock" — a failed
measurement has no lag to plot); fit line unchanged. Caption gains
accepted/attempted counts and an explicit "offset-only" marker when no drift
was fitted: `fit RMS 2.52 ms · 5/12 win · offset-only`.

Gates: cargo test --release -p couple-core --test sync_drift (10, incl. new)
green; swift build both variants + swift test (116) green; full workspace suite
re-run green.

## 2026-07-04 — Phase G started: Electron shell over the same core (experiment)

Built the first cross-platform shell (`electron/`) as the PLAN-CORE-SHELL-BOUNDARY
§4-G exercise: verify a new shell needs ONLY an FFI binding + UI + pool I/O + an
audio callback, with zero resolution/policy logic. Core-side diff for the entire
shell: 2 manifest lines (`ffi` gains crate-type rlib; workspace member added) —
no core code changed.

- **Binding** (`electron/native`, crate `couple-node`): napi cdylib consuming
  couple-ffi's `pub extern "C" cpl_*` surface as a typed Rust dep (no header
  re-parse). One JS export per symbol; BigInt handles; zero-copy TypedArray
  fills for band/rawWindow/auditionFill; ThreadsafeFunction trampoline for job
  progress (worker thread → JS). Same `.node` runs in plain Node and Electron
  (N-API ABI stability; no electron-rebuild).
- **Parity gate (§9)** (`electron/scripts/parity.mjs`): the golden-pair-rows
  scenarios (continuous / simultaneous / restart-legacy / ambiguous-legacy /
  overflow-legacy) resolved via the kit's `resolveStart` vs `couple pair
  --json`, compared on the machine-stable decision projection — **identical on
  all 5**. Wired into CI as a 3-OS matrix job (macos/windows/ubuntu) along with
  the media-free binding smoke test.
- **Audio** (Phase G's "pull frames → device"): cpal output pump inside
  couple-node pulling `cpl_audition_fill` on the device callback thread — no JS
  in the audio path. Verified: clock advances in real time, 0 underruns.
  Muted `<video>` slaved to the audition clock by playbackRate micro-slew
  (±5%, hard-seek >250 ms); measured A/V skew with H.264 media ≈ −15 ms mean
  (< 1 frame @30 fps; spread is `video.currentTime` frame quantization).
- **Shell** (React/TS, engine in the renderer for synchronous hot calls):
  kit/ mirrors CoupleKit file-for-file (Engine/Wire/Job/Track/Session/Audition/
  Policies — all 9 policy wrappers + triangleCloses with couple.h's documented
  conservative fallbacks); app/ ports MediaPool/CoupleRow/PairSession/
  IngestController (unified ingest bar, serialized per-couple sync queue,
  row-merge via `cpl_policy_row_merge`, seeding via `cpl_policy_sync_seed`);
  views: 3-column layout, queue sections + evidence verdict dots, resolve
  banner with candidate re-pick, dual-lane Canvas waveform stage (band() with
  reused buffers, offscreen caching) + drift sparkline, batch export rail
  (Auto/MOV/MP4/WAV, per-row stems, disk preflight, serial continue-past-
  failure — verified: 3 fixture exports, valid containers per ffprobe).
- **Packaging/CI**: electron-builder (dmg/nsis/AppImage+deb; .node outside
  asar; bundled ffmpeg optional per-platform under resources/ffmpeg/), local
  unsigned dmg build verified; `electron` CI job runs smoke + parity +
  typecheck + build + package on all 3 OSes.
- **Editing interactions**: drag on the external lane = live offset
  (`cpl_set_offset_ms` hot setter), shift-drag = pivot drift (live ppm +
  `cpl_commit_drift` on release), stage keys (arrow/bracket nudge families,
  esc restores drag-start alignment), j/k/v/space keymap; verified end-to-end
  (a +500 ms nudge read back from `cpl_state_json` exactly). Manual re-pair:
  candidate re-pick + pool-audio drag onto a row, both through
  `cpl_policy_repair_drop` / `cpl_policy_repair_forced` (re-checked when the
  sync settles) — no shell-side thresholds.
- Dev-harness env family (Electron twin of DebugDrive): COUPLE_OPEN_DIR,
  COUPLE_SHOT_PATH/COUPLE_SHOT_DELAY_MS, COUPLE_AUTOQUIT_MS, COUPLE_HUD,
  COUPLE_PLAY_TEST, COUPLE_EXPORT_TEST.
- Deferred (scope: core workflow first, per owner): queue/session persistence
  (couple-queue/1 / couple-session/1 in userData), undo, restore toast,
  activity-strip composition via cpl_policy_activity (simple honest strip for
  now), loupes, pool preview player, media removal UI, screenshot-test
  scripting. Known warts: sync ffprobe on the drop path can jank the renderer
  (async probe binding is the fix); fixture mpeg4 video doesn't decode in
  Chromium (H.264 real media does); Swift shell unchanged and green.

Gates: parity 5/5 identical; binding smoke green; cargo workspace untouched
except 2 manifest lines; swift build (release) green post-change; tsc clean;
electron-vite build green; unsigned dmg packaged.

## 2026-07-04 (later) — Electron shell: full product-parity round

Owner call after the first demo: bring the Electron shell to FULL feature parity
for an honest shell-vs-shell comparison (supersedes the same-day "deferred"
list above — those items are now built). Also fixed real drag-drop (Electron
32+ removed `File.path`; now `webUtils.getPathForFile`) and added a native
"Choose/Add files…" picker fallback.

- **Anchors + loupes + sync tools**: anchor state on PairSession with proposal/
  add via `cpl_policy_anchor_propose` and onset snap via
  `cpl_best_onset_near_json`; draggable pins, loupe windows (additive blend,
  local-Δ readouts, ⇥ align, pivot-about-anchor), lock-zoom + side-by-side/
  stacked layouts; sync-tools bar (offset/drift nudges + readouts, drift-mode
  toggle, monitor Pan/Overlay/Null, per-track monitor gains); VideoHero
  transport (play/timecode/anchor jumps/fullscreen). Verified: 4 anchors all
  inside overlap, loupes/tools render in headless shots.
- **Persistence + undo + activity strip**: `couple-queue/1` + `couple-session/1`
  ports writing the mac formats under userData/Couple/; autosave on the mac
  app's triggers; launch restore toast → `suppressAutoSync` path (verified:
  relaunch restored 3 couples, offsets byte-equal, 0 re-syncs); ⌘Z/⇧⌘Z undo
  stack (approve/re-pair/rename); ⌘⌫ clear-session with
  `cpl_policy_media_removal` clearCounts in the confirm copy; activity strip
  now composed by `cpl_policy_activity` (shell holds clocks/debounce, formats
  strings — Phase F contract).
- **Media pool parity**: Finder multiselect (PoolSelection port), sticky bottom
  preview player w/ waveform seek strip, video thumbnails (canvas frame grab;
  glyph fallback), metadata popover (MediaMetaCard projection, never re-probes),
  TODO-001 removal via `cpl_policy_media_removal` plan + confirmation +
  correct teardown ordering (verified: removal → re-resolve re-paired the
  survivor). Usage lines ("used by/alternate for N couples").
- Harness env grew: COUPLE_USER_DATA + COUPLE_{PERSIST,RESTORE,UNDO,REMOVE,
  POOL_PREVIEW,POOL_INFO,ANCHOR}_TEST (CLAUDE.md updated).
- Still out of scope: the scripted screenshot/dev harness (DebugDrive
  equivalent) — dev tooling, not product surface.

Gates re-run green after integration: binding smoke; §9 parity 5/5 identical;
tsc; electron-vite build; integrated headless run (anchors+loupes+persistence
in one session) with zero renderer errors.

## 2026-07-04 (later still) — Electron shell: real-media fixes after owner testing

Owner testing with real media (HEVC .MOV camera files + long WAVs) surfaced
failures the fixture-driven harness never hit. All fixed; the root causes are
the sharpest shell-comparison lessons so far:

- **Local media serving** (video hero / pool preview / thumbnails dead in dev):
  Chromium blocks file:// subresources on the dev server's http origin. Added a
  `couple-media://` protocol (main process). THREE stacked requirements before
  a <video> will consume a custom scheme: (1) `registerSchemesAsPrivileged`
  with `standard: true` + `stream: true` — without `standard` the media stack
  silently ignores the element (readyState 0, no error); (2) ranged responses
  must be STREAMED in full — Chromium does not chain short 206 chunks on
  custom schemes (small files masked this; a large .MOV died); (3) .mov must
  be served as `video/mp4` — Chromium rejects `video/quicktime` outright.
  HEVC itself HW-decodes fine (VideoToolbox; canPlayType "probably").
- **Seek plumbing**: the audio clock is the master — seeking the <video>
  directly is snapped back by the slaving loop. Playhead clicks now route
  pair.setPlayhead → audition seek (AppModel-injected hook); pause holds the
  frame at the stop position.
- **Pool preview seek**: Chromium extrapolates WAV duration from the first
  chunk (58.25 s for a 120 s file) → scrubs clamped + a phantom `ended` reset
  the strip. The probe's `durationS` is now authoritative for the strip domain
  /clamp/end detection (the mac AVPlayer model); early `ended` is ignored.
- **Doubled waveform**: the shell had grown a zoomable main stage PLUS a
  separate full-domain overview — the Swift WaveformStage has ONE zoomable
  OverviewStrip owning pins + playhead (no minimap). Collapsed to match.
- **StrictMode ingest race**: dev double-mounted the startup effect → two
  concurrent ingests of one folder; concurrent same-content imports collided
  and silently dropped a file. Startup one-shot guard + MediaPool now
  serializes import batches (also covers real double-drops).
- Real drag-drop fixed earlier the same session (Electron 32+ removed
  File.path → webUtils.getPathForFile) + native "Choose/Add files…" pickers.

Verified with the owner's real pair (8-min HEVC 1080p .MOV × 120 s WAV):
import → confident row → audition with video at readyState 4, measured skew
−4…−33 ms, 0 underruns; seek strip lands within ±0.3 s. Gates re-run green
(smoke, §9 parity 5/5, tsc, build).

## 2026-07-11 — No splash screen: empty session renders the full three-column shell (both shells)

Owner: the launch-time file picker as its own screen made the app a black box —
you couldn't see what the app is like or what you can do until you picked files.
Now the three-column shell renders from first launch and the import surface
(drop zone + "Choose files…" + one orientation line pointing at the rails) sits
in the video hero's slot; rails show their designed empty states around it
(dimmed sections, 0-counts, pool "+ import", disabled Export). UX-DECISIONS
2026-07-11 entry revises §3 "Empty / launch"; SHELL-UI-PARITY updated (center
column + Clear Session disable-when-empty). Shipped mac-first; Electron port
followed the same day after owner approval of the Swift version.

- mac: `RootView` no longer branches on `hasContent`; `LoadSurface` restyled as
  the embedded center empty state (wordmark dropped — the queue-rail header
  owns it); `CenterColumn` swaps it in for `VideoHero` when the pool is empty;
  "Clear Session…" disables when the session is already empty.
- Electron: `App.tsx` splash branch removed (always `.columns`); new
  `views/LoadSurface.tsx` (Swift copy verbatim, incl. importing spinner)
  rendered by `CenterColumn.tsx` when `!hasContent`; `.import-surface` styles
  with the window-level `.drop-active` highlighting its dashed border; same
  Clear Session disable. The pre-existing Electron splash copy ("Drop two
  recordings…") was v1-era and is replaced by the shared copy.
- No engine/policy change; no SPEC-CHANGE (launch surface is shell
  presentation, UX-DECISIONS authority).
- Verified — mac dev harness: `00-launch-empty.png` shows the full shell with
  the centered import surface; post-import walkthrough shots unchanged;
  REMOVE-TEST clear-session checks pass (label updated: "center import
  surface"); `swift test` 116 green. Electron: tsc, smoke, §9 parity 5/5 green;
  harness shots confirm the empty shell and an unchanged 2-couple loaded state.

## 2026-07-11 — Electron visual-parity polish, committed retroactively (commit 6e7b6e7)

Housekeeping entry: an earlier session (after the 2026-07-04 real-media fixes)
left a visual-parity polish round uncommitted in the working tree; it was
committed 2026-07-11 as `6e7b6e7`, split apart from the same-day no-splash
work. Logged here so the ledger covers it — the session that wrote it never
added an entry. What it contains (from the diff):

- **Pool video thumbnails**: the `couple-media://` scheme gains `corsEnabled`
  + an `Access-Control-Allow-Origin: *` response header so a
  `<video crossorigin>` frame grab draws to an untainted canvas — playback
  never cared, `toDataURL` (thumbnails) does.
- **Monitor modes match CoupleKit `MonitorMode.allCases`**: Null is a test
  oracle only and is excluded from the UI; default is Overlay (Swift
  AuditionController parity); label reads "Pan L/R".
- **Waveform stage badge gutter** (WaveformStage.swift twin): anchor pin heads
  and the playhead knob render above the lanes, off the waveform pixels.
- **Export rail / queue rail / footer styling**: destination folder glyph,
  export-run button style, couple-row redesign (verdict spine, coverage strip,
  pairing second line), footer keyboard cheat line matching the mac FooterBar;
  broad restyling across styles.css / editor.css / export.css / pool.css.
- SHELL-UI-PARITY.md gained the queue-rail session-header line and the
  corrected monitor-mode line.

Gates at commit time (run 2026-07-11 on the combined tree): tsc, smoke,
§9 parity 5/5 green.

## 2026-07-11 — Electron overlay scrollbars: auto-hide to match macOS (shell polish)

Owner report (with before/after shots): the queue rail and media pool rendered
Chromium's persistent scrollbar — a heavy always-on bar with a light track that
read badly against the dark UI. The mac shell shows macOS overlay bars: nothing
at rest, a thin transient thumb only while actually scrolling. Made Electron
behave the same, globally so every scrollable surface (queue rail, media pool,
export rail, editor lists) is consistent.

- **styles.css**: global `::-webkit-scrollbar` treatment — an 8 px gutter
  (fixed, so a thumb appearing never reflows content), fully transparent track /
  corner, and a thumb that is transparent at rest. The thumb paints a
  semi-translucent rounded pill (`background-clip: padding-box` inside a 2 px
  transparent border, so it reads as a thin inset bar) only while the surface
  carries a `.scrolling` class or is hovered.
- **scroll-overlay.ts** (new, ~25 lines): `installOverlayScrollbars()` adds one
  capture-phase, passive `scroll` listener on `document` (scroll doesn't bubble),
  marks the scrolled element `.scrolling`, and clears it ~900 ms after motion
  stops via a per-element `WeakMap` timer. Presentation only, no engine coupling
  (PLAN-CORE-SHELL-BOUNDARY §1).
- **main.tsx**: calls `installOverlayScrollbars()` at startup, before render.
- No SPEC-CHANGE (shell presentation parity); no SHELL-UI-PARITY line (no
  affordance added/removed) and no UX-DECISIONS entry (visual polish, matching
  the 2026-07-11 visual-parity precedent above, which likewise recorded only
  here).
- Gates (from `electron/`): `typecheck`, `smoke` (12/12), `parity` (5/5 §9) all
  green. Visual proof: a 24-clip media pool renders 24 entries with the region
  overflowing (only 5 visible) and NO scrollbar painted at rest.

## 2026-07-11 — Electron verdict-dot parity: manual-pick match evidence (TODO-009)

Fixed a shell parity bug where a manually-picked pairing that IS a confident
match showed green / TO APPROVE on mac but questionable / TO FIX in Electron
over the SAME core state. The mac shell probes a manual pick's match evidence
through the resolver's own path (`Resolver.manualPairIsConfident` →
`engine.matchPair`, classification == match) at pick time, on re-pair, and on
restore; the Electron shell never computed it — `manualMatchConfident` was only
ever set to `null` or copied from persistence, and the `match_quality` policy
reads `null` as "not confident". The stale CoupleRow.ts comment ("re-verified by
the sync that follows") described a step that did not exist.

- **Engine.ts**: exposed `matchPair(video, audio, paramsJSON?)` — thin port of
  the existing `native.matchPair` (napi already exported it), mirroring the
  `matchCandidates` idiom and CoupleKit/Engine.swift `matchPair`. Returns the
  decoded `PairEvidence` or `null` on a core fault / null-payload miss. PLUMBING
  ONLY — acceptance stays in core.
- **AppModel.ts**: added `reevaluateManualMatch(couple)` mirroring the mac
  AppModel method + the fingerprint branch of `Resolver.manualPairIsConfident`
  (Electron runs the fingerprint-primary default, so `classification === 'match'`
  is the whole answer). Called on manual pick / re-pair (`repairDrop` and
  CenterColumn `pickCandidate`, the two `applyManualPick` sites — mac routes both
  through `repair`) and on restore when the evidence wasn't persisted
  (`applyRestore`, mirroring AppModel.swift's restore re-eval). A core fault
  (null) is not confident; deferred a microtask so the pick paints questionable
  before the dot may flip (the Swift shell runs it off-main). The undo-snapshot
  path (`applyRepairSnapshot`) still restores the saved value verbatim — mac's
  undo does the same.
- **CoupleRow.ts**: corrected the stale `applyManualPick` comment.
- **harness.ts**: added the `COUPLE_QUALITY_TEST` hook (app-target mirror of the
  Swift DebugDrive COUPLE_QUALITY_TEST, but media-driven since the real probe
  needs handles): re-picks a resolved couple's own audio and asserts
  `manualMatchConfident` flips null→true with a `good` verdict.
- **SHELL-UI-PARITY.md**: verdict-dot line made explicit that a manual pick
  greens only via the same core classifier, on pick / re-pair / restore.
- **CLAUDE.md**: added QUALITY to the `COUPLE_*_TEST` family list.
- No engine/policy change; no SPEC-CHANGE (shell presentation parity —
  the decision has always lived in core `match_quality` + `matchPair`).
- Gates (from `electron/`): `build:native`, `typecheck`, `smoke` (12/12), and
  `parity` (5/5 §9) all green.

## 2026-07-11 — mac activity strip spans the full window width (Electron parity)

Owner request (prefers the Electron treatment): the unified import→match→sync
activity strip (TODO-005 bar) moved from the bottom of the export rail to the
root layout — full window width between the three columns and the FooterBar,
top hairline mirroring Electron's border-top, label 9.5→11 pt. Idle renders
nothing: the old rail placement's fixed 30 px slot (which avoided reflowing the
couple list) is dropped, since at the window bottom there is nothing below to
reflow — matching Electron. Same view over the same pure `ActivitySummaryPolicy`;
no engine/policy change, no SPEC-CHANGE. The UX-DECISIONS 2026-07-11 entry
records the decision; SHELL-UI-PARITY's activity-strip line updated.

- mac: `ActivityStripView` moved out of `ExportRail.swift` into `RootView.swift`
  (root VStack, between the HSplitView and FooterBar), restyled full-width.
- Gates: `cargo build --release -p couple-ffi`, `swift build -c release` (ship
  + `-DCOUPLE_DEVHARNESS`), `swift test` 116/116 all green. Dev-harness shot
  confirms the bar edge-to-edge mid-import.

## 2026-07-11 — CI hardening: golden diff per-push + path-filtered ROC gate

F1 finding: `scripts/golden-pair-rows.sh` (the resolver-decision golden diff
frozen for PLAN-CORE-SHELL-BOUNDARY Phase 0) existed but nothing under
`.github/` ran it — so a decision drift could land unnoticed; and the 33-case
synthetic-corpus ROC gate was weekly/on-demand only, never per push/PR. Both are
now wired into CI. No SPEC-CHANGE (test/CI plumbing only; no engine or policy).

- **Golden diff in the ci.yml test job**: added a `Golden pair-rows decision
  diff` step right after `cargo test --release --workspace`. All inputs are
  committed JSON — specs in `fixtures/golden/specs`, goldens in
  `fixtures/golden/rows` — so it needs no gitignored media, only the ffmpeg the
  job already installs. The update path is unchanged and stays local-only
  (`COUPLE_UPDATE_GOLDEN=1 scripts/golden-pair-rows.sh` → review diff → commit);
  CI never sets that env, so it can only ever compare.
- **Script switched debug→release**: the debug run measured ~10 min wall (fixture
  synthesis + pair are DSP-heavy); release rides the artifacts `cargo test
  --release` already built, locally and in CI. Re-ran under release WITHOUT
  `COUPLE_UPDATE_GOLDEN`: "all scenarios match" against the existing goldens and
  `git status fixtures/golden` clean — verified, NOT recaptured.
- **New `.github/workflows/roc-gate.yml`**: runs the exact existing ROC command
  (`cargo test --release -p couple-core --test matching_evidence -- --ignored
  synthetic_corpus_roc_gates_hold`) on push-to-main and PR, filtered via
  `on.paths` to matcher inputs — `core/src/{matching,match_refine,match_graph,
  matcher_corpus,verify,pairing_params}.rs`, `core/src/fingerprint/**`,
  `core/tests/matching_evidence.rs`, `fixtures/matcher-corpus/**`, and the
  workflow file itself. Kept as its own workflow because `on.paths` gates the
  whole workflow, not a single job — so this filter can't affect the main CI job.
- **ci.yml corpus-gate job kept unchanged** as the no-change drift net (weekly +
  workflow_dispatch): toolchain/brew-ffmpeg updates can move results with no
  matcher-path change, which the path-filtered per-change run would miss. Header
  comment + corpus-gate comment updated to describe the split.
- **CLAUDE.md**: added the golden-diff bullet to the build/test list and noted the
  ROC gate's new push/PR trigger.

## 2026-07-11 — Doc hygiene: SPEC status header + UX-DECISIONS anchor-drag backfill (F6)

F6 finding, docs only — no code, no engine/policy change, no SPEC-CHANGE
(status/history markers, not behavior text).

- **SPEC.md status line**: "planning complete, no implementation" replaced
  with the shipped reality — engine (couple-core), CLI (couple), macOS app
  shipped; Electron shell (Phase G) in progress on the same core — plus the
  SPEC-V2 precedence pointer (supersedes §12 and the single-pair-editor
  framing) and the SPEC-CHANGE ledger pointer. Staleness fix to a status
  header, not a spec deviation.
- **UX-DECISIONS.md**: appended a 2026-07-11 entry revising §1.6/§2.2/§2.9 —
  anchor pins do not snap on drag release (owner SPEC-CHANGE 2026-06-12,
  Session D feedback round 1, archived ledger) and anchor drag is direct 1:1
  pointer→time mapping (TODO-014 Part 1, commit af0c1b2). Backfill of
  already-shipped, owner-ratified decisions; historical sections left
  verbatim per the append-only convention (no back-markers — matches the
  2026-07-03/2026-07-11 appended-entry precedent). Note: the stale text lives
  in §1.6/§2.2/§2.9, not §2.4 as the finding stated.
- **Checked, no edit needed**: UX-DECISIONS §3's two-slot launch picker was
  already retired by the existing 2026-07-11 no-splash entry (commit
  8b3073c); its final bullet explicitly supersedes the two-slot picker and
  "hidden or inert window" clause. TODO.md and CLAUDE.md unaffected (no
  flag/command/layout/doc-role change; TODO-014's status line is already
  accurate).

## 2026-07-11 — FFI panic guard: a core panic is an error, not a process abort (F2)

Every `pub extern "C"` entry in ffi/src/lib.rs (65 of 66; `cpl_abi_version` is a
constant) now runs under a `catch_unwind` guard that converts a panic into the
EXISTING thread-local last-error as `CoupleError::Internal { "panic: …" }`
(exit code 9) and returns the entry's error sentinel (null / 0 / status 9) —
previously any core unwrap reached through the ABI aborted both shells
(Rust ≥1.71 aborts on unwind-across-extern). Job bodies (`spawn_job`) get the
same guard: a panicking job now delivers the terminal "error" callback and is
removed from the registry instead of hanging the shell's Job forever. Locks a
panic could poison are now poison-tolerant (`unwrap_or_else(PoisonError::
into_inner)`, the existing track.rs fp_build_lock pattern): the FFI registry,
`Session::cold` (11 sites, + a poison test), and the ctx.rs
track/session/stream/job registries — one reported fault no longer cascades
into every later session call aborting. Guard is once per FFI entry, never
per-iteration: §9.6 budgets for `cpl_band`/`cpl_audition_fill` unaffected
(zero-cost non-panic path, allocation-free fill preserved). No SPEC-CHANGE:
§4.7 already defines `Internal` and the status+`cpl_take_error_json`
convention; `couple.h` is byte-identical (no new symbols, ABI stays 1). Both
shells are covered by the in-ffi guard (couple-node calls `cpl_*` directly;
napi does not guard by default). CLI is unchanged by design — it links core
directly and a batch-tool panic should stay a loud exit.
- Gates: `cargo test --release --workspace` all green (new: ffi
  `panic_guard_maps_panic_to_internal_error_and_engine_survives` + ffi
  `job_body_panic_delivers_error_terminal_and_frees_job` + core
  `session::tests::cold_lock_poison_tolerated`; ffi lib suite 20 passed);
  `couple.h` diff clean (exit 0); mac `swift build -c release` (ship +
  `-DCOUPLE_DEVHARNESS`) both Build complete, `swift test` 116 passed /
  0 failures; electron `build:native` + `typecheck` clean, `smoke` 12/12,
  `parity` 5/5. (Environment note, not this change: the freshly built
  `couple-node.darwin-arm64.node` was initially SIGKILLed at `require()` with
  "Code Signature Invalid" — stale kernel signature cache on the new artifact;
  `codesign -s - -f` on the .node fixed it and both harnesses then passed.)

## 2026-07-11 — F4: real-data validation made fail-loud + one-command calibration sweep (proposal only)

Finding F4: the realdata suites skipped SILENTLY GREEN without
COUPLE_REALDATA_DIR, and the [TUNE] verdict lines (SPEC-V2 §V2.9 / V2-F2) have
never met real sessions. This session ships the automatable half ONLY —
measurement + a written proposal. ZERO threshold/gate-line/verdict-constant
changes anywhere (verified: core/src/sync.rs, core/src/resolve.rs,
core/src/fingerprint/params.rs, core/src/pairing_params.rs, all test assertion
literals, example_data/expected.json and both shell mirrors are byte-untouched).

- Fail-loud skips: the five COUPLE_REALDATA_DIR-gated tests
  (core realdata_import, realdata_sync ×2, resolve::realdata_pair_confident_row;
  cli pair_gates::realdata_pair_confident_row) are now
  `#[ignore = "SKIPPED (no realdata): …"]` — the matching_evidence ROC-gate
  idiom — so default `cargo test --release --workspace` prints a visible
  per-test marker instead of a lying green (libtest shows ignore reasons in
  default output; eprintln! was swallowed by capture). Run explicitly
  (`--include-ignored`), a missing/incomplete dir PANICS "REALDATA MISSING"
  instead of returning early. common::realdata_dir() now returns PathBuf and
  fails loud. ci.yml UNCHANGED — the marker rides the existing cargo-test step.
- SPEC-CHANGE (mechanism only; owner acked 2026-07-12): SPEC §9.4's realdata suites
  run via COUPLE_REALDATA_DIR + explicit `--include-ignored` (normally through
  scripts/realdata-sweep.sh) instead of implicitly inside plain cargo test.
  All §9.4 assertions/tolerances/freeze values untouched.
- scripts/realdata-sweep.sh + scripts/realdata_sweep.py: the F4 owner sweep.
  ONE command over COUPLE_REALDATA_DIR — discovers session dirs, `couple pair
  --json` per session (full spectral evidenceMatrix + legacy z matrix),
  `couple sync --json` per resolved couple, frozen §9.4 suites when the
  schumann pair is present — and writes realdata-report.md/.json (gitignored):
  measured offset/RMS/ppm/z-margin/coverage/triangle distributions vs the
  CURRENT [TUNE] lines with would-flag counts, and non-selected `match` cells
  called out as the §3.5 different-take hazard. Measures and reports only;
  exits 2 with instructions when the dir is missing/empty. `--self-check`
  synthesizes a 2-video/2-external session (couple fixture session) and
  asserts the full pipeline end-to-end (verified this session: self-check OK,
  2 couples synced, cross cells negative, all 7 report sections).
- PLAN-REALDATA-CALIBRATION.md (root, live plan): the freeze/calibration
  proposal — PROPOSAL AWAITING OWNER DATA + SIGN-OFF — mirroring
  fixtures/matcher-corpus/CALIBRATION.md §6–7 methodology (partition/two-sided
  margins/freeze/value-lock/holdout-once/contamination discipline), with the
  SPEC-CHANGE entries DRAFTED verbatim and explicitly not enacted.
- CLAUDE.md (doc map + build/test lines) edited in the WORKING TREE but left
  UNSTAGED: the file carries an unrelated uncommitted `website/` hunk from
  another session that a whole-file stage would sweep in.

## 2026-07-11 — Core hardening: epoch-lead bound, portable disk preflight (F3 a,c,d)

F3 finding, core-internal — no FFI symbol, wire, or shell change; couple.h
byte-identical.

- **Epoch guard (a)**: `import.rs` now rejects a probed audio `start_time`
  outside ±100 000 s (`MAX_EPOCH_ABS_S`; first round number above the MPEG-TS
  33-bit PCR wrap ≈ 95 444 s — no legitimate container baseline exceeds it)
  as `UnsupportedFormat` instead of honoring it: a corrupt container could
  previously force near-unbounded zero-fill of the PCM cache. Reject, not
  clamp — a truncated epoch would silently misplace the track (F1). The
  disk preflight projection now also counts the epoch zero-fill
  (`projected_cache_bytes`), so a huge-but-legit lead fails early as an
  honest DiskFull instead of filling the disk mid-decode. §7.2's epoch rule
  is unchanged for every input it contemplates; recorded reason per SPEC §0.
- **Portable disk-free query (c)**: `cache::available_bytes` split into
  `#[cfg(unix)]` statvfs and `#[cfg(windows)]` GetDiskFreeSpaceExW
  (windows-sys 0.61, target-gated; already in the windows dep tree via
  tempfile). Previously the unix-only code had no cfg guard, so couple-core
  could not compile on windows at all — the electron windows CI job would
  have failed at build:native on its first real run. libc is now a
  cfg(unix) dependency.
- **Loud fallback (d)**: a failed free-disk query now logs
  `couple-core: free-disk query failed for <path>; assuming enough space
  (disk-full preflight skipped)` to stderr (core's eprintln idiom; no log
  crate) before returning u64::MAX — previously the DiskFull preflight
  (import §6.3 and export §8.3 both) was silently disabled.
- NOT done here (owner decision pending): F3b content-key hardening
  (middle-slice) — SPEC.md §6.3 normatively fixes the key composition
  ([SPEC] tag), so it requires a SPEC-CHANGE with owner sign-off.
- Gates: `cargo test --release --workspace` green (new: import
  `epoch_guard_accepts_real_container_range` /
  `epoch_guard_rejects_corrupt_start_time` /
  `preflight_projection_accounts_for_epoch_lead`, import_pipeline
  `absurd_start_time_is_rejected_before_zero_fill`);
  `scripts/golden-pair-rows.sh` clean; `cargo build --release -p couple-ffi`
  + couple.h diff clean. Swift/electron gates skipped (core-internal, no
  policy/ file, header identical); windows arm is compile-verified by the CI
  electron job, not locally (no MSVC toolchain on macOS).

## 2026-07-11 — F5: Electron shell tests, navigation guard, async import probe, mac zMin from core

Finding F5 (shell hardening, four items). couple-core/couple-ffi untouched —
`couple.h` byte-identical; the only Rust changed is the shell's own couple-node
binding (`probeJsonAsync`). No SHELL-UI-PARITY change (no affordance added or
removed). No SPEC-CHANGE (timing-only + value-identical plumbing).

- **(a) Vitest over the Electron app/kit layer** — first automated tests for the
  renderer's reducers/controllers (the layer the 87136e4 manual-pick
  verdict-parity bug lived in). `npm test` (`vitest run`) in electron/: 6 suites
  — Policies envelope decode + conservative fallbacks, CoupleRow
  verdict/actionWord/forced lifecycle, AppModel manual-pick evidence flow
  (incl. the 87136e4 regression shape: pick ⇒ null ⇒ core classifier 'match' ⇒
  green; core fault ⇒ never green; stale-selection no-op), QueueStore TODO-009
  provenance round-trip, poolSelect/poolListHeight, UndoStack. Tests run in
  plain node against the REAL couple-node binding (vitest.setup.ts shims
  window.require / COUPLE_NATIVE_DIR / an isolated COUPLE_USER_DATA), so every
  cpl_policy_* decision asserted is core's own — no re-encoded policy tables in
  mocks; only media-bound calls (matchPair, session load) are stubbed. Wired
  into the electron CI job on all 3 OSes after the smoke step.
- **(b) Main-process navigation guard** — defense-in-depth for the
  nodeIntegration renderer: every webContents denies window.open and blocks
  will-navigate away from the app's own content (dev ELECTRON_RENDERER_URL or
  packaged file://).
- **(c) Async import probe** — the drop path's `cpl_probe_json` now runs on the
  libuv pool (couple-node `probeJsonAsync`, napi AsyncTask; the core error is
  drained WORKER-side because couple-ffi's LAST_ERROR is thread-local) and
  MediaPool awaits `Engine.probeAsync`, so a slow ffprobe spawn no longer janks
  the renderer — mac parity (MediaPool.swift probes via Task.detached).
  Ordering unchanged: the import loop stays sequential and batches still chain;
  the pre-existing 'probing' activity line now actually paints during the gap.
  The role/no-audio classification stays exactly where it was (shell-side in
  both shells) — no decision moved (PLAN-CORE-SHELL-BOUNDARY §1).
- **(d) mac zMin/zMargin read core defaults** — AppModel.init overwrites the
  PairParams presentation mirrors from `cpl_resolve_params_default_json` (the
  surface Electron already reads); the 8.5/6.0 literals remain only as
  core-fault fallbacks, matching Electron's `?? 8.5` idiom exactly.
  Byte-identical today (core defaults are 8.5/6.0); the mirror can no longer
  drift silently. No FFI addition needed — the binding surface already existed.
- Gates: electron `typecheck` clean, `npm test` all green, `smoke` all-pass,
  `parity` all rows byte-identical (after build:native; codesign -s - -f wart
  applies to the fresh .node); mac `swift build -c release` (ship +
  -DCOUPLE_DEVHARNESS) Build complete, `swift test` 0 failures;
  `git diff core ffi` empty.

## 2026-07-11 — Realdata calibration ENACTED (F4; PLAN-REALDATA-CALIBRATION §6, owner signed off 2026-07-11)

The owner-approved re-freeze of the [TUNE] verdict/coverage lines, from
realdata-report.md (generated 2026-07-12 05:07Z; 4 real multifile-session/*
sessions, n=18 true couples — root fixtures and difficult-match/* excluded as
engineered same-recording variants). Constants + value-locks + §9.4 freeze files
+ consuming test in one change. PLAN-REALDATA-CALIBRATION.md marked ENACTED and
archived. Shell mirrors verified clean (core is the single source since Phase D /
F5d — no Swift/TS value changed).

SPEC-CHANGE (2026-07-11): §5.6/Appendix A sync-verdict lines re-frozen on the
owner corpus (realdata-report.md 2026-07-12 05:07Z, N=18 true couples):
synced_max_rms_ms 1.0 → 12.0 (derivation: true-couple fit RMS min 0.002 /
median 4.713 / p95 7.814 / max 9.012 ms, worst true = 2022orch dvorak.MOV,
owner-confirmed; 12.0 ≈ 1.33× worst true; aligns with pairing
verify_rms_max_ms = 12.0; no wrong-take RMS was measurable — wrong pairings
fail the fit outright with 0 windows, regime gap RECORDED); synced_min_windows
6 → 4 (weakest true = 5 windows, deg recital, owner-confirmed; wrong pairings
lock 0; 4 sits between, above verify_min_windows_floor = 3); synced_fine_ncc
0.5 unchanged (real true min 0.619); suggest_resample_ms 2.0 unchanged (|drift|
median 24.1 / p95 70.2 / max 90.2 ppm). Two-sided margins per CALIBRATION.md
§6.3 discipline. CAVEAT: the fresh-holdout release gate is DEFERRED — every
session was read during tuning (contaminated per CALIBRATION.md §6.5b); the
first future unseen sessions serve as after-the-fact validation. Owner signed
off 2026-07-11. Value-locked in core/src/sync.rs tests (sync_verdict_value_lock).

SPEC-CHANGE (2026-07-11): SPEC-V2 §9 pairing constants re-frozen on the first
real multi-take corpus (V2-F2 closure): COVERAGE_FULL 0.98 → 0.95 (real
full-true coverage min 0.9628; genuine partial (fixture) 0.082 — two-sided
clean); Z_MARGIN 6.0 → unchanged (legacy per-video z-margin min 0.059 / median
13.4 — the sub-margin cells are same-recording fixture copies, not takes);
TRIANGLE_TOL_MS 250 → unchanged (residual p95 135 ms). Different-take hazard
characterized: 5 cross-cell `match` classifications over 810 non-selected
cells — all trimmed/shortened same-recording fixture copies, NOT different
takes → zero true different-take conflicts; Z_MARGIN/COVERAGE/TRIANGLE stay
conservative and the regime gap is RECORDED per plan §4.3. Owner signed off
2026-07-11. Value-locked in core/tests/resolve.rs (resolve_params_value_lock).
Golden rows: no resolution decision changed (no golden scenario's coverage
lies in [0.95, 0.98) — goldens byte-identical, no regeneration).

SPEC-CHANGE (2026-07-11): §9.4 frozen real-pair regression extended: sessions
multifile-session/{2022orch, deg recital, hsBach, large} frozen per the F2
procedure (measured offset/drift written to <session>/expected.json,
ownerVerified=true after ear check, asserted at ±1.0 ms thereafter by
core/tests/realdata_sync.rs::real_sessions_frozen_expectations; 18 couples;
files live under COUPLE_REALDATA_DIR, outside the repo). CAVEAT: the §9.5
hand-sync gate stays dormant (no material). Owner signed off 2026-07-11.

- Gates: `cargo test --release --workspace` all targets pass (realdata tests show
  the SKIPPED markers, now 3 in realdata_sync incl. the new
  real_sessions_frozen_expectations; both value locks green). With
  COUPLE_REALDATA_DIR set: realdata_import 1/1, realdata_sync 3/3 (frozen
  expectations verified 18 couples), resolve realdata 1/1, cli pair_gates
  realdata 1/1. `scripts/golden-pair-rows.sh`: all 5 scenarios match —
  byte-identical, no regeneration (prediction held). `cargo build --release -p
  couple-ffi` clean, couple.h unchanged. mac `swift build -c release` (ship +
  -DCOUPLE_DEVHARNESS) Build complete; `swift test` 116/116. electron
  build:native + typecheck clean, vitest 40/40, smoke all checks passed, parity
  "kit rows identical to CLI rows on all scenarios". Sweep acceptance
  (regenerated report): all 18 multifile-session true couples status=synced
  (was 4), rowState=confident 18/18 — the five 0.96–0.98-coverage rows
  (2022orch dvorak 3 0.9628, prokofiev 0.9785, hsBach bach 169 0.9715,
  bach 170 0.9786, large bach 169 0.9715) flipped partial → confident;
  crossMatches unchanged (the 5 known fixture-copy cells, none new); report §5
  suites all PASS.

Follow-up (same session): scripts/realdata_sweep.py no longer hardcodes a
display-only SYNC_LINES mirror — it parses the four verdict lines live from
core/src/sync.rs `SyncParams::default()` (hard-exit if the parse fails), so
report annotations can never go stale against a re-freeze again. Caught by the
enactment implementer: the first regenerated report annotated distributions
against the OLD lines (`synced_max_rms_ms = 1.0`) while the engine correctly
used the new ones. Verified: --self-check OK; full sweep re-run annotates
`= 12.0`, 0/26 over the line. Script stays read-only (SPEC §0).

## 2026-07-12 — F3b: middle 8 MiB slice in the cache content key

SPEC-CHANGE (2026-07-12): SPEC §6.3 contentKey = BLAKE3(first 8 MiB ‖ middle 8 MiB ‖ last 8 MiB ‖ u64 size), hex128. Middle slice len = min(size, 8 MiB) at offset (size − len)/2 (floor; overlap allowed, matching the head/tail clamp idiom). F3b from the 2026-07-11 review; approved by owner 2026-07-12; middle 8 MiB slice, not mtime, self-invalidating key, no version bump: a file edited only in the middle previously reused stale PCM/pyramids. Old-key entries stay inside the v1 tree where LRU GC evicts them first (never touched again ⇒ oldest lastUsed); a root bump would orphan them outside GC's reach. Accepted consequences: one-time re-decode of imported media on next use; autosaves keyed by old contentKeys aren't offered. Shells unaffected (key is opaque through FFI; couple.h byte-identical). Gates: workspace tests green (4 new key unit tests in cache.rs), golden rows byte-identical (projection excludes keys by design).

## 2026-09-11 — Behavior-preserving maintenance pass

Implemented the twelve maintenance-review items in sixteen isolated code commits.
No specification, thresholds, wire/persistence schemas, C ABI, or UI affordances
were intentionally changed. The separate lifecycle/undo/multi-context correctness
concerns identified during review remain outside this maintenance scope.

| Review item | Cleanup | Commit |
| --- | --- | --- |
| 1 | Remove unused Swift editor scaffolding and Electron minimap branch | `da9225e` |
| 2 | Share Electron error parsing and Swift policy-envelope decoding | `6683c95` |
| 3 | Disarm import/export cleanup guards without leaking their path allocations | `9108b66` |
| 4 | Reuse the core matcher enum in the CLI | `51d58eb` |
| 5 | Share Swift single/batch export-option encoding | `f943978` |
| 6a | Make the already-required Swift app engine nonoptional | `e02a349` |
| 6b | Store the Electron viewport as one optional range | `29d09fa` |
| 6c | Type activity-policy request payloads | `91097fc` |
| 7, Swift | Centralize job-start callback ownership | `8d9598e` |
| 7, Electron | Centralize native job-start callback ownership | `70d9238` |
| 8a | Remove redundant legacy-correlation caches | `0dc5391` |
| 8b | Simplify ordered fit-request bookkeeping | `fabd069` |
| 9 | Reuse pivot-drift arithmetic for anchor drags | `85325c5` |
| 10 | Share sync refinement and result assembly | `9834496` |
| 11 | Normalize resolver candidates and share selected-row assembly | `bd28653` |
| 12 | Reuse CLI matching matrices through a core-owned Rust seam | `404b9ee` |

The matrix seam validates matcher kind and dimensions; callers supply the same
ordered imported tracks and pairing parameters used to build the matrix. Existing
C/progress entrypoints still build their own matrices. Shadow rows still use legacy
correlations, fingerprint rows still use graph-adjusted evidence, and independent
audio/audio triangle checks remain independent. The CLI preserves display/debug
ordering and skips its second matrix computation. No benchmark speedup is claimed.

Validation:

- `cargo test --offline --release --workspace -- --test-threads=1`: 458 passed,
  zero failed, 11 ignored across 27 targets. The ignores were six owner-data
  cases, four explicit long/performance cases, and the separately run ROC gate.
- Five resolver decision goldens and the explicit synthetic ROC gate passed.
- Compared against the immutable pre-maintenance executable (`5f48940`): all 15
  complete matching/sync cases and 18 CLI edge cases match, including errors,
  exit codes, audio-less videos, and empty axes. Only top-level wall-clock timing
  fields were normalized.
- Twenty RMS-on scenarios (36 complete rows) match the capture made immediately
  before candidate normalization, byte-for-byte with no fields excluded.
- Rebuilt CLI, FFI, and native Electron binding; generated `ffi/couple.h` is
  byte-identical to the original header.
- Swift release and dev-harness builds pass; 127 Swift tests pass against a fresh
  test build linked to the final static library.
- Electron: 83 tests, typecheck, native/production builds, smoke, and all five
  cross-shell parity scenarios pass.
- Added focused pre/post characterization for error envelopes, callback ownership,
  viewport and pivot behavior, legacy callback counts, fit ordering, and RMS row
  projection; matrix reuse tests compare complete rows across matcher modes,
  verifier/overflow/RMS settings, empty axes, and invalid matrix inputs.

Verification boundary: owner-only real-data suites and the explicitly ignored
long/performance tests were not exercised. The synthetic ROC gate was run explicitly.
The Swift GUI export harness could not launch through sandboxed LaunchServices;
both app variants compile, and sixteen before/after export-option combinations
matched. Existing unrelated compiler warnings remain. No dependency versions changed.

### Follow-up — Revert matrix reuse and trim maintenance tests

Reverted item 12 in `38cf3eb`: matrix reuse introduced a caller provenance obligation
without measured benefit. Resolution again constructs its own matching matrices.
All other production simplifications remain; production source matches `bd28653`.

Removed 287 net lines of newly added tests/harness code: 89 with the reverted API
and 198 by consolidating ownership setup, removing duplicate parser/policy checks,
trimming unchanged UI behavior permutations, and replacing duplicated full-row JSON
fixtures with focused selection assertions. Preexisting tests remain. The retained
tests cover callback ownership, error boundaries, fit ordering, resolver choices,
and viewport/pivot behavior. Test/harness growth is now 572 lines versus 859;
production code is 191 lines smaller than the pre-maintenance baseline.

Follow-up validation: four resolver unit tests, all 124 Swift tests, all 64 Electron
tests, Electron typecheck, and fresh CLI/FFI/native builds pass. Repeated all 33 CLI
comparisons and 20 RMS scenarios against the baselines described above: no changes.
The C header remains byte-identical. The full Rust suite was not repeated for this
test-only pruning; the earlier verification and its boundaries are recorded above.

## 2026-11-01 — ffmpeg resolution skips candidates that cannot launch

Field failure: `mac/dist/Couple.app` bundled the dev Homebrew ffmpeg 8.0.1 in
`Contents/Helpers/` (make-app.sh copies whatever `COUPLE_FFMPEG`/`$PATH`
provides); a later `brew upgrade` to 9.0.2 deleted the 8.0.1 Cellar dylibs the
copy hard-codes, so the bundled helper died in dyld and every import surfaced
as "unsupported format" with the raw dyld tried-paths as the reason.

SPEC-CHANGE (2026-11-01): §3.3 binary resolution — a candidate must not only
exist but also launch (`<tool> -version` exits 0) to be selected; a present but
non-launching candidate is skipped (recorded in `FfmpegMissing.searched` as
"present but failed to launch") and resolution falls through to the next tier,
typically a working `$PATH` install. Paths that pass once are cached for the
process lifetime, so the steady-state cost is one extra spawn per tool path
(~10 ms, §3.3 already deems spawn cost irrelevant at job granularity). Shells
unaffected (couple.h unchanged). Two unit tests in `core/src/ffmpeg.rs` (unix):
broken configured tool falls through to PATH; no-fallback case names the
launch failure.

## 2026-11-01 — `make-app.sh --dist`: pinned static ffmpeg for bundles

Follow-through on the dyld failure above: the fragile part was *what* got
bundled, not bundling itself. New `mac/Scripts/fetch-ffmpeg.sh` downloads a
pinned ffmpeg/ffprobe 9.0.2 static pair (macOS arm64, ffmpeg.martin-riedl.de
build 1789931890_9.0.2), verifies sha256 against in-repo pins, requires
`otool -L` to show only `/System` + `/usr/lib`, and requires both tools to run
`-version`; cached under `mac/vendor/ffmpeg/<build-id>/` (gitignored).
`make-app.sh --dist` bundles that pair; the default dev build still copies
`COUPLE_FFMPEG`/`$PATH` but now warns when the copy links non-system dylibs.

License note (recorded in the script header): the pinned build bundles
x264/x265, i.e. GPL-configured — fine for owner use; before public
distribution swap the pin for an LGPL-configured static build (any URL +
sha256 works) or meet GPL terms. SPEC §11.5's "ship the pinned LGPL build"
intent stands; this wires the mechanism and pins what exists today.

## 2026-11-03 — A/B monitor: Camera / External solos in the HUD picker

Owner request: a one-click way to compare the camera audio with the coupled
external audio (previously mute one lane, unmute the other). Interactive
mockup of three placements → owner chose extending the monitor picker
(UX-DECISIONS 2026-11-03 entry).

SPEC-CHANGE (2026-11-03): §4.8 `MonitorMode` gains two variants,
`CameraOnly` (wire `"camera"`, ABI code 3) and `ExternalOnly` (`"external"`,
code 4): L = R = g_cam·cam, resp. L = R = g_ext·ext (the Pan L/R channel in
both ears; user gains/mutes multiply in as before). Existing codes 0–2 and
their matrices are unchanged; the crossfade `prev_flags` byte widens its
monitor field from 2 to 3 bits (integer-shift moves from bit 2 to bit 3 —
internal). `cpl_audition_open` / `cpl_audition_set_mode` accept the new codes.
ADDITIVE — `cpl_abi_version` stays 1. Core test
`audition::solo_modes_are_the_pan_channels_in_both_ears`.

- New pure policy `core/src/policy/monitor.rs` (`cpl_policy_monitor`): flip /
  both / select → next mode + the two memories (last solo, last both-mode);
  unit + proptest contract. Mirrors: `CoupleKit/MonitorPolicy.swift`
  (4 tests under `swift test`), `electron kit/Policies.monitorPlan`
  (4 vitest cases through the real binding).
- Swift shell: `MonitorMode.allCases` = [camera, pan, overlay, external]
  (picker order); `AuditionController.selectMonitor/flipMonitor/
  restoreBothMonitor` + `camSilenced/extSilenced`; HUD picker widened to
  236 pt; lanes dim when silenced; lane speaker chip grey-slash state;
  InputRouter `a` / `b`; footer hint. Electron shell: same surface
  (`AuditionController.ts`, `VideoHero.tsx`, `WaveformStage.tsx`,
  `editor.css .lane-mute.silenced`, `App.tsx` keys + hint).
- Dev harness: `COUPLE_MONITOR_TEST=1` (with `COUPLE_PAIR_DIR`) drives the
  picker + `a`/`b` keys through the live policy and InputRouter, checks lane
  silence feedback, and shoots `10-ab-camera-only.png` / `11-ab-external-only.png`
  into `COUPLE_SHOT_DIR`.
- SHELL-UI-PARITY.md: video-hero, per-lane, and keymap lines updated.

## 2026-11-03 — Dist bundle ships an LGPL ffmpeg built from pinned source

Closes the license note from the 2026-11-01 entry. Couple shells out to
ffmpeg/ffprobe over argv/pipes (never links libav*), stream-copies video on
export and encodes audio with AudioToolbox / native `aac` / PCM, so nothing
GPL (x264/x265) or nonfree is needed — but the pinned martin-riedl.de build
was GPL-configured while the bundle's `NSHumanReadableCopyright` claimed LGPL.
No public static macOS build we could vouch for is LGPL-configured, so
`mac/Scripts/fetch-ffmpeg.sh` now **compiles** ffmpeg 9.0.2 from the
ffmpeg.org tarball (sha256-pinned; GPG-verified against the FFmpeg release
signing key when `gpg` is present) with `--disable-autodetect` and no external
libraries; links only /System and /usr/lib, so the unmodified tarball plus the
configure line *is* the corresponding source. Same stdout contract as before
(one line, the helper dir); cached under `mac/vendor/ffmpeg/<ver>-lgpl-<arch>/`
with `licenses/{COPYING.LGPLv2.1,LICENSE.md}` and a `SOURCE.txt` (tarball
URL, sha256, key, configure line). New gate `check_lgpl`: `ffmpeg -L` must
report the LGPL and the configuration must contain neither `--enable-gpl` nor
`--enable-nonfree`. SPEC §11.5's "ship the pinned LGPL build" is now met.

- `make-app.sh --dist` copies the license texts + `SOURCE.txt` to
  `Contents/Resources/Licenses/ffmpeg/`, writes an accurate copyright string,
  and signs the helpers *without* `disable-library-validation` (static build
  loads nothing foreign). Dev builds keep the entitlement (Homebrew dylibs)
  and now label the bundle "Development build … not for distribution".
- New `docs/LICENSING.md`: the LGPL rule, how it is enforced, the per-release
  obligations (attach the source tarball to each public release; three-year
  window), Electron bundling rules (lgpl variants only), open items
  (Rust/Swift/npm notices via `cargo about`; Acknowledgements view in both
  shells). CLAUDE.md doc map + build section updated.

## 2026-11-03 — Tag-triggered Electron release pipeline

Made the Electron app downloadable from GitHub. Nothing publishes on its own:
the owner pushes a `v*` tag, and even then the result is a draft release.

- **`.github/workflows/release.yml` (new).** On a pushed `v*` tag: build the
  napi binding + renderer on macos-15 / windows-latest / ubuntu-latest, run the
  binding smoke test, package with electron-builder, and attach the installers
  to a **draft** GitHub Release (`gh release create --draft`; a re-run refreshes
  the assets with `--clobber`). `workflow_dispatch` from a branch is a dry run
  (workflow artifacts only, no release). The tag is validated as
  `vMAJOR.MINOR.PATCH[-pre]` and stamped into `electron/package.json` with
  `npm version --no-git-tag-version`, so no bump commit is needed.
- **Versionless installer names.** `artifactName:
  ${productName}-${os}-${arch}.${ext}` in `electron/electron-builder.yml` →
  `Couple-mac-arm64.dmg`, `Couple-win-x64.exe`, `Couple-linux-x64.AppImage`,
  `Couple-linux-x64.deb`, so `releases/latest/download/<name>` links never go
  stale. Verified locally (mac).
- **ffmpeg on macOS is bundled** from `mac/Scripts/fetch-ffmpeg.sh` (the same
  pinned build as the Swift `--dist` bundle; since the 2026-11-03 LGPL entry
  above that is the from-source LGPL build, and the workflow copies the whole
  helper dir — binaries, `SOURCE.txt`, `licenses/` — so Acknowledgements shows
  the texts) into `electron/resources/ffmpeg/darwin/`, which
  `electron-builder.yml` already picked up as `extraResources`. Reason: the
  core's discovery chain ends at `$PATH`, and a Finder-launched app's PATH
  excludes `/opt/homebrew/bin`, so an unbundled mac build could not find a
  Homebrew ffmpeg at all. Windows/Linux bundling: next entry (2026-11-04).
- **`README.md` (new, root).** Plain download section with per-platform links,
  the unsigned-app first-launch steps (macOS Privacy & Security "Open Anyway" /
  `xattr -dr com.apple.quarantine`; Windows SmartScreen; Linux AppImage/.deb),
  requirements, build-from-source pointer, and the maintainer release how-to.
- `electron/package.json`: added `author` and `repository` (electron-builder
  warned about the missing author).
- Not done: code signing/notarization (Apple Developer account + Windows cert),
  Intel mac build (runner is arm64 only), website download buttons (still `#`;
  the website's "Download for Mac" refers to the Swift app).

## 2026-11-04 — Dependency notices + Acknowledgements view (LICENSING §3 closed)

Closes both open items from the 2026-11-03 entry.

- `scripts/third-party-notices.sh` (→ `scripts/third_party_notices.py`)
  generates the committed `third-party/` folder: `rust-crates.txt` (180
  crates over normal dependency edges from `cargo metadata`; dev/build deps
  excluded; crate license files inline, canonical MIT/Apache-2.0 text from
  `scripts/license-texts/` for the 21 crates that ship none; NOTICE files
  carried verbatim — none today), `npm-packages.txt` (react, react-dom,
  scheduler + the Electron runtime), `swift-packages.txt` (asserts
  `mac/Package.swift` has no external packages), `components.json` (the list
  the views render). SPDX expressions are evaluated against a permissive-only
  allowed set; unknown or copyleft (incl. MPL) fails the run. `--check` fails
  on a stale folder. Not `cargo about`: it reads the same registry sources
  without another tool to install.
- `make-app.sh` copies `third-party/` into `Contents/Resources/Licenses/`
  (dev and dist; dist runs `--check` first); `electron-builder.yml` copies it
  to `resources/Licenses/` via extraResources.
- About + Acknowledgements in both shells (SHELL-UI-PARITY "App-level"):
  Swift `Window` scenes `about` / `acknowledgements` (CommandGroup replaces
  `.appInfo`; Help ▸ Acknowledgements), `Licenses.swift` loader; Electron
  explicit application menu in `main/index.ts` (About → renderer overlay,
  Help ▸ Acknowledgements), `app/Licenses.ts`, `views/AboutView.tsx` +
  `AcknowledgementsView.tsx` + `about.css`. Loaders parse the `==== name
  version` sections of the notices files and the ffmpeg folder
  (`COPYING.LGPLv2.1`, `LICENSE.md`, `SOURCE.txt`; version from SOURCE.txt's
  first line). `COUPLE_LICENSES_DIR` overrides the folder; Electron harness
  `COUPLE_ACK_TEST=1` opens the view at startup for `COUPLE_SHOT_PATH`.
- docs/LICENSING.md §3 rewritten as the record of the pipeline; CLAUDE.md
  (doc map, layout, build commands, env) updated.


## 2026-11-04 — Windows and Linux installers bundle ffmpeg too

Closes the "Windows/Linux ffmpeg bundling" item from the 2026-11-03 release
entry. Testers on those platforms no longer install ffmpeg.

- **`electron/scripts/fetch-ffmpeg.mjs` (new, Node, runs on all three
  runners).** Downloads the pinned BtbN FFmpeg-Builds `-lgpl` static build for
  the target platform — release `autobuild-2026-10-31-12-59` (a month-end
  build; BtbN prunes dailies but keeps those), ffmpeg `n9.0.2-39-g7adfbe40dc`
  (release/9.0, same line as the mac pin), one asset + sha256 per platform
  computed from an inspected download, never the remote checksum file —
  extracts `bin/ffmpeg(.exe)` + `bin/ffprobe(.exe)`, and lays out
  `electron/resources/ffmpeg/<win32|linux>/` with `SOURCE.txt` (release URL,
  asset, sha256, ffmpeg commit, builder tag) and `licenses/{COPYING.LGPLv3,
  LICENSE.md}` (the archive's `LICENSE.txt` is byte-identical to ffmpeg's
  `COPYING.LGPLv3`, pinned; `LICENSE.md` fetched from the same ffmpeg commit,
  pinned). Gates: the configure line embedded in each binary must contain
  neither `--enable-gpl` nor `--enable-nonfree` (checked on any host, by
  reading the binary); when the host can run the binary, `-version` must
  succeed and `-L` must report the LGPL. `--platform win32|linux` overrides
  the host so both pins can be exercised from a Mac (run checks skipped).
  Cache `electron/vendor/ffmpeg/<tag>/` (gitignored). stdout = the destination
  dir, progress on stderr, like the mac script.
- **These builds are LGPL v3** (BtbN passes `--enable-version3`), not v2.1
  like the compiled mac build. `SOURCE.txt`'s first line says "LGPL v3 or
  later"; both license loaders (`app/Licenses.ts`, `Licenses.swift`) now take
  the license name from that line and also look for `COPYING.LGPLv3`, so
  Acknowledgements states the right license per platform. docs/LICENSING.md
  §2 and SHELL-UI-PARITY.md updated.
- **`release.yml`**: new "Bundle ffmpeg (Windows/Linux …)" step on the
  non-mac runners before "Typecheck + renderer build". The macOS step now
  copies the whole helper dir (`SOURCE.txt` + `licenses/` too, as main's
  from-source script emits them), not just the two binaries. Header comment
  and the draft-release notes' Requirements updated; README Requirements now
  says nothing to install on any platform.
- Verified on this Mac: `--platform linux` and `--platform win32` both
  download, hash-match, extract to the expected binaries, pass the
  configure-line check and produce the layout above. The `-version`/`-L` run
  checks will first execute on the real runners (next `workflow_dispatch`
  dry run).

## 2026-11-04 — Release dry run: ffmpeg source mirrored in-repo, inputs cached

First `workflow_dispatch` dry run of `release.yml` (run 41795729422): Windows
and Linux built and packaged with the bundled ffmpeg; macOS failed before
building anything — ffmpeg.org's HTTPS endpoint was down and the tarball
download timed out. The release build should not depend on a volunteer-run
site being up, and LICENSING §1 already required keeping the exact tarball
under our control. So:

- **`ffmpeg-source-9.0.2` prerelease** in this repo (not marked latest, so
  `releases/latest/download/...` keeps pointing at app builds) carries the
  sha256-verified `ffmpeg-9.0.2.tar.xz` (and its `.asc` once ffmpeg.org is
  reachable again). It is both the LGPL source-retention copy and the build
  input.
- **`mac/Scripts/fetch-ffmpeg.sh`** fetches the mirror first and ffmpeg.org
  second, with curl retries and a connect timeout; the sha256 pin applies
  either way. In CI (`$CI` set) the mirror is required — a pin bump that has
  not uploaded its tarball fails with the `gh release create` line to run.
  The FFmpeg release signing key is now committed
  (`mac/Scripts/ffmpeg-release-signing-key.asc`, fetched from
  keyserver.ubuntu.com by fingerprint) and its fingerprint is checked against
  the pin before use; the `.asc` is tried from both sources and, if neither
  answers, the signature check is skipped with a note (sha256 still gates).
  `SOURCE.txt` lists both URLs.
- **`release.yml`** caches `mac/vendor/ffmpeg` and the staged
  `electron/vendor/ffmpeg` output (archives excluded) with `actions/cache`,
  keyed on the two fetch scripts, so macOS stops recompiling ffmpeg on every
  run and Windows/Linux stop re-downloading 150 MB each.
- docs/LICENSING.md §1 (obligation 3 and the pin-bump recipe) and CLAUDE.md
  updated.
- Second dry run (41799099756) passed on all three OSes: macOS fetched the
  tarball from the mirror, compiled it and packaged; Windows and Linux ran the
  BtbN bundling with the `-version`/`-L` checks; ffmpeg caches saved. One
  catch: electron-builder's `${arch}` is `x86_64` for AppImage and `amd64` for
  deb, so Linux came out as `Couple-linux-x86_64.AppImage` /
  `Couple-linux-amd64.deb` while README and the release notes link
  `Couple-linux-x64.*`. `linux.artifactName` now spells `x64` out.

## 2026-11-04 — v0.1.0 tagged; draft release built

- `v0.1.0` tagged on main `ba5b115` and pushed; run 41867192527 built all
  three installers and created the **draft** release (publish by hand:
  https://github.com/timbeccue/couple/releases). Assets: `Couple-mac-arm64.dmg`
  (136 MB), `Couple-win-x64.exe` (176 MB), `Couple-linux-x64.AppImage`
  (248 MB), `Couple-linux-x64.deb` (198 MB).
- First attempt (tag on `5b59855`) failed on every runner at "Stamp version
  from tag": `npm version 0.1.0` exits 1 with "Version not changed" because
  `electron/package.json` already said 0.1.0. The dry runs never reached that
  step (tag-only). The step now skips the stamp when the versions match; the
  tag was deleted and re-pointed at the fixed commit.
- `--generate-notes` with no previous `v*` tag picked the nearest tag of any
  kind, so the draft's "Full Changelog" compared from `ffmpeg-source-9.0.2`.
  The release job now checks the repo out, generates the changelog only
  against the previous `v*` tag, and appends a Licensing section linking the
  `ffmpeg-source-<version>` prerelease (read from the pin in
  `mac/Scripts/fetch-ffmpeg.sh`) — docs/LICENSING.md §1 obligation 3. The
  v0.1.0 draft's notes were corrected by hand to the same text.
- Still open: upload `ffmpeg-9.0.2.tar.xz.asc` to the `ffmpeg-source-9.0.2`
  prerelease once ffmpeg.org is reachable so CI runs the GPG check; try the
  Windows and Linux installers on a machine without ffmpeg before publishing.

## 2026-11-05 — Compact mode (PLAN-COMPACT-MODE Phase 0)

Owner prototype rounds 3–5 in `design/mockups-compact/` (direction D — fixed
preview + list, animated ⤢ Full) became `PLAN-COMPACT-MODE.md` (TODO-016).
Phase 0 is decisions and docs only; no code. The twelve decisions in the
plan's §2 table are recorded below as SPEC-CHANGE entries; the user-facing
rules are UX-DECISIONS **§V2.11** (compact arrangement) and **§V2.12**
(include-for-export). Numbering note: the plan's first draft called those
two entries V2.7/V2.8; UX-DECISIONS already has a V2.7 (Media pool) and a
V2.8 (Export rail), both cited from Swift code and SPEC-V2, so the new entries
took the next free numbers and the plan was corrected the same day. Cite
V2.11/V2.12.

**OUTSTANDING GATE — realdata verdict check (Phase 0 step 1) not run.** After
D1 the verdict is the only gate before a file leaves the app, so the plan
requires `COUPLE_REALDATA_DIR=… scripts/realdata-sweep.sh` and the per-class
verdict confusion counts recorded here before D1 ships. This environment has
no `COUPLE_REALDATA_DIR` and `example_data/` holds no media (manifests only),
so the sweep could not run. It MUST run on the owner's machine, and the counts
MUST land in this entry, before Phase 1 merges to main. If a true pair lands
in "questionable" or a wrong take lands in "good", stop and recalibrate first
(`fixtures/matcher-corpus/CALIBRATION.md` §6–7).

**2026-11-05 (merge):** the owner merged `feature/compact-mode` to main with this
gate still open, choosing to assume the matcher holds for now. The check is
tracked as TODO-017 (owner-only); its counts land here when it runs.

SPEC-CHANGE (2026-11-05) [D1]: SPEC-V2 §2 item 4 ("Verified ≡ queued", one
opt-in flag, "Export n verified") and UX §V2.5 ("Approve ≡ queued") are
superseded by **include-for-export**. A couple is **included** in the export
batch when its verdict is good and it has not been exported yet, unless the
user toggled its row checkbox (an explicit opt-out, or an explicit opt-in
again after an export); a couple whose verdict is questionable or none ("pick
one" / "no audio") can never be included. The export button reads "Export N
files", N = included couples. The rule lives in core
`policy/export_include.rs` (`included(verdict, explicit, exported)`,
`can_include(verdict)`, `in_files_list(included, exported)`, `export_count`)
through the existing `cpl_policy_*` envelope; Swift mirror
`CoupleKit/ExportIncludePolicy.swift`. Inclusion is derived state — the shell
stores only the user's explicit toggle and `exportedAt` per couple; every
verdict change, undo, restore or sync completion re-derives it, and a manual
re-pair clears both stored values. **Loading note:** a `queue.json` saved
under the approve model carries neither field and its former `approved` flag
is not read, so it loads with **every good couple included** (the policy
default) and every exported/approved distinction gone. ADDITIVE —
`cpl_abi_version` stays 1.

SPEC-CHANGE (2026-11-05) [D2]: UX §V2.2's three sections QUEUED FOR EXPORT /
TO APPROVE / TO FIX become two: **To fix** (verdict questionable or none) and
**Ready** (verdict good). Membership is the verdict alone; inclusion does not
move a row between sections. Shell presentation; the verdict policy is
unchanged. No ABI change.

SPEC-CHANGE (2026-11-05) [D3]: UX §V2.2/§V2.3 — rows no longer show the
verdict dot or the amber coverage strip. The verdict is still computed by the
same policy and still drives sections and the action words ("pick one" red,
"no audio" grey, "syncing…" while pairing), which are now the only state
marks on a row. Coverage moves to the preview's two-lane waveform seek strip
(external lane drawn only where that audio covers the video). The verdict
colors stay on the action-word capsules and the candidate strength bar. No
core change.

SPEC-CHANGE (2026-11-05) [D4]: UX §V2.4/§V2.5 — the Approve button leaves the
editor's bottom bar and `v` is unbound (it no longer approves or advances;
SPEC-V2 §2 item 3's "`v` verifies and advances" is retired). Inclusion is
toggled by the row checkbox only (owner §6.5); the toggle keeps its undo case
(TODO-015). Shell-only; `InputRouter` + SHELL-UI-PARITY keymap.

SPEC-CHANGE (2026-11-05) [D5]: UX §V2.1 "everything visible at once; no modal
screens" stays true of the **full** arrangement. The window gains a second
**compact** arrangement (≈360 × 700: session line, fixed preview, couple list,
export panel) of the same window and session; it is not a screen you
navigate to — the editing surfaces it omits are one click away behind
⤢ Full, the toggle animates the frame and the four blocks, and nothing is
lost or reset by switching. UX §V2.11. Shell-only.

SPEC-CHANGE (2026-11-05) [D6]: `core/src/policy/row_merge.rs` — the
`protected` input (rows a re-pair over new files may not override) is
composed by the shell as **pinned only**, no longer approved ∨ pinned. With
default-on inclusion "included" no longer means a human looked at the row;
a manual pick (📌) does. Rust side: doc + test; mirror
`CoupleKit/RowMergePolicy.swift`; composition in `IngestController.swift`.
ADDITIVE — `cpl_abi_version` stays 1.

SPEC-CHANGE (2026-11-05) [D7]: `core/src/policy/media_removal.rs` — the
removal plan warns for **included** couples instead of approved ones (same
input, renamed meaning: the shell passes inclusion). The wire key keeps its
name until Phase 6 (D12). ADDITIVE — `cpl_abi_version` stays 1.

SPEC-CHANGE (2026-11-05) [D8]: export status is persisted. `queue.json`
(schema `couple-queue/1`, no bump) gains two optional fields per couple:
`exportedAt` (set on each successful export) and the user's explicit include
toggle. `exportedAt` drives the ✓ time on rows, the default-include rule
(exported ⇒ excluded unless explicitly re-included) and Files-list membership
(included ∪ exported). Shell-only state in `QueueStore`, like `pinned`.

SPEC-CHANGE (2026-11-05) [D9]: `core/src/policy/repair_drop.rs` — "Replace
approved pairing?" is asked only when the couple is **pinned** (a manual
pick), never for an included couple; the shell passes `pinned` where it
passed `approved`. Default-on inclusion would otherwise prompt on every drop.
Mirror `CoupleKit/RepairDropPolicy.swift`. ADDITIVE — `cpl_abi_version`
stays 1.

SPEC-CHANGE (2026-11-05) [D10]: UX 2026-11-03 — the full arrangement keeps the
4-way picker Camera · Pan L/R · Overlay · External with the A/B solos. The
**compact** arrangement shows a 3-way picker **Camera · Synced · External**
where Synced = Overlay; a remembered Pan L/R both-mode maps to Synced while
compact; `a`/`b` flip exactly as today. `policy::monitor` is unchanged — the
3-way picker is presentation. Shell-only.

SPEC-CHANGE (2026-11-05) [D11]: UX §V2.2 "empty sections stay visible,
dimmed" is narrowed: an empty **To fix** section is hidden in both
arrangements; **Ready** is always shown (dimmed with "—" when empty).
Shell-only.

SPEC-CHANGE (2026-11-05) [D12]: the policy-envelope wire keys `approved`,
`affectedApproved` and `ClearSessionCounts.approved` (`ffi/src/lib.rs`,
`electron kit/Policies.ts`) **keep their names until Phase 6** because the
Electron shell still reads them; from Phase 1 they carry "included" (D7) —
documented at both ends. Both sides rename in Phase 6. ADDITIVE —
`cpl_abi_version` stays 1.

- UX-DECISIONS.md: §V2.11 and §V2.12 appended; REVISED markers on §V2.1,
  §V2.2, §V2.3, §V2.4, §V2.5, §V2.6, the 2026-07-11 empty-session entry and
  the 2026-11-03 A/B entry, each pointing at V2.11/V2.12.
- SHELL-UI-PARITY.md: queue-rail header/sections/row lines, the center-column
  sync-tools line, the keymap, the export rail and the persistence/dialog
  lines rewritten to the include model; new "Compact arrangement" surface
  section with every required affordance from the mockup README rounds 4–5,
  each line citing the view it will live in and tagged "mac-only until
  Phase 6" (Electron keeps the approve form until then).
- TODO.md: TODO-016 → In progress.
- Plan inconsistencies found while writing (plan not edited here): Phase 4
  step 5 lists `a`/`b` among the compact no-op keys, but D10 says `a`/`b`
  keep flipping in compact — the docs follow D10 (keys work in both
  arrangements). The queue header's "N to export" status line (Phase 1 step
  5) is not in the design record's session line; recorded in PARITY as the
  plan states it.

## 2026-11-05 — Compact mode Phases 1–4 (PLAN-COMPACT-MODE)

Implemented on `claude/minimal-compact-ui-963b4a`, one commit per phase, mac
shell first (`Shell-parity: n/a mac-first per PLAN-COMPACT-MODE Phase 6` on
each; the Electron port is Phase 6). The plan was amended before Phase 1
after an independent review: inclusion is **derived shell state**
(`included = verdict good ∧ (explicit ?? exportedAt == nil)`) with the rule
in `core/src/policy/export_include.rs`; `PairSession.verified` is NOT reused
because core clears it on every sync success (`session.rs`), `resetAlignment`
clears it and the per-pair session file restores it — reusing it would have
silently dropped couples from the export after each auto-sync. D9–D12 were
added in the same amendment.

- **Phase 1a (d13dd6e)** — `export_include` policy (unit + proptests,
  98 policy tests), `cpl_policy_export_include` envelope (ABI 1, header
  regenerated), napi passthrough + `kit/Policies.ts exportInclude`, CoupleKit
  `ExportIncludePolicy` mirror (exact-rule fallback) + tests. `row_merge`
  protected = pinned only (D6); `media_removal` / `repair_drop` inputs carry
  included / pinned under the kept wire keys (D7, D9, D12).
- **Phase 1b (eb07be8)** — `CoupleRow.approved` → stored `explicit`/
  `exportedAt` + derived `included`/`inFilesList`; `queue.json` optional
  fields, schema string unchanged (a file saved under the approve model loads
  with every good couple included — D1 as recorded). AppModel ready/toFix,
  undoable `toggleInclude`, repair clears both inputs and `RepairSnapshot`
  restores them; batch export exports `included`, stamps `exportedAt` per
  success; `PairSession.exportAwait` no longer gates on the v1 `verified`
  flag (the batch rows are never verified). Approve button and `v` removed.
  Harness `COUPLE_INCLUDE_TEST` (23 checks).
- **Phase 2 (4bfbcd4)** — dot-free rows (no verdict spine, no coverage strip,
  "syncing…" text), ✓ HH:mm on exported rows, export rail in the D form with
  the Files list (included ∪ exported).
- **Phase 3 (86799df)** — `PairStrip` two-lane seek strip under the video;
  candidate audition through a scratch session at the candidate's lag
  (nothing pinned by listening); Change… / Undo / Compare others after a
  manual pick; `MonitorPicker` compact variant. Harness
  `COUPLE_CAND_AUDITION_TEST` (13 checks).
- **8f2c1cb** — export-status race found by the include harness: a buffered
  progress update could flip a row from `.done` back to `.running` after the
  job finished; the progress reader now stops once cancelled. Files,
  `exportedAt` and `queue.json` were never affected. The same unguarded loop
  exists in `MediaPool.swift`, `IngestController.swift` and two other
  `PairSession` loops — left for a separate change.
- **Phase 4 (cf0d1f2)** — SessionLine / CoupleList / PairPreview /
  ExportPanel extracted and shared by both arrangements; `CompactLayout`;
  `LayoutMode` persisted (default compact on first launch after the update,
  §6.1); per-arrangement frames; width clamped 340–420 while compact via
  `.windowResizability(.contentSize)` (manual `NSWindow.minSize/maxSize` was
  overwritten by SwiftUI, so the `.defaultSize` 1280×820 now lives in the
  layout's target frame); Pan ↔ Synced mapping (D10); View menu ⌘⇧C.
  Existing harness cases force the full arrangement; `COUPLE_COMPACT_TEST`
  (11 checks, 5 shots) and `COUPLE_LAYOUT_TOGGLE_TEST` (22 checks: playback
  continues across switches, frames and monitor mode round-trip) pass
  headless on a synthetic session (3 takes + a second recorder on take 2 +
  one video with no recorder audio; generated with ffmpeg lavfi sources —
  the resolver gives confident / 2 sources / confident / no match).
- Verification per phase: `cargo test --release -p couple-core --lib policy`,
  `cargo build --release -p couple-ffi`, both `swift build -c release`
  variants, `swift test` (134), the harness cases above. `golden-pair-rows`
  unaffected (no resolver decision changed). The realdata verdict gate from
  Phase 0 is STILL OUTSTANDING (no owner media on this machine).

## 2026-11-05 — Compact mode Phases 5–6; plan archived

- **Phase 5 (963c181)** — animated arrangement switch, explicit FLIP in
  `WindowLayout.animateLayoutSwitch`: the incoming layout is swapped in at its
  final size, the four blocks are drawn back on their old frames (translate +
  even scale) and eased home over 0.45 s while the window frame animates on
  the same curve; one-arrangement-only surfaces fade in after. The plan's
  first choice, `matchedGeometryEffect` across the HSplitView panes, gave
  wrong mid-move frames and leftover offsets and was dropped (reason kept in
  the code comment). Reduce Motion ⇒ instant. `COUPLE_LAYOUT_TOGGLE_TEST` now
  runs four animated switches (38 checks). Not yet watched live.
- **Phase 6 (ea25365)** — Electron parity: include model (explicit/exportedAt
  on `CoupleRow.ts`, `Policies.exportInclude`, queue file optional fields),
  D rows/rails, `PairStrip.tsx` canvas strip, candidate audition via a scratch
  session, compact arrangement as one DOM tree (`display: contents` + order,
  so `<video>` never remounts), frames remembered in `userData`, animated
  `setBounds`, ⌘⇧C / Ctrl+Shift+C, FLIP after the mockup's `toggleFullD`.
  Kit TS names renamed to pinned / included / affectedIncluded with the wire
  keys unchanged (D12 — the rename of the wire keys themselves is deferred:
  nothing is gained by breaking the envelope). Wire `Coverage` type fixed to
  core's `{t0,t1,seconds,fraction}` (the old strip read undefined fields).
  typecheck clean, vitest 76/76, `parity.mjs` byte-identical, smoke passes,
  COMPACT 13/13 and LAYOUT_TOGGLE 24/24 live. Known differences recorded in
  SHELL-UI-PARITY.md (audition "fit" only when core sends it; macOS window
  animation at system speed; Clear Session… in compact via a native context
  menu; confirmations are `window.confirm`).
- `PLAN-COMPACT-MODE.md` → `docs/archive/` (code cites it by name); CLAUDE.md
  doc map + Electron harness keys; TODO-016 Done; SHELL-UI-PARITY "mac-only
  until Phase 6" tags removed.
- Still open before merge to main: the realdata verdict check (Phase 0 step 1,
  needs `COUPLE_REALDATA_DIR`), a live look at the animated switch on a real
  window, and the unguarded progress loops noted under 8f2c1cb.
- Final verification on the branch tip: `cargo test --release --workspace`
  all green (no failures across every crate; the `#[ignore]`d realdata/ROC
  suites skipped as usual), `scripts/golden-pair-rows.sh` all scenarios
  match, `scripts/shell-drift-check.sh main` ok, `third-party-notices --check`
  up to date, `swift test` 134 passed, Electron typecheck/vitest 76/parity/
  smoke green (Phase 6).

## 2026-11-05 — Empty-state cleanup, drop-zone copy, shortcut reference (draft)

Drafted from the owner's annotated screenshot of the empty full window; recorded
as UX-DECISIONS §V2.13 (DRAFT — pending the owner's live look). Both shells.

- **Wordmark 22 pt** in the rail header (was 15). `SessionLine.swift` /
  `SessionLine.tsx`; SHELL-UI-PARITY updated.
- **Quiet empty session**: counts, "N to export", Clear Session… (full header),
  the compact line's counts, the Ready section and the pool's empty list +
  placeholder player are all gated on `hasContent` / a non-empty pool. The
  pool header ("MEDIA POOL · 0", "+ import") stays as the rail's bottom line.
  `SessionLine`, `CoupleList`, `QueueRail` (MediaPoolSection) in both shells.
  The compact empty state still mounts the (now empty) list block so the
  arrangement FLIP has its frame.
- **Drop zone**: "Drop audio and video files"; the rails-orientation paragraph
  removed. `LoadSurface.swift` lost its `compact` parameter (nothing differed
  any more); `LoadSurface.tsx` keeps the prop (it drives the margin class).
- **Shortcut reference** replaces the footer cheat strip — and the footer
  row itself (owner, same day: no window height for it): `ShortcutsButton`
  sits at the bottom-right of the export rail with the reference as its
  popover (`KeyboardShortcutsView.swift`, new; `FooterBar` removed from
  RootView); `AppModel.shortcutsShown`; `?` toggles and Esc closes it
  (`InputRouter`); Help ▸ Keyboard Shortcuts ⌘/ (`CoupleApp.swift`
  HelpCommands, replacing AcknowledgementsCommands); compact presents it as a
  sheet (RootView) with the compact-only rows. Electron: `ShortcutsView.tsx`
  (modal overlay, `useModalKeys`), footer button, `?` key, Help menu item
  `CmdOrCtrl+/` → `couple:show-shortcuts`; the rail button
  (`ShortcutsButton` in ShortcutsView.tsx, placed by CompactLayout.tsx) raises
  a DOM event App.tsx listens for. The `.footer-bar` CSS is gone.
- Not a SPEC-CHANGE: §2.1 / §12 never specified the empty rail's contents or
  the footer; the keys themselves are unchanged.
