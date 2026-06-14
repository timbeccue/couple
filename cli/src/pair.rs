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

use couple_core::pair::PairCorrelation;
use couple_core::sync::overlap_info;
use couple_core::{CancelToken, CoupleError, Ctx, MediaInfo, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

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

    // ---- match classifier (PLAN-MATCH-CLASSIFIER §4.2/§5) ----
    // The fine waveform-lock classifier knobs, mirroring the core SyncParams defaults
    // so BOTH resolvers share identical thresholds (the gates run the CLI). `verify_active`
    // is the single Phase-1↔Phase-2 boundary switch, default OFF: flag OFF ⇒ candidacy is
    // exactly today's z-gate (byte-identical); flag ON ⇒ the loose pre-filter + top-K cap +
    // `is_match` classifier replaces the z-gate candidacy. These NEVER enter a serialized
    // `Row`/`MatrixCell` (§3 / gate6); they are forwarded to `verify_batch` via `sync_params`.
    /// Phase-2 master switch [PHASE-2, D2 default OFF].
    pub verify_active: bool,
    pub verify_min_overlap_s: f64,
    pub verify_win_frac: f64,
    pub verify_half_win_min_s: f64,
    pub verify_density_per_s: f64,
    pub verify_min_windows_floor: usize,
    pub verify_max_windows: usize,
    pub verify_max_candidates_per_video: usize,
    pub verify_span_cap_ms: f64,
    pub drift_lambda_s2: f64,
    pub drift_max_ppm: f64,
    pub verify_min_locked: u32,
    pub verify_lock_fraction_min: f64,
    pub verify_rms_max_ms: f64,
    pub verify_prefilter_ncc: f64,
    pub verify_ncc_window_min: f64,
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
            verify_min_overlap_s: 8.0,
            verify_win_frac: 0.5,
            verify_half_win_min_s: 2.5,
            verify_density_per_s: 0.20,
            verify_min_windows_floor: 3,
            verify_max_windows: 24,
            verify_max_candidates_per_video: 6,
            verify_span_cap_ms: 500.0,
            drift_lambda_s2: 50.0,
            drift_max_ppm: 600.0,
            verify_min_locked: 4,
            verify_lock_fraction_min: 0.66,
            verify_rms_max_ms: 12.0,
            verify_prefilter_ncc: 0.15,
            verify_ncc_window_min: 0.25,
        }
    }
}

impl PairParams {
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let v: f64 = value
            .parse()
            .map_err(|_| CoupleError::invalid(format!("--params {key}: not a number")))?;
        match key {
            "z-min" => self.z_min = v,
            "z-margin" => self.z_margin = v,
            "triangle-tol-ms" => self.triangle_tol_ms = v,
            "coverage-full" => self.coverage_full = v,
            "disjoint-tol-s" => self.disjoint_tol_s = v,
            // Match-classifier knobs (PLAN-MATCH-CLASSIFIER §4.2/§5). `verify-active`
            // takes 0/1 (parsed as f64 like the rest; != 0 ⇒ ON).
            "verify-active" => self.verify_active = v != 0.0,
            "verify-min-overlap-s" => self.verify_min_overlap_s = v,
            "verify-win-frac" => self.verify_win_frac = v,
            "verify-half-win-min-s" => self.verify_half_win_min_s = v,
            "verify-density-per-s" => self.verify_density_per_s = v,
            "verify-min-windows-floor" => self.verify_min_windows_floor = v as usize,
            "verify-max-windows" => self.verify_max_windows = v as usize,
            "verify-max-candidates-per-video" => self.verify_max_candidates_per_video = v as usize,
            "verify-span-cap-ms" => self.verify_span_cap_ms = v,
            "drift-lambda-s2" => self.drift_lambda_s2 = v,
            "drift-max-ppm" => self.drift_max_ppm = v,
            "verify-min-locked" => self.verify_min_locked = v as u32,
            "verify-lock-fraction-min" => self.verify_lock_fraction_min = v,
            "verify-rms-max-ms" => self.verify_rms_max_ms = v,
            "verify-prefilter-ncc" => self.verify_prefilter_ncc = v,
            "verify-ncc-window-min" => self.verify_ncc_window_min = v,
            other => return Err(CoupleError::invalid(format!("unknown pair param {other}"))),
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
    cancel: &CancelToken,
) -> Result<()> {
    if !dir.is_dir() {
        return Err(CoupleError::invalid(format!("{} is not a directory", dir.display())));
    }
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
    let sp = sync_params(params);
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
                // unchanged for a given input, so gate6 stays trivially safe). Flag OFF: the
                // today z-gate. Flag ON: the loose Stage-A pre-filter (NCC floor only; `z`
                // no longer rejects) — the fine classifier disposes below.
                let candidate = if params.verify_active {
                    !m.insufficient_overlap && m.ncc >= params.verify_prefilter_ncc
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

    // 3a. Match classifier (PLAN-MATCH-CLASSIFIER §5.2 / Stage B) — flag ON only. The loose
    // pre-filter above can admit O(V×A) cells, so for each video we CAP to the top-K by NCC
    // (ranked by NCC, NOT z — z's √n term is the short-clip penalty this whole change
    // removes), run `verify_batch` over the capped set, and keep ONLY cells that pass
    // `is_match`. That real-match set REPLACES `per_video[vi]` feeding the UNCHANGED §3.3
    // pipeline (clustering / §3.4 / §3.5 / coverage selection). Flag OFF: this block is
    // skipped entirely and `per_video` is exactly the z-passing set — byte-identical to today.
    if params.verify_active {
        for (vi, id) in video_ids.iter().enumerate() {
            let Some(vid) = id else { continue };
            let cands = &mut per_video[vi];
            if cands.is_empty() {
                continue;
            }
            // Cap by NCC desc (overlap-INDEPENDENT resemblance); ties by file name so the
            // truncation is deterministic and clock-free (SPEC §1.4).
            cands.sort_by(|a, b| {
                b.1.ncc
                    .total_cmp(&a.1.ncc)
                    .then_with(|| audios[a.0].file.cmp(&audios[b.0].file))
            });
            cands.truncate(params.verify_max_candidates_per_video);

            // One batched verify over the capped candidate pairs.
            let req: Vec<(u64, u64, f64)> =
                cands.iter().map(|(ai, m)| (*vid, audio_ids[*ai], m.lag_ms)).collect();
            let fits = ctx.verify_batch(&req, &sp)?;
            // Keep only the real matches (is_match). Verify numbers stay OUT of the Row.
            let mut real: Vec<(usize, PairCorrelation)> = Vec::with_capacity(cands.len());
            for ((ai, m), fit) in cands.iter().zip(fits) {
                if let Some(f) = fit {
                    if couple_core::verify::is_match(&f, &sp) {
                        real.push((*ai, m.clone()));
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

    // 4. Resolution (§3.3) per video row.
    let t_resolve = std::time::Instant::now();
    let mut ab_memo: HashMap<(usize, usize), PairCorrelation> = HashMap::new();
    let mut rows: Vec<Row> = Vec::new();
    for (vi, v) in videos.iter().enumerate() {
        cancel.check()?;
        rows.push(resolve_row(
            ctx, params, &sp, v, &per_video[vi], &audios, &audio_ids, &mut ab_memo,
        )?);
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

fn sync_params(p: &PairParams) -> couple_core::sync::SyncParams {
    // Forward the classifier knobs so BOTH the candidate matrix and `verify_batch` see
    // the same thresholds the resolver decides on (`verify_active` itself is a CLI-only
    // switch — SyncParams has no such field; `verify_pair`/`is_match` are pure functions
    // of the numeric thresholds below). Phase-1 forwarded only z_min; Phase 2 needs the
    // rest in `verify_batch` to match the resolver's `is_match`.
    couple_core::sync::SyncParams {
        z_min: p.z_min,
        verify_min_overlap_s: p.verify_min_overlap_s,
        verify_win_frac: p.verify_win_frac,
        verify_half_win_min_s: p.verify_half_win_min_s,
        verify_density_per_s: p.verify_density_per_s,
        verify_min_windows_floor: p.verify_min_windows_floor,
        verify_max_windows: p.verify_max_windows,
        verify_max_candidates_per_video: p.verify_max_candidates_per_video,
        verify_span_cap_ms: p.verify_span_cap_ms,
        drift_lambda_s2: p.drift_lambda_s2,
        drift_max_ppm: p.drift_max_ppm,
        verify_min_locked: p.verify_min_locked,
        verify_lock_fraction_min: p.verify_lock_fraction_min,
        verify_rms_max_ms: p.verify_rms_max_ms,
        verify_prefilter_ncc: p.verify_prefilter_ncc,
        verify_ncc_window_min: p.verify_ncc_window_min,
        ..Default::default()
    }
}

/// READ-ONLY `--verify-debug` dump (PLAN-MATCH-CLASSIFIER §4.5). Runs fine-verify over
/// EVERY matrix cell with a plausible lag — the z-gate is BYPASSED, so the z-rejected
/// short clip (whose row never clears the gate) is shown — and prints a per-candidate
/// table to STDERR. Adds nothing to the serialized Row/MatrixCell, so gate6 byte-stability
/// is untouched. Changes no decision.
#[allow(clippy::too_many_arguments)]
fn verify_debug_dump(
    ctx: &Arc<Ctx>,
    sp: &couple_core::sync::SyncParams,
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
                cell.video, cell.audio, cell.m.z, cell.m.lag_ms, sp.verify_min_overlap_s
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
                if couple_core::verify::is_match(&f, sp) { "YES" } else { "no" },
            ),
        }
    }
    Ok(())
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
#[allow(clippy::too_many_arguments)]
fn resolve_row(
    ctx: &Arc<Ctx>,
    params: &PairParams,
    sp: &couple_core::sync::SyncParams,
    video: &PoolTrack,
    cands: &[(usize, PairCorrelation)],
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
    };

    if video.has_audio == Some(false) {
        return Ok(row(
            RowState::NoMatch,
            None,
            Vec::new(),
            detail(None, 0, Vec::new(), Some("no audio stream".into())),
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

/// A×B stage-1, memoized with antisymmetric lag (L(B,A) = −L(A,B)).
fn correlate_audio_pair(
    ctx: &Arc<Ctx>,
    audio_ids: &[u64],
    a: usize,
    b: usize,
    sp: &couple_core::sync::SyncParams,
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
