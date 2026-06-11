//! §9.2 / §7.3 real-pair import gates. Feature-gated by COUPLE_REALDATA_DIR (§9.4):
//! skips (with a notice) when the 3.6 GB pair is absent. Run in release.

mod common;

use couple_core::TrackRole;

fn cache_dir() -> std::path::PathBuf {
    // Reuse the repo-local cache so CLI runs and tests share imports.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.couple-cache")
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let ma = a.iter().sum::<f64>() / n as f64;
    let mb = b.iter().sum::<f64>() / n as f64;
    let mut sab = 0.0;
    let mut saa = 0.0;
    let mut sbb = 0.0;
    for i in 0..n {
        let x = a[i] - ma;
        let y = b[i] - mb;
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    sab / (saa * sbb).sqrt()
}

/// Mockup peaks JSON (retired format, §6.1) → span envelope at a level.
fn mockup_spans(path: &std::path::Path, spb: u64) -> Vec<f64> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("peaks json")).expect("parse");
    let levels = v["levels"].as_array().unwrap();
    let lvl = levels
        .iter()
        .find(|l| l["samples_per_bin"].as_u64() == Some(spb))
        .expect("48000 level present");
    let peaks = lvl["peaks"].as_array().unwrap();
    peaks
        .chunks_exact(2)
        .map(|p| p[1].as_f64().unwrap() - p[0].as_f64().unwrap())
        .collect()
}

#[test]
fn real_pair_import_gates() {
    let Some(dir) = common::realdata_dir() else {
        eprintln!("SKIP: COUPLE_REALDATA_DIR not set");
        return;
    };
    let ctx = common::test_ctx(&cache_dir());

    // Camera (§7.3): duration epoch-inclusive, epoch from the AAC stream start_time.
    let cam_id = common::import(&ctx, &dir.join("schumann-video.mp4"), TrackRole::Camera).unwrap();
    let cam = ctx.track(cam_id).unwrap();
    assert!(
        (cam.meta.duration_s - 1435.252).abs() <= 0.05,
        "camera duration {}",
        cam.meta.duration_s
    );
    assert!(
        (cam.meta.epoch_s - 0.1808).abs() <= 0.001,
        "camera epoch {}",
        cam.meta.epoch_s
    );

    // External: exact-duration WAV, epoch 0 by definition.
    let ext_id = common::import(&ctx, &dir.join("schumann-audio.wav"), TrackRole::External).unwrap();
    let ext = ctx.track(ext_id).unwrap();
    assert!((ext.meta.duration_s - 1798.88).abs() <= 0.01, "external duration");
    assert_eq!(ext.meta.epoch_s, 0.0);

    // §9.2: engine 48000-spb envelope vs mockup peaks JSON, Pearson r ≥ 0.99.
    // External aligns bin-for-bin.
    let ext_engine: Vec<f64> = {
        let n = ext.pyramid.n_bins(48_000);
        let bins = ext.pyramid.bins(48_000, 0, n).unwrap();
        bins.chunks_exact(2).map(|p| (p[1] as i16 - p[0] as i16) as f64).collect()
    };
    let ext_mock = mockup_spans(&dir.join("peaks/external.json"), 48_000);
    let r_ext = pearson(&ext_engine, &ext_mock);
    eprintln!("external envelope r = {r_ext:.5}");
    assert!(r_ext >= 0.99, "external r = {r_ext}");

    // Camera: the mockup extraction lacked the 180.79 ms epoch lead (§1.3), so compare
    // engine bins shifted by the epoch — min/max over PCM [epoch + i·1s, epoch + (i+1)·1s).
    let epoch_samples = (cam.meta.epoch_s * 48_000.0).round() as i64;
    let n_bins = ((cam.frames() as i64 - epoch_samples) / 48_000) as usize;
    let mut cam_engine = Vec::with_capacity(n_bins);
    let mut buf = vec![0f32; 48_000];
    for i in 0..n_bins {
        let t0 = (epoch_samples + i as i64 * 48_000) as f64 / 48_000.0;
        cam.raw_window(t0, t0 + 1.0, &mut buf).unwrap();
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for &v in &buf {
            mn = mn.min(v);
            mx = mx.max(v);
        }
        cam_engine.push((mx - mn) as f64);
    }
    let cam_mock = mockup_spans(&dir.join("peaks/camera.json"), 48_000);
    let r_cam = pearson(&cam_engine, &cam_mock);
    eprintln!("camera envelope r = {r_cam:.5} (epoch-shifted)");
    assert!(r_cam >= 0.99, "camera r = {r_cam}");
}
