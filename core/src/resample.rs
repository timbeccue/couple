//! High-quality fractional-position interpolation for the export resample path (§8.2).
//!
//! The export warp is a constant ratio within ±150 ppm of 1.0 — pure fractional-delay
//! interpolation, no rate reduction — so a Kaiser-windowed sinc evaluated exactly per
//! output sample is both transparent and fast: sin(π(f−k)) = ±sin(πf) for integer k,
//! so the whole tap row costs one sin() plus one division per tap (f64 throughout).
//! The quality bar is THD+N ≤ −110 dBFS in the passband [SPEC §8.2]; the unit test
//! below asserts it.

/// Windowed-sinc interpolator with precomputed Kaiser window (linear-interpolated table).
pub struct SincInterp {
    half: i64,
    table: Vec<f64>,
    per_unit: f64,
}

pub const EXPORT_TAPS: usize = 128;
pub const EXPORT_BETA: f64 = 14.0;

fn bessel_i0(x: f64) -> f64 {
    // power series; converges quickly for the β range used here
    let mut sum = 1.0;
    let mut term = 1.0;
    let half_x = x / 2.0;
    for k in 1..64 {
        term *= (half_x / k as f64) * (half_x / k as f64);
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
    }
    sum
}

impl SincInterp {
    pub fn new(taps: usize, beta: f64) -> Self {
        assert!(taps % 2 == 0 && taps >= 8);
        let half = (taps / 2) as i64;
        // Kaiser window sampled densely; linear interpolation error is negligible
        // because the window (unlike the sinc) is slowly varying.
        let per_unit = 512.0;
        let n = (half as f64 * per_unit) as usize + 2;
        let denom = bessel_i0(beta);
        let table: Vec<f64> = (0..n)
            .map(|i| {
                let d = i as f64 / per_unit / half as f64; // 0..1 of the half-aperture
                if d >= 1.0 {
                    0.0
                } else {
                    bessel_i0(beta * (1.0 - d * d).sqrt()) / denom
                }
            })
            .collect();
        Self {
            half,
            table,
            per_unit,
        }
    }

    #[inline]
    fn window_at(&self, d: f64) -> f64 {
        let x = d.abs() * self.per_unit;
        let i = x as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let fr = x - i as f64;
        self.table[i] + (self.table[i + 1] - self.table[i]) * fr
    }

    pub fn half(&self) -> i64 {
        self.half
    }

    /// Evaluate the band-limited signal at fractional frame position `pos`, where
    /// `samples[j]` holds frame `base + j` of one channel. Positions whose support
    /// extends outside the provided range treat missing frames as silence.
    pub fn eval(&self, samples: &[f64], base: i64, pos: f64) -> f64 {
        let i = pos.floor() as i64;
        let f = pos - i as f64;
        if f < 1e-12 {
            // exact integer position: the sinc degenerates to a unit impulse
            let j = i - base;
            return if j >= 0 && (j as usize) < samples.len() {
                samples[j as usize]
            } else {
                0.0
            };
        }
        let k0 = i - self.half + 1;
        let k1 = i + self.half;
        let s = (std::f64::consts::PI * f).sin();
        // sign of sin(π(f + m)) for integer m = i − k alternates with m's parity
        let mut acc = 0.0;
        let mut sign = if (i - k0) % 2 == 0 { 1.0 } else { -1.0 };
        for k in k0..=k1 {
            let j = k - base;
            if j >= 0 && (j as usize) < samples.len() {
                let d = pos - k as f64;
                let sinc = sign * s / (std::f64::consts::PI * d);
                acc += samples[j as usize] * sinc * self.window_at(d);
            }
            sign = -sign;
        }
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §8.2 quality bar: THD+N ≤ −110 dBFS in the passband for the export warp.
    #[test]
    fn thd_n_meets_export_bar() {
        let interp = SincInterp::new(EXPORT_TAPS, EXPORT_BETA);
        let sr = 48_000.0;
        let n_in = 48_000 * 4;
        for freq in [997.0, 4_001.0, 12_007.0] {
            let input: Vec<f64> = (0..n_in)
                .map(|n| 0.5 * (std::f64::consts::TAU * freq * n as f64 / sr).sin())
                .collect();
            let ratio = 1.0 + 30e-6;
            let phase0 = 0.37;
            let n_out = ((n_in as f64 - 256.0) / ratio) as usize;
            let mut err_ss = 0.0;
            let mut sig_ss = 0.0;
            let mut count = 0u64;
            for n in 0..n_out {
                let pos = n as f64 * ratio + phase0;
                if pos < EXPORT_TAPS as f64 || pos > n_in as f64 - EXPORT_TAPS as f64 {
                    continue; // exclude edge support
                }
                let got = interp.eval(&input, 0, pos);
                let ideal = 0.5 * (std::f64::consts::TAU * freq * pos / sr).sin();
                err_ss += (got - ideal) * (got - ideal);
                sig_ss += ideal * ideal;
                count += 1;
            }
            assert!(count > 0);
            let thd_n_db = 10.0 * (err_ss / sig_ss).log10();
            eprintln!("THD+N at {freq} Hz: {thd_n_db:.1} dB");
            assert!(thd_n_db <= -110.0, "{freq} Hz: {thd_n_db:.1} dB");
        }
    }
}
