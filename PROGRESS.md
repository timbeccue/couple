# PROGRESS.md — Couple

Append-only log. Each session adds a dated entry. Spec deviations are recorded as
`SPEC-CHANGE:` lines with rationale (see SPEC.md §0 for the protocol).

## 2026-06-11 — Planning

Status: planning complete. SPEC.md written. No implementation yet.

- Owner attention requested on SPEC.md §2 flags (notably F1/F2 epoch & ground-truth
  freeze, F3 v2 pairing inversion) and the §14 owner-decision items.
- Next: Session C implements Phases 1–3 per the kickoff prompt in SPEC.md §16.
- SPEC-CHANGE (2026-06-11, owner): Session C scope widened from "Phase 1 only" to
  Phases 1–3 — test-driven against synthetic fixtures with known offset/drift,
  ending with §9.4 real-data validation and the offset freeze. §15 phase
  definitions unchanged; SPEC §16 Session C prompt rewritten accordingly. Phase 4
  (export) becomes a short follow-up session before Session D (whose prompt
  assumes Phases 1–4 complete). Also: ffmpeg pin updated 7.1.x → 8.0.x to match
  the dev machine (8.0.1).

## 2026-06-11 — Session C, Phase 1 complete (decode, pyramid, inspection)

Workspace scaffolded (core/, cli/, ffi/), Rust 1.96, system ffmpeg 8.0.1 via
$PATH (COUPLE_FFMPEG honored). Implemented: probe (§7.1), import job with
progress/cancel and the §7.2 epoch rule (single streaming pass → PCM cache +
pyramid + stats; onsets from the 480 level), `pyramid.cpk` binary format (§6.1),
content-addressed cache + LRU GC (§6.3), band/raw_window/rms/onset queries
(§4.3), session state + setters (§4.5, ahead of Phase 3 need), CLI
`probe|import|peaks|onsets|cache` with `--json`/`--progress-json`/SIGINT-cancel
(§10).

§9.2 gates, all green:
- Real pair: camera duration 1435.274 s (window 1435.252 ± 0.05), epoch
  0.180792 s; external 1798.880 s, epoch 0. ✓
- Engine 48000-spb envelope vs mockup `peaks/*.json`: external r = 1.00000;
  camera r = 0.99998 after epoch shift (§1.3). ✓
- Pyramid property tests (band == brute force, all levels, 200 random ranges,
  build→write→mmap round trip). ✓
- Epoch fixture (`-itsoffset 0.18 -c copy` MOV): PCM cache leads with exactly
  8640 zeros; canonical duration includes the lead. ✓
- Cancellation: no cache litter, `Cancelled` surfaced. ✓
- `couple peaks --png` eyeballed against `peaks/*_overview.png`: macro
  structure matches both tracks. ✓
- Import speed: real camera mp4 (3.1 GB, 23.9 min) ≈ 1.8 s, WAV ≈ 0.8 s —
  far inside the §9.6 budget; formal 2 h assertion lands with the Phase-2
  fixture generator.

Notes for the owner (non-blocking):
- Engine camera normalization is 1.0346 vs mockup 1.4631 (= ×√2 exactly):
  mockup downmix used a different stereo→mono gain. Harmless — NCC and the
  r ≥ 0.99 check are scale-invariant; flagging for provenance only.
- decoded camera duration is 1435.274 s, 22 ms over the §7.3 nominal
  (1435.252): AAC priming/trailing-frame variance, inside the ±50 ms window.

## 2026-06-11 — Session C, Phase 2 complete (fixtures + global offset search)

Implemented: fixture generator (§9.1; deterministic ChaCha8 master on a 96 kHz
grid → external WAV + warped/degraded camera with AGC, 16 kHz LP, SNR noise,
optional AAC round-trip / mpeg4 testsrc2 mux / `-itsoffset` epoch remux);
envelope NCC machinery (§5.2: prefix sums + FFT linear correlation, z-score
selection); stage pipeline §5.3 with parabolic refinement; `couple sync
--offset-only`; `couple fixture gen`.

Gates, all green (11 tests in core/tests/sync_offset.rs + budgets.rs):
- §9.3 offset rows (ppm 0): errors 0.03–0.23 ms across offsets/seeds (≤ 0.5). ✓
- Gain 24 dB + AAC round-trip: ≤ 0.5 ms. Rate mix 44.1↔48, 96↔48: ≤ 0.5 ms. ✓
- Partial overlap: 25 s → 0.002 ms err; 12 s → Failed; duration-impossible →
  InsufficientOverlap. ✓
- Hostile noise SNR 0 (3 seeds): synced AND correct (z ≈ 22) — never
  confident-and-wrong. ✓
- Spurious-peak regression: decoy (verbatim camera head appended to external
  tail) wins raw-NCC argmax at the boundary lag; z-selection picks truth. ✓
- FFT path ≡ brute force: identical winner, z within 1e−6 (3 shapes). ✓
- §9.6 budgets on a real 2 h × 2 h pair: import 2.2 s / 2.7 s (≤ 90 s);
  stage 1 0.004 s (≤ 3 s); offset pipeline 0.34 s; band() median 8 µs (≤ 50 µs). ✓

SPEC-CHANGE (2026-06-11, Session C): Z_MIN default 20 → 8.5 (Appendix A said
20 [TUNE]). Reason: z = ncc·√bins cannot exceed √100 = 10 at the §9.3
partial-overlap floor (20 s = 100 bins), so 20 made that row mathematically
unsatisfiable. Empirical calibration: unrelated-content ceiling ≤ 7.5, weakest
must-pass true peak 9.9 (20 s overlap, SNR 10). Real-pair true z = 75 is
unaffected. Owner may re-tune as the corpus grows (§6.4).

SPEC-CHANGE (2026-06-11, Session C, minor): §9.1 fixture transients realized as
8-partial band-passed bursts (800–6000 Hz, random phases) with 0.5 ms attack —
a pure sine burst leaves the 24-spb envelope NCC profile flat over ±5 ms (no
sub-ms timing information; observed: noise shifted the argmax 1.7 ms), which
made the §9.3 tolerances untestable. Broadband attacks restore a sharp peak
(errors 0.08–0.15 ms). Decay τ 80–400 ms, ~0.5/s Poisson placement unchanged.

## 2026-06-11 — Session C, Phase 3 complete (drift, full auto-sync, real-data freeze)

Implemented: measure_local (§5.4: 96-spb coarse scan + 24-spb fine + parabolic,
variance/degenerate-peak gates), drift pass + least-squares fit + high-ppm refit
iteration (§5.5, F6 span scaling), status/suggestion heuristics (§5.6), session
integration (§4.4 write-on-success semantics), full `couple sync` +
`--write-state`, `couple measure|fit|state validate`, C ABI for the Phases-1–3
inventory + committed cbindgen `couple.h` (36 cpl_ symbols).

Robustness fixes found by the synthetic ladder (both principled, §5.4/§5.5
conformant): (1) drift-window centers clamped so the 18 s window stays inside
both tracks for *every scanned lag* — a window hugging the overlap edge had its
scan truncated mid-peak at the file edge and locked a confident wrong lag
(+5–7 ms at ncc 0.92); (2) measure_local treats winners adjacent to
out-of-track lags as degenerate edges → None, per "None rather than a junk
number".

SPEC-CHANGE (2026-06-11, Session C, minor): §9.1 transient rate ~0.5/s → 2/s.
At 0.5/s an 18 s window can be tone-only, and a sustained 200 Hz tone's
bin-span envelope is periodic at its 5 ms pitch period — windows locked a whole
period off with ncc > 0.9, blowing the §9.3 drift ladder (which assumes
0.15–0.3 ms window sigma, i.e. continuous performance content). The
periodic-content hazard (vibrato tones) remains present for anchor realism.

§9.3 gates (synthetic), all green:
- Drift ladder: 1800 s → err 0.006 ppm (±0.2); 600 s → ±0.5 ✓; 180 s → ±2 ✓;
  60 s → ±5 ✓; 100 ppm @300 s ✓. Midpoint mapping error ≤ 1 ms everywhere.
- Local lag (ncc ≥ 0.6 windows): RMS ≤ 0.3 ms ✓; corrected residual ≤ 2 ms at
  every accepted window ✓; short-overlap drift honesty ✓.
- measure_local: median ≪ 5 ms budget; accuracy RMS ≤ 0.3 ms ✓.
- Determinism: bit-identical SyncResult across runs ✓.
- 2 h budgets: full auto-sync 0.37 s (≤ 10 s); import 2.5/3.2 s (≤ 90 s);
  stage 1 0.004 s (≤ 3 s); band() 8 µs (≤ 50 µs) ✓.

§9.4 real-data validation (full pipeline, 137 ms compute), all green:
- drift **3.570 ppm** (window 3.07 ± 0.75 ✓); fit RMS **0.277 ms** (≤ 0.7;
  mockup: 0.276) ✓; **9/12** windows accepted (≥ 8) ✓; status **synced** ✓;
  suggestion **resample** (end-to-end 5.1 ms) ✓; overlap fraction **0.9961**
  (≥ 0.99; mockup: 99.6 %) ✓; coarse z **74.2** / ncc **0.877** (mockup: 75 /
  0.88) ✓; onset zone property 2/3 attacks-from-quiet ✓.
- **Offset frozen (F2): −5536.639 ms** container time, inside the §1.3 sanity
  window (−5535.5 ± 25; −1.14 ms from the prediction). Written to
  `example_data/expected.json` with `ownerVerified: false`; ±1 ms regression
  active and passing. **Owner: verify by ear/loupe once the shell exists, then
  flip ownerVerified to true.**

Headless workflow proven: `couple sync … --write-state s.json` →
`couple state validate s.json` → (render/export are Phase 4).

Session C complete. Next: Phase 4 (export) as a short follow-up session, then
Session D (SwiftUI shell, prompt in SPEC §16).

## 2026-06-11 — Session C2, Phase 4 complete (export pipeline)

Implemented: aligned-audio render (§8.2) from the ORIGINAL external at full
fidelity — None mode = integer-sample shift, bit-exact; Resample mode = own
128-tap Kaiser (β 14) windowed-sinc polyphase at constant ratio with the
fractional offset in the initial phase, measured **THD+N −151 to −154 dBFS**
(997 Hz / 4 kHz / 12 kHz) against the −110 bar — rubato/soxr not needed, no
new crates. Mux (§8.3/§8.4): video stream-copy, codec matrix (mov→PCM,
mp4→aac_at 256k), keepCameraAudio second track non-default, -map_metadata 0,
optional +faststart, temp + atomic rename, disk preflight, -progress-driven
ExportMux phase. WAV/RF64 (§8.5) at source rate/depth/channels. CLI
`render`/`export` (§10.2); `cpl_export_start` added to couple.h.

Implementation notes (caught by tests):
- §8.2 case 4 (external longer than video) deadlocked: the truncated render
  leaves the decoder blocked on a full pipe — it must be killed, not awaited.
- The decode reader returned more frames than requested (pipe chunking),
  underflowing the head-trim skip arithmetic → all-silence output. Capped.

Gates (§9.7 + §15 Phase 4), all green (7 export tests; 41 total in workspace):
- Round-trip keystone: re-sync of the exported render vs original external
  reproduces the mapping within 0.25 ms / 0.3 ppm; vs the camera the residual
  is ≈ 0 (0.008 ms / 0.008 ppm observed). ✓
- None-mode mux of a PCM-in-MOV fixture: byte-identical samples over the
  overlap, digital-silence head, mpeg4 video stream untouched. ✓
- Edge cases 1–5 asserted sample-exactly; case 6 (no mid-span synthesis)
  proven by the byte equality. ✓
- keepCameraAudio: 2 audio tracks, a:0 default, a:1 copied + non-default. ✓
- Container matrix: mp4-family input → mp4 + AAC; mov → PCM s24. ✓
- Cancellation: Cancelled surfaced, no temp litter. ✓
- Budget: 2 h resample-mode render 15.5 s (≤ 90 s). ✓

Real pair (the §15 finish line — iMovie workflow replaced headlessly):
`couple sync --write-state` → `couple export` → /tmp/schumann_synced.mp4 in
16 s: h264 stream copy bit-preserved (identical VFR avg_fps 969030000/32294159,
43068 frames), aac_at stereo default track, creation_time 2022-12-10 preserved,
duration 1435.307 s. In-sync verification: exported audio vs original camera
audio measures +0.50 ms (t=700) / −0.42 ms (t=200) residual — sub-ms, well
under the 40–80 ms perception threshold.
- Export SHA-256 (this machine, ffmpeg 8.0.1):
  d68336c1b8988cc726ff6590fb2626e2b7e7d43e050952451875d39b945e4417
  Caveat for the §15 Phase-6 hash comparison: aac_at is Apple's encoder; the
  hash is reproducible on this machine/ffmpeg pin but not necessarily across
  OS updates — re-derive with the same pin if it drifts.
- Deferred minor: §8.3 best-effort tmcd timecode-track copy not implemented
  (the real pair has none); revisit if a tmcd-bearing camera shows up.

Core complete (Phases 1–4). Next: Session D (SwiftUI shell, Phase 5 prompt in
SPEC §16).

## 2026-06-12 — Session D, Phase 5 complete (SwiftUI shell: stage + loupes)

Implemented `mac/` (Swift 6 toolchain, macOS 14+, SPM package — see SPEC-CHANGE
below):

- **CoupleKit** (§11.1): C-ABI wrapper over `couple.h`/libcouple.a — jobs as
  `AsyncStream<ProgressUpdate>` + awaited decoded result (C-callback trampoline,
  retain-balanced on the terminal call); hot `band`/`raw_window` with reusable
  caller buffers; onset queries; session hot setters 1:1, cold fields via the
  `set_state_json` round trip (the §4.9-conformant route); wire value types for
  MediaInfo/TrackMeta/SyncResult/AlignmentState. 3 integration tests green
  (import job with progress, band/raw/onsets, state round trip incl. the
  core-enforced None-mode semantics, auto-sync on an ffmpeg-generated fixture
  recovering a constructed +2500 ms offset, measure_local).
- **Couple.app** (Phase 5 scope): four-band window with the §1.2 resize policy
  (stage greedy, scroll below 1000×560); load surface (drop + pickers, probe
  classifies roles, same-kind replace confirmation, per-slot import progress +
  cancel); auto-sync on load with live progress ("Searching N%…" → "Measuring
  drift…"), Esc/button cancel, failure → mapping untouched + anchors proposed
  for the rescue flow; overview strip (union extent, external lane through the
  live mapping, overlap highlight, ticks, lock-zoom/+anchor chips, lane labels);
  draggable pins with ±4 s transient snap; bezier connectors (side-by-side
  only); 2–4 loupes (band rendering, raw-sample below 120 ms windows, stepped
  per-bin, additive screen blending, 86% per-loupe local normalization, anchor
  center line, Δ-colored headers with ⌖/⇥/×/auto, guidance arrows past 12% of
  window); offset drag, ⇧-drag pivot drift (§2.4 math), scroll zoom with the
  full lock/auto-frame semantics, coarse external-lane drag; readouts with
  editable fields, nudge buttons, human-readable offset subline, drift group
  greyed in None mode; fit status vs the *current* mapping; drift sparkline;
  450 ms / >30 ms re-measure debounce (immediate on typed commits); keyboard
  map §2.9 verbatim with j/k/Enter/space reserved no-ops. Export panel is the
  honest Phase-6 placeholder but hosts the functional **Verified** toggle and
  **Resample|None** control, since their §1.5 semantics drive the live mapping.

Verification (§15 Phase 5, real pair via `COUPLE_OPEN_*` debug drive; shots in
`design/screenshots/phase5/00–07*.png`):

- In-app auto-sync reproduces the frozen result exactly: offset −5536.639 ms,
  drift +3.570 ppm, fit RMS 0.277 ms · 9 windows, overlap 99.6%, status synced.
  Anchor auto-proposal picked attacks-from-quiet in all three zones; all three
  loupes show interleaved texture with sub-ms green Δs (+0.05/+0.24/−0.32 ms).
- Behavior shots: guidance arrows + red Δs + red fit RMS after a +60 ms nudge
  (auto-frame correctly NOT re-applied on nudge); drift-mode None honestly
  shows the uncorrected ramp (+1.09/+2.81/+4.31 ms at the three anchors) with
  drift group greyed and "5.1 ms uncorrected" summary; stacked layout (no
  connectors, DECIDED); deep-zoom raw rendering at ±30 ms; ⇥-align rescue from
  a +60 ms perturbation restores −5536.165 ms / 2.955 ppm (two-anchor align vs
  the 12-window fit — sub-ms/sub-ppm, as expected).
- Synthetic input events, 10/10: full §2.9 keyboard map (steps exact, modifiers,
  v, reserved no-ops), zoom routing to the loupe under the cursor (zooms all
  under lock, exits auto), auto-chip re-arm.
- **60 fps criterion**: per-frame draw cost on the real pair (release build) —
  3 loupes ≈ 0.08 ms each, overview 0.87 ms, sparkline 0.03 ms ⇒ ~1.1 ms per
  full pass, mean and max: **13× inside the 16.6 ms budget**, and every
  mutation produced exactly one coalesced redraw (no backlog). The on-screen
  rate could not be demonstrated >16 Hz in this session because the machine is
  in Low Power Mode with an idle adaptive-sync display (nominal 120 Hz,
  compositor pinned at ~16 Hz for a non-interacting session; verified via
  CADisplayLink + preferredFrameRateRange). **Owner: one interactive drag with
  LPM off (or `COUPLE_PROFILE=1`) ratifies this; folded into the Phase-6
  manual script alongside §6.10.**

SPEC-CHANGE (2026-06-12, Session D, minor/structural): `mac/` is a Swift
package (`Package.swift`, builds via `mac/Scripts/build.sh`, opens natively in
Xcode) rather than a checked-in `.xcodeproj` (§3.1 repo sketch). No behavioral
impact; Phase 6 packaging (signing, helpers, DMG) may add a generated project
if needed.

Deferred to the owner walkthrough (wiring is thin over tested model paths, but
untestable without a live input session): raw NSEvent/gesture feel of loupe
drag, pin drag, coarse drag, trackpad scroll; window-resize scroll fallback.
Also committed with this entry (owner working-tree edits found at session
start, kept verbatim): mockup screenshots + README under `design/mockups/`,
the UX-DECISIONS §1 screenshot-marker links, SPEC §16 Session-D prompt item 4,
and `expected.json` `ownerVerified` flipped to **true** — the F2 freeze is now
owner-confirmed and the ±1 ms regression is fully armed.

Phase 5 complete. Next: Phase 6 (audition + video preview + export UI + toasts
+ autosave + packaging).

## 2026-06-12 — Session D, owner feedback round 1 (live review of the shell)

Owner reviewed the running app ("responsiveness feels good") and directed the
following changes, all implemented, re-verified (10/10 synthetic input tests,
CoupleKit tests green), and captured in refreshed
`design/screenshots/phase5/00–09*.png`:

- SPEC-CHANGE (owner): anchor pins no longer snap to a transient on drag
  release (UX §2.2 snap-on-release reverted) — the pin stays exactly where
  placed, with a velocity-adaptive precision gain (slow pointer ⇒ finer
  movement). The ⌖ loupe-header button remains the explicit snap.
- SPEC-CHANGE (owner): arrow keys flipped vs UX §2.9 — ←/→ now move the
  *external content* in the arrow's direction (→ = offset −1 ms), matching
  what the eye sees. ⇧/⌥ step modifiers unchanged. Footer copy updated.
- SPEC-CHANGE (owner): auto-sync no longer re-proposes anchors or re-frames
  loupes when anchors already exist (UX §2.2 "after every auto-sync" narrowed
  to first sync / new pair) — re-running sync updates the mapping and
  re-measures but never disturbs the user's view.
- SPEC-CHANGE (owner): window layout inverted vs UX §1.2 resize policy — the
  workflow strip is now the greedy band; all four panels (Input · Video
  preview · Controls · Export) run uniform-height from the window top to the
  waveforms; extra height grows the video preview (16:9, width-capped so the
  other panels keep their minimums). The waveform stage is bounded: loupe row
  ≈ 2.4× the overview height (stacked: ~116 pt/row). Scroll fallback threshold
  raised 560 → 640 pt height accordingly. The title row is gone — wordmark
  moved into the Input panel; overlap chip and the layout toggle moved onto
  the overview strip.
- Controls panel redesigned: Auto-sync is the hero (full-width button + fit
  status under it); offset/drift readouts demoted to compact label·field·unit
  rows with clearer bordered nudge buttons; sparkline near-square (190×132)
  with a DRIFT FIT label, dropping below the readouts when narrow
  (ViewThatFits).
- SPEC-CHANGE (owner): export panel no longer restates offset/drift/RMS;
  shows output duration ("23:55 · video timeline") and a visible FORMAT
  choice (Auto/MOV/MP4/WAV + keep-camera-audio) — supersedes the §8.4/F9
  "no visible codec choice in v1". Wired to shell state now; the Phase-6
  export job consumes it.
- SPEC-CHANGE (owner): "never see blockiness" — loupes leave the pyramid for
  raw samples below 500 ms windows (was 120 ms; §11.2 threshold tag was
  [SPEC, tune visually]); within raw, ≤3 samples/px renders as a connected
  line (round joins), with sample dots only at extreme zoom. Stepped per-bin
  drawing remains for band-level (≥ 500 ms) windows where bins are sub-pixel.

Owner attention (non-blocking): the §9.6 budgets are unaffected (raw fetch at
500 ms = 24 k samples/loupe/frame, draw cost still ≪ 1 ms); UX-DECISIONS.md
§1.2/§2.2/§2.9 still carry the superseded wording — left as-is per the
append-only protocol, this entry is the authority.

## 2026-06-12 — Session D, Phase 6 complete (audition, video, export UI, autosave, packaging)

**Core (new `audition.rs` + 5 `cpl_audition_*` ABI functions, additive, ABI
stays v1):** §4.8 implemented in full — video-time cursor streams; camera from
the PCM cache; external through the mapping with offset always live and
sample-accurate, drift at the *committed* value only (new
`committed_drift_ppm` atomic on Session; commit_drift/apply_sync/
set_state_json/set_drift_mode all keep it coherent), driftMode None ⇒
integer-sample shift matching export (F8); 32-tap/1024-phase Kaiser polyphase
sinc (per-phase DC-normalized) for the resample path; 240-sample crossfade on
any mapping/monitor change between fills; loudness-matched Pan/Overlay/Null
matrices (−20 dBFS target, +24 dB clamp, gains from a ≤120 s window at the
overlap midpoint — full-cache scans were pointless); zeros + short fill only
past the video end. Tests (core/tests/audition.rs): null mode cancels
identical content to exact zero; pan channels bit-match g·source; +100 ms
integer shift reads u(t) exactly; 0-ppm sinc transparency; crossfade ramps
instead of clicking; end-of-video contract; **fill budget: 512-frame sinc-path
fill median ≪ 1 ms (§9.6)**.

**Shell:** CoupleKit audition wrapper (RT path = raw handles, shell render
block calls `cpl_audition_fill` directly); AVAudioEngine + AVAudioSourceNode
(deinterleaved 48 kHz graph with an RT-safe interleave→planar scratch);
muted AVPlayer slaved to the audio clock per §11.4 — output-latency-
compensated 1 Hz skew monitor, gain-scheduled rate nudge (±6% > 60 ms,
±1% > 15 ms, deadband below), reseek > 200 ms; space play/pause, per-anchor
jump buttons (−0.8 s, focuses the loupe), Pan/Overlay/Null picker with the
null-dip caveat, playheads in overview + focused loupe; video preview panel
hosts the real AVPlayerLayer. Export panel wired to `cpl_export_start`
(NSSavePanel, format/keep-camera-audio honored, progress + cancel, reveal-in-
Finder toast). Toast system (sync numbers per UX §2.5, low-confidence/failure,
export, restore offer). Autosave/restore per §14 6.8: session file (§4.5
wrapper) to Application Support keyed by content keys on verify/export/quit/
pair-replace; reopening the same pair offers Restore (cancels the running
auto-sync, applies the saved alignment wholesale). Packaging:
`mac/Scripts/make-app.sh [--dmg] [--sign id]` assembles Couple.app
(Info.plist com.timbeccue.couple, ffmpeg/ffprobe in Contents/Helpers — core
already resolves ../Helpers first), ad-hoc signed by default, DMG via hdiutil;
**notarization + the pinned LGPL ffmpeg build remain owner steps** (need
Developer ID credentials; dev bundle copies the system ffmpeg).

**§15 Phase-6 verification (all on the real pair, headless debug drive):**
- **Export hash keystone: the shell's export is byte-identical to the
  Phase-4 CLI export** — sha256 d68336c1…e417 matched on five consecutive
  runs (deterministic end to end: in-app auto-sync → export job).
- **A/V skew: sustained max |skew| 31.3 ms ≤ 33 ms (1 frame)** over a 16 s
  playback at t=60, measured against the latency-compensated audio clock,
  with mid-playback monitor-mode switches not stalling the stream; playhead
  advanced in real time. (Measured under Low Power Mode in a non-interactive
  session; a live session has more headroom.)
- Full §2.9 event tests still 10/10; all 45 Rust tests + 3 CoupleKit suites
  green; screenshots in `design/screenshots/phase6/` (toasts, monitor picker,
  transport visible; the AVPlayerLayer frame composites via IOSurface and
  doesn't appear in programmatic window captures — it renders on screen).

Debugging note for the record: the first audition bring-up wedged the app —
root cause was `engine.connect` raising kAudioUnitErr_FormatNotSupported
(−10868, mixers reject interleaved connections) as an ObjC exception that
vanished into the runloop. The fix moved the whole engine build+start off-main
behind a 5 s timeout (a wedged HAL can never hang the UI; transport shows
"audio output unavailable"), and the graph is now deinterleaved. App Nap also
proved capable of stretching debug-drive timers ~8×; the drive now pins a
latency-critical activity. Neither affects core.

Owner items for the live walkthrough: listen to the three monitor modes
(incl. the null dip) on the real pair; confirm A/V feel during nudging; the
§6.10 nudge-step ratification and the 60 fps drag check from Phase 5 still
stand. v1 feature work is complete pending that walkthrough — next session is
Phase 7 (v2 queue) per §12, plus the LGPL ffmpeg pin + notarization for
distribution.

## 2026-06-12 — Session E, shell UI fixes (owner review feedback)

- Overview strip gains a 16 pt badge gutter above the lanes (strip 96 → 112 pt):
  anchor pin badges sit in the gutter instead of sinking into the camera
  waveform, and pin stems now run through the tick row to the strip's bottom
  edge, so they meet the loupe connector curves (the tick row had left a 14 pt
  gap). Lanes, ticks, playhead, labels, and the coarse-drag surface all shift
  down by the gutter; lane geometry is otherwise unchanged.
- SPEC-CHANGE (owner): workflow strip absorbs extra *width* as well as height —
  the 1340 pt strip cap and the 560 pt preview width cap are gone. The video
  preview is the flexible column (Input 222 / Controls 400 / Export 240 stay
  fixed) and the player surface fills its panel (the AVPlayerLayer letterboxes
  internally), so a larger window means a larger video, not wider margins.
- Fullscreen video: new ⤢ button in the transport row opens the audition
  player in a borderless screen-filling window (same AVPlayer drives a second
  AVPlayerLayer, so playback/monitor state is shared); esc or double-click
  exits, menu bar and Dock auto-hide while open.
- SPEC-CHANGE (owner): Null monitor mode dropped from the shell — the picker
  is Pan L/R · Overlay. The core keeps mode 2 as the exact-cancellation
  oracle for the §4.8 precision tests; it's just not reachable from the UI.
- Per-track monitor volume + mute (owner request): new core user gains on the
  audition stream (`cpl_audition_set_gains`, multiplied onto the loudness-
  matching gains, click-free via the existing 5 ms params crossfade; covered
  in `core/tests/audition.rs`). The lanes give up a 100 pt column on the
  right where each lane gets a mute button + 0–2× volume slider — own space,
  never overlapping the waveforms (owner). The slider also scales that lane's
  drawn waveform so the picture tracks the ear, and mute dims the lane.
  Gains apply in both Pan L/R and Overlay. The connector layer subtracts the
  same reserve so pin positions stay aligned with the loupes.
- Playhead is now a first-class scrubber: visible whenever a session exists
  (grey when parked, bright while playing), and click or drag anywhere on the
  overview strip moves it there (owner) — audio + video together, including
  while paused, where it moves the shown frame (video seeks are coalesced so
  the frame keeps up with the pointer). Anchor pins sit above the scrub
  surface, so dragging a pin never moves the playhead. First play starts from
  a user-parked playhead instead of the anchor default.
- SPEC-CHANGE (owner, implied): the UX §2.4 coarse offset drag moved from
  plain drag on the external lane to ⌥-drag (plain drag now scrubs). Same
  gesture and scale otherwise; still gated on inputsEnabled.
- Polish round (owner): lane mute buttons get chip chrome (chipBg + border,
  filled warn state while muted) so they read as buttons, not status icons.
  SPEC-CHANGE: default loupe window 240 → 500 ms (`PairSession.defaultWinMs`;
  UX §2.3 floor) — 240 ms was too zoomed-in to judge alignment at a glance.
  Video preview drops its panel box (letterboxes into the bare column) and
  all playback controls — play, timecode, anchor jumps, monitor picker,
  fullscreen — share a single row beneath it.

## 2026-06-12 — v2 planning (session queue + queue-centric redesign)

Owner planning conversation; outcome written as **SPEC-V2.md** (the v2 charter:
pairing model, queue workflow, batch export, UI redesign targets, Phase 7–9
plan, and the Session F/G kickoff prompts). Key records:

- SPEC-CHANGE (2026-06-12, owner): the v1 framing "the editor is the product /
  the queue is an accelerator" (UX-DECISIONS v1 scope, DECIDED) is superseded
  for v2 — auto-sync proved trustworthy enough in daily use that the waveform
  stage is a sanity check, and the product surface becomes the many-takes
  triage queue. The v1 editor survives intact as the per-row drill-down.
  Attention-weight targets (visual weight, not area): queue ~25%, video ~30%,
  export ~20%, waveforms ~20%, sync tools ~5% (SPEC-V2 §1).
- SPEC-CHANGE (2026-06-12, owner): SPEC §12.2's best-z bipartite matching is
  superseded — pairing is not 1:1 (the continuous-recorder session is the
  common real shape; the v1 ground-truth pair itself is external ≫ video).
  v2 model: video-as-unit rows with candidate lists, coverage intervals,
  audio×audio triangle-closure simultaneity check, margin rule biased to
  `ambiguous` (SPEC-V2 §3, flags V2-F1/F2).
- SPEC-CHANGE (2026-06-12, owner): verified ≡ queued-for-export — one flag,
  no separate queue checkbox (SPEC-V2 §2.4, flag V2-F4).
- Decided out of scope for v2: audio stitching across a recorder restart
  (honest `split across files` state instead, V2-F5); export parallelism.
- Owner directions for the redesign recorded in SPEC-V2 §1 (overview narrows,
  sync controls share its row, sparkline stays small, offset/drift readouts
  on the fence → mockup compares disclosure-vs-gone, loupes on-demand).
- Plan: Phase 7 = headless pairing core (Session F), Phase 8 = queue-centric
  HTML mockup (Session G, may run parallel to F), Phase 9 = shell (Session H,
  prompt written after G converges). UX-DECISIONS.md untouched per the
  append-only protocol; SPEC-V2 §0 defines precedence until the
  UX-DECISIONS-V2 section lands with the converged mockup.

## 2026-06-12 — Session F, Phase 7 complete (headless pairing core)

Implemented (SPEC-V2 §4, additive only; no v1 behavior, ABI symbol, or frozen
regression value changed — full v1 suite re-run green, realdata included):

- `core/src/pair.rs`: `pair_correlate` — role-agnostic stage-1 z-selection plus
  the §5.3 refinement ladder between ANY two imported tracks; pure measurement,
  never errors on a poor match (below-floor cells keep the coarse lag for
  calibration, unrefined). `Ctx::pair_candidates` batch (rayon over cells; each
  cell deterministic per §5.8, so the matrix is bit-stable). Lag convention =
  `SyncResult.offset_ms` when (A, B) = (video, audio).
- **Stage-1 role-freedom verified, no SPEC-CHANGE needed** (SPEC-V2 §4 asked):
  the §5.2/§5.3 path consumes pyramid envelopes only; the epoch rule is baked
  into the pyramid at import; `TrackRole` lives solely in `TrackMeta`.
- C ABI: `couple.h` grew exactly two symbols — `cpl_pair_correlate`,
  `cpl_pair_candidates` (JSON-returning, free via `cpl_string_free`).
- SPEC-CHANGE (2026-06-12, Session F, V2-F3 disposition): cache-GC pinning
  mechanism = **liveness via open handles** — `Ctx::cache_gc` exempts the
  content keys of all loaded tracks; `cache::gc` gains an internal pinned set.
  The alternative (a pinned-keys parameter on `cpl_cache_gc`) would have
  changed an existing ABI signature, which Phase 7 forbids. Pinned bytes still
  count toward the cap. Test: `cache_gc_pins_open_track_handles`.
- Fixture session-folder mode (§9.1 extension, SPEC-V2 §7): `SessionSpec`
  (cameras cut from master regions, span-covering externals, per-file
  gain/AGC/SNR degradation, junk files) → `generate_session` + truth.json
  (mechanical same-master pair table: lags + coverage; expected STATES are
  owned by the gate tests, which know the resolver). Files cut from one master
  share a single event-plan domain (Poisson times depend on the domain start);
  the pink bed restarts per file span (different mics, different room noise —
  transients/tones are absolute-time exact). `plan_jitter_ms` perturbs
  transient onsets — the synthetic "different take" proxy for gate §7.8.
  CLI: `couple fixture session --out dir --spec spec.json`.
- `couple pair <dir> [--json]` (`cli/src/pair.rs`): probe → classify → import
  → candidate matrix → FULL §3.3 resolution. Coverage intervals (§5.6 overlap
  arithmetic, ppm 0); disjoint/overlapping discrimination generalized past two
  candidates via interval clustering (connected components, intersection >
  DISJOINT_TOL_S = 0.5 s [SPEC, TUNE]); §3.4 triangle closure per overlapping
  pair (A×B z ≥ Z_MIN AND |lag(V→B) − lag(V→A) − lag(A→B)| ≤ TRIANGLE_TOL_MS,
  memoized, antisymmetric); §3.5 margin rule; states confident / partial /
  split / twoSources / ambiguous / noMatch with the §3.3 chip texts
  ("audio covers 0:30–1:30 of 1:30"). `ambiguous` and `noMatch` rows have NO
  selection (V2-F2: never a coin-flip). Unused-audio tray = audio no row
  selects. Constants table (Z_MIN 8.5, Z_MARGIN 6.0 [TUNE], TRIANGLE_TOL_MS
  250 [TUNE], COVERAGE_FULL 0.98 [TUNE]) overridable via `--params` and echoed
  in `--json` with every raw z/lag/margin/residual — the corpus-calibration
  feed. Timestamps surfaced as metadata only; all tie-breaks are by file name
  (clock-free, SPEC §1.4 honored trivially).
- §3.6 stability semantics in a stateless CLI: rows contain no run-varying
  fields (no timings, no track ids, no absolute paths) and every ordering is
  name/z-deterministic, so unchanged inputs re-serialize byte-identically
  (gate 6 asserts it); user-pin persistence is Phase 9 shell state by design.
- `ffmpeg::probe` split into `probe` (v1 contract unchanged: `NoAudioStream`
  on audio-less files) + `probe_lenient` (Rust-only) so the pool can classify
  a stripped-audio video as an honest `no match` row instead of rejecting it.

Gates (SPEC-V2 §7, all headless via the real binary — cli/tests/pair_gates.rs,
10 tests, green):
1. Continuous recorder: 3 rows confident, same audio, lags ≤ 2 ms of truth. ✓
2. Recorder restart: videos 1/3 confident; video 2 `split`, max-coverage
   selected, other listed as alternate, clusters = 2. ✓
3. Simultaneous recorders: `twoSources`, A×B above floor, triangle residual
   ≤ 250 ms (measured ~53 ms), best-z default. ✓ (= gate 8's sanity direction)
4. Unrelated audio: never a candidate, lands in unused tray; video with only
   unrelated audio ⇒ `noMatch`. ✓
5. Mixed bag: junk file skipped + surfaced once; stripped-audio video ⇒
   `noMatch` ("no audio stream"); no crash, no litter. ✓
6. Stability: +1 unrelated file ⇒ original rows byte-identical in --json. ✓
7. Budget: 36-cell matrix in **46.6 ms** (≤ 5 s). ✓
8. Honesty floor (best-effort, V2-F2 — raw numbers recorded below). ✓
   Plus `margin_rule_prefers_ambiguous`: closure tolerance pinned ≈ 0 +
   Z_MARGIN 1000 via --params forces the conflict path ⇒ `ambiguous`, no
   selection, all candidates listed. ✓

V2-F2 calibration data recorded for the future corpus (the reason [TUNE]
values ride along in --json):
- Same-master/different-span cells (no shared content, similar texture) reach
  z 8.9–9.6 — ABOVE the 8.5 floor (Session C's ≤ 7.5 ceiling was fully
  unrelated content). At 40 s takes the true peak is only z ≈ 14, margin 4.9
  < 6 ⇒ the resolver honestly floated `ambiguous`; at ≥ 80 s takes true z ≈ 19
  and the margin rule rejects the decoy decisively. Short takes thin the
  margin — floor/margin re-calibration should include take length.
- Jittered-take proxy (same event plan, onsets ±400 ms): decoy z 14.8 vs true
  16.8 (margin 2.0), A×B z 19.6 / residual 53 ms ⇒ classified simultaneous,
  best-z selects the TRUE take. At ±900 ms: decoy 12.8, A×B 16.3, residual
  11 ms — STILL simultaneous (same-plan jitter shifts both lags together, and
  200 ms stage-1 bins integrate over it). Recorded limit per gate §7.8:
  synthetic takes cannot fail the simultaneity check the way real different
  takes would, and a jittered decoy ALONE (true take absent) would pair
  confidently (z 14.8 > floor, single candidate has no margin rule). Real
  multi-take corpus calibration remains required before trusting same-piece
  sessions — unchanged disposition of V2-F2.

Finish line (§12 Session F): `couple pair` on the canonical real pair (1 video
+ 1 audio; example_data/ itself now also holds two synced exports, so the run
uses just the pair) ⇒ exactly one row, `confident`, z 74.16 (mockup 75),
coverage 0.9961, lag **−5535.543 ms** vs frozen −5536.639 — Δ 1.10 ms, the
expected stage-ladder-vs-drift-fit-intercept difference at +3.57 ppm; inside
the stage tolerance window. Asserted as `realdata_pair_confident_row`
(COUPLE_REALDATA_DIR-gated, like §9.4). Matrix cell on the 2 h × 2 h pair:
76 ms (release, cache-hit imports).

Session F complete. Next: Session G (queue-centric mockup, Phase 8) per the
SPEC-V2 §12 kickoff; Session H prompt written after G converges.
