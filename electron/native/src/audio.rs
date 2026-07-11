//! audio.rs — the audition output pump (PLAN-CORE-SHELL-BOUNDARY §4-G's shell
//! "audio callback"): a cpal output stream whose data callback pulls
//! `cpl_audition_fill` directly. No JS on the audio path; the renderer only
//! starts/stops the pump and polls `clock()` to slave the muted <video>.
//!
//! One pump at a time (the app auditions one couple at a time). All entry
//! points are called from the single JS thread; the cpal stream (not Send) is
//! kept thread-local to it. The device callback thread shares only atomics.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use couple as c;

struct Pump {
    _stream: cpal::Stream,
    /// f64 bits of the audition stream's video-time after the last fill.
    clock_bits: Arc<AtomicU64>,
    /// Count of callback passes where the core fill failed (device kept fed
    /// with silence, so this is observability, not a crash).
    underruns: Arc<AtomicU32>,
    /// Rough output latency (buffer duration): the clock leads the speaker by
    /// about this much; the shell subtracts it when slaving.
    latency_s: f64,
}

thread_local! {
    static PUMP: RefCell<Option<Pump>> = const { RefCell::new(None) };
}

const RATE: u32 = 48_000; // the core audition stream's fixed rate (§4.8)

pub fn start(ctx: u64, stream: u64) -> Result<(), String> {
    stop();

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;

    // Pick a stereo f32 config that supports 48 kHz (§4.8: the audition stream
    // is interleaved stereo f32 at 48 kHz; no shell-side resampling).
    let config_range = device
        .supported_output_configs()
        .map_err(|e| format!("query output configs: {e}"))?
        .filter(|cfg| cfg.sample_format() == cpal::SampleFormat::F32 && cfg.channels() >= 2)
        .find(|cfg| cfg.min_sample_rate().0 <= RATE && RATE <= cfg.max_sample_rate().0)
        .ok_or_else(|| "output device has no stereo f32 config at 48 kHz".to_string())?;
    let channels = config_range.channels() as usize;
    let config = cpal::StreamConfig {
        channels: config_range.channels(),
        sample_rate: cpal::SampleRate(RATE),
        buffer_size: cpal::BufferSize::Default,
    };

    let clock_bits = Arc::new(AtomicU64::new(f64::to_bits(f64::NAN)));
    let underruns = Arc::new(AtomicU32::new(0));
    let cb_clock = clock_bits.clone();
    let cb_underruns = underruns.clone();

    // Scratch for the stereo pull when the device wants >2 channels.
    let mut stereo: Vec<f32> = Vec::new();

    let data_cb = move |out: &mut [f32], _info: &cpal::OutputCallbackInfo| {
        let frames = out.len() / channels;
        let need = frames * 2;
        if stereo.len() < need {
            stereo.resize(need, 0.0); // grows once; steady-state allocation-free
        }
        let buf = &mut stereo[..need];
        let mut got: u32 = 0;
        let mut stream_t: f64 = f64::NAN;
        let rc = unsafe {
            c::cpl_audition_fill(ctx, stream, buf.as_mut_ptr(), frames as u32, &mut got, &mut stream_t)
        };
        if rc != 0 {
            buf.fill(0.0);
            got = 0;
            cb_underruns.fetch_add(1, Ordering::Relaxed);
        } else {
            // Past the video end the core writes fewer frames; pad silence.
            buf[(got as usize * 2)..].fill(0.0);
            cb_clock.store(f64::to_bits(stream_t), Ordering::Relaxed);
        }
        let _ = got;
        if channels == 2 {
            out.copy_from_slice(buf);
        } else {
            for f in 0..frames {
                out[f * channels] = buf[f * 2];
                out[f * channels + 1] = buf[f * 2 + 1];
                for ch in 2..channels {
                    out[f * channels + ch] = 0.0;
                }
            }
        }
    };

    let stream_handle = device
        .build_output_stream(&config, data_cb, |e| eprintln!("[couple-node audio] {e}"), None)
        .map_err(|e| format!("build output stream: {e}"))?;
    stream_handle.play().map_err(|e| format!("start output stream: {e}"))?;

    // cpal doesn't expose the resolved default buffer size portably; a couple
    // of 512-frame quanta is a workable slaving estimate (the HUD measures the
    // true skew — the budget is verified, not assumed).
    let latency_s = 1024.0 / RATE as f64;

    PUMP.with(|p| {
        *p.borrow_mut() = Some(Pump { _stream: stream_handle, clock_bits, underruns, latency_s });
    });
    Ok(())
}

pub fn stop() {
    PUMP.with(|p| {
        *p.borrow_mut() = None; // dropping the cpal stream stops the device
    });
}

/// The audition clock: the stream's video time minus the output-latency
/// estimate. NaN when the pump is stopped or has not filled yet.
pub fn clock() -> f64 {
    PUMP.with(|p| {
        p.borrow().as_ref().map_or(f64::NAN, |pump| {
            let t = f64::from_bits(pump.clock_bits.load(Ordering::Relaxed));
            if t.is_nan() { f64::NAN } else { t - pump.latency_s }
        })
    })
}

pub fn underruns() -> u32 {
    PUMP.with(|p| p.borrow().as_ref().map_or(0, |pump| pump.underruns.load(Ordering::Relaxed)))
}

pub fn running() -> bool {
    PUMP.with(|p| p.borrow().is_some())
}
