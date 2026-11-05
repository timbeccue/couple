# PLAN — Compact mode and the include-for-export model

Status: implementation plan, not started; owner decisions in §6 resolved. Written 2026-11-05 from the owner's
prototype rounds in `design/mockups-compact/` (direction **D**, rounds 3–5;
README there is the design record). Stays in the repo root while in progress;
`git mv` to `docs/archive/` when Phase 6 ships.

Repository root: `/Users/tim/code/claude_apps/couple`

---

## 1. Objective

One window, two arrangements, one rule for what gets exported.

1. **Compact arrangement** (~360 × 700): session line, a fixed preview block
   (video, two-lane waveform seek strip, monitor picker, offset/fit line, or
   the candidate picker for a "pick one" couple), the couple list, and the
   export panel. No loupes, overview, nudges, anchors or media-pool list.
2. **Full arrangement** = today's three columns, with the left rail replaced
   by the compact list and the export rail by the compact export panel plus a
   Files list. Overview, loupes and sync tools unchanged.
3. **⤢ Full / ⤡ Compact** in the title bar switches arrangements; the window
   frame animates and the four blocks move to their new places.
4. **Include-for-export replaces approve.** A couple with a good verdict is
   included by default; the row checkbox is the opt-out; "Export N files"
   counts included couples. Flagged couples ("pick one", "no audio") cannot be
   included until fixed. No Approve button, no `v`-approve walk, no QUEUED FOR
   EXPORT section.
5. **Rows carry no status dots.** State is the words "pick one" / "no audio"
   (and "syncing…" while pairing). The amber coverage strip leaves the
   thumbnail; coverage is drawn in the preview's waveform strip instead.
6. **Candidate audition**: ▶ on a candidate plays the video with that audio at
   its best-fit lag before anything is pinned. After "use": Undo and
   "Compare others" stay one click away.

Out of scope: audio polish, any change to pairing/sync decisions, the Electron
shell's own layout until Phase 6, a menu-bar or floating-panel variant.

---

## 2. Decisions this plan makes (and records as SPEC-CHANGE in Phase 0)

| # | Today | After | Where |
|---|---|---|---|
| D1 | Approve ≡ queued; opt-in; "Export (n approved)" (SPEC-V2 §2 item 4, UX V2.5) | **Included** flag, default = verdict good ∧ not yet exported; opt-out checkbox; "Export N files" | SPEC-V2 §2.4, UX-DECISIONS new V2.8 |
| D2 | Sections QUEUED FOR EXPORT / TO APPROVE / TO FIX (UX V2.2) | **To fix / Ready** | UX V2.2 amended |
| D3 | Row shows verdict dot + amber coverage strip (UX V2.2, V2.3) | No dot, no strip on rows; verdict still computed (policy unchanged) and drives sections + action words; coverage drawn in the preview strip | UX V2.2/V2.3 amended |
| D4 | Approve button in editor bar; `v` approves and advances (UX V2.4) | Both removed; `v` unbound. Inclusion is the row checkbox only | UX V2.4 amended, PARITY keymap |
| D5 | "Everything visible at once; no modal screens" (UX V2.1) | Still true of the **full** arrangement. The compact arrangement is a second arrangement of the same window and session, not a screen you navigate to; the editing surfaces it omits are one click away and the toggle is animated | UX new V2.7 |
| D6 | `row_merge` protects approved ∨ pinned rows from re-pair overrides | protects **pinned** only. With default-on inclusion, "included" no longer means "a human looked at this"; pinning (manual pick) does | `core/src/policy/row_merge.rs`, mirror, SPEC-CHANGE |
| D7 | `media_removal` warns for approved couples | warns for **included** couples (same input, renamed) | `media_removal.rs` |
| D8 | Export status not persisted | `exportedAt` persisted per couple in `queue.json`; drives the ✓ time on rows, the default-include rule and the Files list | QueueStore |

Caveat carried from the design round: after D1 the verdict is the only gate
before a file leaves the app. **Phase 0 runs the realdata sweep** and records
the verdict confusion counts in PROGRESS.md before D1 ships. If a true pair on
the owner corpus lands in "questionable" or a wrong take lands in "good", stop
and recalibrate first (`fixtures/matcher-corpus/CALIBRATION.md` §6–7).

---

## 3. Boundary placement (PLAN-CORE-SHELL-BOUNDARY §1)

Decisions in Rust, presentation in Swift:

- **New policy `core/src/policy/export_include.rs`** — pure functions, proptest:
  - `default_included(verdict: Verdict, exported_before: bool) -> bool`
  - `can_include(verdict) -> bool` (good only)
  - `export_count(rows: &[IncludeInput]) -> usize`
  - `include_after_resolve(prev: Option<IncludeState>, new_verdict, exported) -> IncludeState`
    where `IncludeState { included: bool, explicit: bool }` — an explicit user
    toggle survives re-resolve while the verdict stays includable; a verdict
    that drops to questionable/none clears inclusion; a return to good restores
    the default.
  - Exposed via the existing `cpl_policy_*` envelope (`ffi/src/lib.rs`
    ~1753–1938); Swift mirror `mac/Sources/CoupleKit/ExportIncludePolicy.swift`
    file-for-file. `cpl_abi_version` stays 1 (additive).
- **Reuse the existing flag**: `PairSession.verified` / `cpl_set_verified`
  (ffi `lib.rs:670`, core `session.rs:61,221`) becomes the storage for
  "included". No ABI change; rename only in shell copy and `CoupleRow`
  (`approved` → `included`, `Couple.swift:91`).
- `row_merge.rs:52` drops the approved input (D6); `media_removal.rs:38`
  keeps it under the new name (D7). Update both Swift mirrors and their tests.
- **Shell-only state** (as `pinned` is today): `explicit` include toggles,
  `exportedAt`, the layout mode. Persisted in `queue.json` / UserDefaults.
- Candidate audition needs **no core change**: core reads
  `session.offset_ms()` on every audition fill (`core/src/audition.rs:183,246`).
- Waveform strip needs **no core change**: both lanes come from `cpl_band`
  (`TrackHandle.band`, `Track.swift:55`), same source as `OverviewStrip`.

---

## 4. Phases

Each phase is independently shippable and ends with the doc/parity updates it
owes. Commits touching `mac/Sources/Couple/Views/` without the Electron views
carry `Shell-parity: n/a mac-first per PLAN-COMPACT-MODE Phase 6` until Phase 6.

### Phase 0 — Decisions, docs, calibration check (S)

1. `COUPLE_REALDATA_DIR=… scripts/realdata-sweep.sh`; record verdict counts
   per class in PROGRESS.md under this plan's name. Gate on §2's caveat.
2. PROGRESS.md: SPEC-CHANGE entries for D1–D8 (format as the 2026-11-03
   `MonitorMode` entry; mark ADDITIVE/ABI 1).
3. UX-DECISIONS.md: append V2.7 (compact arrangement) and V2.8 (include
   model); `REVISED` markers on V2.1, V2.2, V2.3, V2.4, V2.5.
4. SHELL-UI-PARITY.md: rewrite the queue-rail section lines (:23, :32–34),
   the center-column approve line (:99), keymap (:112), export (:116); add a
   "Compact arrangement" surface section with every required affordance from
   `design/mockups-compact/README.md` round 4–5; tag each line
   "mac-only until Phase 6".
5. TODO.md: TODO-016 pointing here (added with this plan).

Acceptance: docs merged, no code.

### Phase 1 — Include model in the full window (M)

Files: `core/src/policy/{export_include.rs,mod.rs,row_merge.rs,media_removal.rs}`,
`ffi/src/lib.rs`, `mac/Sources/CoupleKit/{ExportIncludePolicy,RowMergePolicy,MediaRemovalPolicy}.swift`,
`mac/Sources/Couple/{AppModel,QueueStore,Couple,PairSession,InputRouter,BatchExport}.swift`,
`Views/{QueueRail,CenterColumn,ExportRail}.swift`.

1. Policy + envelope + mirror + proptests + Swift mirror test.
2. `AppModel`: `queued/toApprove/toFix` (:227–232) → `ready/toFix`; `included`
   derived through the policy on every resolve result (`include_after_resolve`);
   `toggleApprove` (:272) → `toggleInclude` (undo case retained: TODO-015).
3. `QueueStore`: persist `explicit` + `exportedAt` (schema bump, old files load
   with `explicit=false`, which yields the policy default — i.e. a session saved
   under the approve model comes back with all good couples included; call this
   out in the SPEC-CHANGE).
4. `BatchExport.swift:62` exports `model.included`; on success stamp
   `exportedAt`, clear inclusion (default rule now returns false for exported).
   "Reveal in Finder" stays. Toast copy unchanged.
5. `QueueRail`: two sections; checkbox bound to `included`, hidden for
   non-includable rows; header status line "N to export".
6. `CenterColumn` bottom bar: remove Approve/Approved. `InputRouter`: unbind
   `v` (:122/:151) — inclusion is checkbox-only (§6.5).
7. `ExportRail`: button "Export N files" / "Export" disabled at 0.
8. DebugDrive: `COUPLE_INCLUDE_TEST` — resolve the harness session, assert
   defaults, toggle one off, export, assert `exportedAt` set and count drops;
   screenshot. Update `queueTest`/`queueNavTest` for the two-section model.

Acceptance: `cargo test --release --workspace` green incl. new proptests;
`scripts/golden-pair-rows.sh` unchanged (no resolver decision moved);
`swift build` both variants; harness case passes; PARITY lines for this phase
updated.

### Phase 2 — Rows and rails to the D form (S)

Files: `Views/{QueueRail,ExportRail}.swift`, `Theme.swift`.

1. Row: drop the verdict dot and the coverage strip (`QueueRail.swift:222`);
   the syncing pulse becomes "syncing…" in the audio line. Keep the thumbnail,
   video name + duration, audio name, action word capsule, 📌, checkbox.
2. Export rail → the D panel: INTO (path + Change…), AS (segmented
   Auto/MOV/MP4/WAV), "keep camera audio as a muted track" on its own line,
   Export N files, then **Files** (one `<stem>_synced.<ext>` per included
   couple with the existing ↺ reset and per-file status) replacing ROWS.
3. Remove `verdictGood/Questionable/None` dot usages from rows only; the
   colors stay for the action-word capsules and the candidate strength bar.

Acceptance: screenshot harness `COUPLE_QUEUE_SHOT_DIR` run re-baselined;
PARITY updated.

### Phase 3 — Preview strip and candidate audition (M)

Files: new `Views/PairStrip.swift`; `Views/CenterColumn.swift`,
`Views/EditorComponents.swift`, `AuditionController.swift`, `Resolver.swift`.

1. **PairStrip**: two lanes from `TrackHandle.band` at strip resolution
   (~2 px/column), camera above in `Theme.camera`, external below in
   `Theme.external` placed in video time using the pair's lag and
   `RowCandidate.coverage` (t0/t1); faint baseline outside coverage; playhead;
   click/drag seeks (direct-manipulation rule from the 2026-06-12 review:
   scrub, never snap). Replaces the plain scrubber under the video in both
   arrangements. The overview stays as the editing surface (full only).
2. **Candidate audition**: in `ResolveBanner`, ▶ per candidate opens a scratch
   session (`engine.sessionNew` → `setOffsetMs(cand.lagMs)` → `auditionOpen` →
   `AuditionController.attach`), monitor Overlay, strip redrawn with the
   candidate's coverage; ■ or selecting another row tears it down. "use" keeps
   calling `model.repair` (pins + re-sync, undoable via `RepairSnapshot`).
3. After a manual pick the preview title gets **Change…** and a line
   "Audio chosen by you · Undo · Compare others": Undo = the existing ⌘Z
   inverse; Compare = reopen the existing "Audio source — alternates
   available" banner **inline under the player** with the current candidate
   marked (§6.3; no popover).
4. Compact-facing monitor picker: Camera · Synced · External, where Synced =
   Overlay (MonitorPolicy decides availability as today); the full arrangement
   keeps the complete picker incl. the A/B solos.
5. DebugDrive: `COUPLE_CAND_AUDITION_TEST` (open an ambiguous row, audition
   candidate 2, assert scratch session offset, screenshot).

Acceptance: harness passes; audition latency unchanged (no extra decode: the
scratch session reuses cached tracks); PARITY updated.

### Phase 4 — Compact arrangement (L)

Files: `AppModel.swift` (`layoutMode`), `CoupleApp.swift`, `Views/RootView.swift`,
new `Views/{CompactLayout,SessionLine,PairPreview,CoupleList,ExportPanel}.swift`
extracted from `QueueRail`/`CenterColumn`/`ExportRail`.

1. Extract the four blocks as standalone views used by **both** layouts (the
   full rails become thin containers around them). This is the refactor that
   makes Phase 5 possible; land it with no visible change first.
2. `LayoutMode { compact, full }` on `AppModel`, persisted in UserDefaults;
   **default `compact`** on the first launch after the update (§6.1), then
   last-used.
3. `RootView`: `switch layoutMode` → `CompactLayout` (VStack: SessionLine,
   PairPreview with the picker/no-audio variants, CoupleList, ExportPanel,
   ActivityStrip) or today's HSplitView. Empty session: drop target fills the
   preview area, export panel pinned (round 4).
4. Window: on toggle, `NSWindow.setFrame(_:display:animate:)` to the last
   compact frame (default 360 × 700; width clamped 340–420 via
   `minSize`/`maxSize` while compact, height free, min 560 — §6.2) or the
   last full frame (min 640 × 400 as today, default 1280 × 820). Title-bar
   button "⤢ Full" / "⤡ Compact"; menu item + ⌘⇧C.
5. Keys in compact: j/k, space, Return (switch to full with the selected
   row's editor open), esc; nudges/[ ]/l/a/b no-ops. Media pool: "+ Add"
   only (§6.4) — the pool list and its player are full-arrangement surfaces.
6. Audition and player instances are owned by controllers, not views, so a
   layout switch never restarts playback.
7. DebugDrive: `COUPLE_COMPACT_TEST` (shots of empty / synced / pick-one /
   exporting in compact) and `COUPLE_LAYOUT_TOGGLE_TEST` (toggle twice, assert
   frames and that selection, playhead and inclusion survive).

Acceptance: both arrangements pass the harness; `swift test`; PARITY compact
section lines all cite a view; CLAUDE.md flags/env updated with the new
`COUPLE_*_TEST` keys and the layout default.

### Phase 5 — Animated transition (M, risk-isolated)

1. Put both layouts in one `ZStack` with a shared `@Namespace`;
   `matchedGeometryEffect` on SessionLine, PairPreview, CoupleList,
   ExportPanel; `withAnimation(.spring(duration: 0.45))` on the mode change,
   window frame animated with the same duration; surfaces unique to one
   arrangement (overview, loupes bar, Files, media pool) fade in after.
2. Known risk: `matchedGeometryEffect` across `HSplitView` children and the
   `NSViewRepresentable` video view. Mitigation order: (a) keep the video
   NSView a single instance re-parented; (b) if geometry matching glitches,
   animate the four blocks' frames explicitly with `GeometryReader`-measured
   rects (the FLIP the HTML mockup does); (c) fallback: crossfade + window
   frame animation only. Ship whichever is clean; record which in PROGRESS.
3. Respect "Reduce motion": instant switch.

Acceptance: harness shot mid-animation not required; a manual check that
playback continues through the toggle and no layout leaves stray offsets.

### Phase 6 — Electron parity (M)

`electron/src/renderer/src/views/{QueueRail,CenterColumn,ExportRail}.tsx`,
`app/AppModel.ts`, `CoupleRow.ts`, a new `CompactLayout.tsx`, CSS layout switch
+ FLIP (`design/mockups-compact/index.html` `toggleFullD` is the reference),
`BrowserWindow.setBounds` animated. The include policy comes through the same
`cpl_policy_*` envelope via `couple-node`; `scripts/parity.mjs` rows must stay
byte-identical to CLI rows (they will: no resolver change). Remove the
`Shell-parity: n/a` opt-outs and the "mac-only" tags in SHELL-UI-PARITY.md.

---

## 5. Verification matrix

| Check | Phases |
|---|---|
| `cargo test --release --workspace` (new `export_include` proptests; row_merge/media_removal tests updated) | 1 |
| `scripts/golden-pair-rows.sh` unchanged | 1 |
| `cd mac && swift build -c release` and `-DCOUPLE_DEVHARNESS`; `swift test` | 1–5 |
| DebugDrive: `COUPLE_INCLUDE_TEST`, `COUPLE_CAND_AUDITION_TEST`, `COUPLE_COMPACT_TEST`, `COUPLE_LAYOUT_TOGGLE_TEST`; existing `queueTest`/`queueNavTest` updated | 1, 3, 4 |
| `scripts/shell-drift-check.sh` quiet (opt-out line) / clean (Phase 6) | all |
| `electron: npm run parity && npm test` | 6 |
| Live check: dev-harness bundle launched beside the installed /Applications/Couple.app | 4, 5 |

---

## 6. Owner decisions (2026-11-05)

Resolved the same day the plan was written; the phases above are updated to
match.

1. **Default arrangement after upgrade: compact.** First launch after the
   update opens compact; the last-used arrangement is remembered after that.
2. **Compact window: free height, narrow width.** Width stays within
   340–420; height is free (min 560); the couple list absorbs extra height.
3. **"Compare others" is inline under the video**: the same candidate list
   the pick used, with the current one marked. Nothing floats over content.
4. **Media pool in compact: "+ Add" only.** The pool list, its player and
   unused-file info are full-arrangement surfaces.
5. **`v` is unbound.** Inclusion is toggled by the checkbox only; no key.
