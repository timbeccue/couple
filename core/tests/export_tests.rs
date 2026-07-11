//! Phase 4 gates (§9.7): round-trip keystone, None-mode bit-exactness, §8.2 edge
//! cases 1–6, second-track presence, container matrix, cancellation litter.

mod common;

use couple_core::export::*;
use couple_core::fixture::{generate, FixtureSpec};
use couple_core::job::{noop_progress, CancelToken, ProgressEmitter};
use couple_core::TrackRole;
use std::path::{Path, PathBuf};
use std::process::Command;

fn ffmpeg() -> PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).unwrap()
}
fn ffprobe() -> PathBuf {
    couple_core::ffmpeg::resolve_tool("ffprobe", None).unwrap()
}


/// Byte-slice equality without Debug-dumping megabytes on failure.
fn assert_bytes_eq(got: &[u8], want: &[u8], label: &str) {
    assert_eq!(got.len(), want.len(), "{label}: length {} vs {}", got.len(), want.len());
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!("{label}: first mismatch at byte {i} ({} vs {})", got[i], want[i]);
    }
}

fn wav_opts(out: &Path) -> ExportOptions {
    ExportOptions {
        kind: ExportKind::Wav,
        output_path: out.to_path_buf(),
        keep_camera_audio: false,
        container: Container::Auto,
        faststart: false,
        overwrite: true,
        allow_silence: false,
    }
}

fn run_export(
    video: &Path,
    audio: &Path,
    mapping: &ExportMapping,
    opts: &ExportOptions,
) -> couple_core::Result<ExportReport> {
    let mut prog = ProgressEmitter::new(0, noop_progress());
    export(
        &ffmpeg(),
        &ffprobe(),
        video,
        audio,
        mapping,
        opts,
        &mut prog,
        &CancelToken::new(),
    )
}

/// Decode any file's first audio stream to raw bytes in the given packed format.
fn extract_raw(path: &Path, fmt: &str) -> Vec<u8> {
    let out = Command::new(ffmpeg())
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "a:0", "-f", fmt, "pipe:1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "extract_raw failed: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// Wrap a mono WAV into a .mov with mpeg4 testsrc2 video + pcm_s24le audio (the
/// §9.7 PCM-in-MOV fixture; F12: no x264 needed).
fn make_mov(dir: &Path, wav: &Path, duration_s: f64) -> PathBuf {
    let mov = dir.join("camera.mov");
    let st = Command::new(ffmpeg())
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(wav)
        .args(["-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size=192x108:rate=30:duration={duration_s}"))
        .args(["-map", "1:v:0", "-map", "0:a:0", "-c:v", "mpeg4", "-q:v", "12"])
        .args(["-c:a", "pcm_s24le", "-shortest"])
        .arg(&mov)
        .status()
        .unwrap();
    assert!(st.success());
    mov
}

// ---------- round-trip keystone (§9.7) ----------

#[test]
fn round_trip_resample_mode() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -5000.0,
        ppm: 30.0,
        dur_cam_s: 300.0,
        dur_ext_s: 340.0,
        snr_db: Some(15.0),
        seed: 401,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let cam_wav = dir.path().join("camera.wav");
    let ext_wav = dir.path().join("external.wav");

    // Measure the mapping with the engine (as the app would), then export with it.
    let ctx = common::test_ctx(&dir.path().join("cache"));
    let cam_id = common::import(&ctx, &cam_wav, TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &ext_wav, TrackRole::External).unwrap();
    let sid = ctx.session_new(cam_id, ext_id).unwrap();
    let r = ctx
        .auto_sync_blocking(
            sid,
            &couple_core::sync::SyncParams::default(),
            None,
            noop_progress(),
            &CancelToken::new(),
            0,
        )
        .unwrap();
    assert!(matches!(r.status, couple_core::sync::SyncStatus::Synced));

    let out = dir.path().join("aligned.wav");
    let mapping = ExportMapping {
        offset_ms: r.offset_ms,
        drift_ppm: r.drift_ppm,
        resample: true,
    };
    let report = run_export(&cam_wav, &ext_wav, &mapping, &wav_opts(&out)).unwrap();
    assert!(report.resampled);
    assert_eq!(report.frames, (300.0f64 * 48_000.0).round() as u64);

    // Re-import the result against the original external: the measured mapping must
    // reproduce the session mapping (≤ 0.25 ms / ≤ 0.3 ppm).
    let res_id = common::import(&ctx, &out, TrackRole::Camera).unwrap();
    let res = ctx.track(res_id).unwrap();
    let ext = ctx.track(ext_id).unwrap();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let rt = couple_core::sync::auto_sync_tracks(
        &res.pyramid,
        &ext.pyramid,
        &couple_core::sync::SyncParams::default(),
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    assert!(
        (rt.offset_ms - r.offset_ms).abs() <= 0.25,
        "offset {} vs applied {}",
        rt.offset_ms,
        r.offset_ms
    );
    assert!(
        (rt.drift_ppm - r.drift_ppm).abs() <= 0.3,
        "drift {} vs applied {}",
        rt.drift_ppm,
        r.drift_ppm
    );

    // And against the camera: the rendered track must be IN sync (≈ 0 / ≈ 0).
    let cam = ctx.track(cam_id).unwrap();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let zero = couple_core::sync::auto_sync_tracks(
        &cam.pyramid,
        &res.pyramid,
        &couple_core::sync::SyncParams::default(),
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    assert!(zero.offset_ms.abs() <= 0.25, "residual offset {}", zero.offset_ms);
    assert!(zero.drift_ppm.abs() <= 0.3, "residual drift {}", zero.drift_ppm);
}

// ---------- None-mode bit-exactness through the mux (§9.7) ----------

#[test]
fn none_mode_mux_is_bit_exact() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -2500.0, // external starts 2.5 s into the video
        dur_cam_s: 30.0,
        dur_ext_s: 40.0,
        snr_db: Some(20.0),
        seed: 402,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let mov = make_mov(dir.path(), &dir.path().join("camera.wav"), 30.0);

    let out = dir.path().join("out.mov");
    let mapping = ExportMapping {
        offset_ms: -2500.0,
        drift_ppm: 0.0,
        resample: false,
    };
    let opts = ExportOptions {
        kind: ExportKind::Mux,
        output_path: out.clone(),
        keep_camera_audio: false,
        container: Container::Auto,
        faststart: false,
        overwrite: true,
        allow_silence: false,
    };
    let report = run_export(&mov, &dir.path().join("external.wav"), &mapping, &opts).unwrap();
    assert_eq!(report.container, "mov");
    assert_eq!(report.audio_codec, "pcm_s24le");

    // Video must be stream-copied untouched.
    let probe = couple_core::ffmpeg::probe(&ffprobe(), &out).unwrap();
    let v = probe.video.as_ref().unwrap();
    assert_eq!(v.codec, "mpeg4");

    // Output audio = external shifted by S = round(−2500 ms · 48 kHz) = −120000
    // frames (silence head), bit-exact over the overlap (§8.2 None mode, F8).
    let got = extract_raw(&out, "s24le");
    let src = extract_raw(&dir.path().join("external.wav"), "s24le");
    let bpf = 3; // mono s24le
    let head = 120_000 * bpf;
    assert!(got[..head].iter().all(|&b| b == 0), "head must be digital silence");
    let n_out_bytes = got.len();
    let overlap_bytes = n_out_bytes - head; // external covers video to its end
    assert_bytes_eq(&got[head..], &src[..overlap_bytes], "overlap not bit-exact");
    // §8.2 case 6: within the external's span there is never synthesized silence —
    // byte equality above proves it (fixture silences are the source's own).
}

// ---------- §8.2 edge cases ----------

#[test]
fn edge_cases_head_trim_tail_and_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 0.0,
        dur_cam_s: 20.0,
        dur_ext_s: 12.0, // ends before the video → silence tail (case 3)
        snr_db: Some(20.0),
        seed: 403,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let ext = dir.path().join("external.wav");
    let cam = dir.path().join("camera.wav");
    let src = extract_raw(&ext, "s24le");
    let bpf = 3;

    // Case 2: offset > 0 → head-trimmed: output[0] == source frame S.
    let out = dir.path().join("trim.wav");
    let s_frames = (3.5f64 * 48_000.0).round() as usize;
    run_export(
        &cam,
        &ext,
        &ExportMapping {
            offset_ms: 3500.0,
            drift_ppm: 0.0,
            resample: false,
        },
        &wav_opts(&out),
    )
    .unwrap();
    let got = extract_raw(&out, "s24le");
    assert_eq!(got.len(), 20 * 48_000 * bpf);
    let content = (12.0f64 * 48_000.0) as usize - s_frames; // remaining source frames
    assert_bytes_eq(&got[..content * bpf], &src[s_frames * bpf..], "head-trimmed content");
    // case 3: silence tail after the external ends
    assert!(got[content * bpf..].iter().all(|&b| b == 0), "tail must be silence");

    // Case 1: offset < 0 → silence head until t = −off/1000.
    let out = dir.path().join("head.wav");
    run_export(
        &cam,
        &ext,
        &ExportMapping {
            offset_ms: -4000.0,
            drift_ppm: 0.0,
            resample: false,
        },
        &wav_opts(&out),
    )
    .unwrap();
    let got = extract_raw(&out, "s24le");
    let head = 4 * 48_000 * bpf;
    assert!(got[..head].iter().all(|&b| b == 0));
    let content = 12 * 48_000 * bpf; // full external fits (4 + 12 ≤ 20)
    assert_bytes_eq(&got[head..head + content], &src[..content], "shifted content");

    // Case 5: offset pushes overlap to zero → refuse (ExportFailed) …
    let no_overlap = ExportMapping {
        offset_ms: -25_000.0,
        drift_ppm: 0.0,
        resample: false,
    };
    let err = run_export(&cam, &ext, &no_overlap, &wav_opts(&dir.path().join("no.wav"))).unwrap_err();
    assert!(
        matches!(err, couple_core::CoupleError::ExportFailed { ref stage, .. } if stage == "render"),
        "{err:?}"
    );
    // … unless --allow-silence (CLI flag): all-silence output of exactly video length.
    let out = dir.path().join("silence.wav");
    let mut opts = wav_opts(&out);
    opts.allow_silence = true;
    run_export(&cam, &ext, &no_overlap, &opts).unwrap();
    let got = extract_raw(&out, "s24le");
    assert_eq!(got.len(), 20 * 48_000 * bpf);
    assert!(got.iter().all(|&b| b == 0));
}

/// Case 4: external longer than the video → trimmed at the video duration (this is
/// also the §8.2 deadlock trap: the decoder must be killed, not awaited).
#[test]
fn edge_case_external_longer_resample() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 1000.0,
        ppm: 25.0,
        dur_cam_s: 15.0,
        dur_ext_s: 60.0,
        snr_db: Some(20.0),
        seed: 404,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let out = dir.path().join("trimmed.wav");
    let report = run_export(
        &dir.path().join("camera.wav"),
        &dir.path().join("external.wav"),
        &ExportMapping {
            offset_ms: 1000.0,
            drift_ppm: 25.0,
            resample: true,
        },
        &wav_opts(&out),
    )
    .unwrap();
    assert_eq!(report.frames, 15 * 48_000);
    let probe = couple_core::ffmpeg::probe(&ffprobe(), &out).unwrap();
    assert!((probe.duration_s - 15.0).abs() < 0.01);
}

// ---------- second track, container matrix ----------

#[test]
fn keep_camera_audio_second_track() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: -1000.0,
        dur_cam_s: 12.0,
        dur_ext_s: 15.0,
        snr_db: Some(20.0),
        seed: 405,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let mov = make_mov(dir.path(), &dir.path().join("camera.wav"), 12.0);
    let out = dir.path().join("two_tracks.mov");
    let opts = ExportOptions {
        kind: ExportKind::Mux,
        output_path: out.clone(),
        keep_camera_audio: true,
        container: Container::Auto,
        faststart: false,
        overwrite: true,
        allow_silence: false,
    };
    let report = run_export(
        &mov,
        &dir.path().join("external.wav"),
        &ExportMapping {
            offset_ms: -1000.0,
            drift_ppm: 0.0,
            resample: false,
        },
        &opts,
    )
    .unwrap();
    assert!(report.kept_camera_audio);

    // ffprobe conformance: 2 audio streams; a:0 default, a:1 non-default + stream-copied.
    let raw = Command::new(ffprobe())
        .args(["-v", "error", "-print_format", "json", "-show_streams"])
        .arg(&out)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&raw.stdout).unwrap();
    let audio: Vec<&serde_json::Value> = v["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0]["codec_name"], "pcm_s24le");
    assert_eq!(audio[0]["disposition"]["default"], 1);
    assert_eq!(audio[1]["codec_name"], "pcm_s24le"); // camera.mov's audio, copied
    assert_eq!(audio[1]["disposition"]["default"], 0);
}

#[test]
fn container_matrix_mp4_gets_aac() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 0.0,
        dur_cam_s: 10.0,
        dur_ext_s: 12.0,
        snr_db: Some(20.0),
        with_video: true, // → camera.mp4 (isom brand)
        seed: 406,
        ..Default::default()
    };
    let truth = generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let out = dir.path().join("out.mp4");
    let opts = ExportOptions {
        kind: ExportKind::Mux,
        output_path: out.clone(),
        keep_camera_audio: false,
        container: Container::Auto, // must resolve to mp4 from the input family
        faststart: true,            // exercise +faststart
        overwrite: true,
        allow_silence: false,
    };
    let report = run_export(
        &dir.path().join(&truth.camera_file),
        &dir.path().join("external.wav"),
        &ExportMapping {
            offset_ms: 0.0,
            drift_ppm: 0.0,
            resample: false,
        },
        &opts,
    )
    .unwrap();
    assert_eq!(report.container, "mp4");
    assert!(report.audio_codec.starts_with("aac"));
    let probe = couple_core::ffmpeg::probe(&ffprobe(), &out).unwrap();
    assert_eq!(probe.audio[0].codec, "aac");
    assert_eq!(probe.video.as_ref().unwrap().codec, "mpeg4"); // stream copy
}

// ---------- cancellation litter (§9.7) ----------

#[test]
fn cancel_mid_export_leaves_no_litter() {
    let dir = tempfile::tempdir().unwrap();
    let spec = FixtureSpec {
        offset_ms: 0.0,
        ppm: 20.0,
        dur_cam_s: 120.0,
        dur_ext_s: 130.0,
        snr_db: Some(20.0),
        seed: 407,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    let out = dir.path().join("cancelled.wav");
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let err = export(
        &ffmpeg(),
        &ffprobe(),
        &dir.path().join("camera.wav"),
        &dir.path().join("external.wav"),
        &ExportMapping {
            offset_ms: 0.0,
            drift_ppm: 20.0,
            resample: true,
        },
        &wav_opts(&out),
        &mut prog,
        &cancel,
    )
    .unwrap_err();
    assert!(matches!(err, couple_core::CoupleError::Cancelled));
    assert!(!out.exists());
    let litter: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(litter.is_empty(), "temp litter: {litter:?}");
}
