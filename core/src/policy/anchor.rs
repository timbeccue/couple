//! Automatic anchor proposal / "+ anchor" placement (TODO-011) — the PURE, testable core behind
//! `PairSession.proposeAnchors()` and `PairSession.addAnchor()`. PLAN-CORE-SHELL-BOUNDARY Phase E
//! policy 5; relocated verbatim from `mac/Sources/CoupleKit/AnchorProposal.swift` (a RELOCATION,
//! not a behavior change — the numeric behavior is preserved EXACTLY). Pure geometry/scoring; the
//! shell owns the side effects (querying ranked onsets, probing per-window non-silence in BOTH
//! lanes to mark candidates usable, building Frames/measurements from the returned times).
//!
//! An anchor is only honest alignment evidence when it sits inside the current video/external
//! OVERLAP. The product rules (TODO-011 / UX-DECISIONS §2.2) bound the count on USABLE OVERLAP
//! duration:
//!   - Overlap ≥ 30 s  ⇒ THREE zoned anchors, one in each of [1,30]/[38,62]/[70,99] % of the
//!     OVERLAP span, each the sharpest usable onset in its zone.
//!   - 0 < overlap < 30 s ⇒ TWO well-separated usable anchors, kept ≥ ~25 % of the overlap apart.
//!   - No usable overlap, or two usable non-degenerate candidates can't be found ⇒ ZERO anchors.
//!     NEVER a misleading midpoint, NEVER exactly one.
//!
//! CONTRACT (the testable invariants):
//!   1. ALWAYS INSIDE THE OVERLAP — every returned time t satisfies start ≤ t ≤ end. No overlap ⇒
//!      empty.
//!   2. ≥30 s ⇒ THREE (one per zone, sorted); a degenerate zoned set falls through to the
//!      short-overlap rule rather than fabricating a pick.
//!   3. <30 s ⇒ TWO OR ZERO — never one, never a midpoint pair.
//!   4. NEVER EXACTLY ONE — the count is always 0, 2, or 3 from `propose`.
//!   5. "+ ANCHOR" STAYS INSIDE — `add_anchor_candidate` returns only an in-overlap time
//!      ≥ min separation from every existing anchor, else `None`.

use serde::{Deserialize, Serialize};

/// One ranked onset as the proposer reasons about it. Built by the shell from the engine's
/// score-ranked onsets after marking each `usable` from an RMS probe of BOTH lanes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorCandidate {
    /// Onset time on the video (camera) timeline, seconds.
    pub t: f64,
    /// Onset strength (the engine's onset score) — higher = sharper attack; used to order picks.
    pub score: f64,
    /// The candidate is a real, non-degenerate event with non-silent structure in BOTH mapped
    /// lanes. Only usable candidates are ever chosen.
    #[serde(default = "default_true")]
    pub usable: bool,
}

fn default_true() -> bool {
    true
}

/// A single zone of the overlap span (fractions in [0,1]) for the long-overlap proposal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Zone {
    pub lo: f64,
    pub hi: f64,
}

/// Tunables (validated starting values from UX-DECISIONS §2.2 / §4.5). Defaults match the
/// historical Swift behavior so the long-overlap path is unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorTuning {
    /// Usable-overlap threshold between the three-zoned and two-anchor regimes.
    pub long_overlap_min_s: f64,
    /// The three zones (fractions of the overlap span) for the long-overlap proposal.
    pub zones: Vec<Zone>,
    /// Minimum separation between the two short-overlap anchors, as a fraction of the overlap.
    pub short_min_separation_fraction: f64,
    /// Minimum separation for automatic "+ anchor", as a fraction of the overlap (UX §2.2: ≥ 8 %).
    pub add_min_separation_fraction: f64,
    /// Hard cap on total anchors (UX §2.2 / §6.9).
    pub max_anchors: usize,
}

impl Default for AnchorTuning {
    fn default() -> Self {
        Self {
            long_overlap_min_s: 30.0,
            zones: vec![
                Zone { lo: 0.01, hi: 0.30 },
                Zone { lo: 0.38, hi: 0.62 },
                Zone { lo: 0.70, hi: 0.99 },
            ],
            short_min_separation_fraction: 0.25,
            add_min_separation_fraction: 0.08,
            max_anchors: 4,
        }
    }
}

/// The closed overlap range on the video timeline, or `None` when there is no overlap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlap {
    pub start: f64,
    pub end: f64,
}

/// `propose` request: the overlap (or `null`), the ranked candidates, and the tuning (defaulted).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposeRequest {
    #[serde(default)]
    pub overlap: Option<Overlap>,
    pub candidates: Vec<AnchorCandidate>,
    #[serde(default)]
    pub tuning: AnchorTuning,
}

/// `addAnchor` request: the overlap, the existing anchor times, the candidates, and the tuning.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAnchorRequest {
    #[serde(default)]
    pub overlap: Option<Overlap>,
    pub existing: Vec<f64>,
    pub candidates: Vec<AnchorCandidate>,
    #[serde(default)]
    pub tuning: AnchorTuning,
}

/// The tagged request the single `cpl_policy_anchor_propose` symbol dispatches on.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    /// The automatic anchor proposal (`propose`).
    Propose(ProposeRequest),
    /// The automatic "+ anchor" pick (`addAnchorCandidate`).
    Add(AddAnchorRequest),
}

/// The response carried by the envelope `value`: the proposed anchor times (sorted), or the single
/// "+ anchor" time (`null` when nothing qualifies).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Response {
    /// `propose` ⇒ a (possibly empty) sorted list of anchor times.
    Propose(Vec<f64>),
    /// `addAnchor` ⇒ one time or `null`.
    Add(Option<f64>),
}

/// Pick the element with the maximum `score`. On ties this returns the LAST maximal element —
/// matching Swift's `max(by:)` ("if there are multiple equal maximum elements, returns the last
/// one") and Rust's own `Iterator::max_by` — so the relocation is numerically identical.
fn max_by_score(items: impl IntoIterator<Item = AnchorCandidate>) -> Option<AnchorCandidate> {
    items
        .into_iter()
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
}

/// Decide the automatic anchor times for a freshly proposed couple (TODO-011). Returns anchor
/// times on the video timeline, sorted ascending, ALL inside `overlap`. Count is 3 / 2 / 0 — never
/// exactly 1, never an out-of-overlap or midpoint pick.
pub fn propose(overlap: Option<Overlap>, candidates: &[AnchorCandidate], tuning: &AnchorTuning) -> Vec<f64> {
    let Some(overlap) = overlap else { return Vec::new() };
    let len = overlap.end - overlap.start;
    if len <= 0.0 {
        return Vec::new();
    }

    // Only in-overlap, usable candidates are ever eligible (invariant 1).
    let eligible: Vec<AnchorCandidate> = candidates
        .iter()
        .copied()
        .filter(|c| c.usable && c.t >= overlap.start && c.t <= overlap.end)
        .collect();

    if len >= tuning.long_overlap_min_s {
        let zoned = propose_zoned(overlap, &eligible, tuning);
        // A long overlap should yield three zoned anchors; only if the zones can't supply a
        // non-degenerate set do we fall through to the short-overlap rule.
        if zoned.len() >= 2 {
            return zoned;
        }
    }

    propose_short_pair(overlap, &eligible, tuning)
}

/// Three anchors, one per zone, each the sharpest usable onset inside that zone of the overlap. A
/// zone with no usable onset contributes nothing (no midpoint). Distinct times only; sorted.
fn propose_zoned(overlap: Overlap, eligible: &[AnchorCandidate], tuning: &AnchorTuning) -> Vec<f64> {
    let len = overlap.end - overlap.start;
    let mut picks: Vec<f64> = Vec::new();
    for zone in &tuning.zones {
        let a = overlap.start + zone.lo * len;
        let b = overlap.start + zone.hi * len;
        let best = max_by_score(
            eligible
                .iter()
                .copied()
                // `!picks.contains(&c.t)` uses exact float equality, matching the Swift body.
                .filter(|c| c.t >= a && c.t <= b && !picks.contains(&c.t)),
        );
        if let Some(best) = best {
            picks.push(best.t);
        }
    }
    picks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    picks
}

/// Two well-separated usable anchors for a short overlap. Returns [] when no two usable candidates
/// are far enough apart (invariant 3 — never one, never a midpoint).
fn propose_short_pair(overlap: Overlap, eligible: &[AnchorCandidate], tuning: &AnchorTuning) -> Vec<f64> {
    if eligible.len() < 2 {
        return Vec::new();
    }
    let len = overlap.end - overlap.start;
    let min_sep = tuning.short_min_separation_fraction * len;
    let mid = overlap.start + len / 2.0;

    // Prefer one clearly-earlier and one clearly-later anchor: the strongest usable onset in each
    // half, if they clear the separation.
    let early = max_by_score(eligible.iter().copied().filter(|c| c.t < mid));
    let late = max_by_score(eligible.iter().copied().filter(|c| c.t >= mid));
    if let (Some(early), Some(late)) = (early, late) {
        if (late.t - early.t).abs() >= min_sep {
            return sorted_pair(early.t, late.t);
        }
    }

    // Otherwise take the strongest candidate and the strongest OTHER candidate ≥ min_sep away.
    // Considering anchors strongest-first keeps the more distinctive shape in the pair. The sort
    // is stable (matching Swift's stable `sorted(by:)`), preserving order among equal scores.
    let mut by_score_desc: Vec<AnchorCandidate> = eligible.to_vec();
    by_score_desc.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    });
    for anchor in &by_score_desc {
        let partner = max_by_score(
            eligible.iter().copied().filter(|c| (c.t - anchor.t).abs() >= min_sep),
        );
        if let Some(partner) = partner {
            return sorted_pair(anchor.t, partner.t);
        }
    }
    Vec::new()
}

fn sorted_pair(a: f64, b: f64) -> Vec<f64> {
    let mut v = vec![a, b];
    v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// The automatic "+ anchor" pick (TODO-011 / UX §2.2): the strongest usable onset inside the
/// overlap and ≥ `add_min_separation_fraction` of the overlap from every existing anchor,
/// respecting the cap. Returns `None` when the cap is reached, the overlap is degenerate, or no
/// usable in-overlap onset is far enough — NEVER an onset outside the overlap (invariant 5).
pub fn add_anchor_candidate(
    overlap: Option<Overlap>,
    existing: &[f64],
    candidates: &[AnchorCandidate],
    tuning: &AnchorTuning,
) -> Option<f64> {
    if existing.len() >= tuning.max_anchors {
        return None;
    }
    let overlap = overlap?;
    let len = overlap.end - overlap.start;
    if len <= 0.0 {
        return None;
    }
    let min_sep = tuning.add_min_separation_fraction * len;
    max_by_score(
        candidates
            .iter()
            .copied()
            .filter(|c| c.usable && c.t >= overlap.start && c.t <= overlap.end)
            .filter(|c| existing.iter().all(|e| (e - c.t).abs() >= min_sep)),
    )
    .map(|c| c.t)
}

/// Dispatch a decoded `Request` to its pure function.
pub fn dispatch(req: &Request) -> Response {
    match req {
        Request::Propose(r) => Response::Propose(propose(r.overlap, &r.candidates, &r.tuning)),
        Request::Add(r) => Response::Add(add_anchor_candidate(r.overlap, &r.existing, &r.candidates, &r.tuning)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov(a: f64, b: f64) -> Option<Overlap> {
        Some(Overlap { start: a, end: b })
    }
    fn cand(t: f64, score: f64, usable: bool) -> AnchorCandidate {
        AnchorCandidate { t, score, usable }
    }
    /// Dense, evenly-spaced, all-usable onsets across [t0, t1]; score rises with index.
    fn usable_onsets(t0: f64, t1: f64, step: f64) -> Vec<AnchorCandidate> {
        let mut out = Vec::new();
        let mut t = t0;
        let mut i = 0.0;
        while t <= t1 {
            out.push(cand(t, 1.0 + i, true));
            t += step;
            i += 1.0;
        }
        out
    }
    fn inside(ts: &[f64], a: f64, b: f64) -> bool {
        ts.iter().all(|&t| t >= a && t <= b)
    }

    // (1) Full overlap (≥30 s) ⇒ three anchors, one per zone, inside, sorted.
    #[test]
    fn full_overlap_proposes_three_inside_overlap() {
        let picks = propose(ov(0.0, 120.0), &usable_onsets(0.0, 120.0, 1.0), &AnchorTuning::default());
        assert_eq!(picks.len(), 3);
        assert!(inside(&picks, 0.0, 120.0));
        let mut sorted = picks.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(picks, sorted);
        assert!(picks[0] <= 36.0);
        assert!(picks[1] >= 45.0 && picks[1] <= 74.0);
        assert!(picks[2] >= 84.0);
    }

    // (2) Partial overlap (≥30 s, shorter than camera) ⇒ three, none past the external content.
    #[test]
    fn partial_overlap_keeps_three_inside_the_overlap_not_camera_duration() {
        let picks = propose(ov(40.0, 100.0), &usable_onsets(0.0, 200.0, 1.0), &AnchorTuning::default());
        assert_eq!(picks.len(), 3);
        assert!(picks.iter().all(|&t| (40.0..=100.0).contains(&t)));
    }

    // (3) Short overlap (<30 s) ⇒ exactly two, ≥25% apart, inside.
    #[test]
    fn short_overlap_proposes_two_separated_inside_overlap() {
        let picks = propose(ov(0.0, 20.0), &usable_onsets(0.0, 20.0, 0.5), &AnchorTuning::default());
        assert_eq!(picks.len(), 2);
        assert!(inside(&picks, 0.0, 20.0));
        assert!((picks[1] - picks[0]).abs() >= 0.25 * 20.0);
    }

    // (4) Zero/degenerate overlap ⇒ zero, never one.
    #[test]
    fn no_overlap_proposes_zero_never_one() {
        assert!(propose(None, &usable_onsets(0.0, 60.0, 1.0), &AnchorTuning::default()).is_empty());
        assert!(propose(ov(10.0, 10.0), &usable_onsets(0.0, 60.0, 1.0), &AnchorTuning::default()).is_empty());
    }

    // (4b) Short overlap with one usable candidate ⇒ zero.
    #[test]
    fn short_overlap_with_one_usable_candidate_proposes_zero() {
        let mut cands: Vec<AnchorCandidate> =
            usable_onsets(0.0, 20.0, 0.5).iter().map(|c| cand(c.t, c.score, false)).collect();
        cands[5] = cand(cands[5].t, 99.0, true);
        assert!(propose(ov(0.0, 20.0), &cands, &AnchorTuning::default()).is_empty());
    }

    // (4c) Short overlap with bunched usable candidates ⇒ zero.
    #[test]
    fn short_overlap_with_bunched_candidates_proposes_zero() {
        let cands = [
            cand(9.0, 5.0, true),
            cand(10.0, 6.0, true),
            cand(2.0, 1.0, false),
            cand(18.0, 1.0, false),
        ];
        assert!(propose(ov(0.0, 20.0), &cands, &AnchorTuning::default()).is_empty());
    }

    // (5) Long overlap all degenerate ⇒ zero.
    #[test]
    fn long_overlap_all_degenerate_proposes_zero() {
        let cands: Vec<AnchorCandidate> =
            usable_onsets(0.0, 120.0, 1.0).iter().map(|c| cand(c.t, c.score, false)).collect();
        assert!(propose(ov(0.0, 120.0), &cands, &AnchorTuning::default()).is_empty());
    }

    // (6) Strongest out-of-overlap onset is ignored.
    #[test]
    fn strongest_outside_overlap_is_ignored() {
        let mut cands = usable_onsets(40.0, 100.0, 1.0);
        cands.push(cand(150.0, 9999.0, true));
        let picks = propose(ov(40.0, 100.0), &cands, &AnchorTuning::default());
        assert!(inside(&picks, 40.0, 100.0));
        assert!(!picks.contains(&150.0));
    }

    // (7) "+ anchor" picks the strongest in-overlap, separated-from-existing candidate.
    #[test]
    fn add_anchor_picks_strongest_inside_separated_from_existing() {
        let cands = [cand(10.0, 5.0, true), cand(50.0, 9.0, true), cand(52.0, 10.0, true)];
        // 52 is < 8% of 100 (= 8 s) from the existing 50 ⇒ excluded ⇒ 10 stays.
        let t = add_anchor_candidate(ov(0.0, 100.0), &[50.0], &cands, &AnchorTuning::default());
        assert_eq!(t, Some(10.0));
    }

    // (8) "+ anchor" never chooses outside the overlap; no overlap ⇒ None.
    #[test]
    fn add_anchor_never_chooses_outside_overlap() {
        let cands = [cand(30.0, 5.0, true), cand(90.0, 9999.0, true)];
        assert_eq!(add_anchor_candidate(ov(0.0, 60.0), &[], &cands, &AnchorTuning::default()), Some(30.0));
        assert_eq!(add_anchor_candidate(None, &[], &cands, &AnchorTuning::default()), None);
    }

    // (9) "+ anchor" respects the cap and returns None when nothing qualifies.
    #[test]
    fn add_anchor_cap_and_no_qualifying_returns_none() {
        let usable = [cand(50.0, 9.0, true)];
        assert_eq!(
            add_anchor_candidate(ov(0.0, 100.0), &[10.0, 30.0, 60.0, 90.0], &usable, &AnchorTuning::default()),
            None
        );
        let unusable = [cand(50.0, 9.0, false)];
        assert_eq!(add_anchor_candidate(ov(0.0, 100.0), &[], &unusable, &AnchorTuning::default()), None);
    }

    // (10) The count is ALWAYS 0, 2, or 3 — never exactly one — across a sweep. The core invariant.
    #[test]
    fn proposal_count_is_never_exactly_one() {
        let mut len: f64 = 0.0;
        while len <= 130.0 {
            let end = len.max(0.0001);
            for &step in &[0.5, 3.0, 17.0] {
                let overlap = if len == 0.0 { None } else { ov(0.0, end) };
                let picks = propose(overlap, &usable_onsets(0.0, len, step), &AnchorTuning::default());
                assert_ne!(picks.len(), 1, "len={len} step={step}");
                assert!(inside(&picks, 0.0, end), "len={len} step={step}");
            }
            len += 2.5;
        }
    }

    // The default tuning round-trips and the tagged request dispatches both ops.
    #[test]
    fn request_dispatch_propose_and_add() {
        let propose_req: Request = serde_json::from_str(
            r#"{"op":"propose","overlap":{"start":0,"end":120},
                "candidates":[{"t":10,"score":1,"usable":true},{"t":50,"score":2,"usable":true},
                              {"t":100,"score":3,"usable":true}]}"#,
        )
        .unwrap();
        match dispatch(&propose_req) {
            Response::Propose(ts) => assert_eq!(ts.len(), 3),
            _ => panic!("expected Propose"),
        }
        let add_req: Request = serde_json::from_str(
            r#"{"op":"add","overlap":{"start":0,"end":60},"existing":[],
                "candidates":[{"t":30,"score":5,"usable":true},{"t":90,"score":9,"usable":true}]}"#,
        )
        .unwrap();
        match dispatch(&add_req) {
            Response::Add(t) => assert_eq!(t, Some(30.0)),
            _ => panic!("expected Add"),
        }
    }
}
