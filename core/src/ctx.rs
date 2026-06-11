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
        })
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

    pub fn cache_gc(&self, max_bytes: u64) -> u64 {
        cache::gc(&self.cache_root, max_bytes)
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
}
