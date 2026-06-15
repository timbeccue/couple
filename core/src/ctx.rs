//! `Ctx`: engine root — config, track/session/job registries (§3.5, §4).

use crate::cache;
use crate::error::{CoupleError, Result};
use crate::ffmpeg;
use crate::import::{self, ImportRequest};
use crate::job::*;
use crate::media::*;
use crate::track::TrackData;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CtxConfig {
    pub ffmpeg_path: Option<PathBuf>,
    pub ffprobe_path: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
}

pub struct Ctx {
    pub config: CtxConfig,
    cache_root: PathBuf,
    tracks: RwLock<HashMap<TrackId, Arc<TrackData>>>,
    pub(crate) sessions: RwLock<HashMap<SessionId, Arc<crate::session::Session>>>,
    next_id: AtomicU64,
    jobs: Mutex<HashMap<JobId, JobEntry>>,
    imports_inflight: AtomicU32,
    streams: RwLock<HashMap<u64, Arc<crate::audition::AuditionStream>>>,
}

struct JobEntry {
    cancel: CancelToken,
    thread: Option<std::thread::JoinHandle<Result<u64>>>,
}

impl Ctx {
    pub fn new(config: CtxConfig) -> Arc<Self> {
        let cache_root = config
            .cache_dir
            .clone()
            .unwrap_or_else(cache::default_cache_dir);
        Arc::new(Self {
            config,
            cache_root,
            tracks: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            jobs: Mutex::new(HashMap::new()),
            imports_inflight: AtomicU32::new(0),
            streams: RwLock::new(HashMap::new()),
        })
    }

    // ---- audition (§4.8) ----

    pub fn audition_open(
        &self,
        session: SessionId,
        mode: crate::audition::MonitorMode,
        t_start: f64,
    ) -> Result<u64> {
        let s = self.session(session)?;
        let cam = self.track(s.camera)?;
        let ext = self.track(s.external)?;
        let id = self.next_id();
        let stream = Arc::new(crate::audition::AuditionStream::open(
            id, s, cam, ext, mode, t_start,
        ));
        self.streams.write().unwrap().insert(id, stream);
        Ok(id)
    }

    pub fn audition_stream(&self, id: u64) -> Result<Arc<crate::audition::AuditionStream>> {
        self.streams
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| CoupleError::invalid("unknown audition stream"))
    }

    pub fn audition_close(&self, id: u64) {
        self.streams.write().unwrap().remove(&id);
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn ffmpeg(&self) -> Result<PathBuf> {
        ffmpeg::resolve_tool("ffmpeg", self.config.ffmpeg_path.as_deref())
    }

    pub fn ffprobe(&self) -> Result<PathBuf> {
        ffmpeg::resolve_tool("ffprobe", self.config.ffprobe_path.as_deref())
    }

    pub fn probe(&self, path: &Path) -> Result<MediaInfo> {
        ffmpeg::probe(&self.ffprobe()?, path)
    }

    /// Probe without the audio-stream gate (SPEC-V2 §2 pool classification).
    pub fn probe_lenient(&self, path: &Path) -> Result<MediaInfo> {
        ffmpeg::probe_lenient(&self.ffprobe()?, path)
    }

    /// Blocking import; the `import_start` job wrapper runs this on a worker thread.
    pub fn import_blocking(
        &self,
        path: &Path,
        role: TrackRole,
        progress: ProgressFn,
        cancel: &CancelToken,
        job_id: JobId,
    ) -> Result<TrackId> {
        let req = ImportRequest {
            path,
            role,
            ffmpeg: &self.ffmpeg()?,
            ffprobe: &self.ffprobe()?,
            cache_root: &self.cache_root,
        };
        let id = self.next_id();
        let mut prog = ProgressEmitter::new(job_id, progress);
        let data = import::import(&req, id, &mut prog, cancel)?;
        self.tracks.write().unwrap().insert(id, Arc::new(data));
        Ok(id)
    }

    /// §4.3 import_start: non-blocking job. Result (TrackId) via `job_wait`.
    /// Concurrency limit [SPEC §3.5]: ≤ 2 import jobs in flight.
    pub fn import_start(
        self: &Arc<Self>,
        path: &Path,
        role: TrackRole,
        progress: ProgressFn,
        cancel: CancelToken,
    ) -> Result<JobId> {
        if self.imports_inflight.fetch_add(1, Ordering::SeqCst) >= 2 {
            self.imports_inflight.fetch_sub(1, Ordering::SeqCst);
            return Err(CoupleError::invalid("more than 2 import jobs in flight"));
        }
        let job_id = self.next_id();
        let ctx = Arc::clone(self);
        let path = path.to_path_buf();
        let c2 = cancel.clone();
        let thread = std::thread::spawn(move || {
            let r = ctx.import_blocking(&path, role, progress, &c2, job_id);
            ctx.imports_inflight.fetch_sub(1, Ordering::SeqCst);
            r
        });
        self.jobs.lock().unwrap().insert(
            job_id,
            JobEntry {
                cancel,
                thread: Some(thread),
            },
        );
        Ok(job_id)
    }

    pub fn job_cancel(&self, job: JobId) {
        if let Some(e) = self.jobs.lock().unwrap().get(&job) {
            e.cancel.cancel();
        }
    }

    /// Join a job; returns its result id (TrackId for imports). Removes the job entry.
    pub fn job_wait(&self, job: JobId) -> Result<u64> {
        let thread = {
            let mut jobs = self.jobs.lock().unwrap();
            let e = jobs.get_mut(&job).ok_or_else(|| CoupleError::invalid("unknown job"))?;
            e.thread.take()
        };
        let thread = thread.ok_or_else(|| CoupleError::invalid("job already waited"))?;
        let r = thread
            .join()
            .unwrap_or_else(|_| Err(CoupleError::internal("job thread panicked")));
        self.jobs.lock().unwrap().remove(&job);
        r
    }

    pub fn track(&self, id: TrackId) -> Result<Arc<TrackData>> {
        self.tracks
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| CoupleError::invalid(format!("unknown track {id}")))
    }

    pub fn track_free(&self, id: TrackId) {
        self.tracks.write().unwrap().remove(&id);
    }

    /// LRU cache GC with session pinning (SPEC-V2 §4, V2-F3). Mechanism: liveness via
    /// open handles — every loaded track's content key is exempt from eviction, so
    /// the `cpl_cache_gc` ABI signature stays v1-identical.
    pub fn cache_gc(&self, max_bytes: u64) -> u64 {
        let pinned: std::collections::HashSet<String> = self
            .tracks
            .read()
            .unwrap()
            .values()
            .map(|t| t.meta.content_key.clone())
            .collect();
        cache::gc(&self.cache_root, max_bytes, &pinned)
    }

    // ---- pairing (SPEC-V2 §4) ----

    /// Role-agnostic stage-1 correlation between any two imported tracks.
    pub fn pair_correlate(
        &self,
        a: TrackId,
        b: TrackId,
        params: &crate::pairing_params::PairingParams,
    ) -> Result<crate::pair::PairCorrelation> {
        let ta = self.track(a)?;
        let tb = self.track(b)?;
        Ok(crate::pair::pair_correlate(&ta.pyramid, &tb.pyramid, params))
    }

    /// Batch candidate matrix: rows = `videos`, columns = `audios`. Cells run in
    /// parallel; each cell is independent and deterministic (§5.8), so the matrix is
    /// bit-stable across runs on the same machine.
    pub fn pair_candidates(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        params: &crate::pairing_params::PairingParams,
    ) -> Result<Vec<Vec<crate::pair::PairCorrelation>>> {
        let vs: Vec<Arc<TrackData>> = videos.iter().map(|&id| self.track(id)).collect::<Result<_>>()?;
        let aus: Vec<Arc<TrackData>> = audios.iter().map(|&id| self.track(id)).collect::<Result<_>>()?;
        use rayon::prelude::*;
        Ok(vs
            .par_iter()
            .map(|v| {
                aus.iter()
                    .map(|a| crate::pair::pair_correlate(&v.pyramid, &a.pyramid, params))
                    .collect()
            })
            .collect())
    }

    /// Per-(video, audio) drift-fit RMS for a set of contested candidate pairs
    /// (SPEC PLAN-RMS-TIEBREAKER §4.1). `pairs`: (video_id, audio_id, coarse_lag_ms),
    /// where `coarse_lag_ms` is a candidate's `PairCorrelation.lag_ms`. Runs in
    /// parallel; each fit is independent and deterministic (§5.8). The result aligns
    /// to input order, element `None` when too few windows lock.
    pub fn pair_fit_batch(
        &self,
        pairs: &[(TrackId, TrackId, f64)],
        params: &crate::pairing_params::PairingParams,
    ) -> Result<Vec<Option<crate::sync::PairFit>>> {
        // Resolve every handle up front so a bad id fails the whole call (matching
        // pair_candidates) rather than racing a partial parallel result.
        let resolved: Vec<(Arc<TrackData>, Arc<TrackData>, f64)> = pairs
            .iter()
            .map(|&(v, a, lag)| Ok((self.track(v)?, self.track(a)?, lag)))
            .collect::<Result<_>>()?;
        use rayon::prelude::*;
        Ok(resolved
            .par_iter()
            .map(|(v, a, lag)| crate::sync::pair_fit_rms(&v.pyramid, &a.pyramid, *lag, params))
            .collect())
    }

    /// Per-(video, audio) fine match-verification for a set of candidate pairs
    /// (PLAN-MATCH-CLASSIFIER §2.4). `pairs`: (video_id, audio_id, coarse_lag_ms), where
    /// `coarse_lag_ms` is a candidate's `PairCorrelation.lag_ms`. Runs in parallel; each
    /// `verify_pair` is independent and deterministic (§5.8). The result aligns to input
    /// order, element `None` only below the absolute overlap floor.
    pub fn verify_batch(
        &self,
        pairs: &[(TrackId, TrackId, f64)],
        params: &crate::pairing_params::PairingParams,
    ) -> Result<Vec<Option<crate::verify::VerifyFit>>> {
        // Resolve every handle up front so a bad id fails the whole call (matching
        // pair_candidates) rather than racing a partial parallel result.
        let resolved: Vec<(Arc<TrackData>, Arc<TrackData>, f64)> = pairs
            .iter()
            .map(|&(v, a, lag)| Ok((self.track(v)?, self.track(a)?, lag)))
            .collect::<Result<_>>()?;
        use rayon::prelude::*;
        Ok(resolved
            .par_iter()
            .map(|(v, a, lag)| crate::verify::verify_pair(&v.pyramid, &a.pyramid, *lag, params))
            .collect())
    }

    // ---- method-neutral pair evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §5.3) ----

    /// Method-neutral evidence for one candidate (video=`a`, audio=`b`) pair (§5.3). NEW additive
    /// API — NOT wired into the default `couple pair` resolver, which stays exactly as-is.
    ///
    /// Orchestration (§5.3 / §9.3):
    ///
    /// - compute the candidate overlap from the two durations; below `params.min_overlap_s` ⇒
    ///   `InsufficientOverlap`;
    /// - [`MatcherMode::LegacyEnvelope`] ⇒ the method-neutral wrapper around `pair_correlate`
    ///   (`EnvelopeV1`), classified by the legacy gate. NO fingerprint is built (§5.2);
    /// - [`MatcherMode::FingerprintShadow`] / [`MatcherMode::FingerprintPrimary`] ⇒ each track's
    ///   `fingerprint_v1()`; if a fingerprint is unavailable but the legacy matcher works ⇒
    ///   `EnvelopeV1` fallback (§9.3); else boundary-4 correspondences → boundary-5 consensus →
    ///   `SpectralLandmarkV1` evidence with the §9.3 classification. In shadow mode the legacy
    ///   `PairCorrelation` is attached for comparison; in primary mode it is `None`.
    ///
    /// `params.validate()` is the caller's responsibility (every FFI/CLI entry validates); this
    /// resolves the handles and runs the deterministic matcher.
    pub fn match_pair(
        &self,
        a: TrackId,
        b: TrackId,
        params: &crate::pairing_params::PairingParams,
    ) -> Result<crate::matching::PairEvidence> {
        use crate::matching::{
            candidate_overlap_s, evidence_from_legacy, FingerprintSlot, MatchMethod, PairEvidence,
        };
        use crate::pairing_params::MatcherMode;

        let ta = self.track(a)?;
        let tb = self.track(b)?;
        let dur_a = ta.pyramid.duration_s();
        let dur_b = tb.pyramid.duration_s();
        let overlap = candidate_overlap_s(dur_a, dur_b);

        // Below the pairing floor: no candidate possible, no matcher runs (§9.3).
        if overlap < params.min_overlap_s {
            let method = match params.matcher_mode {
                MatcherMode::LegacyEnvelope => MatchMethod::EnvelopeV1,
                _ => MatchMethod::SpectralLandmarkV1,
            };
            return Ok(PairEvidence::insufficient_overlap(method, overlap));
        }

        match params.matcher_mode {
            // Legacy: the method-neutral wrapper around pair_correlate. NO fingerprint is built.
            MatcherMode::LegacyEnvelope => {
                let corr = crate::pair::pair_correlate(&ta.pyramid, &tb.pyramid, params);
                Ok(evidence_from_legacy(&corr, overlap, false))
            }
            // Fingerprint modes: try both fingerprints; a 1×1 batch through the shared orchestrator
            // so the §9.3 mapping and the legacy-fallback path are identical to match_candidates.
            mode @ (MatcherMode::FingerprintShadow | MatcherMode::FingerprintPrimary) => {
                let shadow = mode == MatcherMode::FingerprintShadow;
                // Build (or load) each fingerprint; an unavailable one ⇒ a None slot ⇒ legacy
                // fallback for the cell. A build failure is non-fatal here (the matcher degrades to
                // the legacy result), matching §9.3 / §16.2.
                let fp_a = ta.fingerprint_v1().ok();
                let fp_b = tb.fingerprint_v1().ok();
                let slot_a: FingerprintSlot = fp_a.as_deref();
                let slot_b: FingerprintSlot = fp_b.as_deref();
                let videos = [(slot_a, dur_a)];
                let audios = [(slot_b, dur_b)];
                let fmt = crate::fingerprint::FingerprintFormatV1::fixed();
                // The legacy correlation is computed lazily, only when the orchestrator asks (a
                // fallback cell, or shadow attachment) — a normal fingerprint cell pays no legacy
                // cost (§5.2). Compute it once and memoize within this call.
                let mut cached_legacy: Option<crate::pair::PairCorrelation> = None;
                // BOUNDARY 10 (§12): parse the per-pair metadata TimeHint ONCE from the two tracks'
                // probed MediaInfo (BWF/creation-time/device tags). Advisory + non-vetoing: the
                // orchestrator threads it into consensus where it MAY seed a hypothesis / break an
                // exact tie, never gate the lookup or thresholds. Absent metadata (synthetic
                // fixtures) ⇒ None ⇒ the no-hint consensus.
                let cell_hint =
                    crate::fingerprint::TimeHint::from_media(&ta.meta.info, &tb.meta.info);
                let (mut matrix, _stats) = crate::matching::match_candidates_fingerprint(
                    &videos,
                    &audios,
                    &fmt,
                    params,
                    shadow,
                    |_, _| {
                        if cached_legacy.is_none() {
                            cached_legacy =
                                Some(crate::pair::pair_correlate(&ta.pyramid, &tb.pyramid, params));
                        }
                        cached_legacy.clone()
                    },
                    // BOUNDARY 7 (§11): fine-refine an accepted hypothesis with the EXISTING local
                    // aligner over the two pyramids this Ctx holds. The orchestrator only calls this
                    // for a Match / strong-Ambiguous spectral cell, so a non-match pays nothing.
                    |_, _, lag_ms, drift_ppm, t_min, t_max| {
                        crate::match_refine::refine_affine(
                            &ta.pyramid,
                            &tb.pyramid,
                            lag_ms,
                            drift_ppm,
                            t_min,
                            t_max,
                        )
                    },
                    // BOUNDARY 10 (§12): the single (1×1) cell's metadata hint.
                    |_, _| cell_hint,
                );
                Ok(matrix.remove(0).remove(0))
            }
        }
    }

    /// Batch method-neutral evidence: rows = `videos`, columns = `audios` (§5.3). NEW additive API,
    /// NOT wired into the default resolver. Builds ONE audio-side inverted index for the whole
    /// batch (boundary 4's `AudioHashIndex`) in the fingerprint modes — it does NOT call a naive
    /// independent pair matcher per matrix cell (§5.3).
    ///
    /// Per-mode behavior mirrors [`Self::match_pair`]:
    ///
    /// - [`MatcherMode::LegacyEnvelope`] ⇒ a `pair_correlate`-derived `EnvelopeV1` matrix; NO
    ///   fingerprints are built (`pair_candidates`'s parallelism is reused);
    /// - fingerprint modes ⇒ build every audio's fingerprint once, build the ONE batch index, and
    ///   fill the matrix from it via the shared pure orchestrator (legacy fallback per §9.3 for any
    ///   track whose fingerprint is unavailable; shadow attaches the legacy correlation).
    pub fn match_candidates(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        params: &crate::pairing_params::PairingParams,
    ) -> Result<Vec<Vec<crate::matching::PairEvidence>>> {
        use crate::matching::{evidence_from_legacy, FingerprintSlot};
        use crate::pairing_params::MatcherMode;

        let vs: Vec<Arc<TrackData>> =
            videos.iter().map(|&id| self.track(id)).collect::<Result<_>>()?;
        let aus: Vec<Arc<TrackData>> =
            audios.iter().map(|&id| self.track(id)).collect::<Result<_>>()?;

        match params.matcher_mode {
            // Legacy: the method-neutral wrapper over the existing parallel pair_correlate matrix.
            // NO fingerprint is built (§5.2) — shadow computation is not an accidental cost here.
            MatcherMode::LegacyEnvelope => {
                use rayon::prelude::*;
                Ok(vs
                    .par_iter()
                    .map(|v| {
                        aus.iter()
                            .map(|a| {
                                let corr =
                                    crate::pair::pair_correlate(&v.pyramid, &a.pyramid, params);
                                let overlap = crate::matching::candidate_overlap_s(
                                    v.pyramid.duration_s(),
                                    a.pyramid.duration_s(),
                                );
                                evidence_from_legacy(&corr, overlap, false)
                            })
                            .collect()
                    })
                    .collect())
            }
            // Fingerprint modes: build each audio's fingerprint, then the ONE batch index inside the
            // orchestrator, then fill the matrix. A track whose fingerprint is unavailable falls
            // back to the legacy matcher for its cells (§9.3).
            mode @ (MatcherMode::FingerprintShadow | MatcherMode::FingerprintPrimary) => {
                let shadow = mode == MatcherMode::FingerprintShadow;
                // Build/load every fingerprint once (held alive for the borrow); None ⇒ fallback.
                let v_fps: Vec<Option<Arc<crate::fingerprint::FingerprintReader>>> =
                    vs.iter().map(|t| t.fingerprint_v1().ok()).collect();
                let a_fps: Vec<Option<Arc<crate::fingerprint::FingerprintReader>>> =
                    aus.iter().map(|t| t.fingerprint_v1().ok()).collect();
                let v_slots: Vec<(FingerprintSlot, f64)> = vs
                    .iter()
                    .zip(&v_fps)
                    .map(|(t, fp)| (fp.as_deref(), t.pyramid.duration_s()))
                    .collect();
                let a_slots: Vec<(FingerprintSlot, f64)> = aus
                    .iter()
                    .zip(&a_fps)
                    .map(|(t, fp)| (fp.as_deref(), t.pyramid.duration_s()))
                    .collect();
                let fmt = crate::fingerprint::FingerprintFormatV1::fixed();
                // Legacy correlations are computed lazily per (vi, ai) only when the orchestrator
                // needs one (a fallback cell or shadow attachment). A simple per-cell memo cache
                // (keyed by (vi, ai)) avoids recomputing within a single batch.
                let mut legacy_cache: std::collections::HashMap<(usize, usize), Option<crate::pair::PairCorrelation>> =
                    std::collections::HashMap::new();
                let (matrix, _stats) = crate::matching::match_candidates_fingerprint(
                    &v_slots,
                    &a_slots,
                    &fmt,
                    params,
                    shadow,
                    |vi, ai| {
                        legacy_cache
                            .entry((vi, ai))
                            .or_insert_with(|| {
                                Some(crate::pair::pair_correlate(
                                    &vs[vi].pyramid,
                                    &aus[ai].pyramid,
                                    params,
                                ))
                            })
                            .clone()
                    },
                    // BOUNDARY 7 (§11): fine-refine each accepted spectral cell with the existing
                    // local aligner over that cell's pyramids. Called per Match/strong-Ambiguous cell.
                    |vi, ai, lag_ms, drift_ppm, t_min, t_max| {
                        crate::match_refine::refine_affine(
                            &vs[vi].pyramid,
                            &aus[ai].pyramid,
                            lag_ms,
                            drift_ppm,
                            t_min,
                            t_max,
                        )
                    },
                    // BOUNDARY 10 (§12): per-(video, audio) metadata TimeHint parsed from each pair's
                    // probed MediaInfo. Deterministic, cheap, and ADVISORY — consensus may seed a
                    // hypothesis / break an exact tie with it but never gate on it (§12.3). Computed
                    // per cell only for the fingerprint cells the orchestrator actually consults.
                    |vi, ai| crate::fingerprint::TimeHint::from_media(&vs[vi].meta.info, &aus[ai].meta.info),
                );
                Ok(matrix)
            }
        }
    }

    /// GRAPH-ADJUSTED batch evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §13.4). NEW additive,
    /// OBSERVATIONAL-ONLY core operation: computes the per-pair `PairEvidence` over the FULL imported
    /// track graph (every track vs every other — §13.1 "node = imported track"), then applies the §13
    /// cross-device graph-consistency pass ([`crate::match_graph::adjust`]) — promoting an `Ambiguous`
    /// edge to `Match` (or demoting a `Match` to `Ambiguous`) ONLY when independent composed affine
    /// paths close per the §13.3 rules — and projects the video→audio cells back to the
    /// `videos × audios` shape.
    ///
    /// Building the WHOLE track graph (not just the bipartite `videos × audios` cells) is what makes
    /// cross-device triangles closeable: a video and several external recorders of one event corroborate
    /// via audio↔audio / video↔video composed maps, which a lone bipartite matrix cannot express. The
    /// cycle-residual window for each node is its pyramid coverage (track duration).
    ///
    /// Boundary 13 (§13.4) wired this into the DEFAULT resolution: the CLI `couple pair` resolver and
    /// the Swift `Resolver` now consume this graph-adjusted matrix as the identity authority (the
    /// `--matcher legacy` developer override keeps the envelope resolver). The returned matrix has the
    /// SAME shape as [`Self::match_candidates`] and differs only in the `classification` of the cells
    /// the §13.3 rules adjust. In `LegacyEnvelope` mode there is no affine landmark mapping, so the
    /// graph pass is a no-op and this returns the legacy `videos × audios` matrix unchanged.
    pub fn match_candidates_graph(
        &self,
        videos: &[TrackId],
        audios: &[TrackId],
        params: &crate::pairing_params::PairingParams,
    ) -> Result<Vec<Vec<crate::matching::PairEvidence>>> {
        use crate::pairing_params::MatcherMode;

        // LEGACY mode has no affine landmark map for the graph to compose — return the plain
        // bipartite matrix unchanged (graph consistency is a fingerprint-path facility).
        if params.matcher_mode == MatcherMode::LegacyEnvelope {
            return self.match_candidates(videos, audios, params);
        }

        // The unified node set = videos followed by audios, deduplicated by TrackId (a track may be
        // queried as both a video and an audio in odd setups; the graph wants one node per track).
        // We keep videos' positions stable (their node index is their `videos` index) and append the
        // audios that are not already a video node.
        let mut node_ids: Vec<TrackId> = Vec::with_capacity(videos.len() + audios.len());
        node_ids.extend_from_slice(videos);
        let mut audio_node_of: Vec<usize> = Vec::with_capacity(audios.len());
        for &aid in audios {
            let idx = node_ids.iter().position(|&n| n == aid).unwrap_or_else(|| {
                node_ids.push(aid);
                node_ids.len() - 1
            });
            audio_node_of.push(idx);
        }

        // One full-graph matcher run: every node vs every node (§13.1). `match_candidates` builds the
        // ONE audio-side index for the batch; here the "audios" side is the full node set, so each
        // directed (i, j) cell is the evidence for that ordered pair. We then DROP the diagonal.
        let full = self.match_candidates(&node_ids, &node_ids, params)?;
        let n = node_ids.len();
        let mut directed: Vec<Vec<Option<crate::matching::PairEvidence>>> =
            Vec::with_capacity(n);
        for (i, row) in full.into_iter().enumerate() {
            let mut out_row = Vec::with_capacity(n);
            for (j, cell) in row.into_iter().enumerate() {
                out_row.push(if i == j { None } else { Some(cell) });
            }
            directed.push(out_row);
        }

        // Per-node coverage = track duration (the §13.2 residual window).
        let coverage: Vec<f64> = node_ids
            .iter()
            .map(|&id| self.track(id).map(|t| t.pyramid.duration_s()))
            .collect::<Result<_>>()?;

        let adjusted = crate::match_graph::adjust(&directed, &coverage, params);

        // Project the requested video→audio cells back to the `videos × audios` shape. Video `vi` is
        // node `vi`; audio `ai` is node `audio_node_of[ai]`.
        let mut out: Vec<Vec<crate::matching::PairEvidence>> = Vec::with_capacity(videos.len());
        for (vi, _vid) in videos.iter().enumerate() {
            let mut row = Vec::with_capacity(audios.len());
            for (ai, _aid) in audios.iter().enumerate() {
                let a_node = audio_node_of[ai];
                // The diagonal (a video used as its own audio) has no edge cell; fall back to the
                // direct evidence so the returned matrix is always fully populated.
                let cell = adjusted[vi][a_node]
                    .clone()
                    .unwrap_or_else(|| self.match_pair(videos[vi], audios[ai], params).unwrap_or_else(|_| {
                        crate::matching::PairEvidence::unavailable(
                            crate::matching::MatchMethod::SpectralLandmarkV1,
                            0.0,
                        )
                    }));
                row.push(cell);
            }
            out.push(row);
        }
        Ok(out)
    }

    // ---- sessions (§4.5) ----

    pub fn session_new(&self, camera: TrackId, external: TrackId) -> Result<SessionId> {
        self.track(camera)?;
        self.track(external)?;
        let id = self.next_id();
        let s = Arc::new(crate::session::Session::new(id, camera, external));
        self.sessions.write().unwrap().insert(id, s);
        Ok(id)
    }

    pub fn session(&self, id: SessionId) -> Result<Arc<crate::session::Session>> {
        self.sessions
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| CoupleError::invalid(format!("unknown session {id}")))
    }

    pub fn session_free(&self, id: SessionId) {
        self.sessions.write().unwrap().remove(&id);
    }

    /// §4.4 measure_local on a session. Synchronous, ≤ 5 ms budget; the caller (shell)
    /// owns debounce policy and writes results into state.
    pub fn measure_local(
        &self,
        session: SessionId,
        tc: f64,
        half_win_s: f64,
        center_ms: f64,
        span_ms: f64,
    ) -> Result<Option<crate::session::LocalMeasurement>> {
        let s = self.session(session)?;
        let cam = self.track(s.camera)?;
        let ext = self.track(s.external)?;
        Ok(crate::measure::measure_local(
            &cam.pyramid,
            &ext.pyramid,
            tc,
            half_win_s,
            center_ms,
            span_ms,
        ))
    }

    /// Blocking full auto-sync on a session (§4.4 semantics): on Synced/LowConfidence
    /// core writes offset/drift/windows + suggested mode into the session state and
    /// leaves verified = false; on Failed the state is untouched (UX §3).
    pub fn auto_sync_blocking(
        &self,
        session: SessionId,
        params: &crate::sync::SyncParams,
        progress: ProgressFn,
        cancel: &CancelToken,
        job_id: JobId,
    ) -> Result<crate::sync::SyncResult> {
        let s = self.session(session)?;
        let cam = self.track(s.camera)?;
        let ext = self.track(s.external)?;
        let mut prog = ProgressEmitter::new(job_id, progress);
        let result =
            crate::sync::auto_sync_tracks(&cam.pyramid, &ext.pyramid, params, &mut prog, cancel)?;
        if !matches!(result.status, crate::sync::SyncStatus::Failed(_)) {
            let windows = result
                .windows
                .iter()
                .filter(|w| w.accepted)
                .map(|w| crate::session::DriftWindowState {
                    t: w.t,
                    lag_ms: w.lag_ms,
                    ncc: w.ncc,
                })
                .collect();
            s.apply_sync(
                result.offset_ms,
                result.drift_ppm,
                windows,
                result.suggested_drift_mode,
            );
        }
        Ok(result)
    }
}
