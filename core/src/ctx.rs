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
        params: &crate::sync::SyncParams,
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
        params: &crate::sync::SyncParams,
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
