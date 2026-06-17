# PLAN — Core / Shell Boundary Refactor (cross-platform)

Status: PROPOSED · Owner: Tim · Target: a single Rust core that is the sole authority for
all decision logic, with thin per-platform shells (macOS today; Windows + Linux next).

## 0. Goal & motivation

We want to ship the app on Windows and Linux while keeping **one** copy of the engine and
**one** copy of the decision logic. Today the engine (`couple-core`) is already clean and
portable, but a second layer — *pairing resolution and several interaction policies* — is
either duplicated across the Rust CLI and the Swift app or implemented Swift-only. Each new
platform would re-derive that layer in a new language. This plan moves that layer down across
the FFI line **before** the port, so a new shell is "bind the FFI + draw UI + open an audio
device" with **zero** decision logic.

This is a **relocation**, not a behavior change. Every step is gated on byte-identical output
against the existing corpus (the repo's established discipline: parity gates, additive ABI).

## 1. Target architecture — the boundary rule

Three tiers. One rule decides which tier any piece of code belongs to:

> **Decision lives in the Rust core. Presentation and platform live in the shell.**

- **Core (Rust, `couple-core`)** — anything that turns *inputs* (measurements, durations,
  content keys, flags, a gesture reduced to data) into an *outcome*. Deterministic, headless,
  no UI, no audio device, no per-platform assumptions. ffmpeg/ffprobe stay child processes.
- **Shell (per platform)** — pixels, UI state, the audio output device, gesture capture,
  localized strings, and the file/dir I/O that assembles the media pool.
- **FFI (`couple.h`)** — the only contact surface. It already carries two shapes, both reused
  here unchanged in spirit:
  - opaque `u64` handles + caller-allocated buffers for the **hot/real-time** paths
    (`cpl_band`, `cpl_raw_window`, `cpl_audition_fill`) — untouched;
  - **pure JSON-in / JSON-out** functions for decisions. Precedents already exist:
    `cpl_triangle_closes`, `cpl_fit_ls_json`, `cpl_pairing_params_default_json`. We add more in
    the same idiom.

All additions are **additive**: `cpl_abi_version` stays `1`.

## 2. Component inventory — the full map

| Component | Today | Target | Notes |
|---|---|---|---|
| decode/import, pyramid, fingerprint, matching, sync, export, audition DSP, measure, onsets, cache | `couple-core` | **stays** | already correct; the portable nucleus |
| **§3.3 pairing resolver** (coverage clustering, disjoint/overlap, simultaneity triangle, margin rule, row states, candidate-overflow, RMS escalation) | duplicated: `cli/src/pair.rs` **and** `mac/.../Resolver.swift` | **→ core** `couple_core::resolve` | the headline item; ~600 lines on each side collapse to one |
| **resolver constants** (`zMargin`, `triangleTolMs`, `coverageFull`, `disjointTolS`, RMS tiebreaker) | duplicated `PairParams` in both shells | **→ core** `ResolveParams` | mirrors how `PairingParams` is already core-owned |
| `MatchQualityPolicy` | Swift-only (CoupleKit) | **→ core** `policy::match_quality` | pure; thin map over `classification` |
| `RepairDropPolicy` | Swift-only | **→ core** `policy::repair_drop` | pure |
| `RowMergePolicy` | Swift-only | **→ core** `policy::row_merge` | pure |
| `MediaRemovalPolicy` | Swift-only | **→ core** `policy::media_removal` | pure |
| `AnchorProposal` | Swift-only | **→ core** `policy::anchor` | pure geometry/scoring |
| `ShadowTelemetry` projection | Swift-only | **→ core** (fold into `resolve`) | pure projection of `PairEvidence` |
| `ActivitySummary` | Swift-only | **split**: math → core `policy::activity`; strings → shell | English/localized copy stays UI |
| `PoolSelection` (Finder multi-select) | Swift-only | **judgment call** (see §6) | platform multi-select UX may legitimately differ |
| `AuditionController.transportPlan` | Swift-only | optional → core | pure, but low value; defer |
| `AppModel` (queue state/orchestration) | Swift | **stays (per platform)** | UI state shape differs per OS |
| `PairSession` (editor interaction / render shadow) | Swift | **stays (per platform)** | trivial mapping math is acceptable dup (§5) |
| `Views/*`, `WaveformStage`, `LoupeView` | Swift | **stays (per platform)** | pure presentation |
| `Engine.swift`, `Wire.swift` (FFI binding + DTOs) | Swift | **stays; re-implemented per platform** | thin; the template for new shells |
| `DebugDrive.swift` (screenshot/FPS harness) | Swift | **stays (Mac-only)** | dev tooling |
| `QueueStore` / `SessionStore` (persistence) | Swift | **consider → core schema** (§6) | core already owns `SessionFile`/`AlignmentState` |
| pool assembly (CLI dir scan / app drag-drop) | each shell | **stays (per platform)** | core only needs the final track-id sets |

## 3. New core surface

### 3.1 `couple_core::resolve` (the centrepiece)

Core already builds the graph-adjusted `PairEvidence` matrix (`Ctx::match_candidates_graph`)
*and*, internally, the full node×node graph that the audio↔audio triangle needs. The §3.3 row
resolution on top of it is the only missing piece. Move it in:

```rust
// core/src/resolve.rs
pub struct ResolveParams {
    pub pairing: PairingParams,    // already core-owned: verifier thresholds + graph tolerances
    pub matcher: MatcherChoice,    // Fingerprint (default) | Legacy | Shadow  (== CLI --matcher)
    pub z_margin: f64,             // §3.5 decisive-best margin
    pub triangle_tol_ms: f64,      // §3.4 legacy closure tolerance
    pub coverage_full: f64,        // single-match confident floor
    pub disjoint_tol_s: f64,       // interval-overlap clustering tolerance
    pub rms_trust_floor_ms: f64,   // legacy RMS tie-breaker
    pub rms_ratio: f64,
    pub min_fit_windows: usize,
}

// Wire types relocated from the two shells (camelCase serde, identical to today):
pub struct ResolvedRow { /* videoKey, videoName, state, chip, selected, alternates, detail */ }
pub struct RowCandidate { /* audioKey, audioName, score/z, lagMs, coverage, telemetry… */ }
pub enum   RowState { Confident, Partial, Split, TwoSources, Ambiguous, NoMatch }
pub struct ResolutionDetail { /* margin, clusters, triangles, note, candidateOverflow, shadow */ }

impl Ctx {
    /// Full §3.3 resolution over imported tracks. Builds the graph matrix internally
    /// (reusing match_candidates_graph) and resolves every row — including the audio↔audio
    /// triangle, read from the same full graph (no extra match_pair round-trips).
    pub fn resolve(&self, videos: &[TrackId], audios: &[TrackId], p: &ResolveParams)
        -> Result<Vec<ResolvedRow>>;
    /// Job body with progress (MatchFingerprint/MatchCorrelate/MatchGraph + a final resolve tick).
    pub fn resolve_prog(&self, videos: &[TrackId], audios: &[TrackId], p: &ResolveParams,
        prog: &mut ProgressEmitter, cancel: &CancelToken) -> Result<Vec<ResolvedRow>>;
}
```

Core has each track's file name (`meta.info.path`) and content key (`meta.content_key`), so it
produces fully-formed rows (keys, names, deterministic name tiebreaks) — the shell threads no
metadata. Both shells already hold the imported track-ids, so the call site is trivial.

FFI (additive):

```c
char*    cpl_resolve(uint64_t ctx, const uint64_t* videos, uintptr_t nv,
                     const uint64_t* audios, uintptr_t na, const char* params_json);
uint64_t cpl_resolve_start(uint64_t ctx, const uint64_t* videos, uintptr_t nv,
                           const uint64_t* audios, uintptr_t na, const char* params_json,
                           CplProgressCb cb, void* user);   // terminal payload = [ResolvedRow] JSON
char*    cpl_resolve_params_default_json(void);
```

`cpl_resolve_start` replaces today's two-step dance (`cpl_match_candidates_graph_start` →
`Resolver.resolve(prebuiltMatrix:)`) with one job that returns rows directly. The existing
`cpl_match_candidates*` symbols stay for shadow/debug and the ≥1-release legacy override.

### 3.2 `couple_core::policy` (pure decisions)

```rust
// core/src/policy/{mod,match_quality,repair_drop,row_merge,media_removal,anchor,activity}.rs
// Each: serde Request/Response + the decision fn + the invariant unit tests ported from CoupleKitTests.
```

FFI: one pure function per policy, no ctx (exactly like `cpl_triangle_closes`):

```c
char* cpl_policy_match_quality(const char* json);
char* cpl_policy_repair_drop(const char* json);
char* cpl_policy_row_merge(const char* json);
char* cpl_policy_media_removal(const char* json);
char* cpl_policy_anchor_propose(const char* json);
char* cpl_policy_activity(const char* json);   // returns a structured decision; shell formats strings
```

These run on UI gestures (clicks, drops, imports) — never per frame — so a JSON round-trip is
free. The `CoupleKitTests` invariants (1818 lines) become Rust unit tests; a slim Swift test
keeps verifying the wrapper plumbs through.

## 4. Migration phases (each independently shippable, each gated)

**Phase A — Stand up `couple_core::resolve` as a proven superset.** Port the §3.3 logic
(both the fingerprint and legacy paths + clustering + triangle + margin + overflow + RMS
escalation) into core. No shell change yet. Gate: a new core test asserts `ctx.resolve` is
**byte-identical** to today's `cli/src/pair.rs` rows over the fixtures (bach 169→Gigue,
170 ambiguous, 171→Prelude, 172→ZOOM0033; the `multifile-session`; the matcher-corpus).

**Phase B — CLI consumes core.** `run_pair` calls `ctx.resolve(...)` and keeps only its own
report wrapper (matrix/evidence/timings/debug dumps stay CLI presentation). Delete
`resolve_row`, `resolve_row_fp`, `cluster_intervals`, `correlate_audio_pair`,
`match_audio_pair_evidence`, `candidate_overflows` from `pair.rs`. Gate: `couple pair --json`
byte-identical on every fixture (the repo's §7.6 stability gate already exists).

**Phase C — Swift consumes core.** Add `Engine.resolve(...)` / `Engine.resolveStart(...)`;
`AppModel` calls them. Delete the decision body of `Resolver.swift` (`resolve`,
`resolveFingerprint`, `resolveRow*`, `clusterIntervals`, `escalate`, `finalize`, `batchFits`,
`classifyMatches`, `shadowEvidence`, `candidateOverflows`, `matchAudioPairEvidence`); keep a
thin `ResolvedRow` decode in `Wire.swift`. Gate: the app's golden-row expectations + a parity
test that Swift rows equal CLI rows on the shared fixtures.

**Phase D — Collapse the resolver constants.** Delete the duplicated `PairParams` fields in
`cli/src/pair.rs` and `Resolver.swift`; both read `cpl_resolve_params_default_json` (mirrors the
existing `pairingParamsDefault()` pattern). Gate: defaults identical; `--params` overrides still
work in the CLI.

**Phase E — Lift the pure policies, one per change.** Order by value/cleanliness:
`match_quality → repair_drop → row_merge → media_removal → anchor → shadow`. For each: port the
fn + invariant tests to `couple_core::policy`, expose `cpl_policy_*`, replace the Swift body with
a wrapper, keep the Swift test. Gate per policy: ported Rust tests green **and** the Swift suite
green through the wrapper.

**Phase F — Split `ActivitySummary`.** Move priority/weighting/fraction/debounce math to
`policy::activity` (returns a structured decision: winning kind, fraction, phaseKey, counts).
Keep string assembly + localization in the shell. Gate: the shell renders identical lines for a
table of representative snapshots.

**Phase G — New shells.** Port `Engine`/`Wire` to the target language (thin FFI binding), build
the UI, and wire the audio output (the DSP is already core-side via `cpl_audition_fill`; the
platform layer is just "pull frames → device": CoreAudio / WASAPI / ALSA). No resolver, no
policies.

## 5. What stays in the shell — and acceptable duplication

Per-platform, with the Mac app as the reference implementation: `AppModel`, `PairSession`, all
`Views/*`, the audition device glue, persistence I/O, pool assembly, the FFI binding layer, and
all user-facing strings.

**Acceptable duplication — `Mapping.swift`** (34 lines: `offsetMs + ppm·t`). This is a trivial
affine the editor evaluates while scrubbing the waveform; an FFI call per frame would be wrong.
The *semantics* that matter (when drift applies; the `none` vs `resample` rule) are **already**
core-owned and enforced by `session.rs`, so the shell-side arithmetic is a safe mirror. Keep it,
and label it as a deliberate mirror so it isn't mistaken for stray logic.

## 6. Open decisions for the owner

1. **`PoolSelection`** — share as core policy, or treat Finder-style multi-select as
   per-platform UX (Windows Explorer differs)? **Recommendation:** leave in the shell initially;
   revisit if Windows wants byte-identical selection. It is the one "policy" that is arguably
   presentation.
2. **`ActivitySummary` strings** — localize in the shell (recommended) vs. core returns English.
   The plan assumes shell-side localization with core owning only the math.
3. **Project persistence (`QueueStore`/`SessionStore`)** — do we want a Mac project to open
   identically on Windows? If yes, the queue/approval/provenance **schema** should move to core
   (core already owns `SessionFile`/`AlignmentState`). Valuable but scope-expanding; **proposed
   as a separate follow-up**, not part of this boundary refactor.
4. **Deprecation** — keep `cpl_match_candidates_graph*` public (shadow/debug/legacy override) or
   fold entirely under `cpl_resolve`? **Recommendation:** keep for ≥1 release per the existing
   §16.1 override policy.

## 7. Risks & mitigations

- **Behavior drift during the move.** Mitigated by the byte-identity gates at every phase; the
  fixtures already encode the expected rows.
- **Determinism.** The resolver's content/name-deterministic ordering (SPEC §1.4) must be
  preserved; core has the file names and content keys, so ordering is reproduced exactly. Covered
  by the parity gate.
- **"User can override resolution" concern.** Overrides are shell **state** (pin / approve /
  manual repair via `RowMergePolicy`/`RepairDropPolicy`), layered *on top* of resolution. Core
  resolves; the shell remembers what the user decided. Moving resolution to core does **not** move
  user state.
- **Matrix size over JSON.** `[[PairEvidence]]` is produced once per resolve, not per frame —
  JSON is fine (and `cpl_resolve` keeps the matrix internal anyway, returning only rows).
- **ffmpeg discovery on Windows/Linux.** Already parameterized via `CtxConfig`
  (`ffmpegPath`/`ffprobePath`); verify bundling/lookup per platform in Phase G.

## 8. Effort shape (relative)

- Phases A–D (resolver + constants): the bulk — one focused effort, but mechanical and
  gate-protected. Net **−~1200 duplicated lines** across the two shells, **+~600** in core.
- Phase E (policies): small, independent, ~½ day each.
- Phase F (activity split): small.
- Phase G (new shells): the actual port — now free of decision logic.

## 9. Definition of done

A new shell can reach feature parity by implementing only: the FFI binding, the UI, pool
assembly I/O, and the audio output callback. `grep` finds **no** resolution or policy decision
outside `couple-core` (the existing source-search gate style can enforce this). The CLI and every
GUI shell produce identical rows for identical inputs, proven by a shared fixture parity test.
