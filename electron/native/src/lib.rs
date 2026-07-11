//! couple-node — N-API binding over the couple-ffi C ABI (SPEC §4.9, §13).
//!
//! This is the Electron shell's `Engine`/`Wire` port per PLAN-CORE-SHELL-BOUNDARY
//! §4-G: a THIN binding, one exported JS function per `cpl_*` symbol, no logic.
//! Conventions mirror the C ABI exactly:
//!   - handles cross as BigInt (u64; 0 = invalid/error),
//!   - structured data crosses as JSON strings (the kit layer decodes),
//!   - `char*`-returning symbols surface as `string | null`; on null the caller
//!     reads `takeErrorJson`,
//!   - status-returning symbols surface the raw i32 (0 = ok),
//!   - hot paths (`band`, `rawWindow`, `auditionFill`) fill caller-provided
//!     TypedArrays, no per-call allocation on the Rust side,
//!   - job symbols take a JS callback `(phase, fraction, payloadJson|null)` and
//!     forward worker-thread progress through a ThreadsafeFunction; the terminal
//!     phase ("done" | "error" | "cancelled") releases it.
//! Strings returned by core are freed here via `cpl_string_free` before crossing.
#![allow(unused_unsafe)]

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use napi::JsFunction;
use napi_derive::napi;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};

// couple-ffi's [lib] name is "couple" (the staticlib is libcouple.a).
use couple as c;

mod audio;

// ---------- helpers ----------

/// Consume a core-allocated C string (the JS-side twin of Engine.takeString).
fn take(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { c::cpl_string_free(p) };
    Some(s)
}

/// NUL-safe CString (paths/JSON never legitimately contain NUL).
fn cs(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

/// Optional-JSON argument: None/absent crosses as a null pointer (core defaults).
struct OptCStr(Option<CString>);

impl OptCStr {
    fn new(s: Option<String>) -> Self {
        OptCStr(s.map(|v| cs(&v)))
    }
    fn ptr(&self) -> *const c_char {
        self.0.as_ref().map_or(std::ptr::null(), |v| v.as_ptr())
    }
}

fn h(b: &BigInt) -> u64 {
    b.get_u64().1
}

fn ids(v: &[BigInt]) -> Vec<u64> {
    v.iter().map(h).collect()
}

// ---------- job progress plumbing ----------

struct Progress {
    phase: String,
    fraction: f64,
    payload: Option<String>,
}

type Tsfn = ThreadsafeFunction<Progress, ErrorStrategy::Fatal>;

fn make_tsfn(cb: JsFunction) -> Result<Tsfn> {
    cb.create_threadsafe_function(0, |cx: ThreadSafeCallContext<Progress>| {
        let phase = cx.env.create_string(&cx.value.phase)?.into_unknown();
        let fraction = cx.env.create_double(cx.value.fraction)?.into_unknown();
        let payload = match &cx.value.payload {
            Some(s) => cx.env.create_string(s)?.into_unknown(),
            None => cx.env.get_null()?.into_unknown(),
        };
        Ok(vec![phase, fraction, payload])
    })
}

/// The C-side progress trampoline (the JS twin of CoupleKit's `jobTrampoline`).
/// `user` owns a boxed ThreadsafeFunction; the terminal call releases it. The
/// core guarantees the terminal callback is the last for a job, and TSFN release
/// still flushes already-queued calls, so the terminal event always reaches JS.
extern "C" fn trampoline(
    _job: u64,
    phase: *const c_char,
    fraction: f64,
    payload_json: *const c_char,
    user: *mut c_void,
) {
    if user.is_null() {
        return;
    }
    let phase_s = if phase.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(phase) }.to_string_lossy().into_owned()
    };
    let payload = if payload_json.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(payload_json) }.to_string_lossy().into_owned())
    };
    let terminal = matches!(phase_s.as_str(), "done" | "error" | "cancelled");
    let tsfn = unsafe { &*(user as *const Tsfn) };
    tsfn.call(
        Progress { phase: phase_s, fraction, payload },
        ThreadsafeFunctionCallMode::NonBlocking,
    );
    if terminal {
        drop(unsafe { Box::from_raw(user as *mut Tsfn) });
    }
}

/// Box a TSFN into a `user` pointer for a job start; the caller must release it
/// (via `release_user`) if the start returns 0.
fn user_ptr(cb: JsFunction) -> Result<*mut c_void> {
    Ok(Box::into_raw(Box::new(make_tsfn(cb)?)) as *mut c_void)
}

fn release_user(user: *mut c_void) {
    drop(unsafe { Box::from_raw(user as *mut Tsfn) });
}

// ---------- lifecycle ----------

#[napi]
pub fn abi_version() -> u32 {
    unsafe { c::cpl_abi_version() }
}

#[napi]
pub fn version() -> Option<String> {
    take(unsafe { c::cpl_version() })
}

/// `configJson`: `{"ffmpegPath","ffprobePath","cacheDir"}`, all optional. 0n = error.
#[napi]
pub fn ctx_new(config_json: Option<String>) -> u64 {
    let cfg = OptCStr::new(config_json);
    unsafe { c::cpl_ctx_new(cfg.ptr()) }
}

#[napi]
pub fn ctx_free(ctx: BigInt) {
    unsafe { c::cpl_ctx_free(h(&ctx)) }
}

#[napi]
pub fn take_error_json(ctx: BigInt) -> Option<String> {
    take(unsafe { c::cpl_take_error_json(h(&ctx)) })
}

#[napi]
pub fn job_cancel(ctx: BigInt, job: BigInt) {
    unsafe { c::cpl_job_cancel(h(&ctx), h(&job)) }
}

#[napi]
pub fn job_free(ctx: BigInt, job: BigInt) {
    unsafe { c::cpl_job_free(h(&ctx), h(&job)) }
}

// ---------- media / tracks ----------

#[napi]
pub fn probe_json(ctx: BigInt, path: String) -> Option<String> {
    let p = cs(&path);
    take(unsafe { c::cpl_probe_json(h(&ctx), p.as_ptr()) })
}

/// role: 0 = camera, 1 = external. Returns the job id (0n = error).
#[napi]
pub fn import_start(ctx: BigInt, path: String, role: i32, callback: JsFunction) -> Result<u64> {
    let user = user_ptr(callback)?;
    let p = cs(&path);
    let id = unsafe { c::cpl_import_start(h(&ctx), p.as_ptr(), role, Some(trampoline), user) };
    if id == 0 {
        release_user(user);
    }
    Ok(id)
}

#[napi]
pub fn track_info_json(ctx: BigInt, track: BigInt) -> Option<String> {
    take(unsafe { c::cpl_track_info_json(h(&ctx), h(&track)) })
}

#[napi]
pub fn track_free(ctx: BigInt, track: BigInt) {
    unsafe { c::cpl_track_free(h(&ctx), h(&track)) }
}

#[napi(object)]
pub struct BandInfo {
    pub level_spb: u32,
    pub col_t0: f64,
    pub col_dt: f64,
}

/// Fills the first `nCols` of `mins`/`maxs` (caller-allocated, len >= nCols).
/// None on error (read `takeErrorJson`); also None on a short buffer.
#[napi]
pub fn band(
    ctx: BigInt,
    track: BigInt,
    t0: f64,
    t1: f64,
    n_cols: u32,
    mut mins: Int8Array,
    mut maxs: Int8Array,
) -> Option<BandInfo> {
    if (mins.len() as u32) < n_cols || (maxs.len() as u32) < n_cols {
        return None;
    }
    let mut level_spb = 0u32;
    let mut col_t0 = 0f64;
    let mut col_dt = 0f64;
    let rc = unsafe {
        c::cpl_band(
            h(&ctx),
            h(&track),
            t0,
            t1,
            n_cols,
            mins.as_mut_ptr(),
            maxs.as_mut_ptr(),
            &mut level_spb,
            &mut col_t0,
            &mut col_dt,
        )
    };
    (rc == 0).then_some(BandInfo { level_spb, col_t0, col_dt })
}

/// Mono 48 kHz f32 into `out`; returns frames written, None on error.
#[napi]
pub fn raw_window(
    ctx: BigInt,
    track: BigInt,
    t0: f64,
    t1: f64,
    mut out: Float32Array,
) -> Option<u32> {
    let mut frames = 0u32;
    let rc = unsafe {
        c::cpl_raw_window(h(&ctx), h(&track), t0, t1, out.as_mut_ptr(), out.len() as u32, &mut frames)
    };
    (rc == 0).then_some(frames)
}

#[napi]
pub fn rms(ctx: BigInt, track: BigInt, t0: f64, t1: f64) -> Option<f64> {
    let mut dbfs = 0f64;
    let rc = unsafe { c::cpl_rms(h(&ctx), h(&track), t0, t1, &mut dbfs) };
    (rc == 0).then_some(dbfs)
}

#[napi]
pub fn onsets_json(ctx: BigInt, track: BigInt, t0: f64, t1: f64, limit: u32) -> Option<String> {
    take(unsafe { c::cpl_onsets_json(h(&ctx), h(&track), t0, t1, limit) })
}

#[napi]
pub fn best_onset_in_json(ctx: BigInt, track: BigInt, t0: f64, t1: f64) -> Option<String> {
    take(unsafe { c::cpl_best_onset_in_json(h(&ctx), h(&track), t0, t1) })
}

#[napi]
pub fn best_onset_near_json(ctx: BigInt, track: BigInt, t: f64, half_span_s: f64) -> Option<String> {
    take(unsafe { c::cpl_best_onset_near_json(h(&ctx), h(&track), t, half_span_s) })
}

#[napi]
pub fn track_stats_json(ctx: BigInt, track: BigInt) -> Option<String> {
    take(unsafe { c::cpl_track_stats_json(h(&ctx), h(&track)) })
}

// ---------- session ----------

#[napi]
pub fn session_new(ctx: BigInt, camera: BigInt, external: BigInt) -> u64 {
    unsafe { c::cpl_session_new(h(&ctx), h(&camera), h(&external)) }
}

#[napi]
pub fn session_free(ctx: BigInt, session: BigInt) {
    unsafe { c::cpl_session_free(h(&ctx), h(&session)) }
}

#[napi]
pub fn state_json(ctx: BigInt, session: BigInt) -> Option<String> {
    take(unsafe { c::cpl_state_json(h(&ctx), h(&session)) })
}

#[napi]
pub fn set_state_json(ctx: BigInt, session: BigInt, json: String) -> i32 {
    let j = cs(&json);
    unsafe { c::cpl_set_state_json(h(&ctx), h(&session), j.as_ptr()) }
}

#[napi]
pub fn set_offset_ms(ctx: BigInt, session: BigInt, v: f64) -> i32 {
    unsafe { c::cpl_set_offset_ms(h(&ctx), h(&session), v) }
}

#[napi]
pub fn set_drift_ppm_live(ctx: BigInt, session: BigInt, v: f64) -> i32 {
    unsafe { c::cpl_set_drift_ppm_live(h(&ctx), h(&session), v) }
}

#[napi]
pub fn commit_drift(ctx: BigInt, session: BigInt) -> i32 {
    unsafe { c::cpl_commit_drift(h(&ctx), h(&session)) }
}

/// mode: 0 = resample, 1 = none.
#[napi]
pub fn set_drift_mode(ctx: BigInt, session: BigInt, mode: i32) -> i32 {
    unsafe { c::cpl_set_drift_mode(h(&ctx), h(&session), mode) }
}

#[napi]
pub fn set_verified(ctx: BigInt, session: BigInt, verified: bool) -> i32 {
    unsafe { c::cpl_set_verified(h(&ctx), h(&session), verified) }
}

// ---------- sync ----------

#[napi]
pub fn autosync_start(
    ctx: BigInt,
    session: BigInt,
    params_json: Option<String>,
    callback: JsFunction,
) -> Result<u64> {
    let user = user_ptr(callback)?;
    let params = OptCStr::new(params_json);
    let id = unsafe {
        c::cpl_autosync_start(h(&ctx), h(&session), params.ptr(), Some(trampoline), user)
    };
    if id == 0 {
        release_user(user);
    }
    Ok(id)
}

#[napi]
pub fn autosync_start_seeded(
    ctx: BigInt,
    session: BigInt,
    params_json: Option<String>,
    seed_offset_ms: f64,
    seed_drift_ppm: f64,
    callback: JsFunction,
) -> Result<u64> {
    let user = user_ptr(callback)?;
    let params = OptCStr::new(params_json);
    let id = unsafe {
        c::cpl_autosync_start_seeded(
            h(&ctx),
            h(&session),
            params.ptr(),
            seed_offset_ms,
            seed_drift_ppm,
            Some(trampoline),
            user,
        )
    };
    if id == 0 {
        release_user(user);
    }
    Ok(id)
}

#[napi]
pub fn measure_local_json(
    ctx: BigInt,
    session: BigInt,
    tc: f64,
    half_win_s: f64,
    center_ms: f64,
    span_ms: f64,
) -> Option<String> {
    take(unsafe { c::cpl_measure_local_json(h(&ctx), h(&session), tc, half_win_s, center_ms, span_ms) })
}

#[napi]
pub fn fit_ls_json(points_json: String) -> Option<String> {
    let p = cs(&points_json);
    take(unsafe { c::cpl_fit_ls_json(p.as_ptr()) })
}

// ---------- audition ----------

/// mode: 0 = pan, 1 = overlay, 2 = null. Returns the stream id (0n = error).
#[napi]
pub fn audition_open(ctx: BigInt, session: BigInt, mode: i32, t_start: f64) -> u64 {
    unsafe { c::cpl_audition_open(h(&ctx), h(&session), mode, t_start) }
}

#[napi(object)]
pub struct AuditionFillInfo {
    pub frames: u32,
    pub stream_t: f64,
}

/// Fills `out` (interleaved stereo f32, capacity = out.length / 2 frames).
/// The production audio path is the native pump (M4); this export exists for
/// tests and diagnostics.
#[napi]
pub fn audition_fill(ctx: BigInt, stream: BigInt, mut out: Float32Array) -> Option<AuditionFillInfo> {
    let capacity_frames = (out.len() / 2) as u32;
    let mut frames = 0u32;
    let mut stream_t = 0f64;
    let rc = unsafe {
        c::cpl_audition_fill(
            h(&ctx),
            h(&stream),
            out.as_mut_ptr(),
            capacity_frames,
            &mut frames,
            &mut stream_t,
        )
    };
    (rc == 0).then_some(AuditionFillInfo { frames, stream_t })
}

#[napi]
pub fn audition_seek(ctx: BigInt, stream: BigInt, t: f64) -> i32 {
    unsafe { c::cpl_audition_seek(h(&ctx), h(&stream), t) }
}

#[napi]
pub fn audition_set_mode(ctx: BigInt, stream: BigInt, mode: i32) -> i32 {
    unsafe { c::cpl_audition_set_mode(h(&ctx), h(&stream), mode) }
}

#[napi]
pub fn audition_set_gains(ctx: BigInt, stream: BigInt, gain_cam: f64, gain_ext: f64) -> i32 {
    unsafe { c::cpl_audition_set_gains(h(&ctx), h(&stream), gain_cam as f32, gain_ext as f32) }
}

#[napi]
pub fn audition_close(ctx: BigInt, stream: BigInt) {
    unsafe { c::cpl_audition_close(h(&ctx), h(&stream)) }
}

// ---------- audio output pump (shell platform layer, §4-G) ----------

/// Start the cpal output pump over an open audition stream. One pump at a
/// time; restarting replaces the previous one. Throws on device failure.
#[napi]
pub fn audio_start(ctx: BigInt, stream: BigInt) -> Result<()> {
    audio::start(h(&ctx), h(&stream)).map_err(|e| Error::from_reason(e))
}

#[napi]
pub fn audio_stop() {
    audio::stop();
}

/// Audition video-time clock (latency-compensated); NaN when stopped.
#[napi]
pub fn audio_clock() -> f64 {
    audio::clock()
}

#[napi]
pub fn audio_underruns() -> u32 {
    audio::underruns()
}

#[napi]
pub fn audio_running() -> bool {
    audio::running()
}

// ---------- export ----------

#[napi]
pub fn export_start(
    ctx: BigInt,
    session: BigInt,
    opts_json: String,
    callback: JsFunction,
) -> Result<u64> {
    let user = user_ptr(callback)?;
    let opts = cs(&opts_json);
    let id = unsafe {
        c::cpl_export_start(h(&ctx), h(&session), opts.as_ptr(), Some(trampoline), user)
    };
    if id == 0 {
        release_user(user);
    }
    Ok(id)
}

// ---------- pairing / matching ----------

#[napi]
pub fn pairing_params_default_json() -> Option<String> {
    take(unsafe { c::cpl_pairing_params_default_json() })
}

#[napi]
pub fn pair_correlate(ctx: BigInt, track_a: BigInt, track_b: BigInt) -> Option<String> {
    take(unsafe { c::cpl_pair_correlate(h(&ctx), h(&track_a), h(&track_b)) })
}

#[napi]
pub fn pair_correlate_params(
    ctx: BigInt,
    track_a: BigInt,
    track_b: BigInt,
    params_json: Option<String>,
) -> Option<String> {
    let params = OptCStr::new(params_json);
    take(unsafe { c::cpl_pair_correlate_params(h(&ctx), h(&track_a), h(&track_b), params.ptr()) })
}

#[napi]
pub fn pair_candidates(ctx: BigInt, videos: Vec<BigInt>, audios: Vec<BigInt>) -> Option<String> {
    let (vs, aus) = (ids(&videos), ids(&audios));
    take(unsafe {
        c::cpl_pair_candidates(h(&ctx), vs.as_ptr(), vs.len(), aus.as_ptr(), aus.len())
    })
}

#[napi]
pub fn pair_candidates_params(
    ctx: BigInt,
    videos: Vec<BigInt>,
    audios: Vec<BigInt>,
    params_json: Option<String>,
) -> Option<String> {
    let (vs, aus) = (ids(&videos), ids(&audios));
    let params = OptCStr::new(params_json);
    take(unsafe {
        c::cpl_pair_candidates_params(
            h(&ctx),
            vs.as_ptr(),
            vs.len(),
            aus.as_ptr(),
            aus.len(),
            params.ptr(),
        )
    })
}

#[napi]
pub fn match_pair(
    ctx: BigInt,
    track_a: BigInt,
    track_b: BigInt,
    pairing_params_json: Option<String>,
) -> Option<String> {
    let params = OptCStr::new(pairing_params_json);
    take(unsafe { c::cpl_match_pair(h(&ctx), h(&track_a), h(&track_b), params.ptr()) })
}

#[napi]
pub fn match_candidates(
    ctx: BigInt,
    videos: Vec<BigInt>,
    audios: Vec<BigInt>,
    pairing_params_json: Option<String>,
) -> Option<String> {
    let (vs, aus) = (ids(&videos), ids(&audios));
    let params = OptCStr::new(pairing_params_json);
    take(unsafe {
        c::cpl_match_candidates(
            h(&ctx),
            vs.as_ptr(),
            vs.len(),
            aus.as_ptr(),
            aus.len(),
            params.ptr(),
        )
    })
}

#[napi]
pub fn match_candidates_graph(
    ctx: BigInt,
    videos: Vec<BigInt>,
    audios: Vec<BigInt>,
    pairing_params_json: Option<String>,
) -> Option<String> {
    let (vs, aus) = (ids(&videos), ids(&audios));
    let params = OptCStr::new(pairing_params_json);
    take(unsafe {
        c::cpl_match_candidates_graph(
            h(&ctx),
            vs.as_ptr(),
            vs.len(),
            aus.as_ptr(),
            aus.len(),
            params.ptr(),
        )
    })
}

#[napi]
pub fn match_candidates_graph_start(
    ctx: BigInt,
    videos: Vec<BigInt>,
    audios: Vec<BigInt>,
    pairing_params_json: Option<String>,
    callback: JsFunction,
) -> Result<u64> {
    let user = user_ptr(callback)?;
    let (vs, aus) = (ids(&videos), ids(&audios));
    let params = OptCStr::new(pairing_params_json);
    let id = unsafe {
        c::cpl_match_candidates_graph_start(
            h(&ctx),
            vs.as_ptr(),
            vs.len(),
            aus.as_ptr(),
            aus.len(),
            params.ptr(),
            Some(trampoline),
            user,
        )
    };
    if id == 0 {
        release_user(user);
    }
    Ok(id)
}

#[napi]
pub fn triangle_closes(request_json: String) -> Option<String> {
    let r = cs(&request_json);
    take(unsafe { c::cpl_triangle_closes(r.as_ptr()) })
}

#[napi]
pub fn pair_fit_batch(ctx: BigInt, pairs_json: String) -> Option<String> {
    let p = cs(&pairs_json);
    take(unsafe { c::cpl_pair_fit_batch(h(&ctx), p.as_ptr()) })
}

#[napi]
pub fn pair_fit_batch_params(
    ctx: BigInt,
    pairs_json: String,
    params_json: Option<String>,
) -> Option<String> {
    let p = cs(&pairs_json);
    let params = OptCStr::new(params_json);
    take(unsafe { c::cpl_pair_fit_batch_params(h(&ctx), p.as_ptr(), params.ptr()) })
}

#[napi]
pub fn verify_batch(ctx: BigInt, pairs_json: String) -> Option<String> {
    let p = cs(&pairs_json);
    take(unsafe { c::cpl_verify_batch(h(&ctx), p.as_ptr()) })
}

#[napi]
pub fn verify_batch_params(
    ctx: BigInt,
    pairs_json: String,
    params_json: Option<String>,
) -> Option<String> {
    let p = cs(&pairs_json);
    let params = OptCStr::new(params_json);
    take(unsafe { c::cpl_verify_batch_params(h(&ctx), p.as_ptr(), params.ptr()) })
}

// ---------- row resolution ----------

#[napi]
pub fn resolve_params_default_json() -> Option<String> {
    take(unsafe { c::cpl_resolve_params_default_json() })
}

#[napi]
pub fn resolve(
    ctx: BigInt,
    videos: Vec<BigInt>,
    audios: Vec<BigInt>,
    params_json: Option<String>,
) -> Option<String> {
    let (vs, aus) = (ids(&videos), ids(&audios));
    let params = OptCStr::new(params_json);
    take(unsafe {
        c::cpl_resolve(h(&ctx), vs.as_ptr(), vs.len(), aus.as_ptr(), aus.len(), params.ptr())
    })
}

#[napi]
pub fn resolve_start(
    ctx: BigInt,
    videos: Vec<BigInt>,
    audios: Vec<BigInt>,
    params_json: Option<String>,
    callback: JsFunction,
) -> Result<u64> {
    let user = user_ptr(callback)?;
    let (vs, aus) = (ids(&videos), ids(&audios));
    let params = OptCStr::new(params_json);
    let id = unsafe {
        c::cpl_resolve_start(
            h(&ctx),
            vs.as_ptr(),
            vs.len(),
            aus.as_ptr(),
            aus.len(),
            params.ptr(),
            Some(trampoline),
            user,
        )
    };
    if id == 0 {
        release_user(user);
    }
    Ok(id)
}

// ---------- pure policies ----------

#[napi]
pub fn policy_match_quality(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_match_quality(j.as_ptr()) })
}

#[napi]
pub fn policy_repair_drop(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_repair_drop(j.as_ptr()) })
}

#[napi]
pub fn policy_repair_forced(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_repair_forced(j.as_ptr()) })
}

#[napi]
pub fn policy_row_merge(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_row_merge(j.as_ptr()) })
}

#[napi]
pub fn policy_media_removal(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_media_removal(j.as_ptr()) })
}

#[napi]
pub fn policy_anchor_propose(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_anchor_propose(j.as_ptr()) })
}

#[napi]
pub fn policy_shadow_rows(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_shadow_rows(j.as_ptr()) })
}

#[napi]
pub fn policy_activity(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_activity(j.as_ptr()) })
}

#[napi]
pub fn policy_sync_seed(json: String) -> Option<String> {
    let j = cs(&json);
    take(unsafe { c::cpl_policy_sync_seed(j.as_ptr()) })
}

// ---------- cache ----------

#[napi]
pub fn cache_gc(ctx: BigInt, max_bytes: BigInt) -> u64 {
    unsafe { c::cpl_cache_gc(h(&ctx), h(&max_bytes)) }
}
