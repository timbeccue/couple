//! Audition streams (§4.8): sample-accurate aligned stereo for a video-time
//! cursor and monitor mode. The fill path is called from the shell's audio
//! render thread — atomics only, no allocation after open.
//!
//! Mapping semantics (normative, §4.8): offset is always live and
//! sample-accurate; drift renders at the *committed* value, and only when
//! driftMode = resample — in "none" playback is offset-only via an
//! integer-sample shift, matching export (§8.2/F8). Drift rendering is a
//! 32-tap Kaiser windowed sinc [SPEC]. Mapping/mode changes between fills
//! crossfade over 240 samples (5 ms) to suppress clicks.

use crate::pyramid::SAMPLE_RATE;
use crate::session::{DriftMode, Session};
use crate::track::TrackData;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

pub const CROSSFADE_SAMPLES: usize = 240; // 5 ms at 48 kHz [SPEC §4.8]
const TARGET_DBFS: f64 = -20.0; // loudness-matching target [SPEC, Appendix A]
const GAIN_CLAMP: f64 = 24.0; // ≤ +24 dB
const SINC_TAPS: usize = 32;
const SINC_PHASES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorMode {
    #[serde(rename = "pan")]
    PanLR,
    Overlay,
    /// Difference monitor (cam − ext). Dropped from the shell UI; kept in the
    /// core as the exact-cancellation oracle for the §4.8 precision tests.
    Null,
}

impl MonitorMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => MonitorMode::Overlay,
            2 => MonitorMode::Null,
            _ => MonitorMode::PanLR,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            MonitorMode::PanLR => 0,
            MonitorMode::Overlay => 1,
            MonitorMode::Null => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillInfo {
    pub frames: u32,
    /// Video time at the END of the filled block (the advancing cursor).
    pub stream_t: f64,
}

/// Polyphase 32-tap Kaiser windowed-sinc table (audition grade, §4.8).
/// `SINC_PHASES` fractional phases × `SINC_TAPS` taps, each phase normalized
/// to unit DC gain.
fn sinc_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let beta = 8.6f64;
        let cutoff = 0.92f64;
        let half = SINC_TAPS as f64 / 2.0;
        let i0_beta = bessel_i0(beta);
        let mut t = vec![0f32; SINC_PHASES * SINC_TAPS];
        for p in 0..SINC_PHASES {
            let frac = p as f64 / SINC_PHASES as f64;
            let mut sum = 0f64;
            for k in 0..SINC_TAPS {
                // Tap k weights source sample at relative index (k − 15) − frac.
                let x = (k as f64 - (SINC_TAPS as f64 / 2.0 - 1.0)) - frac;
                let sinc = if x == 0.0 {
                    cutoff
                } else {
                    (std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
                };
                let w_arg = x / half;
                let window = if w_arg.abs() <= 1.0 {
                    bessel_i0(beta * (1.0 - w_arg * w_arg).sqrt()) / i0_beta
                } else {
                    0.0
                };
                let h = sinc * window;
                t[p * SINC_TAPS + k] = h as f32;
                sum += h;
            }
            // Unit DC gain per phase (kills inter-phase amplitude ripple).
            if sum.abs() > 1e-12 {
                for k in 0..SINC_TAPS {
                    t[p * SINC_TAPS + k] /= sum as f32;
                }
            }
        }
        t
    })
}

fn bessel_i0(x: f64) -> f64 {
    // Series expansion, converges quickly for the β range used here.
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..32 {
        term *= (x / (2.0 * k as f64)) * (x / (2.0 * k as f64));
        sum += term;
        if term < 1e-16 * sum {
            break;
        }
    }
    sum
}

/// Snapshot of everything that affects rendering, taken once per fill.
#[derive(Clone, Copy, PartialEq)]
struct RenderParams {
    offset_ms: f64,
    drift_ppm: f64, // effective: committed when resample, 0 when none
    integer_shift: bool,
    monitor: MonitorMode,
    /// User monitor gains (volume/mute), on top of the loudness gains.
    gain_cam: f32,
    gain_ext: f32,
}

pub struct AuditionStream {
    pub id: u64,
    session: Arc<Session>,
    cam: Arc<TrackData>,
    ext: Arc<TrackData>,
    monitor: AtomicU8,
    /// Output cursor, frames on the video timeline (i64 stored as u64 bits).
    cursor: AtomicU64,
    video_end_frame: i64,
    g_cam: f32,
    g_ext: f32,
    /// User monitor gains (f32 bits): live per-track volume/mute, applied on
    /// top of the loudness-matching gains. Changes crossfade like any other
    /// param change.
    user_gain_cam_bits: AtomicU32,
    user_gain_ext_bits: AtomicU32,
    // Previous fill's params for click-free crossfades (fill-thread only).
    prev_offset_bits: AtomicU64,
    prev_drift_bits: AtomicU64,
    prev_gain_cam_bits: AtomicU32,
    prev_gain_ext_bits: AtomicU32,
    prev_flags: AtomicU8, // monitor(2 bits) | integer_shift(bit 2) | valid(bit 7)
}

impl AuditionStream {
    pub fn open(
        id: u64,
        session: Arc<Session>,
        cam: Arc<TrackData>,
        ext: Arc<TrackData>,
        mode: MonitorMode,
        t_start: f64,
    ) -> Self {
        // Loudness gains over the overlap region (F7: all three modes).
        let cam_dur = cam.meta.duration_s;
        let ext_dur = ext.meta.duration_s;
        let rate = 1.0 + session.drift_ppm() * 1e-6;
        let to_t = |u: f64| (u - session.offset_ms() / 1000.0) / rate;
        let t0 = to_t(0.0).max(0.0);
        let t1 = to_t(ext_dur).min(cam_dur);
        let (r0, r1) = if t1 > t0 + 1.0 { (t0, t1) } else { (0.0, cam_dur) };
        // A ≤ 120 s window at the overlap midpoint is representative; scanning
        // the entire multi-GB PCM cache for a loudness gain is wasteful.
        let mid = (r0 + r1) / 2.0;
        let half = ((r1 - r0) / 2.0).min(60.0);
        let (s0, s1) = (mid - half, mid + half);
        let gain_for = |rms: f64| -> f32 {
            if !rms.is_finite() {
                return 1.0;
            }
            10f64.powf(((TARGET_DBFS - rms) / 20.0).min(GAIN_CLAMP / 20.0)) as f32
        };
        let g_cam = gain_for(cam.rms(s0, s1).unwrap_or(f64::NEG_INFINITY));
        let u0 = session.u_of_t(s0).max(0.0);
        let u1 = session.u_of_t(s1).min(ext_dur).max(u0 + 0.5);
        let g_ext = gain_for(ext.rms(u0, u1).unwrap_or(f64::NEG_INFINITY));

        let video_end_frame = (cam_dur * SAMPLE_RATE as f64).round() as i64;
        let start = ((t_start.max(0.0) * SAMPLE_RATE as f64).round() as i64).min(video_end_frame);
        Self {
            id,
            session,
            cam,
            ext,
            monitor: AtomicU8::new(mode.as_u8()),
            cursor: AtomicU64::new(start as u64),
            video_end_frame,
            g_cam,
            g_ext,
            user_gain_cam_bits: AtomicU32::new(1f32.to_bits()),
            user_gain_ext_bits: AtomicU32::new(1f32.to_bits()),
            prev_offset_bits: AtomicU64::new(0),
            prev_drift_bits: AtomicU64::new(0),
            prev_gain_cam_bits: AtomicU32::new(0),
            prev_gain_ext_bits: AtomicU32::new(0),
            prev_flags: AtomicU8::new(0), // valid bit clear → no crossfade on first fill
        }
    }

    pub fn seek(&self, t: f64) {
        let f = ((t.max(0.0) * SAMPLE_RATE as f64).round() as i64).min(self.video_end_frame);
        self.cursor.store(f as u64, Ordering::Release);
        self.prev_flags.store(0, Ordering::Release); // no crossfade across a seek
    }

    pub fn set_mode(&self, mode: MonitorMode) {
        self.monitor.store(mode.as_u8(), Ordering::Release);
    }

    /// Per-track monitor gains (volume; 0 = mute), multiplied onto the
    /// loudness-matching gains. Takes effect on the next fill, click-free.
    pub fn set_gains(&self, cam: f32, ext: f32) {
        self.user_gain_cam_bits.store(cam.max(0.0).to_bits(), Ordering::Release);
        self.user_gain_ext_bits.store(ext.max(0.0).to_bits(), Ordering::Release);
    }

    fn current_params(&self) -> RenderParams {
        let mode_none = self.session.drift_mode() == DriftMode::None;
        RenderParams {
            offset_ms: self.session.offset_ms(),
            drift_ppm: if mode_none { 0.0 } else { self.session.committed_drift_ppm() },
            integer_shift: mode_none,
            monitor: MonitorMode::from_u8(self.monitor.load(Ordering::Acquire)),
            gain_cam: f32::from_bits(self.user_gain_cam_bits.load(Ordering::Acquire)),
            gain_ext: f32::from_bits(self.user_gain_ext_bits.load(Ordering::Acquire)),
        }
    }

    /// Fill `out` (interleaved stereo f32, 48 kHz). Real-time path.
    pub fn fill(&self, out: &mut [f32]) -> FillInfo {
        let want = out.len() / 2;
        let cursor = self.cursor.load(Ordering::Acquire) as i64;
        let n = (self.video_end_frame - cursor).max(0).min(want as i64) as usize;
        let params = self.current_params();

        self.render(&params, cursor, &mut out[..n * 2]);

        // Crossfade from the previous params when anything changed mid-stream.
        let prev_flags = self.prev_flags.load(Ordering::Acquire);
        if prev_flags & 0x80 != 0 {
            let prev = RenderParams {
                offset_ms: f64::from_bits(self.prev_offset_bits.load(Ordering::Acquire)),
                drift_ppm: f64::from_bits(self.prev_drift_bits.load(Ordering::Acquire)),
                integer_shift: prev_flags & 0x04 != 0,
                monitor: MonitorMode::from_u8(prev_flags & 0x03),
                gain_cam: f32::from_bits(self.prev_gain_cam_bits.load(Ordering::Acquire)),
                gain_ext: f32::from_bits(self.prev_gain_ext_bits.load(Ordering::Acquire)),
            };
            if prev != params && n > 0 {
                let cf = CROSSFADE_SAMPLES.min(n);
                let mut old = [0f32; CROSSFADE_SAMPLES * 2];
                self.render(&prev, cursor, &mut old[..cf * 2]);
                for i in 0..cf {
                    let w_new = (i as f32 + 0.5) / cf as f32;
                    let w_old = 1.0 - w_new;
                    out[2 * i] = out[2 * i] * w_new + old[2 * i] * w_old;
                    out[2 * i + 1] = out[2 * i + 1] * w_new + old[2 * i + 1] * w_old;
                }
            }
        }
        self.prev_offset_bits.store(params.offset_ms.to_bits(), Ordering::Release);
        self.prev_drift_bits.store(params.drift_ppm.to_bits(), Ordering::Release);
        self.prev_gain_cam_bits.store(params.gain_cam.to_bits(), Ordering::Release);
        self.prev_gain_ext_bits.store(params.gain_ext.to_bits(), Ordering::Release);
        self.prev_flags.store(
            0x80 | params.monitor.as_u8() | if params.integer_shift { 0x04 } else { 0 },
            Ordering::Release,
        );

        // Zeros + frames < requested only past the video end (§4.8).
        for v in &mut out[n * 2..] {
            *v = 0.0;
        }
        let new_cursor = cursor + n as i64;
        self.cursor.store(new_cursor as u64, Ordering::Release);
        FillInfo {
            frames: n as u32,
            stream_t: new_cursor as f64 / SAMPLE_RATE as f64,
        }
    }

    fn render(&self, p: &RenderParams, cursor: i64, out: &mut [f32]) {
        let n = out.len() / 2;
        let cam = self.cam.pcm_samples();
        let ext = self.ext.pcm_samples();
        let sr = SAMPLE_RATE as f64;
        let rate = 1.0 + p.drift_ppm * 1e-6;
        let off_samples = p.offset_ms / 1000.0 * sr;
        let int_shift = off_samples.round() as i64;
        let table = sinc_table();
        let gc = self.g_cam * p.gain_cam;
        let ge = self.g_ext * p.gain_ext;

        for i in 0..n {
            let frame = cursor + i as i64;
            let c = sample_at(cam, frame);
            let e = if p.integer_shift {
                sample_at(ext, frame + int_shift)
            } else {
                // u(t)·sr = frame·rate + off_samples, evaluated by windowed sinc.
                sinc_at(ext, frame as f64 * rate + off_samples, table)
            };
            let (l, r) = match p.monitor {
                MonitorMode::PanLR => (gc * c, ge * e),
                MonitorMode::Overlay => {
                    let m = 0.5 * (gc * c + ge * e);
                    (m, m)
                }
                MonitorMode::Null => {
                    let m = 0.5 * (gc * c - ge * e);
                    (m, m)
                }
            };
            out[2 * i] = l.clamp(-1.0, 1.0);
            out[2 * i + 1] = r.clamp(-1.0, 1.0);
        }
    }
}

#[inline]
fn sample_at(s: &[f32], idx: i64) -> f32 {
    if idx >= 0 && (idx as usize) < s.len() {
        s[idx as usize]
    } else {
        0.0
    }
}

/// 32-tap windowed-sinc interpolation at fractional sample position `pos`.
#[inline]
fn sinc_at(s: &[f32], pos: f64, table: &[f32]) -> f32 {
    let base = pos.floor() as i64;
    let frac = pos - base as f64;
    let phase = ((frac * SINC_PHASES as f64) as usize).min(SINC_PHASES - 1);
    let taps = &table[phase * SINC_TAPS..(phase + 1) * SINC_TAPS];
    let first = base - (SINC_TAPS as i64 / 2 - 1);
    let mut acc = 0f32;
    if first >= 0 && (first + SINC_TAPS as i64) <= s.len() as i64 {
        let src = &s[first as usize..first as usize + SINC_TAPS];
        for k in 0..SINC_TAPS {
            acc += src[k] * taps[k];
        }
    } else {
        for (k, &t) in taps.iter().enumerate() {
            acc += sample_at(s, first + k as i64) * t;
        }
    }
    acc
}
