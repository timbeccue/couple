//! `couple pair <dir>` — the headless v2 pairing surface (SPEC-V2 §3/§4).
//!
//! Core owns candidate measurements and row resolution. This layer assembles the
//! imported media pool, passes CLI overrides to the resolver, and projects its
//! rows and diagnostic matrices into the command-line report.
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
use couple_core::resolve::{
    Coverage, ResolutionDetail, ResolvedRow, RowCandidate as CoreRowCandidate, RowState,
};
use couple_core::{CancelToken, CoupleError, Ctx, MediaInfo, PairingParams, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// The CLI parser and resolver share the same matcher choices and errors.
pub use couple_core::MatcherChoice;

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
    /// Phase D (PLAN-CORE-SHELL-BOUNDARY §4): the resolver constants live in ONE place —
    /// `couple_core::resolve::ResolveParams::default()`. The CLI no longer hardcodes the
    /// literals; it projects core's defaults into its own `PairParams` serialization shape.
    /// The `--json` `constants` field is unchanged (same fields/values), so `couple pair --json`
    /// stays byte-identical — only the SOURCE of the default values moved to core.
    fn default() -> Self {
        let r = couple_core::resolve::ResolveParams::default();
        Self {
            z_min: r.z_min,
            z_margin: r.z_margin,
            triangle_tol_ms: r.triangle_tol_ms,
            coverage_full: r.coverage_full,
            disjoint_tol_s: r.disjoint_tol_s,
            verify_active: r.verify_active,
            pairing: r.pairing,
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

/// One candidate audio source for a video (the CLI's view of core's [`CoreRowCandidate`]).
/// The §3.3 resolution now lives in `couple_core::resolve`; this layer projects each core
/// candidate to the CLI wire shape, which serializes the file name as `audio` (the core type
/// carries BOTH `audioKey` and `audioName` for cross-shell use — the CLI surfaces only the
/// name). `coverage` reuses core's byte-identical [`Coverage`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RowCandidate {
    audio: String,
    z: f64,
    lag_ms: f64,
    coverage: Coverage,
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
    /// The §3.4/§3.5 honesty trail — reused verbatim from core (byte-identical serde).
    resolution: ResolutionDetail,
}

impl RowCandidate {
    /// Project a core [`CoreRowCandidate`] to the CLI wire shape: the CLI surfaces the audio
    /// FILE NAME as `audio` (dropping the content key the core type also carries). `z`/`lagMs`/
    /// `coverage` carry over unchanged.
    fn from_core(c: CoreRowCandidate) -> Self {
        RowCandidate {
            audio: c.audio_name,
            z: c.z,
            lag_ms: c.lag_ms,
            coverage: c.coverage,
        }
    }
}

impl Row {
    /// Project a core [`ResolvedRow`] to the CLI `Row`: `video` is the video FILE NAME (the
    /// core type also carries `videoKey`); `state`/`chip`/`resolution` carry over verbatim
    /// (reused core types, byte-identical serde); `selected`/`alternates` map to the CLI
    /// `RowCandidate` (file name as `audio`).
    fn from_core(rr: ResolvedRow) -> Self {
        Row {
            video: rr.video_name,
            state: rr.state,
            chip: rr.chip,
            selected: rr.selected.map(RowCandidate::from_core),
            alternates: rr.alternates.into_iter().map(RowCandidate::from_core).collect(),
            resolution: rr.resolution,
        }
    }
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
    // The legacy candidate `matrix` field (the per-cell `candidate` boolean is read by
    // gate4/gate7). RESOLUTION no longer reads `per_video` — it is the core resolver's job
    // (`ctx.resolve` below); this loop only builds the display matrix.
    let mut matrix: Vec<MatrixCell> = Vec::new();
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
                // §7.2), consumed not re-derived (invariant 8). Flag OFF: the unchanged default
                // z-gate against the RESOLVER's own `z_min` (a resolution constant distinct from
                // the pairing `legacy_z_min`/`legacy_candidate`, so `--params z-min` still
                // tightens it; the resolver applies the identical gate internally, §7.2).
                let candidate = if params.verify_active {
                    m.prefilter_candidate
                } else {
                    !m.insufficient_overlap && m.z >= params.z_min
                };
                matrix.push(MatrixCell {
                    video: videos[vi].file.clone(),
                    audio: audios[ai].file.clone(),
                    m,
                    candidate,
                });
            }
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
    // §16.1), the SHADOW/FINGERPRINT DISPLAY field. Computed for `shadow` (observational) AND
    // `fingerprint`. For `fingerprint` it is the GRAPH-ADJUSTED matrix under the REAL `sp`
    // (FingerprintPrimary by default); for `shadow` it is a separate FingerprintShadow computation
    // attached for comparison. In `legacy` mode this is skipped ⇒ `evidenceMatrix` is None and the
    // output is byte-identical to the legacy surface.
    //
    // NOTE (perf follow-up): in fingerprint mode the GRAPH-ADJUSTED matrix is computed here for the
    // `evidenceMatrix` display AND AGAIN inside `ctx.resolve` (which builds its own matrix to resolve
    // the rows). This double-compute is correct + byte-identical; a later phase can thread one matrix
    // through both. Sharing it now would change the resolver's public entry point, out of scope here.
    //
    // `importable` (videos with an audio stream) maps row-for-row onto the matrix; audio-less videos
    // are absent (their row is an honest `no match`, woven in shell-side after `ctx.resolve`).
    let evidence_matrix: Option<Vec<Vec<EvidenceCell>>> = if matches!(
        matcher, MatcherChoice::Shadow | MatcherChoice::Fingerprint
    ) {
        let ev = if matcher == MatcherChoice::Fingerprint {
            // The DEFAULT (§16.1): the GRAPH-ADJUSTED matrix under the real params (FingerprintPrimary
            // + profile 2). §16.2 fallback (artifact build/read failure, unsupported fingerprint
            // version, or a forced legacy override) is handled INSIDE core (`match_candidates_graph`
            // → `match_candidates` falls a cell back to `EnvelopeV1` when its fingerprint is
            // unavailable; the method/reason is visible in the cell + the debug table below).
            ctx.match_candidates_graph(&importable, &audio_ids, &sp)?
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

    // 4. Resolution (§3.3) — now the CORE resolver's job (PLAN-CORE-SHELL-BOUNDARY Phase B). The
    // §3.3 coverage clustering / disjoint discrimination / §3.4 triangle / §3.5 margin rule / row
    // states all moved to `couple_core::resolve`; this layer maps the CLI params in, calls
    // `ctx.resolve`, and projects the result back onto the CLI wire `Row`.
    let t_resolve = std::time::Instant::now();

    // Build `ResolveParams` from the CLI `PairParams`. `rms_escalation = false` is CRITICAL: the CLI
    // legacy resolver NEVER escalated, so keeping it off makes the legacy output byte-identical (the
    // rms_* knobs below are unused when escalation is off — they take the core defaults). The CLI
    // `--params` path already wrote every threshold into `params`, so we copy fields directly.
    let resolve_params = couple_core::ResolveParams {
        pairing: params.pairing.clone(),
        matcher,
        z_min: params.z_min,
        z_margin: params.z_margin,
        triangle_tol_ms: params.triangle_tol_ms,
        coverage_full: params.coverage_full,
        disjoint_tol_s: params.disjoint_tol_s,
        verify_active: params.verify_active,
        rms_escalation: false,
        ..couple_core::ResolveParams::default()
    };

    // `ctx.resolve` is IMPORTED-TRACK ONLY — it sees only videos with an analyzable audio stream
    // (`importable`), one resolved row per importable video in order. Audio-less videos are absent.
    let resolved = ctx.resolve(&importable, &audio_ids, &resolve_params)?;

    // WEAVE the core rows back onto the FULL video list IN ORDER: an audio-less video (has_audio ==
    // Some(false)) gets an inert `no match` row constructed shell-side (matching the resolver's own
    // has_audio-false output exactly); every other video consumes the next core `ResolvedRow`.
    let mut rows: Vec<Row> = Vec::with_capacity(videos.len());
    let mut resolved_iter = resolved.into_iter();
    for v in &videos {
        cancel.check()?;
        if v.has_audio == Some(false) {
            rows.push(Row {
                video: v.file.clone(),
                state: RowState::NoMatch,
                chip: RowState::NoMatch.chip().to_string(),
                selected: None,
                alternates: Vec::new(),
                resolution: ResolutionDetail {
                    margin: None,
                    clusters: 0,
                    triangles: Vec::new(),
                    note: Some("no audio stream".into()),
                    candidate_overflow: None,
                },
            });
        } else {
            let rr = resolved_iter.next().expect("a resolved row per importable video");
            rows.push(Row::from_core(rr));
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
