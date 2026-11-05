# PLAN — Compact mode and the include-for-export model

Status: implementation plan, in progress on `claude/minimal-compact-ui-963b4a`; owner decisions in §6 resolved; amended after an independent review (D9–D12, derived inclusion). Written 2026-11-05 from the owner's
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
| D1 | Approve ≡ queued; opt-in; "Export (n approved)" (SPEC-V2 §2 item 4, UX V2.5) | **Included** flag, default = verdict good ∧ not yet exported; opt-out checkbox; "Export N files" | SPEC-V2 §2.4, UX-DECISIONS new V2.12 |
| D2 | Sections QUEUED FOR EXPORT / TO APPROVE / TO FIX (UX V2.2) | **To fix / Ready** | UX V2.2 amended |
| D3 | Row shows verdict dot + amber coverage strip (UX V2.2, V2.3) | No dot, no strip on rows; verdict still computed (policy unchanged) and drives sections + action words; coverage drawn in the preview strip | UX V2.2/V2.3 amended |
| D4 | Approve button in editor bar; `v` approves and advances (UX V2.4) | Both removed; `v` unbound. Inclusion is the row checkbox only | UX V2.4 amended, PARITY keymap |
| D5 | "Everything visible at once; no modal screens" (UX V2.1) | Still true of the **full** arrangement. The compact arrangement is a second arrangement of the same window and session, not a screen you navigate to; the editing surfaces it omits are one click away and the toggle is animated | UX new V2.11 |
| D6 | `row_merge` protects approved ∨ pinned rows from re-pair overrides | protects **pinned** only. With default-on inclusion, "included" no longer means "a human looked at this"; pinning (manual pick) does | `core/src/policy/row_merge.rs`, mirror, SPEC-CHANGE |
| D7 | `media_removal` warns for approved couples | warns for **included** couples (same input, renamed) | `media_removal.rs` |
| D8 | Export status not persisted | `exportedAt` persisted per couple in `queue.json` (optional field, no schema bump); drives the ✓ time on rows, the default-include rule and the Files list | QueueStore |
| D9 | `repair_drop` asks "Replace approved pairing?" when audio is dropped on an approved couple | asks only when the couple is **pinned** (a manual pick); default-on inclusion would otherwise prompt on every drop | `core/src/policy/repair_drop.rs` input semantics, mirror |
| D10 | Monitor picker is 4-way Camera · Pan L/R · Overlay · External + A/B solos (UX V2 + 2026-11-03) | Full arrangement unchanged. **Compact** shows Camera · Synced · External (Synced = Overlay); `a`/`b` keep flipping as today; a remembered Pan selection maps to Synced in compact | UX 2026-11-03 amended |
| D11 | Empty queue sections stay visible, dimmed (UX V2.2) | An empty **To fix** section is hidden in both arrangements; Ready is always shown | UX V2.2 amended |
| D12 | Wire keys `approved` / `affectedApproved` / `ClearSessionCounts.approved` in the policy envelope | Keep the key names until Phase 6 (Electron reads them); document that they carry "included". Rename both sides in Phase 6 | `ffi/src/lib.rs`, `kit/Policies.ts` |

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
- **Do not reuse `PairSession.verified`** (review 2026-11-05): core clears it
  on every sync success (`core/src/session.rs:355`), `PairSession.load` →
  `resetAlignment` clears it (`PairSession.swift:220`), `pullStateFromCore`
  overwrites it (:347) and the per-pair session file restores it (:854).
  Inclusion is **derived shell state with the rule in core**:
  `included = verdict == good && (explicit ?? exportedAt == nil)` from two
  stored optionals on `CoupleRow` — `explicit: Bool?` (user toggle) and
  `exportedAt: Date?` — persisted as optional fields in `queue.json`. No
  recompute points: every verdict change, undo, restore or sync completion
  re-derives it. Files list membership = included ∪ exported (rule in core).
  `verified` stays as the v1 editor flag it is; nothing new reads it.
- **Repair** (manual pick / drop): clears `explicit` and `exportedAt` for the
  row (the old export is stale; the default rule applies again).
  `RepairSnapshot` (AppModel.swift:299–356) captures and restores both.
- `row_merge`'s `protected` is composed in `IngestController.swift:248` as
  approved ∨ pinned → pinned only (D6); Rust side is doc + test. `media_removal`
  and `repair_drop` inputs: media removal gets `included`, repair-drop gets
  `pinned` (D7, D9); keep wire key names (D12).
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
2. PROGRESS.md: SPEC-CHANGE entries for D1–D12 (format as the 2026-11-03
   `MonitorMode` entry; mark ADDITIVE/ABI 1).
3. UX-DECISIONS.md: append V2.11 (compact arrangement) and V2.12 (include
   model); `REVISED` markers on V2.1, V2.2, V2.3, V2.4, V2.5, V2.6 (candidate
   audition + Compare), the 2026-11-03 A/B entry (compact picker) and the
   2026-07-11 empty-session entry (compact empty state).
4. SHELL-UI-PARITY.md: rewrite the queue-rail section lines (:23, :32–34),
   the center-column approve line (:99), keymap (:112), export (:116); add a
   "Compact arrangement" surface section with every required affordance from
   `design/mockups-compact/README.md` round 4–5; tag each line
   "mac-only until Phase 6".
5. TODO.md: TODO-016 pointing here (added with this plan).

Acceptance: docs merged, no code.

### Phase 1 — Include model in the full window (M)

Files: `core/src/policy/{export_include.rs,mod.rs,row_merge.rs,repair_drop.rs,media_removal.rs}`,
`ffi/src/lib.rs` + `ffi/couple.h` (cbindgen), `electron/native/src/lib.rs` +
`index.d.ts` + `src/renderer/src/kit/Policies.ts` (binding only, no UI),
`mac/Sources/CoupleKit/{ExportIncludePolicy,RowMergePolicy,RepairDropPolicy,MediaRemovalPolicy}.swift`,
`mac/Sources/Couple/{AppModel,QueueStore,Couple,IngestController,InputRouter,BatchExport,PoolController,DebugDrive}.swift`,
`Views/{RootView,QueueRail,CenterColumn,ExportRail,LoadSurface}.swift`,
`CoupleKit/ActivitySummary.swift`, `mac/Tests/CoupleKitTests`.

1. Policy `export_include.rs`: `included(verdict, explicit: Option<bool>, exported: bool)`,
   `can_include(verdict)`, `in_files_list(included, exported)`,
   `export_count(&[…])`; proptests (explicit overrides default while good;
   non-good never included; exported ∧ ¬explicit ⇒ excluded). Envelope entry
   in `ffi/src/lib.rs`, header regenerated, node binding + `Policies.ts`
   passthrough (thin), Swift mirror = thin wrapper over the envelope with the
   conservative fallback (Phase-E style), mirror test.
2. `CoupleRow`: `approved` → computed `included` + stored `explicit`,
   `exportedAt`; `QueueStore` persists both as optional fields (schema stays
   `couple-queue/1`, old files load with nil ⇒ policy default — all good
   couples included; call this out in the SPEC-CHANGE).
3. `AppModel`: `queued/toApprove/toFix` (:227–232) → `ready/toFix` (verdict
   only); `toggleApprove` (:272) → `toggleInclude` sets `explicit` (undo case
   kept, TODO-015); repair clears `explicit`/`exportedAt`; `RepairSnapshot`
   carries both. Every other `approved` reader moves to `included`/`pinned`
   per D6/D7/D9: `IngestController.swift:248`, `RootView.swift:54–123,200`,
   `LoadSurface.swift:36`, `PoolController.swift:124–251`,
   `ActivitySummary.swift:110`, `CoupleKitTests:906`.
4. `BatchExport.swift:62` exports `model.included`; on each success stamp
   `exportedAt` (inclusion then derives to false unless `explicit == true`).
   "Reveal in Finder" stays. Toast copy unchanged.
5. `QueueRail`: two sections (To fix hidden when empty, D11); checkbox bound
   to `included`, shown for includable rows only; header status "N to export".
6. `CenterColumn` bottom bar: remove Approve/Approved. `InputRouter`: unbind
   `v` (:122/:151) — inclusion is checkbox-only (§6.5).
7. `ExportRail`: button "Export N files" / "Export" disabled at 0; row list =
   `in_files_list` members.
8. DebugDrive: `COUPLE_INCLUDE_TEST` — resolve the harness session, assert
   defaults, toggle one off, run auto-sync again and assert inclusion survives,
   export, assert `exportedAt` set and the count drops, save/restore and
   assert it round-trips; screenshot. Update `queueTest`/`queueNavTest`/the
   remerge case for the two-section model.

Acceptance: `cargo test --release --workspace` green incl. new proptests;
`scripts/golden-pair-rows.sh` unchanged (no resolver decision moved);
`swift build` both variants; `swift test`; `electron: npm run build:native &&
npm run parity && npm test` still green (binding only); harness case passes;
PARITY lines for this phase updated.

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
   scrub, never snap). New surface under the video in both arrangements
   (today the only seek surface is the overview's playhead; the transport has
   no scrubber). The overview stays as the editing surface (full only).
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
   Overlay and a remembered Pan maps to Synced (D10; `monitor.rs` decides
   next-mode/memory as today — the 3-way picker is presentation); the full
   arrangement keeps the 4-way picker and the A/B solos; `a`/`b` keys work in
   both.
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
4. Window: add a `WindowAccessor` (NSViewRepresentable handing the hosting
   `NSWindow` to the model — the app has no NSWindow access today and
   `.frame(minWidth:640)` in CoupleApp.swift:38 is static). On toggle,
   `setFrame(_:display:animate:)` to the last compact frame (default 360 × 700;
   `minSize`/`maxSize` clamp width 340–420 while compact, height free, min
   560 — §6.2) or the last full frame (min 640 × 400 as today, default
   1280 × 820). The window is `.hiddenTitleBar`, so the "⤢ Full" /
   "⤡ Compact" button sits at the right of the session line; menu item + ⌘⇧C.
5. Keys in compact: j/k, space, Return (switch to full with the selected
   row's editor open), esc, a/b (monitor flips as today, D10); nudges, [ ],
   l are no-ops. Media pool: "+ Add"
   only (§6.4) — the pool list and its player are full-arrangement surfaces.
6. Audition and player instances are owned by controllers, not views, so a
   layout switch never restarts playback.
7. DebugDrive: every existing `COUPLE_*_TEST` forces the full arrangement
   before driving (they assume loupes/overview rects); new
   `COUPLE_COMPACT_TEST` (shots of empty / synced / pick-one / exporting in
   compact) and `COUPLE_LAYOUT_TOGGLE_TEST` (toggle twice, assert frames and
   that selection, playhead and inclusion survive). The dev-harness bundle id
   differs from /Applications/Couple.app, so the compact default never leaks
   into the installed app's defaults — verify, don't assume.

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

## 4b. Ordering

0 → 1 → (2 ∥ 3) → 4 → 5 → 6. Inside Phase 1 the core policy + envelope +
header + node binding can run beside the Swift model work once the wire key
names are settled (D12). Phases 2 and 3 touch disjoint files
(QueueRail/ExportRail/Theme vs PairStrip/CenterColumn/AuditionController) but
share one `.build`, so build serially. Phase 4.1 (view extraction, no visible
change) lands alone before 4.2+. Phase 5 strictly after 4.

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
