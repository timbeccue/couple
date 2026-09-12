//! `couple_core::resolve` — the §3.3 pairing resolver (PLAN-CORE-SHELL-BOUNDARY Phase A).
//!
//! This is the single, cross-platform home for pairing ROW RESOLUTION: coverage
//! clustering, disjoint/overlap discrimination, the §3.4 audio↔audio simultaneity check
//! (triangle closure), the §3.5 margin rule, the §3.3 row states, candidate-overflow
//! handling, and the legacy RMS tie-breaker escalation. It was previously duplicated in
//! `cli/src/pair.rs` (Rust) and `mac/Sources/Couple/Resolver.swift` (Swift); this module
//! is the proven SUPERSET of both. It is a RELOCATION, not a behavior change — the row
//! states / selected audio / lags / clusters / overflow it produces are byte-identical to
//! the two shells' resolvers for identical inputs.
//!
//! Boundary rule (§1): DECISION lives here; PRESENTATION/PLATFORM stays in the shell. The
//! wire types are a SUPERSET carrying BOTH the content key and the file name so each shell
//! can project its own view (the CLI serializes name as `audio`; Swift decodes both).
//! Display-only telemetry (fitRmsMs/lockFraction/expected/ppm/clamped/shadow) is NOT here —
//! it stays shell-side.
//!
//! Honesty posture (V2-F2): conflicting candidates that aren't separable resolve `Ambiguous`
//! with NO selection — never a coin-flip. Orderings are content/name deterministic (SPEC
//! §1.4 — timestamps never order a pairing), so re-running over the same inputs is
//! byte-identical. No serialized field is NaN/Infinity (§20).

use crate::error::{CoupleError, Result};
use crate::job::{noop_progress, CancelToken, Phase, ProgressEmitter};
use crate::matching::{MatchClassification, PairEvidence};
use crate::pair::PairCorrelation;
use crate::pairing_params::PairingParams;
use crate::sync::overlap_info;
use crate::{Ctx, TrackId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================ matcher selection ============================

/// Which recording-identity matcher the resolver runs (== CLI `--matcher`). `Fingerprint`
/// (the default) resolves rows from the core graph-adjusted `PairEvidence` matrix;
/// `Legacy` is the developer-override envelope resolver; `Shadow` resolves with the LEGACY
/// path (its spectral matrix is observational and attached SHELL-SIDE — core resolve does
/// not produce it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatcherChoice {
    /// The developer override: today's envelope resolution, byte-identical output.
    Legacy,
    /// Resolve with LEGACY (the shell attaches the spectral matrix for observation).
    Shadow,
    /// The DEFAULT: resolve rows from the core graph-adjusted `PairEvidence` matrix —
    /// `classification` is the identity authority, `score` the deterministic order.
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

    /// True when the resolved rows come from the spectral `PairEvidence` matrix (the
    /// boundary-13 default). `Legacy`/`Shadow` keep the legacy resolver as the row authority.
    fn resolves_with_fingerprints(self) -> bool {
        matches!(self, Self::Fingerprint)
    }
}

// ============================ resolve params ============================

/// Resolution constants (SPEC-V2 §9) + the Swift RMS tie-breaker knobs. Mirrors the CLI
/// `PairParams` and the Swift `PairParams`. `z_min` is the resolver-level gate, DELIBERATELY
/// SEPARATE from [`PairingParams::legacy_z_min`] (plan §3.1) so `--params z-min` still
/// tightens the legacy row gate / raw-z triangle / margin rule without folding into the
/// pairing params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolveParams {
    /// Core-owned verifier/graph thresholds — the single source of those defaults. Resolver
    /// `--params` keys this struct doesn't own delegate here ([`ResolveParams::set`]).
    pub pairing: PairingParams,
    /// Which matcher resolves the rows (default [`MatcherChoice::Fingerprint`]).
    pub matcher: MatcherChoice,
    /// Pairing floor on stage-1 z [VALIDATED, Session C]; the legacy row gate + raw-z
    /// triangle/margin threshold. SEPARATE from `pairing.legacy_z_min` (plan §3.1).
    pub z_min: f64,
    /// §3.5 decisive-best margin [TUNE, V2-F2].
    pub z_margin: f64,
    /// §3.4 legacy closure tolerance, ms [SPEC, TUNE].
    pub triangle_tol_ms: f64,
    /// Coverage at/above which a single match is `confident` [SPEC, VALIDATED 2026-07-11:
    /// re-frozen on the owner corpus; value-locked by `resolve_params_value_lock`].
    pub coverage_full: f64,
    /// Interval-overlap clustering tolerance, seconds [SPEC, TUNE].
    pub disjoint_tol_s: f64,
    /// The classifier switch (CLI `verify_active` / Swift `verifyClassifierActive`); default
    /// OFF ⇒ candidacy is the legacy z-gate, byte-identical.
    pub verify_active: bool,
    /// The LEGACY-path RMS tie-breaker (PLAN-RMS-TIEBREAKER §5). ONLY consulted on the legacy
    /// path. The CLI legacy resolver NEVER escalated (so the CLI passes `false`); the Swift
    /// legacy resolver DID (so Swift passes `true`). Default `false`.
    pub rms_escalation: bool,
    /// RMS tie-breaker: best fit must lock at/under this (ms) to be trusted.
    pub rms_trust_floor_ms: f64,
    /// RMS tie-breaker: a rival fit must be ≥ this × the leader to be "comparably clean".
    pub rms_ratio: f64,
    /// RMS tie-breaker: fewer accepted windows than this ⇒ don't trust RMS.
    pub min_fit_windows: usize,
    /// The conservative↔strong RMS form switch (mirrors Swift `Resolver.rmsStrongForm`).
    /// Conservative (`false`, the shipped default) confirms the z-leader; strong picks the
    /// single clean fit regardless of z rank.
    pub rms_strong_form: bool,
}

impl Default for ResolveParams {
    fn default() -> Self {
        Self {
            pairing: PairingParams::default(),
            matcher: MatcherChoice::Fingerprint,
            z_min: 8.5,
            z_margin: 6.0,
            triangle_tol_ms: 250.0,
            // SPEC-CHANGE (2026-07-11): SPEC-V2 §9 COVERAGE_FULL 0.98 → 0.95 (realdata-report
            // 2026-07-12 05:07Z): real full-true coverage min 0.9628; the one genuine partial
            // (fixture) measures 0.082 — two-sided clean. z_margin/triangle_tol_ms UNCHANGED
            // (zero true different-take conflicts observed; regime gap RECORDED, V2-F2).
            coverage_full: 0.95,
            disjoint_tol_s: 0.5,
            verify_active: false,
            rms_escalation: false,
            rms_trust_floor_ms: 9.0,
            rms_ratio: 3.0,
            min_fit_windows: 6,
            rms_strong_form: false,
        }
    }
}

impl ResolveParams {
    /// CLI/host `--params k=v` override mirroring the CLI `PairParams::set`: resolver keys
    /// land here, everything else delegates to `pairing.set` so the `--params` path is
    /// unchanged in Phase B.
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
            // ceilings) is core-owned: delegate to PairingParams::set so the resolver and
            // pairing layer can never drift.
            _ => self.pairing.set(key, value)?,
        }
        Ok(())
    }
}

// ============================ wire types (camelCase serde) ============================

/// Video-timeline span the audio covers (§3.2, ppm = 0 arithmetic of §5.6 via
/// `sync::overlap_info`).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub t0: f64,
    pub t1: f64,
    pub seconds: f64,
    pub fraction: f64,
}

/// One candidate audio source for a video. Carries BOTH the pool content key (stable
/// identity) and the file name (the CLI-identical name tie-break, SPEC §1.4). `z` is the
/// stage-1 z on the legacy path and the spectral SCORE on the fingerprint path; `lag_ms` is
/// the effective (refined) lag.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowCandidate {
    pub audio_key: String,
    pub audio_name: String,
    pub z: f64,
    pub lag_ms: f64,
    pub coverage: Coverage,
}

/// §3.4 record for one overlapping audio pair (raw numbers for calibration).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriangleCheck {
    pub audio_a: String,
    pub audio_b: String,
    /// A×B stage-1 z (legacy) or A↔B spectral score (fingerprint).
    pub ab_z: f64,
    pub ab_lag_ms: f64,
    /// |lag(V→B) − lag(V→A) − lag(A→B)| (legacy) or the composed-map MAX offset residual
    /// (fingerprint), ms.
    pub residual_ms: f64,
    pub simultaneous: bool,
}

/// §3.3 row states (chip vocabulary mirrors the CLI/Swift/UX-DECISIONS-V2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowState {
    Confident,
    Partial,
    Split,
    TwoSources,
    Ambiguous,
    NoMatch,
}

impl RowState {
    /// §3.3 chip vocabulary (human output) — identical to both shells.
    pub fn chip(&self) -> &'static str {
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

/// Why a row's candidates could not be verified within the configured resource ceiling
/// (PLAN-AUDIO-MATCHING-PREWORK §9.2). Constructed ONLY when the eligible candidate count
/// would exceed `max_verify_pairs_per_video`, or the running total would exceed
/// `max_verify_pairs_total`. The honest alternative to silently dropping candidates: NO
/// subset is verified, the row is `Ambiguous`, and every candidate name stays visible.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOverflow {
    /// Number of eligible (`prefilter_candidate`) cells for this row.
    pub eligible: usize,
    /// The resource ceiling that was exceeded.
    pub limit: usize,
    /// Which ceiling: "perVideo" (`max_verify_pairs_per_video`) or "total"
    /// (`max_verify_pairs_total`).
    pub scope: String,
}

/// The honesty trail behind a row's state — surfaced raw so the owner can tune. Display/fit
/// telemetry (shadow, fitBest/SecondRmsMs) stays SHELL-only and is NOT here.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionDetail {
    /// z_best − z_second among conflicting candidates; null when the margin rule never ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// Coverage-interval clusters (1 = all overlapping, >1 = disjoint groups).
    pub clusters: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub triangles: Vec<TriangleCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Set ONLY when a resource ceiling forced this row to honest ambiguity (§9.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_overflow: Option<CandidateOverflow>,
}

/// A resolved queue row = one video + its selected audio (None for ambiguous/no-match:
/// never a coin-flip) + the alternates. Carries BOTH the video content key and file name so
/// each shell projects its own view. `chip` is computed here via the shared `chip_text`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRow {
    pub video_key: String,
    pub video_name: String,
    pub state: RowState,
    pub chip: String,
    pub selected: Option<RowCandidate>,
    pub alternates: Vec<RowCandidate>,
    pub resolution: ResolutionDetail,
}

// ============================ track metadata view ============================

/// The video/audio identity the resolver threads into rows — file name + content key +
/// duration + whether the video has an analyzable audio stream (videos only). Built from
/// `Ctx::track` (`meta.content_key`, `meta.info.path` file name, `meta.duration_s`).
struct TrackView {
    key: String,
    name: String,
    duration_s: f64,
    /// Videos: false ⇒ unanalyzable (no audio stream) ⇒ `no match`. Audios: always true.
    /// (The IMPORTED-TRACK-ONLY entry point only sees analyzable videos, so this is always
    /// true here — the field mirrors the shells' `has_audio == Some(false)` branch shape so
    /// a later descriptor API can wrap stripped videos as inert no-match rows.)
    has_audio: bool,
}

/// File name of a path string (its trailing component) — the shell-facing identity the CLI
/// surfaces as `video`/`audio` and the deterministic name tie-break keys on (SPEC §1.4). The
/// CLI builds rows from the bare directory entry name (e.g. "video1.mp4"); core only holds the
/// full `MediaInfo.path`, so we take its file-name component to reproduce that exactly. A path
/// with no file-name component falls back to the whole string.
fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

impl Ctx {
    fn track_view(&self, id: TrackId) -> Result<TrackView> {
        let t = self.track(id)?;
        Ok(TrackView {
            key: t.meta.content_key.clone(),
            name: file_name_of(&t.meta.info.path),
            duration_s: t.meta.duration_s,
            // The IMPORTED-TRACK entry point only admits analyzable videos; an imported track
            // always has audio content (it was decoded into a pyramid). The field mirrors the
            // shells' `has_audio == Some(false)` branch shape so a later descriptor API can wrap
            // stripped videos as inert no-match rows.
            has_audio: true,
        })
    }
}

// ============================ resolve entry points ============================

impl Ctx {
    /// Full §3.3 resolution over imported tracks (IMPORTED-TRACK ONLY — callers exclude
    /// audio-less videos; a shell wraps those as inert no-match rows). Builds the matrix
    /// internally (reusing `match_candidates_graph` / `pair_candidates`) and resolves every
    /// row. Byte-identical to the progress path by construction (the emitter is observational).
    pub fn resolve(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        p: &ResolveParams,
    ) -> Result<Vec<ResolvedRow>> {
        let mut prog = ProgressEmitter::new(0, noop_progress());
        let cancel = CancelToken::new();
        self.resolve_prog(videos, audios, p, &mut prog, &cancel)
    }

    /// Progress-aware [`Self::resolve`]. The expensive work is the matrix build, already
    /// instrumented (`MatchFingerprint`/`MatchCorrelate`/`MatchGraph`) inside
    /// `match_candidates_graph_prog` / `match_candidates_prog`. When `verify_active` (chunked
    /// `verify_batch`) or `rms_escalation` (`pair_fit_batch`) do non-trivial work, the legacy
    /// path emits explicit [`Phase::Verify`] / [`Phase::Resolve`] around it; otherwise a cheap
    /// terminal `Resolve` tick lands the bar. Output is BYTE-IDENTICAL to `resolve`.
    pub fn resolve_prog(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        p: &ResolveParams,
        prog: &mut ProgressEmitter,
        cancel: &CancelToken,
    ) -> Result<Vec<ResolvedRow>> {
        // Validate the core-owned pairing params once before any work.
        p.pairing.validate()?;

        // Identity views for rows (file name + content key + duration). The IMPORTED-TRACK
        // entry point requires every id be a loaded track.
        let video_views: Vec<TrackView> = videos
            .iter()
            .map(|&id| self.track_view(id))
            .collect::<Result<_>>()?;
        let audio_views: Vec<TrackView> = audios
            .iter()
            .map(|&id| self.track_view(id))
            .collect::<Result<_>>()?;

        if p.matcher.resolves_with_fingerprints() {
            self.resolve_fingerprint(videos, audios, &video_views, &audio_views, p, prog, cancel)
        } else {
            self.resolve_legacy(videos, audios, &video_views, &audio_views, p, prog, cancel)
        }
    }

    // ---------------- fingerprint path (the default, §13.4) ----------------

    #[allow(clippy::too_many_arguments)]
    fn resolve_fingerprint(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        video_views: &[TrackView],
        audio_views: &[TrackView],
        p: &ResolveParams,
        prog: &mut ProgressEmitter,
        cancel: &CancelToken,
    ) -> Result<Vec<ResolvedRow>> {
        // The GRAPH-ADJUSTED matrix under the core default (FingerprintPrimary + profile 2):
        // `classification` is the identity authority; this carries the
        // MatchFingerprint/MatchCorrelate/MatchGraph progress.
        let matrix = if videos.is_empty() || audios.is_empty() {
            Vec::new()
        } else {
            self.match_candidates_graph_prog(videos, audios, &p.pairing, prog, cancel)?
        };

        let mut ab_memo: HashMap<(usize, usize), PairEvidence> = HashMap::new();
        let mut rows: Vec<ResolvedRow> = Vec::with_capacity(video_views.len());
        for (vi, v) in video_views.iter().enumerate() {
            cancel.check()?;
            let cells: Vec<(usize, &PairEvidence)> = matrix
                .get(vi)
                .map(|row| row.iter().enumerate().collect())
                .unwrap_or_default();
            rows.push(resolve_row_fp(self, p, v, &cells, audios, audio_views, &mut ab_memo)?);
        }
        // Terminal resolve tick (the row pass is cheap relative to the matrix build).
        prog.emit(Phase::Resolve, 1.0);
        Ok(rows)
    }

    // ---------------- legacy path (developer override + shadow) ----------------

    #[allow(clippy::too_many_arguments)]
    fn resolve_legacy(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        video_views: &[TrackView],
        audio_views: &[TrackView],
        p: &ResolveParams,
        prog: &mut ProgressEmitter,
        cancel: &CancelToken,
    ) -> Result<Vec<ResolvedRow>> {
        let sp = &p.pairing;

        // Candidate matrix (§3.2): stage-1 between every imported (video, audio). The legacy
        // matrix is rayon-parallel; bracket it with a coarse MatchCorrelate 0→1.
        prog.emit(Phase::MatchCorrelate, 0.0);
        let cells = if videos.is_empty() || audios.is_empty() {
            vec![Vec::new(); videos.len()]
        } else {
            self.pair_candidates(videos, audios, sp)?
        };
        cancel.check()?;
        prog.emit(Phase::MatchCorrelate, 1.0);

        // Per-video candidate cells (audio index, measurement). Candidacy is the loose
        // Stage-A pre-filter when `verify_active`, else the default z-gate against the
        // resolver's own `z_min` (byte-identical to today).
        let mut per_video: Vec<Vec<(usize, PairCorrelation)>> = vec![Vec::new(); video_views.len()];
        for (vi, row) in cells.into_iter().enumerate() {
            for (ai, m) in row.into_iter().enumerate() {
                let candidate = if p.verify_active {
                    m.prefilter_candidate
                } else {
                    !m.insufficient_overlap && m.z >= p.z_min
                };
                if candidate {
                    per_video[vi].push((ai, m));
                }
            }
        }

        // 3a. Match classifier (flag ON only). Verify EVERY prefilter candidate (no top-K
        // truncation) in chunks; the is_match survivors REPLACE per_video[vi]. The only bound
        // on work is the core-owned per-video/total verify CEILING; a row that would exceed it
        // verifies NO subset, records a CandidateOverflow, and is forced to honest ambiguity.
        let mut overflow: Vec<Option<CandidateOverflow>> = vec![None; video_views.len()];
        if p.verify_active {
            prog.emit(Phase::Verify, 0.0);
            const VERIFY_CHUNK: usize = 256;
            let per_video_limit = sp.max_verify_pairs_per_video;
            let total_limit = sp.max_verify_pairs_total;
            let mut total_verified: usize = 0;
            for vi in 0..video_views.len() {
                cancel.check()?;
                let eligible = per_video[vi].len();
                if eligible == 0 {
                    continue;
                }
                if eligible > per_video_limit {
                    overflow[vi] = Some(CandidateOverflow {
                        eligible,
                        limit: per_video_limit,
                        scope: "perVideo".into(),
                    });
                    continue;
                }
                if total_verified + eligible > total_limit {
                    overflow[vi] = Some(CandidateOverflow {
                        eligible,
                        limit: total_limit,
                        scope: "total".into(),
                    });
                    continue;
                }
                total_verified += eligible;

                let cands = &per_video[vi];
                let mut real: Vec<(usize, PairCorrelation)> = Vec::new();
                for chunk in cands.chunks(VERIFY_CHUNK) {
                    let req: Vec<(TrackId, TrackId, f64)> =
                        chunk.iter().map(|(ai, m)| (videos[vi], audios[*ai], m.lag_ms)).collect();
                    let fits = self.verify_batch(&req, sp)?;
                    for ((ai, m), fit) in chunk.iter().zip(fits) {
                        if let Some(f) = fit {
                            if crate::verify::is_match(&f) {
                                real.push((*ai, m.clone()));
                            }
                        }
                    }
                }
                per_video[vi] = real;
            }
            prog.emit(Phase::Verify, 1.0);
        }

        // 4. Resolution (§3.3) per video row. When `rms_escalation` is OFF (the CLI legacy
        // default) every row finishes in one pass and an undecided contested cluster goes
        // straight to ambiguous. When ON (the Swift legacy default) a contested,
        // non-simultaneous cluster the z-margin rule leaves undecided is DEFERRED; one batched
        // `pair_fit_batch` drift-fits the contested pairs and the RMS escalation rescues a
        // genuinely-decidable take.
        let mut ab_memo: HashMap<(usize, usize), PairCorrelation> = HashMap::new();
        let mut outcomes: Vec<RowOutcome> = Vec::with_capacity(video_views.len());
        let mut fit_reqs: Vec<FitRequest> = Vec::new();
        for (vi, v) in video_views.iter().enumerate() {
            cancel.check()?;
            outcomes.push(resolve_row_legacy(
                self,
                p,
                v,
                &per_video[vi],
                overflow[vi].clone(),
                videos[vi],
                audios,
                audio_views,
                &mut ab_memo,
                p.rms_escalation,
                &mut fit_reqs,
            )?);
        }

        if !p.rms_escalation || fit_reqs.is_empty() {
            // No deferred clusters (or escalation off): outcomes are already final.
            prog.emit(Phase::Resolve, 1.0);
            return Ok(outcomes.into_iter().map(|o| o.row).collect());
        }

        // ONE batch fit for every contested pair (PLAN-RMS-TIEBREAKER §4.4). De-dups identical
        // (video, audio, lag~0.1 ms) pairs so a pair shared by two rows fits once.
        prog.emit(Phase::Resolve, 0.0);
        let fit_by_cand = batch_fits(&fit_reqs, |pairs| self.pair_fit_batch(pairs, sp))?;
        let rows: Vec<ResolvedRow> = outcomes
            .into_iter()
            .map(|o| finalize(o, &fit_by_cand, p))
            .collect();
        prog.emit(Phase::Resolve, 1.0);
        Ok(rows)
    }
}

// ============================ fingerprint row resolution (§13.4) ============================

/// The effective (lag_ms, drift_ppm) a `PairEvidence` cell describes — the REFINED mapping
/// when boundary 7 populated it, else the consensus mapping (§13.1). Both finite (§20).
fn evidence_lag_drift(e: &PairEvidence) -> (f64, f64) {
    (e.refined_lag_ms.unwrap_or(e.lag_ms), e.refined_drift_ppm.unwrap_or(e.drift_ppm))
}

/// §3.3 resolution for one video from core `PairEvidence` cells (the default, §13.4).
/// Candidacy is core's `classification == Match` (the identity authority, invariant 8);
/// everything else is the same coverage / cluster / triangle / margin policy as the legacy
/// path. Reproduces the CLI `resolve_row_fp` exactly.
#[allow(clippy::too_many_arguments)]
fn resolve_row_fp(
    ctx: &Ctx,
    p: &ResolveParams,
    video: &TrackView,
    cells: &[(usize, &PairEvidence)],
    audios: &[TrackId],
    audio_views: &[TrackView],
    ab_memo: &mut HashMap<(usize, usize), PairEvidence>,
) -> Result<ResolvedRow> {
    let mk = |ai: usize, e: &PairEvidence| RowCandidate {
        audio_key: audio_views[ai].key.clone(),
        audio_name: audio_views[ai].name.clone(),
        z: e.score,
        lag_ms: evidence_lag_drift(e).0,
        coverage: coverage(video.duration_s, audio_views[ai].duration_s, evidence_lag_drift(e).0),
    };
    let row = |state: RowState,
               selected: Option<RowCandidate>,
               alternates: Vec<RowCandidate>,
               resolution: ResolutionDetail| ResolvedRow {
        video_key: video.key.clone(),
        video_name: video.name.clone(),
        chip: chip_text(state, &selected, video.duration_s),
        state,
        selected,
        alternates,
        resolution,
    };

    if !video.has_audio {
        return Ok(row(
            RowState::NoMatch,
            None,
            Vec::new(),
            detail(None, 0, Vec::new(), Some("no audio stream".into())),
        ));
    }

    // CANDIDACY = core's identity verdict: keep only `Match` cells (no re-derived threshold).
    let mut c: Vec<(usize, &PairEvidence)> = cells
        .iter()
        .filter(|(_, e)| e.classification == MatchClassification::Match)
        .map(|(ai, e)| (*ai, *e))
        .collect();
    // Deterministic order: score desc, ties by audio file name (clock-free, SPEC §1.4).
    c.sort_by(|a, b| {
        b.1.score
            .total_cmp(&a.1.score)
            .then_with(|| audio_views[a.0].name.cmp(&audio_views[b.0].name))
    });
    let candidates: Vec<RowCandidate> = c.iter().map(|(ai, e)| mk(*ai, e)).collect();

    match c.len() {
        0 => Ok(row(RowState::NoMatch, None, Vec::new(), detail(None, 0, Vec::new(), None))),
        1 => {
            let sel = candidates[0].clone();
            let state = if sel.coverage.fraction >= p.coverage_full {
                RowState::Confident
            } else {
                RowState::Partial
            };
            Ok(row(state, Some(sel), Vec::new(), detail(None, 1, Vec::new(), None)))
        }
        _ => {
            let covs: Vec<Coverage> = candidates.iter().map(|c| c.coverage).collect();
            let clusters = cluster_intervals(&covs, p.disjoint_tol_s);

            let mut triangles: Vec<TriangleCheck> = Vec::new();
            let mut margin_out: Option<f64> = None;
            let mut winners: Vec<(usize, bool)> = Vec::new();
            let mut ambiguous = false;
            // §3.4 closure window is the video coverage span; the §13.3 tolerances come from
            // the core PairingParams (matching the CLI resolve_row_fp).
            let tri_offset_tol = p.pairing.graph_offset_tol_ms;
            let tri_drift_tol = p.pairing.graph_drift_tol_ppm;
            for cluster in &clusters {
                if cluster.len() == 1 {
                    winners.push((cluster[0], false));
                    continue;
                }
                let mut all_simultaneous = true;
                for (i, &ci) in cluster.iter().enumerate() {
                    for &cj in &cluster[i + 1..] {
                        let (ai, ea) = c[ci];
                        let (aj, eb) = c[cj];
                        let ab = match_audio_pair_evidence(ctx, audios, ai, aj, &p.pairing, ab_memo)?;
                        let va = crate::match_graph::evidence_map(ea);
                        let vb = crate::match_graph::evidence_map(eb);
                        let ab_map = crate::match_graph::evidence_map(&ab);
                        let simultaneous = crate::match_graph::triangle_closes(
                            va,
                            ab_map,
                            vb,
                            ab.classification,
                            video.duration_s,
                            tri_offset_tol,
                            tri_drift_tol,
                        );
                        all_simultaneous &= simultaneous;
                        let residual = composed_residual_max_ms(va, ab_map, vb, video.duration_s);
                        triangles.push(TriangleCheck {
                            audio_a: audio_views[ai].name.clone(),
                            audio_b: audio_views[aj].name.clone(),
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
                    let z_best = c[cluster[0]].1.score;
                    let z_second = c[cluster[1]].1.score;
                    let margin = z_best - z_second;
                    margin_out = Some(margin);
                    if margin >= p.z_margin {
                        winners.push((cluster[0], false));
                    } else {
                        ambiguous = true;
                    }
                }
            }

            if ambiguous {
                return Ok(row(
                    RowState::Ambiguous,
                    None,
                    candidates,
                    detail(margin_out, clusters.len(), triangles, None),
                ));
            }

            Ok(assemble_winner_row(
                &video.key, &video.name, video.duration_s, &candidates,
                &clusters, &winners, &triangles, margin_out, p,
            ))
        }
    }
}

/// The MAX of the start/mid/end composed-offset residuals (ms) between the direct V→B map
/// and the composed V→A→B map — the §3.4 closure read-out. Returns a finite sentinel (0.0)
/// when a map is missing so the row JSON stays finite (§20).
fn composed_residual_max_ms(
    va: Option<crate::match_graph::AffineMap>,
    ab: Option<crate::match_graph::AffineMap>,
    vb: Option<crate::match_graph::AffineMap>,
    coverage_s: f64,
) -> f64 {
    let (Some(va), Some(ab), Some(vb)) = (va, ab, vb) else {
        return 0.0;
    };
    let Some(composed) = va.compose(ab) else {
        return 0.0;
    };
    let t1 = if coverage_s.is_finite() && coverage_s > 0.0 { coverage_s } else { 1.0 };
    crate::match_graph::CycleResidual::between(&vb, &composed, 0.0, t1).max_offset_ms()
}

/// Audio-audio `PairEvidence` for the §3.4 triangle, memoized with antisymmetric lag/drift
/// (L(B,A) = −L(A,B)). Uses `match_pair` (the same core path the matrix used). Keyed by the
/// unordered audio pair.
fn match_audio_pair_evidence(
    ctx: &Ctx,
    audios: &[TrackId],
    a: usize,
    b: usize,
    sp: &PairingParams,
    memo: &mut HashMap<(usize, usize), PairEvidence>,
) -> Result<PairEvidence> {
    let key = (a.min(b), a.max(b));
    if !memo.contains_key(&key) {
        let e = ctx.match_pair(audios[key.0], audios[key.1], sp)?;
        memo.insert(key, e);
    }
    let mut e = memo[&key].clone();
    if a > b {
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

// ============================ legacy row resolution (§3.3) ============================

/// What pass 1 produced for one row: a finished row, or a provisional `ambiguous` row plus
/// the context the RMS escalation needs to re-decide it.
struct RowOutcome {
    row: ResolvedRow,
    pending: Option<Pending>,
}

/// The decision context for a row left provisionally `ambiguous` by an undecidable contested
/// cluster (carried out of pass 1 so the post-batch escalation can finish it without
/// re-running stage-1 / triangle checks).
struct Pending {
    video_key: String,
    video_name: String,
    dur_video: f64,
    /// Normalized candidates in z-desc/name order; coverage and identity travel together.
    candidates: Vec<RowCandidate>,
    clusters: Vec<Vec<usize>>,
    decided: Vec<(usize, bool)>,
    undecided: Vec<Vec<usize>>,
    triangles: Vec<TriangleCheck>,
    margin: Option<f64>,
}

/// One contested candidate pair needing a drift-fit RMS.
struct FitRequest {
    video_key: String,
    audio_key: String,
    video_id: TrackId,
    audio_id: TrackId,
    lag_ms: f64,
}

/// §3.3 resolution for one video from legacy `PairCorrelation` candidates. Mirrors the Swift
/// `resolveRow` (which defers undecided contested clusters when escalation is on) and the CLI
/// `resolve_row` (which sends them straight to ambiguous when escalation is off).
#[allow(clippy::too_many_arguments)]
fn resolve_row_legacy(
    ctx: &Ctx,
    p: &ResolveParams,
    video: &TrackView,
    cands: &[(usize, PairCorrelation)],
    overflow: Option<CandidateOverflow>,
    video_id: TrackId,
    audios: &[TrackId],
    audio_views: &[TrackView],
    ab_memo: &mut HashMap<(usize, usize), PairCorrelation>,
    rms_escalation: bool,
    fit_reqs: &mut Vec<FitRequest>,
) -> Result<RowOutcome> {
    let mk = |ai: usize, m: &PairCorrelation| RowCandidate {
        audio_key: audio_views[ai].key.clone(),
        audio_name: audio_views[ai].name.clone(),
        z: m.z,
        lag_ms: m.lag_ms,
        coverage: coverage(video.duration_s, audio_views[ai].duration_s, m.lag_ms),
    };
    let row = |state: RowState,
               selected: Option<RowCandidate>,
               alternates: Vec<RowCandidate>,
               resolution: ResolutionDetail| ResolvedRow {
        video_key: video.key.clone(),
        video_name: video.name.clone(),
        chip: chip_text(state, &selected, video.duration_s),
        state,
        selected,
        alternates,
        resolution,
    };
    let done = |r: ResolvedRow| RowOutcome { row: r, pending: None };

    if !video.has_audio {
        return Ok(done(row(
            RowState::NoMatch,
            None,
            Vec::new(),
            detail(None, 0, Vec::new(), Some("no audio stream".into())),
        )));
    }

    // Resource ceiling exceeded (§9.2): honest ambiguity, never a silent eviction.
    if let Some(ov) = overflow {
        let alternates: Vec<RowCandidate> = cands.iter().map(|(ai, m)| mk(*ai, m)).collect();
        let note = format!(
            "candidate overflow: {} eligible exceed the {} {} verify limit",
            ov.eligible, ov.limit, ov.scope
        );
        return Ok(done(row(
            RowState::Ambiguous,
            None,
            alternates,
            ResolutionDetail {
                margin: None,
                clusters: 0,
                triangles: Vec::new(),
                note: Some(note),
                candidate_overflow: Some(ov),
            },
        )));
    }

    // Candidates sorted by z desc; ties by audio file name (deterministic, clock-free).
    let mut c: Vec<(usize, PairCorrelation)> = cands.to_vec();
    c.sort_by(|a, b| {
        b.1.z
            .total_cmp(&a.1.z)
            .then_with(|| audio_views[a.0].name.cmp(&audio_views[b.0].name))
    });
    let candidates: Vec<RowCandidate> = c.iter().map(|(ai, m)| mk(*ai, m)).collect();

    match c.len() {
        0 => Ok(done(row(RowState::NoMatch, None, Vec::new(), detail(None, 0, Vec::new(), None)))),
        1 => {
            let sel = candidates[0].clone();
            let state = if sel.coverage.fraction >= p.coverage_full {
                RowState::Confident
            } else {
                RowState::Partial
            };
            Ok(done(row(state, Some(sel), Vec::new(), detail(None, 1, Vec::new(), None))))
        }
        _ => {
            let covs: Vec<Coverage> = candidates.iter().map(|c| c.coverage).collect();
            let clusters = cluster_intervals(&covs, p.disjoint_tol_s);

            let mut triangles: Vec<TriangleCheck> = Vec::new();
            let mut margin_out: Option<f64> = None;
            let mut winners: Vec<(usize, bool)> = Vec::new();
            let mut undecided: Vec<Vec<usize>> = Vec::new();

            for cluster in &clusters {
                if cluster.len() == 1 {
                    winners.push((cluster[0], false));
                    continue;
                }
                // §3.4 simultaneity over every pair in the cluster — raw-z triangle.
                let mut all_simultaneous = true;
                for (i, &ci) in cluster.iter().enumerate() {
                    for &cj in &cluster[i + 1..] {
                        let (ai, ma) = (c[ci].0, &c[ci].1);
                        let (aj, mb) = (c[cj].0, &c[cj].1);
                        let ab = correlate_audio_pair(ctx, audios, ai, aj, &p.pairing, ab_memo)?;
                        let residual = (mb.lag_ms - ma.lag_ms - ab.lag_ms).abs();
                        let simultaneous = ab.z >= p.z_min && residual <= p.triangle_tol_ms;
                        all_simultaneous &= simultaneous;
                        triangles.push(TriangleCheck {
                            audio_a: audio_views[ai].name.clone(),
                            audio_b: audio_views[aj].name.clone(),
                            ab_z: ab.z,
                            ab_lag_ms: ab.lag_ms,
                            residual_ms: residual,
                            simultaneous,
                        });
                    }
                }
                if all_simultaneous {
                    winners.push((cluster[0], true));
                } else {
                    // Conflict: §3.5 margin rule, biased to ambiguous over a coin-flip. A
                    // contested cluster always wants drift-fit telemetry (escalation path).
                    if rms_escalation {
                        for &idx in cluster {
                            fit_reqs.push(FitRequest {
                                video_key: video.key.clone(),
                                audio_key: audio_views[c[idx].0].key.clone(),
                                video_id,
                                audio_id: audios[c[idx].0],
                                lag_ms: c[idx].1.lag_ms,
                            });
                        }
                    }
                    let z_best = c[cluster[0]].1.z;
                    let z_second = c[cluster[1]].1.z;
                    let margin = z_best - z_second;
                    margin_out = Some(margin);
                    if z_second < p.z_min || margin >= p.z_margin {
                        winners.push((cluster[0], false));
                    } else if rms_escalation {
                        undecided.push(cluster.clone());
                    } else {
                        // CLI legacy: an undecided cluster goes straight to ambiguous.
                        return Ok(done(row(
                            RowState::Ambiguous,
                            None,
                            candidates,
                            detail(margin_out, clusters.len(), triangles, None),
                        )));
                    }
                }
            }

            // Any unsettled contested cluster (escalation on) ⇒ provisional ambiguous carrying
            // the Pending context the post-batch escalation needs.
            if !undecided.is_empty() {
                let amb = row(
                    RowState::Ambiguous,
                    None,
                    candidates.clone(),
                    detail(margin_out, clusters.len(), triangles.clone(), None),
                );
                let pending = Pending {
                    video_key: video.key.clone(),
                    video_name: video.name.clone(),
                    dur_video: video.duration_s,
                    candidates,
                    clusters,
                    decided: winners,
                    undecided,
                    triangles,
                    margin: margin_out,
                };
                return Ok(RowOutcome { row: amb, pending: Some(pending) });
            }

            Ok(done(assemble_winner_row(
                &video.key,
                &video.name,
                video.duration_s,
                &candidates,
                &clusters,
                &winners,
                &triangles,
                margin_out,
                p,
            )))
        }
    }
}

/// Build the final selected row from settled cluster winners (§3.3): disjoint clusters ⇒
/// recorder restart ⇒ max coverage; ties by z/score then file name. Shared by fingerprint,
/// legacy pass-1 (no contest), and RMS rescue so each uses the same selection + state rules.
#[allow(clippy::too_many_arguments)]
fn assemble_winner_row(
    video_key: &str,
    video_name: &str,
    dur_video: f64,
    candidates: &[RowCandidate],
    clusters: &[Vec<usize>],
    winners: &[(usize, bool)],
    triangles: &[TriangleCheck],
    margin: Option<f64>,
    p: &ResolveParams,
) -> ResolvedRow {
    let (sel_pos, _) = winners
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let ca = &candidates[a.0];
            let cb = &candidates[b.0];
            ca.coverage.seconds
                .total_cmp(&cb.coverage.seconds)
                .then_with(|| ca.z.total_cmp(&cb.z))
                .then_with(|| cb.audio_name.cmp(&ca.audio_name))
        })
        .expect("non-empty winners");
    let (sel_idx, sel_simul) = winners[sel_pos];
    let sel = candidates[sel_idx].clone();
    let alternates: Vec<RowCandidate> =
        candidates.iter().enumerate().filter(|(i, _)| *i != sel_idx).map(|(_, c)| c.clone()).collect();
    let state = if clusters.len() > 1 {
        RowState::Split
    } else if sel_simul {
        RowState::TwoSources
    } else if sel.coverage.fraction >= p.coverage_full {
        RowState::Confident
    } else {
        RowState::Partial
    };
    ResolvedRow {
        video_key: video_key.to_string(),
        video_name: video_name.to_string(),
        chip: chip_text(state, &Some(sel.clone()), dur_video),
        state,
        selected: Some(sel),
        alternates,
        resolution: ResolutionDetail {
            margin,
            clusters: clusters.len(),
            triangles: triangles.to_vec(),
            note: None,
            candidate_overflow: None,
        },
    }
}

// ============================ RMS escalation (Phase 2, §5.2) ============================

/// Pass 2 for one row: a provisional ambiguous row may be rescued by RMS. A row WITHOUT a
/// `Pending` is returned unchanged (the CLI legacy default never produces a `Pending`).
fn finalize(outcome: RowOutcome, fit_by_cand: &HashMap<String, crate::sync::PairFit>, p: &ResolveParams) -> ResolvedRow {
    if let Some(pending) = outcome.pending {
        if let Some(rescued) = escalate(&pending, fit_by_cand, p) {
            return rescued;
        }
    }
    outcome.row
}

/// De-dup/stitch key for a fit (video key ⊕ audio key).
fn fit_key(video_key: &str, audio_key: &str) -> String {
    format!("{video_key}\u{0}{audio_key}")
}

/// Run the batch fit for all collected contested pairs once and return a (videoKey ⊕
/// audioKey) → PairFit map. De-dups identical (video, audio, lag~0.1 ms) pairs WITHIN this
/// resolve so a pair shared by two rows is fitted once.
fn batch_fits(
    requests: &[FitRequest],
    fit_batch: impl FnOnce(&[(TrackId, TrackId, f64)]) -> Result<Vec<Option<crate::sync::PairFit>>>,
) -> Result<HashMap<String, crate::sync::PairFit>> {
    if requests.is_empty() {
        return Ok(HashMap::new());
    }
    // De-dup key: (video_key, audio_key, round(lag*10)).
    let mut order: Vec<&FitRequest> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String, i64)> = std::collections::HashSet::new();
    for r in requests {
        let k = (r.video_key.clone(), r.audio_key.clone(), (r.lag_ms * 10.0).round() as i64);
        if seen.insert(k) {
            order.push(r);
        }
    }
    let pairs: Vec<(TrackId, TrackId, f64)> = order
        .iter()
        .map(|r| (r.video_id, r.audio_id, r.lag_ms))
        .collect();
    let fits = fit_batch(&pairs)?;
    let mut fit_by_cand: HashMap<String, crate::sync::PairFit> = HashMap::new();
    for (r, fit) in order.iter().zip(fits) {
        if let Some(f) = fit {
            fit_by_cand.insert(fit_key(&r.video_key, &r.audio_key), f);
        }
    }
    Ok(fit_by_cand)
}

/// Try to rescue a provisional ambiguous row using drift-fit RMS. EVERY undecided contested
/// cluster must yield a confident pick; if any stays ambiguous, the whole row stays ambiguous
/// (no partial rescue, no coin-flip). Returns the finished selected row, or None to keep the
/// provisional row.
fn escalate(
    p: &Pending,
    fit_by_cand: &HashMap<String, crate::sync::PairFit>,
    params: &ResolveParams,
) -> Option<ResolvedRow> {
    let fit = |i: usize| fit_by_cand.get(&fit_key(&p.video_key, &p.candidates[i].audio_key)).copied();
    let mut winners = p.decided.clone();
    for cluster in &p.undecided {
        let pick = rms_escalate(cluster, &p.candidates, &fit, params)?;
        winners.push((pick, false));
    }
    Some(assemble_winner_row(
        &p.video_key,
        &p.video_name,
        p.dur_video,
        &p.candidates,
        &p.clusters,
        &winners,
        &p.triangles,
        p.margin,
        params,
    ))
}

/// The RMS tie-breaker predicate (PLAN-RMS-TIEBREAKER §5.2). Returns the index in `c` to
/// SELECT for this contested cluster, or None to stay ambiguous. ACTIVE form = conservative
/// ("confirm the z-leader"); `rms_strong_form` switches to the strong form ("pick the single
/// clean fit").
fn rms_escalate(
    cluster: &[usize],
    candidates: &[RowCandidate],
    fit: &dyn Fn(usize) -> Option<crate::sync::PairFit>,
    params: &ResolveParams,
) -> Option<usize> {
    if cluster.len() < 2 {
        return None;
    }
    if params.rms_strong_form {
        rms_escalate_strong(cluster, candidates, fit, params)
    } else {
        rms_escalate_conservative(cluster, candidates, fit, params)
    }
}

/// Conservative: confirm `cluster[0]` (best z) is the real take.
fn rms_escalate_conservative(
    cluster: &[usize],
    candidates: &[RowCandidate],
    fit: &dyn Fn(usize) -> Option<crate::sync::PairFit>,
    params: &ResolveParams,
) -> Option<usize> {
    let leader_idx = cluster[0];
    let lf = fit(leader_idx)?;
    if (lf.windows as usize) < params.min_fit_windows || lf.rms_ms > params.rms_trust_floor_ms {
        return None;
    }
    let leader_cov = candidates[leader_idx].coverage.seconds;
    for &o in &cluster[1..] {
        if !comparable_coverage(leader_cov, candidates[o].coverage.seconds) {
            continue;
        }
        match fit(o) {
            // o didn't lock ⇒ decisively worse ⇒ SUPPORTS the pick.
            None => continue,
            Some(of) => {
                if (of.windows as usize) < params.min_fit_windows {
                    continue;
                }
                if of.rms_ms < lf.rms_ms * params.rms_ratio {
                    return None; // o is comparably clean ⇒ ambiguous
                }
            }
        }
    }
    Some(leader_idx)
}

/// Strong: pick the single candidate that is a clean lock AND decisively better than all
/// others, regardless of z rank. Exactly one such candidate or None.
fn rms_escalate_strong(
    cluster: &[usize],
    candidates: &[RowCandidate],
    fit: &dyn Fn(usize) -> Option<crate::sync::PairFit>,
    params: &ResolveParams,
) -> Option<usize> {
    let cleans: Vec<usize> = cluster
        .iter()
        .copied()
        .filter(|&i| match fit(i) {
            Some(f) => (f.windows as usize) >= params.min_fit_windows && f.rms_ms <= params.rms_trust_floor_ms,
            None => false,
        })
        .collect();
    if cleans.len() != 1 {
        return None;
    }
    let winner = cleans[0];
    let wf = fit(winner)?;
    let winner_cov = candidates[winner].coverage.seconds;
    for &o in cluster {
        if o == winner {
            continue;
        }
        if !comparable_coverage(winner_cov, candidates[o].coverage.seconds) {
            continue;
        }
        match fit(o) {
            None => continue,
            Some(of) => {
                if (of.windows as usize) < params.min_fit_windows {
                    continue;
                }
                if of.rms_ms < wf.rms_ms * params.rms_ratio {
                    return None;
                }
            }
        }
    }
    Some(winner)
}

/// Two overlap spans are comparable when within 20% (§5.2 coverage guard).
fn comparable_coverage(a: f64, b: f64) -> bool {
    let hi = a.max(b);
    if hi <= 0.0 {
        true
    } else {
        (a - b).abs() / hi <= 0.20
    }
}

// ============================ legacy audio-audio correlation ============================

/// A×B stage-1, memoized with antisymmetric lag (L(B,A) = −L(A,B)).
fn correlate_audio_pair(
    ctx: &Ctx,
    audios: &[TrackId],
    a: usize,
    b: usize,
    sp: &PairingParams,
    memo: &mut HashMap<(usize, usize), PairCorrelation>,
) -> Result<PairCorrelation> {
    let key = (a.min(b), a.max(b));
    if !memo.contains_key(&key) {
        let m = ctx.pair_correlate(audios[key.0], audios[key.1], sp)?;
        memo.insert(key, m);
    }
    let mut m = memo[&key].clone();
    if a > b {
        m.lag_ms = -m.lag_ms;
    }
    Ok(m)
}

// ============================ shared geometry / chip helpers ============================

fn coverage(dur_video: f64, dur_audio: f64, lag_ms: f64) -> Coverage {
    let o = overlap_info(dur_video, dur_audio, lag_ms, 0.0);
    Coverage {
        t0: o.t0,
        t1: o.t1,
        seconds: o.seconds,
        fraction: o.fraction_of_video,
    }
}

/// Connected components of intervals overlapping by more than `tol` seconds, each component
/// preserving input (z-descending) order; groups by first index. Deterministic.
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

/// A ResolutionDetail with no overflow (the common construction path).
fn detail(
    margin: Option<f64>,
    clusters: usize,
    triangles: Vec<TriangleCheck>,
    note: Option<String>,
) -> ResolutionDetail {
    ResolutionDetail {
        margin,
        clusters,
        triangles,
        note,
        candidate_overflow: None,
    }
}

fn fmt_mmss(s: f64) -> String {
    let total = s.round() as i64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// §3.3 chip detail, e.g. partial → "audio covers 0:00–18:40 of 23:55". Identical to both
/// shells.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::PairFit;

    fn assembly_fixture() -> Pending {
        let candidate = |key: &str, name: &str, z, lag_ms, coverage| RowCandidate {
            audio_key: key.into(), audio_name: name.into(), z, lag_ms, coverage,
        };
        Pending {
            video_key: "video-key".into(),
            video_name: "video.mp4".into(),
            dur_video: 100.0,
            candidates: vec![
                candidate("zeta-key", "zeta.wav", 14.0, 100.0,
                    Coverage { t0: 0.0, t1: 40.0, seconds: 40.0, fraction: 0.4 }),
                candidate("alpha-key", "alpha.wav", 12.0, 200.0,
                    Coverage { t0: 0.0, t1: 60.0, seconds: 60.0, fraction: 0.6 }),
                candidate("beta-key", "beta.wav", 12.0, 300.0,
                    Coverage { t0: 20.0, t1: 80.0, seconds: 60.0, fraction: 0.6 }),
            ],
            clusters: vec![vec![0], vec![1, 2]],
            decided: vec![(0, true)],
            undecided: vec![vec![1, 2]],
            triangles: vec![TriangleCheck {
                audio_a: "alpha.wav".into(), audio_b: "beta.wav".into(),
                ab_z: 10.0, ab_lag_ms: 125.0, residual_ms: 25.0, simultaneous: false,
            }],
            margin: Some(2.0),
        }
    }

    fn expected_split_row(selected: usize) -> serde_json::Value {
        let candidates = [
            serde_json::json!({"audioKey":"zeta-key","audioName":"zeta.wav","z":14.0,"lagMs":100.0,
                "coverage":{"t0":0.0,"t1":40.0,"seconds":40.0,"fraction":0.4}}),
            serde_json::json!({"audioKey":"alpha-key","audioName":"alpha.wav","z":12.0,"lagMs":200.0,
                "coverage":{"t0":0.0,"t1":60.0,"seconds":60.0,"fraction":0.6}}),
            serde_json::json!({"audioKey":"beta-key","audioName":"beta.wav","z":12.0,"lagMs":300.0,
                "coverage":{"t0":20.0,"t1":80.0,"seconds":60.0,"fraction":0.6}}),
        ];
        let (chip, alternates) = match selected {
            1 => ("split across files — audio covers 0:00–1:00 of 1:40", vec![&candidates[0], &candidates[2]]),
            2 => ("split across files — audio covers 0:20–1:20 of 1:40", vec![&candidates[0], &candidates[1]]),
            _ => panic!("fixture expects alpha or beta"),
        };
        serde_json::json!({
            "videoKey":"video-key", "videoName":"video.mp4", "state":"split", "chip":chip,
            "selected":candidates[selected], "alternates":alternates,
            "resolution":{"margin":2.0,"clusters":2,"triangles":[{
                "audioA":"alpha.wav","audioB":"beta.wav","abZ":10.0,
                "abLagMs":125.0,"residualMs":25.0,"simultaneous":false
            }]}
        })
    }

    #[test]
    fn winner_assembly_preserves_ties_state_precedence_and_full_row() {
        let f = assembly_fixture();
        let params = ResolveParams::default();
        let assemble = |clusters: &[Vec<usize>], winners: &[(usize, bool)], p: &ResolveParams| {
            assemble_winner_row(&f.video_key, &f.video_name, f.dur_video, &f.candidates,
                clusters, winners, &f.triangles, f.margin, p)
        };
        // Coverage beats the higher-z first candidate; alphabetical name resolves the equal-z
        // coverage tie even when winners are supplied in reverse order. Split beats simultaneity.
        let row = assemble(&f.clusters, &[(2, false), (1, true), (0, true)], &params);
        assert_eq!(serde_json::to_value(row).unwrap(), expected_split_row(1));
        let one_cluster = vec![vec![0, 1, 2]];
        for (simultaneous, coverage_full, state, chip) in [
            (true, 0.6, RowState::TwoSources, "2 sources"),
            (false, 0.6, RowState::Confident, "confident"),
            (false, 0.95, RowState::Partial, "partial — audio covers 0:00–1:00 of 1:40"),
        ] {
            let p = ResolveParams { coverage_full, ..params.clone() };
            let row = assemble(&one_cluster, &[(1, simultaneous)], &p);
            assert_eq!(row.state, state);
            assert_eq!(row.chip, chip);
        }
    }

    #[test]
    fn rms_rescues_preserve_full_rows_and_decline_comparable_fits() {
        let f = assembly_fixture();
        let params = ResolveParams::default();
        let fit = |rms_ms| PairFit { off_ms: 500.0, ppm: 3.0, rms_ms, windows: 12 };
        let mut fits = HashMap::from([
            (fit_key("video-key", "alpha-key"), fit(0.5)),
            (fit_key("video-key", "beta-key"), fit(12.0)),
        ]);
        let conservative = escalate(&f, &fits, &params).unwrap();
        assert_eq!(serde_json::to_value(conservative).unwrap(), expected_split_row(1));
        fits.insert(fit_key("video-key", "alpha-key"), fit(12.0));
        fits.insert(fit_key("video-key", "beta-key"), fit(0.5));
        assert!(escalate(&f, &fits, &params).is_none());
        let strong = ResolveParams { rms_strong_form: true, ..params.clone() };
        assert_eq!(serde_json::to_value(escalate(&f, &fits, &strong).unwrap()).unwrap(), expected_split_row(2));
        fits.insert(fit_key("video-key", "alpha-key"), fit(0.5));
        assert!(escalate(&f, &fits, &params).is_none());
        assert!(escalate(&f, &fits, &strong).is_none());
    }

    #[test]
    fn batch_fits_keeps_first_request_order_and_last_successful_fit() {
        let request = |audio_key: &str, video_id, audio_id, lag_ms| FitRequest {
            video_key: "video".into(),
            audio_key: audio_key.into(),
            video_id,
            audio_id,
            lag_ms,
        };
        let requests = [
            request("a", 1, 2, 1.041),
            request("a", 11, 12, 1.049), // same rounded bucket; first handles/lag win
            request("b", 3, 4, -0.06),
            request("b", 13, 14, -0.09), // same negative rounded bucket
            request("a", 5, 6, 1.06),   // new rounded bucket, same final candidate key
            request("a", 7, 8, 1.16),
            request("b", 9, 10, -0.04), // rounds to zero, distinct from -0.06
        ];
        let fit = |off_ms| PairFit { off_ms, ppm: 2.0, rms_ms: 0.5, windows: 12 };
        let first = fit(10.0);
        let other = fit(20.0);
        let last = fit(30.0);
        let fits = batch_fits(&requests, |pairs| {
            assert_eq!(
                pairs,
                &[(1, 2, 1.041), (3, 4, -0.06), (5, 6, 1.06), (7, 8, 1.16), (9, 10, -0.04)]
            );
            Ok(vec![Some(first), Some(other), Some(last), None, None])
        }).unwrap();

        assert_eq!(fits.len(), 2);
        assert_eq!(fits[&fit_key("video", "a")], last);
        assert_eq!(fits[&fit_key("video", "b")], other);
    }

    #[test]
    fn batch_fits_empty_requests_skip_fitting() {
        let fits = batch_fits(&[], |_| panic!("empty requests must not run the fitter")).unwrap();
        assert!(fits.is_empty());
    }
}
