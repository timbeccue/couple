//! §9.2: pyramid round-trip property tests — band() equals brute-force min/max
//! over any range/level, through build → write → mmap-read.

use couple_core::pyramid::{band, PyramidBuilder, PyramidReader, LEVELS_SPB, SAMPLE_RATE};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

fn build_and_open(samples: &[f32], dir: &std::path::Path) -> PyramidReader {
    let mut b = PyramidBuilder::new();
    // push in ragged chunks to exercise bin-boundary handling
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let mut i = 0;
    while i < samples.len() {
        let n = rng.gen_range(1..=8192).min(samples.len() - i);
        b.push(&samples[i..i + n]);
        i += n;
    }
    let fin = b.finalize();
    let path = dir.join("pyramid.cpk");
    fin.write(&path).unwrap();
    PyramidReader::open(&path).unwrap()
}

fn quantize(v: f32, norm: f64) -> i8 {
    if norm <= 0.0 {
        return 0;
    }
    ((v as f64 / norm * 127.0).round() as i32).clamp(-127, 127) as i8
}

#[test]
fn levels_match_brute_force() {
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    // ~37.6 s of random audio, deliberately not a multiple of any spb
    let n = 1_805_557usize;
    let samples: Vec<f32> = (0..n).map(|_| rng.gen_range(-0.8f32..0.8)).collect();
    let dir = tempfile::tempdir().unwrap();
    let p = build_and_open(&samples, dir.path());

    assert_eq!(p.duration_samples, n as u64);
    let norm = p.normalization;
    let expect_norm = samples.iter().fold(0f32, |m, &v| m.max(v.abs())) as f64;
    assert!((norm - expect_norm).abs() < 1e-12);

    for spb in LEVELS_SPB {
        let n_bins = (n as u64).div_ceil(spb as u64);
        assert_eq!(p.n_bins(spb), n_bins, "level {spb}");
        let bins = p.bins(spb, 0, n_bins).unwrap();
        for b in 0..n_bins as usize {
            let a = b * spb as usize;
            let e = ((b + 1) * spb as usize).min(n);
            let mut mn = f32::INFINITY;
            let mut mx = f32::NEG_INFINITY;
            for &s in &samples[a..e] {
                mn = mn.min(s);
                mx = mx.max(s);
            }
            assert_eq!(bins[b * 2], quantize(mn, norm), "level {spb} bin {b} min");
            assert_eq!(bins[b * 2 + 1], quantize(mx, norm), "level {spb} bin {b} max");
        }
    }
}

#[test]
fn band_matches_reference_aggregation() {
    let mut rng = ChaCha8Rng::seed_from_u64(1234);
    let n = 3_000_000usize; // 62.5 s
    let samples: Vec<f32> = (0..n).map(|_| rng.gen_range(-1.0f32..1.0)).collect();
    let dir = tempfile::tempdir().unwrap();
    let p = build_and_open(&samples, dir.path());
    let sr = SAMPLE_RATE as f64;
    let dur = n as f64 / sr;

    for _ in 0..200 {
        let t0 = rng.gen_range(-5.0..dur);
        let t1 = t0 + rng.gen_range(0.001..(dur * 1.2));
        let n_cols = rng.gen_range(1..=2048u32);
        let band = band(&p, t0, t1, n_cols).unwrap();

        // Level selection: coarsest level still yielding >= n_cols bins over [t0,t1].
        let span = (t1 - t0) * sr;
        let expect_spb = LEVELS_SPB
            .iter()
            .copied()
            .filter(|&spb| span / spb as f64 >= n_cols as f64)
            .max()
            .unwrap_or(LEVELS_SPB[0]);
        assert_eq!(band.level_spb, expect_spb);

        // Each column = min/max over that level's bins intersecting the column span;
        // columns fully outside the track are (0,0).
        let spb = band.level_spb;
        let n_bins = p.n_bins(spb) as i64;
        let bins = p.bins(spb, 0, p.n_bins(spb)).unwrap();
        let dt = (t1 - t0) / n_cols as f64;
        for c in 0..n_cols as usize {
            let ct0 = t0 + c as f64 * dt;
            let b0 = ((ct0 * sr / spb as f64).floor() as i64).max(0);
            let b1 = (((ct0 + dt) * sr / spb as f64).ceil() as i64).min(n_bins);
            let (mut mn, mut mx) = (0i8, 0i8);
            if b0 < b1 {
                mn = i8::MAX;
                mx = i8::MIN;
                for b in b0..b1 {
                    mn = mn.min(bins[b as usize * 2]);
                    mx = mx.max(bins[b as usize * 2 + 1]);
                }
            }
            assert_eq!(band.mins[c], mn, "col {c} min (t0={t0} t1={t1} cols={n_cols})");
            assert_eq!(band.maxs[c], mx, "col {c} max (t0={t0} t1={t1} cols={n_cols})");
        }
    }
}

#[test]
fn silence_quantizes_to_zero() {
    let samples = vec![0f32; 100_000];
    let dir = tempfile::tempdir().unwrap();
    let p = build_and_open(&samples, dir.path());
    assert_eq!(p.normalization, 0.0);
    let bins = p.bins(24, 0, p.n_bins(24)).unwrap();
    assert!(bins.iter().all(|&v| v == 0));
}
