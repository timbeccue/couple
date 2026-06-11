//! couple-ffi — the C ABI porting surface (§4.9), Phases 1–3 inventory.
//! Audition/export functions land with Phase 4/6.

#![allow(clippy::missing_safety_doc)]

use couple_core::job::{CancelToken, ProgressFn};
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
    #[test]
    fn abi_version_is_one() {
        assert_eq!(super::cpl_abi_version(), 1);
    }
}

// `Path` is referenced for doc clarity above; silence unused-import lints.
#[allow(unused)]
fn _keep(_: &Path) {}
