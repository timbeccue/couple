//! Generator for the fingerprint half-band decimation FIR coefficients
//! (PLAN-SPECTRAL-FINGERPRINT-MATCHING §7.1).
//!
//! This is the ONE documented helper that produces the checked-in
//! `core/src/fingerprint/coeffs.rs` constants. It is committed so the coefficients are
//! reproducible, and the COEFFICIENT GOLDEN TEST in `fingerprint::downsample` re-derives the
//! taps with this exact math and asserts byte-equality against the checked-in table — so the
//! constants can never silently drift from the generator.
//!
//! Design: a symmetric 63-tap half-band FIR, Kaiser window beta 8.6.
//!
//! - Ideal half-band lowpass with cutoff at ωc = π/2 (quarter of the input sample rate, i.e.
//!   the output Nyquist): `h_ideal[n] = 0.5 · sinc(n/2)`, where `sinc(x) = sin(πx)/(πx)` and
//!   `n` runs symmetrically over `−31..=31`.
//! - The HALF-BAND property: `sinc(n/2) = 0` for every even `n ≠ 0`, so all even-offset taps
//!   except the center vanish exactly. The center tap is `0.5`. Only the odd-offset taps are
//!   non-zero — the decimator exploits this and performs only those multiplies.
//! - Window: Kaiser(beta = 8.6), `w[n] = I0(beta·sqrt(1−(n/M)^2)) / I0(beta)` with `M = 31`
//!   (so `w[±31]` are the smallest non-zero window values, none forced to 0).
//! - No DC normalization is applied: the analytic half-band sum is already 1.0 to within
//!   f64 rounding (the test also checks the embedded taps sum to ≈ 1). Each 2:1 stage decimates
//!   by taking every other output sample; the passband reaches ≥ 90% of the output Nyquist by
//!   construction of the π/2 cutoff.
//!
//! Run with:
//!
//! ```text
//! cargo run -p couple-core --bin gen_halfband
//! ```
//!
//! and paste stdout over `core/src/fingerprint/coeffs.rs`. The float literals are emitted with
//! `{:?}` (Rust's shortest round-trip form) so re-parsing yields the identical f64 bits the
//! generator computed.

/// Half-band length (odd, symmetric). Center index is `HALF = 31`.
const N_TAPS: usize = 63;
const HALF: i64 = (N_TAPS as i64 - 1) / 2; // 31
const BETA: f64 = 8.6;

/// Modified Bessel function of the first kind, order 0, via the standard power series. The
/// series converges quickly for the arguments here (max ≈ BETA = 8.6); a fixed generous term
/// count keeps it deterministic (no convergence-dependent branching).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half_x = x / 2.0;
    for k in 1..=40 {
        // term_k = term_{k-1} · (x/2)^2 / k^2
        term *= (half_x * half_x) / (k as f64 * k as f64);
        sum += term;
    }
    sum
}

/// `sinc(x) = sin(πx)/(πx)`, with the removable singularity `sinc(0) = 1`.
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-15 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

fn main() {
    // Full windowed half-band taps, n = −31..=31.
    let i0_beta = bessel_i0(BETA);
    let mut taps = vec![0f64; N_TAPS];
    for (idx, tap) in taps.iter_mut().enumerate() {
        let n = idx as i64 - HALF; // −31..=31
        let ideal = 0.5 * sinc(n as f64 / 2.0);
        let r = n as f64 / HALF as f64; // −1..=1
        let w = bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
        *tap = ideal * w;
    }

    // Force the analytic half-band zeros exactly (even n ≠ 0). Floating sinc(n/2) for even n is
    // ~1e-17, not bit-zero; the decimator skips these anyway, but zeroing them makes the
    // "every other tap is zero" structure exact and the golden table clean.
    for (idx, tap) in taps.iter_mut().enumerate() {
        let n = idx as i64 - HALF;
        if n != 0 && n % 2 == 0 {
            *tap = 0.0;
        }
    }

    // The non-zero taps are: the center (n=0, value 0.5) and the odd-offset taps. We store the
    // half-band as (offset_from_center, coefficient) for offsets 0..=HALF where the coeff is
    // non-zero; symmetry recovers the negative side. Offsets: 0, 1, 3, 5, …, 31.
    let mut nonzero: Vec<(i64, f64)> = Vec::new();
    for off in 0..=HALF {
        let v = taps[(HALF + off) as usize];
        if v != 0.0 {
            nonzero.push((off, v));
        }
    }

    // Emit the coeffs.rs module.
    println!("{}", header());
    println!("/// Number of taps in the symmetric half-band FIR (odd; center at `HALF`).");
    println!("pub const N_TAPS: usize = {N_TAPS};");
    println!("/// Center index / half-length: taps run over offsets `-HALF..=HALF`.");
    println!("pub const HALF: usize = {HALF};");
    println!();
    println!("/// Non-zero half-band taps as `(offset_from_center, coefficient)` for offsets");
    println!("/// `>= 0`. The filter is symmetric, so a negative offset `-k` shares the");
    println!("/// coefficient of `+k`. Even offsets except 0 are exact half-band zeros and are");
    println!("/// OMITTED — the decimator performs only these multiplies (§7.1).");
    println!("pub const HALFBAND_NONZERO: [(usize, f64); {}] = [", nonzero.len());
    for (off, v) in &nonzero {
        println!("    ({off}, {v:?}),");
    }
    println!("];");

    // A human-facing sanity line on stderr (not part of the module).
    let sum: f64 = taps.iter().sum();
    eprintln!(
        "// generated {} taps, {} non-zero, DC sum = {:?}",
        N_TAPS,
        nonzero.len(),
        sum
    );
}

fn header() -> String {
    "\
//! Half-band decimation FIR coefficients for the fingerprint downsampler (§7.1).
//!
//! GENERATED by `core/src/bin/gen_halfband.rs` — do not hand-edit. A 63-tap symmetric
//! half-band Kaiser (beta 8.6) filter, cutoff ωc = π/2. The coefficient golden test
//! (`fingerprint::downsample::tests::coefficient_golden`) re-derives these with the
//! generator's math and asserts byte-equality, so this table cannot silently drift.
//!
//! Stored as the non-zero `(offset, coefficient)` taps only: the center (offset 0) plus the
//! odd offsets. Even offsets except 0 are exact half-band zeros, so the decimator multiplies
//! only the taps listed here."
        .to_string()
}
