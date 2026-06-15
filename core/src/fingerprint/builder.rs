//! Streaming fingerprint signal builder (PLAN-SPECTRAL-FINGERPRINT-MATCHING §7.2–§7.3).
//!
//! [`FingerprintBuilder::push`] accepts canonical mono 48 kHz PCM in ANY chunking, runs the
//! deterministic 48k→12k [`Downsampler`], windows the 12 kHz stream into 1024-sample Hann
//! frames at a 256 hop, takes a real FFT, converts power to `10·log10(max(power, eps))`,
//! restricts to the analyzed band, and extracts spectral peaks (§7.3). [`finalize`] applies the
//! deterministic per-second cap and returns the peak list sorted by `(frame, freq_bin)`.
//!
//! Chunk-boundary invariance (§7.2): every stage emits work as a function ONLY of the
//! cumulative sample/frame count, never of chunk boundaries — the downsampler emits an output
//! once enough input has arrived; an STFT frame is computed once `frame_start + fft_size`
//! 12 kHz samples exist; a peak for frame `f` is emitted once frames `f − R ..= f + R` exist
//! (R = `peak_time_radius_frames`). So the same PCM pushed all-at-once, one sample at a time,
//! or in random chunks yields the byte-identical peak list. The peak-level chunk-invariance
//! gate lives in `core/tests/fingerprint_builder.rs` (structured so boundary 2 extends it to
//! landmarks).
//!
//! Determinism (§3 invariant 2): pure arithmetic over the input and the fixed window/coeffs;
//! the FFT (`realfft`) is deterministic; no RNG / hash order / wall clock.

use super::downsample::Downsampler;
use super::landmarks::Landmark;
use super::params::FingerprintFormatV1;
use super::peaks::{apply_per_second_cap, eligible_peaks_for_frame, Frame, Peak};
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Floor inside the log so silence maps to a finite, very-negative dB rather than `-inf`
/// (§20: no non-finite values flow downstream). With f32 PCM the smallest meaningful power is
/// far above this; `1e-12` ⇒ `-120 dB`, a clean silence floor.
const POWER_EPS: f64 = 1e-12;

/// Compact the working 12 kHz buffer (drop the consumed prefix) only once this many samples
/// have accumulated as consumed, so the amortized cost of front-draining a `Vec` stays O(N)
/// rather than O(N²) on a single large `push`. Purely a performance knob — it does NOT affect
/// which frames/peaks are produced (chunk-invariance is unchanged), only when memory is
/// reclaimed. ~64 K samples ≈ 5.5 s of 12 kHz audio.
const COMPACT_THRESHOLD: usize = 65_536;

/// Streaming builder: PCM in, spectral peaks out. One builder fingerprints one track.
pub struct FingerprintBuilder {
    fmt: FingerprintFormatV1,
    down: Downsampler,
    fft: Arc<dyn RealToComplex<f64>>,
    /// Precomputed Hann window, length `fft_size`.
    hann: Vec<f64>,
    /// Reusable FFT input/output scratch.
    fft_in: Vec<f64>,
    fft_out: Vec<realfft::num_complex::Complex<f64>>,
    /// 12 kHz samples not yet consumed by a frame. `samples_consumed` tracks how many 12 kHz
    /// samples have already been dropped from the FRONT, so absolute frame starts map in.
    buf: Vec<f64>,
    samples_dropped: u64,
    /// Next STFT frame index to compute.
    next_frame: u32,
    /// Band geometry (cached from `fmt`).
    min_bin: usize,
    max_bin: usize,
    /// Sliding window of recent dB frames, length up to `2·peak_time_radius_frames + 1`, kept
    /// so a center frame's time-neighborhood maximum can be confirmed once its future frames
    /// exist. Frames enter in ascending index order.
    frame_ring: Vec<Frame>,
    /// Per-frame survivors accumulated across the whole track (the per-second cap + final sort
    /// run once at `finalize`, over a chunk-invariant input).
    survivors: Vec<Peak>,
    /// Center index within `frame_ring` whose peaks have been emitted up to (exclusive). Frames
    /// before this in the ring are still needed as the LEADING neighbors of later centers.
    emitted_centers: u32,
}

impl FingerprintBuilder {
    /// Build with the frozen v1 format ([`FingerprintFormatV1::fixed`]). The format is validated
    /// (it always passes for `fixed()`, but this keeps the invariant explicit).
    pub fn new() -> Self {
        Self::with_format(FingerprintFormatV1::fixed())
    }

    /// Build with an explicit format (used by tests). Panics if the format is invalid — callers
    /// in production always pass `fixed()`.
    pub fn with_format(fmt: FingerprintFormatV1) -> Self {
        fmt.validate().expect("FingerprintBuilder requires a valid format");
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(fmt.fft_size);
        let fft_in = fft.make_input_vec();
        let fft_out = fft.make_output_vec();
        let hann = hann_window(fmt.fft_size);
        let min_bin = fmt.min_bin();
        let max_bin = fmt.max_bin();
        Self {
            fft,
            hann,
            fft_in,
            fft_out,
            down: Downsampler::new(),
            buf: Vec::new(),
            samples_dropped: 0,
            next_frame: 0,
            min_bin,
            max_bin,
            frame_ring: Vec::new(),
            survivors: Vec::new(),
            emitted_centers: 0,
            fmt,
        }
    }

    /// 48 kHz time of STFT frame `f`'s window CENTER on the original timeline (§7.1 group
    /// delay): the frame covers 12 kHz samples `[f·hop, f·hop + fft_size)`, its center is at
    /// 12 kHz sample `f·hop + fft_size/2`, mapped back through the downsampler delay.
    pub fn frame_center_time_s(fmt: &FingerprintFormatV1, frame: u32) -> f64 {
        let center_12k = frame as f64 * fmt.hop_size as f64 + fmt.fft_size as f64 / 2.0;
        center_12k / fmt.analysis_sample_rate as f64 + Downsampler::group_delay_s()
    }

    /// Feed 48 kHz PCM (any chunking). Computes every now-complete frame and emits every
    /// now-confirmable peak; both depend only on the cumulative sample count.
    pub fn push(&mut self, samples: &[f32]) {
        // 1. Downsample to 12 kHz, appended to the working buffer.
        self.down.push(samples, &mut self.buf);
        // 2. Compute every frame whose full window is now buffered.
        self.compute_ready_frames();
    }

    /// Finish the stream: emit peaks for the trailing frames (their future neighbors will never
    /// arrive, so their available leading/partial neighborhood is final), apply the per-second
    /// cap, and return the peak list sorted by `(frame, freq_bin)`.
    pub fn finalize(mut self) -> Vec<Peak> {
        self.finalize_peaks_internal()
    }

    /// Finish the stream and return the §7.4 LANDMARK set sorted by `(hash, anchor_frame)`. This
    /// is the production finalize: it runs the exact same chunk-invariant peak pipeline as
    /// [`finalize`](Self::finalize), then generates landmarks from those peaks. Because the peak
    /// list is byte-identical across chunkings, the landmark set is too (the boundary-2
    /// chunk-invariance gate).
    pub fn finalize_landmarks(mut self) -> Vec<Landmark> {
        let fmt = self.fmt.clone();
        let peaks = self.finalize_peaks_internal();
        super::landmarks::generate_landmarks(&fmt, &peaks)
    }

    /// Shared finalize body: flush trailing centers, then apply the deterministic per-second cap,
    /// yielding peaks sorted by `(frame, freq_bin)`. Used by both the peak and landmark finalizers
    /// so they cannot diverge.
    fn finalize_peaks_internal(&mut self) -> Vec<Peak> {
        // Emit any remaining centers in the ring using whatever neighbors exist (the stream is
        // over; missing future frames simply do not constrain the trailing maxima).
        self.flush_centers(true);
        apply_per_second_cap(&self.fmt, std::mem::take(&mut self.survivors))
    }

    /// Compute every STFT frame whose `[frame_start, frame_start + fft_size)` window is fully
    /// present in `buf`, then advance peak emission.
    fn compute_ready_frames(&mut self) {
        let hop = self.fmt.hop_size as u64;
        let fft_size = self.fmt.fft_size as u64;
        loop {
            let frame_start = self.next_frame as u64 * hop; // absolute 12k sample index
            let window_end = frame_start + fft_size;
            // Available absolute 12k samples = samples_dropped + buf.len().
            let available = self.samples_dropped + self.buf.len() as u64;
            if window_end > available {
                break;
            }
            let local_start = (frame_start - self.samples_dropped) as usize;
            let db = self.frame_db(local_start);
            self.frame_ring.push(Frame { index: self.next_frame, db });
            self.next_frame += 1;

            // Compact the buffer's consumed prefix, but AMORTIZED: draining from the front of a
            // Vec shifts the remainder, so doing it every frame would be O(N²) for an all-at-once
            // push. Only physically drain once the consumed prefix exceeds a threshold, so the
            // total shifted bytes stay O(N). The next frame starts at `next_frame·hop`; anything
            // strictly before that is consumable.
            let next_start = self.next_frame as u64 * hop;
            let consumed = (next_start - self.samples_dropped).min(self.buf.len() as u64) as usize;
            if consumed >= COMPACT_THRESHOLD {
                self.buf.drain(..consumed);
                self.samples_dropped += consumed as u64;
            }
        }
        self.flush_centers(false);
    }

    /// Window + real-FFT + power→dB the frame starting at `buf[local_start]`, restricted to the
    /// analyzed band `[min_bin, max_bin]`. Returns the dB column.
    fn frame_db(&mut self, local_start: usize) -> Vec<f64> {
        let n = self.fmt.fft_size;
        for i in 0..n {
            self.fft_in[i] = self.buf[local_start + i] * self.hann[i];
        }
        self.fft.process(&mut self.fft_in, &mut self.fft_out).expect("realfft forward");
        let mut db = Vec::with_capacity(self.max_bin - self.min_bin + 1);
        for bin in self.min_bin..=self.max_bin {
            let c = self.fft_out[bin];
            let power = c.re * c.re + c.im * c.im;
            db.push(10.0 * power.max(POWER_EPS).log10());
        }
        db
    }

    /// Emit peaks for every ring center whose time-neighborhood (`±R` frames) is satisfied.
    /// When `final_flush` is set, the stream is over so a center is emitted with whatever
    /// neighbors exist (no more future frames will arrive); otherwise a center waits until its
    /// `+R` future frame is present. Consumed leading frames are dropped from the ring front.
    fn flush_centers(&mut self, final_flush: bool) {
        let r = self.fmt.peak_time_radius_frames as u32;
        // The center is the lowest ring index whose peaks are not yet emitted; loop while one
        // exists, stopping early if its future neighbor has not arrived (and not a final flush).
        while let Some(center_pos) = self
            .frame_ring
            .iter()
            .position(|f| f.index >= self.emitted_centers)
        {
            let center_index = self.frame_ring[center_pos].index;
            // Future-neighbor readiness: need a frame at `center_index + r` (or the stream end).
            let have_future = self
                .frame_ring
                .last()
                .map(|f| f.index >= center_index + r)
                .unwrap_or(false);
            if !have_future && !final_flush {
                break;
            }

            // Build the neighbor window: ring frames with index in [center−r, center+r] that
            // exist. (At the stream start/end fewer than 2r+1 frames; that is correct — a true
            // maximum need only beat the neighbors that exist.)
            let lo = center_index.saturating_sub(r);
            let hi = center_index + r;
            let window: Vec<Frame> = self
                .frame_ring
                .iter()
                .filter(|f| f.index >= lo && f.index <= hi)
                .map(|f| Frame { index: f.index, db: f.db.clone() })
                .collect();
            let local_center = window
                .iter()
                .position(|f| f.index == center_index)
                .expect("center is in its own window");

            let mut got = eligible_peaks_for_frame(&self.fmt, &window, local_center, self.min_bin);
            self.survivors.append(&mut got);
            self.emitted_centers = center_index + 1;

            // Drop ring frames that can no longer be a LEADING neighbor of any future center.
            // A future center has index ≥ emitted_centers, whose window reaches back to
            // emitted_centers − r; frames below that are unneeded.
            let keep_from = self.emitted_centers.saturating_sub(r);
            self.frame_ring.retain(|f| f.index >= keep_from);
        }
    }
}

impl Default for FingerprintBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Periodic Hann window, length `n`: `w[i] = 0.5·(1 − cos(2π·i/n))`. The periodic (NOT
/// symmetric) form is the right choice for STFT analysis (it makes overlapping windows sum
/// smoothly) and is deterministic.
fn hann_window(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Hann window is the periodic form, peaks at 1.0 mid-window, 0 at the start.
    #[test]
    fn hann_periodic_shape() {
        let w = hann_window(1024);
        assert_eq!(w.len(), 1024);
        assert!(w[0].abs() < 1e-12, "periodic Hann starts at 0");
        assert!((w[512] - 1.0).abs() < 1e-9, "periodic Hann peaks at the midpoint");
        // Symmetric about the midpoint.
        for i in 1..512 {
            assert!((w[i] - w[1024 - i]).abs() < 1e-12, "Hann must be symmetric at {i}");
        }
    }

    // Frame-time mapping: frame 0 center is at fft_size/2 12k-samples + the cumulative group
    // delay, on the original 48 kHz timeline.
    #[test]
    fn frame_center_time_accounts_group_delay() {
        let f = FingerprintFormatV1::fixed();
        let t0 = FingerprintBuilder::frame_center_time_s(&f, 0);
        let expected = (1024.0 / 2.0) / 12_000.0 + Downsampler::group_delay_s();
        assert!((t0 - expected).abs() < 1e-15, "frame-0 center time {t0} vs {expected}");
        // Frames advance by exactly the hop period.
        let t1 = FingerprintBuilder::frame_center_time_s(&f, 1);
        assert!((t1 - t0 - 256.0 / 12_000.0).abs() < 1e-12, "frame step must be the hop period");
    }
}
