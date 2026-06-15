//! Spectral peak extraction (PLAN-SPECTRAL-FINGERPRINT-MATCHING §7.3).
//!
//! A [`Peak`] is a local time/frequency maximum of the log-power STFT, selected by the §7.3
//! rules:
//!
//! 1. maximum in the configured time/frequency neighborhood;
//! 2. equal maxima resolve to exactly one peak by LOWER frequency bin, then EARLIER frame;
//! 3. exceeds the local frequency-neighborhood median by `peak_min_prominence_db`;
//! 4. inside the configured frequency band.
//!
//! Then, after eligibility:
//!
//! - keep the strongest `max_peaks_per_frame` per frame;
//! - enforce `max_peaks_per_second` with a deterministic sliding one-second cap;
//! - sort retained peaks by time, then frequency.
//!
//! Loudness handling (§7.3): the prominence test is LOCAL (a per-frame frequency-neighborhood
//! median), never a whole-track maximum, so a quiet section and a loud section are judged on
//! their own spectra. Determinism (§3 invariant 2): every comparison has an explicit total
//! tie-break; no value depends on RNG, hash order, or a wall clock.
//!
//! This module owns the pure peak-selection math over a dB spectrogram. The streaming STFT
//! that produces the spectrogram, and the chunk-invariant emission, live in
//! [`super::builder`].

use super::params::FingerprintFormatV1;

/// One selected spectral peak. `frame` is the STFT frame index (time), `freq_bin` the FFT bin
/// index (frequency), `magnitude_db` the log-power value the peak was selected at. These three
/// fully determine the deterministic serialization used by the chunk-invariance tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    pub frame: u32,
    pub freq_bin: u32,
    pub magnitude_db: f64,
}

/// A column of the dB spectrogram restricted to the analyzed band: `db[i]` is the log-power of
/// FFT bin `min_bin + i`. Frames are indexed by their absolute STFT frame number.
pub(crate) struct Frame {
    pub index: u32,
    /// dB values for bins `min_bin..=max_bin` (inclusive), length `max_bin - min_bin + 1`.
    pub db: Vec<f64>,
}

/// Select the eligible peaks of `center` given the time-neighborhood frames `window`
/// (`window` holds frames `center.index − R ..= center.index + R`, R = `peak_time_radius_frames`,
/// already filtered to those that exist — at the stream edges fewer than `2R+1`). Returns the
/// per-frame survivors (already capped to `max_peaks_per_frame`), in `(freq_bin)` ascending
/// order. The per-second cap and final ordering are applied later over all frames' survivors.
///
/// `min_bin` is the FFT bin index of `db[0]`. The neighborhood maximum compares `center`
/// against itself and the other frames at the same and nearby bins.
pub(crate) fn eligible_peaks_for_frame(
    fmt: &FingerprintFormatV1,
    window: &[Frame],
    center_pos: usize,
    min_bin: usize,
) -> Vec<Peak> {
    let center = &window[center_pos];
    let n_bins = center.db.len();
    let freq_r = fmt.peak_freq_radius_bins;
    let prom = fmt.peak_min_prominence_db;

    let mut survivors: Vec<Peak> = Vec::new();

    for i in 0..n_bins {
        let v = center.db[i];

        // (4) in-range is guaranteed by construction (db only spans the band), but the
        // frequency neighborhood must fit so the peak is a true interior maximum — skip bins
        // whose ±freq_r neighborhood would run off the band edge (they cannot be confirmed as
        // maxima against a full neighborhood, and §7.4 anchors need interior bins anyway).
        if i < freq_r || i + freq_r >= n_bins {
            continue;
        }

        // (1)+(2): is `v` the maximum over the time×frequency neighborhood, with equal maxima
        // resolved to the LOWER freq bin then EARLIER frame? We accept `(center, i)` iff no
        // neighbor strictly exceeds it, and no EQUAL neighbor is "earlier" in the tie order
        // (lower bin, or same bin + earlier frame). That makes exactly one cell in a flat
        // plateau win — the lowest bin, earliest frame.
        let mut is_peak = true;
        'nb: for f in window.iter() {
            for db_idx in i.saturating_sub(freq_r)..=(i + freq_r).min(n_bins - 1) {
                let nv = f.db[db_idx];
                if nv > v {
                    is_peak = false;
                    break 'nb;
                }
                if nv == v {
                    // Tie: the winner is the lower bin, then the earlier frame. If a neighbor
                    // ties and is "earlier" in that order than (center.index, i), center loses.
                    let neighbor_earlier = db_idx < i
                        || (db_idx == i && f.index < center.index);
                    if neighbor_earlier {
                        is_peak = false;
                        break 'nb;
                    }
                }
            }
        }
        if !is_peak {
            continue;
        }

        // (3): local frequency-neighborhood median prominence. The reference is the median of
        // the SAME-FRAME bins in `[i − freq_r, i + freq_r]` (a local, loudness-relative floor —
        // never the whole-track max). `v` must exceed it by ≥ `peak_min_prominence_db`.
        let lo = i - freq_r;
        let hi = i + freq_r;
        let mut neigh: Vec<f64> = center.db[lo..=hi].to_vec();
        neigh.sort_by(f64::total_cmp);
        let median = {
            let n = neigh.len();
            if n % 2 == 1 {
                neigh[n / 2]
            } else {
                0.5 * (neigh[n / 2 - 1] + neigh[n / 2])
            }
        };
        if v - median < prom {
            continue;
        }

        survivors.push(Peak {
            frame: center.index,
            freq_bin: (min_bin + i) as u32,
            magnitude_db: v,
        });
    }

    // Keep the strongest `max_peaks_per_frame` (deterministic tie-break: stronger first, then
    // lower bin). Then restore ascending-bin order for stable downstream handling.
    if survivors.len() > fmt.max_peaks_per_frame {
        survivors.sort_by(|a, b| {
            b.magnitude_db
                .total_cmp(&a.magnitude_db)
                .then(a.freq_bin.cmp(&b.freq_bin))
        });
        survivors.truncate(fmt.max_peaks_per_frame);
    }
    survivors.sort_by_key(|p| p.freq_bin);
    survivors
}

/// Apply the deterministic sliding one-second cap (§7.3) to all per-frame survivors and return
/// the final peak list sorted by `(frame, freq_bin)`.
///
/// Rule: process candidates STRONGEST-first (tie-break: earlier frame, then lower bin) and
/// greedily accept a peak iff accepting it keeps EVERY one-second window that contains its
/// frame within `max_peaks_per_second`. A one-second window is `fps` frames wide
/// (`fps = round(analysis_sample_rate / hop_size)`); the windows containing frame `f` start at
/// `s ∈ [f − fps + 1, f]`. A window's load only changes at accepted-peak frames, so checking
/// the windows anchored at each accepted neighbor frame (and at `f` itself) is exact. This
/// bounds every sliding one-second span by the cap while keeping the strongest peaks.
pub(crate) fn apply_per_second_cap(fmt: &FingerprintFormatV1, mut peaks: Vec<Peak>) -> Vec<Peak> {
    let cap = fmt.max_peaks_per_second;
    let fps = (fmt.analysis_sample_rate as f64 / fmt.hop_size as f64).round() as i64;
    let fps = fps.max(1);

    // Strongest-first, deterministic ties (earlier frame, then lower bin).
    peaks.sort_by(|a, b| {
        b.magnitude_db
            .total_cmp(&a.magnitude_db)
            .then(a.frame.cmp(&b.frame))
            .then(a.freq_bin.cmp(&b.freq_bin))
    });

    // Accepted peaks, plus a frame→accepted-count index for the window-load check. The BTreeMap
    // gives O(log n) range queries/inserts, and every window scan is bounded to the ±fps
    // neighborhood of the candidate (≈ 47 frames) — so the whole cap is O(n·fps·log n), not the
    // O(n²) a sorted-vec insert would cost. A candidate at frame `f` lies in every one-second
    // window `[s, s + fps)` with `s ∈ [f − fps + 1, f]`; accepting it adds 1 to each. We reject
    // it iff ANY such window's current accepted load already equals the cap. Scanning every
    // integer start `s` in that range is the unambiguous, obviously-correct check (the range is
    // only `fps` wide), so no clever anchor argument is needed.
    let cap = cap as u32;
    let mut accepted: Vec<Peak> = Vec::with_capacity(peaks.len());
    let mut counts: std::collections::BTreeMap<i64, u32> = std::collections::BTreeMap::new();

    for cand in peaks {
        let f = cand.frame as i64;
        // Local accepted frames that could share any window with `f`: those in (f − fps, f + fps).
        let local: Vec<(i64, u32)> = counts
            .range((f - fps + 1)..(f + fps))
            .map(|(&fr, &c)| (fr, c))
            .collect();

        let mut ok = true;
        // Every window start s ∈ [f − fps + 1, f] (each contains f).
        for s in (f - fps + 1)..=f {
            let load: u32 = local
                .iter()
                .filter(|&&(fr, _)| fr >= s && fr < s + fps)
                .map(|&(_, c)| c)
                .sum();
            if load + 1 > cap {
                ok = false;
                break;
            }
        }
        if ok {
            *counts.entry(f).or_insert(0) += 1;
            accepted.push(cand);
        }
    }

    // Final ordering: by time then frequency (§7.3).
    accepted.sort_by(|a, b| a.frame.cmp(&b.frame).then(a.freq_bin.cmp(&b.freq_bin)));
    accepted
}

/// Deterministic byte serialization of a peak list for the chunk-invariance gate (§7.5(1)).
/// Each peak is encoded as `frame(u32 LE) ‖ freq_bin(u32 LE) ‖ magnitude_db(f64 bits LE)`.
/// Two peak lists are byte-identical iff every field of every peak matches to the bit.
/// (Boundary 2 will extend the same idea to serialized LANDMARKS.)
pub fn serialize_peaks(peaks: &[Peak]) -> Vec<u8> {
    let mut out = Vec::with_capacity(peaks.len() * 16);
    for p in peaks {
        out.extend_from_slice(&p.frame.to_le_bytes());
        out.extend_from_slice(&p.freq_bin.to_le_bytes());
        out.extend_from_slice(&p.magnitude_db.to_bits().to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Frame` from a dense dB row covering bins `min_bin..` (length = row.len()).
    fn frame(index: u32, row: &[f64]) -> Frame {
        Frame { index, db: row.to_vec() }
    }

    // §7.3(1)+(2): an EQUAL-MAXIMA plateau resolves to EXACTLY ONE eligible peak — the lowest
    // freq bin, then the earliest frame. Construct TWO adjacent equal high bins (B, B+1, within
    // the freq radius of each other) held identical across all frames in the time window, on a
    // floor. The freq tie (B vs B+1) must pick the LOWER bin B; the time tie (frame 0 vs later)
    // must pick the EARLIEST frame. So the unique survivor is (frame 0, bin B).
    #[test]
    fn tie_break_plateau_single_peak() {
        let fmt = FingerprintFormatV1::fixed();
        let r = fmt.peak_time_radius_frames;
        let min_bin = fmt.min_bin();
        let n_bins = fmt.max_bin() - min_bin + 1;

        let b = 200usize; // lower plateau bin (offset within db); B+1 is its equal neighbor
        let high = 0.0;
        let low = -60.0;
        let row: Vec<f64> = (0..n_bins)
            .map(|i| if i == b || i == b + 1 { high } else { low })
            .collect();

        // Window = frames 0..=2r, all identical (the plateau extends in time too).
        let window: Vec<Frame> = (0..=(2 * r as u32)).map(|f| frame(f, &row)).collect();

        // Center on an interior frame r: it ties the earlier frame 0 in time ⇒ suppressed.
        let got_mid = eligible_peaks_for_frame(&fmt, &window, r, min_bin);
        assert!(
            got_mid.is_empty(),
            "an interior plateau frame must lose the time tie to the earlier frame 0: {got_mid:?}"
        );

        // Center on frame 0 (the earliest): exactly one peak — the LOWER plateau bin B.
        let got0 = eligible_peaks_for_frame(&fmt, &window, 0, min_bin);
        assert_eq!(got0.len(), 1, "plateau must resolve to exactly one peak: {got0:?}");
        assert_eq!(got0[0].freq_bin as usize, min_bin + b, "must keep the LOWER plateau bin");
        assert_eq!(got0[0].frame, 0, "must keep the EARLIEST frame");
    }

    // §7.3(3): the prominence reference is the LOCAL same-frame frequency-neighborhood median,
    // not a whole-track maximum. A lone bin `prom`+ dB above its neighborhood is a peak; the
    // same bin only `prom−1` dB above is not.
    #[test]
    fn prominence_is_local_median() {
        let mut fmt = FingerprintFormatV1::fixed();
        // Use a tiny prominence and radius for a crisp hand-checkable case.
        fmt.peak_min_prominence_db = 6.0;
        let fr = fmt.peak_freq_radius_bins;
        let min_bin = fmt.min_bin();
        let n_bins = fmt.max_bin() - min_bin + 1;
        let bin = 200usize;

        let make = |bump: f64| -> Vec<f64> {
            let mut row = vec![-50.0; n_bins];
            row[bin] = -50.0 + bump;
            row
        };
        // One frame only (time radius satisfied trivially: it beats itself); freq neighborhood
        // is floor except the bump → median ≈ floor, so prominence = bump.
        let above = make(7.0);
        let w_above = vec![frame(0, &above)];
        let got = eligible_peaks_for_frame(&fmt, &w_above, 0, min_bin);
        assert!(got.iter().any(|p| p.freq_bin as usize == min_bin + bin),
            "a +7 dB bin (> 6 dB prominence) must be a peak: {got:?}");

        let below = make(5.0);
        let w_below = vec![frame(0, &below)];
        let got2 = eligible_peaks_for_frame(&fmt, &w_below, 0, min_bin);
        assert!(!got2.iter().any(|p| p.freq_bin as usize == min_bin + bin),
            "a +5 dB bin (< 6 dB prominence) must NOT be a peak: {got2:?}");
        let _ = fr;
    }

    // §7.3 per-frame cap: more than `max_peaks_per_frame` eligible bins in one frame ⇒ keep the
    // strongest `max_peaks_per_frame`.
    #[test]
    fn per_frame_cap_keeps_strongest() {
        let fmt = FingerprintFormatV1::fixed();
        let min_bin = fmt.min_bin();
        let n_bins = fmt.max_bin() - min_bin + 1;
        // Place 8 well-separated prominent bins of increasing strength; cap is 5.
        let mut row = vec![-60.0; n_bins];
        let bins = [50usize, 90, 130, 170, 210, 250, 290, 330];
        for (k, &b) in bins.iter().enumerate() {
            row[b] = -20.0 + k as f64; // strictly increasing
        }
        let w = vec![frame(0, &row)];
        let got = eligible_peaks_for_frame(&fmt, &w, 0, min_bin);
        assert_eq!(got.len(), fmt.max_peaks_per_frame, "per-frame cap must keep exactly 5: {got:?}");
        // The kept ones are the 5 strongest (the highest-index bins here).
        let kept: std::collections::HashSet<usize> =
            got.iter().map(|p| p.freq_bin as usize - min_bin).collect();
        for &b in &bins[3..] {
            assert!(kept.contains(&b), "strongest bin {b} must be kept");
        }
    }

    // §7.3 sliding one-second cap: EVERY one-second window must hold ≤ max_peaks_per_second.
    // Build many equal-strength peaks at one frame-per-bin density and confirm the cap bounds
    // every window, keeps the strongest, and is deterministic.
    #[test]
    fn per_second_cap_bounds_every_window() {
        let fmt = FingerprintFormatV1::fixed();
        let fps = (fmt.analysis_sample_rate as f64 / fmt.hop_size as f64).round() as u32;
        let cap = fmt.max_peaks_per_second;

        // 200 frames, 3 peaks per frame at distinct bins, all equal strength → way over cap.
        let mut peaks = Vec::new();
        for f in 0..200u32 {
            for k in 0..3u32 {
                peaks.push(Peak { frame: f, freq_bin: 100 + k, magnitude_db: -10.0 });
            }
        }
        let out = apply_per_second_cap(&fmt, peaks.clone());
        let last = out.iter().map(|p| p.frame).max().unwrap_or(0);
        for s in 0..=last {
            let c = out.iter().filter(|p| p.frame >= s && p.frame < s + fps).count();
            assert!(c <= cap, "window [{s},{}) holds {c} > cap {cap}", s + fps);
        }
        assert!(!out.is_empty(), "the cap must keep SOME peaks");
        // Sorted by (frame, freq_bin).
        assert!(out.windows(2).all(|w| (w[0].frame, w[0].freq_bin) <= (w[1].frame, w[1].freq_bin)),
            "cap output must be sorted by (frame, freq_bin)");
        // Deterministic.
        assert_eq!(out, apply_per_second_cap(&fmt, peaks), "cap must be deterministic");
    }

    // The cap prefers STRONGER peaks: in a single dense second, the survivors are the strongest.
    #[test]
    fn per_second_cap_prefers_strong() {
        let fmt = FingerprintFormatV1::fixed();
        let cap = fmt.max_peaks_per_second;
        // All within one second (frames 0..10), strengths increasing with bin.
        let mut peaks = Vec::new();
        for f in 0..10u32 {
            for b in 0..10u32 {
                peaks.push(Peak { frame: f, freq_bin: 100 + b, magnitude_db: -50.0 + (f * 10 + b) as f64 });
            }
        }
        let out = apply_per_second_cap(&fmt, peaks);
        assert!(out.len() <= cap, "single-second survivors {} must be ≤ cap {cap}", out.len());
        // Every survivor is stronger than every dropped peak would require; check the minimum
        // survivor strength is in the top `cap` of all strengths.
        let mut all_strengths: Vec<f64> = (0..100).map(|i| -50.0 + i as f64).collect();
        all_strengths.sort_by(|a, b| b.total_cmp(a));
        let threshold = all_strengths[cap - 1];
        for p in &out {
            assert!(p.magnitude_db >= threshold, "a kept peak {p:?} is weaker than the cap cutoff {threshold}");
        }
    }

    // Serialization is byte-stable and order-sensitive (the chunk-invariance substrate).
    #[test]
    fn serialize_is_stable() {
        let a = vec![
            Peak { frame: 1, freq_bin: 10, magnitude_db: -3.5 },
            Peak { frame: 2, freq_bin: 20, magnitude_db: -7.0 },
        ];
        assert_eq!(serialize_peaks(&a), serialize_peaks(&a.clone()));
        assert_eq!(serialize_peaks(&a).len(), 2 * 16);
        let mut b = a.clone();
        b[0].magnitude_db = -3.5000001;
        assert_ne!(serialize_peaks(&a), serialize_peaks(&b), "a bit change must change the bytes");
    }
}
