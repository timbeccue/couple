//! Batch landmark index + correspondence generation (PLAN-SPECTRAL-FINGERPRINT-MATCHING §9.1, §9.2).
//!
//! Two deterministic building blocks that FEED the boundary-5 affine consensus; neither makes a
//! classification decision (no §9.3, no §10 scoring — that is boundary 5+). Both are pure
//! functions of their inputs: no RNG, no hash-map ITERATION order, no wall clock reaches the
//! output (§3 invariant 2).
//!
//! ## §9.1 Batch audio index — [`AudioHashIndex`]
//!
//! For ONE `match_candidates(videos, audios)` batch we build the audios-side inverted index once,
//! mapping `hash -> sorted list of (audio_index, anchor_frame)`. Construction uses a transient
//! `HashMap<u32, Vec<Posting>>` for grouping, but the published structure is a `Vec` of
//! `(hash, postings)` SORTED by hash, and every postings list is sorted by `(audio_index,
//! anchor_frame)` — so a lookup and the resulting correspondence stream never depend on map order.
//!
//! Uninformative-hash suppression (§9.1):
//!
//! - a hash is dropped FOR A TRACK when its occurrence count in that track exceeds
//!   `max_occurrences_per_hash_per_track` (a hash that fires dozens of times in one recording
//!   carries almost no timing information and would otherwise dominate the correspondence budget);
//! - the per-track drops are recorded as telemetry ([`IndexStats`]), never just logged.
//!
//! The video-hash/audio-hash *occurrence-product* guard (`max_occurrence_product`) is applied
//! per pair at correspondence time (§9.2 below), because the product needs the video-side count;
//! its drops are recorded in [`CorrespondenceStats`].
//!
//! ## §9.2 Correspondences — [`Correspondence`] / [`correspondences_for_pair`]
//!
//! For a (video, audio) pair we visit every shared hash and, for each matching landmark pair,
//! convert both anchor frames to seconds with the SAME boundary-1 frame-center/group-delay
//! convention ([`FingerprintBuilder::frame_center_time_s`]). These are EVIDENCE times, never a
//! search prior — no timestamp is ever used as a hard window (§20).
//!
//! Each raw observation gets `weight = 1 / sqrt(occ_a · occ_b)` (a hash shared by few landmarks
//! on each side is stronger evidence than a common one). Then, IN ORDER:
//!
//! 1. group records sharing one `(a_anchor_frame, b_anchor_frame)` into a single correspondence
//!    with `weight = min(1.0, Σ contributing weights)` — so one dense anchor fan-out cannot
//!    masquerade as many independent timing observations and inflate boundary-5 consensus;
//! 2. sort by `(t_a_s, t_b_s)`;
//! 3. if the grouped count exceeds `max_correspondences_per_pair`, retain a deterministic
//!    TIME-STRATIFIED sample over A's duration (128 bins, proportional allocation with ≥1 per
//!    non-empty bin, highest weight within a bin, time as the tie-break) — NEVER the first N,
//!    which would bias early audio (§20).
//!
//! No NaN/Infinity is produced: weights guard the `1/sqrt(...)` against a zero/negative product,
//! and time math is finite for finite frames (§20).

use super::builder::FingerprintBuilder;
use super::file::FingerprintReader;
use super::params::{FingerprintFormatV1, FingerprintMatchParams};
use std::collections::HashMap;

/// One posting in the audio index: which audio track (its index within the batch's `audios`
/// slice) and the anchor STFT frame of the landmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Posting {
    /// Index of the audio track within the batch (`audios[audio_index]`).
    pub audio_index: u32,
    /// Anchor STFT frame of the landmark (time), as in [`super::landmarks::Landmark`].
    pub anchor_frame: u32,
}

/// Debug telemetry for the batch index build (§9.1). Returned, not logged, so a caller/test can
/// assert exactly how many hashes were suppressed and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexStats {
    /// Total landmarks fed into the index across all audio tracks (before suppression).
    pub landmarks_in: u64,
    /// Landmark postings actually retained in the index (after the per-track occurrence cap).
    pub postings_kept: u64,
    /// Number of (track, hash) groups dropped because the hash occurred more than
    /// `max_occurrences_per_hash_per_track` times in that track.
    pub over_occurrence_hashes_dropped: u64,
    /// Total landmark postings discarded by those drops (the sum of the dropped groups' sizes).
    pub over_occurrence_postings_dropped: u64,
    /// Number of distinct hashes present in the published index.
    pub distinct_hashes: u64,
}

/// The audios-side inverted index for one batch (§9.1): `hash -> sorted postings`, with the outer
/// list sorted by hash. Built once per `match_candidates` call and queried per video.
///
/// Determinism: the public form is fully sorted, so neither a lookup nor the correspondence stream
/// it feeds depends on `HashMap` iteration order. Equal inputs (in any track/landmark ordering)
/// produce a byte-identical [`serialize`](Self::serialize).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioHashIndex {
    /// `(hash, postings)` entries sorted ascending by `hash`. Each postings list is non-empty and
    /// sorted by `(audio_index, anchor_frame)`.
    entries: Vec<(u32, Vec<Posting>)>,
}

impl AudioHashIndex {
    /// Build the batch audio index from each audio track's landmark reader (§9.1). `audios[i]`
    /// becomes `audio_index = i`. Hashes occurring more than
    /// `params.max_occurrences_per_hash_per_track` times WITHIN A SINGLE TRACK are dropped for
    /// that track (recorded in the returned [`IndexStats`]); a hash dropped for one track may
    /// still be indexed from another.
    ///
    /// `audios.len()` must be `<= u32::MAX`; callers pass batch slices far below that.
    pub fn build(
        audios: &[&FingerprintReader],
        params: &FingerprintMatchParams,
    ) -> (Self, IndexStats) {
        let cap = params.max_occurrences_per_hash_per_track;
        let mut stats = IndexStats::default();

        // Group postings by hash in a transient map. The map is ONLY for grouping; the published
        // structure below is fully sorted, so iteration order here cannot reach the output.
        let mut by_hash: HashMap<u32, Vec<Posting>> = HashMap::new();
        for (ai, reader) in audios.iter().enumerate() {
            let audio_index = ai as u32;
            stats.landmarks_in += reader.landmarks.len() as u64;

            // Count this track's occurrences per hash first, so the per-track cap is applied to the
            // WHOLE track's count for a hash (not incrementally), and a dropped hash contributes
            // nothing — not a truncated prefix. A track's landmark set is sorted by
            // (hash, anchor_frame), so equal hashes are already contiguous; count via a scan.
            let lms = &reader.landmarks;
            let mut i = 0usize;
            while i < lms.len() {
                let hash = lms[i].hash;
                let mut j = i + 1;
                while j < lms.len() && lms[j].hash == hash {
                    j += 1;
                }
                let occ = j - i;
                if occ > cap {
                    // Uninformative in this track: drop the whole group, record telemetry.
                    stats.over_occurrence_hashes_dropped += 1;
                    stats.over_occurrence_postings_dropped += occ as u64;
                } else {
                    let bucket = by_hash.entry(hash).or_default();
                    for lm in &lms[i..j] {
                        bucket.push(Posting { audio_index, anchor_frame: lm.anchor_frame });
                    }
                }
                i = j;
            }
        }

        // Publish a fully sorted structure: outer by hash, inner by (audio_index, anchor_frame).
        let mut entries: Vec<(u32, Vec<Posting>)> = by_hash.into_iter().collect();
        entries.sort_by_key(|(h, _)| *h);
        for (_, postings) in &mut entries {
            postings.sort_unstable();
            stats.postings_kept += postings.len() as u64;
        }
        stats.distinct_hashes = entries.len() as u64;

        (Self { entries }, stats)
    }

    /// Postings for `hash`, or `None` if absent. Sorted by `(audio_index, anchor_frame)`. Binary
    /// search over the hash-sorted `entries`, so lookup is `O(log H)` and order-independent.
    pub fn get(&self, hash: u32) -> Option<&[Posting]> {
        self.entries
            .binary_search_by_key(&hash, |(h, _)| *h)
            .ok()
            .map(|idx| self.entries[idx].1.as_slice())
    }

    /// Number of distinct hashes in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index holds no hashes.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Deterministic byte serialization of the whole index, for the determinism gate (§9.1): two
    /// indexes built from the same landmark sets — in ANY track/landmark input ordering — are
    /// byte-identical here. Layout per entry: `hash(u32 LE) ‖ count(u32 LE) ‖ count×(audio_index
    /// u32 LE ‖ anchor_frame u32 LE)`, entries in hash order. This pins "no hash-map-order
    /// dependence" as an asserted property, not a hope.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (hash, postings) in &self.entries {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&(postings.len() as u32).to_le_bytes());
            for p in postings {
                out.extend_from_slice(&p.audio_index.to_le_bytes());
                out.extend_from_slice(&p.anchor_frame.to_le_bytes());
            }
        }
        out
    }
}

/// One time correspondence between recordings A (video) and B (audio) (§9.2): the same spectral
/// landmark observed at `t_a_s` in A and `t_b_s` in B, weighted by how distinctive the shared hash
/// is. These are evidence points for boundary-5 affine consensus — never a search prior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correspondence {
    /// Time of the anchor in A (video), seconds on A's original 48 kHz timeline.
    pub t_a_s: f64,
    /// Time of the anchor in B (audio), seconds on B's original 48 kHz timeline.
    pub t_b_s: f64,
    /// Evidence weight in `(0, 1]`: `min(1.0, Σ 1/sqrt(occ_a·occ_b))` over the hashes that share
    /// this exact `(a_anchor_frame, b_anchor_frame)`.
    pub weight: f64,
}

/// Debug telemetry for one pair's correspondence generation (§9.1 product guard + §9.2 cap).
/// Returned, not logged, so a caller/test can see exactly what was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CorrespondenceStats {
    /// Shared-hash groups dropped because `occ_a · occ_b` exceeded `max_occurrence_product` (§9.1).
    pub over_product_hashes_dropped: u64,
    /// Raw landmark-pair observations skipped by those product drops (`occ_a · occ_b` summed).
    pub over_product_pairs_dropped: u64,
    /// Raw landmark-pair observations considered (after the product guard, before frame grouping).
    pub raw_pairs: u64,
    /// Distinct `(a_anchor_frame, b_anchor_frame)` correspondences after grouping, before the cap.
    pub grouped: u64,
    /// Correspondences retained after the time-stratified cap (== `grouped` when under the cap).
    pub kept: u64,
    /// Whether the ROUTINE §9.2 time-stratified SAMPLE cap fired (`grouped >
    /// max_correspondences_per_pair`). This is a SAMPLING signal ONLY: the retained representative
    /// subset is scored NORMALLY (§9.2). It does NOT by itself force `Ambiguous` — that conflation was
    /// the boundary-12 recall bug; the genuine resource ceiling is [`Self::hard_ceiling_exceeded`].
    pub capped: bool,
    /// Whether the GENUINE §9.3 resource ceiling fired (`grouped > hard_correspondence_ceiling`, a
    /// threshold FAR above the §9.2 sample cap). A truly-exhausted pair: the method-neutral
    /// classification is forced `Ambiguous` (never `NoMatch`, §20). Profile 0/1 keep the ceiling
    /// unreachable, so this stays `false` there (the §9.2 sample is always scored normally).
    pub hard_ceiling_exceeded: bool,
}

/// The output of [`correspondences_for_pair`]: the deterministic correspondence set for one pair
/// plus its telemetry. NO classification — that is boundary 5.
#[derive(Debug, Clone, PartialEq)]
pub struct PairCorrespondences {
    /// Correspondences sorted by `(t_a_s, t_b_s)`, grouped by frame pair and capped (§9.2).
    pub correspondences: Vec<Correspondence>,
    /// Drop/keep telemetry for this pair.
    pub stats: CorrespondenceStats,
}

/// Generate the correspondence set for one (video=A, audio=B) pair against the batch index (§9.2).
///
/// `video` is A's landmark reader; `index` is the batch audio index; `audio_index` selects B's
/// postings within it. The frame-center/group-delay time convention is boundary-1's
/// [`FingerprintBuilder::frame_center_time_s`], applied to BOTH sides under the frozen `fmt`.
///
/// Steps (exactly §9.2): per shared hash, apply the §9.1 occurrence-PRODUCT guard, emit one raw
/// observation per matching landmark pair with `weight = 1/sqrt(occ_a·occ_b)`; group records with
/// the same `(a_anchor_frame, b_anchor_frame)` into one correspondence with `min(1.0, Σ weight)`;
/// sort by `(t_a_s, t_b_s)`; if over `max_correspondences_per_pair`, retain a 128-bin
/// time-stratified sample. Telemetry is recorded throughout.
pub fn correspondences_for_pair(
    video: &FingerprintReader,
    index: &AudioHashIndex,
    audio_index: u32,
    fmt: &FingerprintFormatV1,
    params: &FingerprintMatchParams,
) -> PairCorrespondences {
    let mut stats = CorrespondenceStats::default();

    // Per-hash occurrence count on the VIDEO side (occ_a). The reader's landmarks are sorted by
    // (hash, anchor_frame), so a hash's run is contiguous and we can count it once.
    let v_lms = &video.landmarks;

    // Accumulate the summed weight per (a_anchor_frame, b_anchor_frame) frame pair, so a dense
    // anchor fan-out collapses to one correspondence (§9.2). Keyed by the two frames; the values
    // are summed weights. A transient HashMap again — output order is established by the explicit
    // sort below, never by map iteration.
    let mut grouped: HashMap<(u32, u32), f64> = HashMap::new();

    let mut i = 0usize;
    while i < v_lms.len() {
        let hash = v_lms[i].hash;
        let mut j = i + 1;
        while j < v_lms.len() && v_lms[j].hash == hash {
            j += 1;
        }
        let occ_a = j - i;

        // B-side postings for this hash, restricted to the selected audio track. Postings are
        // sorted by (audio_index, anchor_frame); slice out the contiguous run for `audio_index`.
        if let Some(postings) = index.get(hash) {
            let b_postings = postings_for_audio(postings, audio_index);
            let occ_b = b_postings.len();
            if occ_b > 0 {
                // §9.1 occurrence-PRODUCT guard: a hash whose shared occurrences explode on both
                // sides is uninformative for THIS pair. Drop the whole group, record telemetry.
                let product = (occ_a as u64).saturating_mul(occ_b as u64);
                if product > params.max_occurrence_product as u64 {
                    stats.over_product_hashes_dropped += 1;
                    stats.over_product_pairs_dropped += product;
                } else {
                    // weight = 1/sqrt(occ_a·occ_b). product ≥ 1 here, so the sqrt is finite and the
                    // weight is in (0, 1] — never NaN/Infinity (§20).
                    let weight = 1.0 / (product as f64).sqrt();
                    stats.raw_pairs += product;
                    for a_lm in &v_lms[i..j] {
                        let a_frame = a_lm.anchor_frame;
                        for b_posting in b_postings {
                            // Group by the exact (a_anchor_frame, b_anchor_frame): summing the
                            // per-pair weights here is the §9.2 fan-out collapse.
                            *grouped.entry((a_frame, b_posting.anchor_frame)).or_insert(0.0) +=
                                weight;
                        }
                    }
                }
            }
        }
        i = j;
    }

    stats.grouped = grouped.len() as u64;

    // Materialize one Correspondence per frame pair, converting frames → seconds with boundary-1's
    // convention for BOTH sides, and clamping the summed weight to 1.0 (§9.2). Then sort by
    // (t_a_s, t_b_s). We sort the frame-pair keys deterministically FIRST (so floating times never
    // decide order between two identical-time pairs), then map to times.
    let mut keys: Vec<(u32, u32, f64)> =
        grouped.into_iter().map(|((a, b), w)| (a, b, w)).collect();
    // Order by (a_frame, b_frame): a total order on integers, independent of map iteration and of
    // float comparison. Because frame_center_time_s is strictly increasing in the frame, this is
    // exactly the (t_a_s, t_b_s) order the plan asks for, but computed on exact integers.
    keys.sort_unstable_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)));

    let mut all: Vec<Correspondence> = Vec::with_capacity(keys.len());
    for (a_frame, b_frame, w) in keys {
        all.push(Correspondence {
            t_a_s: FingerprintBuilder::frame_center_time_s(fmt, a_frame),
            t_b_s: FingerprintBuilder::frame_center_time_s(fmt, b_frame),
            weight: w.min(1.0),
        });
    }

    // §9.3 GENUINE resource ceiling: the grouped count exceeds the HARD ceiling (FAR above the §9.2
    // sample cap). This is a truly-exhausted pair — recorded so boundary 6 can force `Ambiguous`
    // (never `NoMatch`, §20). It is computed on the GROUPED count BEFORE sampling. Profile 0/1 hold
    // the ceiling unreachable, so this stays `false` there.
    stats.hard_ceiling_exceeded = all.len() > params.hard_correspondence_ceiling;

    // §9.2 SAMPLE cap: if over the routine limit, retain a deterministic time-stratified sample over
    // A's duration. The retained subset is scored NORMALLY downstream — `capped` is a sampling flag,
    // not a "give up" signal (the §9.2/§9.3 separation: only `hard_ceiling_exceeded` forces Ambiguous).
    let cap = params.max_correspondences_per_pair;
    let correspondences = if all.len() > cap {
        stats.capped = true;
        time_stratified_sample(all, cap, video.duration_frames, fmt)
    } else {
        all
    };
    stats.kept = correspondences.len() as u64;

    PairCorrespondences { correspondences, stats }
}

/// The contiguous `anchor_frame`s in `postings` (sorted by `(audio_index, anchor_frame)`) that
/// belong to `audio_index`. Returns a sub-slice viewed as frames via the two binary-search bounds.
fn postings_for_audio(postings: &[Posting], audio_index: u32) -> &[Posting] {
    // Lower bound: first posting with audio_index >= target. Upper bound: first with > target.
    let lo = postings.partition_point(|p| p.audio_index < audio_index);
    let hi = postings.partition_point(|p| p.audio_index <= audio_index);
    &postings[lo..hi]
}

/// Deterministic time-stratified subsample of `all` down to `cap` correspondences (§9.2). `all`
/// MUST already be sorted by `(t_a_s, t_b_s)`. Divides A's duration into 128 equal bins, allocates
/// the cap proportionally to each bin's population with ≥1 from every non-empty bin, keeps the
/// highest-weight members within a bin (tie-broken by time), and returns the union re-sorted by
/// `(t_a_s, t_b_s)`. NEVER a first-N prefix.
fn time_stratified_sample(
    all: Vec<Correspondence>,
    cap: usize,
    duration_frames: u64,
    fmt: &FingerprintFormatV1,
) -> Vec<Correspondence> {
    const BINS: usize = 128;
    debug_assert!(all.len() > cap, "caller only subsamples when over the cap");
    if cap == 0 {
        return Vec::new();
    }

    // A-duration span in seconds, using the SAME frame-center convention as the correspondences so
    // a correspondence's t_a_s lands in the right bin. The usable span is [t(frame 0), t(last
    // frame)]; we bin on t_a_s relative to that. If the duration is unknown/degenerate, fall back
    // to the empirical t_a_s range so binning is still well-defined and finite.
    let t_lo = FingerprintBuilder::frame_center_time_s(fmt, 0);
    let last_frame = duration_frames
        .saturating_sub(1)
        .min(u32::MAX as u64) as u32;
    let t_hi_dur = FingerprintBuilder::frame_center_time_s(fmt, last_frame);
    // Empirical max t_a_s (all is sorted by (t_a_s, t_b_s), so the last element has the max t_a_s).
    let t_hi_emp = all.last().map(|c| c.t_a_s).unwrap_or(t_lo);
    let t_hi = t_hi_dur.max(t_hi_emp);
    let span = (t_hi - t_lo).max(f64::MIN_POSITIVE); // > 0, finite ⇒ no div-by-zero/NaN.

    // Assign each correspondence to a bin by t_a_s. Keep indices into `all` so we can recover the
    // (already time-sorted) order. Within a bin the items stay in their global (time) order.
    let mut bins: Vec<Vec<usize>> = vec![Vec::new(); BINS];
    for (idx, c) in all.iter().enumerate() {
        let frac = ((c.t_a_s - t_lo) / span).clamp(0.0, 1.0);
        // floor to a bin; the high edge maps to the last bin.
        let mut b = (frac * BINS as f64) as usize;
        if b >= BINS {
            b = BINS - 1;
        }
        bins[b].push(idx);
    }

    let non_empty = bins.iter().filter(|b| !b.is_empty()).count();
    // Cap can be below the number of non-empty bins: we cannot honor "≥1 per non-empty bin" for
    // every bin then, so allocate the cap to the MOST-POPULATED non-empty bins first (a
    // deterministic choice), one slot each, which still spreads across the timeline rather than
    // taking a prefix.
    let mut quota = vec![0usize; BINS];
    if cap <= non_empty {
        // One slot to each of the `cap` most-populated non-empty bins (tie-break: earlier bin).
        let mut order: Vec<usize> = (0..BINS).filter(|&b| !bins[b].is_empty()).collect();
        order.sort_by(|&a, &b| bins[b].len().cmp(&bins[a].len()).then(a.cmp(&b)));
        for &b in order.iter().take(cap) {
            quota[b] = 1;
        }
    } else {
        // ≥1 per non-empty bin, then distribute the remaining budget proportionally to population.
        let mut remaining = cap;
        for b in 0..BINS {
            if !bins[b].is_empty() {
                quota[b] = 1;
                remaining -= 1;
            }
        }
        let total: usize = all.len();
        // Proportional shares using integer floor; track fractional remainders for a deterministic
        // largest-remainder distribution of the leftover after flooring.
        let mut remainders: Vec<(f64, usize)> = Vec::new();
        let mut floor_sum = 0usize;
        for b in 0..BINS {
            let pop = bins[b].len();
            if pop == 0 {
                continue;
            }
            // Proportional EXTRA share (beyond the guaranteed 1) of the `remaining` budget.
            let exact = remaining as f64 * (pop as f64 / total as f64);
            let fl = exact.floor();
            let add = fl as usize;
            // Never allocate more than the bin can supply (it already has 1 guaranteed slot).
            let add = add.min(pop.saturating_sub(quota[b]));
            quota[b] += add;
            floor_sum += add;
            remainders.push((exact - fl, b));
        }
        // Largest-remainder: hand out the still-unallocated budget one at a time to the bins with
        // the biggest fractional part (tie-break: earlier bin), skipping any already at capacity.
        // Sweeping `remainders` (which lists every non-empty bin) at most twice is sufficient: the
        // first sweep places against every bin with spare capacity, and `leftover` (the rounding
        // remainder) is strictly below the bin count, so it is exhausted before a third pass —
        // the explicit `break` on `leftover == 0` makes termination unconditional regardless.
        let mut leftover = remaining.saturating_sub(floor_sum);
        remainders.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        'fill: for _ in 0..2 {
            for &(_, b) in &remainders {
                if leftover == 0 {
                    break 'fill;
                }
                if quota[b] < bins[b].len() {
                    quota[b] += 1;
                    leftover -= 1;
                }
            }
        }
    }

    // Within each bin, KEEP the highest-weight members up to its quota, tie-broken by time. The
    // bin's indices are in global time order; selecting by (−weight, time) and then restoring time
    // order keeps the result deterministic and time-sorted within the bin.
    let mut kept_idx: Vec<usize> = Vec::with_capacity(cap);
    for b in 0..BINS {
        let q = quota[b];
        if q == 0 || bins[b].is_empty() {
            continue;
        }
        if q >= bins[b].len() {
            kept_idx.extend_from_slice(&bins[b]);
            continue;
        }
        // Select the q strongest by (weight desc, t_a_s asc, t_b_s asc). `all[idx]` carries the
        // times; the index order within `order` is the global time order, a stable secondary key.
        let mut order = bins[b].clone();
        order.sort_by(|&x, &y| {
            all[y]
                .weight
                .total_cmp(&all[x].weight)
                .then(all[x].t_a_s.total_cmp(&all[y].t_a_s))
                .then(all[x].t_b_s.total_cmp(&all[y].t_b_s))
                .then(x.cmp(&y))
        });
        order.truncate(q);
        kept_idx.extend_from_slice(&order);
    }

    // Re-sort the union by (t_a_s, t_b_s) (the §9.2 output ordering). Indices into the time-sorted
    // `all` give a stable integer tie-break.
    kept_idx.sort_unstable();
    kept_idx.into_iter().map(|idx| all[idx]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::landmarks::Landmark;

    /// Build a `FingerprintReader` directly from a landmark list (sorted as the codec requires)
    /// for a given duration. Bypasses the signal pipeline so the index/correspondence logic can be
    /// driven on hand-crafted hashes/frames. Mirrors the real reader's fields.
    fn reader(mut lms: Vec<Landmark>, duration_frames: u64) -> FingerprintReader {
        lms.sort();
        let fmt = FingerprintFormatV1::fixed();
        FingerprintReader {
            analysis_sample_rate: fmt.analysis_sample_rate,
            fft_size: fmt.fft_size as u32,
            hop_size: fmt.hop_size as u32,
            min_frequency_millihz: (fmt.min_frequency_hz * 1000.0).round() as u32,
            max_frequency_millihz: (fmt.max_frequency_hz * 1000.0).round() as u32,
            params_hash: fmt.params_hash(),
            duration_frames,
            landmarks: lms,
        }
    }

    fn lm(hash: u32, anchor_frame: u32) -> Landmark {
        Landmark { hash, anchor_frame }
    }

    fn fmt() -> FingerprintFormatV1 {
        FingerprintFormatV1::fixed()
    }

    fn params() -> FingerprintMatchParams {
        FingerprintMatchParams::default()
    }

    // ---- §9.1 index determinism: no hash-map-order dependence ----
    //
    // Building the index twice, and from DIFFERENT input orderings of equal landmark sets, must
    // yield a byte-identical serialized index. We shuffle both the track order and each track's
    // landmark order (within the (hash, anchor_frame) sort the reader re-imposes) and require the
    // published index to be identical.
    #[test]
    fn index_build_is_order_independent_and_deterministic() {
        let a0 = reader(vec![lm(5, 10), lm(5, 40), lm(2, 7), lm(9, 1)], 1000);
        let a1 = reader(vec![lm(2, 3), lm(7, 20), lm(5, 99)], 1000);
        let a2 = reader(vec![lm(9, 50), lm(2, 7)], 1000);

        let (idx_a, stats_a) = AudioHashIndex::build(&[&a0, &a1, &a2], &params());
        // Same inputs again ⇒ identical.
        let (idx_b, stats_b) = AudioHashIndex::build(&[&a0, &a1, &a2], &params());
        assert_eq!(idx_a.serialize(), idx_b.serialize(), "two builds must be byte-identical");
        assert_eq!(stats_a, stats_b);

        // A DIFFERENT track ordering changes which audio_index a track gets, so we cannot expect
        // an identical index across track permutations (audio_index is meaningful). Instead prove
        // landmark-order independence WITHIN fixed track positions: rebuild readers from reordered
        // landmark inputs (the reader re-sorts), same positions, and require an identical index.
        let a0r = reader(vec![lm(9, 1), lm(5, 40), lm(2, 7), lm(5, 10)], 1000);
        let a1r = reader(vec![lm(5, 99), lm(2, 3), lm(7, 20)], 1000);
        let a2r = reader(vec![lm(2, 7), lm(9, 50)], 1000);
        let (idx_r, _) = AudioHashIndex::build(&[&a0r, &a1r, &a2r], &params());
        assert_eq!(
            idx_a.serialize(),
            idx_r.serialize(),
            "landmark input order must not change the published index"
        );

        // Spot-check structure: entries are hash-sorted, postings are (audio_index, anchor_frame)-
        // sorted, and a get() returns the expected postings.
        let h5 = idx_a.get(5).unwrap();
        assert_eq!(h5, &[Posting { audio_index: 0, anchor_frame: 10 }, Posting { audio_index: 0, anchor_frame: 40 }, Posting { audio_index: 1, anchor_frame: 99 }]);
        assert!(idx_a.get(123).is_none(), "absent hash ⇒ None");
    }

    // ---- §9.1 suppression: per-track occurrence cap drops the hash with telemetry ----
    #[test]
    fn over_occurrence_hash_dropped_with_telemetry() {
        let mut p = params();
        p.max_occurrences_per_hash_per_track = 3;
        // Hash 7 occurs 5× in one track (> 3 ⇒ dropped for that track); hash 4 occurs 2× (kept).
        let a = reader(
            vec![lm(7, 1), lm(7, 2), lm(7, 3), lm(7, 4), lm(7, 5), lm(4, 10), lm(4, 20)],
            1000,
        );
        let (idx, stats) = AudioHashIndex::build(&[&a], &p);
        assert!(idx.get(7).is_none(), "over-cap hash must be dropped from the index");
        assert_eq!(idx.get(4).unwrap().len(), 2, "under-cap hash retained");
        assert_eq!(stats.over_occurrence_hashes_dropped, 1);
        assert_eq!(stats.over_occurrence_postings_dropped, 5);
        assert_eq!(stats.landmarks_in, 7);
        assert_eq!(stats.postings_kept, 2);

        // The SAME hash that is over-cap in track 0 can still be indexed from a track where it is
        // under-cap (the cap is per-track).
        let b = reader(vec![lm(7, 100)], 1000);
        let (idx2, stats2) = AudioHashIndex::build(&[&a, &b], &p);
        assert_eq!(idx2.get(7).unwrap(), &[Posting { audio_index: 1, anchor_frame: 100 }]);
        assert_eq!(stats2.over_occurrence_hashes_dropped, 1, "only track 0's group dropped");
    }

    // ---- §9.1 occurrence-product guard: a shared hash whose occ_a·occ_b explodes is dropped ----
    #[test]
    fn over_product_group_dropped_with_telemetry() {
        let mut p = params();
        p.max_occurrence_product = 6; // occ_a·occ_b must be <= 6
        // Hash 1: 3 in video × 3 in audio = 9 > 6 ⇒ dropped. Hash 2: 2×2 = 4 <= 6 ⇒ kept.
        let video = reader(vec![lm(1, 0), lm(1, 5), lm(1, 9), lm(2, 0), lm(2, 7)], 1000);
        let audio = reader(vec![lm(1, 0), lm(1, 4), lm(1, 8), lm(2, 1), lm(2, 6)], 1000);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert_eq!(pc.stats.over_product_hashes_dropped, 1);
        assert_eq!(pc.stats.over_product_pairs_dropped, 9);
        // Only hash 2's pairs survived: 2×2 = 4 raw pairs.
        assert_eq!(pc.stats.raw_pairs, 4);
    }

    // ---- §9.2 weight formula: 1/sqrt(occ_a·occ_b) on a hand-checked case ----
    #[test]
    fn weight_is_inverse_sqrt_occ_product() {
        let p = params();
        // One shared hash, occ_a = 1, occ_b = 1 ⇒ weight 1.0, single frame pair.
        let video = reader(vec![lm(42, 3)], 1000);
        let audio = reader(vec![lm(42, 8)], 1000);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert_eq!(pc.correspondences.len(), 1);
        assert!((pc.correspondences[0].weight - 1.0).abs() < 1e-12);
        // Times come from the boundary-1 frame-center convention.
        let f = fmt();
        assert_eq!(pc.correspondences[0].t_a_s, FingerprintBuilder::frame_center_time_s(&f, 3));
        assert_eq!(pc.correspondences[0].t_b_s, FingerprintBuilder::frame_center_time_s(&f, 8));

        // occ_a = 2, occ_b = 3 ⇒ each of the 6 DISTINCT frame pairs gets 1/sqrt(6). (Anchors and
        // targets all distinct so no two land on the same frame pair ⇒ no summing.)
        let video2 = reader(vec![lm(9, 1), lm(9, 2)], 1000);
        let audio2 = reader(vec![lm(9, 10), lm(9, 20), lm(9, 30)], 1000);
        let (idx2, _) = AudioHashIndex::build(&[&audio2], &p);
        let pc2 = correspondences_for_pair(&video2, &idx2, 0, &fmt(), &p);
        assert_eq!(pc2.correspondences.len(), 6, "2×3 distinct frame pairs");
        let expect = 1.0 / 6f64.sqrt();
        for c in &pc2.correspondences {
            assert!((c.weight - expect).abs() < 1e-12, "weight {} != 1/sqrt(6)", c.weight);
        }
        // Sorted by (t_a_s, t_b_s): t_a_s non-decreasing, ties broken by t_b_s.
        assert!(pc2
            .correspondences
            .windows(2)
            .all(|w| (w[0].t_a_s, w[0].t_b_s) <= (w[1].t_a_s, w[1].t_b_s)));
    }

    // ---- §9.2 frame-pair grouping: a dense fan-out on one (a,b) frame pair collapses to ONE ----
    //
    // Several DISTINCT shared hashes all anchored at the same A frame and the same B frame must
    // yield ONE correspondence with weight = min(1.0, Σ weights), NOT one observation per hash.
    #[test]
    fn dense_fanout_collapses_to_one_correspondence() {
        let p = params();
        // 4 distinct hashes, each occ_a = 1, occ_b = 1 (weight 1.0 each), ALL at A-frame 5 / B-frame 9.
        let video = reader(vec![lm(10, 5), lm(11, 5), lm(12, 5), lm(13, 5)], 1000);
        let audio = reader(vec![lm(10, 9), lm(11, 9), lm(12, 9), lm(13, 9)], 1000);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert_eq!(pc.stats.raw_pairs, 4, "four raw landmark-pair observations");
        assert_eq!(pc.correspondences.len(), 1, "they collapse to ONE frame-pair correspondence");
        assert_eq!(pc.stats.grouped, 1);
        // Σ weight = 4.0, clamped to 1.0 (§9.2 min(1.0, Σ)).
        assert_eq!(pc.correspondences[0].weight, 1.0, "summed weight clamped to 1.0");

        // A partial sum below 1.0 is NOT clamped: two hashes with occ 2×2 ⇒ weight 0.5 each ⇒ Σ 1.0
        // exactly; use occ 2×3 ⇒ 1/sqrt(6) ≈ 0.408 each, two hashes on one frame pair ⇒ ≈ 0.816.
        let video2 = reader(vec![lm(20, 5), lm(20, 6), lm(21, 5), lm(21, 6)], 1000);
        let audio2 = reader(
            vec![lm(20, 9), lm(20, 10), lm(20, 11), lm(21, 9), lm(21, 10), lm(21, 11)],
            1000,
        );
        let (idx2, _) = AudioHashIndex::build(&[&audio2], &p);
        let pc2 = correspondences_for_pair(&video2, &idx2, 0, &fmt(), &p);
        // The (a=5, b=9) frame pair receives hash 20's 1/sqrt(6) and hash 21's 1/sqrt(6) ⇒ ≈ 0.816.
        let c59 = pc2
            .correspondences
            .iter()
            .find(|c| {
                let f = fmt();
                c.t_a_s == FingerprintBuilder::frame_center_time_s(&f, 5)
                    && c.t_b_s == FingerprintBuilder::frame_center_time_s(&f, 9)
            })
            .expect("(5,9) frame pair present");
        assert!((c59.weight - 2.0 / 6f64.sqrt()).abs() < 1e-12, "sub-1 sum not clamped: {}", c59.weight);
    }

    // ---- §9.2 time-stratified cap: NOT first-N; proportional; deterministic; early/mid/late ----
    //
    // Construct far more than `cap` correspondences spread across A's whole duration, with the
    // STRONGEST evidence deliberately placed in the LATE region, then cap. The retained set must
    // (a) be exactly `cap`, (b) include early, middle AND late correspondences (so it is not a
    // first-N prefix, which would drop the late region entirely), (c) be deterministic, and
    // (d) prefer higher weight within a region.
    #[test]
    fn time_stratified_cap_is_proportional_not_prefix() {
        let mut p = params();
        p.max_correspondences_per_pair = 60;
        // Duration ≈ 600 frames of A; spread 300 correspondences across it via 300 distinct hashes,
        // each a unique (a_frame, b_frame) ⇒ each its own correspondence (occ 1×1 ⇒ weight 1.0).
        // To exercise the "highest weight within a bin" rule, give LATE correspondences a higher
        // effective weight by stacking two hashes on the same late frame pair (Σ weight → ~? ); but
        // to keep weights varied yet ≤ 1 we instead vary occ to set distinct weights per region.
        let dur_frames = 600u64;
        let mut v = Vec::new();
        let mut a = Vec::new();
        // 300 single-hash correspondences at A-frames 0..300*2 (step 2), B-frame = A-frame + 50.
        for k in 0..300u32 {
            let h = 1000 + k;
            let af = k * 2; // 0..600
            let bf = af + 50;
            v.push(lm(h, af));
            a.push(lm(h, bf));
        }
        let video = reader(v, dur_frames);
        let audio = reader(a, 1000);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert!(pc.stats.capped, "the cap must have fired");
        assert_eq!(pc.correspondences.len(), 60, "retained exactly the cap");

        // Determinism: a second run is byte-identical.
        let pc2 = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert_eq!(
            serialize_correspondences(&pc.correspondences),
            serialize_correspondences(&pc2.correspondences),
            "the capped sample must be deterministic"
        );

        // Early / middle / late representation. A spans t(0)..t(599); split the RETAINED set into
        // thirds by t_a_s and require each third to be non-empty (a first-N prefix would leave the
        // last third empty).
        let f = fmt();
        let t0 = FingerprintBuilder::frame_center_time_s(&f, 0);
        let t_last = FingerprintBuilder::frame_center_time_s(&f, 599);
        let third = (t_last - t0) / 3.0;
        let mut early = 0;
        let mut mid = 0;
        let mut late = 0;
        for c in &pc.correspondences {
            if c.t_a_s < t0 + third {
                early += 1;
            } else if c.t_a_s < t0 + 2.0 * third {
                mid += 1;
            } else {
                late += 1;
            }
        }
        assert!(early > 0 && mid > 0 && late > 0, "early={early} mid={mid} late={late} must all be > 0");
        // Roughly proportional: with a uniform spread the three thirds should be within a small
        // factor of each other (not 59/1/0 as a prefix would give).
        assert!(early >= 10 && mid >= 10 && late >= 10, "each third roughly proportional");
        // Retained correspondences are sorted by (t_a_s, t_b_s).
        assert!(pc
            .correspondences
            .windows(2)
            .all(|w| (w[0].t_a_s, w[0].t_b_s) <= (w[1].t_a_s, w[1].t_b_s)));
    }

    // Within a bin the HIGHEST weight wins (tie-break by time). Cap to 1 per bin via a tiny cap and
    // place two correspondences in one early bin with different weights; the stronger must survive.
    #[test]
    fn cap_keeps_highest_weight_within_bin() {
        let mut p = params();
        p.max_correspondences_per_pair = 2;
        // Three frame pairs: two share an early bin (frames 0 and 1 → bin 0), one is far late.
        // Weight differs by occ_b: hash A occ 1×1 (w=1.0) at frame 0; hash B occ 1×4 (w=0.5) at
        // frame 1; hash C occ 1×1 (w=1.0) at the late frame. cap=2 over 3 grouped ⇒ stratify.
        let video = reader(vec![lm(1, 0), lm(2, 1), lm(3, 590)], 600);
        let audio = reader(
            vec![
                lm(1, 100),
                lm(2, 200),
                lm(2, 201),
                lm(2, 202),
                lm(2, 203),
                lm(3, 300),
            ],
            1000,
        );
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert!(pc.stats.capped);
        assert_eq!(pc.correspondences.len(), 2);
        let f = fmt();
        // The early bin keeps hash A (frame 0, weight 1.0), NOT hash B (frame 1, weight 0.5).
        let kept_a_frames: Vec<f64> = pc.correspondences.iter().map(|c| c.t_a_s).collect();
        assert!(kept_a_frames.contains(&FingerprintBuilder::frame_center_time_s(&f, 0)));
        assert!(!kept_a_frames.contains(&FingerprintBuilder::frame_center_time_s(&f, 1)), "weaker early correspondence dropped");
        assert!(kept_a_frames.contains(&FingerprintBuilder::frame_center_time_s(&f, 590)), "late kept");
    }

    // ---- §9.2 same-master pair: correspondences cluster on the true affine line ----
    //
    // Two excerpts of one master at a known frame offset share hashes; for the shared hashes the
    // (t_a_s, t_b_s) points lie on t_b = t_a − offset (B started `offset` later relative to A's
    // content, so the same content is at a SMALLER time in B). The dominant (t_b_s − t_a_s) must
    // equal −offset. This is exactly what boundary 5's consensus consumes.
    #[test]
    fn same_master_correspondences_cluster_on_true_line() {
        let p = params();
        // Model: master frame f appears in A at frame f and in B at frame f - OFF (B is the same
        // event observed from OFF frames earlier in its own timeline). Build shared hashes.
        const OFF: u32 = 120;
        let mut v = Vec::new();
        let mut a = Vec::new();
        for f in OFF..OFF + 200 {
            let h = 5000 + f; // distinct hash per frame ⇒ a clean 1×1 correspondence
            v.push(lm(h, f)); // A: at frame f
            a.push(lm(h, f - OFF)); // B: same content, OFF frames earlier
        }
        let video = reader(v, 2000);
        let audio = reader(a, 2000);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert!(pc.correspondences.len() >= 150, "rich shared set: {}", pc.correspondences.len());

        // Every correspondence has t_b_s − t_a_s == −OFF·frame_period (within fp). Histogram the
        // delta in frame_period units and require the dominant bucket to equal −OFF.
        let f = fmt();
        let fp = f.frame_period_s();
        let mut hist: HashMap<i64, u32> = HashMap::new();
        for c in &pc.correspondences {
            let d_frames = ((c.t_b_s - c.t_a_s) / fp).round() as i64;
            *hist.entry(d_frames).or_insert(0) += 1;
        }
        let (best, count) = hist.iter().max_by_key(|(_, &c)| c).map(|(&d, &c)| (d, c)).unwrap();
        assert_eq!(best, -(OFF as i64), "dominant t_b−t_a delta must equal the true −offset");
        assert_eq!(count as usize, pc.correspondences.len(), "ALL correspondences on the true line");
    }

    // ---- cross-event pair: independent events ⇒ few/scattered correspondences, no dominant line ----
    #[test]
    fn cross_event_few_scattered_no_dominant_line() {
        let p = params();
        // Two independent events: disjoint hash spaces ⇒ NO shared hashes ⇒ no correspondences.
        let video = reader((0..200u32).map(|k| lm(10_000 + k, k)).collect(), 1000);
        let audio = reader((0..200u32).map(|k| lm(20_000 + k, k)).collect(), 1000);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert!(pc.correspondences.is_empty(), "disjoint events share nothing");

        // A few INCIDENTAL shared hashes (chance collisions) at unrelated frames produce a handful
        // of scattered correspondences whose deltas do NOT concentrate on any single value.
        let mut v: Vec<Landmark> = (0..200u32).map(|k| lm(10_000 + k, k)).collect();
        let mut a: Vec<Landmark> = (0..200u32).map(|k| lm(20_000 + k, k)).collect();
        // Inject 6 coincidental shared hashes at scattered, inconsistent frame deltas.
        for (k, (af, bf)) in [(3u32, 190u32), (50, 12), (88, 170), (130, 5), (160, 60), (199, 100)]
            .into_iter()
            .enumerate()
        {
            let h = 30_000 + k as u32;
            v.push(lm(h, af));
            a.push(lm(h, bf));
        }
        let video2 = reader(v, 1000);
        let audio2 = reader(a, 1000);
        let (idx2, _) = AudioHashIndex::build(&[&audio2], &p);
        let pc2 = correspondences_for_pair(&video2, &idx2, 0, &fmt(), &p);
        assert_eq!(pc2.correspondences.len(), 6, "only the incidental collisions correspond");
        // No dominant line: the 6 deltas are all distinct, so the modal bucket holds just 1.
        let f = fmt();
        let fp = f.frame_period_s();
        let mut hist: HashMap<i64, u32> = HashMap::new();
        for c in &pc2.correspondences {
            *hist.entry(((c.t_b_s - c.t_a_s) / fp).round() as i64).or_insert(0) += 1;
        }
        let max_bucket = hist.values().copied().max().unwrap();
        assert_eq!(max_bucket, 1, "no dominant timing line for independent events");
    }

    // The correspondence stream restricts to the SELECTED audio_index: postings from other tracks
    // in the same index are not mixed in.
    #[test]
    fn correspondences_select_only_the_named_audio() {
        let p = params();
        let video = reader(vec![lm(77, 5)], 1000);
        let audio0 = reader(vec![lm(77, 10)], 1000);
        let audio1 = reader(vec![lm(77, 20), lm(77, 30)], 1000);
        let (idx, _) = AudioHashIndex::build(&[&audio0, &audio1], &p);
        // Against audio 0: one pair (occ_a 1 × occ_b 1).
        let pc0 = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert_eq!(pc0.correspondences.len(), 1);
        let f = fmt();
        assert_eq!(pc0.correspondences[0].t_b_s, FingerprintBuilder::frame_center_time_s(&f, 10));
        // Against audio 1: occ_b 2 ⇒ two frame pairs, weight 1/sqrt(2) each.
        let pc1 = correspondences_for_pair(&video, &idx, 1, &fmt(), &p);
        assert_eq!(pc1.correspondences.len(), 2);
        for c in &pc1.correspondences {
            assert!((c.weight - 1.0 / 2f64.sqrt()).abs() < 1e-12);
        }
    }

    // Full pair pipeline is deterministic: two runs serialize byte-identically (the §3 invariant-2
    // anchor at the correspondence level, including the times).
    #[test]
    fn correspondences_are_deterministic() {
        let p = params();
        let video = reader((0..50u32).map(|k| lm(100 + (k % 7), k)).collect(), 500);
        let audio = reader((0..50u32).map(|k| lm(100 + (k % 7), k + 3)).collect(), 500);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let a = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        let b = correspondences_for_pair(&video, &idx, 0, &fmt(), &p);
        assert_eq!(
            serialize_correspondences(&a.correspondences),
            serialize_correspondences(&b.correspondences)
        );
        assert_eq!(a.stats, b.stats);
    }

    // No NaN/Infinity reaches a correspondence even at the degenerate edges: a zero-duration video,
    // an empty index, and single-landmark inputs all produce finite (or empty) output (§20).
    #[test]
    fn no_nan_or_inf_at_edges() {
        let p = params();
        // Empty index.
        let video = reader(vec![lm(1, 0)], 100);
        let (empty_idx, _) = AudioHashIndex::build(&[], &p);
        let pc = correspondences_for_pair(&video, &empty_idx, 0, &fmt(), &p);
        assert!(pc.correspondences.is_empty());

        // Zero-duration video with a real shared hash: time math must stay finite.
        let video0 = reader(vec![lm(1, 0)], 0);
        let audio = reader(vec![lm(1, 0)], 100);
        let (idx, _) = AudioHashIndex::build(&[&audio], &p);
        let pc0 = correspondences_for_pair(&video0, &idx, 0, &fmt(), &p);
        for c in &pc0.correspondences {
            assert!(c.t_a_s.is_finite() && c.t_b_s.is_finite() && c.weight.is_finite());
            assert!(c.weight > 0.0 && c.weight <= 1.0);
        }
    }

    /// Deterministic byte serialization of a correspondence list, for the determinism gates: each
    /// correspondence is `t_a_s(f64 bits LE) ‖ t_b_s(f64 bits LE) ‖ weight(f64 bits LE)`.
    fn serialize_correspondences(cs: &[Correspondence]) -> Vec<u8> {
        let mut out = Vec::with_capacity(cs.len() * 24);
        for c in cs {
            out.extend_from_slice(&c.t_a_s.to_bits().to_le_bytes());
            out.extend_from_slice(&c.t_b_s.to_bits().to_le_bytes());
            out.extend_from_slice(&c.weight.to_bits().to_le_bytes());
        }
        out
    }
}
