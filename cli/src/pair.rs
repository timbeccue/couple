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
}

impl Default for PairParams {
    fn default() -> Self {
        Self {
            z_min: 8.5,
            z_margin: 6.0,
            triangle_tol_ms: 250.0,
            coverage_full: 0.98,
            disjoint_tol_s: 0.5,
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

pub fn run_pair(
    json: bool,
    quiet: bool,
    progress_json: bool,
    ctx: &Arc<Ctx>,
    dir: &Path,
    params: &PairParams,
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
                let candidate = !m.insufficient_overlap && m.z >= params.z_min;
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
    let matrix_ms = t_matrix.elapsed().as_secs_f64() * 1000.0;

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
    couple_core::sync::SyncParams {
        z_min: p.z_min,
        ..Default::default()
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
