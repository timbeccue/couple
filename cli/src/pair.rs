//! `couple pair <dir>` — the headless v2 pairing surface (SPEC-V2 §3/§4).
//!
//! Core supplies mechanical measurements (`pair_candidates`); THIS layer owns
//! resolution — coverage intervals, disjoint/overlapping discrimination, the §3.4
//! audio×audio simultaneity check (triangle closure), the §3.5 margin rule, and the
//! §3.3 row states — because resolution is shell logic the user can override.
//!
//! Honesty posture (V2-F2): when conflicting candidates aren't separable, the row is
//! `ambiguous` with NO selection — never a coin-flip. Timestamps are surfaced as
//! metadata only; nothing here creates, vetoes, or orders a pairing from them
//! (SPEC §1.4/F3 — ties order by file name, which is deterministic and clock-free).
//!
//! Stability (§3.6 / gate §7.6): resolved rows contain no run-varying fields (no
//! timings, no track ids, no absolute paths) and all orderings are content/name
//! deterministic, so re-running over the same inputs is byte-identical and a new
//! file changes only what it can change. Pinning of user-verified rows is shell
//! state and lives with the queue persistence (Phase 9), not in this stateless CLI.

use couple_core::matching::{MatchClassification, MatchMethod, PairEvidence};
use couple_core::pair::PairCorrelation;
use couple_core::pairing_params::MatcherMode;
use couple_core::sync::overlap_info;
use couple_core::{CancelToken, CoupleError, Ctx, MediaInfo, PairingParams, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Which recording-identity matcher `couple pair` runs (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.3 /
/// §16.1). Boundary 13 made `Fingerprint` the DEFAULT resolving authority (the §13.4 resolver
/// migration): rows are resolved from core `PairEvidence` (the graph-adjusted matrix), with
/// `classification` the identity authority — NO acceptance threshold is re-derived in the shell
/// (invariant 8). `Legacy` is the DEVELOPER OVERRIDE (`--matcher legacy`, §16.1): it keeps the
/// proven envelope resolution + resolved rows BYTE-IDENTICAL for ≥ 1 release. `Shadow` resolves
/// with legacy but ALSO attaches the spectral matrix for observation (calibration read-out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherChoice {
    /// The boundary-13 DEVELOPER OVERRIDE: today's envelope resolution, byte-identical output, no
    /// spectral computation (§16.1).
    Legacy,
    /// Resolve with LEGACY but ALSO compute the spectral matrix and include both matrices in --json
    /// (observational; the resolved rows stay legacy — for calibration comparison).
    Shadow,
    /// The DEFAULT (boundary 13): resolve rows from the core graph-adjusted `PairEvidence` matrix —
    /// `classification` is the identity authority, `score` the deterministic order; coverage
    /// clustering + override stay shell policy (§13.4).
    Fingerprint,
}

impl MatcherChoice {
    /// Parse the `--matcher` value. Unknown values are rejected with a clear message.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "legacy" => Ok(Self::Legacy),
            "shadow" => Ok(Self::Shadow),
            "fingerprint" => Ok(Self::Fingerprint),
            other => Err(CoupleError::invalid(format!(
                "--matcher must be legacy|shadow|fingerprint, got {other}"
            ))),
        }
    }

    /// True when the spectral `PairEvidence` matrix must be computed (Shadow displays it alongside
    /// the legacy resolution; Fingerprint resolves FROM it).
    fn computes_evidence(self) -> bool {
        matches!(self, Self::Shadow | Self::Fingerprint)
    }

    /// True when the resolved rows come from the spectral `PairEvidence` matrix (the boundary-13
    /// default). `Legacy`/`Shadow` keep the legacy resolver as the row authority.
    fn resolves_with_fingerprints(self) -> bool {
        matches!(self, Self::Fingerprint)
    }
}

/// Resolution constants (SPEC-V2 §9). All [TUNE] values are surfaced verbatim in the
/// --json output so the future corpus calibration has the raw numbers (V2-F2).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairParams {
    /// Pairing floor on stage-1 z [VALIDATED, Session C].
    pub z_min: f64,
    /// Decisive-best margin [TUNE, V2-F2].
    pub z_margin: f64,
    /// §3.4 closure tolerance [SPEC, TUNE].
    pub triangle_tol_ms: f64,
    /// Coverage at/above which a single match is `confident` [SPEC, TUNE].
    pub coverage_full: f64,
    /// Coverage intervals intersecting by less than this are `disjoint` (recorder
    /// restart) rather than `overlapping` (alternates/conflict) [SPEC, TUNE]. Below
    /// the stage-1 floor (20 s) the §3.4 check could not measure anyway.
    pub disjoint_tol_s: f64,

    // ---- resolver-owned classifier policy (PLAN-AUDIO-MATCHING-PREWORK §6.5) ----
    // `verify_active` is the Phase-1↔Phase-2 switch (default OFF ⇒ candidacy is exactly
    // today's z-gate, byte-identical). Phase 4 (§9) RETIRED the old top-K
    // `verify_max_candidates_per_video` truncation as a silent false-negative path: every
    // `prefilter_candidate` is now verified (in chunks), and the only bound on work is the
    // core-owned `max_verify_pairs_per_video`/`max_verify_pairs_total` SAFETY CEILING, which
    // surfaces as honest `ambiguous` (a `candidateOverflow` condition) rather than dropping
    // candidates. Every VERIFIER THRESHOLD lives in the core-owned `pairing` below — no
    // copies here, so the CLI and core never drift (the source-search gate, §6.8.6, fails if
    // a verifier literal reappears).
    /// Phase-2 master switch [PHASE-2, D2 default OFF].
    pub verify_active: bool,
    /// Core-owned pairing + fine-verifier parameters — the single source of those defaults.
    /// Resolver `--params` keys it doesn't own delegate here ([`PairParams::set`]). The
    /// `max_verify_pairs_*` resource ceilings (Phase 4 §9) live here.
    pub pairing: PairingParams,
}

impl Default for PairParams {
    fn default() -> Self {
        Self {
            z_min: 8.5,
            z_margin: 6.0,
            triangle_tol_ms: 250.0,
            coverage_full: 0.98,
            disjoint_tol_s: 0.5,
            verify_active: false,
            pairing: PairingParams::default(),
        }
    }
}

impl PairParams {
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let num = || {
            value
                .parse::<f64>()
                .map_err(|_| CoupleError::invalid(format!("--params {key}: not a number")))
        };
        match key {
            "z-min" => self.z_min = num()?,
            "z-margin" => self.z_margin = num()?,
            "triangle-tol-ms" => self.triangle_tol_ms = num()?,
            "coverage-full" => self.coverage_full = num()?,
            "disjoint-tol-s" => self.disjoint_tol_s = num()?,
            // verify-active takes 0/1 (!= 0 ⇒ ON).
            "verify-active" => self.verify_active = num()? != 0.0,
            // Every verifier/pairing threshold (incl. the `max-verify-pairs-*` resource
            // ceilings) is core-owned: delegate to PairingParams::set
            // so the CLI and core can never drift (§6.5 / §2.2).
            _ => self.pairing.set(key, value)?,
        }
        Ok(())
    }
}

// ---------- wire types ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolTrack {
    file: String,
    duration_s: f64,
    content_key: String,
    /// Metadata timestamp, hint only (SPEC §1.4) — surfaced, never consulted.
    creation_time_utc: Option<String>,
    /// Videos only: false ⇒ unanalyzable, row is `no match` (gate §7.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    has_audio: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MatrixCell {
    video: String,
    audio: String,
    #[serde(flatten)]
    m: PairCorrelation,
    /// z ≥ Z_MIN — kept for resolution.
    candidate: bool,
}

/// Video-timeline span the audio covers (§3.2, ppm = 0 arithmetic of §5.6).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct Coverage {
    t0: f64,
    t1: f64,
    seconds: f64,
    fraction: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RowCandidate {
    audio: String,
    z: f64,
    lag_ms: f64,
    coverage: Coverage,
}

/// §3.4 record for one overlapping audio pair (raw numbers for calibration).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TriangleCheck {
    audio_a: String,
    audio_b: String,
    /// A×B stage-1 z (must clear Z_MIN).
    ab_z: f64,
    ab_lag_ms: f64,
    /// |lag(V→B) − lag(V→A) − lag(A→B)|, ms.
    residual_ms: f64,
    simultaneous: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum RowState {
    Confident,
    Partial,
    Split,
    TwoSources,
    Ambiguous,
    NoMatch,
}

impl RowState {
    /// §3.3 chip vocabulary (human output).
    fn chip(&self) -> &'static str {
        match self {
            RowState::Confident => "confident",
            RowState::Partial => "partial",
            RowState::Split => "split across files",
            RowState::TwoSources => "2 sources",
            RowState::Ambiguous => "ambiguous — check me",
            RowState::NoMatch => "no match",
        }
    }
}

/// Why a row's candidates could not be verified within the configured resource
/// ceiling (PLAN-AUDIO-MATCHING-PREWORK §9.2). Constructed ONLY when the eligible
/// candidate count for a row would exceed `max_verify_pairs_per_video`, or the
/// running total across rows would exceed `max_verify_pairs_total`. The honest
/// alternative to silently dropping candidates: NO subset is verified, the row is
/// `ambiguous` (never `noMatch`/`confident`), and every candidate name stays visible
/// so the user can resolve it manually. Non-persisted (resolver-local telemetry).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateOverflow {
    /// Number of eligible (`prefilter_candidate`) cells for this row.
    eligible: usize,
    /// The resource ceiling that was exceeded.
    limit: usize,
    /// Which ceiling: "perVideo" (`max_verify_pairs_per_video`) or "total"
    /// (`max_verify_pairs_total`).
    scope: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolutionDetail {
    /// z_best − z_second among conflicting (overlapping, non-simultaneous)
    /// candidates; null when the margin rule never ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    margin: Option<f64>,
    /// Coverage-interval clusters (1 = all overlapping, >1 = disjoint groups).
    clusters: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    triangles: Vec<TriangleCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    /// Set ONLY when a resource ceiling forced this row to honest ambiguity instead
    /// of silently truncating candidates (§9.2). Optional + non-persisted.
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_overflow: Option<CandidateOverflow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    video: String,
    state: RowState,
    chip: String,
    /// Null for `no match` and `ambiguous` (V2-F2: no coin-flip selection).
    selected: Option<RowCandidate>,
    alternates: Vec<RowCandidate>,
    resolution: ResolutionDetail,
}

/// One spectral-evidence cell (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.3), emitted ONLY in
/// `--matcher shadow|fingerprint`. Carries the (video, audio) names alongside the method-neutral
/// [`PairEvidence`] so the shadow matrix is self-describing for calibration. Observational only:
/// it NEVER feeds the resolved rows (those stay legacy until boundary 13).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceCell {
    video: String,
    audio: String,
    #[serde(flatten)]
    evidence: PairEvidence,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairReport {
    schema: &'static str,
    dir: String,
    constants: PairParams,
    skipped: Vec<String>,
    videos: Vec<PoolTrack>,
    audios: Vec<PoolTrack>,
    matrix: Vec<MatrixCell>,
    /// The spectral `PairEvidence` matrix (rows = videos, columns = audios), present ONLY in
    /// `--matcher shadow|fingerprint`. `None` (skipped) in the default `legacy` mode, so the legacy
    /// JSON is BYTE-IDENTICAL to before this field existed (§14.3, byte-identity gate). Observational:
    /// the legacy `rows` below remain the resolution authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_matrix: Option<Vec<Vec<EvidenceCell>>>,
    rows: Vec<Row>,
    unused_audio: Vec<String>,
    timing_ms: TimingMs,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimingMs {
    probe: f64,
    import: f64,
    matrix: f64,
    resolve: f64,
}

// ---------- the command ----------

#[allow(clippy::too_many_arguments)]
pub fn run_pair(
    json: bool,
    quiet: bool,
    progress_json: bool,
    ctx: &Arc<Ctx>,
    dir: &Path,
    params: &PairParams,
    verify_debug: bool,
    matcher: MatcherChoice,
    matcher_debug: bool,
    cancel: &CancelToken,
) -> Result<()> {
    if !dir.is_dir() {
        return Err(CoupleError::invalid(format!("{} is not a directory", dir.display())));
    }
    // Validate the core-owned pairing params once before any work (§6.2).
    params.pairing.validate()?;
    // Deterministic listing: plain files, sorted by name; dotfiles ignored.
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| CoupleError::invalid(format!("read dir: {e}")))?
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();

    // 1. Probe + classify (§2.1): video stream ⇒ video role; audio-only ⇒ audio;
    //    non-media/unreadable ⇒ skipped, surfaced once.
    let t_probe = std::time::Instant::now();
    let mut skipped: Vec<String> = Vec::new();
    let mut video_probes: Vec<(String, MediaInfo)> = Vec::new();
    let mut audio_probes: Vec<(String, MediaInfo)> = Vec::new();
    for n in &names {
        cancel.check()?;
        match ctx.probe_lenient(&dir.join(n)) {
            Ok(info) if info.video.is_some() => video_probes.push((n.clone(), info)),
            Ok(info) if !info.audio.is_empty() => audio_probes.push((n.clone(), info)),
            _ => skipped.push(n.clone()),
        }
    }
    let probe_ms = t_probe.elapsed().as_secs_f64() * 1000.0;

    // 2. Import the analyzable pool (videos need an audio stream to correlate).
    let t_import = std::time::Instant::now();
    let mut videos: Vec<PoolTrack> = Vec::new();
    let mut video_ids: Vec<Option<u64>> = Vec::new();
    for (n, info) in &video_probes {
        cancel.check()?;
        if info.audio.is_empty() {
            videos.push(PoolTrack {
                file: n.clone(),
                duration_s: info.duration_s,
                content_key: couple_core::cache::content_key(&dir.join(n))?,
                creation_time_utc: info.creation_time_utc.clone(),
                has_audio: Some(false),
            });
            video_ids.push(None);
            continue;
        }
        let id = import_quiet(ctx, &dir.join(n), couple_core::TrackRole::Camera, quiet, progress_json, cancel)?;
        let t = ctx.track(id)?;
        videos.push(PoolTrack {
            file: n.clone(),
            duration_s: t.meta.duration_s,
            content_key: t.meta.content_key.clone(),
            creation_time_utc: info.creation_time_utc.clone(),
            has_audio: Some(true),
        });
        video_ids.push(Some(id));
    }
    let mut audios: Vec<PoolTrack> = Vec::new();
    let mut audio_ids: Vec<u64> = Vec::new();
    for (n, info) in &audio_probes {
        cancel.check()?;
        let id = import_quiet(ctx, &dir.join(n), couple_core::TrackRole::External, quiet, progress_json, cancel)?;
        let t = ctx.track(id)?;
        audios.push(PoolTrack {
            file: n.clone(),
            duration_s: t.meta.duration_s,
            content_key: t.meta.content_key.clone(),
            creation_time_utc: info.creation_time_utc.clone(),
            has_audio: None,
        });
        audio_ids.push(id);
    }
    let import_ms = t_import.elapsed().as_secs_f64() * 1000.0;

    // 3. Candidate matrix (§3.2): stage-1 between every analyzable (video, audio).
    let t_matrix = std::time::Instant::now();
    // Core-owned pairing params drive BOTH the candidate matrix and `verify_batch`, so the
    // matrix and the classifier see identical thresholds (§6.5). `verify_active` itself is a
    // resolver-only switch (the verifier is a pure function of `pairing`).
    let sp = params.pairing.clone();
    let importable: Vec<u64> = video_ids.iter().flatten().copied().collect();
    let cells = ctx.pair_candidates(&importable, &audio_ids, &sp)?;
    let mut matrix: Vec<MatrixCell> = Vec::new();
    // Per-video candidate measurements, indexed like `videos`.
    let mut per_video: Vec<Vec<(usize, PairCorrelation)>> = vec![Vec::new(); videos.len()];
    {
        let mut row_iter = cells.into_iter();
        for (vi, id) in video_ids.iter().enumerate() {
            if id.is_none() {
                continue;
            }
            let row = row_iter.next().expect("matrix row per imported video");
            for (ai, m) in row.into_iter().enumerate() {
                // `candidate` = "passed the pre-filter" (ruling D4 — its meaning/shape is
                // unchanged for a given input, so gate6 stays trivially safe). Flag ON: the
                // loose Stage-A pre-filter — core's `prefilter_candidate` (ncc ≥ refine_ncc_min,
                // §7.2), consumed not re-derived (invariant 8); `z` no longer rejects and the
                // fine classifier disposes below. Flag OFF: the unchanged default z-gate against
                // the RESOLVER's own `z_min` (a resolution constant distinct from the pairing
                // `legacy_z_min`/`legacy_candidate`, so `--params z-min` still tightens it and
                // the row decision is byte-identical to today, §7.2 / invariant 7).
                let candidate = if params.verify_active {
                    m.prefilter_candidate
                } else {
                    !m.insufficient_overlap && m.z >= params.z_min
                };
                matrix.push(MatrixCell {
                    video: videos[vi].file.clone(),
                    audio: audios[ai].file.clone(),
                    m: m.clone(),
                    candidate,
                });
                if candidate {
                    per_video[vi].push((ai, m));
                }
            }
        }
    }

    // 3a. Match classifier (PLAN-MATCH-CLASSIFIER §5.2 / Stage B; Phase-4 §9 rework) —
    // flag ON only. We verify EVERY `prefilter_candidate` (no sort-and-truncate top-K — that
    // was a silent false-negative path, §2.5/§9.1): the real-match set (is_match survivors)
    // REPLACES `per_video[vi]` feeding the UNCHANGED §3.3 pipeline. The ONLY bound on work is
    // the core-owned `max_verify_pairs_per_video`/`max_verify_pairs_total` SAFETY CEILING; a
    // row that would exceed it verifies NO arbitrary subset, records a `candidate_overflow`
    // condition, and is forced to honest `ambiguous` with every candidate still visible
    // (§9.2) — a resource limit can degrade to ambiguity but can never manufacture a false
    // negative. Verification runs in chunks to bound temporary allocation (the request/result
    // buffers), independent of how many cells a video admits. Flag OFF: this block is skipped
    // and `per_video` is the z-passing set — byte-identical to today.
    let mut overflow: Vec<Option<CandidateOverflow>> = vec![None; videos.len()];
    if params.verify_active {
        // Chunk size bounds the transient verify request/result allocation; correctness is
        // independent of it (every eligible cell within the ceiling is verified).
        const VERIFY_CHUNK: usize = 256;
        let per_video_limit = sp.max_verify_pairs_per_video;
        let total_limit = sp.max_verify_pairs_total;
        let mut total_verified: usize = 0;
        for (vi, id) in video_ids.iter().enumerate() {
            let Some(vid) = id else { continue };
            let cands = &mut per_video[vi];
            if cands.is_empty() {
                continue;
            }
            let eligible = cands.len();
            // Resource guard, NOT a ranking policy: a row whose eligible set exceeds the
            // per-video ceiling, or whose addition would exceed the per-resolve ceiling,
            // overflows. We keep `cands` intact (all names visible) and verify nothing for
            // it — `resolve_row` will surface it as `ambiguous` with the overflow detail.
            if eligible > per_video_limit {
                overflow[vi] = Some(CandidateOverflow { eligible, limit: per_video_limit, scope: "perVideo" });
                continue;
            }
            if total_verified + eligible > total_limit {
                overflow[vi] = Some(CandidateOverflow { eligible, limit: total_limit, scope: "total" });
                continue;
            }
            total_verified += eligible;

            // Verify ALL eligible cells in chunks, keeping only the real matches (is_match).
            // Verify numbers stay OUT of the serialized Row/MatrixCell (gate6 byte-stability).
            let mut real: Vec<(usize, PairCorrelation)> = Vec::new();
            for chunk in cands.chunks(VERIFY_CHUNK) {
                let req: Vec<(u64, u64, f64)> =
                    chunk.iter().map(|(ai, m)| (*vid, audio_ids[*ai], m.lag_ms)).collect();
                let fits = ctx.verify_batch(&req, &sp)?;
                for ((ai, m), fit) in chunk.iter().zip(fits) {
                    if let Some(f) = fit {
                        if couple_core::verify::is_match(&f) {
                            real.push((*ai, m.clone()));
                        }
                    }
                }
            }
            *cands = real;
        }
    }
    let matrix_ms = t_matrix.elapsed().as_secs_f64() * 1000.0;

    // 3b. --verify-debug (PLAN-MATCH-CLASSIFIER §4.5): READ-ONLY fine-verify over the
    // FULL V×A matrix with the z-gate BYPASSED, printed to STDERR only (never into the
    // serialized Row/MatrixCell — gate6 stays byte-stable). This is the owner's window
    // onto the z-rejected short clip, whose row never clears the z-gate, so it would be
    // invisible if verify ran only over candidates. Changes no decision.
    if verify_debug {
        verify_debug_dump(ctx, &sp, &videos, &video_ids, &audios, &audio_ids, &matrix)?;
    }

    // 3c. The spectral `PairEvidence` matrix (PLAN-SPECTRAL-FINGERPRINT-MATCHING §13.4 / §14.3 /
    // §16.1). Computed for `shadow` (observational) AND `fingerprint` (the boundary-13 DEFAULT
    // resolution authority). For `fingerprint` it is the GRAPH-ADJUSTED matrix under the REAL `sp`
    // (FingerprintPrimary by default), resolved into rows below; for `shadow` it is a separate
    // FingerprintShadow computation attached for comparison while the legacy rows resolve. In
    // `legacy` mode this is skipped ⇒ `evidenceMatrix` is None and the output is byte-identical to
    // the legacy surface.
    //
    // `importable` (videos with an audio stream) maps row-for-row onto the matrix; audio-less videos
    // are absent (their row is an honest `no match`). `fp_rows` holds the graph-adjusted evidence
    // indexed like `importable` for the §13.4 resolver.
    let mut fp_rows: Option<Vec<Vec<PairEvidence>>> = None;
    let evidence_matrix: Option<Vec<Vec<EvidenceCell>>> = if matcher.computes_evidence() {
        let ev = if matcher.resolves_with_fingerprints() {
            // The DEFAULT (§16.1): the GRAPH-ADJUSTED matrix under the real params (FingerprintPrimary
            // + profile 2). §16.2 fallback (artifact build/read failure, unsupported fingerprint
            // version, or a forced legacy override) is handled INSIDE core (`match_candidates_graph`
            // → `match_candidates` falls a cell back to `EnvelopeV1` when its fingerprint is
            // unavailable; the method/reason is visible in the cell + the debug table below).
            let g = ctx.match_candidates_graph(&importable, &audio_ids, &sp)?;
            fp_rows = Some(g.clone());
            g
        } else {
            // `shadow`: a SEPARATE FingerprintShadow computation (legacy correlation attached); the
            // legacy rows remain the resolution authority.
            let mut shadow_params = sp.clone();
            shadow_params.matcher_mode = MatcherMode::FingerprintShadow;
            ctx.match_candidates(&importable, &audio_ids, &shadow_params)?
        };
        // Re-attach the pool (video, audio) names, walking `video_ids` so audio-less videos are
        // skipped exactly like the importable list was built.
        let mut cells: Vec<Vec<EvidenceCell>> = Vec::with_capacity(ev.len());
        let mut ev_iter = ev.into_iter();
        for (vi, id) in video_ids.iter().enumerate() {
            if id.is_none() {
                continue;
            }
            let row = ev_iter.next().expect("evidence row per imported video");
            let named: Vec<EvidenceCell> = row
                .into_iter()
                .enumerate()
                .map(|(ai, evidence)| EvidenceCell {
                    video: videos[vi].file.clone(),
                    audio: audios[ai].file.clone(),
                    evidence,
                })
                .collect();
            cells.push(named);
        }
        if matcher_debug {
            matcher_debug_dump(matcher, &cells);
            // §16.2: when fingerprint-primary fell back to the legacy matcher for any cell, the
            // method + reason are visible in debug output.
            fallback_debug_dump(matcher, &cells);
        }
        Some(cells)
    } else {
        None
    };

    // 4. Resolution (§3.3) per video row. The boundary-13 DEFAULT resolves from the graph-adjusted
    // `PairEvidence` (`resolve_row_fp`, §13.4 — `classification` is the identity authority); the
    // `--matcher legacy` developer override (and `shadow`) keep the byte-identical legacy resolver.
    let t_resolve = std::time::Instant::now();
    let mut rows: Vec<Row> = Vec::new();
    if let Some(fp) = fp_rows {
        // Map the importable-indexed evidence rows back onto the full video list; an audio-less
        // video resolves `no match` (no evidence row, exactly like the legacy path).
        let mut ab_memo: HashMap<(usize, usize), PairEvidence> = HashMap::new();
        let mut fp_iter = fp.into_iter();
        for (vi, v) in videos.iter().enumerate() {
            cancel.check()?;
            if video_ids[vi].is_none() {
                rows.push(resolve_row_fp(
                    ctx, params, &sp, v, &[], None, &audios, &audio_ids, &mut ab_memo,
                )?);
                continue;
            }
            let ev_row = fp_iter.next().expect("evidence row per imported video");
            let cells: Vec<(usize, PairEvidence)> = ev_row.into_iter().enumerate().collect();
            rows.push(resolve_row_fp(
                ctx, params, &sp, v, &cells, None, &audios, &audio_ids, &mut ab_memo,
            )?);
        }
    } else {
        let mut ab_memo: HashMap<(usize, usize), PairCorrelation> = HashMap::new();
        for (vi, v) in videos.iter().enumerate() {
            cancel.check()?;
            rows.push(resolve_row(
                ctx, params, &sp, v, &per_video[vi], overflow[vi].clone(), &audios, &audio_ids,
                &mut ab_memo,
            )?);
        }
    }
    // §3.6: audio no row SELECTS stays visible in the unused tray.
    let used: std::collections::HashSet<&str> = rows
        .iter()
        .filter_map(|r| r.selected.as_ref().map(|s| s.audio.as_str()))
        .collect();
    let unused_audio: Vec<String> = audios
        .iter()
        .map(|a| a.file.clone())
        .filter(|f| !used.contains(f.as_str()))
        .collect();
    let resolve_ms = t_resolve.elapsed().as_secs_f64() * 1000.0;

    let report = PairReport {
        schema: "couple-pair/1",
        dir: dir.display().to_string(),
        constants: params.clone(),
        skipped,
        videos,
        audios,
        matrix,
        evidence_matrix,
        rows,
        unused_audio,
        timing_ms: TimingMs {
            probe: probe_ms,
            import: import_ms,
            matrix: matrix_ms,
            resolve: resolve_ms,
        },
    };

    if json {
        println!("{}", serde_json::to_string(&report).expect("serialize"));
    } else {
        print_human(&report);
    }
    Ok(())
}

/// READ-ONLY `--verify-debug` dump (PLAN-MATCH-CLASSIFIER §4.5). Runs fine-verify over
/// EVERY matrix cell with a plausible lag — the z-gate is BYPASSED, so the z-rejected
/// short clip (whose row never clears the gate) is shown — and prints a per-candidate
/// table to STDERR. Adds nothing to the serialized Row/MatrixCell, so gate6 byte-stability
/// is untouched. Changes no decision.
#[allow(clippy::too_many_arguments)]
fn verify_debug_dump(
    ctx: &Arc<Ctx>,
    sp: &PairingParams,
    videos: &[PoolTrack],
    video_ids: &[Option<u64>],
    audios: &[PoolTrack],
    audio_ids: &[u64],
    matrix: &[MatrixCell],
) -> Result<()> {
    // Map file name → track id (audios all have ids; videos may be stripped ⇒ None).
    let mut vid_id: HashMap<&str, u64> = HashMap::new();
    for (v, id) in videos.iter().zip(video_ids) {
        if let Some(id) = id {
            vid_id.insert(v.file.as_str(), *id);
        }
    }
    let mut aud_id: HashMap<&str, u64> = HashMap::new();
    for (a, id) in audios.iter().zip(audio_ids) {
        aud_id.insert(a.file.as_str(), *id);
    }

    // Build the verify request from EVERY cell that has a plausible lag (z NOT consulted).
    // `verify_pair` itself returns None below the absolute overlap floor.
    let mut req: Vec<(u64, u64, f64)> = Vec::new();
    let mut req_cells: Vec<&MatrixCell> = Vec::new();
    for cell in matrix {
        if cell.m.insufficient_overlap {
            continue;
        }
        let (Some(&vi), Some(&ai)) = (vid_id.get(cell.video.as_str()), aud_id.get(cell.audio.as_str()))
        else {
            continue;
        };
        req.push((vi, ai, cell.m.lag_ms));
        req_cells.push(cell);
    }

    eprintln!(
        "verify-debug: FULL matrix, z-gate BYPASSED — {} plausibly-overlapping cell(s) of {} total",
        req.len(),
        matrix.len()
    );
    eprintln!(
        "verify-debug cols: video x audio | z lag_ms overlap_s locked/expected lock_fraction rms_ms ppm clamped is_match"
    );
    if req.is_empty() {
        return Ok(());
    }
    let fits = ctx.verify_batch(&req, sp)?;
    for (cell, fit) in req_cells.iter().zip(fits) {
        match fit {
            None => eprintln!(
                "  {} x {} | z {:.1} lag {:.0} — below verify_min_overlap_s ({:.0} s), no fit",
                cell.video, cell.audio, cell.m.z, cell.m.lag_ms, sp.min_overlap_s
            ),
            Some(f) => eprintln!(
                "  {} x {} | z {:.1} lag {:.0} overlap {:.1} locked {}/{} frac {:.2} rms {:.2} ppm {:.0} clamped {} is_match {}",
                cell.video,
                cell.audio,
                cell.m.z,
                cell.m.lag_ms,
                f.overlap_s,
                f.locked,
                f.expected,
                f.lock_fraction,
                f.rms_ms,
                f.ppm,
                if f.clamped { "Y" } else { "n" },
                if couple_core::verify::is_match(&f) { "YES" } else { "no" },
            ),
        }
    }
    Ok(())
}

/// READ-ONLY `--matcher-debug` table (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.3). Prints the
/// per-(video, audio) spectral evidence to STDERR with the §14.3 columns: method, class, lag, ppm,
/// support, inlier%, coverage%, p95, score, margin. Opt-in; adds nothing to the serialized report
/// and changes no decision (§14.4 — the normal queue-row state vocabulary is untouched). `p95` is
/// `--` when the residual is absent (no fittable hypothesis).
fn matcher_debug_dump(matcher: MatcherChoice, cells: &[Vec<EvidenceCell>]) {
    let mode = match matcher {
        MatcherChoice::Shadow => "shadow",
        MatcherChoice::Fingerprint => "fingerprint",
        MatcherChoice::Legacy => "legacy", // unreachable (debug only runs when evidence computed)
    };
    let total: usize = cells.iter().map(|r| r.len()).sum();
    eprintln!(
        "matcher-debug ({mode}): spectral PairEvidence over {} cell(s) — OBSERVATIONAL, rows still resolved by legacy",
        total
    );
    eprintln!(
        "matcher-debug cols: video x audio | method class lag_ms ppm support inlier% coverage% p95_ms score margin"
    );
    let p95 = |v: Option<f64>| match v {
        Some(x) => format!("{x:.1}"),
        None => "--".to_string(),
    };
    let method = |m: couple_core::matching::MatchMethod| match m {
        couple_core::matching::MatchMethod::SpectralLandmarkV1 => "spectral",
        couple_core::matching::MatchMethod::EnvelopeV1 => "envelope",
    };
    let class = |c: couple_core::matching::MatchClassification| {
        use couple_core::matching::MatchClassification::*;
        match c {
            Match => "match",
            NoMatch => "no-match",
            Ambiguous => "ambiguous",
            InsufficientOverlap => "insuff-overlap",
            Unavailable => "unavailable",
        }
    };
    for row in cells {
        for cell in row {
            let e = &cell.evidence;
            eprintln!(
                "  {:<24} x {:<20} | {:<8} {:<14} {:>10.1} {:>7.0} {:>7} {:>6.1} {:>8.1} {:>7} {:>8.3} {:>6.3}",
                cell.video,
                cell.audio,
                method(e.method),
                class(e.classification),
                e.lag_ms,
                e.drift_ppm,
                e.support,
                e.inlier_fraction * 100.0,
                e.temporal_coverage * 100.0,
                p95(e.residual_p95_ms),
                e.score,
                e.score_margin,
            );
        }
    }
}

/// §16.2 fallback visibility: under fingerprint-primary, list every cell that FELL BACK to the
/// legacy `EnvelopeV1` matcher (its fingerprint was unavailable — artifact build/read failure or an
/// unsupported internal version). "Method + reason must be visible in debug output" (§16.2). The
/// fingerprint matcher does NOT fall back merely because it reports `NoMatch` — a deliberate
/// fingerprint rejection stands (so a `noMatch`/`ambiguous` SpectralLandmarkV1 cell is NOT listed).
/// Prints to STDERR only; adds nothing to --json (changes no decision). Empty ⇒ a concise "none".
fn fallback_debug_dump(matcher: MatcherChoice, cells: &[Vec<EvidenceCell>]) {
    if matcher != MatcherChoice::Fingerprint {
        return;
    }
    let fallbacks: Vec<&EvidenceCell> = cells
        .iter()
        .flatten()
        .filter(|c| c.evidence.method == MatchMethod::EnvelopeV1)
        .collect();
    eprintln!(
        "matcher-debug (fallback §16.2): {} cell(s) fell back to the legacy EnvelopeV1 matcher \
         (fingerprint unavailable: artifact build/read failure or unsupported version)",
        fallbacks.len()
    );
    for c in fallbacks {
        eprintln!(
            "  fallback: {} x {} | method envelope class {} (reason: fingerprint artifact unavailable)",
            c.video,
            c.audio,
            match c.evidence.classification {
                MatchClassification::Match => "match",
                MatchClassification::NoMatch => "no-match",
                MatchClassification::Ambiguous => "ambiguous",
                MatchClassification::InsufficientOverlap => "insuff-overlap",
                MatchClassification::Unavailable => "unavailable",
            },
        );
    }
}

fn import_quiet(
    ctx: &Arc<Ctx>,
    path: &Path,
    role: couple_core::TrackRole,
    quiet: bool,
    progress_json: bool,
    cancel: &CancelToken,
) -> Result<u64> {
    let reporter = crate::progress::Reporter::new(quiet, progress_json, path);
    let job = ctx.next_id();
    let id = ctx.import_blocking(path, role, reporter.progress_fn(), cancel, job)?;
    reporter.finish();
    Ok(id)
}

fn coverage(dur_video: f64, dur_audio: f64, lag_ms: f64) -> Coverage {
    let o = overlap_info(dur_video, dur_audio, lag_ms, 0.0);
    Coverage {
        t0: o.t0,
        t1: o.t1,
        seconds: o.seconds,
        fraction: o.fraction_of_video,
    }
}

/// §3.3 resolution for one video. `cands` = (audio index, measurement) with z ≥ floor.
/// `overflow` (Phase-4 §9.2) is `Some` only when this row's eligible candidates exceeded a
/// resource ceiling, in which case the row is forced to honest `ambiguous` (no subset was
/// verified) with every candidate name still visible.
#[allow(clippy::too_many_arguments)]
fn resolve_row(
    ctx: &Arc<Ctx>,
    params: &PairParams,
    sp: &PairingParams,
    video: &PoolTrack,
    cands: &[(usize, PairCorrelation)],
    overflow: Option<CandidateOverflow>,
    audios: &[PoolTrack],
    audio_ids: &[u64],
    ab_memo: &mut HashMap<(usize, usize), PairCorrelation>,
) -> Result<Row> {
    let mk = |ai: usize, m: &PairCorrelation| RowCandidate {
        audio: audios[ai].file.clone(),
        z: m.z,
        lag_ms: m.lag_ms,
        coverage: coverage(video.duration_s, audios[ai].duration_s, m.lag_ms),
    };
    let row = |state: RowState,
               selected: Option<RowCandidate>,
               alternates: Vec<RowCandidate>,
               resolution: ResolutionDetail| Row {
        video: video.file.clone(),
        chip: chip_text(state, &selected, video.duration_s),
        state,
        selected,
        alternates,
        resolution,
    };
    let detail = |margin, clusters, triangles, note: Option<String>| ResolutionDetail {
        margin,
        clusters,
        triangles,
        note,
        candidate_overflow: None,
    };

    if video.has_audio == Some(false) {
        return Ok(row(
            RowState::NoMatch,
            None,
            Vec::new(),
            detail(None, 0, Vec::new(), Some("no audio stream".into())),
        ));
    }

    // Resource ceiling exceeded (§9.2): NEVER a silent eviction. We verified no subset, so
    // we cannot confidently select — the row is `ambiguous` with all candidate names listed
    // and the overflow detail (eligible count + configured limit) for the user/calibration.
    if let Some(ov) = overflow {
        let alternates: Vec<RowCandidate> = cands.iter().map(|(ai, m)| mk(*ai, m)).collect();
        return Ok(row(
            RowState::Ambiguous,
            None,
            alternates,
            ResolutionDetail {
                margin: None,
                clusters: 0,
                triangles: Vec::new(),
                note: Some(format!(
                    "candidate overflow: {} eligible exceed the {} {} verify limit",
                    ov.eligible, ov.limit, ov.scope
                )),
                candidate_overflow: Some(ov),
            },
        ));
    }

    // Candidates sorted by z desc; ties by file name — deterministic, clock-free
    // (timestamps MUST NOT order a pairing decision, SPEC §1.4).
    let mut c: Vec<(usize, &PairCorrelation)> = cands.iter().map(|(ai, m)| (*ai, m)).collect();
    c.sort_by(|a, b| b.1.z.total_cmp(&a.1.z).then_with(|| audios[a.0].file.cmp(&audios[b.0].file)));

    match c.len() {
        0 => Ok(row(
            RowState::NoMatch,
            None,
            Vec::new(),
            detail(None, 0, Vec::new(), None),
        )),
        1 => {
            let (ai, m) = c[0];
            let sel = mk(ai, m);
            let state = if sel.coverage.fraction >= params.coverage_full {
                RowState::Confident
            } else {
                RowState::Partial
            };
            Ok(row(state, Some(sel), Vec::new(), detail(None, 1, Vec::new(), None)))
        }
        _ => {
            // Cluster coverage intervals into connected components by overlap
            // (§3.3 disjoint/overlapping discrimination, generalized past 2).
            let covs: Vec<Coverage> = c
                .iter()
                .map(|(ai, m)| coverage(video.duration_s, audios[*ai].duration_s, m.lag_ms))
                .collect();
            let clusters = cluster_intervals(&covs, params.disjoint_tol_s);

            // Resolve each cluster to a winner (index into `c`), or None = ambiguous.
            let mut triangles: Vec<TriangleCheck> = Vec::new();
            let mut margin_out: Option<f64> = None;
            let mut winners: Vec<(usize, bool)> = Vec::new(); // (idx in c, simultaneous-cluster)
            let mut ambiguous = false;
            for cluster in &clusters {
                if cluster.len() == 1 {
                    winners.push((cluster[0], false));
                    continue;
                }
                // §3.4 simultaneity over every pair in the cluster.
                let mut all_simultaneous = true;
                for (i, &ci) in cluster.iter().enumerate() {
                    for &cj in &cluster[i + 1..] {
                        let (ai, ma) = c[ci];
                        let (aj, mb) = c[cj];
                        let ab = correlate_audio_pair(ctx, audio_ids, ai, aj, sp, ab_memo)?;
                        let residual = (mb.lag_ms - ma.lag_ms - ab.lag_ms).abs();
                        let simultaneous =
                            ab.z >= params.z_min && residual <= params.triangle_tol_ms;
                        all_simultaneous &= simultaneous;
                        triangles.push(TriangleCheck {
                            audio_a: audios[ai].file.clone(),
                            audio_b: audios[aj].file.clone(),
                            ab_z: ab.z,
                            ab_lag_ms: ab.lag_ms,
                            residual_ms: residual,
                            simultaneous,
                        });
                    }
                }
                if all_simultaneous {
                    // Genuine alternates: best z wins, cluster is `2 sources`.
                    winners.push((cluster[0], true));
                } else {
                    // Conflict (different takes?): §3.5 margin rule, biased to
                    // `ambiguous` over a coin-flip (V2-F2).
                    let z_best = c[cluster[0]].1.z;
                    let z_second = c[cluster[1]].1.z;
                    let margin = z_best - z_second;
                    margin_out = Some(margin);
                    if z_second < params.z_min || margin >= params.z_margin {
                        winners.push((cluster[0], false));
                    } else {
                        ambiguous = true;
                    }
                }
            }

            if ambiguous {
                let alternates: Vec<RowCandidate> = c.iter().map(|(ai, m)| mk(*ai, m)).collect();
                return Ok(row(
                    RowState::Ambiguous,
                    None,
                    alternates,
                    detail(margin_out, clusters.len(), triangles, None),
                ));
            }

            // Pick the row selection among cluster winners: disjoint clusters ⇒
            // recorder restart ⇒ max coverage (§3.3); ties by z then file name.
            let (sel_pos, _) = winners
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    let ca = &covs[a.0];
                    let cb = &covs[b.0];
                    ca.seconds
                        .total_cmp(&cb.seconds)
                        .then_with(|| c[a.0].1.z.total_cmp(&c[b.0].1.z))
                        .then_with(|| audios[c[b.0].0].file.cmp(&audios[c[a.0].0].file))
                })
                .expect("non-empty winners");
            let (sel_idx, sel_simul) = winners[sel_pos];
            let sel = mk(c[sel_idx].0, c[sel_idx].1);
            let alternates: Vec<RowCandidate> = c
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != sel_idx)
                .map(|(_, (ai, m))| mk(*ai, m))
                .collect();
            let state = if clusters.len() > 1 {
                RowState::Split
            } else if sel_simul {
                RowState::TwoSources
            } else if sel.coverage.fraction >= params.coverage_full {
                RowState::Confident
            } else {
                RowState::Partial
            };
            Ok(row(state, Some(sel), alternates, detail(margin_out, clusters.len(), triangles, None)))
        }
    }
}

// ======================= boundary-13 fingerprint resolver (§13.4) =======================
//
// The DEFAULT resolver: rows come from core `PairEvidence` (the graph-adjusted matrix from
// `Ctx::match_candidates_graph`). `classification` is the SOLE identity authority — a cell is a
// candidate iff `classification == Match` (NO z-gate / acceptance threshold is re-derived here,
// invariant 8). Coverage clustering, disjoint/split/2-sources discrimination, the §3.4 simultaneity
// triangle, the §3.5 margin rule, and user override stay shell policy. The audio-audio triangle's
// raw `z` is replaced by the core `match_graph::triangle_closes` predicate over AFFINE evidence +
// composed-map residuals (§13.4): the A↔B edge's core `classification` decides identity and the
// composed V→A→B map must reproduce V→B at start/mid/end. `score` gives the deterministic order.

/// The effective (lag_ms, drift_ppm) a `PairEvidence` cell describes — the REFINED mapping when
/// boundary 7 populated it, else the consensus mapping (§13.1). Both finite (§20).
fn evidence_lag_drift(e: &PairEvidence) -> (f64, f64) {
    (e.refined_lag_ms.unwrap_or(e.lag_ms), e.refined_drift_ppm.unwrap_or(e.drift_ppm))
}

/// §3.3 resolution for one video from core `PairEvidence` cells (the boundary-13 default, §13.4).
/// `cells` = (audio index, evidence) for THIS video, one per audio. Candidacy is core's
/// `classification == Match` (the identity authority); everything else is the same shell coverage /
/// cluster / triangle / margin policy as the legacy `resolve_row`. `overflow` forces honest
/// ambiguity exactly as the legacy path (the resource-ceiling guard is matcher-independent).
#[allow(clippy::too_many_arguments)]
fn resolve_row_fp(
    ctx: &Arc<Ctx>,
    params: &PairParams,
    sp: &PairingParams,
    video: &PoolTrack,
    cells: &[(usize, PairEvidence)],
    overflow: Option<CandidateOverflow>,
    audios: &[PoolTrack],
    audio_ids: &[u64],
    ab_memo: &mut HashMap<(usize, usize), PairEvidence>,
) -> Result<Row> {
    // A row candidate's displayed `z` carries the spectral SCORE (the deterministic ranking lever),
    // and `lag_ms` the effective (refined) lag — coverage uses ppm = 0 like the legacy `coverage()`
    // so the §3.2 video-timeline span is identical to the legacy surface.
    let mk = |ai: usize, e: &PairEvidence| RowCandidate {
        audio: audios[ai].file.clone(),
        z: e.score,
        lag_ms: evidence_lag_drift(e).0,
        coverage: coverage(video.duration_s, audios[ai].duration_s, evidence_lag_drift(e).0),
    };
    let row = |state: RowState,
               selected: Option<RowCandidate>,
               alternates: Vec<RowCandidate>,
               resolution: ResolutionDetail| Row {
        video: video.file.clone(),
        chip: chip_text(state, &selected, video.duration_s),
        state,
        selected,
        alternates,
        resolution,
    };
    let detail = |margin, clusters, triangles, note: Option<String>| ResolutionDetail {
        margin,
        clusters,
        triangles,
        note,
        candidate_overflow: None,
    };

    if video.has_audio == Some(false) {
        return Ok(row(
            RowState::NoMatch,
            None,
            Vec::new(),
            detail(None, 0, Vec::new(), Some("no audio stream".into())),
        ));
    }

    // Resource ceiling exceeded (§9.2): identical honest-ambiguity behavior to the legacy resolver.
    if let Some(ov) = overflow {
        let alternates: Vec<RowCandidate> = cells.iter().map(|(ai, e)| mk(*ai, e)).collect();
        return Ok(row(
            RowState::Ambiguous,
            None,
            alternates,
            ResolutionDetail {
                margin: None,
                clusters: 0,
                triangles: Vec::new(),
                note: Some(format!(
                    "candidate overflow: {} eligible exceed the {} {} verify limit",
                    ov.eligible, ov.limit, ov.scope
                )),
                candidate_overflow: Some(ov),
            },
        ));
    }

    // CANDIDACY = core's identity verdict: keep only `Match` cells (invariant 8 — no re-derived
    // threshold). `Ambiguous`/`NoMatch`/`InsufficientOverlap`/`Unavailable` are NOT row candidates;
    // a deliberate fingerprint rejection is honored (§16.2 — the weaker matcher never overrides it).
    let mut c: Vec<(usize, &PairEvidence)> = cells
        .iter()
        .filter(|(_, e)| e.classification == MatchClassification::Match)
        .map(|(ai, e)| (*ai, e))
        .collect();
    // Deterministic order: score desc, ties by audio file name (clock-free, SPEC §1.4).
    c.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then_with(|| audios[a.0].file.cmp(&audios[b.0].file)));

    match c.len() {
        0 => Ok(row(RowState::NoMatch, None, Vec::new(), detail(None, 0, Vec::new(), None))),
        1 => {
            let (ai, e) = c[0];
            let sel = mk(ai, e);
            let state = if sel.coverage.fraction >= params.coverage_full {
                RowState::Confident
            } else {
                RowState::Partial
            };
            Ok(row(state, Some(sel), Vec::new(), detail(None, 1, Vec::new(), None)))
        }
        _ => {
            let covs: Vec<Coverage> = c
                .iter()
                .map(|(ai, e)| coverage(video.duration_s, audios[*ai].duration_s, evidence_lag_drift(e).0))
                .collect();
            let clusters = cluster_intervals(&covs, params.disjoint_tol_s);

            let mut triangles: Vec<TriangleCheck> = Vec::new();
            let mut margin_out: Option<f64> = None;
            let mut winners: Vec<(usize, bool)> = Vec::new();
            let mut ambiguous = false;
            // The §3.4 closure window is the video coverage span (the residual time window in V's
            // clock); the §13.3 tolerances come from the core PairingParams.
            let tri_offset_tol = sp.graph_offset_tol_ms;
            let tri_drift_tol = sp.graph_drift_tol_ppm;
            for cluster in &clusters {
                if cluster.len() == 1 {
                    winners.push((cluster[0], false));
                    continue;
                }
                // §3.4 simultaneity over every pair in the cluster — the AFFINE composed-map check
                // (replaces the raw-`z` triangle, §13.4).
                let mut all_simultaneous = true;
                for (i, &ci) in cluster.iter().enumerate() {
                    for &cj in &cluster[i + 1..] {
                        let (ai, ea) = c[ci];
                        let (aj, eb) = c[cj];
                        let ab = match_audio_pair_evidence(ctx, audio_ids, ai, aj, sp, ab_memo)?;
                        // V→A, V→B from the row cells; A→B from the audio-audio evidence. The
                        // closure window is V's coverage; A↔B identity is core's classification.
                        let va = couple_core::match_graph::evidence_map(ea);
                        let vb = couple_core::match_graph::evidence_map(eb);
                        let ab_map = couple_core::match_graph::evidence_map(&ab);
                        let simultaneous = couple_core::match_graph::triangle_closes(
                            va, ab_map, vb, ab.classification, video.duration_s,
                            tri_offset_tol, tri_drift_tol,
                        );
                        all_simultaneous &= simultaneous;
                        // The triangle record keeps the start/mid/end composed-residual MAX (ms) and
                        // surfaces the A↔B core score in `ab_z` for calibration read-out.
                        let residual = composed_residual_max_ms(va, ab_map, vb, video.duration_s);
                        triangles.push(TriangleCheck {
                            audio_a: audios[ai].file.clone(),
                            audio_b: audios[aj].file.clone(),
                            ab_z: ab.score,
                            ab_lag_ms: evidence_lag_drift(&ab).0,
                            residual_ms: residual,
                            simultaneous,
                        });
                    }
                }
                if all_simultaneous {
                    winners.push((cluster[0], true));
                } else {
                    // Conflict (different takes registering the same span): §3.5 margin rule on the
                    // spectral SCORE, biased to `ambiguous` over a coin-flip (V2-F2).
                    let z_best = c[cluster[0]].1.score;
                    let z_second = c[cluster[1]].1.score;
                    let margin = z_best - z_second;
                    margin_out = Some(margin);
                    if margin >= params.z_margin {
                        winners.push((cluster[0], false));
                    } else {
                        ambiguous = true;
                    }
                }
            }

            if ambiguous {
                let alternates: Vec<RowCandidate> = c.iter().map(|(ai, e)| mk(*ai, e)).collect();
                return Ok(row(
                    RowState::Ambiguous,
                    None,
                    alternates,
                    detail(margin_out, clusters.len(), triangles, None),
                ));
            }

            let (sel_pos, _) = winners
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    let ca = &covs[a.0];
                    let cb = &covs[b.0];
                    ca.seconds
                        .total_cmp(&cb.seconds)
                        .then_with(|| c[a.0].1.score.total_cmp(&c[b.0].1.score))
                        .then_with(|| audios[c[b.0].0].file.cmp(&audios[c[a.0].0].file))
                })
                .expect("non-empty winners");
            let (sel_idx, sel_simul) = winners[sel_pos];
            let sel = mk(c[sel_idx].0, c[sel_idx].1);
            let alternates: Vec<RowCandidate> = c
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != sel_idx)
                .map(|(_, (ai, e))| mk(*ai, e))
                .collect();
            let state = if clusters.len() > 1 {
                RowState::Split
            } else if sel_simul {
                RowState::TwoSources
            } else if sel.coverage.fraction >= params.coverage_full {
                RowState::Confident
            } else {
                RowState::Partial
            };
            Ok(row(state, Some(sel), alternates, detail(margin_out, clusters.len(), triangles, None)))
        }
    }
}

/// The MAX of the start/mid/end composed-offset residuals (ms) between the direct V→B map and the
/// composed V→A→B map — the calibration read-out behind the §3.4 closure (the affine analogue of
/// the legacy scalar `|lag_vb − lag_va − lag_ab|`). `f64::INFINITY` would never be serialized; we
/// return a finite sentinel (`0.0`) when a map is missing so the row JSON stays finite (§20).
fn composed_residual_max_ms(
    va: Option<couple_core::match_graph::AffineMap>,
    ab: Option<couple_core::match_graph::AffineMap>,
    vb: Option<couple_core::match_graph::AffineMap>,
    coverage_s: f64,
) -> f64 {
    let (Some(va), Some(ab), Some(vb)) = (va, ab, vb) else {
        return 0.0;
    };
    let Some(composed) = va.compose(ab) else {
        return 0.0;
    };
    let t1 = if coverage_s.is_finite() && coverage_s > 0.0 { coverage_s } else { 1.0 };
    couple_core::match_graph::CycleResidual::between(&vb, &composed, 0.0, t1).max_offset_ms()
}

/// Audio-audio `PairEvidence` for the §3.4 triangle, memoized with antisymmetric lag/drift
/// (L(B,A) = −L(A,B)). Uses `match_pair` (the same core path the matrix used, so the A↔B identity
/// `classification` is consistent with the V→A / V→B cells). Keyed by the unordered audio pair.
fn match_audio_pair_evidence(
    ctx: &Arc<Ctx>,
    audio_ids: &[u64],
    a: usize,
    b: usize,
    sp: &PairingParams,
    memo: &mut HashMap<(usize, usize), PairEvidence>,
) -> Result<PairEvidence> {
    let key = (a.min(b), a.max(b));
    if !memo.contains_key(&key) {
        let e = ctx.match_pair(audio_ids[key.0], audio_ids[key.1], sp)?;
        memo.insert(key, e);
    }
    let mut e = memo[&key].clone();
    if a > b {
        // Antisymmetric orientation: negate the (refined) lag so V→A→B composes correctly when the
        // memo holds the (lo, hi) orientation but the caller asked for (hi, lo).
        e.lag_ms = -e.lag_ms;
        e.drift_ppm = -e.drift_ppm;
        if let Some(l) = e.refined_lag_ms.as_mut() {
            *l = -*l;
        }
        if let Some(d) = e.refined_drift_ppm.as_mut() {
            *d = -*d;
        }
    }
    Ok(e)
}

/// A×B stage-1, memoized with antisymmetric lag (L(B,A) = −L(A,B)).
fn correlate_audio_pair(
    ctx: &Arc<Ctx>,
    audio_ids: &[u64],
    a: usize,
    b: usize,
    sp: &PairingParams,
    memo: &mut HashMap<(usize, usize), PairCorrelation>,
) -> Result<PairCorrelation> {
    let key = (a.min(b), a.max(b));
    if !memo.contains_key(&key) {
        let m = ctx.pair_correlate(audio_ids[key.0], audio_ids[key.1], sp)?;
        memo.insert(key, m);
    }
    let mut m = memo[&key].clone();
    if a > b {
        m.lag_ms = -m.lag_ms;
    }
    Ok(m)
}

/// Connected components of intervals overlapping by more than `tol` seconds,
/// each component sorted by input order (z-descending order of `covs`).
fn cluster_intervals(covs: &[Coverage], tol: f64) -> Vec<Vec<usize>> {
    let n = covs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        if p[i] != i {
            let r = find(p, p[i]);
            p[i] = r;
        }
        p[i]
    }
    for i in 0..n {
        for j in i + 1..n {
            let inter = covs[i].t1.min(covs[j].t1) - covs[i].t0.max(covs[j].t0);
            if inter > tol {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri.max(rj)] = ri.min(rj);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort_by_key(|g| g[0]);
    out
}

fn fmt_mmss(s: f64) -> String {
    let total = s.round() as i64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// §3.3 chip detail, e.g. partial → "audio covers 0:00–18:40 of 23:55".
fn chip_text(state: RowState, selected: &Option<RowCandidate>, dur_video: f64) -> String {
    match (state, selected) {
        (RowState::Partial, Some(s)) | (RowState::Split, Some(s)) => format!(
            "{} — audio covers {}–{} of {}",
            state.chip(),
            fmt_mmss(s.coverage.t0),
            fmt_mmss(s.coverage.t1),
            fmt_mmss(dur_video)
        ),
        _ => state.chip().to_string(),
    }
}

fn print_human(r: &PairReport) {
    if !r.skipped.is_empty() {
        println!("skipped {} unrecognized file(s): {}", r.skipped.len(), r.skipped.join(", "));
    }
    println!(
        "pool: {} video(s), {} audio file(s)",
        r.videos.len(),
        r.audios.len()
    );
    if !r.matrix.is_empty() {
        println!("\ncandidate matrix (z / lag ms; * = candidate, z >= {}):", r.constants.z_min);
        for cell in &r.matrix {
            println!(
                "  {:<28} x {:<24} z {:>7.1}  lag {:>12.1}{}",
                cell.video,
                cell.audio,
                cell.m.z,
                cell.m.lag_ms,
                if cell.candidate { "  *" } else { "" }
            );
        }
    }
    println!("\nrows:");
    for row in &r.rows {
        match &row.selected {
            Some(s) => println!(
                "  {:<28} [{}] -> {} (z {:.1}, lag {:.1} ms, coverage {:.1}%)",
                row.video,
                row.chip,
                s.audio,
                s.z,
                s.lag_ms,
                s.coverage.fraction * 100.0
            ),
            None => println!("  {:<28} [{}]", row.video, row.chip),
        }
        for alt in &row.alternates {
            println!("      alt: {} (z {:.1}, lag {:.1} ms)", alt.audio, alt.z, alt.lag_ms);
        }
    }
    if !r.unused_audio.is_empty() {
        println!("\nunused audio: {}", r.unused_audio.join(", "));
    }
}
