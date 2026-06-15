//! Deterministic 48 kHz → 12 kHz downsampler for fingerprint analysis (§7.1).
//!
//! Two fixed 2:1 half-band FIR stages (48k→24k→12k), each a symmetric 63-tap Kaiser
//! (beta 8.6) filter whose coefficients are the checked-in [`super::coeffs`] table generated
//! by `core/src/bin/gen_halfband.rs`. Every even-offset tap except the center is an exact
//! half-band zero, so each stage performs only the 17 non-zero multiplies per output sample
//! (§7.1 "only the non-zero multiplies").
//!
//! Streaming + chunk-boundary invariant (§7.2): [`HalfBandDecimator::push`] keeps a sliding
//! input history with a base index, and emits output sample `m` exactly when enough INPUT has
//! arrived for its centered window — a function only of the cumulative input count, never of
//! how the input was chunked. Pushing the same PCM all-at-once, one sample at a time, or in
//! arbitrary chunks yields the identical output stream.
//!
//! Determinism (§3 invariant 2): pure f64 arithmetic over the input slice and the constant
//! taps; no RNG, hash iteration, or wall clock.
//!
//! Group delay (§7.1 "account for cumulative group delay consistently"): a length-`N_TAPS`
//! symmetric FIR has group delay `HALF` samples at ITS input rate. We center output `m` on
//! input index `2·m + HALF`, so:
//!
//! - the 24 kHz sample `m₁` sits at 48 kHz time `(2·m₁ + HALF)/48000 = m₁/24000 + HALF/48000`;
//! - the 12 kHz sample `m₂` sits at 24 kHz center `2·m₂ + HALF`, hence 48 kHz time
//!   `m₂/12000 + HALF/24000 + HALF/48000`.
//!
//! So a 12 kHz output index `m₂` maps to the original 48 kHz timeline at
//! `t = m₂/12000 + GROUP_DELAY_S`, with the cumulative
//! `GROUP_DELAY_S = HALF/24000 + HALF/48000` (≈ 1.9375 ms). [`Downsampler::group_delay_s`]
//! exposes it so the builder can place STFT frame times on the original timeline. (In PAIR
//! matching the constant delay is identical for both tracks and cancels in the relative lag;
//! carrying it keeps absolute frame times correct regardless.)

use super::coeffs::{HALF, HALFBAND_NONZERO, N_TAPS};
use std::collections::VecDeque;

// Structural invariant the decimator relies on: the symmetric filter has `N_TAPS` taps with
// center index `HALF`, so `N_TAPS == 2·HALF + 1`. A compile-time check ties the checked-in
// `N_TAPS` to `HALF` (and keeps the generated constant load-bearing).
const _: () = assert!(N_TAPS == 2 * HALF + 1, "half-band tap count must be 2·HALF + 1");

/// One streaming 2:1 half-band decimation stage. Holds a sliding window of recent input
/// samples (as f64 for the multiply-accumulate) plus the absolute index of the oldest
/// retained sample, so output emission depends only on the cumulative input count.
struct HalfBandDecimator {
    /// Recent input samples; `hist[0]` is absolute input index `base`.
    hist: VecDeque<f64>,
    /// Absolute input index of `hist.front()`.
    base: u64,
    /// Total input samples seen (absolute next index = `n_in`).
    n_in: u64,
    /// Number of output samples already emitted.
    n_out: u64,
}

impl HalfBandDecimator {
    fn new() -> Self {
        Self {
            hist: VecDeque::new(),
            base: 0,
            n_in: 0,
            n_out: 0,
        }
    }

    /// Feed `input` (this stage's rate) and append every newly-completable decimated output to
    /// `out`. Output `m` is centered on input index `2·m + HALF` and needs inputs through
    /// `2·m + 2·HALF`; it is emitted as soon as `n_in` passes that index — independent of how
    /// `input` was chunked across calls.
    fn push(&mut self, input: &[f64], out: &mut Vec<f64>) {
        for &x in input {
            self.hist.push_back(x);
            self.n_in += 1;
        }

        // Emit every output whose full centered window has arrived.
        let half = HALF as u64;
        loop {
            let center = 2 * self.n_out + half; // input index this output centers on
            let last_needed = center + half; // highest input index its window touches
            if last_needed >= self.n_in {
                break; // not enough input yet
            }
            // Window spans absolute input indices [center - HALF, center + HALF] =
            // [2·n_out, 2·n_out + 2·HALF]. Exploit the symmetric half-band zeros: for each
            // non-zero tap at offset `off`, x[center-off] and x[center+off] share h[off]
            // (off=0 is the center, counted once).
            let mut acc = 0.0;
            for &(off, h) in HALFBAND_NONZERO.iter() {
                let off = off as u64;
                if off == 0 {
                    acc += h * self.at(center);
                } else {
                    acc += h * (self.at(center - off) + self.at(center + off));
                }
            }
            out.push(acc);
            self.n_out += 1;

            // Drop history strictly before the next output's window start. The next output is
            // `n_out`, whose window starts at absolute index `2·n_out`.
            let next_start = 2 * self.n_out;
            while self.base < next_start {
                self.hist.pop_front();
                self.base += 1;
            }
        }
    }

    /// Absolute-indexed history read (the sample must still be retained, which it always is for
    /// the window of the current output — the drop logic only removes indices below the next
    /// window start).
    fn at(&self, abs_idx: u64) -> f64 {
        self.hist[(abs_idx - self.base) as usize]
    }
}

/// Streaming 48 kHz → 12 kHz downsampler: two cascaded half-band stages. `push` accepts
/// canonical mono 48 kHz PCM in any chunking and appends the produced 12 kHz samples.
pub struct Downsampler {
    stage1: HalfBandDecimator, // 48k → 24k
    stage2: HalfBandDecimator, // 24k → 12k
    /// Scratch for stage-1 output handed to stage 2 (reused to avoid per-call allocation).
    mid: Vec<f64>,
}

impl Default for Downsampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Downsampler {
    pub fn new() -> Self {
        Self {
            stage1: HalfBandDecimator::new(),
            stage2: HalfBandDecimator::new(),
            mid: Vec::new(),
        }
    }

    /// Cumulative group delay, seconds: the 48 kHz time of 12 kHz output sample `m` is
    /// `m / 12000 + group_delay_s()`. Constant `HALF/24000 + HALF/48000` (≈ 1.9375 ms).
    pub fn group_delay_s() -> f64 {
        HALF as f64 / 24_000.0 + HALF as f64 / 48_000.0
    }

    /// Feed 48 kHz samples; append produced 12 kHz samples to `out`. Streaming and
    /// chunk-boundary invariant.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<f64>) {
        // Stage 1: 48k → 24k into the reused `mid` scratch.
        self.mid.clear();
        // Convert to f64 once; the stage works in f64 to match the coefficient precision.
        // (A small temporary is fine; chunk size is the caller's, typically a decode block.)
        let in64: Vec<f64> = samples.iter().map(|&s| s as f64).collect();
        self.stage1.push(&in64, &mut self.mid);
        // Stage 2: 24k → 12k.
        let mid = std::mem::take(&mut self.mid);
        self.stage2.push(&mid, out);
        self.mid = mid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Coefficient golden test (§7.1) ----
    //
    // Re-derive the 63-tap half-band Kaiser(8.6) coefficients with the SAME math as the
    // generator (`core/src/bin/gen_halfband.rs`) and assert byte-equality (to_bits) against the
    // checked-in `coeffs::HALFBAND_NONZERO`. This pins the table: any drift in the generator OR
    // a hand-edit of the constants fails here.

    const BETA: f64 = 8.6;

    fn bessel_i0(x: f64) -> f64 {
        let mut sum = 1.0;
        let mut term = 1.0;
        let half_x = x / 2.0;
        for k in 1..=40 {
            term *= (half_x * half_x) / (k as f64 * k as f64);
            sum += term;
        }
        sum
    }

    fn sinc(x: f64) -> f64 {
        if x.abs() < 1e-15 {
            1.0
        } else {
            let px = std::f64::consts::PI * x;
            px.sin() / px
        }
    }

    /// Regenerate the full 63-tap window with the generator's exact procedure.
    fn regen_taps() -> Vec<f64> {
        let n_taps = 2 * HALF + 1;
        let half = HALF as i64;
        let i0_beta = bessel_i0(BETA);
        let mut taps = vec![0f64; n_taps];
        for (idx, tap) in taps.iter_mut().enumerate() {
            let n = idx as i64 - half;
            let ideal = 0.5 * sinc(n as f64 / 2.0);
            let r = n as f64 / half as f64;
            let w = bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
            *tap = ideal * w;
        }
        for (idx, tap) in taps.iter_mut().enumerate() {
            let n = idx as i64 - half;
            if n != 0 && n % 2 == 0 {
                *tap = 0.0;
            }
        }
        taps
    }

    #[test]
    fn coefficient_golden() {
        // The checked-in tap count matches the generator's N_TAPS (63) and HALF (31).
        assert_eq!(super::super::coeffs::N_TAPS, 2 * HALF + 1);
        assert_eq!(super::super::coeffs::N_TAPS, 63);
        let taps = regen_taps();
        // Rebuild the (offset, coeff) non-zero list exactly as the generator does.
        let mut expected: Vec<(usize, f64)> = Vec::new();
        for off in 0..=HALF {
            let v = taps[HALF + off];
            if v != 0.0 {
                expected.push((off, v));
            }
        }
        assert_eq!(
            expected.len(),
            HALFBAND_NONZERO.len(),
            "non-zero tap count drifted from the generator"
        );
        for (i, (&(eoff, ev), &(coff, cv))) in
            expected.iter().zip(HALFBAND_NONZERO.iter()).enumerate()
        {
            assert_eq!(eoff, coff, "tap {i} offset drifted");
            assert_eq!(
                ev.to_bits(),
                cv.to_bits(),
                "tap {i} (offset {coff}) coefficient drifted: golden {cv:?} vs checked-in {ev:?}"
            );
        }
        // Structural facts the rest of the module relies on.
        assert_eq!(HALFBAND_NONZERO[0], (0, 0.5), "center tap must be (0, 0.5)");
        // 17 non-zero = center + 16 odd offsets (1,3,…,31).
        assert_eq!(HALFBAND_NONZERO.len(), 17);
        for &(off, _) in HALFBAND_NONZERO.iter().skip(1) {
            assert_eq!(off % 2, 1, "every non-center non-zero tap sits at an odd offset");
        }
    }

    // The windowed half-band has ~unity DC gain (analytic half-band sums to 1; the Kaiser
    // window adds a tiny gain). Full DC sum = center + 2·Σ(positive-offset taps).
    #[test]
    fn dc_gain_near_unity() {
        let dc: f64 = HALFBAND_NONZERO
            .iter()
            .map(|&(off, h)| if off == 0 { h } else { 2.0 * h })
            .sum();
        assert!((dc - 1.0).abs() < 1e-5, "half-band DC gain {dc} should be ≈ 1");
    }

    // ---- Chunk-boundary invariance of the decimator itself (§7.2 foundation) ----
    //
    // The same 48 kHz signal pushed all-at-once vs one sample at a time vs deterministic random
    // chunks must yield the byte-identical 12 kHz stream. (The full peak-level invariance test
    // lives in the integration suite; this is the unit-level anchor on the resampler.)

    fn test_signal(n: usize) -> Vec<f32> {
        // A deterministic mix of tones + a pseudo-random component, no RNG crate needed.
        (0..n)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                let s = 0.3 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()
                    + 0.2 * (2.0 * std::f64::consts::PI * 1234.0 * t).sin()
                    + 0.1 * (2.0 * std::f64::consts::PI * 3001.0 * t).sin();
                // deterministic dither from a hash of the index
                let mut z = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                z ^= z >> 29;
                let d = (z >> 40) as f64 / (1u64 << 24) as f64 - 0.5;
                (s + 0.05 * d) as f32
            })
            .collect()
    }

    fn downsample_all(sig: &[f32]) -> Vec<f64> {
        let mut d = Downsampler::new();
        let mut out = Vec::new();
        d.push(sig, &mut out);
        out
    }

    fn downsample_chunked(sig: &[f32], chunks: &[usize]) -> Vec<f64> {
        let mut d = Downsampler::new();
        let mut out = Vec::new();
        let mut i = 0;
        let mut ci = 0;
        while i < sig.len() {
            let len = chunks[ci % chunks.len()].max(1).min(sig.len() - i);
            d.push(&sig[i..i + len], &mut out);
            i += len;
            ci += 1;
        }
        out
    }

    #[test]
    fn decimator_chunk_invariant() {
        let sig = test_signal(50_000);
        let whole = downsample_all(&sig);

        // One sample at a time.
        let mut d = Downsampler::new();
        let mut single = Vec::new();
        for &s in &sig {
            d.push(&[s], &mut single);
        }
        assert_eq!(whole.len(), single.len(), "one-at-a-time changed the output length");
        for (i, (a, b)) in whole.iter().zip(&single).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "sample {i} differs one-at-a-time");
        }

        // Deterministic irregular chunk sizes (coprime-ish, includes < and > the FIR length).
        let chunks = [1usize, 7, 64, 3, 257, 13, 1000, 31];
        let chunked = downsample_chunked(&sig, &chunks);
        assert_eq!(whole.len(), chunked.len(), "chunking changed the output length");
        for (i, (a, b)) in whole.iter().zip(&chunked).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "sample {i} differs under chunking");
        }
    }

    // The 2:1·2:1 decimation ratio holds: ~N/4 output samples for N input (minus the fixed
    // filter warm-up). Confirms the group-delay accounting maps output count sensibly.
    #[test]
    fn output_rate_is_quarter() {
        let n = 48_000; // 1 s
        let out = downsample_all(&test_signal(n));
        // Each stage loses ~HALF samples of warm-up; final count ≈ n/4 − a small constant.
        let expected = n / 4;
        assert!(
            (out.len() as i64 - expected as i64).abs() < 64,
            "12 kHz output {} not ≈ n/4 = {expected}",
            out.len()
        );
    }

    // A pure tone inside the 12 kHz passband survives; a tone above 6 kHz (the 12 kHz Nyquist)
    // is strongly attenuated by the cascade (anti-alias). RMS-based, not bit-exact.
    #[test]
    fn passband_keeps_stopband_rejects() {
        let n = 48_000;
        let tone = |f: f64| -> Vec<f32> {
            (0..n)
                .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / 48_000.0).sin() as f32)
                .collect()
        };
        let rms = |v: &[f64]| -> f64 {
            // Skip warm-up edges.
            let s = &v[v.len() / 8..v.len() * 7 / 8];
            (s.iter().map(|x| x * x).sum::<f64>() / s.len() as f64).sqrt()
        };
        let pass = rms(&downsample_all(&tone(1000.0))); // well inside passband
        let stop = rms(&downsample_all(&tone(9000.0))); // above 6 kHz Nyquist → must alias-reject
        assert!(pass > 0.5, "1 kHz passband RMS {pass} should be near the input 0.707");
        assert!(stop < 0.02, "9 kHz stopband RMS {stop} should be deeply attenuated");
        assert!(pass / stop > 50.0, "passband/stopband ratio {} too low", pass / stop);
    }
}
