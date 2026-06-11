//! Shared test helpers: ctx with temp cache, WAV authoring via hound, imports.

use couple_core::job::{noop_progress, CancelToken};
use couple_core::{Ctx, CtxConfig, TrackRole};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn test_ctx(cache_dir: &Path) -> Arc<Ctx> {
    Ctx::new(CtxConfig {
        cache_dir: Some(cache_dir.to_path_buf()),
        ..Default::default()
    })
}

pub fn write_wav_mono_48k(path: &Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

pub fn import(ctx: &Arc<Ctx>, path: &Path, role: TrackRole) -> couple_core::Result<u64> {
    ctx.import_blocking(path, role, noop_progress(), &CancelToken::new(), 0)
}

/// Real-data gate (§9.4): tests calling this skip unless COUPLE_REALDATA_DIR is set.
pub fn realdata_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("COUPLE_REALDATA_DIR").map(PathBuf::from)?;
    let video = dir.join("schumann-video.mp4");
    let audio = dir.join("schumann-audio.wav");
    if video.is_file() && audio.is_file() {
        Some(dir)
    } else {
        eprintln!("COUPLE_REALDATA_DIR set but files missing; skipping");
        None
    }
}
