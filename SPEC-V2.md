# Couple v2 — Session Queue & Queue-Centric Redesign (Technical Specification)

Status: planning complete 2026-06-12. Supersedes SPEC.md §12 (which sketched this
feature) and, for v2 scope, the v1 "single-pair editor is the product" framing.
Authored from the 2026-06-12 owner planning conversation; every owner decision in
this document was made explicitly there.

## 0. How to read this document

**Precedence.** SPEC.md (v1) remains the authority for everything it specifies:
engine contracts, sync algorithms, formats, export behavior, the editor's
interaction semantics. This document specifies only what v2 adds or changes. Where
the two conflict, this document wins for v2 scope — each such conflict is flagged
in §10 and recorded as a SPEC-CHANGE in PROGRESS.md.

**UX authority.** UX-DECISIONS.md §2.6 sketched the v2 queue before v1 was built;
parts of it are superseded here (notably timestamp-primary pairing, already
inverted by SPEC §1.4/F3, and the assumption that pairing is 1:1). For v2 UX,
**this document carries authority until the Session G mockup converges**; Session
G's deliverable includes a UX-DECISIONS-V2 section written from the converged
mockup, which then takes over for behavior (same precedence model as v1).

**Protocol.** The SPEC-CHANGE protocol (SPEC §0), conformance language, and
constant tags ([DECIDED]/[VALIDATED]/[SPEC]/[TUNE]) carry over unchanged.

---

## 1. What changed and why (owner decision record, 2026-06-12)

v1 shipped a verification-first single-pair editor on the theory that careful
manual verification was the product. Reality (the owner's, after living with the
working app): **auto-sync is trustworthy enough that the waveform stage is a
sanity check, not a workbench.** The workflow that matters is many-takes triage:

> Dump a full recording session (many audio + video files) into the app; it finds
> the corresponding pairs automatically; the user walks through them, marking each
> for the export queue and adjusting only where needed; one export action writes
> everything queued.

This is the queue-centric mockup philosophy winning the rematch now that auto-sync
has proven itself on real data — v1's editor-centric choice was correct *for v1*
and is not regretted; it becomes the drill-down view inside the queue.

**Attention-weight targets (owner, normative for Session G).** These describe
visual weight and signalled importance, NOT screen-area percentages:

| Element | v1 actual | v2 target |
|---|---|---|
| File input / auto-paired results (the queue) | ~3% | **~25%** |
| Video preview of the selected pair | ~40% | **~30%** |
| Auto-sync button + adjust tools | ~8% | **~5%** |
| Export controls | ~4% | **~20%** |
| Waveform stage (overview + loupes) | ~45% | **~20%** |

Specific owner directions for the redesign (binding on the Session G mockup as
starting positions; the mockup is where they get ratified or revised):

- The waveform **overview** narrows further; the auto-sync button and sync
  controls share its row.
- Offset/drift numeric readouts: owner is on the fence about showing them at all.
  Spec recommendation (to be tested in the mockup): **demote to an on-demand
  disclosure, never delete** — they are the manual rescue path that the honesty
  architecture requires when a row comes back ambiguous/failed; a failure-mode
  surface, not a default surface.
- The **drift-fit sparkline stays but smaller** — a glanceable trust indicator
  next to the fit-status text.
- Loupes become an **on-demand verification mode** rather than permanent
  residents (consistent with "sanity check").
- Video is the hero of the selected pair; the queue and export controls are the
  hero of the app.

---

## 2. The v2 workflow (normative)

1. **Drop anything.** The load surface accepts any number of files and/or
   folders. Everything is probed and classified (has video stream ⇒ video role;
   audio-only ⇒ audio role); non-media and unreadable files are skipped silently,
   surfaced once as "skipped N unrecognized files". Files import into a
   session-level **media pool** (§5.1) — tracks exist before pairs do.
2. **Auto-pairing.** The all-pairs candidate matrix is computed (§3), resolved
   into queue rows (one row per video), and full auto-sync runs per resolved row
   in a background queue to power the status chips. Anchor proposal and loupe
   framing remain lazy (first time a row is opened), preserving the Session-D
   rule that sync never disturbs a view the user has seen.
3. **Triage.** The user walks the rows: high-confidence rows can be verified from
   the chip + a quick audition without opening the editor; rows flagged partial /
   ambiguous / failed get opened (the v1 editor, intact) and adjusted or manually
   re-paired. `j`/`k` move rows, `v` verifies and advances to the next
   unverified, `Enter` opens/closes the editor for the selected row (the keys
   reserved since Phase 5).
4. **Verified ≡ queued (owner decision).** One flag. The v1 Verified toggle *is*
   queue membership; there is no separate "add to export queue" checkbox. The
   export action reads "Export n verified".
5. **Batch export.** One destination-folder choice, automatic naming (§6), serial
   export jobs over the existing pipeline, per-row status, continue-past-failure.

---

## 3. Pairing model

### 3.1 The video is the unit

Every export is one video with replaced audio, so **a queue row = one video plus
its selected audio source plus an alternates list**. Audio files are resources
that rows point at; an audio file appearing in many rows (continuous recorder) is
the expected common case, not an edge case. Pairing is therefore NOT a bipartite
matching — nothing "consumes" an audio file.

### 3.2 Candidate matrix

For every (video, audio) pair in the media pool, run the stage-1 envelope search
(SPEC §5.2/§5.3, existing machinery) and record `{z, lag_ms}`. Keep candidates
with `z ≥ Z_MIN` (8.5, the Session-C calibrated floor). From the lag and the two
known durations, derive each candidate's **coverage interval** — the span of the
video timeline the audio overlaps (same arithmetic as the §5.6 overlap fraction).
Cost: milliseconds per cell; all-pairs for a session of a dozen files is trivial
(measured stage-1 on a real 2 h × 2 h pair: 0.34 s).

**Timestamps are tiebreak/ordering hints only**, never primary — SPEC §1.4/F3 is
binding (the real recorder's clock is off by 11.6 years). Same-device
monotonicity MAY order otherwise-tied candidates; it MUST NOT create or veto a
pairing.

### 3.3 Resolution (per video)

- **Zero candidates** → row state `no match` (muted/absent/unusable scratch
  audio). Manual pairing is the only path; the row says so honestly.
- **One candidate, coverage ≥ COVERAGE_FULL** → auto-pair, confident.
- **One candidate, coverage < COVERAGE_FULL** → auto-pair with state `partial`;
  the chip states the uncovered span ("audio covers 0:00–18:40 of 23:55").
  Suggest keep-camera-audio for these rows. Export already handles the silence
  honestly (SPEC §8.2).
- **Multiple candidates, disjoint coverage intervals** → a video straddling a
  recorder restart. Select max coverage, state `split across files`, list the
  rest as alternates. **Stitching two audio files into one export is explicitly
  out of v2 scope** (it would break the two-track Session model and require seam
  crossfades); honest partial coverage, user decides.
- **Multiple candidates, overlapping coverage** → run the §3.4 simultaneity
  check. Simultaneous recorders ⇒ genuine alternates: default to best z, state
  `2 sources`, picker for the alternates. Not simultaneous ⇒ different takes,
  one match is false: select best z only if the §3.5 margin is decisive,
  otherwise state `ambiguous — check me` and float the row to the top.

### 3.4 The audio×audio simultaneity check (triangle closure)

When audio A and audio B both clear the floor against video V, correlate **A
against B** directly (envelopes already exist; another stage-1 cell). Two
recorders capturing the same event sync to each other; two takes of the same
piece do not (micro-timing diverges). Additionally the three lags must close a
triangle: `|lag(A→V) − lag(B→V) − lag(A→B)| ≤ TRIANGLE_TOL_MS`. A↔B z ≥ Z_MIN
**and** closure ⇒ simultaneous. This is three independent measurements that must
agree — the same never-confident-and-wrong posture as v1.

### 3.5 Margin rule

When candidates conflict (overlapping coverage, not simultaneous), the best z is
decisive only if `z_second < Z_MIN` or `z_best − z_second ≥ Z_MARGIN`. Otherwise
the row is `ambiguous`. **Honesty caveat, flagged (V2-F2):** Z_MIN was calibrated
against *unrelated* content (ceiling ≤ 7.5); cross-correlation between different
takes of the *same piece* — this feature's daily diet — is uncharacterized.
Z_MARGIN ships [TUNE] and MUST be re-calibrated when a real multi-take session
corpus exists; until then the resolver SHOULD prefer `ambiguous` over a coin-flip.

### 3.6 Stability and manual override

- **Re-drops/additions re-run pairing only for unresolved rows.** A row the user
  verified, edited, or manually paired is **pinned**: new files may add to its
  alternates list, never change its selection or chip.
- **Manual re-pair is a required affordance** (drag an audio from the unused
  tray / alternates picker onto a row, or equivalent — mockup decides the
  gesture). Manual pairing runs a full sync for the chosen pair; if z is below
  floor, the row keeps the pairing but the chip says `forced — below floor`.
- **Unused-audio tray**: audio files no row selects stay visible. Nothing the
  user dropped silently disappears.

---

## 4. Core additions (Rust) — additive only, ABI stays v1-compatible

- `cpl_pair_correlate(ctx, track_a, track_b) → {z, lag_ms}` — role-agnostic
  stage-1 between any two imported tracks. (Session F MUST verify the stage-1
  path is genuinely role-free apart from epoch handling, which import already
  bakes into the cache; record a SPEC-CHANGE if anything assumes camera/external
  roles.)
- `cpl_pair_candidates(ctx, videos[], audios[]) → matrix of {z, lag_ms}` —
  convenience batch over the above (MAY be implemented shell-side over
  `cpl_pair_correlate`; the dedicated call exists so the CLI and ports get it).
- Coverage intervals, resolution states, triangle closure, margins: **shell/CLI
  layer, not core** — core stays mechanical (measurements), resolution lives
  where the user can override it.
- CLI: `couple pair <dir> [--json]` — probe/import/classify everything in a
  directory, print the candidate matrix and the resolved rows with states. This
  is the headless test surface for the whole pairing layer and lands **before
  any UI work**.
- Cache GC: tracks referenced by the open session MUST be exempt from LRU
  eviction (`cpl_cache_gc` gains a pinned-keys parameter or core tracks
  liveness via open handles — Session F picks; flag V2-F3).

No changes to sync, export, audition, or any existing ABI symbol.

---

## 5. Shell architecture deltas

### 5.1 Media pool (the one real refactor)

v1's `PairSession.loadFile(url)` owns probe → classify → import → auto-sync:
files enter the world through a pair. v2 inverts this: a session-level **media
pool** owns probe/import and holds `TrackHandle`s keyed by content key;
`PairSession` gains an init path from two existing handles. The v1 URL path
remains for single-pair drops (dropping exactly one video + one audio SHOULD
behave exactly like v1: one row, auto-paired, opened).

### 5.2 Queue model

`AppModel.pair: PairSession` becomes a row list plus a selection. Each row owns
its `PairSession` (which already encapsulates alignment shadow, view state,
export options, sync jobs — the "deliberately list-ready" design pays off here).
Per-row sync runs in a serialized background queue, chips update progressively.

### 5.3 Audition lifecycle (the second real refactor)

One `AVAudioEngine`, one output, one fullscreen window — but N rows. Row
switching follows a deliberate detach/attach protocol: stop the stream, close
the core audition handle, swap the AVPlayer item, reattach. Exactly one audition
handle open at a time. (Phase-6 history says the audio graph is where surprises
live; budget accordingly.)

### 5.4 Persistence

- Per-pair alignment autosave (`SessionStore`, keyed `cameraKey_externalKey`)
  is **unchanged** — each row keeps its own file.
- New session-level file (versioned, `couple-queue/1`): the media-pool file
  list, pairing resolutions (selected source + pinned flag per row), verified
  flags, export destination. Saved on the v1 triggers (verify/export/quit) plus
  pairing changes. Reopening the app with the same content keys offers restore
  of the whole queue.

---

## 6. Batch export

- **One folder, per-file naming.** Each couple's output filename defaults to
  `<video-stem>_synced` and is **edited in place** in the export list (click the
  name). The global editable naming *template* was tried and removed (owner
  2026-06-13, Phase 9): it was brittle and unrecoverable once mangled — per-file
  names are simpler and safer. The extension comes from the **single global**
  format selector (§8.4 codec matrix; Auto = camera container) — there is **no
  per-couple format override** (owner 2026-06-13). Collisions append `_2`, `_3`,
  …; an existing file from a previous run prompts once per batch (replace all /
  skip existing).
- **Disk preflight sums the whole queue** before the first job starts.
- **Serial execution** over the existing export job (stream-copy is ~16 s per
  24-min take; parallelism is not worth the I/O contention or the progress-UI
  complexity in v2).
- **Continue past failure**: a failed row records its error on the chip, the
  batch proceeds, and the summary toast reports "n exported, m failed" with the
  first failure's message. No aborted-batch ambiguity.
- Per-row progress on the row; batch progress on the export button.

---

## 7. Test strategy (Session F gates)

Extend the §9.1 fixture generator with a **session-folder mode**: one master
timeline; camera files cut from regions of it; external file(s) covering chosen
spans; per-file independent degradation. Truth table = which rows, which states,
which coverage.

Gates (all headless, via `couple pair`):

1. **Continuous recorder**: 3 videos cut from one long master + 1 external
   covering all ⇒ 3 rows, same audio, full coverage, correct per-row lags
   (each within the §9.3 stage-1 tolerance).
2. **Recorder restart**: 2 externals splitting the master mid-video-2 ⇒ video 1
   and 3 full-coverage; video 2 `split across files` with max-coverage selected
   and the other listed as alternate.
3. **Simultaneous recorders**: 2 externals = same master span, independent
   degradation/SNR ⇒ `2 sources`, triangle closure within TRIANGLE_TOL_MS,
   best-z default.
4. **Unrelated audio** (different master) in the pool ⇒ never paired (z below
   floor), lands in the unused tray; a video with only unrelated audio ⇒
   `no match`.
5. **Mixed bag**: a non-media file and a video with its audio stream stripped ⇒
   skipped-file report and `no match` row respectively; no crash, no litter.
6. **Stability**: re-running `couple pair` over the same dir with one new file
   added changes only what the new file can change (assert resolved rows for
   pinned/unchanged inputs are byte-identical in `--json`).
7. **Budget**: 12-file synthetic session (6 video × 6 audio) — candidate matrix
   ≤ 5 s on top of imports [SPEC]; imports themselves already budgeted (§9.6).
8. **Honesty floor (best-effort, V2-F2)**: same-master different-degradation
   pairs must NOT be separable as "different takes" by the simultaneity check
   (they ARE simultaneous — sanity direction); a genuinely different master with
   matched tempo/structure SHOULD land `ambiguous` rather than confidently
   wrong. Real same-piece-different-take calibration deferred to the owner
   corpus (this is the recorded limit of synthetic coverage).

Shell-phase gates (Session H) reuse the v1 keystones: the per-row export remains
byte-identical to the CLI export for the same state (the Phase-6 hash discipline,
now asserted on a batch), and the §2.9 event tests extend to j/k/v/Enter.

---

## 8. Phases & sessions

- **Phase 7 — pairing core, headless (Session F).** §4 core additions, fixture
  session-folder mode, resolution layer (in the CLI for now — it is shell logic,
  but `couple pair` needs it and it must be UI-blind), gates §7. No Swift.
- **Phase 8 — queue-centric mockup (Session G).** HTML mockup(s) in the lineage
  of `design/mockups/`, implementing §1's attention weights and §2's workflow
  with the real pair's data baked in (the original mockups' method). Iterate
  with the owner; deliverable includes the UX-DECISIONS-V2 behavior section
  drafted from the converged mockup. **No production code.** May run in
  parallel with Session F (different surfaces; coordinate commits).
- **Phase 9 — shell (Session H, prompt written after G converges).** Media-pool
  refactor, queue UI per the mockup, audition lifecycle, batch export UI,
  persistence. The v1 editor survives as the drill-down.

Phase numbering continues v1's (Phases 1–6 complete).

---

## 9. New constants (Appendix A conventions)

| Constant | Value | Tag | Rationale |
|---|---|---|---|
| Z_MIN (pairing floor) | 8.5 | [VALIDATED] | Session-C calibration, reused as-is |
| Z_MARGIN (decisive-best margin) | 6.0 | [TUNE] | Placeholder; unrelated-content ceiling 7.5 vs real-pair 75 suggests huge separation for unrelated content; same-piece-takes uncharacterized (V2-F2) |
| TRIANGLE_TOL_MS | 250 | [SPEC, TUNE] | Stage-1 coarse bin is 200 ms at 9600 spb; refinement is finer, slack allowed |
| COVERAGE_FULL | 0.98 | [SPEC, TUNE] | Real pair measured 0.996; below this the chip must disclose the gap |
| PAIR_MATRIX_BUDGET | 5 s / 36 cells | [SPEC] | §7 gate 7 |

---

## 10. Flags (per SPEC §0 process; none blocks implementation)

| # | Flag | Resolution adopted |
|---|---|---|
| V2-F1 | Pairing is not 1:1 — the real session shape is a continuous recorder spanning takes (the v1 ground-truth pair itself has external ≫ video), and §12.2's best-z matching assumed 1:1. | Video-as-unit model (§3.1); audio files are shared resources; SPEC §12.2 superseded. |
| V2-F2 | Z floor/margin calibrated only on unrelated content; same-piece-different-take false matches are the realistic hazard and synthetic fixtures cannot faithfully produce them. | Triangle check (§3.4) + conservative margin (§3.5) + `ambiguous` bias; [TUNE] constants re-calibrated on the owner's first real multi-take corpus; gate §7.8 records the limit. |
| V2-F3 | LRU cache GC can evict a track the open queue still references (v1 never held >2 tracks). | Pinning required (§4); mechanism chosen in Session F with a SPEC-CHANGE note. |
| V2-F4 | Verified-vs-queued as two flags invites "verified but not queued" zombie states. | Owner decision: one flag (§2.4). |
| V2-F5 | Audio stitching for recorder-restart straddles would break the two-track Session model. | Out of scope; `split across files` honesty instead (§3.3). |
| V2-F6 | The v1 editor's "Auto-sync hero button" contradicts the 5% attention target — it advertises a step the user shouldn't think about. | Mockup direction: sync controls share the narrowed overview row; auto-sync is ambient (runs on pairing), the button is the re-run affordance. |

> **V2-F2 closure (2026-07-11):** the [TUNE] re-calibration on the owner's first real
> multi-take corpus happened — see the SPEC-CHANGE (2026-07-11) entries in PROGRESS.md
> (COVERAGE_FULL 0.98 → 0.95; Z_MARGIN/TRIANGLE_TOL_MS held; zero true different-take
> conflicts observed, regime gap recorded). §9's table values are superseded by that ledger.

---

## 11. Open owner decisions — SETTLED at Session G convergence (2026-06-12)

All six were resolved in/after the converged Session G mockup. Full behavior is
in UX-DECISIONS.md Part V2; dispositions below. (DECIDED by owner unless noted.)

1. **Offset/drift readouts → on-demand disclosure (option A).** The numeric
   readouts + nudge buttons appear when the persistent bottom bar's "sync tools"
   is expanded; one level, not a nested sub-disclosure. Never deleted — they are
   the manual-rescue surface. Option B ("gone entirely") rejected.
   (UX-DECISIONS-V2 §V2.4.)
2. **Loupes → on-demand, summoned by the "loupes" toggle / `l` / clicking the
   fit-status line / clicking an anchor pin** (which focuses that anchor's
   loupe). Default view = video + overview + drift-fit sparkline, no loupes.
   (§V2.4.)
3. **Chip vocabulary → the verdict model**: a single status dot good/questionable/
   none (green/red/grey) + an action word only when needed. **The triage bar is
   replaced by three live workflow sections** (QUEUED FOR EXPORT / TO APPROVE /
   TO FIX), not a sort control. (§V2.2–V2.3.)
4. **`v` does NOT require a prior audition.** The verdict is trustworthy on its
   own; uncertain couples already float to TO FIX. (§V2.5.)
5. **Per-file naming, edited in place** (default `<video-stem>_synced`; click the
   name in the export list to rename, ↺ to reset). The global editable *template*
   originally settled here was **removed in Phase 9** (owner 2026-06-13): brittle
   and unrecoverable once mistyped. The extension is owned by the single global
   format selector. (§V2.8.)
6. **Manual re-pair gesture → drag an audio file from the media pool (or pick
   from the candidate list) onto a couple.** Below-floor pairings are kept and
   marked "forced". (§V2.6.)

Additional decisions taken at convergence (beyond the six listed): "couple" as
the user-facing noun; "Approve" as the verb for verify≡queue; the persistent
bottom tool bar; drag-resizable side rails; the media pool generalizing the
unused tray with usage indicators + audio preview; the v1 format selector
(Auto/MOV/MP4/WAV + keep-camera-audio) carried over as a **single global**
control (the per-couple override was removed in Phase 9, owner 2026-06-13);
export size/resolution targeting deferred to its own post-Session-H phase. The
verdict good/questionable thresholds ship [TUNE] pending owner-corpus
calibration (real true-pair fit RMS ~5 ms breaks v1's thresholds — V2-F2, now
with real numbers).

---

## 12. Kickoff prompts

### Session F (pairing core, Phase 7)

```
You are implementing Phase 7 of Couple — the headless pairing core for the v2
session queue. Phases 1–6 (v1) are complete and shipping; nothing you do may
change v1 behavior, the existing ABI symbols, or the frozen real-pair
regression values.

Read first, in this order, before writing any code:
  1. SPEC-V2.md — all of it. Binding for this session: §3 (pairing model),
     §4 (core additions), §7 (gates), §9 (constants), §10 (flags V2-F1..F3).
  2. SPEC.md §5.2–5.3 (the stage-1 machinery you are exposing), §9.1 (fixture
     generator you are extending), §1.4/F3 (timestamps are hints, never
     primary), §0 (SPEC-CHANGE protocol).
  3. PROGRESS.md — the 2026-06-12 v2 planning entry and the Session C
     SPEC-CHANGEs (Z_MIN 8.5, fixture transient changes).

Scope, strictly:
  - Core: cpl_pair_correlate (role-agnostic stage-1 between any two imported
    tracks) and cpl_pair_candidates (batch). Verify stage-1 is genuinely
    role-free; SPEC-CHANGE if not. Cache-GC pinning for session-referenced
    tracks (V2-F3 — pick the mechanism, record it).
  - Fixture generator: session-folder mode (§7) — master timeline, region-cut
    camera files, span-covering external(s), per-file degradation, truth table.
  - CLI: couple pair <dir> [--json] — probe/import/classify a directory,
    candidate matrix, and the FULL §3.3 resolution (coverage intervals,
    disjoint/overlapping discrimination, §3.4 triangle closure, §3.5 margin,
    states: confident/partial/split/2-sources/ambiguous/no-match, unused-audio
    list, §3.6 stability semantics for re-runs).
Out of scope — do not start: anything Swift/shell, the mockup, batch export,
persistence formats. couple.h grows only the two new symbols.

Hard rules:
  - Timestamps as ordering hints only (SPEC §1.4) — never create/veto a pairing.
  - Resolution prefers `ambiguous` over a coin-flip (V2-F2); never
    confident-and-wrong is the project posture.
  - Constants from SPEC-V2 §9, overridable; all [TUNE] values surfaced in
    --json output so the future corpus calibration has the raw numbers.
  - Work the gates back-to-back (§7 gates 1–8); SPEC-CHANGE protocol for any
    conflict; commit + dated PROGRESS.md entry when green.

Finish line: `couple pair` on a generated session folder reproduces the §7
truth tables, and on example_data/ (1 video + 1 audio) yields exactly one
confident row whose lag matches the frozen offset within stage-1 tolerance.
Report the resolved-rows JSON for both in your final summary.
```

### Session G (queue-centric mockup, Phase 8)

```
You are designing the v2 queue-centric UI of Couple as interactive HTML
mockup(s) — the same method that designed v1 (see design/mockups/README.md and
the provenance note at the top of UX-DECISIONS.md). No production code; no
Swift; nothing in mac/ changes.

Read first: SPEC-V2.md §1 (attention-weight targets — your acceptance
criteria), §2 (workflow), §3.3 (row states your chips must express), §6
(batch export controls), §11 (open decisions you must present options for);
UX-DECISIONS.md §1–2 (the v1 editor you are demoting-but-keeping);
design/mockups/ (visual lineage); PROGRESS.md 2026-06-12 entries.

Deliverable: design/mockups-v2/ containing mockup(s) that:
  - Realize the §1 attention weights — queue list ~25%, video hero ~30%,
    export ~20%, waveforms ~20% (narrowed overview sharing a row with the
    sync controls per V2-F6), sync tools ~5%. Visual WEIGHT, not area.
  - Use the real pair's peak data (design/mockups/ shows how) plus plausible
    fake rows for a 5-take session: every §3.3 chip state appears somewhere.
  - Show the triage flow (j/k/v/Enter), a row opened into the editor
    drill-down, the unused-audio tray, manual re-pair, batch export with
    per-row status, and BOTH options for SPEC-V2 §11.1 (readouts as
    disclosure vs gone) as toggleable variants the owner can compare live.
  - Loupes appear only in their on-demand mode; propose the summoning gesture.

Iterate with the owner — this session is conversational, not autonomous; small
rounds, screenshots in design/mockups-v2/screenshots/. When the owner declares
convergence: write the UX-DECISIONS-V2 behavior section (append to
UX-DECISIONS.md as a clearly-marked v2 part, same DECIDED/owner-decision
format), update SPEC-V2 §11 dispositions, and add a dated PROGRESS.md entry.
The Session H (shell) kickoff prompt gets written then, not before.
```

### Session H (queue-centric shell, Phase 9) — written at Session G convergence

```
You are implementing Phase 9 of Couple — the v2 queue-centric SwiftUI shell.
Phases 1–8 are complete: v1 ships, the Phase-7 pairing core is in (couple pair/
correlate/candidates), and the Session-G mockup has CONVERGED. Nothing you do
may change v1 engine behavior, the existing ABI symbols, the frozen real-pair
regression values, or the Phase-7 pairing results.

Read first, in this order:
  1. UX-DECISIONS.md PART V2 — the binding behavior spec for this work. It is
     the authority; where it is silent on a v1-unchanged behavior, Part v1
     governs (it says which). Read Part v1 §2 (editor interaction), §2.7
     (audition), §2.8 (export) — the drill-down editor is v1, intact.
  2. SPEC-V2.md §4 (the core you call), §5 (shell architecture deltas: media
     pool, queue model, audition lifecycle, persistence), §6 (batch export),
     §7 (the per-row export-hash + j/k/v/Enter event gates), §10 (flags),
     §11 (settled dispositions).
  3. design/mockups-v2/ — the converged reference design. README.md lists the
     settled intent and the deliberately-deferred cosmetic items. index.html is
     the interaction reference (run it: python3 -m http.server, then open it);
     it is structural/behavioral reference, NOT a visual contract (same rule as
     design/mockups/ for v1 — you have creative license on pixels, none on
     behavior; a behavior departure is a SPEC-CHANGE).
  4. PROGRESS.md — the Session D/E owner SPEC-CHANGEs (no anchor snap, content-
     relative arrows, sync-never-disturbs-a-seen-view, uniform panels, raw-
     sample loupes, ⌥ coarse drag) and the 2026-06-12 Session G entry. Project
     memory carries the owner's standing UX prefs — honor them.

Scope, strictly (SPEC-V2 §5, §8 Phase 9):
  - Media-pool refactor (§5.1): a session-level pool owns probe/import and holds
    TrackHandles by content key; PairSession gains a two-handles init path. The
    v1 single-drop path still behaves exactly like v1 (one couple, auto-paired,
    opened). This is the one real refactor — budget for it.
  - Queue model (§5.2): AppModel becomes a couple list + selection; each couple
    owns its PairSession (alignment shadow, view state, export options, sync
    jobs). Per-couple sync runs in a serialized background queue; verdict dots
    update progressively. Resolution (coverage, triangle, margin, states) is the
    Phase-7 CLI logic, lifted into the shell so the user can override it.
  - Audition lifecycle (§5.3): one AVAudioEngine / one output / one fullscreen
    window across N couples; the detach/attach/swap protocol with exactly one
    audition handle open at a time. (Phase-6 history: the audio graph is where
    surprises live.)
  - The shell per UX-DECISIONS-V2: three-column layout, workflow sections,
    verdict dots, persistent bottom tool bar (§11.1=A on-demand readouts;
    loupes on-demand per §11.2), Approve≡queue (clickable 3 ways; no audition
    gate per §11.4), candidates-never-destroyed + pinning, media pool with
    usage indicators + audio preview, drag-resizable rails, the v1 editor as
    the drill-down. "Couple"/"Approve" vocabulary.
  - Batch export UI (§6): destination, editable naming template + per-couple
    stem override, format selector (Auto/MOV/MP4/WAV + keep-camera-audio) with
    per-couple override, disk preflight, serial jobs, per-row status,
    continue-past-failure.
  - Persistence (§5.4): the new versioned session file (couple-queue/1) beside
    the unchanged per-pair autosave; restore the whole queue on reopen.

Out of scope — do not start: export size/resolution targeting (its own later
phase, SPEC-CHANGE territory); any change to sync/export/audition engine math;
new ABI symbols (the shell uses cpl_pair_* and the existing v1/Phase-6 surface).
Cosmetic polish from the mockup is app-side and owner-reviewed live, not a gate.

Hard rules:
  - UX-DECISIONS-V2 is binding behavior; v1 Part governs anything it doesn't
    change. A behavior departure is a SPEC-CHANGE in PROGRESS.md.
  - Aspect ratio inviolable: the video letterboxes, never distorts.
  - Sync never re-proposes anchors or re-frames a view the user has seen
    (Session-D rule); auto-sync is ambient, "re-sync" is the manual re-run.
  - Never confident-and-wrong: ambiguous/no-match couples carry no selection;
    below-floor manual pairings are kept but marked "forced".
  - Verdict good/questionable thresholds are [TUNE] — surface the raw fit/z so
    the owner can calibrate on real sessions (v1's RMS thresholds DO NOT
    transfer; real true-pairs run ~5 ms — V2-F2/§V2.9).

Finish line (SPEC-V2 §7 shell-phase gates):
  - The per-couple export is BYTE-IDENTICAL to the Phase-4 CLI export for the
    same state — the Phase-6 hash keystone, now asserted across a batch.
  - The §2.9 event tests extend to j/k/v/Enter over the queue.
  - couple pair on the owner's multifile-session and the canonical real pair
    reproduce in the shell what the CLI/mockup show (same couples, states,
    selections, unused pool). Report the batch-export hashes and the resolved
    queue in your final summary.
```
