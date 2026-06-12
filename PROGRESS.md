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
