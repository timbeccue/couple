//! PLAN-SPECTRAL-FINGERPRINT-MATCHING fingerprint signal-builder gates (§7.5). Boundary 1's
//! gates run at the PEAK level (1–5); boundary 2 adds the LANDMARK-level gates (6–8) plus the
//! landmark/`.cpf` extension of the chunk-invariance gate.
//!
//! Peak-level gates (boundary 1):
//! 1. CHUNK-BOUNDARY INVARIANCE (§7.2/§7.5.1): the same 48 kHz PCM pushed all-at-once, one
//!    sample at a time, and in deterministic random chunk sizes ⇒ BYTE-IDENTICAL serialized
//!    peak lists.
//! 2. GAIN INVARIANCE over ±24 dB (§7.5.2): scaling the input leaves the peak SET stable.
//! 3. DETERMINISTIC PEAK TIE-BREAKING (§7.5.3): a synthetic equal-maxima plateau resolves to
//!    exactly one peak by the documented rule (lower freq bin, then earlier frame).
//! 4. NO PEAKS FROM DIGITAL SILENCE (§7.5.4).
//! 5. BOUNDED PEAK DENSITY on a sustained periodic tone (§7.5.5): the per-second cap holds.
//!
//! Landmark-level gates (boundary 2):
//! - CHUNK-BOUNDARY INVARIANCE extended to LANDMARKS and the encoded CPF1 artifact (§7.5.1):
//!   byte-identical landmark sets AND byte-identical `.cpf` bytes across all chunkings.
//! 6. STABLE under AGC + moderate filtering (§7.5.6): a strong majority of hashes survive.
//! 7. NEGLIGIBLE exact-hash overlap for a DIFFERENT random event (§7.5.7).
//! 8. SAME-MASTER excerpts share hashes at the CORRECT offset (§7.5.8): the dominant
//!    anchor-frame delta equals the true start offset.
//!
//! The FIR coefficient golden test, the landmark hash-packing tests, and the CPF1 format/reject
//! tests live in the core lib unit tests, alongside the code they pin.
//!
//! Determinism (§3 invariant 2): every signal here is a pure function of its parameters; the
//! builder reads no RNG / hash order / wall clock, so two runs are byte-identical.

use couple_core::fingerprint::file::encode;
use couple_core::fingerprint::landmarks::{serialize_landmarks, Landmark};
use couple_core::fingerprint::params::FingerprintFormatV1;
use couple_core::fingerprint::peaks::{serialize_peaks, Peak};
use couple_core::fingerprint::FingerprintBuilder;

const SR: f64 = 48_000.0;

/// Deterministic broadband 48 kHz signal: a pink-ish bed + randomly-placed exponential-decay
/// broadband transients (sharp spectral events the peak picker locks onto), keyed to the
/// ABSOLUTE sample index so it is reproducible and pure. No RNG crate — a splitmix hash drives
/// both the carrier and the event plan.
fn signal(seed: u64, n: usize) -> Vec<f32> {
    // Event plan: ~3 transients/s with random center freqs, amplitudes, decay.
    struct Ev {
        t0: f64,
        freqs: [f64; 6],
        amp: f64,
        tau: f64,
    }
    let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x1234_5678);
    let mut rnd = || {
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 27;
        h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
        (h >> 11) as f64 / (1u64 << 53) as f64 // [0,1)
    };
    let dur = n as f64 / SR;
    let mut evs: Vec<Ev> = Vec::new();
    let mut t = 0.0;
    loop {
        t += -(rnd().max(1e-9)).ln() / 3.0; // Poisson, ~3/s
        if t >= dur {
            break;
        }
        let mut freqs = [0f64; 6];
        for f in &mut freqs {
            *f = 300.0 + rnd() * 5000.0; // inside the 100–5500 Hz band
        }
        evs.push(Ev {
            t0: t,
            freqs,
            amp: 0.3 + 0.6 * rnd(),
            tau: 0.03 + 0.15 * rnd(),
        });
    }

    // Carrier: a low broadband bed from a per-sample hash (decorrelated), plus the transients.
    (0..n)
        .map(|i| {
            let tt = i as f64 / SR;
            let mut z = (i as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
            z ^= z >> 29;
            z = z.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z ^= z >> 31;
            let bed = ((z >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 0.04;
            let mut s = bed;
            for e in &evs {
                let dt = tt - e.t0;
                if dt >= 0.0 && dt < 8.0 * e.tau {
                    let env = e.amp * (1.0 - (-dt / 0.0005).exp()) * (-dt / e.tau).exp();
                    let mut burst = 0.0;
                    for &f in &e.freqs {
                        burst += (2.0 * std::f64::consts::PI * f * dt).sin();
                    }
                    s += env * burst / (6f64).sqrt();
                }
            }
            s.clamp(-0.999, 0.999) as f32
        })
        .collect()
}

/// Peaks of a whole signal pushed all-at-once.
fn peaks_of(sig: &[f32]) -> Vec<Peak> {
    let mut b = FingerprintBuilder::new();
    b.push(sig);
    b.finalize()
}

/// Peaks of a signal pushed in the given repeating chunk sizes.
fn peaks_chunked(sig: &[f32], chunks: &[usize]) -> Vec<Peak> {
    let mut b = FingerprintBuilder::new();
    let mut i = 0;
    let mut ci = 0;
    while i < sig.len() {
        let len = chunks[ci % chunks.len()].max(1).min(sig.len() - i);
        b.push(&sig[i..i + len]);
        i += len;
        ci += 1;
    }
    b.finalize()
}

/// Landmarks of a whole signal pushed all-at-once.
fn landmarks_of(sig: &[f32]) -> Vec<Landmark> {
    let mut b = FingerprintBuilder::new();
    b.push(sig);
    b.finalize_landmarks()
}

/// Landmarks of a signal pushed in the given repeating chunk sizes.
fn landmarks_chunked(sig: &[f32], chunks: &[usize]) -> Vec<Landmark> {
    let mut b = FingerprintBuilder::new();
    let mut i = 0;
    let mut ci = 0;
    while i < sig.len() {
        let len = chunks[ci % chunks.len()].max(1).min(sig.len() - i);
        b.push(&sig[i..i + len]);
        i += len;
        ci += 1;
    }
    b.finalize_landmarks()
}

/// Encode a landmark set into CPF1 bytes with the fixed format and a fixed duration, so two
/// equal landmark sets must yield byte-identical artifacts (the §8.1 half of the gate).
fn cpf_bytes(landmarks: &[Landmark]) -> Vec<u8> {
    // Use a generous duration so every anchor_frame is in range regardless of clip length.
    encode(&FingerprintFormatV1::fixed(), u32::MAX as u64, landmarks)
}

// ---- Gate 1: chunk-boundary invariance at the PEAK level (§7.5.1) ----
#[test]
fn chunk_boundary_invariant_peaks_byte_identical() {
    let sig = signal(7, (8.0 * SR) as usize); // 8 s of broadband content

    let whole = serialize_peaks(&peaks_of(&sig));
    assert!(!whole.is_empty(), "8 s of broadband content must yield peaks");

    // (a) one sample at a time.
    let mut b = FingerprintBuilder::new();
    for &s in &sig {
        b.push(&[s]);
    }
    let one = serialize_peaks(&b.finalize());
    assert_eq!(whole, one, "peaks differ when pushed one sample at a time");

    // (b) deterministic irregular chunk sizes (mix of small/large, < and > the FFT window).
    for chunks in [
        &[1usize, 2, 3, 5, 7, 11, 13][..],
        &[64, 1, 1024, 17, 4096, 3][..],
        &[257, 31, 99, 1, 65536][..],
    ] {
        let chunked = serialize_peaks(&peaks_chunked(&sig, chunks));
        assert_eq!(
            whole, chunked,
            "peaks differ under chunking {chunks:?} (chunk-boundary invariance broken)"
        );
    }

    // Determinism across repeated whole runs (the §3 invariant-2 anchor).
    assert_eq!(whole, serialize_peaks(&peaks_of(&sig)), "two whole runs must be byte-identical");
}

// ---- Gate 1 (boundary 2): chunk-boundary invariance at the LANDMARK + .cpf level (§7.5.1) ----
//
// The boundary-1 gate proved BYTE-IDENTICAL PEAKS across chunkings. This extends the same
// premise through the production landmark pipeline: the same PCM pushed all-at-once, one sample
// at a time, and in deterministic random chunk sizes must yield BYTE-IDENTICAL serialized
// LANDMARK sets AND byte-identical encoded CPF1 artifacts. (Landmark generation is a pure
// function of the chunk-invariant peak list, and CPF1 encode is a pure function of the landmark
// list, so byte-identity propagates.)
#[test]
fn chunk_boundary_invariant_landmarks_and_cpf_byte_identical() {
    let sig = signal(7, (8.0 * SR) as usize); // 8 s of broadband content

    let whole_lms = landmarks_of(&sig);
    assert!(!whole_lms.is_empty(), "8 s of broadband content must yield landmarks");
    let whole = serialize_landmarks(&whole_lms);
    let whole_cpf = cpf_bytes(&whole_lms);

    // Landmarks are sorted by (hash, anchor_frame) (§7.4).
    assert!(
        whole_lms.windows(2).all(|w| (w[0].hash, w[0].anchor_frame) <= (w[1].hash, w[1].anchor_frame)),
        "landmark set must be sorted by (hash, anchor_frame)"
    );

    // (a) one sample at a time.
    let mut b = FingerprintBuilder::new();
    for &s in &sig {
        b.push(&[s]);
    }
    let one_lms = b.finalize_landmarks();
    assert_eq!(whole, serialize_landmarks(&one_lms), "landmarks differ pushed one sample at a time");
    assert_eq!(whole_cpf, cpf_bytes(&one_lms), ".cpf bytes differ pushed one sample at a time");

    // (b) deterministic irregular chunk sizes (mix of small/large, < and > the FFT window).
    for chunks in [
        &[1usize, 2, 3, 5, 7, 11, 13][..],
        &[64, 1, 1024, 17, 4096, 3][..],
        &[257, 31, 99, 1, 65536][..],
    ] {
        let chunked_lms = landmarks_chunked(&sig, chunks);
        assert_eq!(
            whole,
            serialize_landmarks(&chunked_lms),
            "landmarks differ under chunking {chunks:?} (landmark chunk-invariance broken)"
        );
        assert_eq!(
            whole_cpf,
            cpf_bytes(&chunked_lms),
            ".cpf bytes differ under chunking {chunks:?}"
        );
    }

    // Determinism across repeated whole runs (the §3 invariant-2 anchor, at the landmark level).
    assert_eq!(whole, serialize_landmarks(&landmarks_of(&sig)), "two whole landmark runs must match");
    assert_eq!(whole_cpf, cpf_bytes(&landmarks_of(&sig)), "two whole .cpf runs must match");
}

// ---- Gate 2: gain invariance over ±24 dB (§7.5.2) ----
//
// The peak SET (frame, freq_bin) must be stable when the input is scaled. The prominence test
// is in dB against a LOCAL median, so a uniform gain shifts every dB value by the same constant
// and the relative structure — hence the selected peaks — is unchanged. We give the source
// generous headroom (peak ≈ 0.06) so the FULL ±24 dB range stays inside ±1.0 with NO clipping;
// the gain is then a pure linear scale and the peak set must be BIT-for-bit identical across
// the whole range. (We compare the (frame, bin) identity — exactly what a landmark consumes.)
#[test]
fn gain_invariance_peak_set_stable() {
    // The generator's transients already peak well under full-scale; rescale to be safe so
    // +24 dB (×15.8) cannot clip.
    let raw = signal(11, (6.0 * SR) as usize);
    let peak = raw.iter().fold(0f32, |m, &s| m.max(s.abs())).max(1e-6);
    let headroom = 0.05 / peak; // scale so the source peaks at ~0.05 → +24 dB ≈ 0.79
    let sig: Vec<f32> = raw.iter().map(|&s| s * headroom).collect();

    let base: Vec<(u32, u32)> = peaks_of(&sig).iter().map(|p| (p.frame, p.freq_bin)).collect();
    assert!(base.len() > 20, "need a non-trivial peak set to test gain invariance");

    for db in [-24.0_f64, -18.0, -12.0, -6.0, 6.0, 12.0, 18.0, 24.0] {
        let g = 10f64.powf(db / 20.0) as f32;
        let scaled: Vec<f32> = sig.iter().map(|&s| s * g).collect();
        // No clipping in range, so the peak SET must be identical to the bit.
        assert!(scaled.iter().all(|&s| s.abs() < 1.0), "{db} dB must not clip in this test");
        let got: Vec<(u32, u32)> =
            peaks_of(&scaled).iter().map(|p| (p.frame, p.freq_bin)).collect();
        assert_eq!(got, base, "{db} dB linear gain must leave the peak set identical");
    }
}

// ---- Gate 3: deterministic peak tie-breaking (§7.5.3) ----
//
// The §7.3 equal-maxima rule (a flat time×freq plateau resolves to exactly one peak: lowest
// freq bin, then earliest frame) is driven directly over a crafted dB spectrogram in the
// crate-private `peaks` unit tests (where the selection fn is reachable). End-to-end here, a
// pure-tone burst produces a clean spectral ridge; the picker must keep a sparse, deterministic
// set with NO two peaks at the same (frame, bin), and two runs must be byte-identical.
#[test]
fn deterministic_tie_break_end_to_end() {
    let n = (3.0 * SR) as usize;
    let tone: Vec<f32> = (0..n)
        .map(|i| (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / SR).sin()) as f32)
        .collect();
    let peaks = peaks_of(&tone);
    let mut seen = std::collections::HashSet::new();
    for p in &peaks {
        assert!(seen.insert((p.frame, p.freq_bin)), "duplicate peak at {:?}", (p.frame, p.freq_bin));
    }
    // Deterministic across runs (the tie-break makes selection single-valued).
    assert_eq!(
        serialize_peaks(&peaks),
        serialize_peaks(&peaks_of(&tone)),
        "tie-broken peak selection must be byte-identical across runs"
    );
}

// ---- Gate 4: no peaks from digital silence (§7.5.4) ----
#[test]
fn silence_yields_no_peaks() {
    let n = (5.0 * SR) as usize;
    let silence = vec![0.0f32; n];
    let peaks = peaks_of(&silence);
    assert!(peaks.is_empty(), "digital silence must produce no peaks, got {}", peaks.len());

    // Near-silence at the f32 denormal floor is still silence to the picker (no prominence).
    let tiny = vec![1e-9f32; n];
    assert!(peaks_of(&tiny).is_empty(), "a DC near-zero level must produce no peaks");
}

// ---- Gate 5: bounded peak density on a sustained periodic tone (§7.5.5) ----
//
// A sustained tone is the periodic-content hazard: its spectrum is identical frame after frame,
// so without a cap the picker would emit a peak every frame at the tone bin. The per-second cap
// must bound the count in EVERY one-second window by `max_peaks_per_second`.
#[test]
fn sustained_tone_density_bounded() {
    let fmt = FingerprintFormatV1::fixed();
    let dur_s = 10.0;
    let n = (dur_s * SR) as usize;
    // A two-tone sustained signal (two stable partials) — a dense periodic source.
    let tone: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / SR;
            (0.4 * (2.0 * std::f64::consts::PI * 900.0 * t).sin()
                + 0.3 * (2.0 * std::f64::consts::PI * 2300.0 * t).sin()) as f32
        })
        .collect();
    let peaks = peaks_of(&tone);

    // Frames per second of analysis: 12000/256 ≈ 46.875 → 47 frames.
    let fps = (fmt.analysis_sample_rate as f64 / fmt.hop_size as f64).round() as u32;
    // Check EVERY sliding one-second window [f, f+fps).
    let last = peaks.iter().map(|p| p.frame).max().unwrap_or(0);
    for start in 0..=last {
        let count = peaks
            .iter()
            .filter(|p| p.frame >= start && p.frame < start + fps)
            .count();
        assert!(
            count <= fmt.max_peaks_per_second,
            "one-second window [{start}, {}) holds {count} peaks > cap {}",
            start + fps,
            fmt.max_peaks_per_second
        );
    }
    // And the cap actually bit (a sustained tone WOULD exceed it uncapped): the overall density
    // is well under one-peak-per-frame.
    assert!(
        (peaks.len() as f64) < (last as f64 + 1.0),
        "sustained tone should be sparsified by the cap, not one-peak-per-frame"
    );
}

// Performance probe (ignored by default; run with `--ignored`): wall-time to fingerprint a 60 s
// and a 600 s clip on this machine, plus peak counts. Reported in PROGRESS, not a gate.
#[test]
#[ignore]
fn perf_probe_60s_600s() {
    for secs in [60.0_f64, 600.0] {
        let n = (secs * SR) as usize;
        let sig = signal(7, n);
        let t0 = std::time::Instant::now();
        let peaks = peaks_of(&sig);
        let dt = t0.elapsed();
        eprintln!(
            "PERF {secs:.0}s clip: {:?} wall, {} peaks ({:.1} peaks/s), {:.1}x realtime",
            dt,
            peaks.len(),
            peaks.len() as f64 / secs,
            secs / dt.as_secs_f64()
        );
    }
}

// A different random event has a peak set largely DISJOINT from another — the spectral-identity
// premise. (The strong cross-event hash-overlap gate, §7.5.7, is a boundary-2 LANDMARK test;
// at the peak level we only assert the picks are event-specific, not coincidental.)
#[test]
fn distinct_events_have_distinct_peaks() {
    let a = peaks_of(&signal(1, (6.0 * SR) as usize));
    let b = peaks_of(&signal(2, (6.0 * SR) as usize));
    let sa: std::collections::HashSet<(u32, u32)> =
        a.iter().map(|p| (p.frame, p.freq_bin)).collect();
    let overlap = b.iter().filter(|p| sa.contains(&(p.frame, p.freq_bin))).count();
    let frac = overlap as f64 / b.len().max(1) as f64;
    assert!(frac < 0.30, "distinct events share {:.0}% of exact peak cells — too high", frac * 100.0);
}

// ============================================================================================
// Boundary 2 §7.5 LANDMARK tests (6, 7, 8). These operate on landmark HASHES, the matching
// substrate. Helpers below summarize a landmark set as a hash multiset / hash set and compute
// the dominant anchor-frame delta between two sets that share an event.
// ============================================================================================

/// The set of distinct landmark hashes present.
fn hash_set(landmarks: &[Landmark]) -> std::collections::HashSet<u32> {
    landmarks.iter().map(|l| l.hash).collect()
}

// ---- Gate 6 (§7.5.6): landmarks STABLE under AGC and moderate filtering ----
//
// A recording captured through a different mic/AGC/filter chain must still share a STRONG
// MAJORITY of landmark hashes with the clean capture (that is what lets the matcher identify it).
// We degrade the clean master with BOTH a slow time-varying gain (AGC) AND a mild one-pole
// low-pass (a moderate EQ/band-limit), then require that a strong majority of the CLEAN hash set
// still appears in the degraded set. (Filtering perturbs some peaks, so the survival is high but
// not total — exactly the tolerance the quantized hash is designed to provide.)
#[test]
fn landmarks_stable_under_agc_and_filtering() {
    let master = signal(21, (12.0 * SR) as usize);
    let clean = hash_set(&landmarks_of(&master));
    assert!(clean.len() > 200, "need a rich clean landmark set, got {}", clean.len());

    // Degrade with BOTH a realistic SLOW AGC (a 30 s-period gain envelope swinging ≈ ×0.7..×1.3 —
    // slow relative to the 1-second peak cap, the way a real leveler behaves) AND a mild one-pole
    // low-pass (alpha 0.8 ≈ a soft top-end roll-off, a moderate EQ, not a brick wall). Together
    // these are the "different mic / AGC / filter chain" §7.5.6 targets. A fast/deep compressor is
    // a heavier degradation than "moderate" and is out of scope here.
    let mut degraded = vec![0f32; master.len()];
    let mut lp = 0.0f64; // one-pole low-pass state
    let alpha = 0.8;
    for (i, &x) in master.iter().enumerate() {
        let t = i as f64 / SR;
        let agc = 1.0 + 0.3 * (2.0 * std::f64::consts::PI * t / 30.0).sin();
        lp = alpha * x as f64 + (1.0 - alpha) * lp;
        degraded[i] = (lp * agc).clamp(-0.999, 0.999) as f32;
    }
    let deg = hash_set(&landmarks_of(&degraded));

    let survived = clean.iter().filter(|h| deg.contains(h)).count();
    let frac = survived as f64 / clean.len() as f64;
    // A strong majority must survive. The measured rate on this signal is ≈70%; the gate floor is
    // set comfortably below that so it is robust to f32/platform jitter while still demanding a
    // strong majority (and staying far above the <15% a DIFFERENT event shows in gate 7).
    assert!(
        frac >= 0.60,
        "only {:.0}% of clean landmark hashes survived AGC+filtering (need a strong majority ≥60%)",
        frac * 100.0
    );
}

// ---- Gate 7 (§7.5.7): a DIFFERENT random event has NEGLIGIBLE exact-hash overlap ----
//
// Two independently generated events must NOT look alike at the hash level, or the matcher would
// hallucinate identity from coincidence. We require the shared-hash fraction (relative to the
// smaller set) to be small. Hashes are only ~26 effective bits, so a little chance collision is
// expected; "negligible" here is well under the strong-majority survival a true match shows
// (gate 6), giving a wide separation between same-event and different-event.
#[test]
fn distinct_events_negligible_hash_overlap() {
    let a = hash_set(&landmarks_of(&signal(101, (12.0 * SR) as usize)));
    let b = hash_set(&landmarks_of(&signal(202, (12.0 * SR) as usize)));
    assert!(a.len() > 200 && b.len() > 200, "need rich sets: {} / {}", a.len(), b.len());

    let shared = a.iter().filter(|h| b.contains(h)).count();
    let denom = a.len().min(b.len());
    let frac = shared as f64 / denom as f64;
    assert!(
        frac < 0.15,
        "distinct events share {:.1}% of exact hashes — not negligible (shared {shared}/{denom})",
        frac * 100.0
    );
}

// ---- Gate 8 (§7.5.8): two excerpts of the SAME master share hashes at the CORRECT offset ----
//
// The matching premise: two recordings of one acoustic event, started at different times, share
// many landmark hashes, and for a shared hash the anchor-frame DELTA is the true start offset.
// `signal(seed, n)` keys content to the ABSOLUTE sample index, so slicing the same master at two
// 48 kHz offsets yields two genuine excerpts of one event. We choose the excerpt offset as an
// exact multiple of one analysis frame (1024 samples @48k = hop·4 = 256 @12k) so the true frame
// delta is an integer, then require the DOMINANT anchor-frame delta among shared hashes to equal
// that offset (and to dominate decisively).
#[test]
fn same_master_excerpts_share_hashes_at_offset() {
    let total = (30.0 * SR) as usize;
    let master = signal(33, total);

    // One analysis frame at 48 kHz = hop_size(256) · decim(4) = 1024 samples. Offset the second
    // excerpt by a whole number of frames so the true delta is exact.
    let off_frames = 200u32; // 200 frames ≈ 4.27 s
    let off_samples_48k = off_frames as usize * 1024;
    let excerpt_len = (20.0 * SR) as usize;
    assert!(off_samples_48k + excerpt_len <= total, "excerpt B must fit inside the master");

    let a = &master[0..excerpt_len];
    let b = &master[off_samples_48k..off_samples_48k + excerpt_len];

    let la = landmarks_of(a);
    let lb = landmarks_of(b);
    assert!(la.len() > 200 && lb.len() > 200, "need rich excerpt sets: {} / {}", la.len(), lb.len());

    // Index B's anchor frames by hash. For every A landmark whose hash appears in B, record the
    // delta (anchor_frame_A − anchor_frame_B). A landmark anchored at master frame f appears in A
    // near frame f and in B near frame f − off_frames, so the delta clusters at +off_frames.
    let mut b_by_hash: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for l in &lb {
        b_by_hash.entry(l.hash).or_default().push(l.anchor_frame);
    }
    let mut delta_hist: std::collections::HashMap<i64, u32> = std::collections::HashMap::new();
    let mut shared_pairs = 0u64;
    for l in &la {
        if let Some(bs) = b_by_hash.get(&l.hash) {
            for &bf in bs {
                let d = l.anchor_frame as i64 - bf as i64;
                *delta_hist.entry(d).or_insert(0) += 1;
                shared_pairs += 1;
            }
        }
    }
    assert!(shared_pairs > 100, "too few shared hash pairs ({shared_pairs}) to recover an offset");

    // The dominant delta must equal the true frame offset, and dominate decisively. We accept a
    // ±1 frame slop bucket around the true offset (excerpt edges + group delay can nudge a few
    // anchors by a frame), and require that bucket to hold a strong plurality of all shared pairs.
    let true_delta = off_frames as i64;
    let near: u32 = (-1..=1).map(|dd| *delta_hist.get(&(true_delta + dd)).unwrap_or(&0)).sum();
    let (best_delta, best_count) =
        delta_hist.iter().max_by_key(|(_, &c)| c).map(|(&d, &c)| (d, c)).unwrap();

    assert!(
        (best_delta - true_delta).abs() <= 1,
        "dominant anchor-frame delta {best_delta} != true offset {true_delta} (hist peak {best_count})"
    );
    let frac = near as f64 / shared_pairs as f64;
    assert!(
        frac >= 0.40,
        "true-offset delta bucket holds only {:.0}% of shared pairs — not a decisive peak",
        frac * 100.0
    );
}
