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
