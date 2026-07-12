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

/// Real-data gate (§9.4). The realdata tests are `#[ignore]`d so a default
/// `cargo test` prints a visible "SKIPPED (no realdata)" marker per test instead
/// of a silent green (F4 fail-loud, 2026-07-11 — see PLAN-REALDATA-CALIBRATION.md).
/// When one IS run (`--include-ignored`, normally via scripts/realdata-sweep.sh),
/// a missing or incomplete COUPLE_REALDATA_DIR is a loud FAILURE: the caller
/// explicitly asked for real data.
pub fn realdata_dir() -> PathBuf {
    let Some(dir) = std::env::var_os("COUPLE_REALDATA_DIR").map(PathBuf::from) else {
        panic!(
            "REALDATA MISSING: this test was run explicitly (--include-ignored) but \
             COUPLE_REALDATA_DIR is not set. Point it at the directory holding \
             schumann-video.mp4 + schumann-audio.wav, or run scripts/realdata-sweep.sh."
        );
    };
    let video = dir.join("schumann-video.mp4");
    let audio = dir.join("schumann-audio.wav");
    assert!(
        video.is_file() && audio.is_file(),
        "REALDATA MISSING: COUPLE_REALDATA_DIR={} is set but schumann-video.mp4 / \
         schumann-audio.wav are not both present",
        dir.display()
    );
    dir
}
