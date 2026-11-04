//! Audition engine gates (§4.8): matrix correctness, offset semantics,
//! crossfade behavior, end-of-video contract, and the §9.6 fill budget.

use couple_core::audition::{FillInfo, MonitorMode, CROSSFADE_SAMPLES};
use couple_core::job::CancelToken;
use couple_core::{Ctx, CtxConfig, TrackRole};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn temp_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("couple-audition-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn gen_wav(dir: &PathBuf, name: &str, filt: &str, dur: f64) -> PathBuf {
    let out = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("{filt}:duration={dur}:sample_rate=48000"))
        .args(["-ac", "1", out.to_str().unwrap()])
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    out
}

struct Rig {
    ctx: Arc<Ctx>,
    session: u64,
}

/// Same content as both tracks (camera = external file) → null mode can cancel
/// exactly in integer-shift mode.
fn rig() -> Rig {
    let dir = temp_dir();
    let ctx = Ctx::new(CtxConfig {
        cache_dir: Some(dir.join("cache")),
        ..Default::default()
    });
    let wav = gen_wav(&dir, "content.wav", "anoisesrc=c=pink:a=0.5:seed=11", 20.0);
    let nul = couple_core::job::ProgressFn::from(Arc::new(|_: &couple_core::Progress| {})
        as Arc<dyn Fn(&couple_core::Progress) + Send + Sync>);
    let cam = ctx
        .import_blocking(&wav, TrackRole::Camera, nul.clone(), &CancelToken::new(), 0)
        .unwrap();
    let ext = ctx
        .import_blocking(&wav, TrackRole::External, nul, &CancelToken::new(), 0)
        .unwrap();
    let session = ctx.session_new(cam, ext).unwrap();
    Rig { ctx, session }
}

fn fill(ctx: &Ctx, stream: u64, frames: usize) -> (Vec<f32>, FillInfo) {
    let mut buf = vec![0f32; frames * 2];
    let info = ctx.audition_stream(stream).unwrap().fill(&mut buf);
    (buf, info)
}

#[test]
fn matrices_offsets_end_and_crossfade() {
    let r = rig();
    let ctx = &r.ctx;
    let s = ctx.session(r.session).unwrap();

    // --- Null cancels identical content exactly (offset 0, None mode = integer path).
    s.set_drift_mode(couple_core::session::DriftMode::None);
    let st = ctx.audition_open(r.session, MonitorMode::Null, 1.0).unwrap();
    let (out, info) = fill(ctx, st, 4800);
    assert_eq!(info.frames, 4800);
    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(peak == 0.0, "null of identical content must be exact zero, got {peak}");

    // --- Pan: L = g·cam slice exactly; R = g·ext (same here).
    ctx.audition_stream(st).unwrap().set_mode(MonitorMode::PanLR);
    // One fill to absorb the mode crossfade, then check exactness.
    let _ = fill(ctx, st, 512);
    let cam = ctx.track(s.camera).unwrap();
    let t_before = {
        let (_, i) = fill(ctx, st, 0); // cursor probe (0-frame fill)
        i.stream_t
    };
    let (out, _) = fill(ctx, st, 2400);
    let mut raw = vec![0f32; 2400];
    cam.raw_window(t_before, t_before + 0.05, &mut raw).unwrap();
    let g = out[0] / raw[0]; // loudness gain (equal for both tracks here)
    assert!(g.is_finite() && g > 0.0);
    for i in 0..2400 {
        assert!(
            (out[2 * i] - g * raw[i]).abs() < 1e-6,
            "pan L must be g·camera at sample {i}"
        );
        assert!((out[2 * i + 1] - out[2 * i]).abs() < 1e-6, "same content ⇒ L == R");
    }

    // --- Integer offset shift: +100 ms offset reads external 100 ms ahead.
    s.set_offset_ms(100.0);
    let st2 = ctx.audition_open(r.session, MonitorMode::PanLR, 2.0).unwrap();
    let (out, _) = fill(ctx, st2, 2400);
    let mut cam_w = vec![0f32; 2400];
    let mut ext_w = vec![0f32; 2400];
    cam.raw_window(2.0, 2.05, &mut cam_w).unwrap();
    ctx.track(s.external).unwrap().raw_window(2.1, 2.15, &mut ext_w).unwrap();
    let gl = out[0] / cam_w[0];
    let gr = out[1] / ext_w[0];
    for i in 0..2400 {
        assert!((out[2 * i] - gl * cam_w[i]).abs() < 1e-6);
        assert!((out[2 * i + 1] - gr * ext_w[i]).abs() < 1e-6, "ext must be read at u(t)=t+0.1");
    }
    ctx.audition_close(st2);
    s.set_offset_ms(0.0);

    // --- Resample path (drift committed) ≈ content shifted continuously; sanity:
    // 0 ppm committed drift through the sinc path reproduces the signal closely.
    s.set_drift_mode(couple_core::session::DriftMode::Resample);
    s.set_drift_ppm_live(0.0);
    s.commit_drift();
    let st3 = ctx.audition_open(r.session, MonitorMode::PanLR, 3.0).unwrap();
    let _ = fill(ctx, st3, 512); // settle crossfade state
    let probe = fill(ctx, st3, 0).1.stream_t;
    let (out, _) = fill(ctx, st3, 2400);
    let mut ext_raw = vec![0f32; 2400];
    ctx.track(s.external).unwrap().raw_window(probe, probe + 0.05, &mut ext_raw).unwrap();
    let g = out[1] / ext_raw[0];
    // The sinc path is a 0.92-cutoff lowpass (anti-aliasing), so broadband
    // pink noise loses a little near-Nyquist energy — audition-grade bound.
    let mut err = 0f64;
    for i in 16..2384 {
        err = err.max((out[2 * i + 1] as f64 - (g * ext_raw[i]) as f64).abs());
    }
    assert!(err < 0.06, "0-ppm sinc path should be near-transparent, max err {err}");
    ctx.audition_close(st3);

    // --- Crossfade: switching pan → null on identical content ramps to zero
    // across CROSSFADE_SAMPLES instead of jumping.
    let st4 = ctx.audition_open(r.session, MonitorMode::PanLR, 4.0).unwrap();
    s.set_drift_mode(couple_core::session::DriftMode::None);
    let _ = fill(ctx, st4, 512);
    ctx.audition_stream(st4).unwrap().set_mode(MonitorMode::Null);
    let (out, _) = fill(ctx, st4, 1024);
    let head = out[..8].iter().fold(0f32, |m, v| m.max(v.abs()));
    let tail = out[CROSSFADE_SAMPLES * 2..CROSSFADE_SAMPLES * 2 + 200]
        .iter()
        .fold(0f32, |m, v| m.max(v.abs()));
    assert!(head > 0.0, "crossfade start must still carry the old (pan) signal");
    assert!(tail == 0.0, "past the crossfade, null of identical content is zero");
    ctx.audition_close(st4);

    // --- User gains: pan with (2.0, 0.5) on identical content ⇒ L = 4·R once
    // the gain crossfade settles; (0, 1) mutes the camera leg exactly.
    let stg = ctx.audition_open(r.session, MonitorMode::PanLR, 5.0).unwrap();
    ctx.audition_stream(stg).unwrap().set_gains(2.0, 0.5);
    let _ = fill(ctx, stg, 512); // absorb the gain crossfade
    let (out, _) = fill(ctx, stg, 2400);
    for i in 0..2400 {
        assert!(
            (out[2 * i] - 4.0 * out[2 * i + 1]).abs() < 1e-5,
            "gains (2.0, 0.5) on identical content ⇒ L = 4·R at sample {i}"
        );
    }
    ctx.audition_stream(stg).unwrap().set_gains(0.0, 1.0);
    let _ = fill(ctx, stg, 512);
    let (out, _) = fill(ctx, stg, 2400);
    assert!(out.iter().step_by(2).all(|&v| v == 0.0), "muted camera leg must be exact zero");
    assert!(out.iter().skip(1).step_by(2).any(|&v| v != 0.0), "ext leg must stay audible");
    ctx.audition_close(stg);

    // --- End of video: short fill + zeros, then 0 frames.
    let dur = cam.meta.duration_s;
    let st5 = ctx.audition_open(r.session, MonitorMode::Overlay, dur - 0.01).unwrap();
    let (out, info) = fill(ctx, st5, 1024);
    assert!(info.frames < 1024, "must truncate at video end");
    assert!((info.stream_t - dur).abs() < 1e-6);
    assert!(out[info.frames as usize * 2..].iter().all(|&v| v == 0.0));
    let (_, info2) = fill(ctx, st5, 1024);
    assert_eq!(info2.frames, 0, "past the end: zero frames");
    ctx.audition_close(st5);
    ctx.audition_close(st);
}

#[test]
fn fill_budget_512_frames_under_1ms() {
    let r = rig();
    let ctx = &r.ctx;
    let s = ctx.session(r.session).unwrap();
    // Worst case: resample (sinc) path.
    s.set_drift_ppm_live(12.0);
    s.commit_drift();
    let st = ctx.audition_open(r.session, MonitorMode::Overlay, 0.0).unwrap();
    let mut buf = vec![0f32; 512 * 2];
    let stream = ctx.audition_stream(st).unwrap();
    let _ = stream.fill(&mut buf); // warm (sinc table build)
    let mut times = Vec::with_capacity(200);
    for _ in 0..200 {
        let t0 = std::time::Instant::now();
        let _ = stream.fill(&mut buf);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    let max = *times.last().unwrap();
    println!("audition_fill 512: median {median:.4} ms, max {max:.4} ms");
    assert!(median < 1.0, "§9.6: fill 512 must be ≤ 1 ms (median {median:.3})");
}

/// A/B solos (UX-DECISIONS 2026-11-03): camera-only is the pan-mode LEFT channel in both ears,
/// external-only the pan-mode RIGHT channel in both ears; the user gains still apply.
#[test]
fn solo_modes_are_the_pan_channels_in_both_ears() {
    let r = rig();
    let ctx = &r.ctx;
    let s = ctx.session(r.session).unwrap();
    s.set_drift_mode(couple_core::session::DriftMode::None);
    let st = ctx.audition_open(r.session, MonitorMode::PanLR, 2.0).unwrap();
    let stream = ctx.audition_stream(st).unwrap();
    // Distinct gains so L and R differ even with identical content.
    stream.set_gains(0.5, 0.25);
    let _ = fill(ctx, st, 512); // absorb the gain crossfade
    let t0 = fill(ctx, st, 0).1.stream_t;
    let (pan, _) = fill(ctx, st, 2400);
    for mode in [MonitorMode::CameraOnly, MonitorMode::ExternalOnly] {
        stream.seek(t0);
        stream.set_mode(mode);
        let (solo, info) = fill(ctx, st, 2400);
        assert_eq!(info.frames, 2400);
        for i in 0..2400 {
            let want = if mode == MonitorMode::CameraOnly { pan[2 * i] } else { pan[2 * i + 1] };
            assert!((solo[2 * i] - want).abs() < 1e-6, "{mode:?} L at {i}");
            assert!((solo[2 * i + 1] - want).abs() < 1e-6, "{mode:?} R at {i}");
        }
        let peak = solo.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.0, "{mode:?} must not be silent");
    }
    assert_eq!(MonitorMode::from_u8(3), MonitorMode::CameraOnly);
    assert_eq!(MonitorMode::from_u8(4), MonitorMode::ExternalOnly);
    assert_eq!(serde_json::to_string(&MonitorMode::CameraOnly).unwrap(), "\"camera\"");
}
