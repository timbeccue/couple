# Couple — Technical Specification

**Status:** planning complete, no implementation.
**Audience:** the implementation sessions (Session C: core + CLI; Session D: SwiftUI shell) and any future Windows/Linux port.
**Companion documents:** `UX-DECISIONS.md` (authoritative for all UX behavior and validated algorithm parameters), `PROGRESS.md` (running log, append-only).

---

## 0. How to read this document

**Precedence.** For *what the product does and how it behaves*, UX-DECISIONS.md wins. This spec formalizes engineering contracts (types, formats, tolerances, processes); where it restates UX behavior it cites the UX section. If an implementation session finds a conflict between the two documents, or between this spec and reality, it must record a `SPEC-CHANGE:` entry in PROGRESS.md and get owner sign-off — never silently diverge.

**Conformance language.** MUST / MUST NOT are binding. SHOULD means deviate only with a recorded reason. MAY is free choice.

**Constant tags.** Every numeric constant in Appendix A carries one of:
- `[DECIDED]` — fixed in UX-DECISIONS.md; do not change without owner sign-off.
- `[VALIDATED]` — from the validated mockup; treat as correct starting value, tune only with evidence.
- `[SPEC]` — chosen by this spec (new territory: 2-hour scaling, formats, budgets); change with a SPEC-CHANGE entry.
- `[TUNE]` — explicitly expected to be tuned in Session C against a growing corpus (UX §4.9, §6.4).

---

## 1. Ground truth from example_data/

All format, codec, and metadata decisions below are grounded in the real recording pair in `example_data/`. Probe summaries are in Appendix B.

### 1.1 What the devices produce

**Camera** (`schumann-video.mp4`, 3.1 GB): Android 13 phone (`com.android.version=13` container tag). MP4 (`mp42`), H.264 High@4.0 1920x1080, **variable frame rate** (nominal 30 fps tag, actual average 30.0064 fps), 17 Mbps, bt709. Audio: AAC-LC, 48 kHz stereo, 256 kbps, heavily AGC-limited. Container `creation_time=2022-12-10T02:30:01Z` (plausible/valid UTC). GPS tag present but zeroed.

**External recorder** (`schumann-audio.wav`, 518 MB): Zoom H4n. WAV, PCM s24le, 48 kHz stereo, 1798.88 s. BWF `bext` metadata present: `encoded_by=ZOOM Handy Recorder H4n`, `date=2011-04-20`, `creation_time=06:34:12`, `time_reference=1135296000` samples (= 06:34:12 since midnight, consistent with the time-of-day field).

### 1.2 Finding: the camera's audio stream does not start at container time zero

The mp4's audio stream has `start_time = 0.180792 s` (first audio PTS 8678 at 1/48000). The video stream starts at 0. **Camera audio sample 0 corresponds to video time 0.180792 s, not 0.**

Consequences (normative):

- **The canonical timeline for everything in this app is the video container's presentation timeline** ("video time", `t`, seconds, 0 = container start). This is what AVFoundation displays, what export muxes against, and what UX-DECISIONS §2.1 calls "video time".
- The engine MUST position decoded camera audio on this timeline using container timestamps (§7.2). Ignoring the stream epoch would make every measured offset — and therefore every export — wrong by the epoch amount (~181 ms here; an obvious lip-sync error, human perception threshold being ~40–80 ms per UX §6.2).
- The engine MUST NOT use any timestamp-resynthesis or async-resample decode options that would mask or alter this epoch (§7.2).

### 1.3 Finding: the mockup's validated offset is in a different time base

`peaks/camera.json` has `duration_seconds = 1435.0933`, which matches the audio *stream* duration (1435.0709 s, plus ~22 ms of AAC priming variance) and not the epoch-inclusive duration (1435.2517 s). So the mockup's peak extraction dropped the 180.79 ms lead, and the validated numbers in UX-DECISIONS (offset −5354.74 ms) are **camera-audio-stream-relative**.

Converted to the canonical (container) timeline: `offset_container = offset_stream − epoch · (1 + drift·1e−6)` = −5354.74 − 180.79 ≈ **−5535.53 ms**, with ~±25 ms of uncertainty from the unknown details of the mockup's extraction (AAC priming handling). Drift (+3.07 ppm) and fit RMS (0.276 ms) are epoch-invariant and carry over unchanged.

The real-data test plan therefore uses a **freeze procedure** (§9.4) instead of asserting an exact offset from day one.

### 1.4 Finding: recording timestamps are unreliable for pairing (v2)

The H4n's clock was never set: it believes the recording happened 2011-04-20 06:34, while the camera (correctly) says 2022-12-10. Even the WAV's filesystem mtime carries the bogus 2011 date. Cross-device timestamp pairing is therefore **hopeless on this hardware**: an 11.6-year disagreement, and no reason to expect users to maintain recorder clocks.

Normative consequence for the v2 queue (§12): pairing MUST be correlation-primary (all-pairs envelope correlation, which §2.6 of UX-DECISIONS already requires as verification and which the cached envelopes make cheap), with metadata timestamps demoted to a *same-device ordering hint* (a device's clock, though set wrong, still runs monotonically within a session). This is a flagged, evidence-based inversion of UX §2.6's phrasing ("via recording timestamps …, verified or supplemented by cross-correlation").

V1 consequence: the probe MUST already extract and preserve these metadata fields (container creation_time, BWF bext block, device strings) so v2 has them (§4.2 `MediaInfo`); this costs nothing now.

### 1.5 Other grounded observations

- **VFR video**: no engine or shell math may ever derive time from nominal frame rate; always use container timestamps. (Affects a possible future "frame-relative drift threshold" — half a frame at 30 fps is 16.7 ms — noted under §6.3 disposition, §14.)
- **The AGC asymmetry is real and extreme** (see `peaks/sanity_compare.png`): the camera track is a near-square limiter wall, the external preserves dynamics. This is why per-loupe local normalization is DECIDED (UX §1.6) and why log-compressed envelopes validated well for NCC.
- **Hand-synced exports are absent** from example_data/ (only the two media files and `peaks/`). The hand-sync regression check (§9.5) is specified but dormant until the owner drops exports into `example_data/handsync/`.
- The earlier extraction in `peaks/` (`SCHEMA.md`, `camera.json`, `external.json`) is a useful Phase-1 cross-check: the engine's pyramid levels must correlate ≥ 0.99 with it (§9.2, with the epoch caveat from §1.3).

---

## 2. Flags raised during this spec (concentrated, per process instruction)

Each flag states the issue and the resolution adopted by this spec. None blocks implementation; owner may override any of them via PROGRESS.md.

| # | Flag | Resolution adopted |
|---|---|---|
| F1 | Camera audio stream epoch (start_time 180.79 ms) is not mentioned in UX-DECISIONS and would silently break sync/export if ignored. | Canonical-timeline rule, §1.2/§7.2. Epoch handled once at import; everything downstream is container time. |
| F2 | Mockup-validated offset is stream-relative; exact container-time ground truth uncertain by ~±25 ms (AAC priming). | Freeze procedure §9.4: first verified engine run establishes the frozen regression value; predicted −5535.5 ± 25 ms used as a sanity window only. |
| F3 | Recorder clock unset (2011 vs 2022) makes timestamp-based v2 pairing unreliable. | v2 pairing spec inverted: correlation-primary, timestamps as same-device ordering hints (§12.2). Deviation from UX §2.6 wording, flagged here. |
| F4 | No hand-synced exports in example_data/. | Regression check specified but dormant; drop-in protocol defined (§9.5). |
| F5 | "Core is async throughout" (prompt) read literally suggests async/await. | Interpreted as: non-blocking jobs, progress callbacks, cancellation tokens, implemented with OS threads + rayon (§3.5). Rust async/await is deliberately NOT used in core (FFI hostility, no I/O concurrency need). |
| F6 | Mockup's brute-force stage-1 NCC won't meet budget at 2 h × 2 h (~25x the validated workload). | Stage-1 numerator MUST be FFT-based (or provably-equivalent SIMD) with identical z-score selection (§5.3); drift-pass search span scales with distance from overlap center (§5.6). New constants marked [SPEC]. |
| F7 | UX §2.7 specifies loudness matching for Overlay/Null only; Pan L/R unstated. | Loudness matching applied in all three monitor modes for consistency (§4.8). One-line change if owner disagrees. |
| F8 | "Offset applied sample-accurately" + "audio untouched (offset only)" (UX §1.5/§2.8) are in tension below one sample. | Drift mode None = integer-sample shift, bit-exact samples (max placement error 10.4 µs at 48 kHz). Resample mode = full fractional mapping. §8.2. |
| F9 | PCM in MP4 is non-portable; UX §2.8 delegates codec defaults. | Export matrix §8.4: MOV gets PCM (s24le), MP4 gets AAC 256 kbps (AudioToolbox encoder on macOS). No visible codec choice in v1. |
| F10 | §6.5 (sub-ms zoom) resolved as raw-sample fetch, which requires a decoded-PCM disk cache (~0.7 GB per hour per track, f32 mono). | Accepted; cache + GC policy §6.3. Also powers audition and avoids re-decode. Disk preflight required. |
| F11 | Shell A/V audition sync runs on two clocks (AVAudioEngine vs AVPlayer). | Audio is master; video slaved with ≤ 1 frame (33 ms) skew budget (§11.4). Interacts with §6.2 (transient lamps) — owner decision unchanged. |
| F12 | Synthetic fixtures need a video stream but the LGPL ffmpeg build excludes x264. | Fixture videos encoded with the native `mpeg4` encoder (quality irrelevant for tests). §9.1. |

---

## 3. Architecture

### 3.1 Components

- **`couple-core`** (Rust library, the product): decode orchestration, peak pyramids, sync algorithms, alignment session state, audition rendering, export pipeline. No UI, no audio hardware, no video rendering. Exposes (a) a Rust API and (b) a C ABI (`couple.h`, generated by cbindgen) — the **porting surface**.
- **`couple-cli`** (Rust binary `couple`, thin wrapper over couple-core): every core capability as a subcommand with `--json` output. The **portable artifact**, the test harness, and the reference client. Non-negotiable per the architecture constraints.
- **`CoupleKit`** (Swift package): Swift wrapper over the C ABI — handles, async/await bridging of jobs, value types mirroring the JSON schemas.
- **`Couple.app`** (SwiftUI, macOS 14+): window, drag-drop, waveform stage rendering, AVFoundation video preview, AVAudioEngine audition output, keyboard map. Owns the audio device and all rendering per UX §4.7.
- **Bundled ffmpeg + ffprobe** (separate LGPL-built executables, §3.3): all demux/decode/encode/mux, always as child processes of couple-core.

Repository layout:

```
couple/
  core/           # couple-core crate
  cli/            # couple-cli crate
  ffi/            # cbindgen config, couple.h output, ABI tests
  mac/            # Xcode project: CoupleKit + Couple.app
  fixtures/       # synthetic fixture generator output (gitignored) + goldens
  example_data/   # real pair (gitignored content, present locally) + handsync/ drop-in
  third_party/ffmpeg/   # build scripts + pinned version manifest (binaries not in git)
  SPEC.md  UX-DECISIONS.md  PROGRESS.md
```

### 3.2 Core-to-shell transport: FFI vs subprocess — analysis and decision

The workload has three traffic classes:

1. **Hot pull paths**: `band()` for 4 loupes + overview at 60 fps during drags (~300 calls/s, each must return in tens of µs); `audition_fill()` from the audio render thread every 5–10 ms with hard deadlines.
2. **Interactive one-shots**: `measure_local()` on a 450 ms debounce, ≤ 5 ms each.
3. **Jobs**: import, auto-sync, export — seconds to minutes, progress + cancel.

A subprocess boundary (JSON or even binary over pipes) handles class 3 well and class 2 acceptably, but fails class 1: per-call pipe round-trips are 100 µs–1 ms with scheduling jitter, and the audio render thread cannot block on a pipe. The alternatives — replicating the pyramid into the shell process or streaming audio ahead over pipes — re-implement half the core on the shell side of the boundary, which is exactly what the porting contract is supposed to prevent.

**Decision: the macOS shell links couple-core in-process via the C ABI.** Crash-isolation concerns are low: couple-core is safe Rust doing DSP and file I/O; the crash-prone surface (codec parsing) already lives in ffmpeg child processes (§3.3). The subprocess option remains available to ports via the CLI for everything except real-time audition (§13.3 defines the degraded mode).

### 3.3 ffmpeg: libraries vs subprocess — analysis and decision

| Dimension | Link libav* | Bundle ffmpeg/ffprobe executables |
|---|---|---|
| LGPL compliance | Dynamic linking required for a clean §6 story; static linking obliges object-file relink provisions. App Store + LGPL dylibs is a known legal grey zone (FFmpeg's own FAQ flags it). | Separate processes = mere aggregation, the cleanest LGPL story for both direct distribution and App Store. Obligations: ship LGPL-only build, publish exact source + build scripts, keep the binary replaceable. |
| Build/maintenance | Cross-compile + vendor 6 libraries per arch; API churn between majors. | One pinned build script; binaries are leaf artifacts. |
| Crash isolation | A demuxer crash takes the app down. | A crash kills one job; core reports `FfmpegFailed`. |
| Performance | In-process, zero-copy. | Raw f32 PCM over pipes moves at GB/s; decode is the bottleneck, not the pipe. Process spawn ~10 ms, irrelevant at job granularity. |
| Capability | Full control of edit lists, priming, muxing. | The CLI exposes everything this app needs; it is also the most battle-tested mux path. |

**Decision: bundled subprocess executables** (`ffmpeg`, `ffprobe`), signed, in `Couple.app/Contents/Helpers/`, invoked by couple-core. Version pinned in `third_party/ffmpeg/VERSION` (initial pin: ffmpeg 8.0.x, matching the dev machine's 8.0.1); CI and the app use the same pin — this matters because AAC priming/edit-list handling differences across ffmpeg versions can shift decoded epochs by a few ms (§1.3). Resolution order for the binary: explicit config (`CtxConfig.ffmpeg_path`) > `COUPLE_FFMPEG` env > bundled helper > `$PATH` (dev convenience).

**Build profile (LGPL-only, minimal):** `--disable-everything --disable-gpl --disable-nonfree`, then enable: protocols `file,pipe`; demuxers `mov,mp4,m4a,3gp` (one family), `wav,aiff,flac,mp3,matroska`; decoders `aac,alac,flac,mp3,pcm_*`; encoders `pcm_s16le,pcm_s24le,pcm_f32le,aac,aac_at` (aac_at via `--enable-audiotoolbox`, macOS only), `mpeg4` (fixtures only, §9.1); muxers `mov,mp4,wav,ipod,f32le,s24le,null`; filters `pan,aresample,atrim,volume`. No video *decoders* are needed in v1 — video is always stream-copied, and demuxing/copying does not require a decoder. Resulting binary ~5–10 MB/arch.

Licensing summary: app proprietary; ffmpeg LGPL-2.1+ as separate executables with published build provenance; all Rust crates MIT/Apache-2.0 (§3.6). No GPL anywhere.

### 3.4 Why Rust (constraint accepted, briefly argued)

Invited to argue; no argument. Requirements — SIMD-friendly DSP, rayon-grade data parallelism, memory-mapped multi-GB caches, a stable C ABI surface, zero-runtime distribution on three OSes — are Rust's home turf. C++ offers no capability win and loses memory safety; Swift core would make the Windows/Linux port the second-class citizen this project explicitly refuses to have. Crate licenses are all MIT/Apache-2.0.

### 3.5 Concurrency model (interpretation of "async throughout" — see F5)

- Every long operation (import, auto-sync, export) is a **job**: started non-blocking, reports progress via callback, honors a cancellation token. Implemented with OS threads + rayon worker pools — **no async runtime in core** (tokio adds FFI hostility and solves an I/O-concurrency problem this engine doesn't have).
- Progress callbacks: invoked from worker threads, ≤ 30 Hz per job, monotone non-decreasing fraction within a phase; payload per §4.6. Callbacks MUST be cheap and non-blocking; the shell hops to the main actor.
- Cancellation: cooperative flag, checked at least every 100 ms of work; ffmpeg children are killed on cancel; partial outputs/caches are removed; result is the `Cancelled` error. Idempotent.
- Hot calls (`band`, `raw_window`, `audition_fill`, `measure_local`, state getters/setters) are synchronous, thread-safe, and callable from any thread. `audition_fill` is real-time-safe after stream open (no locks, no allocation; mmap page faults mitigated per §4.8).
- Concurrency limits [SPEC]: ≤ 2 import jobs in flight (one per slot); 1 auto-sync job per session (second start returns `InvalidArgument`); 1 export job per session.

### 3.6 Crate set (pinned at Phase 1, all MIT/Apache-2.0)

`serde`/`serde_json`, `clap`, `thiserror`, `rayon`, `rustfft` + `realfft`, `rubato` (resampling; `soxr` behind a feature flag as quality fallback — Session C benchmarks against the §8.2 quality bar), `blake3`, `memmap2`, `tempfile`, `crossbeam-channel`, `indicatif` (CLI only), `cbindgen` (build), `hound` (dev/test WAV convenience only), `rand`+`rand_chacha` (fixtures).

### 3.7 Data flow (one pair, v1)

1. Shell hands two paths to core. Core probes both (ffprobe JSON), classifies camera (has video stream) vs external (audio-only) — same-kind collisions are the shell's confirmation flow (UX §3).
2. Import per file (job): ffmpeg decodes to mono 48 kHz f32 on the canonical timeline (§7.2); a single streaming pass tees into (a) the PCM cache file, (b) the peak pyramid builder, (c) level statistics; onset ranking runs from the finished 480-spb level. Artifacts land in the content-addressed cache (§6.3).
3. Auto-sync (job, auto-started when both imports land, per UX §2.5): §5 pipeline; on success core updates the session's AlignmentState and returns a `SyncResult`.
4. Shell renders: overview + loupes via `band`/`raw_window`; readouts from state; sparkline from `drift_windows`.
5. User nudges: shell writes `set_offset_ms`/`set_drift_ppm` at gesture rate; re-measures anchors via `measure_local` on its 450 ms debounce (UX §2.4 owns the debounce policy).
6. Audition: shell opens an audition stream; AVAudioSourceNode pulls `audition_fill`; AVPlayer (muted) is slaved for video (§11.4).
7. Export (job): render aligned audio (§8.2), pipe into ffmpeg mux with video stream-copy (§8.4); or standalone WAV.

---

## 4. Core/shell contract (typed API)

Rust signatures are normative for semantics; the C ABI mapping (§4.9) is normative for ports. JSON field names below are the wire format (camelCase, matching UX §4.6).

### 4.1 Conventions

- **Video time** `t`: seconds, f64, canonical container timeline (§1.2). **External time** `u`: seconds into the external audio file (its sample 0 = 0.0).
- **Mapping** (UX §2.1, verbatim): `u(t) = t·(1 + driftPpm·1e−6) + offsetMs/1000`. Local lag `L(t) = offsetMs + t·driftPpm·1e−3` (ms). Measured local lag `λ(t)` same sign/units; displayed residual `Δᵢ = λ(tᵢ) − L(tᵢ)`. Positive drift ⇒ the external recorder's clock runs fast relative to the camera.
- Offsets in **ms** (f64; UI resolution 0.01 ms), drift in **ppm** (f64; UI resolution 0.01 ppm), times in **seconds** (f64). All lag math in f64.
- **Failure is data, error is plumbing**: a sync that finds no peak returns `Ok(SyncResult{status: Failed(...)})` because the UI treats it as a state (UX §3); `Err(CoupleError)` is reserved for environmental problems (missing file, decode failure, disk full, cancelled).

### 4.2 Identity and media types

```rust
pub type TrackId = u64;  pub type SessionId = u64;  pub type JobId = u64;
pub enum TrackRole { Camera, External }

pub struct MediaInfo {                    // from probe (ffprobe -print_format json)
    pub path: String,
    pub container: String,
    pub duration_s: f64,                  // container duration
    pub video: Option<VideoStreamInfo>,   // codec, width, height, nominal_fps, avg_fps, start_time_s, bit_rate
    pub audio: Vec<AudioStreamInfo>,      // codec, sample_rate, channels, bits_per_sample, start_time_s, duration_s
    pub creation_time_utc: Option<String>,// ISO 8601, container-level
    pub bwf: Option<BwfInfo>,             // { date, time, time_reference_samples, encoded_by, coding_history }
    pub tags: BTreeMap<String, String>,   // remaining container tags (device hints for v2 pairing)
}
```

`bwf`, `creation_time_utc`, and `tags` MUST be populated in v1 even though only v2 consumes them (§1.4).

```rust
pub struct Track {
    pub id: TrackId,
    pub role: TrackRole,
    pub info: MediaInfo,
    pub epoch_s: f64,        // decoded audio stream's container start_time (camera); 0.0 for plain audio files
    pub duration_s: f64,     // decoded duration on the canonical timeline (includes epoch lead)
    pub sample_rate: u32,    // analysis rate, always 48_000
    pub normalization: f64,  // track absolute peak (UX §4.1)
    pub stats: TrackStats,
}
pub struct TrackStats {
    pub abs_peak: f64,
    pub active_p95_9600: f64,    // p95 of envelope bins > 12% of track max, at 9600 spb (UX §4.1 gain rule input)
    pub active_rms_dbfs: f64,    // RMS (dBFS) over active bins at 2400 spb (loudness matching input, §4.8)
}
```

### 4.3 Import, peaks, raw windows, onsets

```rust
pub fn probe(ctx: &Ctx, path: &Path) -> Result<MediaInfo>;

pub fn import_start(ctx: &Ctx, path: &Path, role: TrackRole,
                    progress: ProgressFn, cancel: CancelToken) -> Result<JobId>;
// job result: Track. Caches reused when content key matches (§6.3); a cache hit still reports progress (fast).

pub struct Band { pub level_spb: u32, pub col_t0: f64, pub col_dt: f64,
                  pub mins: Vec<i8>, pub maxs: Vec<i8> }   // FFI: caller-allocated buffers
pub fn band(ctx: &Ctx, track: TrackId, t0: f64, t1: f64, n_cols: u32) -> Result<Band>;
// Coarsest pyramid level still yielding >= n_cols bins over [t0,t1]; aggregate bins to columns by min/max
// (UX §4.1). Columns outside the track are (0,0). Hot path: tens of µs, no allocation on the FFI route.

pub fn raw_window(ctx: &Ctx, track: TrackId, t0: f64, t1: f64, out: &mut [f32]) -> Result<u32>;
// Mono 48 kHz f32 from the PCM cache, canonical timeline. Max span RAW_FETCH_MAX_S = 8 s [SPEC].
// Serves loupe deep zoom below ~120 ms windows (§14, item 6.5) — sample resolution 20.8 µs beats
// the 8-spb alternative (167 µs) and costs no RAM.

pub fn rms(ctx: &Ctx, track: TrackId, t0: f64, t1: f64) -> Result<f64>;  // dBFS over active bins in range

pub struct Onset { pub t: f64, pub score: f64 }
pub fn best_onset_in(ctx: &Ctx, track: TrackId, t0: f64, t1: f64) -> Option<Onset>;
pub fn best_onset_near(ctx: &Ctx, track: TrackId, t: f64, half_span_s: f64) -> Option<Onset>; // snap: ±4 s
pub fn onsets(ctx: &Ctx, track: TrackId, t0: f64, t1: f64, limit: u32) -> Vec<Onset>;
```

### 4.4 Sync

```rust
pub struct SyncParams { /* every Appendix-A sync constant, Default::default() = Appendix A values */ }

pub struct StagePeak   { pub lag_ms: f64, pub ncc: f64, pub z: f64, pub overlap_bins: u64 }
pub struct DriftWindow { pub t: f64, pub lag_ms: f64, pub ncc: f64, pub accepted: bool }
pub struct OverlapInfo { pub t0: f64, pub t1: f64, pub seconds: f64, pub fraction_of_video: f64 }

pub enum FailReason  { NoPlausiblePeak { best_z: f64 }, InsufficientOverlap { overlap_s: f64 } }
pub enum SyncStatus  { Synced, LowConfidence, Failed(FailReason) }
pub enum DriftMode   { Resample, None }          // wire: "resample" | "none"

pub struct SyncResult {
    pub offset_ms: f64,              // final mapping (post-fit when drift measurable)
    pub drift_ppm: f64,              // 0.0 when not measurable
    pub drift_measurable: bool,      // ≥ 3 accepted windows (UX §4.3)
    pub coarse: StagePeak,           // stage-1 winner (z-selected)
    pub runner_up_z: f64,            // best stage-1 z outside ±2 bins of winner (separation confidence)
    pub fine_ncc: f64,               // NCC at the 24-spb stage
    pub windows: Vec<DriftWindow>,   // all 12, accepted or not (sparkline wants both, UX §1.4)
    pub fit_rms_ms: Option<f64>,
    pub end_to_end_drift_ms: f64,    // |drift_ppm| · overlap.seconds / 1000
    pub overlap: OverlapInfo,
    pub status: SyncStatus,
    pub suggested_drift_mode: DriftMode,   // UX §4.9 heuristic, threshold SUGGEST_RESAMPLE_MS
    pub compute_ms: f64,
}

pub fn auto_sync_start(ctx: &Ctx, session: SessionId, params: &SyncParams,
                       progress: ProgressFn, cancel: CancelToken) -> Result<JobId>;
// On Synced/LowConfidence: core writes offset/drift/driftWindows into the session state, sets
// driftMode + savedDriftPpm per the suggestion, leaves verified=false. On Failed: state untouched
// (mapping stays zero per UX §3 "no half-applied guess").

pub struct LocalMeasurement { pub lag_ms: f64, pub ncc: f64 }
pub fn measure_local(ctx: &Ctx, session: SessionId, tc: f64, half_win_s: f64,
                     center_ms: f64, span_ms: f64) -> Result<Option<LocalMeasurement>>;
// UX §4.3. Synchronous; budget ≤ 5 ms. None when either window lacks signal (variance gate) or
// the peak is degenerate. Caller (shell) owns debounce policy and writes results into state.

pub struct Fit { pub off_ms: f64, pub ppm: f64, pub rms_ms: f64 }
pub fn fit_ls(points: &[(f64, f64)]) -> Option<Fit>;   // (t_s, lag_ms); None if < 2 points. Pure function.
```

### 4.5 Session and alignment state (UX §4.6 formalized)

The core owns the canonical `AlignmentState`; the shell mutates it through setters and mirrors it for rendering. Rationale: `audition_fill` (audio thread) and auto-sync both need it, and persistence serializes it. The shell is the only mutator outside auto-sync, so it needs no change-subscription in v1.

```rust
pub fn session_new(ctx: &Ctx, camera: TrackId, external: TrackId) -> Result<SessionId>;
pub fn session_free(ctx: &Ctx, session: SessionId);

// Hot setters (gesture-rate, atomic, lock-free):
pub fn set_offset_ms(ctx: &Ctx, s: SessionId, v: f64);
pub fn set_drift_ppm_live(ctx: &Ctx, s: SessionId, v: f64);  // mapping/rendering only
pub fn commit_drift(ctx: &Ctx, s: SessionId);                // audition re-render point (UX §2.7/§4.7)
// Cold setters: set_drift_mode (implements UX §1.5 None-zeroes-mapping + savedDriftPpm restore +
// edit-flips-to-resample), set_verified, set_anchors, set_frames, set_lock_zoom, set_loupe_layout,
// set_per_anchor_measurements, set_drift_windows.
pub fn state_json(ctx: &Ctx, s: SessionId) -> String;
pub fn set_state_json(ctx: &Ctx, s: SessionId, json: &str) -> Result<()>;
```

Wire schema (exactly UX §4.6 plus a version field):

```json
{
  "version": 1,
  "offsetMs": -5535.53, "driftPpm": 3.07,
  "driftMode": "resample", "savedDriftPpm": 3.07,
  "anchors": [12.4, 700.0, 1410.2],
  "frames": [{ "winMs": 240, "auto": true }],
  "lockZoom": true,
  "perAnchorMeasurements": [{ "lagMs": -5531.2, "ncc": 0.71 }, null],
  "driftWindows": [{ "t": 60.1, "lagMs": -5535.3, "ncc": 0.65 }],
  "verified": false,
  "loupeLayout": "grid"
}
```

Invariants enforced by core: anchors ascending, 2–4 of them [DECIDED, cap confirmed §14]; `frames`/`perAnchorMeasurements` parallel to `anchors`; `driftMode: "none"` implies the live mapping uses drift 0 while `savedDriftPpm` retains the measurement; any drift write while in `"none"` flips mode to `"resample"` (UX §1.5 — enforced in core so CLI and shell agree).

Persistence wrapper (§14, item 6.8):

```json
{ "schema": "couple-session/1", "savedAt": "ISO8601",
  "camera":   { "path": "...", "contentKey": "blake3-128…", "durationS": 1435.296 },
  "external": { "path": "...", "contentKey": "blake3-128…", "durationS": 1798.88 },
  "alignment": { /* AlignmentState v1 */ } }
```

### 4.6 Jobs, progress, cancellation

```rust
pub enum Phase { Probe, Decode, Pyramid, Finalize,            // import
                 SyncCoarse, SyncRefine, SyncWindows, SyncFit, // auto-sync (UI copy per UX §3)
                 ExportRender, ExportMux }
pub struct Progress { pub job: JobId, pub phase: Phase, pub fraction: f64, pub detail: Option<String> }
pub type ProgressFn = /* thread-safe callback, ≤ 30 Hz, monotone within phase */;
pub struct CancelToken(Arc<AtomicBool>);   // cancel(): cooperative, observed ≤ 100 ms, idempotent
```

Auto-sync progress mapping [SPEC]: SyncCoarse 0–70%, SyncRefine 70–75%, SyncWindows 75–98% ("Measuring drift…"), SyncFit to 100%. Import: Decode 0–80%, Pyramid 80–95% (single streaming pass; fractions from bytes consumed), Finalize to 100%.

### 4.7 Errors

```rust
pub enum CoupleError {
    FileNotFound { path },           UnsupportedFormat { path, detail },
    NoAudioStream { path },          AmbiguousRole { path, detail },     // e.g. two video files
    FfmpegMissing { searched },      FfmpegFailed { stage, exit_code, stderr_tail },
    DecodeFailed { path, detail },   CacheCorrupt { path },              // self-heals: evict + reimport
    DiskFull { needed_bytes, available_bytes },
    InvalidArgument { detail },      Cancelled,
    ExportFailed { stage, detail },  Internal { detail },
}
```

FFI: every call returns an i32 status (0 ok, else error class); details retrieved as JSON via `cpl_take_error_json(ctx)` (thread-local last-error). Exit-code mapping for the CLI in §10.3.

### 4.8 Audition interface (matches UX §4.7 precisely)

Core supplies sample-accurate aligned audio for a video-time position and monitor mode; shell owns the device, the video renderer, and A/V synchronization. Pull model with a stream cursor:

```rust
pub enum MonitorMode { PanLR, Overlay, Null }    // wire: "pan" | "overlay" | "null"

pub fn audition_open(ctx: &Ctx, s: SessionId, mode: MonitorMode, t_start: f64) -> Result<StreamId>;
pub fn audition_fill(ctx: &Ctx, stream: StreamId, out_interleaved_stereo: &mut [f32]) -> Result<FillInfo>;
//   FillInfo { frames: u32, stream_t: f64 }  — 48 kHz stereo f32; advances the cursor; zeros + frames<requested
//   only past the video end. Real-time-safe: no locks/allocation after open; core touches (madvise) the
//   upcoming cache region on open/seek.
pub fn audition_seek(ctx: &Ctx, stream: StreamId, t: f64) -> Result<()>;
pub fn audition_set_mode(ctx: &Ctx, stream: StreamId, mode: MonitorMode) -> Result<()>;
pub fn audition_close(ctx: &Ctx, stream: StreamId);
```

Semantics (normative):

- Timeline = video time. Camera signal comes from the camera PCM cache (epoch-correct by construction, §7.2); external signal is read through the **current mapping**: offset always live and sample-accurate; drift per the *committed* value only, and only when `driftMode = "resample"` — when `"none"`, playback is offset-only, matching export (UX §1.5/§2.7).
- Drift rendering is windowed-sinc interpolation evaluated at `u(t)` (32-tap Kaiser [SPEC] — audition-grade; export uses the §8.2 high-quality path). No live varispeed while dragging: drift changes take effect at `commit_drift` (DECIDED, UX §2.7); offset changes take effect on the next fill.
- On any mapping change between contiguous fills, core SHOULD crossfade old/new renderings over 240 samples (5 ms) to suppress clicks during nudge-while-playing.
- Monitor matrices, with per-track loudness gains `g = 10^((TARGET_DBFS − active_rms_dbfs)/20)` clamped to ≤ +24 dB, computed over the overlap region (flag F7: applied in all three modes):
  - **PanLR**: L = g_cam · cam_mono, R = g_ext · ext_mono.
  - **Overlay**: L = R = 0.5 · (g_cam · cam + g_ext · ext).
  - **Null**: L = R = 0.5 · (g_cam · cam − g_ext · ext). UI copy caveat per UX §2.7 (relative dip, never silence).
  - Hard clamp to [−1, 1]; no limiter in v1.
- Mono feeds are the cached analysis signals (mean of source channels, §7.2) — stereo imaging is not a goal of audition.
- Optional helper kept cheap per UX §4.7: `envelope_at(track, t)` and `onsets_between(track, t0, t1)` for meters/lamps if the owner keeps them (§14, item 6.2).

### 4.9 C ABI (the porting surface)

Rules: C calling convention; all functions prefixed `cpl_`; opaque u64 handles; UTF-8 paths; caller-allocated buffers for hot paths; JSON strings (core-allocated, freed with `cpl_string_free`) for structured data; `cpl_abi_version()` gate (bump on any breaking change). cbindgen generates `couple.h`; the header is committed and ABI-diffed in CI.

Function inventory (signatures in the header; semantics above):

- Lifecycle: `cpl_version`, `cpl_abi_version`, `cpl_ctx_new(config_json)`, `cpl_ctx_free`, `cpl_string_free`, `cpl_take_error_json`.
- Media: `cpl_probe_json`, `cpl_import_start`, `cpl_track_info_json`, `cpl_track_free`.
- Jobs: `cpl_job_cancel`, `cpl_job_free`; progress/completion via `CplProgressCb(job, phase, fraction, payload_json, user)` — terminal call has phase `"done"` (payload = result JSON), `"error"`, or `"cancelled"`; invoked from worker threads.
- Reads: `cpl_band`, `cpl_raw_window`, `cpl_rms`, `cpl_onsets_json`, `cpl_best_onset_in_json`, `cpl_best_onset_near_json`, `cpl_track_stats_json`.
- Session/state: `cpl_session_new`, `cpl_session_free`, `cpl_state_json`, `cpl_set_state_json`, `cpl_set_offset_ms`, `cpl_set_drift_ppm_live`, `cpl_commit_drift`, `cpl_set_drift_mode`, `cpl_set_verified`.
- Sync: `cpl_autosync_start`, `cpl_measure_local_json`, `cpl_fit_ls_json`.
- Audition: `cpl_audition_open`, `cpl_audition_fill`, `cpl_audition_seek`, `cpl_audition_set_mode`, `cpl_audition_close`.
- Export: `cpl_export_start(ctx, session, opts_json, cb, user)` (opts cover mux and WAV kinds, §8).
- Cache: `cpl_cache_gc(ctx, max_bytes)`.

---

## 5. Sync algorithm (formalizing UX §4.2–4.5; parameters preserved exactly)

### 5.1 Envelope definition

For pyramid level with `spb` samples/bin and int8 peak pairs `(min_i, max_i)` (range −127..127): span `s_i = max_i − min_i` (0..254, int domain — chosen for mockup parity, since Pearson NCC is scale-invariant but the log curvature is not), envelope `e_i = ln(1 + s_i)`. The log compression is part of the validated design [DECIDED].

### 5.2 Normalized cross-correlation and the significance score

For camera envelope `a` (length Na bins) and external envelope `b` (length Nb) at the same level, integer lag `ℓ` (bins, external relative to camera): the overlap index set is `i ∈ [max(0,−ℓ), min(Na, Nb−ℓ))`, size `n(ℓ)`. NCC(ℓ) is the Pearson correlation of `a[i]` vs `b[i+ℓ]` over that set. Selection score at stage 1 [DECIDED, empirically mandatory per UX §4.2]:

```
z(ℓ) = NCC(ℓ) · sqrt(n(ℓ))      subject to n(ℓ) ≥ MIN_OVERLAP_BINS
```

Implementation requirement [SPEC, F6]: per-lag means/variances come from prefix sums of `a, a², b, b²`; the cross term `Σ a[i]·b[i+ℓ]` for all lags at stage 1 MUST be computed via FFT linear correlation (real FFT, zero-padded to ≥ Na+Nb−1) or a SIMD method proven equivalent on the test fixtures (identical winner and z within 1e−6). This is exact Pearson per lag, O((Na+Nb) log) instead of O(Na·Nb). At 2 h x 2 h (36k bins each at stage 1) this is milliseconds; brute force (~2.6e9 multiplies) would blow the §9.6 budget.

### 5.3 Stage sequence (global offset search; UX §4.2 verbatim)

| Stage | Level (spb) | Bin duration | Lag span searched | Selection |
|---|---|---|---|---|
| 1 | 9600 | 0.2 s | full ±(dur_cam + dur_ext), `n ≥ 100` bins (= 20 s min overlap) | max z |
| 2 | 2400 | 50 ms | ±500 ms around stage-1 peak | max NCC |
| 3 | 480 | 10 ms | ±80 ms | max NCC |
| 4 | 96 | 2 ms | ±14 ms | max NCC |
| 5 | 24 | 0.5 ms | ±3 ms | max NCC |

Each stage scans integer bin lags across its span and finishes with **parabolic sub-bin interpolation**: for winner `y₀` with neighbors `y₋, y₊`, `δ = 0.5·(y₋ − y₊)/(y₋ − 2y₀ + y₊)` (clamped to |δ| ≤ 0.6; skipped at span edges), refined lag = `(ℓ + δ) · binMs`. Refinement stages (2–5) compute NCC directly (spans are ≤ 21 lags; envelopes windowed to the overlap implied by the current lag). Stage 5's interpolated lag is the **stage offset** `offset_stage` (ms). Outputs retained for `SyncResult`: stage-1 `StagePeak` + runner-up z (best z at distance > 2 bins from winner), stage-5 NCC as `fine_ncc`.

Progress callbacks during stage 1 only (UX §4.2); stages 2–5 are near-instant.

### 5.4 Windowed local-lag measurement (UX §4.3 verbatim, formalized)

`measure_local(tc, half_win_s = 9, center_ms, span_ms)`:

1. Camera window: envelope bins covering `[tc − half_win, tc + half_win]` at 96 spb (2 ms bins; ~9000 bins for the default 18 s window).
2. Coarse scan: for lag λ over `center ± span` in 2 ms steps, NCC of camera window vs external envelope at `[tc + λ/1000 − half_win, tc + λ/1000 + half_win]`; take max.
3. Fine scan: ±3 ms around the coarse winner at 24 spb (0.5 ms steps), parabolic sub-bin.
4. Return `{lag_ms, ncc}`; return `None` when either window's envelope variance is ~0 (silence) or the parabolic fit is degenerate.

Variance gate and degenerate-fit details are implementation-defined within: "must return None rather than a junk number." Budget ≤ 5 ms per call (validated: window-to-window RMS 0.276 ms on the real pair).

### 5.5 Drift pass and least-squares fit (UX §4.3/§4.4)

1. Overlap `[T0, T1]` from `offset_stage` (drift 0 assumed). 12 window centers at `T0 + (k + 0.5)/12 · (T1 − T0)`, k = 0..11, clamped so windows stay inside both tracks.
2. Per window: `measure_local(t_k, 9 s, center = offset_stage, span = span_k)` with the [SPEC] scaling for long material (F6): `span_k = min(120 + 0.150·|t_k − t_mid|·1000·1e−3, 2000) ms` — i.e. base ±120 ms plus 150 ppm of the distance from the overlap midpoint, capped ±2 s. (At the validated pair's scale this is ≈ the mockup's behavior; at 2 h it covers ±150 ppm true drift.)
3. Accept windows with `ncc > NCC_WINDOW_MIN (0.4)` [VALIDATED].
4. If accepted ≥ 3: `fit_ls` over `(t_k, λ_k)` gives `off_ms` (intercept), slope ms/s, `ppm = slope·1000`, `rms_ms`. The fit **replaces** the stage offset: final mapping = `(off_ms, ppm)`.
5. Iteration [SPEC, SHOULD]: if `|ppm| > 20`, re-measure all 12 windows once with centers on the fitted mapping `L(t_k)` and span ±120 ms, then refit (recovers windows the first pass missed at long range).
6. If accepted < 3: drift "not measurable" (UX §3) — final mapping = `(offset_stage, 0)`, `drift_measurable = false`.

### 5.6 Status and suggestion heuristics (UX §4.9; all [TUNE])

- **Failed**: stage-1 best `z < Z_MIN (20)` (no statistically plausible peak — calibrated against the validated pair: true peak z = 75, spurious z = 12) or overlap < 20 s.
- **Synced**: `fine_ncc ≥ 0.5` AND accepted windows ≥ 6 AND `fit_rms_ms < 1.0`.
- **LowConfidence**: found but below any of those.
- **Drift-mode suggestion** [DECIDED form]: `end_to_end_drift_ms = |ppm| · overlap_s / 1000 > SUGGEST_RESAMPLE_MS (2.0)` suggests Resample, else None. The engine also reports the video frame duration in `SyncResult.overlap` context so a future frame-relative policy (§14, item 6.3) is a shell-side change only.

### 5.7 Onset ranking (UX §4.5 formalized; micro-parameters [TUNE])

From the 480 spb level, linear span domain `s_i` (0..254, not log): with 10 ms bins,

```
rise_i  = s_i − s_{i−2}                       // the "20 ms rise"
prec_i  = mean(s_{i−30} .. s_{i−3})           // preceding level, 280 ms window [SPEC default]
score_i = rise_i² / (prec_i + 0.15 · s_max)
```

Eligible: `rise_i > 0.10 · s_max`, `t_i` ≥ 1 s from either file edge. Non-max suppression: within 0.3 s keep the max score. Ranked list capped at 4000. Queries per §4.3. Validation property (from the UX provenance: picks were "envelope ~20–60 → ~180–240 of 254"): on the real pair, at least 2 of the 3 zone winners MUST satisfy `prec < 0.25·s_max` and local peak ≥ 0.6·s_max (§9.4).

### 5.8 Numerical notes

f64 throughout lag math; prefix sums in f64 (envelope values ≤ ln(255) ≈ 5.54, 36k bins — no precision risk); population variance; guard `σ < 1e−9` → NCC undefined → skip lag. The FFT path MUST produce bit-stable results across runs on the same machine (no nondeterministic reductions in the scoring path) so CI assertions are exact-repeatable.

---

## 6. Data formats and caches

### 6.1 Peak pyramid (in-memory and on-disk, binary)

Levels [DECIDED]: 24, 96, 480, 2400, 9600, 48000 samples/bin at 48 kHz (0.5 ms to 1 s). Values int8 −127..127 with per-track f64 `normalization` (UX §4.1). JSON interchange is explicitly retired (mockup scaffolding).

File `pyramid.cpk` [SPEC], little-endian, designed for mmap:

```
header (64 B):  magic "CPLPK1\0\0" | u32 version=1 | u32 flags | u32 sample_rate
                | u64 duration_samples | f64 normalization | u32 n_levels | pad
level table:    n_levels × { u32 spb, u32 pad, u64 n_bins, u64 byte_offset }   // offsets 64-B aligned
level data:     interleaved i8 pairs (min,max) per bin, finest level first
```

`duration_samples` includes the epoch lead (§7.2): bin `i` of level `spb` covers canonical time `[i, i+1)·spb/48000`.

### 6.2 PCM cache

`pcm48f32.raw`: headerless mono 48 kHz f32le on the canonical timeline (epoch lead materialized as zero samples). Sidecar `meta.json`: `{ version, contentKey, sampleRate, frames, epochS, sourceDurationS, normalization, stats, createdBy }`. Serves `raw_window`, audition, and re-import skips. Cost ≈ 0.69 GB per hour per track (accepted, F10).

### 6.3 Cache directory and lifecycle

- Location: `~/Library/Caches/Couple/v1/<contentKey>/{pyramid.cpk, pcm48f32.raw, onsets.json, meta.json}` (CLI: `--cache-dir`, default platform cache dir).
- `contentKey` = BLAKE3 of (first 8 MiB ‖ last 8 MiB ‖ u64 file size), hex-truncated to 128 bits [SPEC]. Path-independent, rename-proof, fast on 3 GB files.
- Preflight: import MUST verify free disk ≥ 1.5x the projected cache size and fail early with `DiskFull`.
- GC: LRU by directory atime/recorded lastUsed, default cap 20 GB [SPEC]; `cpl_cache_gc` / `couple cache gc`. Partial writes go to a temp name + atomic rename; `CacheCorrupt` evicts and re-imports.
- Onsets stored as JSON (`onsets.json`, ~100 KB) — small, debuggable, re-derivable.

---

## 7. Import pipeline

### 7.1 Probe and role classification

`ffprobe -v error -print_format json -show_format -show_streams`. Camera = has a video stream; external = audio-only. Two video files or two audio files: `AmbiguousRole` — shell runs the replace-confirmation flow (UX §3). Multiple audio streams in the camera file: take stream 0, record the rest in `MediaInfo` (v2 may expose a picker).

### 7.2 Decode (the epoch rule, normative)

- Command shape: `ffmpeg -nostdin -i <file> -map a:0 -af "pan=mono|c0=0.5*c0+0.5*c1" -ar 48000 -f f32le pipe:1` (pan filter generalized per channel count: equal-weight mean of all channels; mono in = passthrough).
- **Forbidden**: `aresample=async`, `first_pts` rewriting, `atempo`, or anything that resynthesizes timestamps — those would mask or distort the very drift and epoch this app measures (F1).
- Epoch: core reads the audio stream's `start_time` from the probe. Positive epoch ⇒ prepend `round(start_time · 48000)` zero samples to the cache/pyramid stream; negative (rare) ⇒ drop `round(−start_time · 48000)` samples. Plain audio files (external) have epoch 0 by definition; a nonzero epoch on a WAV is recorded but still applied. AAC priming/edit-lists are ffmpeg's job (pinned version, §3.3) — core trusts the decoder's output timeline.
- Single streaming pass tees the f32 stream into: PCM cache writer, pyramid builder (each finer level folds into the next coarser; O(N), constant memory), abs-peak/stats accumulators. Onset ranking runs from the finished 480 level. Progress from bytes consumed vs `duration_s · 48000 · 4`.
- Sample-rate normalization: any source rate is resampled to 48 kHz by ffmpeg (`-ar 48000`, default swr). Nominal-rate conversion preserves *relative clock drift* (the quantity we measure), so 44.1/96 kHz externals need no special handling in analysis. Export reads the original file, not this cache (§8).

### 7.3 Import acceptance (Phase 1 gates, also §9.2)

Real pair: camera track `duration_s` = 1435.252 ± 0.05 (epoch-inclusive), `epoch_s` = 0.1808 ± 0.001; external 1798.88 ± 0.01, epoch 0. Engine 48000-spb envelope vs `peaks/*.json` 48000 level: Pearson r ≥ 0.99 (camera compared after shifting the engine envelope by the epoch; the mockup extraction lacked it, §1.3).

---

## 8. Export pipeline

### 8.1 Options

```json
{ "kind": "mux" | "wav",
  "outputPath": "...",
  "keepCameraAudio": false,        // default off [DECIDED-pending §6.7 owner confirmation]
  "container": "auto",             // auto: follow input family (mov→mov, mp4/3gp→mp4); explicit "mov"|"mp4"
  "faststart": false,              // off by default: +faststart rewrites a 3 GB file [SPEC]
  "overwrite": false }
```

### 8.2 Aligned-audio render (shared by mux and WAV)

Output timeline = video time, `t ∈ [0, video_duration]`, exactly the video container duration. Source = the **original external file** (full fidelity — channels and bit depth preserved; the mono 48 kHz analysis cache is never used for export), decoded once, streamed.

- **driftMode "none"** (F8): integer-sample shift `S = round(offsetMs · sr_src / 1000)`; output frame `n` = source frame `n + S`; zero-fill outside `[0, src_frames)`. Samples are **bit-exact** ("audio untouched", UX §1.5); worst-case placement error half a sample (10.4 µs at 48 kHz).
- **driftMode "resample"**: output frame `n` = source evaluated at `u(n/sr_out) · sr_src` via a single-pass high-quality polyphase resampler at constant ratio `(1 + driftPpm·1e−6) · sr_src/sr_out` with the fractional offset folded into the initial phase. Quality bar [SPEC]: THD+N ≤ −110 dBFS in the passband, linear phase, no async correction (the resampler exists because the None mode protects quality — it must be transparent, UX §4.8). Candidates: rubato sinc (preferred, pure Rust/MIT) or soxr VHQ behind a feature flag; Session C benchmarks against this bar.
- Edge cases (enumerated; all MUST have fixture tests, §9.3):
  1. `offset < 0` (external starts after video): silence head until `t = −(offsetMs/1000)/(1 + driftPpm·1e−6)`.
  2. `offset > 0`: external content trimmed at the head (starts mid-file at `u(0)`).
  3. External ends before video: silence tail from `t` where `u(t) = ext_duration`.
  4. External longer than video: trimmed at `video_duration` (the 30-min WAV vs 23.9-min video case).
  5. Offset pushes overlap to zero: refuse with `ExportFailed{stage:"render", "no overlap"}` unless CLI `--allow-silence` (UI can't reach this state verified, UX §3).
  6. Mid-stream silence is never synthesized *inside* the external's span — gaps don't exist in continuous recordings; only head/tail pad.

### 8.3 Muxed video (primary deliverable)

```
ffmpeg -nostdin -i <video> -f s24le -ar <sr_src> -ac <ch_src> -i pipe:0
       -map 0:v:0 -map 1:a:0 [-map 0:a:0]
       -c:v copy -c:a:0 <codec> [-c:a:1 copy -disposition:a:1 0] -disposition:a:0 default
       -map_metadata 0 [-movflags +faststart] -f <mov|mp4> <tmp> && atomic rename
```

- Video stream copied untouched [DECIDED] — VFR timestamps preserved by stream copy; no fps math anywhere.
- Rendered audio piped in at source rate/channels/depth (f32le pipe when the render is float).
- `keepCameraAudio`: original audio track stream-copied (no re-encode) as track 2, non-default disposition. Note: player handling of disabled tracks varies; the contract is "present, not default."
- Metadata: global tags copied (`-map_metadata 0`) — preserves `creation_time` (useful for v2 pairing). A `tmcd` timecode track, if present, is copied on a best-effort, non-fatal basis.
- Temp file + atomic rename; cancel deletes the temp. Disk preflight: video size + projected audio + 5%.
- Progress: ExportRender from audio frames rendered; ExportMux from ffmpeg `-progress pipe:2` `out_time` vs duration.

### 8.4 Container/codec matrix [SPEC, F9; §6.7 defaults pending owner]

| Input family | Output container | Audio codec (track 1) | Notes |
|---|---|---|---|
| mov | mov | PCM (`pcm_s24le`; `pcm_s16le` if source ≤ 16-bit) | "PCM where the container allows" (UX §2.8) |
| mp4 / m4a / 3gp | mp4 | AAC-LC 256 kbps — `aac_at` (AudioToolbox) on macOS, ffmpeg `aac` elsewhere | PCM-in-MP4 is non-portable; matches camera-side bitrate |
| anything else ffmpeg demuxes | mov | PCM s24le | best-effort; v1 officially supports mp4/mov/wav |

No user-visible codec choice in v1 (UX §2.8: "one visible choice at most" — we ship zero; the one visible option is `keepCameraAudio`).

### 8.5 Standalone WAV export

External rendered to the video timeline (§8.2 identically), at **source sample rate**, source bit depth (float sources → 24-bit), source channel count; duration exactly the video duration. Written via the ffmpeg wav muxer with `-rf64 auto` (a 2 h stereo 24-bit 96 kHz render exceeds 4 GiB). Offered as the secondary export per UX §2.8; default prominence is an owner call (§14, item 6.7).

---

## 9. Test strategy

### 9.1 Synthetic fixture generator (`couple fixture gen`, dev subcommand)

Deterministic (ChaCha8 seed). Internal master signal at 96 kHz f64: pink-noise bed at −38 dBFS + Poisson-spaced transient events (~0.5/s; band-passed attacks, exponential decay τ 80–400 ms) + occasional sustained vibrato tones 2–6 s (the periodic-content aliasing hazard that motivates transient-only anchors) + multi-second silences (exercises onset zoning and the active-bin stats).

From the master, with ground truth `(offset_ms, ppm)` defining `u(t)`:

- **External** = master sampled at `rate_ext` (44.1/48/96 kHz), 16/24-bit WAV, reference gain.
- **Camera audio** = master warped through `u(t)`, then degraded: gain delta (±24 dB range), AGC simulation (feedback limiter, 5 ms attack / 200 ms release, threshold −12 dBFS), 16 kHz low-pass, additive noise to target SNR, optional AAC 256k round-trip, optionally muxed into an mp4 with `mpeg4`-encoded testsrc2 video (F12) and **one variant authored with an audio `start_time` offset via `-itsoffset 0.18 -c copy` remux** — this fixture locks the F1 epoch rule forever.
- Parameters: durations (20 s – 2 h), offset up to ±full duration, ppm 0–100, SNR, gain, overlap down to 20 s, rates, seed. Sidecar `truth.json` per pair.

### 9.2 Phase-1 gates (import correctness)

§7.3 real-pair assertions + pyramid round-trip property tests (random signals: band() equals brute-force min/max over any range/level) + epoch fixture (decoded cache leads with exactly `round(0.18·48000)` zeros).

### 9.3 Accuracy tolerances (synthetic; assert in CI)

Conditions unless stated: SNR ≥ 10 dB, gain delta ≤ 24 dB, AGC on, overlap ≥ 60 s.

| Quantity | Condition | Tolerance |
|---|---|---|
| Offset (ppm = 0) | — | abs error ≤ 0.5 ms |
| Offset (ppm ≤ 100) | drift measurable | mapping error at overlap midpoint ≤ 1.0 ms |
| Drift | overlap ≥ 1800 s | ≤ ±0.2 ppm |
| Drift | overlap ≥ 600 s | ≤ ±0.5 ppm |
| Drift | overlap ≥ 180 s | ≤ ±2 ppm |
| Drift | overlap ≥ 60 s | ≤ ±5 ppm |
| Drift | overlap < 60 s | may report not-measurable; MUST NOT report a confident wrong value |
| Local lag (per accepted window, ncc ≥ 0.6) | — | RMS ≤ 0.3 ms |
| Corrected end-to-end residual | resample mode, any fixture above | ≤ 2 ms at every measured window |
| Partial overlap | 20 s ≤ overlap < 60 s | offset ≤ 2 ms; below 20 s MUST fail (InsufficientOverlap), never a confident result |
| Hostile noise | SNR 0 dB | result is correct, LowConfidence, or Failed — never confident-and-wrong (z-gate test) |
| Nominal-rate mix | 44.1 ↔ 48, 96 ↔ 48 | same rows as above apply unchanged |

Rationale: the validated mockup achieved 0.276 ms local-lag RMS and ±3 ppm-scale drift on real, AGC-mangled material; synthetic tolerances are set ~2x looser than demonstrated performance, and the drift ladder follows the statistics of fitting 12 windows of ~0.15–0.3 ms sigma over the available baseline. The 2-hour rows are the reason for the F6 scaling work.

A regression case reproducing the **spurious-peak finding** (UX §4.2) MUST exist: a short-overlap decoy constructed so raw-NCC argmax picks a boundary lag while z-selection picks truth; assert z wins (and on the real pair, assert the chosen overlap is ≈ 99.6% of video, not ~42 s).

### 9.4 Real-data validation (CI, against example_data/)

Runs via CLI (`couple sync … --json | jq`), feature-gated `realdata` (files are 3.6 GB and not in git): enabled on dev machines and the designated runner via `COUPLE_REALDATA_DIR`; GitHub-hosted CI runs synthetic-only.

- Epoch-invariant assertions from day one: `drift_ppm = 3.07 ± 0.75`; `fit_rms_ms ≤ 0.7`; accepted windows ≥ 8 of 12; status Synced; `suggested_drift_mode = resample` (end-to-end ≈ 4.4 ms > 2 ms); overlap fraction ≥ 0.99; onset zone property (§5.7).
- **Offset freeze procedure (F2)**: first Phase-3 run reports container-time offset; sanity window: within −5535.5 ± 25 ms (the §1.3 prediction). The owner (or Session C via loupe/audition once available) verifies once; the measured value is then **frozen** into `example_data/expected.json` and asserted at ± 1 ms thereafter. Drift/RMS asserted unconditionally from the start.
- Performance assertions per §9.6 on the designated runner only.

### 9.5 Hand-sync regression (dormant until material exists, F4)

Protocol: owner drops hand-synced exports as `example_data/handsync/<name>/{export file}` (+ optional `note.txt`). Test: import the export's audio track, run `measure_local` at 5 spread points between engine-aligned external audio and the hand-synced track; assert median |Δ| ≤ 10 ms (hand syncs are sanity references, not sub-ms ground truth). Skips with a visible notice when the directory is empty.

### 9.6 Performance budgets (Apple Silicon, release build) [SPEC]

| Operation | Budget | Target |
|---|---|---|
| Import 2 h file (decode + pyramid + cache) | ≤ 90 s | 45 s |
| Auto-sync 2 h x 2 h | ≤ 10 s | 3 s |
| Stage 1 alone, 2 h x 2 h | ≤ 3 s | — |
| `measure_local` (18 s window) | ≤ 5 ms | 2 ms |
| `band` (any range, ≤ 2048 cols) | ≤ 50 µs typical | — |
| `audition_fill` 512 frames | ≤ 1 ms, allocation-free | — |
| Export 2 h (excl. faststart) | ≤ 90 s + stream-copy I/O | — |
| Core RAM, 2 h pair loaded | ≤ 600 MB (pyramids + scratch; PCM stays mmap'd) | — |

### 9.7 Other suites

- **Round-trip end-to-end** (Phase 4 keystone): export a drifted fixture with resample mode, re-import the result against the original external, auto-sync → measured offset ≤ 0.25 ms, drift ≤ 0.3 ppm (the muxed file is *in* sync).
- Bit-exactness: None-mode mux of a PCM-in-MOV fixture → extracted samples byte-identical over the overlap.
- Cancellation: every job type cancelled at random progress points; no temp/cache litter; `Cancelled` surfaced.
- ABI: `couple.h` diffed in CI; JSON schemas round-trip serde ↔ documented examples.
- Determinism: same inputs → bit-identical SyncResult on the same machine (twice in CI).

---

## 10. CLI specification (`couple`)

The CLI is the porting artifact and test harness; every subcommand is a thin veneer over the core API.

### 10.1 Global flags

`--json` (machine output: single JSON object on stdout, schema = §4 wire types), `--cache-dir <dir>`, `--ffmpeg <path>`, `--quiet`, `--progress-json` (NDJSON `{phase, fraction}` on stderr; default is human progress bars on a TTY).

### 10.2 Subcommands

```
couple probe <file>
couple import <file> --role camera|external|auto
couple peaks <file> --t0 --t1 --columns N [--png out.png]      # band() dump; PNG for eyeballing
couple onsets <file> [--in t0 t1 | --near t --span 4] [--limit N]
couple sync <video> <audio> [--offset-only] [--params k=v…] [--write-state state.json]
couple measure <video> <audio> --at t [--center ms --span ms --halfwin s]
couple fit --points points.json                                 # exposes fit_ls for tooling
couple render <video> <audio> --state state.json --out out.wav  # §8.5 WAV
couple export <video> <audio> --state state.json --out out.mov
       [--keep-camera-audio] [--container auto|mov|mp4] [--faststart] [--allow-silence] [-f]
couple state validate <state.json>
couple fixture gen --out dir/ --offset-ms X --ppm Y --snr-db Z --overlap-s W
       [--dur-cam --dur-ext --rate-cam --rate-ext --bits --agc --aac-roundtrip --with-video --itsoffset] --seed N
couple cache ls | gc [--max-gb 20]
```

`couple sync` runs the full §5 pipeline (import implied, cache-hit fast) and prints `SyncResult`; `--write-state` emits a session file (§4.5 wrapper) consumable by `render`/`export` — the complete headless workflow, which is also the v2 batch-export substrate.

### 10.3 Exit codes

0 success · 2 usage · 3 probe/unsupported · 4 decode/import · 5 sync failed (NoPlausiblePeak) · 6 insufficient overlap · 7 export · 8 cancelled (SIGINT honors job cancellation, cleans temps) · 9 internal. With `--json`, errors print `{ "error": { "kind", "detail" } }` on stdout and the code still applies.

---

## 11. macOS shell (SwiftUI) — engineering spec

UX-DECISIONS §1–§3 is the behavioral spec; this section covers structure and the technically risky parts. Visual styling is deferred (UX §5).

### 11.1 Structure

- `CoupleKit`: C-ABI wrapper; jobs as `async` functions yielding `AsyncStream<Progress>`; value types decoded from the wire JSON; `AlignmentState` shadow + setters passthrough (core owns canon, §4.5).
- App model: `PairSession` (observable; identity-stable; deliberately list-ready for v2 — "+ add pair" replaces the single element of a one-element collection).
- View tree mirrors UX §1.2's four bands: `TitleBar`, `WorkflowStrip { InputPanel, SyncPanel, ExportPanel }`, `WaveformStage { OverviewStrip, ConnectorLayer, LoupeRow }`, `Footer`. Resize policy per UX §1.2 (stage is the only greedy region; min window ~1000x560 then scroll).

### 11.2 Waveform rendering

SwiftUI `Canvas` first: per frame, each visible surface issues one `band()` (overview: 2 lanes; each loupe: 2 tracks) into preallocated buffers — ~10 calls x ≤ 50 µs is well inside a 60 fps budget; stepped per-bin drawing [DECIDED], additive blending in loupes, per-loupe local normalization (each track's in-window peak to 86% half-height [DECIDED]). Loupes switch to `raw_window` below 120 ms windows [SPEC threshold, tune visually]. If profiling shows Canvas can't hold 60 fps during drags, drop to CALayer/Metal for the stage only — an internal change, no contract impact. Overview gain: `1.44 / active_p95_9600` (UX §4.1).

### 11.3 Interaction wiring (all behavior per UX §2)

Gestures/keys mutate state via `set_offset_ms`/`set_drift_ppm_live` (+ `commit_drift` on release/commit); the shell recomputes displayed Δs against cached measurements every frame, and re-runs `measure_local` per anchor on the 450 ms / >30 ms debounce (UX §2.4). Pivot-drift drag math is shell-side (UX §2.4 formula) but calls core's `measure_local` for evidence; `fit_ls` is available for the sparkline line. Auto-sync triggers on load completion (UX §2.5), shows SyncCoarse progress percentage, Esc cancels. Anchor auto-proposal zones and snap use `best_onset_*`. Keyboard map = UX §2.9 verbatim; **`j`, `k`, and `Enter` (outside text fields) MUST be reserved no-ops in v1** (v2 queue triage).

### 11.4 Playback (shell-side, per UX §4.7; flag F11)

- Audio: `AVAudioEngine` + `AVAudioSourceNode` whose render callback calls `cpl_audition_fill` directly (C closure, no Swift allocation on the render thread). 48 kHz stereo; device sample-rate conversion left to the engine.
- Video: muted `AVPlayer` on the original file. Audio is the master clock: on play, seek player to `t`, start both with a shared `hostTime` (`AVAudioTime` ↔ `CMClock` correlation); monitor skew at 1 Hz; |skew| > 20 ms → nudge `player.rate` (0.98/1.02) until converged, > 200 ms → reseek. Acceptance: sustained |skew| ≤ 1 frame (33 ms). If AVPlayer rate-nudging proves unstable, fallback is AVSampleBufferDisplayLayer fed by a decode session — out of v1 scope unless the acceptance test fails.
- Monitor mode switch mid-playback = `audition_set_mode` (click-free via the §4.8 crossfade). Jump buttons seek to anchor − 0.8 s (UX §1.4). Playheads from `FillInfo.stream_t`.

### 11.5 Files, persistence, packaging

- Drag-drop + NSOpenPanel into the two slots; role from `probe` (AmbiguousRole → replace-confirmation per UX §3). Security-scoped bookmarks retained for the session.
- Autosave (§14, item 6.8): on every verified-toggle, export, and clean quit, write the session file (§4.5 wrapper) to Application Support keyed by pair contentKeys; on reopening the same pair, offer restore. No document type in v1. "1 of N" chip resets each launch (item 6.11).
- Unsaved-changes confirmation on "+ add pair"/replace per UX §1.3/§2.6.
- Packaging: hardened-runtime app, ffmpeg/ffprobe helpers signed in `Contents/Helpers/`, notarized DMG. App Store is kept *possible* (sandbox-compatible helper invocation, LGPL-clean subprocess model) but is not a v1 deliverable.

---

## 12. v2 forward-compatibility (spec'd so v1 doesn't paint into a corner; NOT v1 scope)

### 12.1 What v1 already does to stay compatible (binding on Sessions C/D)

- Core: `Ctx` holds N independent tracks; a `Session` is a (camera, external) *view* over two tracks — nothing assumes one global pair. Caches are content-addressed (multi-take sessions reuse imports). Probe captures creation_time/BWF/device tags (§4.2). The CLI `sync → state → export` chain is already the headless batch unit.
- Shell: `PairSession` list-ready (§11.1); `j`/`k`/`Enter` reserved (§11.3); "1 of N" chip exists (UX §2.6).
- Names reserved: CLI `couple pair <dir>`; core `pair_candidates`.

### 12.2 Pairing algorithm requirements (informative, grounded in §1.4)

Given a dropped folder: probe everything; classify video vs audio. **Primary signal**: stage-1 z (§5.2/5.3, 9600 spb) for every video x audio candidate — envelopes exist after import, each pair costs milliseconds, so all-pairs is cheap for the target 3–6 takes (UX §2.6). Assign by best-z matching with a z-floor (Z_MIN); a candidate beating the floor yields the pairing *and* a warm-start offset. **Secondary signals** (ordering/tiebreak only, never primary — F3): same-device timestamp monotonicity (a wrong clock still runs), duration plausibility, filename sequence numbers. Per-row confidence = z + accepted-window count, feeding the §2.6 status chips. The v1 `SyncResult` already carries everything the queue's chips need.

### 12.3 Known v1→v2 deltas (flagged now, deliberately cheap later)

- Core adds `pair_candidates(tracks…) → matrix of {z, lag}`; no v1 API changes required.
- Shell grows a left-rail list + triage keys; editor unchanged.
- Persistence: the §4.5 session file becomes an array (sessions of pairs) — version field exists for exactly this.
- Batch export = a job queue over the existing export job; progress conventions already per-job.

---

## 13. Porting contract (Windows / Linux shell)

### 13.1 What a port consumes

The C ABI in `couple.h` (§4.9) — identical core, CLI, formats, and caches on all platforms (paths/cache-dir differ). Alternatively, everything except real-time audition is reachable through the CLI's `--json`/`--progress-json` surface (a legitimate bring-up path; §13.3).

### 13.2 What a port must implement (the shell duties)

| Duty | macOS reference | Port equivalent |
|---|---|---|
| Audio output pulling `cpl_audition_fill` (48 kHz stereo f32, 5–10 ms buffers) | AVAudioEngine source node | WASAPI shared-mode / PipeWire-PulseAudio callback |
| Muted video player, seekable, rate-adjustable, slaved to audio clock (≤ 1 frame skew) | AVPlayer | Media Foundation / GStreamer or libmpv |
| File pickers + drag-drop yielding two paths; replace-confirmation flow | NSOpenPanel/drop | native |
| Waveform stage: `band`/`raw_window` at 60 fps, UX §1.6 semantics | SwiftUI Canvas | any GPU/canvas stack |
| Keyboard map UX §2.9, with j/k/Enter reserved | §11.3 | same map, platform modifiers (Alt for ⌥, Shift) |
| Job progress UI + cancel; toasts | §11 | native idiom |
| Autosave of session JSON (§4.5) | App Support | platform config dir |
| Bundled LGPL ffmpeg/ffprobe + `COUPLE_FFMPEG` override | Contents/Helpers | alongside executable |

### 13.3 Specified vs shell-idiomatic

**Specified (must match)**: all §4 contracts and JSON schemas; mapping math and sign conventions; UX §2 interaction semantics (nudge steps, pivot math, zoom lock, auto-frame rules); monitor-mode matrices; export behavior; reserved keys. **Shell-idiomatic (free)**: window chrome, dialogs, fonts/exact colors (semantic color *mapping* is fixed: camera teal, external amber, anchor palette), toasts, menus, settings storage. **Degraded audition mode (MAY, CLI-only ports)**: render the audition region to a temp WAV via `couple render` and play it with any system player — acceptable for bring-up, not for the polished port.

---

## 14. Dispositions of UX-DECISIONS §6 open questions

| Item | Disposition |
|---|---|
| 6.1 Name | Couple [DECIDED]. Binary `couple`, bundle id `com.timbeccue.couple` [SPEC]. |
| 6.2 Transient lamps | **Owner decision, unblocked either way**: §4.8 keeps `envelope_at`/onset queries (near-zero cost), so lamps are purely a shell choice after real playback lands (Phase 6). |
| 6.3 Drift-suggest threshold | Form is DECIDED (measured end-to-end > 2 ms). Value stays 2 ms [TUNE]; engine reports both end-to-end ms and frame duration so a frame-relative policy is a shell-side tweak. **Owner decides post-corpus.** |
| 6.4 Status thresholds | [TUNE] in Session C as the corpus grows (§5.6 constants). |
| 6.5 Sub-ms zoom | **Resolved by this spec (F10): raw-sample fetch** from the import-time PCM cache; pyramid keeps its 24-spb floor. Finer-than-8-spb resolution (one sample = 20.8 µs), zero RAM cost, ~0.69 GB/h/track disk, and the same cache powers audition. The 8-spb alternative would cost ~86 MB RAM per 2 h track and still bottom out at 167 µs. |
| 6.6 Pivot reference | Owner decision; core is indifferent (pivot math is shell-side; `measure_local` takes explicit times), so changing it later touches only UI. v1 ships anchor-1 rule per UX §2.4. |
| 6.7 Export defaults | Owner decision. Spec proposals on the table: keep-camera-audio **off**; codec matrix §8.4; WAV export always visible as the secondary action; faststart off. |
| 6.8 Persistence | **Recommendation: no document type in v1.** Freeze the serialization now (versioned AlignmentState + session wrapper, §4.5), autosave to Application Support keyed by content, offer restore on re-open of the same pair. A real document model (and "recent pairs" UI) arrives with the v2 queue, which needs sessions anyway. |
| 6.9 Anchor cap | **Confirm 4 for v1.** Evidence: drift quality comes from the 12-window pass, not anchor count; 4 side-by-side loupes at the 1000 px minimum window already hit ~240 px each (the legibility floor); state schema imposes no limit, so raising later is a constant change. |
| 6.10 Nudge steps | Ship as decided-in-testing (1/10/0.1 ms; 1/0.1 ppm); ratify during Phase 6 manual script. |
| 6.11 "1 of N" persistence | Resets each launch in v1 (consistent with the 6.8 recommendation). |
| 6.12 Auto-sync on load | DECIDED yes; wired in §3.7/§11.3. |

---

## 15. Milestones (core before UI; each phase independently verifiable via CLI)

**Phase 1 — Decode, pyramid, inspection.** couple-core + couple-cli scaffolds; ffmpeg/ffprobe discovery; probe; import job (epoch rule §7.2, streaming pyramid §7.2, PCM cache, stats, onsets); `band`/`raw_window`/`rms`/onset queries; binary cache format + GC; CLI `probe|import|peaks|onsets|cache`.
*Verify:* §9.2 gates — real-pair durations/epoch, r ≥ 0.99 vs `peaks/*.json`, pyramid property tests, epoch fixture, import budget; `couple peaks --png` eyeball against `peaks/*_overview.png`.

**Phase 2 — Global offset search.** Envelope NCC machinery (prefix sums + FFT, §5.2); stage pipeline §5.3 with z-selection and parabolic refinement; fixture generator (offset-only paths); `couple sync --offset-only`; progress + cancel.
*Verify:* §9.3 offset rows incl. partial-overlap and hostile-noise rows; spurious-peak regression; real pair offset lands in the −5535.5 ± 25 ms sanity window; stage-1 ≤ 3 s budget on 2 h fixtures.

**Phase 3 — Drift + full auto-sync.** `measure_local` (§5.4); drift pass + fit + iteration (§5.5); onset ranking (§5.7) feeding anchor queries; status/suggestion heuristics; full `couple sync` + `--write-state`; session state + setters.
*Verify:* §9.3 drift/local-lag/corrected-residual rows; full real-data suite §9.4 incl. **offset freeze**; `measure_local` ≤ 5 ms; determinism check.

**Phase 4 — Export.** Aligned render (None bit-exact path + resample path with quality benchmark, §8.2); mux (§8.3/8.4); WAV/RF64 (§8.5); edge-case fixtures 1–6; cancel/atomicity.
*Verify:* §9.7 round-trip (≤ 0.25 ms), bit-exactness, second-track presence, ffprobe conformance of outputs, cancellation litter check. **Core complete: the manual iMovie workflow is fully replaced headlessly.**

**Phase 5 — Shell: stage + loupes.** CoupleKit; window bands + resize policy; load surface + import progress; auto-sync-on-load; overview strip, pins/snap, connectors, loupes (band/raw, local normalization, additive blend, guidance arrows); zoom lock + auto-frame; offset/drift gestures incl. pivot drag; readouts + sparkline; keyboard map with reservations.
*Verify:* §16 Session-D checklist — manual walkthrough on the real pair against UX §1–§2 + 60 fps drag profiling + A/V of §11.2 thresholds.

**Phase 6 — Shell: full workflow.** Audition (3 modes, §11.4 clock slaving, jump buttons, playheads); video preview; export panel with Verified gate + drift-mode control semantics (UX §1.5); toasts; states of UX §3; autosave/restore; packaging (signed helpers, notarized DMG).
*Verify:* end-to-end manual script (load → inspect → audition incl. null-dip → verify → export → play result in QuickTime); A/V skew ≤ 1 frame test; export of the real pair matches Phase-4 CLI output hash.

**Phase 7 — v2 queue.** Separate session; §12 is its charter. Not sequenced here.

---

## 16. Kickoff prompts for the next sessions

### Session C (core + CLI, Phases 1–3)

Scope widened from "Phase 1 only" by owner decision on 2026-06-11 (see PROGRESS.md):
Session C is test-driven against synthetic fixtures with known offset/drift and ends
with the engine validated on the real recordings via the CLI. Phase 4 (export) becomes
a short follow-up session before Session D.

```
You are implementing the core sync engine of Couple — Phases 1–3 of SPEC.md §15:
test-driven against synthetic fixtures with known offset/drift, ending with the
engine validated against the real recordings in example_data/ via the CLI.

Read first, in this order, before writing any code:
  1. SPEC.md — all of it once. Binding for this session: §0 (precedence + the
     SPEC-CHANGE protocol), §1 (ground truth, especially the epoch rule §1.2),
     §3 (architecture), §4 (core/CLI contract), §5 (sync algorithms — validated
     parameters, preserve exactly), §6 (formats/caches), §7 (import), §9.1–9.4 +
     §9.6 (fixtures, tolerances, real-data validation, budgets), §10 (CLI),
     §15 (phase gates).
  2. PROGRESS.md — current status and SPEC-CHANGE entries.
  3. UX-DECISIONS.md §4 — the validated engine requirements behind SPEC §5.

Scope, strictly in order; each phase's §15 verification gates must be green before
the next phase starts, with a git commit and a dated PROGRESS.md entry at every
phase boundary:
  - Phase 1: workspace scaffold (core/, cli/, ffi/), probe, import job with
    progress + cancel and the §7.2 epoch rule, peak pyramid + PCM cache + stats +
    onsets, band/raw_window/rms/onset queries, cache GC, CLI
    probe/import/peaks/onsets/cache. Gates: §9.2.
  - Phase 2: the fixture generator (§9.1) and its truth-table tests FIRST, then
    the global offset search (§5.2–5.3: FFT numerator, z-score selection,
    parabolic refinement), `couple sync --offset-only`, progress + cancel.
    Gates: §9.3 offset, partial-overlap and hostile-noise rows; the spurious-peak
    regression; the stage-1 budget on a 2 h fixture.
  - Phase 3: measure_local (§5.4), drift pass + least-squares fit + iteration
    (§5.5), onset ranking (§5.7), status/suggestion heuristics (§5.6), session
    state + setters (§4.5), full `couple sync` + `--write-state`. Gates: §9.3
    drift, local-lag and corrected-residual rows; measure_local ≤ 5 ms;
    determinism check.
Out of scope — do not start: Phase 4 (export), anything Swift/shell, FFI work
beyond generating couple.h via cbindgen.

Setup before coding: verify cargo/rustc exist (install Rust via rustup if
missing); `git init` with a .gitignore covering example_data/ (except SCHEMA.md
and the to-be-created expected.json), fixture output, target/, caches, .env.
Local commits only; if asked to push, GitHub account timbeccue.

Hard rules:
  - The canonical-timeline/epoch rule (§1.2/§7.2) is the #1 correctness trap: the
    real camera file's audio starts at container time 0.180792 s and §9.2 asserts it.
  - Never use timestamp-resynthesizing ffmpeg options (§7.2 forbidden list) — they
    destroy the very drift and epoch this engine measures.
  - ffmpeg/ffprobe as subprocesses only (§3.3); the system binaries (8.0.x) are
    fine for dev, honoring COUPLE_FFMPEG; the pinned LGPL build is packaging work.
  - Crates MIT/Apache-2.0 only (§3.6); all lag math in f64; constants from
    Appendix A, overridable via SyncParams.
  - Budgets (§9.6) asserted on this machine for import, stage 1, full auto-sync,
    and measure_local; run at least one 2-hour synthetic pair locally.
  - Work the phases back-to-back without waiting for approval. Stop and surface
    only if a gate cannot be made green or the spec is wrong — in that case record
    a SPEC-CHANGE entry in PROGRESS.md (protocol in SPEC §0); never silently
    diverge.

Finish line — real-data validation via CLI (§9.4), with COUPLE_REALDATA_DIR set
to example_data/:
  couple sync example_data/schumann-video.mp4 example_data/schumann-audio.wav --json
  - Assert: drift 3.07 ± 0.75 ppm; fit RMS ≤ 0.7 ms; ≥ 8 of 12 windows accepted;
    status synced; suggested mode resample; overlap ≥ 99% of video; offset inside
    the −5535.5 ± 25 ms container-time sanity window (§1.3).
  - If the offset lands in the window, execute the §9.4 freeze: write it to
    example_data/expected.json with ownerVerified=false, add the ±1 ms regression
    test, and flag the value for the owner to confirm by ear once the shell exists.
  - Report the full SyncResult JSON in your final summary.
```

### Session D (SwiftUI shell, Phase 5)

```
You are implementing Phase 5 of Couple — the SwiftUI shell's waveform stage and
loupes. Core Phases 1–4 are complete: couple-core passes its test suite and the
`couple` CLI can probe/import/sync/render/export end to end. Before writing any
code, read in this order:
  1. UX-DECISIONS.md — §1, §2, §3 fully (it is the authoritative UX description;
     every DECIDED item is binding), plus §4.6/§4.7 for the state and playback
     boundaries.
  2. SPEC.md — §4 (core/shell contract; you consume the C ABI via a new CoupleKit
     wrapper), §11 (shell engineering spec), §15 Phase 5 (scope + verification).
  3. PROGRESS.md — status and SPEC-CHANGE entries from the core sessions.

Scope: Phase 5 ONLY — CoupleKit FFI wrapper, the four-band window with the §1.2
resize policy, empty/launch + import-progress states, auto-sync on load with
progress and cancel (UX §2.5/§3), overview strip with anchor pins/snap and
connector layer, 2–4 loupes with per-loupe local normalization, additive blending,
stepped rendering, guidance arrows, zoom lock and auto-framing, all offset/drift
gestures and keyboard nudges from UX §2.9 (j/k/Enter reserved as no-ops), readouts,
and the drift sparkline. Audition, video preview, export UI, and toasts are
Phase 6 — leave honest placeholders.

Hard rules:
  - Shell owns rendering and (later) the audio device; core owns alignment math and
    state (SPEC §4.5 ownership rule). Mutate state only through the setters.
  - 60 fps during loupe drags on the real example_data pair is an acceptance
    criterion (SPEC §11.2); profile before reaching for Metal.
  - Use the real pair for manual verification and walk the §15 Phase-5 checklist;
    capture screenshots mirroring the [SCREENSHOT] markers in UX-DECISIONS §1.
Propose spec changes via SPEC-CHANGE entries in PROGRESS.md rather than diverging.
When done: checklist results + screenshots referenced in a dated Phase-5 entry in
PROGRESS.md.
```

---

## Appendix A — Constants

| Constant | Value | Tag | Where |
|---|---|---|---|
| Analysis sample rate | 48 000 Hz mono f32 | DECIDED | §7.2 |
| Pyramid levels (spb) | 24, 96, 480, 2400, 9600, 48000 | DECIDED | §6.1 |
| Sub-ms zoom source | raw PCM cache; RAW_FETCH_MAX_S = 8 s; loupe switch < 120 ms window | SPEC | §4.3, §11.2 |
| MIN_OVERLAP_S / _BINS | 20 s / 100 bins @ 9600 | DECIDED | §5.3 |
| Stage spans | ±full, ±500, ±80, ±14, ±3 ms | VALIDATED | §5.3 |
| Parabolic clamp | abs(δ) ≤ 0.6 bin | SPEC | §5.3 |
| Z_MIN (failed threshold) | 20 | TUNE | §5.6 |
| NCC_WINDOW_MIN (drift accept) | 0.4 | VALIDATED | §5.5 |
| Synced gate | fine NCC ≥ 0.5, windows ≥ 6, RMS < 1 ms | TUNE | §5.6 |
| Local-lag window | half_win 9 s (18 s window) | VALIDATED | §5.4 |
| Anchor measure span | ±80 ms | VALIDATED | UX §4.3 |
| Drift windows | 12; centers (k+0.5)/12 of overlap | VALIDATED | §5.5 |
| Drift span scaling | ±(120 ms + 150 ppm·distance-from-mid), cap ±2 s; refit iteration when ppm > 20 | SPEC | §5.5 |
| SUGGEST_RESAMPLE_MS | 2.0 ms end-to-end | DECIDED form, TUNE value | §5.6 |
| Onset params | rise 2 bins @480; prec window 280 ms; floor 0.15·max; rise gate 10%·max; NMS 0.3 s; edge skip 1 s; cap 4000 | VALIDATED + TUNE (prec window SPEC) | §5.7 |
| Anchor zones / separation / cap | [1,30] [38,62] [70,99] % of overlap; ≥ 8%; 2–4 anchors | DECIDED (cap confirmed) | UX §2.2, §14 |
| Snap search | ±4 s | DECIDED | UX §2.2 |
| Loupe defaults | 240 ms window; auto-frame max(240, 5·abs(Δ)) round 10 ms cap 2 s | DECIDED | UX §2.3 |
| Δ color thresholds | green < 0.7 ms, amber < 3 ms | DECIDED | UX §1.6 |
| Re-measure debounce | 450 ms, after mapping moves > 30 ms | DECIDED | UX §2.4 |
| Nudge steps | 1 / 10 / 0.1 ms; 1 / 0.1 ppm | DECIDED (ratify §6.10) | UX §2.9 |
| Overview gain | 1.44 / p95(active bins @9600, > 12% max) | VALIDATED | §4.2 |
| Loudness target | −20 dBFS active-RMS, gain clamp +24 dB | SPEC | §4.8 |
| Audition | 32-tap Kaiser sinc; 240-sample crossfade | SPEC | §4.8 |
| Export resampler bar | THD+N ≤ −110 dBFS, linear phase | SPEC | §8.2 |
| AAC bitrate | 256 kbps | SPEC | §8.4 |
| Cache | BLAKE3(first 8 MiB ‖ last 8 MiB ‖ size) → 128-bit key; GC cap 20 GB | SPEC | §6.3 |
| Progress / cancel | ≤ 30 Hz; cancel observed ≤ 100 ms | SPEC | §4.6 |
| Budgets | see §9.6 table | SPEC | §9.6 |

## Appendix B — example_data probe summary (2026-06-11, ffprobe 8.0.1)

```
schumann-video.mp4   QuickTime/MP4 (mp42), 1435.296 s, 3 096 771 561 B, creation 2022-12-10T02:30:01Z,
                     com.android.version=13, location zeroed
  v:0  h264 High@4.0 1920x1080 yuv420p bt709, start 0.000, 17.0 Mbps, 43068 frames,
       nominal 30 fps, avg 30.0064 fps (VFR)
  a:0  aac LC 48000 Hz stereo 256 kbps, start_pts 8678 → start_time 0.180792 s,
       stream duration 1435.0709 s
schumann-audio.wav   WAV pcm_s24le 48000 Hz stereo, 1798.880 s, 518 078 534 B
  bext: encoded_by "ZOOM Handy Recorder H4n", date 2011-04-20 (clock never set — 11.6 y wrong),
        creation_time 06:34:12, time_reference 1135296000 samples (= 06:34:12.000)
peaks/               mockup-era extraction: camera.json duration 1435.0933 s (stream-relative,
                     epoch lead absent → §1.3), external.json 1798.88 s; SCHEMA.md documents the
                     retired JSON format; *_overview.png + sanity_compare.png eyeball references
Validated mockup result for this pair (stream-relative): offset −5354.74 ms, drift +3.07 ppm,
fit RMS 0.276 ms over 12 windows; predicted container-time offset −5535.5 ± 25 ms (§1.3, §9.4).
No hand-synced exports present (§9.5 dormant).
```

---

## PROGRESS.md stub (authoritative copy lives in PROGRESS.md; append there, not here)

> ## 2026-06-11 — Planning
> Status: planning complete. SPEC.md written. No implementation yet.

*End of SPEC.md.*
