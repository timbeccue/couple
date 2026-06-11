//! Export pipeline (§8): aligned-audio render (None = bit-exact integer shift,
//! Resample = §8.2-quality fractional warp), mux with video stream-copy (§8.3/§8.4),
//! standalone WAV/RF64 (§8.5). Output timeline = video container time, duration
//! exactly the video duration. Source = the ORIGINAL external file at full fidelity —
//! the mono 48 kHz analysis cache is never used here.

use crate::cache;
use crate::error::{CoupleError, Result};
use crate::ffmpeg;
use crate::job::{CancelToken, Phase, ProgressEmitter};
use crate::resample::{SincInterp, EXPORT_BETA, EXPORT_TAPS};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportKind {
    Mux,
    Wav,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Auto,
    Mov,
    Mp4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub kind: ExportKind,
    pub output_path: PathBuf,
    /// §8.1 default off [DECIDED-pending §6.7].
    #[serde(default)]
    pub keep_camera_audio: bool,
    #[serde(default = "default_container")]
    pub container: Container,
    /// Off by default: +faststart rewrites a multi-GB file [SPEC].
    #[serde(default)]
    pub faststart: bool,
    #[serde(default)]
    pub overwrite: bool,
    /// CLI-only escape hatch for the §8.2 no-overlap refusal.
    #[serde(default)]
    pub allow_silence: bool,
}

fn default_container() -> Container {
    Container::Auto
}

/// The alignment applied at export time (from AlignmentState: driftMode "none" ⇒
/// resample=false and the live drift is already 0).
#[derive(Debug, Clone, Copy)]
pub struct ExportMapping {
    pub offset_ms: f64,
    pub drift_ppm: f64,
    pub resample: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub output_path: String,
    pub container: String,
    pub audio_codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub frames: u64,
    pub kept_camera_audio: bool,
    pub resampled: bool,
}

// ---------- source format ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireFmt {
    /// Integer path: decode/encode via s32le (exact for PCM ≤ 32-bit).
    I32,
    /// Float path (lossy/float sources).
    F32,
}

struct SourcePlan {
    sample_rate: u32,
    channels: u32,
    wire: WireFmt,
    /// effective source bit depth for the PCM output codec choice
    bits: u32,
    duration_s: f64,
}

fn plan_source(info: &crate::media::MediaInfo) -> SourcePlan {
    let a = &info.audio[0];
    let integer = a.codec.starts_with("pcm_s")
        || a.codec.starts_with("pcm_u")
        || a.codec == "flac"
        || a.codec == "alac";
    let bits = a.bits_per_sample.unwrap_or(if integer { 16 } else { 24 }).min(32);
    SourcePlan {
        sample_rate: a.sample_rate,
        channels: a.channels,
        wire: if integer && bits <= 32 { WireFmt::I32 } else { WireFmt::F32 },
        bits,
        duration_s: a.duration_s.unwrap_or(info.duration_s),
    }
}

/// §8.4 audio codec for PCM containers: s16 for ≤ 16-bit integer sources, else s24
/// (float/lossy → 24-bit per §8.5).
fn pcm_codec(plan: &SourcePlan) -> &'static str {
    if plan.wire == WireFmt::I32 && plan.bits <= 16 {
        "pcm_s16le"
    } else {
        "pcm_s24le"
    }
}

/// §8.4 container resolution: follow the input family (mov→mov, mp4/m4a/3gp→mp4),
/// brand first, extension as fallback, mov for anything else.
fn resolve_container(video_info: &crate::media::MediaInfo, opt: Container) -> &'static str {
    match opt {
        Container::Mov => "mov",
        Container::Mp4 => "mp4",
        Container::Auto => {
            if let Some(brand) = video_info.tags.get("major_brand") {
                let b = brand.trim();
                if b.starts_with("qt") {
                    return "mov";
                }
                if b.starts_with("isom") || b.starts_with("mp4") || b.starts_with("M4")
                    || b.starts_with("m4") || b.starts_with("3gp")
                {
                    return "mp4";
                }
            }
            match Path::new(&video_info.path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref()
            {
                Some("mp4") | Some("m4v") | Some("3gp") => "mp4",
                _ => "mov",
            }
        }
    }
}

// ---------- decode reader ----------

struct SourceReader {
    child: Child,
    stdout: std::process::ChildStdout,
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
    wire: WireFmt,
    channels: usize,
    carry: Vec<u8>,
    eof: bool,
}

impl SourceReader {
    fn spawn(ffmpeg_bin: &Path, path: &Path, plan: &SourcePlan) -> Result<Self> {
        let fmt = match plan.wire {
            WireFmt::I32 => "s32le",
            WireFmt::F32 => "f32le",
        };
        let mut cmd = Command::new(ffmpeg_bin);
        cmd.arg("-nostdin")
            .args(["-v", "error"])
            .arg("-i")
            .arg(path)
            .args(["-map", "a:0", "-f", fmt, "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| CoupleError::internal(format!("spawn export decode: {e}")))?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        Ok(Self {
            child,
            stdout,
            stderr_thread: Some(stderr_thread),
            wire: plan.wire,
            channels: plan.channels as usize,
            carry: Vec::new(),
            eof: false,
        })
    }

    /// Read up to max_frames interleaved frames as f64. Returns frames read (0 = EOF).
    fn read_frames(&mut self, out: &mut Vec<f64>, max_frames: usize) -> Result<usize> {
        out.clear();
        if self.eof {
            return Ok(0);
        }
        let frame_bytes = 4 * self.channels;
        let want_bytes = max_frames * frame_bytes;
        let mut bytes = std::mem::take(&mut self.carry);
        bytes.reserve(want_bytes);
        let mut chunk = vec![0u8; 1 << 20];
        while bytes.len() < want_bytes {
            let n = self
                .stdout
                .read(&mut chunk)
                .map_err(|e| CoupleError::internal(format!("export decode read: {e}")))?;
            if n == 0 {
                self.eof = true;
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
        }
        // Cap at the requested frame count — pipe reads arrive in larger chunks, and
        // callers do arithmetic on the returned count (skip loops would underflow).
        let whole = (bytes.len() / frame_bytes * frame_bytes).min(want_bytes);
        self.carry = bytes.split_off(whole);
        match self.wire {
            WireFmt::I32 => {
                for c in bytes.chunks_exact(4) {
                    out.push(i32::from_le_bytes(c.try_into().unwrap()) as f64);
                }
            }
            WireFmt::F32 => {
                for c in bytes.chunks_exact(4) {
                    out.push(f32::from_le_bytes(c.try_into().unwrap()) as f64);
                }
            }
        }
        Ok(out.len() / self.channels)
    }

    fn finish(mut self) -> Result<()> {
        if !self.eof {
            // §8.2 case 4: output truncated at the video duration — the decoder still
            // has frames buffered and is blocked on its stdout pipe; waiting would
            // deadlock. Kill it; its exit status is meaningless here.
            self.kill();
            if let Some(t) = self.stderr_thread.take() {
                let _ = t.join();
            }
            return Ok(());
        }
        let status = self
            .child
            .wait()
            .map_err(|e| CoupleError::internal(format!("export decode wait: {e}")))?;
        let stderr = self.stderr_thread.take().map(|t| t.join().unwrap_or_default());
        if !status.success() {
            return Err(CoupleError::FfmpegFailed {
                stage: "export-decode".into(),
                exit_code: status.code(),
                stderr_tail: ffmpeg::stderr_tail(&stderr.unwrap_or_default()),
            });
        }
        Ok(())
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- sample wire encoding ----------

fn encode_frames(samples: &[f64], wire: WireFmt, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * 4);
    match wire {
        WireFmt::I32 => {
            for &s in samples {
                let v = s.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        WireFmt::F32 => {
            for &s in samples {
                out.extend_from_slice(&(s as f32).to_le_bytes());
            }
        }
    }
}

// ---------- consumer (mux / wav child) ----------

struct Consumer {
    child: Child,
    stdin: Option<std::io::BufWriter<std::process::ChildStdin>>,
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
    /// out_time microseconds parsed from -progress (shared with the stderr thread)
    out_time_us: Arc<AtomicU64>,
}

impl Consumer {
    fn spawn(mut cmd: Command) -> Result<Self> {
        cmd.stdin(Stdio::piped()).stderr(Stdio::piped()).stdout(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| CoupleError::internal(format!("spawn export mux: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let out_time_us = Arc::new(AtomicU64::new(0));
        let shared = Arc::clone(&out_time_us);
        let stderr_thread = std::thread::spawn(move || {
            // -progress pipe:2 interleaves key=value lines with (rare) error text;
            // keep non-progress lines as the error tail.
            let mut tail: Vec<u8> = Vec::new();
            let mut line: Vec<u8> = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for &b in &buf[..n] {
                    if b == b'\n' {
                        let s = String::from_utf8_lossy(&line);
                        let s = s.trim();
                        if let Some(v) = s.strip_prefix("out_time_us=") {
                            if let Ok(us) = v.parse::<u64>() {
                                shared.store(us, Ordering::Relaxed);
                            }
                        } else if !s.contains('=') && !s.is_empty() {
                            tail.extend_from_slice(line.as_slice());
                            tail.push(b'\n');
                        }
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
            tail
        });
        Ok(Self {
            child,
            stdin: Some(std::io::BufWriter::with_capacity(1 << 20, stdin)),
            stderr_thread: Some(stderr_thread),
            out_time_us,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.stdin
            .as_mut()
            .unwrap()
            .write_all(bytes)
            .map_err(|e| CoupleError::ExportFailed {
                stage: "mux".into(),
                detail: format!("pipe: {e}"),
            })
    }

    fn finish(mut self, stage: &str) -> Result<()> {
        drop(self.stdin.take()); // close stdin → ffmpeg finalizes
        let status = self
            .child
            .wait()
            .map_err(|e| CoupleError::internal(format!("export mux wait: {e}")))?;
        let tail = self.stderr_thread.take().map(|t| t.join().unwrap_or_default());
        if !status.success() {
            return Err(CoupleError::FfmpegFailed {
                stage: stage.into(),
                exit_code: status.code(),
                stderr_tail: ffmpeg::stderr_tail(&tail.unwrap_or_default()),
            });
        }
        Ok(())
    }

    fn kill(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- the export job ----------

pub fn export(
    ffmpeg_bin: &Path,
    ffprobe_bin: &Path,
    video_path: &Path,
    external_path: &Path,
    mapping: &ExportMapping,
    opts: &ExportOptions,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<ExportReport> {
    let video_info = ffmpeg::probe(ffprobe_bin, video_path)?;
    let ext_info = ffmpeg::probe(ffprobe_bin, external_path)?;
    if opts.kind == ExportKind::Mux && video_info.video.is_none() {
        return Err(CoupleError::invalid(format!(
            "{}: no video stream to mux",
            video_path.display()
        )));
    }
    let plan = plan_source(&ext_info);
    let sr = plan.sample_rate as f64;
    let video_duration = video_info.duration_s;
    let n_out = (video_duration * sr).round() as u64;
    let channels = plan.channels as usize;

    // §8.2 edge case 5: refuse a zero-overlap mapping unless explicitly allowed.
    let delta = 1.0 + mapping.drift_ppm * 1e-6;
    let off_s = mapping.offset_ms / 1000.0;
    let ov_t0 = ((0.0 - off_s) / delta).max(0.0);
    let ov_t1 = ((plan.duration_s - off_s) / delta).min(video_duration);
    if ov_t1 <= ov_t0 && !opts.allow_silence {
        return Err(CoupleError::ExportFailed {
            stage: "render".into(),
            detail: "no overlap between external audio and the video timeline \
                     (pass --allow-silence to render silence anyway)"
                .into(),
        });
    }

    if opts.output_path.exists() && !opts.overwrite {
        return Err(CoupleError::invalid(format!(
            "{} exists (pass -f to overwrite)",
            opts.output_path.display()
        )));
    }

    // Disk preflight (§8.3): video size (mux) + projected audio + 5 %.
    let video_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
    let audio_bytes = n_out * channels as u64 * 4;
    let needed = if opts.kind == ExportKind::Mux {
        (video_bytes + audio_bytes) * 105 / 100
    } else {
        audio_bytes * 105 / 100
    };
    let out_dir = opts.output_path.parent().unwrap_or(Path::new("."));
    let avail = cache::available_bytes(if out_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        out_dir
    });
    if avail < needed {
        return Err(CoupleError::DiskFull {
            needed_bytes: needed,
            available_bytes: avail,
        });
    }

    // Temp file + atomic rename; -f <muxer> makes the temp extension irrelevant.
    let container = resolve_container(&video_info, opts.container);
    let tmp = opts.output_path.with_extension(format!(
        "{}.tmp",
        opts.output_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("out")
    ));

    // Build the consumer command (§8.3 mux / §8.5 wav).
    let pcm = pcm_codec(&plan);
    let wire_fmt_str = match plan.wire {
        WireFmt::I32 => "s32le",
        WireFmt::F32 => "f32le",
    };
    let keep_camera = opts.keep_camera_audio
        && opts.kind == ExportKind::Mux
        && !video_info.audio.is_empty();
    let audio_codec: String;
    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-nostdin").args(["-v", "error", "-y"]);
    match opts.kind {
        ExportKind::Mux => {
            cmd.arg("-i").arg(video_path);
            cmd.args(["-f", wire_fmt_str, "-ar", &plan.sample_rate.to_string()])
                .args(["-ac", &plan.channels.to_string()])
                .args(["-i", "pipe:0"]);
            cmd.args(["-map", "0:v:0", "-map", "1:a:0"]);
            if keep_camera {
                cmd.args(["-map", "0:a:0"]);
            }
            cmd.args(["-c:v", "copy"]);
            audio_codec = if container == "mp4" {
                // PCM-in-MP4 is non-portable (F9): AAC 256 kbps, AudioToolbox on macOS.
                let codec = if cfg!(target_os = "macos") { "aac_at" } else { "aac" };
                cmd.args(["-c:a:0", codec, "-b:a:0", "256k"]);
                codec.to_string()
            } else {
                cmd.args(["-c:a:0", pcm]);
                pcm.to_string()
            };
            if keep_camera {
                cmd.args(["-c:a:1", "copy", "-disposition:a:1", "0"]);
            }
            cmd.args(["-disposition:a:0", "default"]);
            cmd.args(["-map_metadata", "0"]);
            if opts.faststart {
                cmd.args(["-movflags", "+faststart"]);
            }
            cmd.args(["-f", container]);
        }
        ExportKind::Wav => {
            cmd.args(["-f", wire_fmt_str, "-ar", &plan.sample_rate.to_string()])
                .args(["-ac", &plan.channels.to_string()])
                .args(["-i", "pipe:0"]);
            audio_codec = pcm.to_string();
            cmd.args(["-c:a", pcm]);
            // §8.5: a long high-rate render exceeds 4 GiB — RF64 auto.
            cmd.args(["-rf64", "auto", "-f", "wav"]);
        }
    }
    cmd.args(["-progress", "pipe:2"]);
    cmd.arg(&tmp);

    let cleanup = TmpCleanup { path: tmp.clone() };
    let mut consumer = Consumer::spawn(cmd)?;
    let mut reader = SourceReader::spawn(ffmpeg_bin, external_path, &plan)?;

    let render = render_stream(
        &mut reader,
        &mut consumer,
        mapping,
        &plan,
        n_out,
        prog,
        cancel,
    );
    if let Err(e) = render {
        reader.kill();
        consumer.kill();
        return Err(e); // cleanup drops → temp removed
    }
    reader.finish()?;

    // Mux finalization (faststart etc.) with ExportMux progress from out_time.
    let out_time = Arc::clone(&consumer.out_time_us);
    prog.emit(Phase::ExportRender, 1.0);
    prog.emit(
        Phase::ExportMux,
        (out_time.load(Ordering::Relaxed) as f64 / 1e6 / video_duration).clamp(0.0, 0.99),
    );
    let stage = if opts.kind == ExportKind::Mux { "mux" } else { "wav" };
    consumer.finish(stage)?;
    cancel.check()?;
    prog.emit(Phase::ExportMux, 1.0);

    std::fs::rename(&tmp, &opts.output_path)
        .map_err(|e| CoupleError::internal(format!("rename output: {e}")))?;
    std::mem::forget(cleanup);

    Ok(ExportReport {
        output_path: opts.output_path.display().to_string(),
        container: if opts.kind == ExportKind::Wav { "wav".into() } else { container.into() },
        audio_codec,
        sample_rate: plan.sample_rate,
        channels: plan.channels,
        frames: n_out,
        kept_camera_audio: keep_camera,
        resampled: mapping.resample && mapping.drift_ppm != 0.0,
    })
}

/// §8.2 aligned-audio render, streamed into the consumer.
fn render_stream(
    reader: &mut SourceReader,
    consumer: &mut Consumer,
    mapping: &ExportMapping,
    plan: &SourcePlan,
    n_out: u64,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<()> {
    let channels = plan.channels as usize;
    let sr = plan.sample_rate as f64;
    let mut wire_buf: Vec<u8> = Vec::new();
    let mut emit = |consumer: &mut Consumer, samples: &[f64]| -> Result<()> {
        encode_frames(samples, plan.wire, &mut wire_buf);
        consumer.write(&wire_buf)
    };

    if !mapping.resample || mapping.drift_ppm == 0.0 {
        // driftMode "none" (F8): integer-sample shift S = round(offsetMs·sr/1000);
        // output frame n = source frame n + S; bit-exact samples; zero-fill outside.
        let shift = (mapping.offset_ms * sr / 1000.0).round() as i64;
        let mut written: u64 = 0;
        let mut frames: Vec<f64> = Vec::new();

        // silence head while n + S < 0
        if shift < 0 {
            let head = ((-shift) as u64).min(n_out);
            write_zeros(consumer, head, channels, plan.wire)?;
            written += head;
        } else if shift > 0 {
            // §8.2 case 2: external content trimmed at the head — discard S frames.
            let mut to_skip = shift as u64;
            while to_skip > 0 {
                cancel.check()?;
                let n = reader.read_frames(&mut frames, to_skip.min(1 << 16) as usize)?;
                if n == 0 {
                    break;
                }
                to_skip -= n as u64;
            }
        }
        while written < n_out {
            cancel.check()?;
            let want = ((n_out - written) as usize).min(1 << 16);
            let n = reader.read_frames(&mut frames, want)?;
            if n == 0 {
                break;
            }
            emit(consumer, &frames[..n * channels])?;
            written += n as u64;
            prog.emit(Phase::ExportRender, written as f64 / n_out as f64);
        }
        // §8.2 case 3: silence tail to exactly the video duration.
        if written < n_out {
            write_zeros(consumer, n_out - written, channels, plan.wire)?;
        }
        return Ok(());
    }

    // driftMode "resample": output frame n = source at u(n/sr)·sr, constant ratio
    // (1 + δ) with the fractional offset folded into the initial phase (§8.2).
    let ratio = 1.0 + mapping.drift_ppm * 1e-6;
    let phase0 = mapping.offset_ms * sr / 1000.0;
    let interp = SincInterp::new(EXPORT_TAPS, EXPORT_BETA);
    let half = interp.half();

    // Deinterleaved sliding buffers with an absolute base frame index.
    let mut chans: Vec<Vec<f64>> = vec![Vec::new(); channels];
    let mut base: i64 = 0;
    let mut frames: Vec<f64> = Vec::new();
    let mut out_chunk: Vec<f64> = Vec::new();
    const CHUNK: u64 = 1 << 16;

    let mut n: u64 = 0;
    while n < n_out {
        cancel.check()?;
        let count = CHUNK.min(n_out - n) as usize;
        let last_pos = (n + count as u64 - 1) as f64 * ratio + phase0;
        let need_hi = last_pos.floor() as i64 + half + 1;

        // grow the buffer to cover the chunk's support
        while !reader.eof && base + (chans[0].len() as i64) <= need_hi {
            let got = reader.read_frames(&mut frames, 1 << 16)?;
            if got == 0 {
                break;
            }
            for (c, chan) in chans.iter_mut().enumerate() {
                chan.extend(frames.iter().skip(c).step_by(channels));
            }
        }

        // render the chunk (parallel over channels × sub-ranges; read-only buffers)
        out_chunk.resize(count * channels, 0.0);
        {
            use rayon::prelude::*;
            let chans_ref = &chans;
            let interp_ref = &interp;
            out_chunk
                .par_chunks_mut(channels * 4096)
                .enumerate()
                .for_each(|(blk, out)| {
                    let start = n + (blk * 4096) as u64;
                    for (i, frame_out) in out.chunks_mut(channels).enumerate() {
                        let pos = (start + i as u64) as f64 * ratio + phase0;
                        for (c, slot) in frame_out.iter_mut().enumerate() {
                            *slot = interp_ref.eval(&chans_ref[c], base, pos);
                        }
                    }
                });
        }
        emit(consumer, &out_chunk)?;
        n += count as u64;
        prog.emit(Phase::ExportRender, n as f64 / n_out as f64);

        // drop buffer below the next chunk's support
        let next_lo = (n as f64 * ratio + phase0).floor() as i64 - half - 2;
        let drop = (next_lo - base).max(0) as usize;
        if drop > 1 << 16 {
            for chan in &mut chans {
                chan.drain(..drop.min(chan.len()));
            }
            base += drop as i64;
        }
    }
    Ok(())
}

fn write_zeros(consumer: &mut Consumer, frames: u64, channels: usize, wire: WireFmt) -> Result<()> {
    let zeros_f = vec![0f64; 1 << 16];
    let mut wire_buf = Vec::new();
    let mut remaining = frames * channels as u64;
    while remaining > 0 {
        let n = (zeros_f.len() as u64).min(remaining) as usize;
        encode_frames(&zeros_f[..n], wire, &mut wire_buf);
        consumer.write(&wire_buf)?;
        remaining -= n as u64;
    }
    Ok(())
}

struct TmpCleanup {
    path: PathBuf,
}

impl Drop for TmpCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
