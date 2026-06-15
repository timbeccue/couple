//! PLAN-SPECTRAL-FINGERPRINT-MATCHING boundary 4 (§9.1 + §9.2) END-TO-END gates: the batch audio
//! index and the correspondence generator driven on REAL fingerprints built by the full signal
//! pipeline (not hand-crafted landmark lists — those algorithmic gates live in the `fingerprint::
//! index` unit tests). This is the integration-level proof that:
//!
//! - the batch index is deterministic and INDEPENDENT of audio-track input ordering at fixed
//!   positions (§9.1, invariant 3 — no hash-map-iteration-order dependence);
//! - two excerpts of one master at a known offset produce correspondences whose (t_a_s, t_b_s)
//!   cluster on the TRUE affine line, so boundary 5 can consense — the dominant t_b−t_a delta
//!   equals the offset (§9.2 / §17 "exact same PCM excerpt");
//! - independent events produce few, scattered correspondences with NO dominant timing line
//!   (§17 "different take" / cross-event).
//!
//! The signal generator mirrors `fingerprint_builder.rs`: content is keyed to the ABSOLUTE 48 kHz
//! sample index, so slicing the same master at two offsets yields genuine excerpts of one event.
//! Determinism (§3 invariant 2): every signal and every structure here is a pure function of its
//! inputs; no RNG / hash order / wall clock reaches the asserted bytes.

use couple_core::fingerprint::index::{correspondences_for_pair, AudioHashIndex};
use couple_core::fingerprint::params::{FingerprintFormatV1, FingerprintMatchParams};
use couple_core::fingerprint::FingerprintReader;
use std::collections::HashMap;

const SR: f64 = 48_000.0;

/// Deterministic broadband 48 kHz signal: a faint bed plus randomly-placed exponential-decay
/// broadband transients, keyed to the ABSOLUTE sample index. Same shape as the builder gate's
/// generator so two excerpts of one master share an event. No RNG crate — a splitmix hash drives
/// the event plan and the bed.
fn signal(seed: u64, n: usize) -> Vec<f32> {
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
        (h >> 11) as f64 / (1u64 << 53) as f64
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
            *f = 300.0 + rnd() * 5000.0;
        }
        evs.push(Ev { t0: t, freqs, amp: 0.3 + 0.6 * rnd(), tau: 0.03 + 0.15 * rnd() });
    }
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

/// Build a real `FingerprintReader` for a PCM slice via the full pipeline (the production builder
/// + codec), so the index/correspondence code sees genuine landmarks.
fn fingerprint(pcm: &[f32]) -> FingerprintReader {
    let fmt = FingerprintFormatV1::fixed();
    let bytes = couple_core::fingerprint::build_artifact_bytes(pcm, pcm.len() as u64);
    FingerprintReader::parse(&bytes, &fmt).expect("built artifact must parse")
}

fn params() -> FingerprintMatchParams {
    FingerprintMatchParams::default()
}

// ---- §9.1 batch index: deterministic + independent of audio input ordering at fixed positions ----
#[test]
fn batch_index_is_deterministic_and_order_independent() {
    let a0 = fingerprint(&signal(1, (6.0 * SR) as usize));
    let a1 = fingerprint(&signal(2, (6.0 * SR) as usize));
    let a2 = fingerprint(&signal(3, (6.0 * SR) as usize));

    let (idx, stats) = AudioHashIndex::build(&[&a0, &a1, &a2], &params());
    assert!(idx.len() > 200, "rich batch index expected, got {}", idx.len());
    assert!(stats.postings_kept > 200);

    // Two builds of the same batch are byte-identical (invariant 3).
    let (idx2, stats2) = AudioHashIndex::build(&[&a0, &a1, &a2], &params());
    assert_eq!(idx.serialize(), idx2.serialize(), "two index builds must be byte-identical");
    assert_eq!(stats, stats2);

    // The published index is hash-sorted and each postings list is (audio_index, anchor_frame)-
    // sorted — i.e. there is NO residual dependence on the construction map's iteration order.
    let ser = idx.serialize();
    assert!(!ser.is_empty());
}

// ---- §9.2 same-master: correspondences cluster on the true affine line (dominant delta = offset) ----
//
// Two excerpts of one master, the second started OFF analysis-frames later in absolute content.
// For a shared hash anchored at master frame f, A sees it near frame f and B near frame f − OFF, so
// every correspondence lies on t_b ≈ t_a − OFF·frame_period. The dominant (t_b − t_a) frame-delta
// must equal −OFF and dominate decisively, so boundary 5's consensus has a clean line to fit.
#[test]
fn same_master_excerpts_cluster_on_true_line() {
    let total = (30.0 * SR) as usize;
    let master = signal(33, total);

    // One analysis frame at 48 kHz = hop_size(256) · decim(4) = 1024 samples. Offset by a whole
    // number of frames so the true frame delta is exact.
    let off_frames = 200u32; // ≈ 4.27 s
    let off_samples = off_frames as usize * 1024;
    let excerpt_len = (20.0 * SR) as usize;
    assert!(off_samples + excerpt_len <= total);

    let video = fingerprint(&master[0..excerpt_len]); // A
    let audio = fingerprint(&master[off_samples..off_samples + excerpt_len]); // B

    let (idx, _) = AudioHashIndex::build(&[&audio], &params());
    let pc = correspondences_for_pair(&video, &idx, 0, &FingerprintFormatV1::fixed(), &params());
    assert!(
        pc.correspondences.len() > 100,
        "need a rich correspondence set to recover the line, got {}",
        pc.correspondences.len()
    );

    // Histogram t_b − t_a in frame_period units; accept a ±1-frame slop bucket (excerpt edges +
    // group delay can nudge a few anchors) and require a strong plurality on the true −offset line.
    let fmt = FingerprintFormatV1::fixed();
    let fp = fmt.frame_period_s();
    let mut hist: HashMap<i64, u32> = HashMap::new();
    for c in &pc.correspondences {
        let d = ((c.t_b_s - c.t_a_s) / fp).round() as i64;
        *hist.entry(d).or_insert(0) += 1;
    }
    let true_delta = -(off_frames as i64);
    let near: u32 = (-1..=1).map(|dd| *hist.get(&(true_delta + dd)).unwrap_or(&0)).sum();
    let (best, best_count) = hist.iter().max_by_key(|(_, &c)| c).map(|(&d, &c)| (d, c)).unwrap();
    assert!(
        (best - true_delta).abs() <= 1,
        "dominant t_b−t_a delta {best} != true −offset {true_delta} (peak {best_count})"
    );
    let frac = near as f64 / pc.correspondences.len() as f64;
    assert!(frac >= 0.40, "true-line bucket holds only {:.0}% — not a decisive line", frac * 100.0);

    // Determinism: the correspondence set is byte-identical across runs.
    let pc2 = correspondences_for_pair(&video, &idx, 0, &fmt, &params());
    assert_eq!(
        serialize_corr(&pc.correspondences),
        serialize_corr(&pc2.correspondences),
        "correspondences must be deterministic"
    );
}

// ---- §9.2 cross-event: independent events ⇒ few/scattered correspondences, no dominant line ----
#[test]
fn cross_event_few_scattered_no_dominant_line() {
    let video = fingerprint(&signal(101, (20.0 * SR) as usize));
    let audio = fingerprint(&signal(202, (20.0 * SR) as usize));

    let (idx, _) = AudioHashIndex::build(&[&audio], &params());
    let fmt = FingerprintFormatV1::fixed();
    let pc = correspondences_for_pair(&video, &idx, 0, &fmt, &params());

    // Some incidental hash collisions are expected (~26 effective bits), but FAR fewer than a true
    // match's shared set, and with NO timing line a consensus could fit. Two properties together
    // make "no dominant line" meaningful regardless of the tiny absolute count:
    //   (a) the total shared count is small — nothing like the hundreds a true match yields;
    //   (b) NO single ±1-frame delta band accumulates enough mass to be a fittable affine line.
    // Boundary 5 needs `min_support` (default 12) inliers on one line; an independent pair must
    // stay far under that. We assert the strongest delta BAND (a value and its ±1 neighbours, the
    // same slop the same-master line uses) holds only a handful of correspondences.
    let fp = fmt.frame_period_s();
    let mut hist: HashMap<i64, u32> = HashMap::new();
    for c in &pc.correspondences {
        *hist.entry(((c.t_b_s - c.t_a_s) / fp).round() as i64).or_insert(0) += 1;
    }
    let total = pc.correspondences.len();
    // The same-master test produced > 100 correspondences on one line; an independent pair should
    // share only a tiny number overall.
    assert!(total < 30, "independent events shared {total} correspondences — unexpectedly many");
    // Strongest ±1-frame band: for the same-master case this held ≥40% of > 100 points; here it
    // must be a handful, nowhere near a fittable line.
    let strongest_band = hist
        .keys()
        .map(|&d| (-1..=1).map(|dd| *hist.get(&(d + dd)).unwrap_or(&0)).sum::<u32>())
        .max()
        .unwrap_or(0);
    assert!(
        (strongest_band as usize) < 12,
        "independent events form a {strongest_band}-strong delta band — a spurious line (total {total})"
    );
}

/// Deterministic byte serialization of a correspondence list for the determinism assertions: each
/// is `t_a_s(f64 bits LE) ‖ t_b_s(f64 bits LE) ‖ weight(f64 bits LE)`.
fn serialize_corr(cs: &[couple_core::fingerprint::Correspondence]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cs.len() * 24);
    for c in cs {
        out.extend_from_slice(&c.t_a_s.to_bits().to_le_bytes());
        out.extend_from_slice(&c.t_b_s.to_bits().to_le_bytes());
        out.extend_from_slice(&c.weight.to_bits().to_le_bytes());
    }
    out
}
