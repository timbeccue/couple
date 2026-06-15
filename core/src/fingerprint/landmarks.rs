//! Landmark generation and hash packing (PLAN-SPECTRAL-FINGERPRINT-MATCHING §7.4).
//!
//! A [`Landmark`] is a constellation pair: an ANCHOR peak and one of up to `fanout` TARGET
//! peaks that follow it by `target_min_s..=target_max_s`. The pair is packed into a single
//! `u32` [`hash`](Landmark::hash) describing the two quantized frequencies and the quantized
//! anchor→target time gap, plus the anchor's STFT frame index. Two recordings of the SAME
//! acoustic event produce overlapping hash sets at a consistent anchor-frame delta; that is the
//! spectral-identity premise the later boundaries exploit.
//!
//! ## Selection (§7.4)
//!
//! For each anchor peak, in ascending `(frame, freq_bin)` order:
//!
//! - consider every target peak strictly later by a gap in `[target_min_s, target_max_s]`
//!   (gap measured in whole hop counts, the quantization grid — see below);
//! - choose up to `fanout` targets by STRONGEST magnitude;
//! - tie-break by EARLIER frame, then LOWER frequency bin;
//! - emit one landmark per surviving anchor→target pair.
//!
//! ## Hash payload (§7.4)
//!
//! The 32-bit hash packs four fields. Their widths are frozen for v1; the bit POSITIONS are an
//! internal contract pinned by the golden test in `core/tests/fingerprint_builder.rs`. We place
//! the anchor frequency in the lowest bits and the reserved field in the highest:
//!
//! ```text
//! bits  0..=8   anchor frequency index   (9 bits)
//! bits  9..=17  target frequency index   (9 bits)
//! bits 18..=25  delta-time index         (8 bits)
//! bits 26..=31  reserved / version       (6 bits, all zero for v1)
//! ```
//!
//! Quantization happens BEFORE packing, via integer floor division (§7.4):
//!
//! ```text
//! anchor frequency index = anchor_fft_bin / frequency_quantization_bins
//! target frequency index = target_fft_bin / frequency_quantization_bins
//! delta-time index       = (target_frame − anchor_frame) / delta_time_quantization_frames
//! ```
//!
//! Quantization makes a hash tolerant of small clock-rate and peak-bin shifts. Whether the
//! configured ranges actually fit their bit widths is settled at FORMAT VALIDATION
//! ([`FingerprintFormatV1::validate`]) — an over-width configuration FAILS validation rather
//! than truncating here (§7.4). [`pack_hash`] additionally `debug_assert!`s the fit so a logic
//! error is caught in tests, and masks defensively in release as a belt-and-suspenders measure
//! against ever silently corrupting an adjacent field.
//!
//! ## Determinism (§3 invariant 2)
//!
//! Every comparison has an explicit total tie-break; the final landmark set is sorted by
//! `(hash, anchor_frame)` before serialization. No value depends on RNG, hash-map iteration, or
//! a wall clock, so the byte output is reproducible.

use super::params::FingerprintFormatV1;
use super::peaks::Peak;

/// One spectral landmark: a packed anchor/target [`hash`](Self::hash) and the anchor's STFT
/// frame index. These two fields are the entire on-disk body record (§8.1) and fully determine
/// the serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Landmark {
    /// Packed `(anchor_freq, target_freq, delta_time, reserved)` — see the module docs.
    pub hash: u32,
    /// STFT frame index of the anchor peak (time). Maps to a 48 kHz instant via
    /// [`super::builder::FingerprintBuilder::frame_center_time_s`].
    pub anchor_frame: u32,
}

/// v1 landmark hash field widths (§7.4). The sum is exactly 32.
pub(crate) const ANCHOR_FREQ_BITS: u32 = 9;
pub(crate) const TARGET_FREQ_BITS: u32 = 9;
pub(crate) const DELTA_TIME_BITS: u32 = 8;
pub(crate) const RESERVED_BITS: u32 = 6;
const _: () =
    assert!(ANCHOR_FREQ_BITS + TARGET_FREQ_BITS + DELTA_TIME_BITS + RESERVED_BITS == 32);

// Bit offsets of each field (anchor lowest, reserved highest). Frozen for v1.
const ANCHOR_FREQ_SHIFT: u32 = 0;
const TARGET_FREQ_SHIFT: u32 = ANCHOR_FREQ_BITS; // 9
const DELTA_TIME_SHIFT: u32 = ANCHOR_FREQ_BITS + TARGET_FREQ_BITS; // 18
#[allow(dead_code)]
const RESERVED_SHIFT: u32 = ANCHOR_FREQ_BITS + TARGET_FREQ_BITS + DELTA_TIME_BITS; // 26

#[inline]
fn mask(bits: u32) -> u32 {
    (1u32 << bits) - 1
}

/// Pack the three quantized indices into the v1 hash. Reserved/version bits are zero (§7.4).
///
/// In debug builds this asserts every index fits its field (a logic bug, since
/// [`FingerprintFormatV1::validate`] already rejects over-width FORMATS at construction). In
/// release it masks each field, so even an unforeseen bug can never bleed into an adjacent
/// field — it would at worst alias within the same field, never silently corrupt another. The
/// real "fail rather than truncate" guarantee is the format validation, not this mask.
#[inline]
pub(crate) fn pack_hash(anchor_freq_idx: u32, target_freq_idx: u32, delta_idx: u32) -> u32 {
    debug_assert!(anchor_freq_idx <= mask(ANCHOR_FREQ_BITS), "anchor freq index overflows field");
    debug_assert!(target_freq_idx <= mask(TARGET_FREQ_BITS), "target freq index overflows field");
    debug_assert!(delta_idx <= mask(DELTA_TIME_BITS), "delta-time index overflows field");
    ((anchor_freq_idx & mask(ANCHOR_FREQ_BITS)) << ANCHOR_FREQ_SHIFT)
        | ((target_freq_idx & mask(TARGET_FREQ_BITS)) << TARGET_FREQ_SHIFT)
        | ((delta_idx & mask(DELTA_TIME_BITS)) << DELTA_TIME_SHIFT)
    // reserved/version bits left zero.
}

/// Unpack a v1 hash into `(anchor_freq_idx, target_freq_idx, delta_idx, reserved)`. Inverse of
/// [`pack_hash`]; exercised by this module's tests now and intended for later match-diagnostics,
/// so it has no non-test caller in this boundary yet.
#[allow(dead_code)]
#[inline]
pub(crate) fn unpack_hash(hash: u32) -> (u32, u32, u32, u32) {
    let anchor = (hash >> ANCHOR_FREQ_SHIFT) & mask(ANCHOR_FREQ_BITS);
    let target = (hash >> TARGET_FREQ_SHIFT) & mask(TARGET_FREQ_BITS);
    let delta = (hash >> DELTA_TIME_SHIFT) & mask(DELTA_TIME_BITS);
    let reserved = (hash >> RESERVED_SHIFT) & mask(RESERVED_BITS);
    (anchor, target, delta, reserved)
}

/// Generate the landmark set from peaks sorted by `(frame, freq_bin)` (the order
/// [`super::peaks::apply_per_second_cap`] returns). The format must already be valid (its
/// quantized indices fit the hash fields — §7.4); callers in this crate always pass `fixed()`.
///
/// Returns landmarks sorted by `(hash, anchor_frame)` (§7.4), ready for serialization.
pub(crate) fn generate_landmarks(fmt: &FingerprintFormatV1, peaks: &[Peak]) -> Vec<Landmark> {
    let q_freq = fmt.frequency_quantization_bins;
    let q_dt = fmt.delta_time_quantization_frames;

    // Convert the anchor→target time window to whole HOP counts (the delta-time grid). A target
    // qualifies when its frame gap lies in [min_frames, max_frames]. floor/ceil keep the window
    // conservative: the smallest gap that is ≥ target_min_s, the largest that is ≤ target_max_s.
    // (frame_period_s = hop_size / analysis_sample_rate.)
    let frame_period_s = fmt.frame_period_s();
    let min_frames = (fmt.target_min_s / frame_period_s).ceil() as i64;
    let min_frames = min_frames.max(1); // a target must be strictly later than its anchor
    let max_frames = (fmt.target_max_s / frame_period_s).floor() as i64;

    let mut landmarks: Vec<Landmark> = Vec::new();
    if max_frames < min_frames || peaks.is_empty() {
        return landmarks;
    }

    // Peaks are sorted by (frame, freq_bin); the target band for an anchor is a contiguous
    // forward run [lo, hi) found by advancing two cursors that never move backward, so the whole
    // pass is O(n + total_targets) rather than O(n²).
    let n = peaks.len();
    let mut lo = 0usize; // first peak with frame ≥ anchor.frame + min_frames
    let mut hi = 0usize; // first peak with frame > anchor.frame + max_frames
    for (ai, anchor) in peaks.iter().enumerate() {
        let anchor_frame = anchor.frame as i64;
        let target_lo_frame = anchor_frame + min_frames;
        let target_hi_frame = anchor_frame + max_frames; // inclusive

        // Advance lo to the first peak at frame ≥ target_lo_frame. lo never needs to move back
        // because anchors advance monotonically in frame.
        if lo < ai + 1 {
            lo = ai + 1;
        }
        while lo < n && (peaks[lo].frame as i64) < target_lo_frame {
            lo += 1;
        }
        // Advance hi to the first peak at frame > target_hi_frame.
        if hi < lo {
            hi = lo;
        }
        while hi < n && (peaks[hi].frame as i64) <= target_hi_frame {
            hi += 1;
        }

        if lo >= hi {
            continue;
        }

        // Candidate targets are peaks[lo..hi]. Choose up to `fanout` by STRONGEST magnitude,
        // tie-break by EARLIER frame then LOWER freq bin (§7.4). The candidate band is already in
        // (frame, freq_bin) ascending order, which is exactly the tie order; a STABLE selection
        // of the strongest therefore yields the tie-break for free.
        let band = &peaks[lo..hi];
        let chosen = choose_targets(band, fmt.fanout);

        let anchor_freq_idx = anchor.freq_bin / q_freq;
        for t in chosen {
            let target_freq_idx = t.freq_bin / q_freq;
            let delta_frames = t.frame - anchor.frame; // u32, > 0 by construction
            let delta_idx = delta_frames / q_dt;
            let hash = pack_hash(anchor_freq_idx, target_freq_idx, delta_idx);
            landmarks.push(Landmark { hash, anchor_frame: anchor.frame });
        }
    }

    // §7.4: sort by (hash, anchor_frame) before serialization. The derived Ord on Landmark is
    // exactly (hash, anchor_frame), and we use a STABLE sort so equal (hash, anchor_frame)
    // records — produced when two targets of one anchor quantize to the same hash — keep their
    // emission order, which is deterministic (target selection is deterministic). This makes the
    // full landmark vector a pure function of the peak list.
    landmarks.sort();
    landmarks
}

/// Pick up to `fanout` strongest targets from `band` (already in `(frame, freq_bin)` ascending
/// order), tie-broken by earlier frame then lower freq bin (§7.4). Returns them in that same
/// ascending order so emission is deterministic.
fn choose_targets(band: &[Peak], fanout: usize) -> Vec<&Peak> {
    if band.len() <= fanout {
        return band.iter().collect();
    }
    // Index the band so we can recover the original (frame, freq_bin) order after selecting by
    // magnitude. Select the strongest `fanout`: sort indices by (−magnitude, frame, freq_bin) —
    // the exact §7.4 tie order — take the first `fanout`, then restore ascending band order.
    let mut idx: Vec<usize> = (0..band.len()).collect();
    idx.sort_by(|&a, &b| {
        band[b]
            .magnitude_db
            .total_cmp(&band[a].magnitude_db)
            .then(band[a].frame.cmp(&band[b].frame))
            .then(band[a].freq_bin.cmp(&band[b].freq_bin))
    });
    idx.truncate(fanout);
    idx.sort_unstable();
    idx.into_iter().map(|i| &band[i]).collect()
}

/// Deterministic byte serialization of a landmark list (§8.1 body layout): each landmark is
/// `hash(u32 LE) ‖ anchor_frame(u32 LE)`. Two lists are byte-identical iff every field of every
/// landmark matches. This is the LANDMARK-level extension of [`super::peaks::serialize_peaks`]
/// used by the chunk-invariance gate, and the exact bytes the CPF1 body holds.
pub fn serialize_landmarks(landmarks: &[Landmark]) -> Vec<u8> {
    let mut out = Vec::with_capacity(landmarks.len() * 8);
    for l in landmarks {
        out.extend_from_slice(&l.hash.to_le_bytes());
        out.extend_from_slice(&l.anchor_frame.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(frame: u32, freq_bin: u32, magnitude_db: f64) -> Peak {
        Peak { frame, freq_bin, magnitude_db }
    }

    // Pack/unpack round-trips and lays fields out exactly as documented (anchor low, reserved
    // high, reserved == 0). This pins the v1 bit layout.
    #[test]
    fn pack_unpack_round_trip() {
        // Max legal values per field.
        let a = mask(ANCHOR_FREQ_BITS); // 511
        let t = mask(TARGET_FREQ_BITS); // 511
        let d = mask(DELTA_TIME_BITS); // 255
        let h = pack_hash(a, t, d);
        assert_eq!(unpack_hash(h), (a, t, d, 0), "reserved must be zero and fields recover");

        // Field isolation: setting only the anchor field leaves the others zero, and the field
        // sits in the lowest 9 bits.
        let only_anchor = pack_hash(0b1_0101_0101, 0, 0);
        assert_eq!(only_anchor, 0b1_0101_0101);
        assert_eq!(unpack_hash(only_anchor), (0b1_0101_0101, 0, 0, 0));

        // Target field sits at bits 9..=17.
        let only_target = pack_hash(0, 0b1_1001_1001, 0);
        assert_eq!(only_target, 0b1_1001_1001 << 9);

        // Delta field sits at bits 18..=25.
        let only_delta = pack_hash(0, 0, 0b1010_1010);
        assert_eq!(only_delta, 0b1010_1010 << 18);

        // The top 6 bits are never set by pack_hash (reserved/version == 0 for v1).
        assert_eq!(pack_hash(a, t, d) >> 26, 0, "reserved bits must be zero");
    }

    // §7.4 "fail validation rather than truncate" lives in FingerprintFormatV1::validate (proven
    // in params::tests). Here we pin the COMPLEMENT: under a VALID fixed() format, every quantized
    // index a real peak can produce fits its field, so pack_hash never truncates. The widest
    // anchor/target index is max_bin / q_freq; the widest delta is (target_max in frames) / q_dt.
    #[test]
    fn fixed_format_indices_fit_fields() {
        let f = FingerprintFormatV1::fixed();
        let max_freq_idx = f.max_bin() as u32 / f.frequency_quantization_bins;
        assert!(max_freq_idx <= mask(ANCHOR_FREQ_BITS), "freq index {max_freq_idx} must fit 9 bits");
        let max_delta_frames = (f.target_max_s / f.frame_period_s()).floor() as u32;
        let max_delta_idx = max_delta_frames / f.delta_time_quantization_frames;
        assert!(max_delta_idx <= mask(DELTA_TIME_BITS), "delta index {max_delta_idx} must fit 8 bits");
    }

    // Basic generation: one anchor with several targets in-window emits up to `fanout`, choosing
    // the STRONGEST and ordering output by (hash, anchor_frame).
    #[test]
    fn generates_fanout_strongest_targets() {
        let mut f = FingerprintFormatV1::fixed();
        f.fanout = 3;
        let fp = f.frame_period_s();
        // target window [0.5 s, 4.0 s] → frames roughly [24, 187].
        let anchor = peak(0, 20, -10.0);
        // Six targets CLUSTERED inside the window (frames base..base+5) with distinct magnitudes;
        // the three strongest must win. The cluster is < min_frames wide, so no target can pair
        // with another target — only the single anchor at frame 0 has targets.
        let mut peaks = vec![anchor];
        let base = ((1.0 / fp).ceil() as u32).max(1);
        for k in 0..6u32 {
            peaks.push(peak(base + k, 40 + k * 4, -30.0 + k as f64)); // strength increases with k
        }
        peaks.sort_by(|a, b| a.frame.cmp(&b.frame).then(a.freq_bin.cmp(&b.freq_bin)));
        let lms = generate_landmarks(&f, &peaks);
        assert_eq!(lms.len(), 3, "exactly fanout landmarks for the single anchor: {lms:?}");
        // Sorted by (hash, anchor_frame).
        assert!(lms.windows(2).all(|w| (w[0].hash, w[0].anchor_frame) <= (w[1].hash, w[1].anchor_frame)));
        // All anchored at frame 0.
        assert!(lms.iter().all(|l| l.anchor_frame == 0));
        // The chosen targets are the three strongest (k = 3,4,5). Recover target freq indices.
        let q = f.frequency_quantization_bins;
        let mut chosen_targets: Vec<u32> = lms.iter().map(|l| unpack_hash(l.hash).1).collect();
        chosen_targets.sort_unstable();
        let mut expected: Vec<u32> = [3u32, 4, 5].iter().map(|&k| (40 + k * 4) / q).collect();
        expected.sort_unstable();
        assert_eq!(chosen_targets, expected, "the strongest three targets must be chosen");
    }

    // Targets outside [target_min_s, target_max_s] are excluded; only in-window ones pair. Frame
    // gaps are chosen so the ONLY in-range pair in the whole set is anchor→in_win: every other
    // pairwise gap is either < min_frames (≈24) or > max_frames (≈187).
    #[test]
    fn excludes_out_of_window_targets() {
        let f = FingerprintFormatV1::fixed();
        // min_frames = ceil(0.5/fp) ≈ 24, max_frames = floor(4.0/fp) ≈ 187.
        let anchor = peak(0, 30, -5.0);
        let too_soon = peak(9, 50, -5.0); // gap-from-anchor 9 < 24 ⇒ excluded; also < 24 to in_win
        let in_win = peak(30, 60, -5.0); // gap-from-anchor 30 ∈ [24, 187] ⇒ the one valid target
        let too_late = peak(500, 70, -5.0); // gap-from-everything > 187 ⇒ excluded, anchors nothing
        let mut peaks = vec![anchor, too_soon, in_win, too_late];
        peaks.sort_by(|a, b| a.frame.cmp(&b.frame).then(a.freq_bin.cmp(&b.freq_bin)));
        let lms = generate_landmarks(&f, &peaks);
        // Only anchor→in_win is in range (too_soon→in_win gap 21 < 24; nothing reaches too_late).
        assert_eq!(lms.len(), 1, "only the in-window target should pair: {lms:?}");
        let (afi, tfi, _, _) = unpack_hash(lms[0].hash);
        assert_eq!(afi, 30 / f.frequency_quantization_bins, "anchor freq index");
        assert_eq!(tfi, 60 / f.frequency_quantization_bins, "target freq index");
        assert_eq!(lms[0].anchor_frame, 0);
    }

    // Quantization: two anchor freq bins in the same quantization bucket and two delta values in
    // the same delta bucket collapse to the SAME hash (tolerance to small shifts, §7.4).
    #[test]
    fn quantization_collapses_near_values() {
        let f = FingerprintFormatV1::fixed();
        assert_eq!(f.frequency_quantization_bins, 2);
        assert_eq!(f.delta_time_quantization_frames, 2);
        // anchor bins 40 and 41 → both /2 = 20; target bins 80 and 81 → both 40.
        let a_idx_lo = pack_hash(40 / 2, 80 / 2, 30 / 2);
        let a_idx_hi = pack_hash(41 / 2, 81 / 2, 31 / 2);
        assert_eq!(a_idx_lo, a_idx_hi, "neighbor bins/deltas in one bucket must share a hash");
    }

    // Determinism: input order of equal-magnitude targets does not change the output, and two
    // runs are byte-identical.
    #[test]
    fn deterministic_and_order_independent() {
        let mut f = FingerprintFormatV1::fixed();
        f.fanout = 2;
        let fp = f.frame_period_s();
        let g = ((1.0 / fp) as u32).max(1);
        // Equal-magnitude targets so the tie-break (earlier frame, lower bin) decides.
        let peaks_sorted = {
            let mut p = vec![
                peak(0, 20, -10.0),
                peak(g, 40, -20.0),
                peak(g + 1, 44, -20.0),
                peak(g + 2, 48, -20.0),
            ];
            p.sort_by(|a, b| a.frame.cmp(&b.frame).then(a.freq_bin.cmp(&b.freq_bin)));
            p
        };
        let a = generate_landmarks(&f, &peaks_sorted);
        let b = generate_landmarks(&f, &peaks_sorted);
        assert_eq!(serialize_landmarks(&a), serialize_landmarks(&b), "two runs must match");
        // fanout = 2 over three equal targets ⇒ the two EARLIEST (frame g and g+1) win.
        assert_eq!(a.len(), 2);
        let chosen_deltas: std::collections::HashSet<u32> =
            a.iter().map(|l| unpack_hash(l.hash).2).collect();
        // The two earliest target deltas (g and g+1), each floor-quantized by q_dt.
        let q = f.delta_time_quantization_frames;
        let expect: std::collections::HashSet<u32> = [g / q, (g + 1) / q].into_iter().collect();
        assert_eq!(chosen_deltas, expect, "the two earliest equal targets must win the tie");
    }

    // No anchors / empty input ⇒ no landmarks; a single peak (no possible target) ⇒ none.
    #[test]
    fn empty_and_single_peak_yield_none() {
        let f = FingerprintFormatV1::fixed();
        assert!(generate_landmarks(&f, &[]).is_empty());
        assert!(generate_landmarks(&f, &[peak(0, 20, -5.0)]).is_empty());
    }

    // Serialization is byte-stable, 8 bytes per landmark, and order-sensitive.
    #[test]
    fn serialize_is_stable_8_bytes() {
        let a = vec![
            Landmark { hash: 0x0001_0002, anchor_frame: 5 },
            Landmark { hash: 0x0003_0004, anchor_frame: 9 },
        ];
        assert_eq!(serialize_landmarks(&a), serialize_landmarks(&a.clone()));
        assert_eq!(serialize_landmarks(&a).len(), 2 * 8);
        let mut b = a.clone();
        b[0].anchor_frame = 6;
        assert_ne!(serialize_landmarks(&a), serialize_landmarks(&b), "a field change must change bytes");
    }
}
