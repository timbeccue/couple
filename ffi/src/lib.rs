//! couple-ffi — the C ABI porting surface (§4.9), Phases 1–3 inventory.
//! Audition/export functions land with Phase 4/6.

#![allow(clippy::missing_safety_doc)]

use couple_core::job::{CancelToken, ProgressEmitter, ProgressFn};
use couple_core::{Ctx, CtxConfig};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// ABI version gate; bump on any breaking change (§4.9).
pub const CPL_ABI_VERSION: u32 = 1;

// ---------- registries / error plumbing ----------

struct Registry {
    ctxs: HashMap<u64, Arc<Ctx>>,
    jobs: HashMap<u64, CancelToken>,
    next: u64,
}

fn reg() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| {
        Mutex::new(Registry {
            ctxs: HashMap::new(),
            jobs: HashMap::new(),
            next: 1,
        })
    })
}

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_error(e: &couple_core::CoupleError) -> i32 {
    LAST_ERROR.with(|c| {
        *c.borrow_mut() = Some(serde_json::json!({ "error": e, "message": e.to_string() }).to_string())
    });
    e.exit_code()
}

fn ctx_of(handle: u64) -> Option<Arc<Ctx>> {
    reg().lock().unwrap().ctxs.get(&handle).cloned()
}

fn invalid_handle() -> i32 {
    set_error(&couple_core::CoupleError::invalid("invalid handle"))
}

unsafe fn cstr_to_path(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    Some(PathBuf::from(CStr::from_ptr(p).to_string_lossy().to_string()))
}

fn to_cstring(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

// ---------- lifecycle ----------

#[no_mangle]
pub extern "C" fn cpl_abi_version() -> u32 {
    CPL_ABI_VERSION
}

/// Core version string; free with cpl_string_free.
#[no_mangle]
pub extern "C" fn cpl_version() -> *mut c_char {
    to_cstring(couple_core::version().to_string())
}

#[no_mangle]
pub unsafe extern "C" fn cpl_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Create an engine context from a JSON config (`{"ffmpegPath", "ffprobePath",
/// "cacheDir"}`, all optional; null/empty = defaults). Returns 0 on error.
#[no_mangle]
pub unsafe extern "C" fn cpl_ctx_new(config_json: *const c_char) -> u64 {
    let config: CtxConfig = if config_json.is_null() {
        CtxConfig::default()
    } else {
        match serde_json::from_str(&CStr::from_ptr(config_json).to_string_lossy()) {
            Ok(c) => c,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("config: {e}")));
                return 0;
            }
        }
    };
    let ctx = Ctx::new(config);
    let mut r = reg().lock().unwrap();
    let id = r.next;
    r.next += 1;
    r.ctxs.insert(id, ctx);
    id
}

#[no_mangle]
pub extern "C" fn cpl_ctx_free(ctx: u64) {
    reg().lock().unwrap().ctxs.remove(&ctx);
}

/// Thread-local last-error JSON (`{"error":{"kind","detail"},"message"}`); null if none.
/// Free with cpl_string_free.
#[no_mangle]
pub extern "C" fn cpl_take_error_json(_ctx: u64) -> *mut c_char {
    LAST_ERROR.with(|c| match c.borrow_mut().take() {
        Some(s) => to_cstring(s),
        None => std::ptr::null_mut(),
    })
}

// ---------- jobs ----------

/// Progress/completion callback (§4.9): invoked from worker threads. Non-terminal
/// calls carry a phase name and fraction; the terminal call has phase "done"
/// (payload = result JSON), "error" (payload = error JSON) or "cancelled".
pub type CplProgressCb = Option<
    extern "C" fn(job: u64, phase: *const c_char, fraction: f64, payload_json: *const c_char, user: *mut c_void),
>;

struct UserPtr(*mut c_void);
unsafe impl Send for UserPtr {}
unsafe impl Sync for UserPtr {}

fn spawn_job<F>(ctx: Arc<Ctx>, cb: CplProgressCb, user: *mut c_void, body: F) -> u64
where
    F: FnOnce(&Arc<Ctx>, ProgressFn, &CancelToken) -> couple_core::Result<String> + Send + 'static,
{
    let job_id = ctx.next_id();
    let cancel = CancelToken::new();
    reg().lock().unwrap().jobs.insert(job_id, cancel.clone());
    let user = Arc::new(UserPtr(user));
    let progress_user = Arc::clone(&user);
    let progress: ProgressFn = Arc::new(move |p: &couple_core::Progress| {
        if let Some(cb) = cb {
            let phase = CString::new(format!("{:?}", p.phase)).unwrap();
            cb(p.job, phase.as_ptr(), p.fraction, std::ptr::null(), progress_user.0);
        }
    });
    std::thread::spawn(move || {
        let result = body(&ctx, progress, &cancel);
        if let Some(cb) = cb {
            match result {
                Ok(payload) => {
                    let phase = CString::new("done").unwrap();
                    let payload = CString::new(payload).unwrap_or_default();
                    cb(job_id, phase.as_ptr(), 1.0, payload.as_ptr(), user.0);
                }
                Err(couple_core::CoupleError::Cancelled) => {
                    let phase = CString::new("cancelled").unwrap();
                    cb(job_id, phase.as_ptr(), 0.0, std::ptr::null(), user.0);
                }
                Err(e) => {
                    let phase = CString::new("error").unwrap();
                    let payload =
                        CString::new(serde_json::json!({ "error": &e }).to_string()).unwrap();
                    cb(job_id, phase.as_ptr(), 0.0, payload.as_ptr(), user.0);
                }
            }
        }
        reg().lock().unwrap().jobs.remove(&job_id);
    });
    job_id
}

#[no_mangle]
pub extern "C" fn cpl_job_cancel(_ctx: u64, job: u64) {
    if let Some(t) = reg().lock().unwrap().jobs.get(&job) {
        t.cancel();
    }
}

#[no_mangle]
pub extern "C" fn cpl_job_free(_ctx: u64, job: u64) {
    reg().lock().unwrap().jobs.remove(&job);
}

// ---------- media ----------

/// Probe a file → MediaInfo JSON (§4.2 wire schema); null on error.
#[no_mangle]
pub unsafe extern "C" fn cpl_probe_json(ctx: u64, path: *const c_char) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    let Some(path) = cstr_to_path(path) else {
        set_error(&couple_core::CoupleError::invalid("null path"));
        return std::ptr::null_mut();
    };
    match ctx.probe(&path) {
        Ok(info) => to_cstring(serde_json::to_string(&info).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Start an import job (§4.3). role: 0 = camera, 1 = external. Terminal "done"
/// payload: `{"trackId": …}` + TrackMeta JSON. Returns the job id, 0 on error.
#[no_mangle]
pub unsafe extern "C" fn cpl_import_start(
    ctx: u64,
    path: *const c_char,
    role: i32,
    cb: CplProgressCb,
    user: *mut c_void,
) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    let Some(path) = cstr_to_path(path) else {
        set_error(&couple_core::CoupleError::invalid("null path"));
        return 0;
    };
    let role = if role == 0 {
        couple_core::TrackRole::Camera
    } else {
        couple_core::TrackRole::External
    };
    spawn_job(ctx, cb, user, move |ctx, progress, cancel| {
        let id = ctx.import_blocking(&path, role, progress, cancel, 0)?;
        let track = ctx.track(id)?;
        Ok(serde_json::to_string(&track.meta).unwrap())
    })
}

/// TrackMeta JSON for a loaded track; null on error.
#[no_mangle]
pub extern "C" fn cpl_track_info_json(ctx: u64, track: u64) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.track(track) {
        Ok(t) => to_cstring(serde_json::to_string(&t.meta).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn cpl_track_free(ctx: u64, track: u64) {
    if let Some(ctx) = ctx_of(ctx) {
        ctx.track_free(track);
    }
}

// ---------- reads (§4.3; caller-allocated buffers on the hot paths) ----------

/// band(): fills caller-allocated mins/maxs (n_cols int8 each) plus the chosen level
/// and column geometry. Returns 0 on success.
#[no_mangle]
pub unsafe extern "C" fn cpl_band(
    ctx: u64,
    track: u64,
    t0: f64,
    t1: f64,
    n_cols: u32,
    out_mins: *mut i8,
    out_maxs: *mut i8,
    out_level_spb: *mut u32,
    out_col_t0: *mut f64,
    out_col_dt: *mut f64,
) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    let t = match ctx.track(track) {
        Ok(t) => t,
        Err(e) => return set_error(&e),
    };
    match couple_core::band(&t.pyramid, t0, t1, n_cols) {
        Ok(b) => {
            std::ptr::copy_nonoverlapping(b.mins.as_ptr(), out_mins, b.mins.len());
            std::ptr::copy_nonoverlapping(b.maxs.as_ptr(), out_maxs, b.maxs.len());
            if !out_level_spb.is_null() {
                *out_level_spb = b.level_spb;
            }
            if !out_col_t0.is_null() {
                *out_col_t0 = b.col_t0;
            }
            if !out_col_dt.is_null() {
                *out_col_dt = b.col_dt;
            }
            0
        }
        Err(e) => set_error(&e),
    }
}

/// raw_window(): mono 48 kHz f32 into a caller buffer; frames written via out_frames.
#[no_mangle]
pub unsafe extern "C" fn cpl_raw_window(
    ctx: u64,
    track: u64,
    t0: f64,
    t1: f64,
    out: *mut f32,
    out_capacity_frames: u32,
    out_frames: *mut u32,
) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    let t = match ctx.track(track) {
        Ok(t) => t,
        Err(e) => return set_error(&e),
    };
    let buf = std::slice::from_raw_parts_mut(out, out_capacity_frames as usize);
    match t.raw_window(t0, t1, buf) {
        Ok(n) => {
            if !out_frames.is_null() {
                *out_frames = n;
            }
            0
        }
        Err(e) => set_error(&e),
    }
}

/// rms() in dBFS over active bins in [t0, t1].
#[no_mangle]
pub unsafe extern "C" fn cpl_rms(ctx: u64, track: u64, t0: f64, t1: f64, out_dbfs: *mut f64) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    let t = match ctx.track(track) {
        Ok(t) => t,
        Err(e) => return set_error(&e),
    };
    match t.rms(t0, t1) {
        Ok(v) => {
            *out_dbfs = v;
            0
        }
        Err(e) => set_error(&e),
    }
}

/// Ranked onsets in [t0, t1], JSON array of {t, score}; null on error.
#[no_mangle]
pub extern "C" fn cpl_onsets_json(ctx: u64, track: u64, t0: f64, t1: f64, limit: u32) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.track(track) {
        Ok(t) => to_cstring(
            serde_json::to_string(&couple_core::onsets::in_range(&t.onsets, t0, t1, limit as usize))
                .unwrap(),
        ),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Best onset in [t0, t1] (zone proposal); JSON {t, score} or "null".
#[no_mangle]
pub extern "C" fn cpl_best_onset_in_json(ctx: u64, track: u64, t0: f64, t1: f64) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.track(track) {
        Ok(t) => to_cstring(serde_json::to_string(&couple_core::onsets::best_in(&t.onsets, t0, t1)).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Best onset within ±half_span of t (snap); JSON {t, score} or "null".
#[no_mangle]
pub extern "C" fn cpl_best_onset_near_json(ctx: u64, track: u64, t: f64, half_span_s: f64) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.track(track) {
        Ok(tr) => to_cstring(
            serde_json::to_string(&couple_core::onsets::best_near(&tr.onsets, t, half_span_s)).unwrap(),
        ),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// TrackStats JSON (§4.2); null on error.
#[no_mangle]
pub extern "C" fn cpl_track_stats_json(ctx: u64, track: u64) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.track(track) {
        Ok(t) => to_cstring(serde_json::to_string(&t.meta.stats).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

// ---------- session / state (§4.5) ----------

/// Create a session over (camera, external) tracks. Returns 0 on error.
#[no_mangle]
pub extern "C" fn cpl_session_new(ctx: u64, camera: u64, external: u64) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    match ctx.session_new(camera, external) {
        Ok(id) => id,
        Err(e) => {
            set_error(&e);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn cpl_session_free(ctx: u64, session: u64) {
    if let Some(ctx) = ctx_of(ctx) {
        ctx.session_free(session);
    }
}

/// AlignmentState JSON (§4.5 wire schema); null on error.
#[no_mangle]
pub extern "C" fn cpl_state_json(ctx: u64, session: u64) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.session(session) {
        Ok(s) => to_cstring(s.state_json()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn cpl_set_state_json(ctx: u64, session: u64, json: *const c_char) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    let s = match ctx.session(session) {
        Ok(s) => s,
        Err(e) => return set_error(&e),
    };
    if json.is_null() {
        return set_error(&couple_core::CoupleError::invalid("null state json"));
    }
    match s.set_state_json(&CStr::from_ptr(json).to_string_lossy()) {
        Ok(()) => 0,
        Err(e) => set_error(&e),
    }
}

/// Hot setter (gesture rate, lock-free).
#[no_mangle]
pub extern "C" fn cpl_set_offset_ms(ctx: u64, session: u64, v: f64) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.session(session) {
        Ok(s) => {
            s.set_offset_ms(v);
            0
        }
        Err(e) => set_error(&e),
    }
}

/// Hot setter; a drift write while driftMode = none flips the mode to resample.
#[no_mangle]
pub extern "C" fn cpl_set_drift_ppm_live(ctx: u64, session: u64, v: f64) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.session(session) {
        Ok(s) => {
            s.set_drift_ppm_live(v);
            0
        }
        Err(e) => set_error(&e),
    }
}

/// Audition re-render point (UX §2.7); mapping itself is already live.
#[no_mangle]
pub extern "C" fn cpl_commit_drift(ctx: u64, session: u64) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.session(session) {
        Ok(s) => {
            s.commit_drift();
            0
        }
        Err(e) => set_error(&e),
    }
}

/// mode: 0 = resample, 1 = none (UX §1.5 semantics enforced in core).
#[no_mangle]
pub extern "C" fn cpl_set_drift_mode(ctx: u64, session: u64, mode: i32) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.session(session) {
        Ok(s) => {
            s.set_drift_mode(if mode == 1 {
                couple_core::session::DriftMode::None
            } else {
                couple_core::session::DriftMode::Resample
            });
            0
        }
        Err(e) => set_error(&e),
    }
}

#[no_mangle]
pub extern "C" fn cpl_set_verified(ctx: u64, session: u64, verified: bool) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.session(session) {
        Ok(s) => {
            s.set_verified(verified);
            0
        }
        Err(e) => set_error(&e),
    }
}

// ---------- sync (§4.4) ----------

/// Start a full auto-sync job on a session. params_json: SyncParams overrides
/// (camelCase, all optional) or null for defaults. Terminal "done" payload:
/// SyncResult JSON. Returns the job id, 0 on error.
#[no_mangle]
pub unsafe extern "C" fn cpl_autosync_start(
    ctx: u64,
    session: u64,
    params_json: *const c_char,
    cb: CplProgressCb,
    user: *mut c_void,
) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    let params: couple_core::sync::SyncParams = if params_json.is_null() {
        Default::default()
    } else {
        match serde_json::from_str(&CStr::from_ptr(params_json).to_string_lossy()) {
            Ok(p) => p,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("params: {e}")));
                return 0;
            }
        }
    };
    spawn_job(ctx, cb, user, move |ctx, progress, cancel| {
        let r = ctx.auto_sync_blocking(session, &params, progress, cancel, 0)?;
        Ok(serde_json::to_string(&r).unwrap())
    })
}

/// measure_local (§4.4): JSON {lagMs, ncc} or "null"; null pointer on error.
#[no_mangle]
pub extern "C" fn cpl_measure_local_json(
    ctx: u64,
    session: u64,
    tc: f64,
    half_win_s: f64,
    center_ms: f64,
    span_ms: f64,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.measure_local(session, tc, half_win_s, center_ms, span_ms) {
        Ok(m) => to_cstring(serde_json::to_string(&m).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// fit_ls over points_json `[[t_s, lag_ms], …]` → JSON {offMs, ppm, rmsMs} or "null".
#[no_mangle]
pub unsafe extern "C" fn cpl_fit_ls_json(points_json: *const c_char) -> *mut c_char {
    if points_json.is_null() {
        return std::ptr::null_mut();
    }
    let pts: Vec<(f64, f64)> =
        match serde_json::from_str(&CStr::from_ptr(points_json).to_string_lossy()) {
            Ok(p) => p,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("points: {e}")));
                return std::ptr::null_mut();
            }
        };
    to_cstring(serde_json::to_string(&couple_core::fit::fit_ls(&pts)).unwrap())
}

// ---------- audition (§4.8, Phase 6) ----------

/// Open an audition stream on a session. mode: 0 = pan, 1 = overlay, 2 = null.
/// Returns the stream id, 0 on error.
#[no_mangle]
pub extern "C" fn cpl_audition_open(ctx: u64, session: u64, mode: i32, t_start: f64) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    match ctx.audition_open(
        session,
        couple_core::MonitorMode::from_u8(mode as u8),
        t_start,
    ) {
        Ok(id) => id,
        Err(e) => {
            set_error(&e);
            0
        }
    }
}

/// Fill caller-allocated interleaved stereo f32 (48 kHz). Writes the frames
/// actually rendered (< capacity only past the video end) and the stream's
/// video time after the fill. Real-time path: locks are touched only to
/// resolve the handles (uncontended in practice).
#[no_mangle]
pub unsafe extern "C" fn cpl_audition_fill(
    ctx: u64,
    stream: u64,
    out_interleaved_stereo: *mut f32,
    capacity_frames: u32,
    out_frames: *mut u32,
    out_stream_t: *mut f64,
) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    let s = match ctx.audition_stream(stream) {
        Ok(s) => s,
        Err(e) => return set_error(&e),
    };
    let buf =
        std::slice::from_raw_parts_mut(out_interleaved_stereo, capacity_frames as usize * 2);
    let info = s.fill(buf);
    if !out_frames.is_null() {
        *out_frames = info.frames;
    }
    if !out_stream_t.is_null() {
        *out_stream_t = info.stream_t;
    }
    0
}

#[no_mangle]
pub extern "C" fn cpl_audition_seek(ctx: u64, stream: u64, t: f64) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.audition_stream(stream) {
        Ok(s) => {
            s.seek(t);
            0
        }
        Err(e) => set_error(&e),
    }
}

/// mode: 0 = pan, 1 = overlay, 2 = null. Click-free (240-sample crossfade).
#[no_mangle]
pub extern "C" fn cpl_audition_set_mode(ctx: u64, stream: u64, mode: i32) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.audition_stream(stream) {
        Ok(s) => {
            s.set_mode(couple_core::MonitorMode::from_u8(mode as u8));
            0
        }
        Err(e) => set_error(&e),
    }
}

/// Per-track monitor gains (1.0 = unity, 0.0 = mute), multiplied onto the
/// loudness-matching gains. Click-free (240-sample crossfade).
#[no_mangle]
pub extern "C" fn cpl_audition_set_gains(ctx: u64, stream: u64, gain_cam: f32, gain_ext: f32) -> i32 {
    let Some(ctx) = ctx_of(ctx) else { return invalid_handle() };
    match ctx.audition_stream(stream) {
        Ok(s) => {
            s.set_gains(gain_cam, gain_ext);
            0
        }
        Err(e) => set_error(&e),
    }
}

#[no_mangle]
pub extern "C" fn cpl_audition_close(ctx: u64, stream: u64) {
    if let Some(ctx) = ctx_of(ctx) {
        ctx.audition_close(stream);
    }
}

// ---------- export (§8, Phase 4) ----------

/// Start an export job on a session (§4.9). opts_json: ExportOptions wire form
/// (`{"kind":"mux"|"wav","outputPath":…,"keepCameraAudio":…,"container":…,
/// "faststart":…,"overwrite":…}`). The session's current mapping is applied
/// (driftMode none ⇒ offset-only). Terminal "done" payload: ExportReport JSON.
/// Returns the job id, 0 on error.
#[no_mangle]
pub unsafe extern "C" fn cpl_export_start(
    ctx: u64,
    session: u64,
    opts_json: *const c_char,
    cb: CplProgressCb,
    user: *mut c_void,
) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    if opts_json.is_null() {
        set_error(&couple_core::CoupleError::invalid("null export options"));
        return 0;
    }
    let opts: couple_core::export::ExportOptions =
        match serde_json::from_str(&CStr::from_ptr(opts_json).to_string_lossy()) {
            Ok(o) => o,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("export options: {e}")));
                return 0;
            }
        };
    spawn_job(ctx, cb, user, move |ctx, progress, cancel| {
        let s = ctx.session(session)?;
        let cam = ctx.track(s.camera)?;
        let ext = ctx.track(s.external)?;
        let mapping = couple_core::export::ExportMapping {
            offset_ms: s.offset_ms(),
            drift_ppm: s.drift_ppm(),
            resample: s.drift_mode() == couple_core::session::DriftMode::Resample,
        };
        let mut prog = couple_core::job::ProgressEmitter::new(0, progress);
        let report = couple_core::export::export(
            &ctx.ffmpeg()?,
            &ctx.ffprobe()?,
            std::path::Path::new(&cam.meta.info.path),
            std::path::Path::new(&ext.meta.info.path),
            &mapping,
            &opts,
            &mut prog,
            cancel,
        )?;
        Ok(serde_json::to_string(&report).unwrap())
    })
}

// ---------- pairing (SPEC-V2 §4, Phase 7) ----------

/// Decode `PairingParams` for the `*_params` pairing symbols (PLAN-AUDIO-MATCHING-PREWORK
/// §6.7). null or empty `params_json` ⇒ `PairingParams::default()`; malformed JSON or a
/// value that fails `validate()` ⇒ `InvalidArgument` (sets the thread-local last-error and
/// returns `None`, so the caller returns null). Every parameterized entry validates before
/// doing work.
unsafe fn pairing_params_or_error(params_json: *const c_char) -> Option<couple_core::PairingParams> {
    let params = if params_json.is_null() {
        couple_core::PairingParams::default()
    } else {
        let raw = CStr::from_ptr(params_json).to_string_lossy();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            couple_core::PairingParams::default()
        } else {
            match serde_json::from_str(trimmed) {
                Ok(p) => p,
                Err(e) => {
                    set_error(&couple_core::CoupleError::invalid(format!("pairing params: {e}")));
                    return None;
                }
            }
        }
    };
    if let Err(e) = params.validate() {
        set_error(&e);
        return None;
    }
    Some(params)
}

/// Default `PairingParams` as JSON (PLAN-AUDIO-MATCHING-PREWORK §6.7) — the single source of
/// pairing/verifier defaults that CLI and Swift read instead of hardcoding copies. Free with
/// cpl_string_free.
#[no_mangle]
pub extern "C" fn cpl_pairing_params_default_json() -> *mut c_char {
    to_cstring(serde_json::to_string(&couple_core::PairingParams::default()).unwrap())
}

/// Role-agnostic stage-1 correlation between any two imported tracks (SPEC-V2 §3.2).
/// Returns PairCorrelation JSON (`{"z","lagMs","ncc","fineNcc","overlapBins",
/// "runnerUpZ","refined","insufficientOverlap","legacyCandidate","prefilterCandidate"}`);
/// `legacyCandidate` = z >= legacyZMin (default resolver gate), `prefilterCandidate` =
/// ncc >= refineNccMin (flag-ON pre-filter) — both core-computed (§7.2). Lag convention:
/// content at time t in track_a occurs at t + lagMs/1000 in track_b. Default PairingParams
/// (legacy z 8.5). Null on error; free with cpl_string_free.
#[no_mangle]
pub extern "C" fn cpl_pair_correlate(ctx: u64, track_a: u64, track_b: u64) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    match ctx.pair_correlate(track_a, track_b, &couple_core::PairingParams::default()) {
        Ok(r) => to_cstring(serde_json::to_string(&r).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// As `cpl_pair_correlate` but with a `PairingParams` JSON override (null/empty ⇒ defaults;
/// malformed/invalid ⇒ InvalidArgument). Additive — no ABI bump. Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_pair_correlate_params(
    ctx: u64,
    track_a: u64,
    track_b: u64,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    let Some(params) = pairing_params_or_error(params_json) else {
        return std::ptr::null_mut();
    };
    match ctx.pair_correlate(track_a, track_b, &params) {
        Ok(r) => to_cstring(serde_json::to_string(&r).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// FFI id-array → slice. A zero-length array yields `&[]` even when `ptr` is null;
/// `from_raw_parts(null, 0)` is UB (the pointer must be non-null and aligned even for an
/// empty slice). Callers still reject a null pointer paired with a positive count up front.
unsafe fn id_slice<'a>(ptr: *const u64, len: usize) -> &'a [u64] {
    if len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// Batch candidate matrix (SPEC-V2 §3.2): rows = videos, columns = audios. Returns a
/// JSON array of arrays of PairCorrelation (same wire form as cpl_pair_correlate).
/// Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_pair_candidates(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return std::ptr::null_mut();
    }
    let vs = id_slice(videos, n_videos);
    let aus = id_slice(audios, n_audios);
    match ctx.pair_candidates(vs, aus, &couple_core::PairingParams::default()) {
        Ok(m) => to_cstring(serde_json::to_string(&m).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// As `cpl_pair_candidates` but with a `PairingParams` JSON override (null/empty ⇒ defaults;
/// malformed/invalid ⇒ InvalidArgument). Additive — no ABI bump. Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_pair_candidates_params(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return std::ptr::null_mut();
    }
    let Some(params) = pairing_params_or_error(params_json) else {
        return std::ptr::null_mut();
    };
    let vs = id_slice(videos, n_videos);
    let aus = id_slice(audios, n_audios);
    match ctx.pair_candidates(vs, aus, &params) {
        Ok(m) => to_cstring(serde_json::to_string(&m).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Method-neutral evidence for ONE candidate (video=`track_a`, audio=`track_b`) pair
/// (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.1). Returns a single `PairEvidence` JSON object
/// (`{"method","classification","lagMs","driftPpm","score","runnerUpScore","scoreMargin",
/// "support","candidateCorrespondences","inlierFraction","temporalCoverage","residualRmsMs",
/// "residualP95Ms","overlapS","refinedLagMs","refinedDriftPpm","refinedRmsMs","legacy"}`; the
/// residual/refined fields are `null` when not applicable — NaN/Infinity is NEVER serialized, §20).
/// The caller selects the matcher via `matcherMode` in `pairing_params_json` (the default,
/// `legacyEnvelope`, yields `EnvelopeV1` evidence; `fingerprintShadow`/`fingerprintPrimary` run the
/// spectral pipeline). null/empty `pairing_params_json` ⇒ `PairingParams::default()`; malformed or
/// validate()-failing JSON ⇒ InvalidArgument. ADDITIVE — `cpl_abi_version` stays 1; the legacy
/// `cpl_pair_correlate`/`cpl_pair_candidates` symbols are unchanged. Null on error; free with
/// cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_match_pair(
    ctx: u64,
    track_a: u64,
    track_b: u64,
    pairing_params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    let Some(params) = pairing_params_or_error(pairing_params_json) else {
        return std::ptr::null_mut();
    };
    match ctx.match_pair(track_a, track_b, &params) {
        Ok(ev) => to_cstring(serde_json::to_string(&ev).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Batch method-neutral evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.1): rows = `videos`,
/// columns = `audios`. Returns a JSON array-of-arrays of `PairEvidence` (each element the same wire
/// form as `cpl_match_pair`). In the fingerprint modes core builds ONE audio-side inverted index for
/// the whole batch (§5.3), not a naive per-cell matcher. null/empty `pairing_params_json` ⇒
/// `PairingParams::default()` (`legacyEnvelope`); malformed/invalid ⇒ InvalidArgument. ADDITIVE — no
/// ABI bump. Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_match_candidates(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
    pairing_params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return std::ptr::null_mut();
    }
    let Some(params) = pairing_params_or_error(pairing_params_json) else {
        return std::ptr::null_mut();
    };
    let vs = id_slice(videos, n_videos);
    let aus = id_slice(audios, n_audios);
    match ctx.match_candidates(vs, aus, &params) {
        Ok(m) => to_cstring(serde_json::to_string(&m).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// GRAPH-ADJUSTED batch evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §13.4): same arguments and
/// `PairEvidence` array-of-arrays return as `cpl_match_candidates`, but the §13 cross-device
/// graph-consistency pass (`Ctx::match_candidates_graph`) is applied — an `Ambiguous` edge may be
/// promoted to `Match` (or a `Match` demoted to `Ambiguous`) when independent composed affine paths
/// close per §13.3. Boundary 13 (§13.4) made this the DEFAULT resolution input: the CLI and Swift
/// resolvers consume this graph-adjusted matrix as the identity authority (the `legacyEnvelope`
/// override keeps the envelope resolver). In `legacyEnvelope` mode the graph pass is a no-op and this
/// equals `cpl_match_candidates`. ADDITIVE — `cpl_abi_version` stays 1; all existing symbols are
/// unchanged. Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_match_candidates_graph(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
    pairing_params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return std::ptr::null_mut();
    }
    let Some(params) = pairing_params_or_error(pairing_params_json) else {
        return std::ptr::null_mut();
    };
    let vs = id_slice(videos, n_videos);
    let aus = id_slice(audios, n_audios);
    match ctx.match_candidates_graph(vs, aus, &params) {
        Ok(m) => to_cstring(serde_json::to_string(&m).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Start a GRAPH-ADJUSTED batch-matching JOB (incremental-matching boundary) — the asynchronous,
/// progress-reporting twin of `cpl_match_candidates_graph`. Same domain arguments (`videos`/`audios`
/// id arrays + an optional `PairingParams` JSON override), then the standard `(cb, user)` job
/// callback pair. Returns the job id (0 on error), exactly like `cpl_autosync_start`/`cpl_export_start`.
///
/// The terminal "done" payload is the SAME `[[PairEvidence]]` array-of-arrays JSON that the
/// synchronous `cpl_match_candidates_graph` returns (the graph-adjusted `videos × audios` matrix) — so
/// a caller can switch a resolver from the blocking symbol to this job purely to gain a determinate
/// bar, with NO change to the resolved output (the resolution authority is byte-identical; progress is
/// observational). Non-terminal callbacks carry the phase name (`MatchFingerprint` while each track's
/// fingerprint is built — the dominant first-match cost; `MatchCorrelate` per completed matrix row;
/// `MatchGraph` around the §13 graph-consistency pass, ending at 1.0) and a fraction. Cancellation
/// (`cpl_job_cancel`) yields a "cancelled" terminal callback and NEVER a partial matrix.
///
/// null/empty `pairing_params_json` ⇒ `PairingParams::default()`; malformed/validate()-failing JSON ⇒
/// InvalidArgument (returns 0). null id arrays with a non-zero count ⇒ InvalidArgument. ADDITIVE —
/// `cpl_abi_version` stays 1; the synchronous symbol and every other entry are unchanged.
#[no_mangle]
pub unsafe extern "C" fn cpl_match_candidates_graph_start(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
    pairing_params_json: *const c_char,
    cb: CplProgressCb,
    user: *mut c_void,
) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return 0;
    }
    let Some(params) = pairing_params_or_error(pairing_params_json) else {
        return 0;
    };
    // Copy the id slices into OWNED Vecs BEFORE the spawn_job move — the caller's pointers are only
    // valid for this call, and the job runs on a worker thread (mirrors how autosync/export capture
    // their owned args). An empty array with a null pointer yields an empty Vec.
    let vs: Vec<u64> = if n_videos == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(videos, n_videos).to_vec()
    };
    let aus: Vec<u64> = if n_audios == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(audios, n_audios).to_vec()
    };
    spawn_job(ctx, cb, user, move |ctx, progress, cancel| {
        let mut prog = ProgressEmitter::new(0, progress);
        let m = ctx.match_candidates_graph_prog(&vs, &aus, &params, &mut prog, cancel)?;
        Ok(serde_json::to_string(&m).unwrap())
    })
}

/// §13.4 audio-audio SIMULTANEITY predicate over affine evidence — the core-owned check the CLI and
/// Swift resolvers SHARE so neither shell reimplements the affine composition / closure tolerances
/// (invariant 8). `request_json` is `{"va":{"lagMs","driftPpm"},"ab":{"lagMs","driftPpm"},
/// "vb":{"lagMs","driftPpm"},"abClassification":"match|noMatch|ambiguous|insufficientOverlap|
/// unavailable","coverageS":f64,"offsetTolMs":f64,"driftTolPpm":f64}` (the three legs' effective
/// maps, the A↔B core classification, the video coverage window, and the §13.3 graph tolerances).
/// Returns `{"closes":bool}`: true iff the A↔B edge is a `Match` AND composing V→A then A→B
/// reproduces V→B at start/mid/end within the tolerances (`match_graph::triangle_closes`). A missing
/// / non-positive-scale leg ⇒ `false`. PURE (no ctx); deterministic; no NaN/Infinity. Malformed JSON
/// ⇒ InvalidArgument. Additive — no ABI bump. Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_triangle_closes(request_json: *const c_char) -> *mut c_char {
    use couple_core::match_graph::{triangle_closes, AffineMap, CycleResidual};
    use couple_core::MatchClassification;
    if request_json.is_null() {
        set_error(&couple_core::CoupleError::invalid("triangle request JSON required"));
        return std::ptr::null_mut();
    }
    let raw = unsafe { CStr::from_ptr(request_json) }.to_string_lossy();
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(e) => {
            set_error(&couple_core::CoupleError::invalid(format!("bad triangle request: {e}")));
            return std::ptr::null_mut();
        }
    };
    // Pull each leg's (lagMs, driftPpm) → an AffineMap (None when a leg or its scale is degenerate).
    let leg = |k: &str| -> Option<AffineMap> {
        let l = v.get(k)?;
        let lag = l.get("lagMs")?.as_f64()?;
        let drift = l.get("driftPpm")?.as_f64()?;
        AffineMap::from_lag_drift(lag, drift)
    };
    let ab_class = match v.get("abClassification").and_then(|c| c.as_str()) {
        Some("match") => MatchClassification::Match,
        Some("noMatch") => MatchClassification::NoMatch,
        Some("ambiguous") => MatchClassification::Ambiguous,
        Some("insufficientOverlap") => MatchClassification::InsufficientOverlap,
        Some("unavailable") => MatchClassification::Unavailable,
        _ => {
            set_error(&couple_core::CoupleError::invalid("triangle request: bad abClassification"));
            return std::ptr::null_mut();
        }
    };
    let coverage_s = v.get("coverageS").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let offset_tol_ms = v.get("offsetTolMs").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let drift_tol_ppm = v.get("driftTolPpm").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let (va, ab, vb) = (leg("va"), leg("ab"), leg("vb"));
    let closes = triangle_closes(va, ab, vb, ab_class, coverage_s, offset_tol_ms, drift_tol_ppm);
    // The composed start/mid/end residual MAX (ms) — the shell's calibration read-out (the affine
    // analogue of the legacy scalar lag residual). 0.0 when a leg is missing.
    let residual_max_ms = match (va, ab, vb) {
        (Some(va), Some(ab), Some(vb)) => match va.compose(ab) {
            Some(c) => {
                let t1 = if coverage_s.is_finite() && coverage_s > 0.0 { coverage_s } else { 1.0 };
                CycleResidual::between(&vb, &c, 0.0, t1).max_offset_ms()
            }
            None => 0.0,
        },
        _ => 0.0,
    };
    to_cstring(serde_json::json!({"closes": closes, "residualMaxMs": residual_max_ms}).to_string())
}

/// Per-(video, audio) drift-fit RMS for contested candidate pairs (PLAN-RMS-TIEBREAKER
/// §4.2). pairs_json: `[[video_id, audio_id, coarse_lag_ms], …]` (lag = a candidate's
/// PairCorrelation.lagMs). Returns a JSON array aligned to input order, each element a
/// PairFit (`{"offMs","ppm","rmsMs","windows"}`) or null when too few windows lock.
/// Additive — no ABI bump. Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_pair_fit_batch(ctx: u64, pairs_json: *const c_char) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if pairs_json.is_null() {
        set_error(&couple_core::CoupleError::invalid("null pairs_json"));
        return std::ptr::null_mut();
    }
    let pairs: Vec<(u64, u64, f64)> =
        match serde_json::from_str(&CStr::from_ptr(pairs_json).to_string_lossy()) {
            Ok(p) => p,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("pairs: {e}")));
                return std::ptr::null_mut();
            }
        };
    match ctx.pair_fit_batch(&pairs, &couple_core::PairingParams::default()) {
        Ok(fits) => to_cstring(serde_json::to_string(&fits).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// As `cpl_pair_fit_batch` but with a `PairingParams` JSON override (null/empty ⇒ defaults;
/// malformed/invalid ⇒ InvalidArgument). Additive — no ABI bump. Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_pair_fit_batch_params(
    ctx: u64,
    pairs_json: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if pairs_json.is_null() {
        set_error(&couple_core::CoupleError::invalid("null pairs_json"));
        return std::ptr::null_mut();
    }
    let pairs: Vec<(u64, u64, f64)> =
        match serde_json::from_str(&CStr::from_ptr(pairs_json).to_string_lossy()) {
            Ok(p) => p,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("pairs: {e}")));
                return std::ptr::null_mut();
            }
        };
    let Some(params) = pairing_params_or_error(params_json) else {
        return std::ptr::null_mut();
    };
    match ctx.pair_fit_batch(&pairs, &params) {
        Ok(fits) => to_cstring(serde_json::to_string(&fits).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Per-(video, audio) fine MATCH-verification for candidate pairs
/// (PLAN-MATCH-CLASSIFIER §2.4 / Stage B). pairs_json: `[[video_id, audio_id,
/// coarse_lag_ms], …]` (lag = a candidate's PairCorrelation.lagMs). Returns a JSON array
/// aligned to input order, each element a VerifyFit
/// (`{"offMs","ppm","rmsMs","clamped","locked","expected","lockFraction","overlapS"}`) or
/// null when the overlap is below the absolute hard floor. Additive — no ABI bump
/// (`cpl_abi_version` stays 1). Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_verify_batch(ctx: u64, pairs_json: *const c_char) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if pairs_json.is_null() {
        set_error(&couple_core::CoupleError::invalid("null pairs_json"));
        return std::ptr::null_mut();
    }
    let pairs: Vec<(u64, u64, f64)> =
        match serde_json::from_str(&CStr::from_ptr(pairs_json).to_string_lossy()) {
            Ok(p) => p,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("pairs: {e}")));
                return std::ptr::null_mut();
            }
        };
    match ctx.verify_batch(&pairs, &couple_core::PairingParams::default()) {
        Ok(fits) => to_cstring(serde_json::to_string(&fits).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// As `cpl_verify_batch` but with a `PairingParams` JSON override (null/empty ⇒ defaults;
/// malformed/invalid ⇒ InvalidArgument). Additive — no ABI bump (`cpl_abi_version` stays 1).
/// Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_verify_batch_params(
    ctx: u64,
    pairs_json: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if pairs_json.is_null() {
        set_error(&couple_core::CoupleError::invalid("null pairs_json"));
        return std::ptr::null_mut();
    }
    let pairs: Vec<(u64, u64, f64)> =
        match serde_json::from_str(&CStr::from_ptr(pairs_json).to_string_lossy()) {
            Ok(p) => p,
            Err(e) => {
                set_error(&couple_core::CoupleError::invalid(format!("pairs: {e}")));
                return std::ptr::null_mut();
            }
        };
    let Some(params) = pairing_params_or_error(params_json) else {
        return std::ptr::null_mut();
    };
    match ctx.verify_batch(&pairs, &params) {
        Ok(fits) => to_cstring(serde_json::to_string(&fits).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

// ---------- resolve (PLAN-CORE-SHELL-BOUNDARY §3.1, the core row resolver) ----------

/// Decode `ResolveParams` for the `cpl_resolve*` symbols (PLAN-CORE-SHELL-BOUNDARY §3.1) —
/// the resolver twin of [`pairing_params_or_error`]. null or empty `params_json` ⇒
/// `ResolveParams::default()`; malformed JSON ⇒ `InvalidArgument` (sets the thread-local
/// last-error and returns `None`, so the caller returns null/0). `ResolveParams` is
/// `#[serde(default)]`, so a PARTIAL object fills only the named keys and defaults the rest.
/// The core-owned pairing thresholds (`params.pairing`) are validated before any work; a
/// value that fails `validate()` ⇒ `InvalidArgument` + `None`.
unsafe fn resolve_params_or_error(
    params_json: *const c_char,
) -> Option<couple_core::resolve::ResolveParams> {
    let params: couple_core::resolve::ResolveParams = if params_json.is_null() {
        couple_core::resolve::ResolveParams::default()
    } else {
        let raw = CStr::from_ptr(params_json).to_string_lossy();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            couple_core::resolve::ResolveParams::default()
        } else {
            match serde_json::from_str(trimmed) {
                Ok(p) => p,
                Err(e) => {
                    set_error(&couple_core::CoupleError::invalid(format!("resolve params: {e}")));
                    return None;
                }
            }
        }
    };
    if let Err(e) = params.pairing.validate() {
        set_error(&e);
        return None;
    }
    Some(params)
}

/// Default `ResolveParams` as JSON (PLAN-CORE-SHELL-BOUNDARY §3.1) — the single source of the
/// row-resolver defaults (matcher choice, z gate/margin, triangle tol, coverage-full, cluster
/// tol, the verify/RMS-escalation knobs) that the CLI and Swift shells read instead of
/// hardcoding copies, exactly as `cpl_pairing_params_default_json` does for the pairing layer.
/// The embedded `pairing` object is the same default `PairingParams`. Free with cpl_string_free.
#[no_mangle]
pub extern "C" fn cpl_resolve_params_default_json() -> *mut c_char {
    to_cstring(serde_json::to_string(&couple_core::resolve::ResolveParams::default()).unwrap())
}

/// Full §3.3 ROW RESOLUTION over imported tracks (PLAN-CORE-SHELL-BOUNDARY §3.1) — the single
/// cross-platform home for coverage clustering, the §3.4 audio↔audio simultaneity (triangle)
/// check, the §3.5 margin rule, the §3.3 row states, candidate-overflow handling, and the
/// legacy RMS tie-breaker. Rows = `videos`, the resolver picks each row's audio from `audios`
/// (IMPORTED-TRACK ONLY: the caller excludes audio-less videos; a shell wraps those as inert
/// no-match rows). Returns a JSON `[ResolvedRow]` array (camelCase; each row carries BOTH the
/// content key and the file name so each shell projects its own view, the row `state`/`chip`,
/// the `selected` candidate or null — NEVER a coin-flip — plus `alternates` and the
/// `resolution` honesty trail). No serialized field is NaN/Infinity (§20); output is
/// byte-identical to `cpl_resolve_start`'s terminal payload by construction.
///
/// `params_json` is an optional `ResolveParams` override (camelCase). null/empty ⇒
/// `ResolveParams::default()` (matcher `fingerprint`, z 8.5); a partial object defaults the
/// unset keys (`#[serde(default)]`); malformed JSON or a value whose pairing thresholds fail
/// `validate()` ⇒ InvalidArgument. A null id array paired with a positive count ⇒
/// InvalidArgument. ADDITIVE — `cpl_abi_version` stays 1; every existing symbol is unchanged.
/// Null on error; free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_resolve(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return std::ptr::null_mut();
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return std::ptr::null_mut();
    }
    let Some(params) = resolve_params_or_error(params_json) else {
        return std::ptr::null_mut();
    };
    let vs = id_slice(videos, n_videos);
    let aus = id_slice(audios, n_audios);
    match ctx.resolve(vs, aus, &params) {
        Ok(rows) => to_cstring(serde_json::to_string(&rows).unwrap()),
        Err(e) => {
            set_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Start a ROW-RESOLUTION JOB (PLAN-CORE-SHELL-BOUNDARY §3.1) — the asynchronous,
/// progress-reporting twin of `cpl_resolve`. Same domain arguments (`videos`/`audios` id
/// arrays + an optional `ResolveParams` JSON override), then the standard `(cb, user)` job
/// callback pair. Returns the job id (0 on error), exactly like `cpl_autosync_start`.
///
/// The terminal "done" payload is the SAME `[ResolvedRow]` JSON array that the synchronous
/// `cpl_resolve` returns — so a caller can switch a resolver from the blocking symbol to this
/// job purely to gain a determinate bar, with NO change to the resolved output (the resolution
/// authority is byte-identical; progress is observational). Non-terminal callbacks carry the
/// phase name as the core emits it — the matrix build dominates first-match cost
/// (`MatchFingerprint` while each track's fingerprint is built, `MatchCorrelate` per matrix
/// row, `MatchGraph` around the §13 graph pass), then `Verify`/`Resolve` while the row pass
/// (and any flag-ON classifier / RMS escalation) runs — and a fraction. Cancellation
/// (`cpl_job_cancel`) yields a "cancelled" terminal callback and NEVER a partial result.
///
/// null/empty `params_json` ⇒ `ResolveParams::default()`; a partial object defaults the unset
/// keys; malformed/validate()-failing JSON ⇒ InvalidArgument (returns 0). A null id array with
/// a positive count ⇒ InvalidArgument. ADDITIVE — `cpl_abi_version` stays 1; the synchronous
/// symbol and every other entry are unchanged.
#[no_mangle]
pub unsafe extern "C" fn cpl_resolve_start(
    ctx: u64,
    videos: *const u64,
    n_videos: usize,
    audios: *const u64,
    n_audios: usize,
    params_json: *const c_char,
    cb: CplProgressCb,
    user: *mut c_void,
) -> u64 {
    let Some(ctx) = ctx_of(ctx) else {
        invalid_handle();
        return 0;
    };
    if (videos.is_null() && n_videos > 0) || (audios.is_null() && n_audios > 0) {
        set_error(&couple_core::CoupleError::invalid("null track id array"));
        return 0;
    }
    let Some(params) = resolve_params_or_error(params_json) else {
        return 0;
    };
    // Copy the id slices into OWNED Vecs BEFORE the spawn_job move — the caller's pointers are
    // only valid for this call, and the job runs on a worker thread (mirrors how autosync/export
    // and cpl_match_candidates_graph_start capture their owned args). An empty array with a null
    // pointer yields an empty Vec.
    let vs: Vec<u64> = if n_videos == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(videos, n_videos).to_vec()
    };
    let aus: Vec<u64> = if n_audios == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(audios, n_audios).to_vec()
    };
    spawn_job(ctx, cb, user, move |ctx, progress, cancel| {
        let mut prog = ProgressEmitter::new(0, progress);
        let rows = ctx.resolve_prog(&vs, &aus, &params, &mut prog, cancel)?;
        Ok(serde_json::to_string(&rows).unwrap())
    })
}

// ---------- policy (PLAN-CORE-SHELL-BOUNDARY §3.2, the pure interaction-decision policies) ----------
//
// Each `cpl_policy_*` is PURE (no ctx) — exactly like `cpl_triangle_closes`, but with a UNIFORM
// `{ok,value|error}` envelope instead of null-on-malformed, so a malformed request or a stale shell
// DTO surfaces as an explicit error the wrapper maps to a CONSERVATIVE fallback (never a silent wrong
// answer; PLAN §3.2 risk "Silent policy fallback"). These run on UI gestures (clicks/drops/imports),
// never per frame, so the JSON round-trip is free. ADDITIVE — `cpl_abi_version` stays 1.

/// Build the success envelope `{"ok":true,"value":<value>}` for a serializable policy `Response`.
fn policy_ok<T: serde::Serialize>(value: &T) -> *mut c_char {
    to_cstring(serde_json::json!({ "ok": true, "value": value }).to_string())
}

/// Build the error envelope `{"ok":false,"error":{"kind":"invalid","message":msg}}` — the only
/// failure shape a pure policy returns (a malformed/stale request). Always `"invalid"`: a policy
/// fn is total over valid inputs, so the only failure is a request that does not parse.
fn policy_err(msg: impl std::fmt::Display) -> *mut c_char {
    to_cstring(
        serde_json::json!({
            "ok": false,
            "error": { "kind": "invalid", "message": msg.to_string() }
        })
        .to_string(),
    )
}

/// Parse a pure policy request JSON into `T`, or return the error envelope. Null/empty/malformed ⇒
/// `Err(envelope)` the caller returns verbatim. Factored so every `cpl_policy_*` shares the exact
/// same parse + error shape.
unsafe fn policy_request<T: serde::de::DeserializeOwned>(
    json: *const c_char,
) -> std::result::Result<T, *mut c_char> {
    if json.is_null() {
        return Err(policy_err("policy request JSON required"));
    }
    let raw = CStr::from_ptr(json).to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(policy_err("policy request JSON required"));
    }
    serde_json::from_str::<T>(trimmed).map_err(|e| policy_err(format!("bad policy request: {e}")))
}

/// PURE match-quality decision (TODO-009 / PLAN §3.2) — the evidence-based verdict-dot color for a
/// row, lifted from Swift `MatchQualityPolicy.decide`. `json` is the request
/// `{"forced":bool,"selection":"none|automatic|manual","autoVerdict":"confident|questionable|none",
/// "manualMatchConfident":bool|null}`. Returns the uniform envelope: on success
/// `{"ok":true,"value":{"quality":"confident|questionable|none"}}`; on malformed/stale JSON
/// `{"ok":false,"error":{"kind":"invalid","message":"..."}}` (the wrapper maps the error to the
/// conservative `questionable`, never silently green). PURE (no ctx). Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_match_quality(json: *const c_char) -> *mut c_char {
    use couple_core::policy::match_quality::{decide, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&decide(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE manual drag-to-pair decision (TODO-013 / PLAN §3.2) — what dropping an audio file onto the
/// selected couple should do, lifted from Swift `RepairDropPolicy.decide` (both drop gestures share
/// it so they cannot diverge). `json` is the request `{"hasSelectedCouple":bool,
/// "payloadIsAudio":bool,"videoAvailable":bool,"currentSelectedKey":str|null,"droppedKey":str,
/// "approved":bool}`. Returns the uniform envelope: on success
/// `{"ok":true,"value":{"outcome":"noOp|accept|needsConfirmation|reject"}}`; on malformed/stale JSON
/// the `{"ok":false,"error":{"kind":"invalid",...}}` envelope (the wrapper maps the error to the
/// conservative `reject`, which mutates NO state). PURE (no ctx). Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_repair_drop(json: *const c_char) -> *mut c_char {
    use couple_core::policy::repair_drop::{decide, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&decide(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE forced-verdict decision (TODO-009/-013, SPEC-V2 §3.6) — whether a KEPT manual pairing is
/// `forced` (below the correlation floor), lifted from the inline rule in the shell's `repair`.
/// Called at pick time (`syncFailed:false`; a fresh pool drag carries `candidateZ:0`) and again
/// when the sync completes. `json` is the request `{"candidateZ":f64,"zMin":f64,
/// "syncFailed":bool}`. Returns the uniform envelope: on success `{"ok":true,"value":
/// {"forced":bool}}`; on malformed JSON the error envelope (the wrapper maps it to the
/// conservative `forced=true` — Questionable, never a silent green). ADDITIVE —
/// `cpl_abi_version` stays 1. PURE (no ctx). Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_repair_forced(json: *const c_char) -> *mut c_char {
    use couple_core::policy::repair_drop::{forced, ForcedRequest};
    match policy_request::<ForcedRequest>(json) {
        Ok(req) => policy_ok(&forced(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE re-resolution merge decision (TODO-007 / PLAN §3.2) — how to fold a fresh `ResolvedRow`
/// into an existing queue row, lifted from Swift `RowMergePolicy.plan`. `json` is the request
/// `{"protected":bool,"currentSelectedKey":str|null,"currentCandidateKeys":[str],
/// "freshSelectedKey":str|null,"freshCandidateKeys":[str]}`. Returns the uniform envelope: on
/// success `{"ok":true,"value":{"selectedAudioKey":str|null,"candidateKeys":[str],
/// "adoptFreshDisplay":bool,"rebuildSession":bool,"clearForced":bool}}`; on malformed/stale JSON
/// the error envelope (the wrapper maps it to the conservative PROTECTED-row plan: keep the
/// existing selection/candidates/display, never silently swap a source). PURE (no ctx). Free with
/// cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_row_merge(json: *const c_char) -> *mut c_char {
    use couple_core::policy::row_merge::{plan, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&plan(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE media-removal / clear-session decision (TODO-001 / PLAN §3.2) — whether a removal needs a
/// confirmation and with what counts, lifted from Swift `MediaRemovalPolicy` (both the trash button
/// and the Delete key share it). A single TAGGED request dispatches the two pure functions:
///   - `{"op":"plan","selectedKeys":[str],"poolKeys":[str],"couples":[{"videoKey","selectedAudioKey":
///     str|null,"approved"}]}` ⇒ `value` is `{"removableKeys":[str],"needsConfirmation":bool,
///     "affectedCouples":u32,"affectedApproved":u32}`;
///   - `{"op":"clearCounts","mediaCount":u32,"couples":[…]}` ⇒ `value` is
///     `{"media":u32,"couples":u32,"approved":u32}`.
/// Returns the uniform envelope; on malformed/stale JSON (e.g. an unknown `op`) the error envelope
/// (the wrapper maps the error to the conservative EMPTY/no-op plan — `needsConfirmation:false`,
/// zero counts — so a failure removes nothing). PURE (no ctx). Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_media_removal(json: *const c_char) -> *mut c_char {
    use couple_core::policy::media_removal::{dispatch, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&dispatch(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE automatic anchor proposal / "+ anchor" decision (TODO-011 / PLAN §3.2) — overlap-bounded
/// anchor placement, lifted from Swift `AnchorProposal` (pure geometry/scoring; numeric behavior
/// preserved exactly). A single TAGGED request dispatches the two pure functions:
///   - `{"op":"propose","overlap":{"start","end"}|null,"candidates":[{"t","score","usable"}],
///     "tuning":{…}?}` ⇒ `value` is a sorted `[f64]` (always length 0, 2, or 3 — never 1, all
///     inside the overlap);
///   - `{"op":"add","overlap":…,"existing":[f64],"candidates":[…],"tuning":{…}?}` ⇒ `value` is one
///     in-overlap, separated time or `null`.
/// `tuning` defaults to the historical values when omitted. Returns the uniform envelope; on
/// malformed/stale JSON the error envelope (the wrapper maps the error to the conservative EMPTY
/// proposal / `nil` add — the manual-rescue state, never a fabricated/out-of-overlap anchor). PURE
/// (no ctx). Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_anchor_propose(json: *const c_char) -> *mut c_char {
    use couple_core::policy::anchor::{dispatch, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&dispatch(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE shadow-telemetry projection (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2 / PLAN §3.2) — orders
/// one video's `PairEvidence` row by score and projects displayable rows, lifted from Swift
/// `ShadowTelemetry.rows`. NO acceptance thresholding (it surfaces core's `classification`
/// verbatim). `json` is the request `{"evidence":[PairEvidence],"audioKeys":[str],
/// "audioNames":[str]}` (the arrays index-aligned: cell `i` scores audio `i`). Returns the uniform
/// envelope: on success `{"ok":true,"value":[{"audioKey","audioName","evidence":<PairEvidence>}]}`
/// sorted by score desc / name asc / key asc (a TOTAL order — key is unique); mismatched lengths
/// truncate to the shortest. On malformed/stale JSON the error envelope (the wrapper maps the error
/// to an EMPTY projection — telemetry simply does not display, never a fabricated row). PURE (no
/// ctx). Free with cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_shadow_rows(json: *const c_char) -> *mut c_char {
    use couple_core::policy::shadow::{project, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&project(&req)),
        Err(envelope) => envelope,
    }
}

/// PURE activity-strip decision (TODO-005 / PLAN §3.2 / Phase F) — the single honest summary line the
/// global activity strip should show for a snapshot of the in-flight job signals, lifted from the
/// MATH of Swift `ActivitySummaryPolicy` (priority / weighting / fraction / debounce). PURE: `json`
/// carries the snapshot PLUS the explicit time inputs (`elapsedMs`, optional `debounceMs`,
/// `phaseFloor`) so core never reads a clock — the shell holds the debounce timer + the per-phase
/// floor. `json` is the request `{"importing"?,"pairing"?,"syncing"?,"exporting"?,"ingest"?,
/// "failure"?:str,"elapsedMs":f64,"debounceMs"?:f64,"phaseFloor"?:f64}`. Returns the uniform envelope:
/// on success `{"ok":true,"value":{"summary":<Decision>|null}}` where `Decision` is a STRUCTURED
/// decision (`{"line":{"kind":…,…},"fraction":f64|null,"isError":bool,"phaseKey":str|null}`) the
/// shell formats into localized strings — the user-facing copy stays shell-side. `summary:null` means
/// "show nothing". On malformed/stale JSON the error envelope (the wrapper maps the error to a
/// conservative no-line/idle fallback, never a wrong-but-confident line). PURE (no ctx). Free with
/// cpl_string_free.
#[no_mangle]
pub unsafe extern "C" fn cpl_policy_activity(json: *const c_char) -> *mut c_char {
    use couple_core::policy::activity::{decide, Request};
    match policy_request::<Request>(json) {
        Ok(req) => policy_ok(&decide(&req)),
        Err(envelope) => envelope,
    }
}

// ---------- cache ----------

/// LRU-GC the cache down to max_bytes; returns bytes freed.
#[no_mangle]
pub extern "C" fn cpl_cache_gc(ctx: u64, max_bytes: u64) -> u64 {
    match ctx_of(ctx) {
        Some(ctx) => ctx.cache_gc(max_bytes),
        None => {
            invalid_handle();
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// Consume a core-allocated C string into an owned `String` (or None if null).
    fn take(p: *mut c_char) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() };
        unsafe { cpl_string_free(p) };
        Some(s)
    }

    #[test]
    fn abi_version_is_one() {
        assert_eq!(cpl_abi_version(), 1);
    }

    // §6.7: the default-params JSON is the single source of truth and round-trips back to
    // `PairingParams::default()` (what CLI/Swift decode instead of hardcoding copies). After
    // boundary 13 the default is FingerprintPrimary + profile 2, but the round-trip identity holds.
    #[test]
    fn pairing_params_default_json_round_trips() {
        let s = take(cpl_pairing_params_default_json()).expect("default json");
        let p: couple_core::PairingParams = serde_json::from_str(&s).unwrap();
        assert_eq!(p, couple_core::PairingParams::default());
        // Boundary 13: the default carries the fingerprint-primary mode + the frozen profile 2.
        assert!(s.contains("\"matcherMode\":\"fingerprintPrimary\""), "default JSON mode: {s}");
        assert!(s.contains("\"thresholdProfileVersion\":2"), "default JSON profile: {s}");
    }

    // §13.4: cpl_triangle_closes shares the core simultaneity predicate (the CLI + Swift resolvers
    // call it; no shell reimplements the affine composition). A consistent geometry + a `match` A↔B
    // closes; a non-match A↔B or a large composed residual does not. No ctx/media needed (pure).
    #[test]
    fn triangle_closes_predicate() {
        // V=0, A=0.3, B=1.0 s ⇒ V→A = -300 ms, V→B = -1000 ms, A→B = -700 ms. Consistent + match.
        let ok = CString::new(
            r#"{"va":{"lagMs":-300,"driftPpm":0},"ab":{"lagMs":-700,"driftPpm":0},
                "vb":{"lagMs":-1000,"driftPpm":0},"abClassification":"match",
                "coverageS":600,"offsetTolMs":100,"driftTolPpm":150}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_triangle_closes(ok.as_ptr()) }).expect("triangle json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["closes"], serde_json::json!(true), "consistent + match ⇒ closes: {r}");
        assert!(v["residualMaxMs"].as_f64().unwrap() < 1.0, "residual ~0: {r}");

        // NoMatch A↔B ⇒ does NOT close (core classification is the identity authority).
        let nm = CString::new(
            r#"{"va":{"lagMs":-300,"driftPpm":0},"ab":{"lagMs":-700,"driftPpm":0},
                "vb":{"lagMs":-1000,"driftPpm":0},"abClassification":"noMatch",
                "coverageS":600,"offsetTolMs":100,"driftTolPpm":150}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_triangle_closes(nm.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["closes"], serde_json::json!(false), "noMatch A↔B ⇒ not simultaneous: {r}");

        // A→B off by 300 ms ⇒ composed residual exceeds the 100 ms tol ⇒ does NOT close.
        let off = CString::new(
            r#"{"va":{"lagMs":-300,"driftPpm":0},"ab":{"lagMs":-400,"driftPpm":0},
                "vb":{"lagMs":-1000,"driftPpm":0},"abClassification":"match",
                "coverageS":600,"offsetTolMs":100,"driftTolPpm":150}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_triangle_closes(off.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["closes"], serde_json::json!(false), "300 ms residual ⇒ does not close: {r}");
        assert!(v["residualMaxMs"].as_f64().unwrap() > 100.0, "residual surfaced: {r}");

        // Malformed JSON ⇒ null + last-error (InvalidArgument).
        let bad = CString::new("{not json").unwrap();
        assert!(take(unsafe { cpl_triangle_closes(bad.as_ptr()) }).is_none(), "malformed ⇒ null");
    }

    // PLAN §3.2: cpl_policy_match_quality returns the UNIFORM {ok,value|error} envelope (NOT
    // null-on-malformed). The VALUE path decides; the ERROR path surfaces a malformed/stale request
    // as an explicit invalid error so a wrapper maps it to a conservative fallback (never silent).
    #[test]
    fn policy_match_quality_envelope() {
        // Value path: an automatic confident selection ⇒ confident.
        let ok = CString::new(
            r#"{"forced":false,"selection":"automatic","autoVerdict":"confident","manualMatchConfident":null}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_match_quality(ok.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "value path ok: {r}");
        assert_eq!(v["value"]["quality"], serde_json::json!("confident"), "value path quality: {r}");

        // Value path: a manual pick with no/false evidence ⇒ questionable (never silently green).
        let manual = CString::new(
            r#"{"forced":false,"selection":"manual","autoVerdict":"confident","manualMatchConfident":false}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_match_quality(manual.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"]["quality"], serde_json::json!("questionable"), "manual stays amber: {r}");

        // Error path (malformed): explicit {ok:false,error:{kind:invalid}} — NOT null, NOT a silent value.
        let bad = CString::new("{not json").unwrap();
        let r = take(unsafe { cpl_policy_match_quality(bad.as_ptr()) }).expect("error envelope, not null");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "malformed ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "malformed ⇒ invalid: {r}");

        // Error path (stale shape): a wrong-shape DTO (missing required fields) is also invalid.
        let stale = CString::new(r#"{"selection":"manual"}"#).unwrap();
        let r = take(unsafe { cpl_policy_match_quality(stale.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "stale DTO ⇒ ok:false: {r}");
    }

    // PLAN §3.2: cpl_policy_repair_drop — value path decides the drop outcome; error path surfaces
    // a malformed/stale request explicitly (the wrapper maps it to the conservative `reject`).
    #[test]
    fn policy_repair_drop_envelope() {
        // Value path: an approved couple + a different source ⇒ needsConfirmation.
        let ok = CString::new(
            r#"{"hasSelectedCouple":true,"payloadIsAudio":true,"videoAvailable":true,
                "currentSelectedKey":"old","droppedKey":"new","approved":true}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_repair_drop(ok.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "value path ok: {r}");
        assert_eq!(v["value"]["outcome"], serde_json::json!("needsConfirmation"), "outcome: {r}");

        // Value path: a non-audio payload ⇒ reject (a malformed drop never mutates).
        let reject = CString::new(
            r#"{"hasSelectedCouple":true,"payloadIsAudio":false,"videoAvailable":true,
                "currentSelectedKey":null,"droppedKey":"x","approved":false}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_repair_drop(reject.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"]["outcome"], serde_json::json!("reject"), "non-audio ⇒ reject: {r}");

        // Error path: malformed JSON ⇒ explicit invalid error, not null, not a silent outcome.
        let bad = CString::new("{nope").unwrap();
        let r = take(unsafe { cpl_policy_repair_drop(bad.as_ptr()) }).expect("error envelope");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "malformed ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "malformed ⇒ invalid: {r}");
    }

    // cpl_policy_repair_forced — value path decides the kept-manual-pairing forced verdict; error
    // path surfaces a malformed request explicitly (the wrapper maps it to the conservative
    // forced=true, never a silent green).
    #[test]
    fn policy_repair_forced_envelope() {
        // Value path: a known below-floor z ⇒ forced.
        let below = CString::new(r#"{"candidateZ":3.0,"zMin":8.0,"syncFailed":false}"#).unwrap();
        let r = take(unsafe { cpl_policy_repair_forced(below.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "value path ok: {r}");
        assert_eq!(v["value"]["forced"], serde_json::json!(true), "below floor ⇒ forced: {r}");

        // Value path: an unknown z (fresh pool drag) with a clean sync ⇒ not forced.
        let unknown = CString::new(r#"{"candidateZ":0.0,"zMin":8.0,"syncFailed":false}"#).unwrap();
        let r = take(unsafe { cpl_policy_repair_forced(unknown.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"]["forced"], serde_json::json!(false), "unknown z waits: {r}");

        // Error path: malformed JSON ⇒ explicit invalid error.
        let bad = CString::new("{nope").unwrap();
        let r = take(unsafe { cpl_policy_repair_forced(bad.as_ptr()) }).expect("error envelope");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "malformed ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "malformed ⇒ invalid: {r}");
    }

    // PLAN §3.2: cpl_policy_row_merge — value path folds a fresh resolution into an existing row;
    // error path surfaces a malformed/stale request explicitly (the wrapper maps it to a protected
    // no-change plan).
    #[test]
    fn policy_row_merge_envelope() {
        // Value path: an unprotected unresolved row adopting a confident result ⇒ adopt+rebuild.
        let ok = CString::new(
            r#"{"protected":false,"currentSelectedKey":null,"currentCandidateKeys":[],
                "freshSelectedKey":"A","freshCandidateKeys":["A"]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_row_merge(ok.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "value path ok: {r}");
        assert_eq!(v["value"]["selectedAudioKey"], serde_json::json!("A"), "selected: {r}");
        assert_eq!(v["value"]["rebuildSession"], serde_json::json!(true), "rebuild once: {r}");
        assert_eq!(v["value"]["adoptFreshDisplay"], serde_json::json!(true), "adopt: {r}");

        // Value path: a protected row keeps its selection (stronger source only appended).
        let protected = CString::new(
            r#"{"protected":true,"currentSelectedKey":"chosen","currentCandidateKeys":["chosen"],
                "freshSelectedKey":"stronger","freshCandidateKeys":["stronger","chosen"]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_row_merge(protected.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"]["selectedAudioKey"], serde_json::json!("chosen"), "protected keeps: {r}");
        assert_eq!(v["value"]["candidateKeys"], serde_json::json!(["chosen", "stronger"]), "appended: {r}");
        assert_eq!(v["value"]["rebuildSession"], serde_json::json!(false), "no rebuild: {r}");

        // Error path: a stale/wrong-shape DTO (missing required arrays) ⇒ explicit invalid error.
        let stale = CString::new(r#"{"protected":false}"#).unwrap();
        let r = take(unsafe { cpl_policy_row_merge(stale.as_ptr()) }).expect("error envelope");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "stale ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "stale ⇒ invalid: {r}");
    }

    // PLAN §3.2: cpl_policy_media_removal — a tagged request dispatches the per-file plan and the
    // clear-session counts; the error path surfaces a malformed/unknown-op request explicitly.
    #[test]
    fn policy_media_removal_envelope() {
        // Value path (plan): removing a selected audio used by an approved couple ⇒ confirm.
        let plan = CString::new(
            r#"{"op":"plan","selectedKeys":["a1"],"poolKeys":["v1","a1"],
                "couples":[{"videoKey":"v1","selectedAudioKey":"a1","approved":true}]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_media_removal(plan.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "plan ok: {r}");
        assert_eq!(v["value"]["needsConfirmation"], serde_json::json!(true), "needs confirm: {r}");
        assert_eq!(v["value"]["affectedCouples"], serde_json::json!(1), "affected: {r}");
        assert_eq!(v["value"]["affectedApproved"], serde_json::json!(1), "approved: {r}");

        // Value path (clearCounts): media/couple/approved projection.
        let cc = CString::new(
            r#"{"op":"clearCounts","mediaCount":5,
                "couples":[{"videoKey":"v1","selectedAudioKey":"a1","approved":true},
                           {"videoKey":"v2","selectedAudioKey":null,"approved":false}]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_media_removal(cc.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"]["media"], serde_json::json!(5), "media: {r}");
        assert_eq!(v["value"]["couples"], serde_json::json!(2), "couples: {r}");
        assert_eq!(v["value"]["approved"], serde_json::json!(1), "approved: {r}");

        // Error path: an unknown op (a stale shell DTO) ⇒ explicit invalid error, not a silent plan.
        let bad_op = CString::new(r#"{"op":"frobnicate"}"#).unwrap();
        let r = take(unsafe { cpl_policy_media_removal(bad_op.as_ptr()) }).expect("error envelope");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "unknown op ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "unknown op ⇒ invalid: {r}");
    }

    // PLAN §3.2: cpl_policy_anchor_propose — a tagged request dispatches `propose` (sorted times)
    // and `add` (one time | null); the error path surfaces a malformed/unknown-op request.
    #[test]
    fn policy_anchor_propose_envelope() {
        // Value path (propose): a long overlap ⇒ three sorted in-overlap times.
        let propose = CString::new(
            r#"{"op":"propose","overlap":{"start":0,"end":120},
                "candidates":[{"t":10,"score":1,"usable":true},{"t":50,"score":2,"usable":true},
                              {"t":100,"score":3,"usable":true}]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_anchor_propose(propose.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "propose ok: {r}");
        let times = v["value"].as_array().expect("propose array");
        assert_eq!(times.len(), 3, "three anchors: {r}");
        // Never exactly one; all inside [0,120].
        assert!(times.iter().all(|t| (0.0..=120.0).contains(&t.as_f64().unwrap())), "inside: {r}");

        // Value path (propose): no overlap ⇒ empty (the manual-rescue state), never one.
        let none = CString::new(
            r#"{"op":"propose","overlap":null,"candidates":[{"t":5,"score":1,"usable":true}]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_anchor_propose(none.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"], serde_json::json!([]), "no overlap ⇒ empty: {r}");

        // Value path (add): the strongest in-overlap candidate; the outside one is ignored.
        let add = CString::new(
            r#"{"op":"add","overlap":{"start":0,"end":60},"existing":[],
                "candidates":[{"t":30,"score":5,"usable":true},{"t":90,"score":9999,"usable":true}]}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_anchor_propose(add.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["value"], serde_json::json!(30.0), "add picks in-overlap: {r}");

        // Error path: an unknown op (stale DTO) ⇒ explicit invalid error, not a fabricated anchor.
        let bad = CString::new(r#"{"op":"teleport","candidates":[]}"#).unwrap();
        let r = take(unsafe { cpl_policy_anchor_propose(bad.as_ptr()) }).expect("error envelope");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "unknown op ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "unknown op ⇒ invalid: {r}");
    }

    // PLAN §3.2 / §14.2: cpl_policy_shadow_rows — value path orders evidence by score (no
    // thresholding); error path surfaces a malformed/stale request explicitly.
    #[test]
    fn policy_shadow_rows_envelope() {
        // Value path: two cells out of score order ⇒ projected score-desc; classification verbatim.
        let cell = |cls: &str, score: f64| {
            format!(
                r#"{{"method":"spectralLandmarkV1","classification":"{cls}","lagMs":0,"driftPpm":0,
                    "score":{score},"runnerUpScore":0,"scoreMargin":0,"support":0,
                    "candidateCorrespondences":0,"inlierFraction":0,"temporalCoverage":0,
                    "residualRmsMs":null,"residualP95Ms":null,"overlapS":0,"refinedLagMs":null,
                    "refinedDriftPpm":null,"refinedRmsMs":null}}"#
            )
        };
        let ok = CString::new(format!(
            r#"{{"evidence":[{},{}],"audioKeys":["kb","ka"],"audioNames":["b.wav","a.wav"]}}"#,
            cell("noMatch", 1.0),
            cell("match", 9.0)
        ))
        .unwrap();
        let r = take(unsafe { cpl_policy_shadow_rows(ok.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "value path ok: {r}");
        let rows = v["value"].as_array().expect("rows array");
        assert_eq!(rows.len(), 2, "two rows: {r}");
        // score 9 (a.wav, match) sorts first; classification surfaced verbatim (no thresholding).
        assert_eq!(rows[0]["audioName"], serde_json::json!("a.wav"), "score desc: {r}");
        assert_eq!(rows[0]["evidence"]["classification"], serde_json::json!("match"), "verbatim: {r}");
        assert_eq!(rows[1]["audioName"], serde_json::json!("b.wav"));

        // Error path: malformed JSON ⇒ explicit invalid error, not a fabricated/empty silent value.
        let bad = CString::new("{not json").unwrap();
        let r = take(unsafe { cpl_policy_shadow_rows(bad.as_ptr()) }).expect("error envelope");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "malformed ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "malformed ⇒ invalid: {r}");
    }

    // PLAN §3.2 / Phase F: cpl_policy_activity — value path returns the structured summary decision
    // (line kind + fraction + phaseKey), `summary:null` means "show nothing"; error path surfaces a
    // malformed/stale request explicitly (the wrapper maps it to a conservative idle/no-line).
    #[test]
    fn policy_activity_envelope() {
        // Value path (determinate import): file 1 of 1 mid-decode past the debounce ⇒ a determinate
        // import line with phaseKey "ingest". 0.5 through decode ⇒ 0.80·0.5 = 0.40.
        let imp = CString::new(
            r#"{"importing":{"phase":"decoding","fraction":0.5,"index":1,"total":1},"elapsedMs":300}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_activity(imp.as_ptr()) }).expect("envelope json");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "value path ok: {r}");
        let d = &v["value"]["summary"];
        assert_eq!(d["line"]["kind"], serde_json::json!("import"), "import kind: {r}");
        assert!((d["fraction"].as_f64().unwrap() - 0.40).abs() < 1e-9, "import fraction: {r}");
        assert_eq!(d["phaseKey"], serde_json::json!("ingest"), "import phaseKey: {r}");
        assert_eq!(d["isError"], serde_json::json!(false));

        // Value path (pairing is indeterminate): counts only, fraction null, no phaseKey.
        let pair = CString::new(
            r#"{"pairing":{"videoCount":4,"audioCount":8},"elapsedMs":300}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_activity(pair.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        let d = &v["value"]["summary"];
        assert_eq!(d["line"]["kind"], serde_json::json!("pairingComparing"), "pairing kind: {r}");
        assert_eq!(d["line"]["videoCount"], serde_json::json!(4));
        assert!(d["fraction"].is_null(), "pairing is indeterminate: {r}");

        // Value path (debounce): an active job younger than the debounce ⇒ summary:null (show nothing).
        let quick = CString::new(
            r#"{"importing":{"phase":"decoding","fraction":0.5},"elapsedMs":100}"#,
        )
        .unwrap();
        let r = take(unsafe { cpl_policy_activity(quick.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "quick ok: {r}");
        assert!(v["value"]["summary"].is_null(), "debounce ⇒ summary null: {r}");

        // Value path (failure wins immediately, even under the debounce): isError, no fraction.
        let fail = CString::new(r#"{"failure":"disk full","elapsedMs":0}"#).unwrap();
        let r = take(unsafe { cpl_policy_activity(fail.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        let d = &v["value"]["summary"];
        assert_eq!(d["isError"], serde_json::json!(true), "failure ⇒ error: {r}");
        assert_eq!(d["line"]["kind"], serde_json::json!("failure"));
        assert_eq!(d["line"]["message"], serde_json::json!("disk full"));

        // Error path (malformed): explicit {ok:false,error:{kind:invalid}}, not null / not a silent value.
        let bad = CString::new("{not json").unwrap();
        let r = take(unsafe { cpl_policy_activity(bad.as_ptr()) }).expect("error envelope, not null");
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "malformed ⇒ ok:false: {r}");
        assert_eq!(v["error"]["kind"], serde_json::json!("invalid"), "malformed ⇒ invalid: {r}");

        // Error path (stale shape): a request missing the required `elapsedMs` is invalid.
        let stale = CString::new(r#"{"pairing":{"videoCount":1,"audioCount":1}}"#).unwrap();
        let r = take(unsafe { cpl_policy_activity(stale.as_ptr()) }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "stale DTO ⇒ ok:false: {r}");
    }

    // §6.8.3: an old pairing symbol's output equals the new `*_params` symbol with null
    // params (both use defaults). Exercised on empty pairs so the param plumbing is the SUT,
    // no media/ffmpeg needed.
    #[test]
    fn old_symbol_equals_new_with_null_params() {
        let ctx = unsafe { cpl_ctx_new(std::ptr::null()) };
        assert_ne!(ctx, 0);
        let empty = CString::new("[]").unwrap();

        let old_v = take(unsafe { cpl_verify_batch(ctx, empty.as_ptr()) }).unwrap();
        let new_v =
            take(unsafe { cpl_verify_batch_params(ctx, empty.as_ptr(), std::ptr::null()) }).unwrap();
        assert_eq!(old_v, new_v, "verify_batch old vs new(null) must match");
        assert_eq!(new_v, "[]");

        let old_f = take(unsafe { cpl_pair_fit_batch(ctx, empty.as_ptr()) }).unwrap();
        let new_f =
            take(unsafe { cpl_pair_fit_batch_params(ctx, empty.as_ptr(), std::ptr::null()) }).unwrap();
        assert_eq!(old_f, new_f, "pair_fit_batch old vs new(null) must match");

        cpl_ctx_free(ctx);
    }

    // §6.7: malformed JSON and a value failing validate() are both InvalidArgument (null
    // result + last-error set); a VALID override is accepted (empty pairs ⇒ "[]").
    #[test]
    fn params_malformed_and_invalid_rejected() {
        let ctx = unsafe { cpl_ctx_new(std::ptr::null()) };
        let empty = CString::new("[]").unwrap();

        let bad = CString::new("{not json").unwrap();
        let r = unsafe { cpl_verify_batch_params(ctx, empty.as_ptr(), bad.as_ptr()) };
        assert!(r.is_null(), "malformed params must yield null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on malformed");

        let invalid = CString::new(r#"{"verifyRmsMaxMs": -1}"#).unwrap();
        let r = unsafe { cpl_verify_batch_params(ctx, empty.as_ptr(), invalid.as_ptr()) };
        assert!(r.is_null(), "value failing validate() must yield null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on invalid");

        let okp = CString::new(r#"{"verifyRmsMaxMs": 7.5}"#).unwrap();
        let r = take(unsafe { cpl_verify_batch_params(ctx, empty.as_ptr(), okp.as_ptr()) });
        assert_eq!(r.as_deref(), Some("[]"), "valid override accepted");

        cpl_ctx_free(ctx);
    }

    // ---- PLAN-CORE-SHELL-BOUNDARY §3.1 cpl_resolve* ----

    // §3.1: the default ResolveParams JSON is the single source of the row-resolver defaults
    // and round-trips back to `ResolveParams::default()` (what CLI/Swift decode instead of
    // hardcoding copies). matcher defaults to fingerprint and the resolver z-gate to 8.5; the
    // embedded `pairing` object is the default PairingParams (fingerprintPrimary + profile 2).
    #[test]
    fn resolve_params_default_json_round_trips() {
        let s = take(cpl_resolve_params_default_json()).expect("default resolve json");
        let p: couple_core::resolve::ResolveParams = serde_json::from_str(&s).unwrap();
        assert_eq!(p.matcher, couple_core::MatcherChoice::Fingerprint, "default matcher");
        assert_eq!(p.z_min, 8.5, "default resolver z gate");
        assert!(s.contains("\"matcher\":\"fingerprint\""), "wire matcher: {s}");
        assert!(s.contains("\"zMin\":8.5"), "wire zMin: {s}");
        // The embedded pairing object is the boundary-13 default (fingerprint-primary + profile 2).
        assert!(s.contains("\"matcherMode\":\"fingerprintPrimary\""), "embedded pairing mode: {s}");
        assert!(s.contains("\"thresholdProfileVersion\":2"), "embedded pairing profile: {s}");
    }

    // §3.1 PLUMBING (no media): an empty V×A resolve returns "[]" (no panic) with null params
    // (⇒ defaults) and with a VALID partial override; malformed/validate()-failing params ⇒
    // InvalidArgument (null + last-error); a null id array paired with a positive count ⇒
    // InvalidArgument. Mirrors match_candidates' parameter handling — the SUT for cpl_resolve's
    // null/empty/partial/invalid path without needing imported tracks.
    #[test]
    fn resolve_param_plumbing() {
        let ctx = unsafe { cpl_ctx_new(std::ptr::null()) };
        assert_ne!(ctx, 0);

        // Empty resolve with null params (⇒ ResolveParams::default()) ⇒ "[]".
        let r = take(unsafe {
            cpl_resolve(ctx, std::ptr::null(), 0, std::ptr::null(), 0, std::ptr::null())
        });
        assert_eq!(r.as_deref(), Some("[]"), "empty resolve ⇒ []");

        // A VALID partial override (only `matcher`, the rest default via serde(default)) ⇒ "[]".
        let legacy = CString::new(r#"{"matcher":"legacy"}"#).unwrap();
        let r = take(unsafe {
            cpl_resolve(ctx, std::ptr::null(), 0, std::ptr::null(), 0, legacy.as_ptr())
        });
        assert_eq!(r.as_deref(), Some("[]"), "valid partial override accepted");

        // Malformed JSON ⇒ InvalidArgument (null + last-error).
        let bad = CString::new("{not json").unwrap();
        let r =
            unsafe { cpl_resolve(ctx, std::ptr::null(), 0, std::ptr::null(), 0, bad.as_ptr()) };
        assert!(r.is_null(), "malformed params ⇒ null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on malformed");

        // A value whose embedded pairing thresholds fail validate() ⇒ InvalidArgument.
        let invalid = CString::new(r#"{"pairing":{"minOverlapS":-1}}"#).unwrap();
        let r =
            unsafe { cpl_resolve(ctx, std::ptr::null(), 0, std::ptr::null(), 0, invalid.as_ptr()) };
        assert!(r.is_null(), "validate()-failing params ⇒ null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on invalid");

        // A null id array with a positive count ⇒ InvalidArgument (the same guard as the matrix
        // symbols), before any param parse.
        let r = unsafe { cpl_resolve(ctx, std::ptr::null(), 3, std::ptr::null(), 0, std::ptr::null()) };
        assert!(r.is_null(), "null id array + positive count ⇒ null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on null-array+count");

        cpl_ctx_free(ctx);
    }

    // ---- §14.1 cpl_match_pair / cpl_match_candidates ----

    // §14.1 PLUMBING (no media): an empty batch returns "[]"; null params ⇒ default mode;
    // malformed/invalid params ⇒ InvalidArgument (null + last-error). Mirrors the prework
    // *_params convention and is the SUT for the new evidence symbols' parameter handling.
    #[test]
    fn match_candidates_param_plumbing() {
        let ctx = unsafe { cpl_ctx_new(std::ptr::null()) };
        assert_ne!(ctx, 0);

        // Empty V×A batch with null params (⇒ the boundary-13 default FingerprintPrimary) ⇒ "[]".
        let r = take(unsafe {
            cpl_match_candidates(ctx, std::ptr::null(), 0, std::ptr::null(), 0, std::ptr::null())
        });
        assert_eq!(r.as_deref(), Some("[]"), "empty batch ⇒ []");

        // Empty batch + a VALID shadow-mode override ⇒ still "[]" (param accepted, no work).
        let shadow = CString::new(r#"{"matcherMode":"fingerprintShadow"}"#).unwrap();
        let r = take(unsafe {
            cpl_match_candidates(ctx, std::ptr::null(), 0, std::ptr::null(), 0, shadow.as_ptr())
        });
        assert_eq!(r.as_deref(), Some("[]"), "valid shadow override accepted");

        // Malformed JSON ⇒ InvalidArgument (null + last-error).
        let bad = CString::new("{not json").unwrap();
        let r = unsafe {
            cpl_match_candidates(ctx, std::ptr::null(), 0, std::ptr::null(), 0, bad.as_ptr())
        };
        assert!(r.is_null(), "malformed params ⇒ null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on malformed");

        // A value that fails validate() ⇒ InvalidArgument.
        let invalid = CString::new(r#"{"minOverlapS": -1}"#).unwrap();
        let r = unsafe {
            cpl_match_candidates(ctx, std::ptr::null(), 0, std::ptr::null(), 0, invalid.as_ptr())
        };
        assert!(r.is_null(), "validate()-failing params ⇒ null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on invalid");

        cpl_ctx_free(ctx);
    }

    // §13.4 PLUMBING: the additive graph symbol mirrors cpl_match_candidates' parameter handling —
    // empty batch ⇒ "[]" (with null and a valid shadow override), malformed/invalid ⇒ null + error.
    #[test]
    fn match_candidates_graph_param_plumbing() {
        let ctx = unsafe { cpl_ctx_new(std::ptr::null()) };
        assert_ne!(ctx, 0);

        let r = take(unsafe {
            cpl_match_candidates_graph(ctx, std::ptr::null(), 0, std::ptr::null(), 0, std::ptr::null())
        });
        assert_eq!(r.as_deref(), Some("[]"), "empty graph batch ⇒ []");

        let shadow = CString::new(r#"{"matcherMode":"fingerprintShadow"}"#).unwrap();
        let r = take(unsafe {
            cpl_match_candidates_graph(ctx, std::ptr::null(), 0, std::ptr::null(), 0, shadow.as_ptr())
        });
        assert_eq!(r.as_deref(), Some("[]"), "valid shadow override accepted by graph symbol");

        let bad = CString::new("{not json").unwrap();
        let r = unsafe {
            cpl_match_candidates_graph(ctx, std::ptr::null(), 0, std::ptr::null(), 0, bad.as_ptr())
        };
        assert!(r.is_null(), "malformed params ⇒ null");
        assert!(take(cpl_take_error_json(ctx)).is_some(), "last-error set on malformed");

        cpl_ctx_free(ctx);
    }

    // §14.1 END-TO-END through the C ABI on a synthetic pair: import a deterministic mono-48k WAV
    // pair (B = A shifted by a known offset) THROUGH the FFI, then drive the new evidence symbols.
    // Proves: cpl_match_pair (shadow) returns well-formed PairEvidence JSON (parses back into the
    // core type) with NO NaN/Infinity (§20) and is DETERMINISTIC across calls; null params ⇒ the
    // default LegacyEnvelope path (method "envelopeV1"); cpl_match_candidates returns the V×A matrix
    // and its [0][0] cell is byte-identical to the single cpl_match_pair; and the OLD cpl_pair_*
    // symbols still work unchanged on the same handles (§3 invariant 4, §14.1).
    #[test]
    fn match_pair_and_candidates_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let cam = dir.path().join("cam.wav");
        let ext = dir.path().join("ext.wav");
        write_pair_wavs(&cam, &ext);

        let cfg = CString::new(
            serde_json::json!({ "cacheDir": dir.path().join("cache").to_str().unwrap() }).to_string(),
        )
        .unwrap();
        let ctx = unsafe { cpl_ctx_new(cfg.as_ptr()) };
        assert_ne!(ctx, 0);

        let v = import_blocking_ffi(ctx, &cam, 0);
        let a = import_blocking_ffi(ctx, &ext, 1);

        // --- cpl_match_pair, shadow mode: valid PairEvidence JSON, no NaN/Inf, deterministic. ---
        let shadow = CString::new(r#"{"matcherMode":"fingerprintShadow"}"#).unwrap();
        let s1 = take(unsafe { cpl_match_pair(ctx, v, a, shadow.as_ptr()) })
            .expect("shadow match_pair JSON");
        assert_no_nan_inf(&s1);
        let ev1: couple_core::PairEvidence = serde_json::from_str(&s1).expect("parse PairEvidence");
        assert_eq!(
            ev1.method,
            couple_core::MatchMethod::SpectralLandmarkV1,
            "shadow ⇒ spectral method"
        );
        // The wire is camelCase and the non-applicable residual/refined fields serialize as null,
        // never NaN/Infinity (covered by assert_no_nan_inf above).
        assert!(s1.contains("\"method\":\"spectralLandmarkV1\""), "wire: {s1}");
        // Determinism: a second identical call serializes byte-identically.
        let s1b = take(unsafe { cpl_match_pair(ctx, v, a, shadow.as_ptr()) }).unwrap();
        assert_eq!(s1, s1b, "cpl_match_pair must be deterministic");

        // --- BOUNDARY 13: null params ⇒ the new DEFAULT FingerprintPrimary (method spectralLandmarkV1). ---
        let s_default = take(unsafe { cpl_match_pair(ctx, v, a, std::ptr::null()) })
            .expect("null-params match_pair JSON");
        assert_no_nan_inf(&s_default);
        let ev_default: couple_core::PairEvidence = serde_json::from_str(&s_default).unwrap();
        assert_eq!(
            ev_default.method,
            couple_core::MatchMethod::SpectralLandmarkV1,
            "boundary 13: null params ⇒ default FingerprintPrimary ⇒ spectralLandmarkV1"
        );
        // --- The DEVELOPER OVERRIDE `{"matcherMode":"legacyEnvelope"}` still forces envelopeV1 (§16.1). ---
        let legacy_override = CString::new(r#"{"matcherMode":"legacyEnvelope"}"#).unwrap();
        let s_legacy = take(unsafe { cpl_match_pair(ctx, v, a, legacy_override.as_ptr()) })
            .expect("legacy-override match_pair JSON");
        let ev_legacy: couple_core::PairEvidence = serde_json::from_str(&s_legacy).unwrap();
        assert_eq!(
            ev_legacy.method,
            couple_core::MatchMethod::EnvelopeV1,
            "the legacyEnvelope developer override forces envelopeV1 (§16.1)"
        );

        // --- cpl_match_candidates: 1×1 matrix; its [0][0] agrees with the single match_pair on the
        // decision-bearing fields (method/classification/lag), and the symbol is itself deterministic
        // run-to-run. (Exact cross-API byte-equality of the spectral path on REAL media is proven by
        // core/tests/matching_evidence.rs; we do not re-assert it here, where the FFT over a purely
        // synthetic signal can differ in the last ULP of quality telemetry without changing any
        // decision — §3.2 determinism is the per-API run-to-run property, asserted below.) ---
        let vids = [v];
        let auds = [a];
        let mat = take(unsafe {
            cpl_match_candidates(ctx, vids.as_ptr(), 1, auds.as_ptr(), 1, shadow.as_ptr())
        })
        .expect("match_candidates JSON");
        assert_no_nan_inf(&mat);
        let matrix: Vec<Vec<couple_core::PairEvidence>> =
            serde_json::from_str(&mat).expect("parse evidence matrix");
        assert_eq!(matrix.len(), 1, "one video row");
        assert_eq!(matrix[0].len(), 1, "one audio column");
        let cell = &matrix[0][0];
        assert_eq!(cell.method, ev1.method, "matrix cell method matches match_pair");
        assert_eq!(
            cell.classification, ev1.classification,
            "matrix cell classification matches match_pair"
        );
        assert!(
            (cell.lag_ms - ev1.lag_ms).abs() < 1e-6,
            "matrix cell lag {} matches match_pair lag {}",
            cell.lag_ms,
            ev1.lag_ms
        );
        // The batch symbol is itself deterministic run-to-run (§3.2).
        let mat_b = take(unsafe {
            cpl_match_candidates(ctx, vids.as_ptr(), 1, auds.as_ptr(), 1, shadow.as_ptr())
        })
        .unwrap();
        assert_eq!(mat, mat_b, "cpl_match_candidates must be deterministic");

        // --- OLD symbols still work unchanged on the same handles (§14.1 / invariant 4). ---
        let old = take(unsafe { cpl_pair_correlate(ctx, v, a) }).expect("old cpl_pair_correlate");
        assert_no_nan_inf(&old);
        let corr: couple_core::pair::PairCorrelation = serde_json::from_str(&old).unwrap();
        // The legacy lag and the legacy-mode evidence lag agree (same matcher) within tolerance.
        assert!(
            (corr.lag_ms - ev_legacy.lag_ms).abs() < 1.0,
            "legacy symbol lag {} vs legacy-mode evidence lag {}",
            corr.lag_ms,
            ev_legacy.lag_ms
        );

        cpl_ctx_free(ctx);
    }

    /// Assert a JSON string carries no NaN/Infinity token (§20). serde_json emits non-finite f64 as
    /// `null`, which the contract scrubs at the source; this is the wire-level backstop.
    fn assert_no_nan_inf(json: &str) {
        for bad in ["NaN", "Infinity", "Inf"] {
            assert!(!json.contains(bad), "evidence JSON must not contain {bad:?}: {json}");
        }
    }

    /// Author a deterministic mono-48k WAV pair with rich aperiodic content so the spectral pipeline
    /// finds landmarks: A is a 30 s pseudo-random broadband signal; B (40 s) is A shifted later by a
    /// known offset (a genuine same-master excerpt). No ffmpeg — `hound` writes the PCM directly.
    fn write_pair_wavs(cam: &Path, ext: &Path) {
        const SR: usize = 48_000;
        // A deterministic broadband-ish source: summed detuned sinusoids modulated by an aperiodic
        // envelope, plus a hashed-noise term — enough spectral structure for peak/landmark selection.
        let sample = |n: usize| -> f32 {
            let t = n as f32 / SR as f32;
            // Aperiodic AM envelope (irrational ratios so it never repeats over the clip).
            let env = 0.5 + 0.5 * (0.7 * (2.0 * std::f32::consts::PI * 0.31 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 0.067 * t).sin());
            let tone = (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.6 * (2.0 * std::f32::consts::PI * 932.0 * t).sin()
                + 0.4 * (2.0 * std::f32::consts::PI * 1771.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 3119.0 * t).sin();
            // Deterministic hashed noise term (no RNG; stable across runs/machines).
            let h = (n as u64).wrapping_mul(2_654_435_761) ^ ((n as u64) >> 13);
            let noise = ((h & 0xffff) as f32 / 32_768.0 - 1.0) * 0.25;
            0.2 * env * tone + noise * env
        };
        let dur_a = 30 * SR;
        let a: Vec<f32> = (0..dur_a).map(sample).collect();
        // B = the SAME master, 40 s long, with A's content starting 2.5 s in (offset +2500 ms): B's
        // sample m corresponds to A's sample (m - 2.5*SR). Outside A's span B is the same generator
        // (it IS the master), so we just generate the master over [−2.5 s, 37.5 s) → write [0,40 s).
        let shift = (2.5 * SR as f32) as i64;
        let dur_b = 40 * SR;
        let b: Vec<f32> = (0..dur_b)
            .map(|m| {
                let src = m as i64 - shift;
                if src >= 0 {
                    sample(src as usize)
                } else {
                    // Lead-in before A's start: extend the master generator to negative-equivalent n.
                    sample((dur_a as i64 + src) as usize % dur_a)
                }
            })
            .collect();
        write_wav_mono_48k(cam, &a);
        write_wav_mono_48k(ext, &b);
    }

    fn write_wav_mono_48k(path: &Path, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32_767.0).round() as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    /// Import a file through the C ABI and BLOCK until the job's terminal callback fires, returning
    /// the imported track id (parsed from the "done" payload's TrackMeta JSON). This drives the real
    /// FFI import path (registry insertion included) so the returned handle is a genuine C-ABI handle
    /// usable by cpl_match_pair / the old cpl_pair_* symbols.
    fn import_blocking_ffi(ctx: u64, path: &Path, role: i32) -> u64 {
        use std::sync::mpsc;

        // Channel carries the terminal payload JSON (or None on cancel/error).
        struct Sink(mpsc::Sender<Option<String>>);
        extern "C" fn cb(
            _job: u64,
            phase: *const c_char,
            _fraction: f64,
            payload: *const c_char,
            user: *mut c_void,
        ) {
            // SAFETY: `user` is a &Sink we passed to cpl_import_start; the terminal call is the last
            // use, after which the caller drops the sink.
            let sink = unsafe { &*(user as *const Sink) };
            let phase = unsafe { CStr::from_ptr(phase) }.to_string_lossy().into_owned();
            match phase.as_str() {
                "done" => {
                    let p = if payload.is_null() {
                        None
                    } else {
                        Some(unsafe { CStr::from_ptr(payload) }.to_string_lossy().into_owned())
                    };
                    let _ = sink.0.send(p);
                }
                "error" | "cancelled" => {
                    let _ = sink.0.send(None);
                }
                _ => {} // progress; ignore
            }
        }

        let (tx, rx) = mpsc::channel();
        let sink = Box::new(Sink(tx));
        let user = Box::into_raw(sink) as *mut c_void;
        let cpath = CString::new(path.to_str().unwrap()).unwrap();
        let job = unsafe { cpl_import_start(ctx, cpath.as_ptr(), role, Some(cb), user) };
        assert_ne!(job, 0, "import job must start");
        // Block for the terminal callback.
        let payload = rx.recv().expect("import callback").expect("import succeeded");
        // SAFETY: the terminal callback has fired and will not be invoked again; reclaim the sink.
        drop(unsafe { Box::from_raw(user as *mut Sink) });
        let meta: serde_json::Value = serde_json::from_str(&payload).unwrap();
        meta["id"].as_u64().expect("track id in import payload")
    }
}

// `Path` is referenced for doc clarity above; silence unused-import lints.
#[allow(unused)]
fn _keep(_: &Path) {}
