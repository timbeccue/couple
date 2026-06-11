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
