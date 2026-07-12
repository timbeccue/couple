//! Import pipeline (§7): probe → decode (epoch rule) → single streaming pass tee into
//! PCM cache + pyramid + stats; onsets from the finished 480 level.

use crate::cache::{self, CacheEntry, CacheMeta, CACHE_META_VERSION};
use crate::error::{CoupleError, Result};
use crate::ffmpeg;
use crate::job::{CancelToken, Phase, ProgressEmitter};
use crate::media::*;
use crate::onsets::{self, OnsetParams};
use crate::pyramid::{PyramidBuilder, SAMPLE_RATE};
use crate::track::TrackData;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SR: f64 = SAMPLE_RATE as f64;

/// §7.2 epoch guard: sanity bound on the probed audio start_time. Legitimate
/// epochs are sub-second (MP4 edit lists / AAC priming; the reference camera
/// is 0.1808 s, SPEC §1.2) or, at the structural extreme, an MPEG-TS PCR
/// baseline, which wraps at 2^33/90000 ≈ 95 444 s. Anything beyond that is
/// container corruption; honoring it would zero-fill the PCM cache without
/// bound. Reject rather than clamp: a truncated epoch would silently misplace
/// the track on the canonical timeline (F1).
const MAX_EPOCH_ABS_S: f64 = 100_000.0;

/// Ok(lead samples at 48 kHz) or Err(detail) when start_time cannot be a real
/// capture timestamp. Pure so the bound is unit-testable without ffmpeg.
fn epoch_lead(epoch_s: f64) -> std::result::Result<i64, String> {
    if !epoch_s.is_finite() || epoch_s.abs() > MAX_EPOCH_ABS_S {
        return Err(format!(
            "audio start_time {epoch_s}s is outside the sane range \
             (±{MAX_EPOCH_ABS_S}s); container timestamps look corrupt"
        ));
    }
    Ok((epoch_s * SR).round() as i64)
}

/// Import disk projection (§6.3): PCM bytes for duration PLUS the epoch
/// zero-fill lead, with the 5 % pyramid/onsets overhead.
fn projected_cache_bytes(duration_s: f64, epoch_s: f64) -> u64 {
    ((duration_s.max(1.0) + epoch_s.max(0.0)) * SR * 4.0 * 1.05) as u64
}

pub struct ImportRequest<'a> {
    pub path: &'a Path,
    pub role: TrackRole,
    pub ffmpeg: &'a Path,
    pub ffprobe: &'a Path,
    pub cache_root: &'a Path,
}

/// Blocking import. Job wrapping (thread + JobId) lives in ctx.rs.
pub fn import(
    req: &ImportRequest,
    id: TrackId,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<TrackData> {
    prog.emit(Phase::Probe, 0.0);
    let info = ffmpeg::probe(req.ffprobe, req.path)?;
    let key = cache::content_key(req.path)?;
    prog.emit(Phase::Probe, 1.0);
    cancel.check()?;

    let entry = CacheEntry::new(req.cache_root, &key);
    if let Some(cmeta) = entry.load_meta(&key) {
        // Cache hit: still report progress, fast (§4.3).
        for ph in [Phase::Decode, Phase::Pyramid, Phase::Finalize] {
            prog.emit(ph, 1.0);
        }
        entry.touch();
        let meta = track_meta(id, req.role, &info, &cmeta, &key);
        return TrackData::open(entry, &cmeta, meta);
    }

    decode_into_cache(req, &info, &entry, &key, prog, cancel)?;
    let cmeta = entry.load_meta(&key).ok_or_else(|| CoupleError::CacheCorrupt {
        path: entry.dir.clone(),
    })?;
    let meta = track_meta(id, req.role, &info, &cmeta, &key);
    TrackData::open(entry, &cmeta, meta)
}

fn track_meta(id: TrackId, role: TrackRole, info: &MediaInfo, c: &CacheMeta, key: &str) -> TrackMeta {
    TrackMeta {
        id,
        role,
        info: info.clone(),
        epoch_s: c.epoch_s,
        duration_s: c.frames as f64 / SR,
        sample_rate: SAMPLE_RATE,
        normalization: c.normalization,
        stats: c.stats.clone(),
        content_key: key.to_string(),
    }
}

/// Equal-weight downmix filter per channel count; mono in = passthrough (§7.2).
fn pan_filter(channels: u32) -> Option<String> {
    if channels <= 1 {
        return None;
    }
    let w = 1.0 / channels as f64;
    let terms: Vec<String> = (0..channels).map(|c| format!("{w:.10}*c{c}")).collect();
    Some(format!("pan=mono|c0={}", terms.join("+")))
}

fn decode_into_cache(
    req: &ImportRequest,
    info: &MediaInfo,
    entry: &CacheEntry,
    key: &str,
    prog: &mut ProgressEmitter,
    cancel: &CancelToken,
) -> Result<()> {
    let audio = &info.audio[0];
    // §1.2/§7.2: the canonical timeline is the container timeline. The decoded audio
    // stream's start_time becomes leading zeros (positive) or dropped samples (negative).
    let epoch_s = audio.start_time_s;
    let lead_samples: i64 = epoch_lead(epoch_s).map_err(|detail| {
        CoupleError::UnsupportedFormat {
            path: req.path.to_path_buf(),
            detail,
        }
    })?;

    let projected = projected_cache_bytes(info.duration_s, epoch_s);
    cache::preflight(req.cache_root, projected)?;
    std::fs::create_dir_all(&entry.dir)
        .map_err(|e| CoupleError::internal(format!("create cache entry dir: {e}")))?;

    let cleanup = TempCleanup {
        dir: entry.dir.clone(),
        armed: true,
    };

    let mut cmd = Command::new(req.ffmpeg);
    cmd.arg("-nostdin").args(["-v", "error"]).arg("-i").arg(req.path);
    cmd.args(["-map", "a:0"]);
    if let Some(pan) = pan_filter(audio.channels) {
        cmd.args(["-af", &pan]);
    }
    // Forbidden (§7.2): aresample=async, first_pts rewriting, atempo — nothing that
    // resynthesizes timestamps. Plain -ar 48000 only.
    cmd.args(["-ar", "48000", "-f", "f32le", "pipe:1"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| CoupleError::internal(format!("spawn ffmpeg: {e}")))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let pcm_tmp = entry.dir.join("pcm48f32.raw.tmp");
    let pcm_file = std::fs::File::create(&pcm_tmp)
        .map_err(|e| CoupleError::internal(format!("create pcm tmp: {e}")))?;
    let mut pcm = std::io::BufWriter::with_capacity(1 << 20, pcm_file);

    let mut builder = PyramidBuilder::new();
    let mut energy = EnergyAccum::new(2400);

    // Epoch lead materialized as zero samples (§6.2).
    if lead_samples > 0 {
        builder.push_zeros(lead_samples as u64);
        energy.push_zeros(lead_samples as u64);
        write_zeros(&mut pcm, lead_samples as u64)?;
    }
    let mut drop_remaining: u64 = if lead_samples < 0 { (-lead_samples) as u64 } else { 0 };

    let total_frames = (info.duration_s * SR).max(1.0);
    let mut frames_done: u64 = 0;
    let mut byte_buf = vec![0u8; 1 << 20];
    let mut carry: Vec<u8> = Vec::with_capacity(4);
    let mut samples: Vec<f32> = Vec::with_capacity(1 << 18);

    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            drop(pcm);
            drop(cleanup); // removes partial cache entry
            return Err(CoupleError::Cancelled);
        }
        let n = stdout
            .read(&mut byte_buf)
            .map_err(|e| CoupleError::internal(format!("read ffmpeg pipe: {e}")))?;
        if n == 0 {
            break;
        }
        let mut bytes: &[u8] = &byte_buf[..n];
        samples.clear();
        if !carry.is_empty() {
            while carry.len() < 4 && !bytes.is_empty() {
                carry.push(bytes[0]);
                bytes = &bytes[1..];
            }
            if carry.len() == 4 {
                samples.push(f32::from_le_bytes(carry[..].try_into().unwrap()));
                carry.clear();
            }
        }
        let whole = bytes.len() / 4 * 4;
        for c in bytes[..whole].chunks_exact(4) {
            samples.push(f32::from_le_bytes(c.try_into().unwrap()));
        }
        carry.extend_from_slice(&bytes[whole..]);

        let mut slice: &[f32] = &samples;
        if drop_remaining > 0 {
            let d = (drop_remaining as usize).min(slice.len());
            slice = &slice[d..];
            drop_remaining -= d as u64;
        }
        if !slice.is_empty() {
            builder.push(slice);
            energy.push(slice);
            pcm.write_all(bytes_of(slice))
                .map_err(|e| CoupleError::internal(format!("write pcm: {e}")))?;
        }
        frames_done += samples.len() as u64;

        // §4.6 mapping: Decode 0–80%, Pyramid 80–95%, fractions from bytes consumed.
        let overall = (frames_done as f64 / total_frames).min(1.0) * 0.95;
        if overall < 0.80 {
            prog.emit(Phase::Decode, overall / 0.80);
        } else {
            prog.emit(Phase::Decode, 1.0);
            prog.emit(Phase::Pyramid, (overall - 0.80) / 0.15);
        }
    }
    let status = child
        .wait()
        .map_err(|e| CoupleError::internal(format!("wait ffmpeg: {e}")))?;
    let stderr_buf = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        return Err(CoupleError::FfmpegFailed {
            stage: "decode".into(),
            exit_code: status.code(),
            stderr_tail: ffmpeg::stderr_tail(&stderr_buf),
        });
    }
    if builder.n_samples() == 0 {
        return Err(CoupleError::DecodeFailed {
            path: req.path.to_path_buf(),
            detail: "decoder produced no samples".into(),
        });
    }
    pcm.flush()
        .map_err(|e| CoupleError::internal(format!("flush pcm: {e}")))?;
    prog.emit(Phase::Pyramid, 1.0);
    prog.emit(Phase::Finalize, 0.0);
    cancel.check().inspect_err(|_| drop_pcm_tmp(&pcm_tmp))?;

    let frames = builder.n_samples();
    let pyramid = builder.finalize();
    let normalization = pyramid.normalization;

    // Stats (§4.2): p95 of active envelope bins at 9600 spb; active RMS dBFS at 2400 spb.
    let stats = compute_stats(&pyramid, &energy, normalization);

    // Onsets from the finished 480 level (§5.7), int span domain.
    let lvl480 = pyramid.level(480).expect("480 level exists");
    let spans: Vec<u8> = lvl480
        .mins
        .iter()
        .zip(&lvl480.maxs)
        .map(|(&mn, &mx)| (mx as i16 - mn as i16) as u8)
        .collect();
    let onset_list = onsets::rank_onsets(&spans, 480, SAMPLE_RATE, &OnsetParams::default());
    prog.emit(Phase::Finalize, 0.4);

    // Artifacts: temp + atomic rename; meta.json last (its presence marks validity).
    pyramid.write(&entry.pyramid_path())?;
    std::fs::rename(&pcm_tmp, entry.pcm_path())
        .map_err(|e| CoupleError::internal(format!("rename pcm: {e}")))?;
    write_atomic(
        &entry.onsets_path(),
        &serde_json::to_vec(&onset_list).expect("onsets serialize"),
    )?;

    // Spectral fingerprint sidecar (PLAN-SPECTRAL-FINGERPRINT-MATCHING §8.3.1): generated in THIS
    // decode pass, NOT by a second decode of the source media — we mmap the canonical PCM we just
    // wrote and run the deterministic builder over it (the exact same path `fingerprint_v1` takes
    // for an old entry). It is an optional, lazily-rebuildable sidecar with an independent format
    // lifecycle, so a failure here must NOT fail the import or invalidate the entry: the artifact
    // will simply be built on first request. `meta.json` (written below) marks entry validity and
    // does not depend on the fingerprint.
    generate_fingerprint(entry, frames);
    let cmeta = CacheMeta {
        version: CACHE_META_VERSION,
        content_key: key.to_string(),
        sample_rate: SAMPLE_RATE,
        frames,
        epoch_s: if lead_samples != 0 { epoch_s } else { 0.0 },
        source_duration_s: info.duration_s,
        normalization,
        stats,
        created_by: format!("couple-core {}", env!("CARGO_PKG_VERSION")),
    };
    write_atomic(&entry.meta_path(), &serde_json::to_vec_pretty(&cmeta).unwrap())?;
    std::mem::forget(cleanup); // cache entry is now valid
    prog.emit(Phase::Finalize, 1.0);
    Ok(())
}

fn compute_stats(
    pyramid: &crate::pyramid::FinishedPyramid,
    energy: &EnergyAccum,
    normalization: f64,
) -> TrackStats {
    let gate = 0.12 * normalization;

    let idx9600 = crate::pyramid::LEVELS_SPB.iter().position(|&s| s == 9600).unwrap();
    let (mins, maxs) = &pyramid.float_levels[idx9600];
    let mut active: Vec<f64> = mins
        .iter()
        .zip(maxs)
        .map(|(&mn, &mx)| (mn.abs().max(mx.abs())) as f64)
        .filter(|&a| a > gate)
        .collect();
    active.sort_by(f64::total_cmp);
    let active_p95_9600 = if active.is_empty() {
        0.0
    } else {
        active[((active.len() - 1) as f64 * 0.95).floor() as usize]
    };

    let idx2400 = crate::pyramid::LEVELS_SPB.iter().position(|&s| s == 2400).unwrap();
    let (mins, maxs) = &pyramid.float_levels[idx2400];
    let mut sum_sq = 0f64;
    let mut count = 0u64;
    for (b, (&mn, &mx)) in mins.iter().zip(maxs).enumerate() {
        if (mn.abs().max(mx.abs())) as f64 > gate {
            if let Some((e, c)) = energy.bin(b) {
                sum_sq += e;
                count += c;
            }
        }
    }
    let active_rms_dbfs = if count == 0 {
        f64::NEG_INFINITY
    } else {
        20.0 * (sum_sq / count as f64).sqrt().log10()
    };

    TrackStats {
        abs_peak: normalization,
        active_p95_9600,
        active_rms_dbfs,
    }
}

/// Per-bin sum-of-squares accumulator at a fixed samples-per-bin (powers active-RMS stats).
struct EnergyAccum {
    spb: u64,
    bins: Vec<(f64, u64)>,
    cur_sum: f64,
    cur_count: u64,
}

impl EnergyAccum {
    fn new(spb: u64) -> Self {
        Self {
            spb,
            bins: Vec::new(),
            cur_sum: 0.0,
            cur_count: 0,
        }
    }
    fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.cur_sum += (s as f64) * (s as f64);
            self.cur_count += 1;
            if self.cur_count == self.spb {
                self.bins.push((self.cur_sum, self.cur_count));
                self.cur_sum = 0.0;
                self.cur_count = 0;
            }
        }
    }
    fn push_zeros(&mut self, count: u64) {
        let mut remaining = count;
        while remaining > 0 {
            let take = (self.spb - self.cur_count).min(remaining);
            self.cur_count += take;
            if self.cur_count == self.spb {
                self.bins.push((self.cur_sum, self.cur_count));
                self.cur_sum = 0.0;
                self.cur_count = 0;
            }
            remaining -= take;
        }
    }
    fn bin(&self, i: usize) -> Option<(f64, u64)> {
        if i < self.bins.len() {
            Some(self.bins[i])
        } else if i == self.bins.len() && self.cur_count > 0 {
            Some((self.cur_sum, self.cur_count))
        } else {
            None
        }
    }
}

fn bytes_of(samples: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 4) }
}

fn write_zeros<W: Write>(w: &mut W, frames: u64) -> Result<()> {
    let zeros = vec![0u8; 1 << 16];
    let mut remaining = frames * 4;
    while remaining > 0 {
        let n = (zeros.len() as u64).min(remaining) as usize;
        w.write_all(&zeros[..n])
            .map_err(|e| CoupleError::internal(format!("write pcm zeros: {e}")))?;
        remaining -= n as u64;
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| CoupleError::internal(format!("write {path:?}: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| CoupleError::internal(format!("rename {path:?}: {e}")))?;
    Ok(())
}

/// Generate the `fingerprint-v1.cpf` sidecar during import (§8.3.1) by mmap'ing the canonical PCM
/// just written into the entry and running the deterministic builder over it — the SAME
/// build-from-PCM path the lazy `TrackData::fingerprint_v1` uses, so an import-time artifact is
/// byte-identical to one built later on demand. No second source decode happens.
///
/// Best-effort: the fingerprint is an optional, independently-versioned, lazily-rebuildable
/// sidecar (§3 invariant 7). A failure to mmap or write it must not fail the import or invalidate
/// the cache entry — the artifact is simply built on first request. `load_or_build` already does
/// the atomic temp-rename and removes any `.tmp` on error, so a failure leaves no litter.
fn generate_fingerprint(entry: &CacheEntry, frames: u64) {
    let pcm_path = entry.pcm_path();
    let Ok(file) = std::fs::File::open(&pcm_path) else {
        return;
    };
    // SAFETY: the PCM was just written and is not concurrently mutated; this is the same mmap the
    // track loader performs. A mapping failure (e.g. zero-length file) is non-fatal.
    let Ok(mmap) = (unsafe { memmap2::Mmap::map(&file) }) else {
        return;
    };
    let bytes: &[u8] = &mmap;
    if !(bytes.as_ptr() as usize).is_multiple_of(4) || !bytes.len().is_multiple_of(4) {
        // Page-aligned and a whole number of f32 frames in practice; bail rather than risk an
        // unaligned/partial read.
        return;
    }
    let samples = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) };
    let _ = crate::fingerprint::load_or_build(&entry.fingerprint_path_v1(), samples, frames);
}

fn drop_pcm_tmp(p: &Path) {
    let _ = std::fs::remove_file(p);
}

/// Removes the (partial) cache entry dir on drop unless defused via mem::forget.
struct TempCleanup {
    dir: PathBuf,
    armed: bool,
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_guard_accepts_real_container_range() {
        assert_eq!(epoch_lead(0.0), Ok(0));
        assert_eq!(epoch_lead(0.180792), Ok(8678)); // §1.2 reference camera
        assert_eq!(epoch_lead(-0.5), Ok(-24_000)); // negative epoch: dropped samples
        assert!(epoch_lead(95_443.7).is_ok()); // MPEG-TS PCR wrap baseline
    }

    #[test]
    fn epoch_guard_rejects_corrupt_start_time() {
        for bad in [1.0e9, -1.0e9, 100_001.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let err = epoch_lead(bad).unwrap_err();
            assert!(err.contains("start_time"), "detail should name the field: {err}");
        }
    }

    #[test]
    fn preflight_projection_accounts_for_epoch_lead() {
        let base = projected_cache_bytes(10.0, 0.0);
        assert_eq!(base, (10.0 * 48_000.0 * 4.0 * 1.05) as u64);
        assert_eq!(
            projected_cache_bytes(10.0, 5.0),
            (15.0 * 48_000.0 * 4.0 * 1.05) as u64
        );
        assert_eq!(projected_cache_bytes(10.0, -5.0), base);
    }
}
