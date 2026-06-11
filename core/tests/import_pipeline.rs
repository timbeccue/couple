//! Import pipeline tests: epoch fixture (§9.2), cache round-trip, cancellation litter.

mod common;

use couple_core::job::{noop_progress, CancelToken};
use couple_core::TrackRole;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::process::Command;

fn ffmpeg_path() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg required for tests")
}

/// §9.2 epoch fixture: a MOV remuxed with `-itsoffset 0.18 -c copy` must decode to a
/// PCM cache leading with exactly round(0.18 · 48000) zeros (F1 — locked forever).
#[test]
fn epoch_rule_materializes_lead_zeros() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = ChaCha8Rng::seed_from_u64(99);
    // Noise: first real sample is almost surely nonzero (a sine would start at 0).
    let samples: Vec<f32> = (0..48_000 * 3).map(|_| rng.gen_range(-0.5f32..0.5)).collect();
    let wav = dir.path().join("src.wav");
    common::write_wav_mono_48k(&wav, &samples);

    let mov = dir.path().join("epoch.mov");
    let st = Command::new(ffmpeg_path())
        .args(["-nostdin", "-v", "error", "-y", "-itsoffset", "0.18", "-i"])
        .arg(&wav)
        .args(["-c", "copy"])
        .arg(&mov)
        .status()
        .unwrap();
    assert!(st.success());

    let cache = dir.path().join("cache");
    let ctx = common::test_ctx(&cache);
    let info = ctx.probe(&mov).unwrap();
    let epoch = info.audio[0].start_time_s;
    assert!((epoch - 0.18).abs() < 1e-6, "fixture start_time = {epoch}");

    let id = common::import(&ctx, &mov, TrackRole::External).unwrap();
    let track = ctx.track(id).unwrap();
    assert!((track.meta.epoch_s - 0.18).abs() < 1e-6);

    let lead = (0.18f64 * 48_000.0).round() as usize; // 8640
    let mut buf = vec![1.0f32; lead + 100];
    track.raw_window(0.0, (lead + 100) as f64 / 48_000.0, &mut buf).unwrap();
    assert!(
        buf[..lead].iter().all(|&v| v == 0.0),
        "first {lead} samples must be exactly zero"
    );
    assert!(
        buf[lead..].iter().any(|&v| v != 0.0),
        "signal must start right after the epoch lead"
    );
    // Canonical-timeline duration includes the lead.
    assert!((track.meta.duration_s - (3.0 + 0.18)).abs() < 0.02);
}

#[test]
fn plain_wav_has_zero_epoch_and_roundtrips_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    let samples: Vec<f32> = (0..48_000 * 2).map(|_| rng.gen_range(-0.9f32..0.9)).collect();
    let wav = dir.path().join("a.wav");
    common::write_wav_mono_48k(&wav, &samples);

    let cache = dir.path().join("cache");
    let ctx = common::test_ctx(&cache);
    let id = common::import(&ctx, &wav, TrackRole::External).unwrap();
    let t1 = ctx.track(id).unwrap();
    assert_eq!(t1.meta.epoch_s, 0.0);
    assert!((t1.meta.duration_s - 2.0).abs() < 1e-6);

    // Second import = cache hit, identical results.
    let id2 = common::import(&ctx, &wav, TrackRole::External).unwrap();
    let t2 = ctx.track(id2).unwrap();
    assert_eq!(t1.meta.content_key, t2.meta.content_key);
    assert_eq!(t1.meta.normalization, t2.meta.normalization);
    assert_eq!(t1.frames(), t2.frames());

    // raw_window returns the decoded samples (16-bit quantized).
    let mut buf = vec![0f32; 480];
    t1.raw_window(0.5, 0.51, &mut buf).unwrap();
    for (j, &v) in buf.iter().enumerate() {
        let expect = (samples[24_000 + j].clamp(-1.0, 1.0) * 32_767.0).round() / 32_767.0;
        assert!((v - expect).abs() < 1e-4, "sample {j}: {v} vs {expect}");
    }
}

/// §9.7 cancellation: no temp/cache litter, `Cancelled` surfaced.
#[test]
fn cancel_mid_import_leaves_no_litter() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    let samples: Vec<f32> = (0..48_000 * 30).map(|_| rng.gen_range(-0.5f32..0.5)).collect();
    let wav = dir.path().join("long.wav");
    common::write_wav_mono_48k(&wav, &samples);

    let cache = dir.path().join("cache");
    let ctx = common::test_ctx(&cache);
    let cancel = CancelToken::new();
    cancel.cancel(); // observed at the first chunk boundary
    let err = ctx
        .import_blocking(&wav, TrackRole::External, noop_progress(), &cancel, 0)
        .unwrap_err();
    assert!(matches!(err, couple_core::CoupleError::Cancelled));

    // No cache entry litter (cache root may exist but must be empty of entries).
    let entries = couple_core::cache::list_entries(&cache);
    assert!(entries.is_empty(), "cache litter: {entries:?}");
}

#[test]
fn import_reports_monotone_progress() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..48_000 * 5)
        .map(|i| (i as f32 * 0.01).sin() * 0.5)
        .collect();
    let wav = dir.path().join("p.wav");
    common::write_wav_mono_48k(&wav, &samples);
    let ctx = common::test_ctx(&dir.path().join("cache"));

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let progress: couple_core::ProgressFn = std::sync::Arc::new(move |p: &couple_core::Progress| {
        seen2.lock().unwrap().push((p.phase, p.fraction));
    });
    ctx.import_blocking(&wav, TrackRole::External, progress, &CancelToken::new(), 0)
        .unwrap();
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    // Monotone within each phase.
    for w in seen.windows(2) {
        if w[0].0 == w[1].0 {
            assert!(w[1].1 >= w[0].1, "fraction regressed within {:?}", w[0].0);
        }
    }
    // Finalize completes.
    assert!(seen
        .iter()
        .any(|(p, f)| *p == couple_core::Phase::Finalize && *f >= 1.0));
}
