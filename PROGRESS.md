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
