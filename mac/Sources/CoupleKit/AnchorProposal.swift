// Automatic anchor proposal / "+ anchor" placement (TODO-011) — the PURE, testable
// core behind `PairSession.proposeAnchors()` and `PairSession.addAnchor()`. An anchor
// is only honest alignment evidence when it sits inside the current video/external
// OVERLAP: outside the overlap one lane has no content, so the loupe can't show two
// corresponding shapes. The "how many anchors, and where?" decision is therefore lifted
// here (out of the app target / out of the SwiftUI model) where `swift test` can lock it
// down — mirroring the codebase's other pure-decision idioms (RowMergePolicy.plan,
// PoolSelection.select, MatchQualityPolicy.decide, RepairDropPolicy.decide,
// MediaRemovalPolicy.plan). The app owns the side effects (querying the engine's ranked
// onsets, probing per-window non-silence in BOTH lanes to mark candidates usable, then
// building Frames/measurements from the returned times); this type only decides the
// anchor COUNT and which onset times to keep, so the automatic proposal and the
// automatic "+ anchor" cannot drift apart or invent their own placement rules.
//
// The product rules (TODO-011 / UX-DECISIONS §2.2) bound the count on USABLE OVERLAP
// duration — NOT the camera clip's total duration (the old fall-back to camera duration
// was exactly the bug: a <30 s or zero overlap placed anchors where the external source
// had no content):
//   - Overlap ≥ 30 s  ⇒ THREE zoned anchors, one in each of [1,30] / [38,62] / [70,99] %
//     of the OVERLAP span, each the sharpest usable onset in its zone.
//   - 0 < overlap < 30 s ⇒ TWO well-separated, human-readable anchors chosen from the
//     ranked onsets (sharpest first as the proxy for a clear shape), kept ≥ ~25% of the
//     overlap apart and preferably one earlier / one later.
//   - No usable overlap, or two usable non-degenerate candidates can't be found ⇒ ZERO
//     anchors (the manual-rescue state). NEVER a misleading midpoint, NEVER exactly one.
// This preserves the core invariant: the automatic set is either empty or 2-4 anchors.
//
// Usability: a candidate is `usable` when it is a real waveform event with non-silent,
// non-degenerate structure in BOTH mapped lanes (the live path derives this from the
// onset score + an RMS probe of both tracks at the candidate). Silence / sustained /
// periodic passages are not usable. A degenerate (empty/inside-out) overlap has no
// usable candidates by construction, so the same code path yields zero.
//
// CONTRACT (the testable invariants):
//   1. ALWAYS INSIDE THE OVERLAP — every returned time t satisfies
//      overlap.start ≤ t ≤ overlap.end. With no overlap the result is empty. This holds
//      for the long-overlap zoned path AND the short-overlap pair AND "+ anchor".
//   2. ≥30 s ⇒ THREE — a long overlap returns three times, one drawn from each zone,
//      sorted ascending. A zone with no usable onset contributes no midpoint; if that
//      leaves the set degenerate the result drops to the short-overlap rule rather than
//      faking a pick.
//   3. <30 s ⇒ TWO OR ZERO — a short overlap returns exactly two well-separated usable
//      onsets (≥ minSeparationFraction of the overlap apart) or, when two such
//      candidates don't exist, ZERO. It NEVER returns one and NEVER a midpoint pair.
//   4. NEVER EXACTLY ONE — the count is always 0, 2, or 3 from this proposer (the app's
//      "+ anchor"/remove keep it within 2-4 thereafter).
//   5. "+ ANCHOR" STAYS INSIDE — `addAnchorCandidate` only ever returns a time that is
//      inside the overlap and ≥ minSeparation from every existing anchor; with no usable
//      in-overlap onset it returns nil (no anchor added), never an out-of-overlap onset.
//
// RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase E): the DECISION (the pure geometry/scoring)
// now lives in `couple_core::policy::anchor` and is reached over the FFI
// (`cpl_policy_anchor_propose`, a single tagged endpoint dispatching `propose`/`add`).
// These functions are thin WRAPPERS; the numeric behavior is byte-faithful. On a core error
// (a malformed/stale request — unreachable with this in-process call) `propose` maps to the
// CONSERVATIVE EMPTY set and `addAnchorCandidate` to `nil` — the manual-rescue state, NEVER
// a fabricated, out-of-overlap, or count-of-one anchor (CONTRACT).

import CCouple
import Foundation

public enum AnchorProposal {
    /// One ranked onset as the proposer reasons about it. The live path builds these from
    /// the engine's `TrackHandle.onsets` (already score-ranked) after marking each
    /// `usable` from an RMS probe of BOTH lanes; tests construct them synthetically.
    public struct Candidate: Sendable, Equatable {
        /// Onset time on the video (camera) timeline, seconds.
        public var t: Double
        /// Onset strength (the engine's onset score) — higher = sharper attack out of
        /// silence, the §4.5 proxy for "a clear, isolated shape." Used to order picks.
        public var score: Double
        /// The candidate is a real, non-degenerate event with non-silent structure in
        /// BOTH mapped lanes (flat silence / sustained / periodic passages are false).
        /// Only usable candidates are ever chosen.
        public var usable: Bool

        public init(t: Double, score: Double, usable: Bool = true) {
            self.t = t
            self.score = score
            self.usable = usable
        }
    }

    /// Tunables (validated starting values from UX-DECISIONS §2.2 / §4.5). Defaults match
    /// the historical behavior so the long-overlap path is unchanged.
    public struct Tuning: Sendable, Equatable {
        /// Usable-overlap threshold between the three-zoned and two-anchor regimes.
        public var longOverlapMinS: Double
        /// The three zones (fractions of the overlap span) for the long-overlap proposal.
        public var zones: [(lo: Double, hi: Double)]
        /// Minimum separation between the two short-overlap anchors, as a fraction of the
        /// overlap (TODO-011: at least ~25%).
        public var shortMinSeparationFraction: Double
        /// Minimum separation for automatic "+ anchor", as a fraction of the overlap
        /// (UX §2.2: ≥ 8%).
        public var addMinSeparationFraction: Double
        /// Hard cap on total anchors (UX §2.2 / §6.9).
        public var maxAnchors: Int

        public init(longOverlapMinS: Double = 30,
                    zones: [(lo: Double, hi: Double)] = [(0.01, 0.30), (0.38, 0.62), (0.70, 0.99)],
                    shortMinSeparationFraction: Double = 0.25,
                    addMinSeparationFraction: Double = 0.08,
                    maxAnchors: Int = 4) {
            self.longOverlapMinS = longOverlapMinS
            self.zones = zones
            self.shortMinSeparationFraction = shortMinSeparationFraction
            self.addMinSeparationFraction = addMinSeparationFraction
            self.maxAnchors = maxAnchors
        }

        public static func == (a: Tuning, b: Tuning) -> Bool {
            a.longOverlapMinS == b.longOverlapMinS
                && a.shortMinSeparationFraction == b.shortMinSeparationFraction
                && a.addMinSeparationFraction == b.addMinSeparationFraction
                && a.maxAnchors == b.maxAnchors
                && a.zones.count == b.zones.count
                && zip(a.zones, b.zones).allSatisfy { $0.lo == $1.lo && $0.hi == $1.hi }
        }
    }

    /// Decide the automatic anchor times for a freshly proposed couple (TODO-011).
    ///
    /// Returns anchor times on the video timeline, sorted ascending, ALL inside `overlap`.
    /// The count is 3 (overlap ≥ `longOverlapMinS`), 2 (shorter usable overlap with two
    /// separated usable candidates), or 0 (no usable overlap, or two usable candidates
    /// can't be found) — never exactly 1, never an out-of-overlap or midpoint pick.
    ///
    /// - Parameters:
    ///   - overlap: the current video/external overlap on the video timeline, or `nil`
    ///     when there is no overlap (degenerate / disjoint). `nil` ⇒ empty result.
    ///   - candidates: ranked onsets (the engine already orders them score-descending;
    ///     this function does not assume any order and re-sorts where it matters). Only
    ///     candidates inside the overlap and marked `usable` are ever chosen.
    ///   - tuning: thresholds; defaults reproduce the historical long-overlap behavior.
    public static func propose(overlap: ClosedRange<Double>?,
                               candidates: [Candidate],
                               tuning: Tuning = Tuning()) -> [Double] {
        var req: [String: Any] = [
            "op": "propose",
            "candidates": candidates.map(\.wireDict),
            "tuning": tuning.wireDict,
        ]
        req["overlap"] = overlap.map(Self.overlapDict) ?? NSNull()
        // CONSERVATIVE fallback on any failure: the EMPTY set (manual-rescue), never a fabricated
        // or count-of-one proposal.
        return PolicyEnvelope.decodeValueArray(callPolicy(req))?.compactMap { ($0 as? NSNumber)?.doubleValue } ?? []
    }

    /// The automatic "+ anchor" pick (TODO-011 / UX §2.2): the strongest usable onset that
    /// is inside the overlap and ≥ `addMinSeparationFraction` of the overlap from every
    /// existing anchor, respecting the cap. Returns `nil` (add nothing) when the cap is
    /// reached, the overlap is degenerate, or no usable in-overlap onset is far enough
    /// from the current anchors — NEVER an onset outside the overlap (invariant 5).
    public static func addAnchorCandidate(overlap: ClosedRange<Double>?,
                                          existing: [Double],
                                          candidates: [Candidate],
                                          tuning: Tuning = Tuning()) -> Double? {
        var req: [String: Any] = [
            "op": "add",
            "existing": existing,
            "candidates": candidates.map(\.wireDict),
            "tuning": tuning.wireDict,
        ]
        req["overlap"] = overlap.map(Self.overlapDict) ?? NSNull()
        // CONSERVATIVE fallback on any failure: add NOTHING (nil), never an out-of-overlap onset.
        // The `add` value is a bare number or JSON null; decodeValueArray expects an array, so use
        // the object-less decode path: parse the envelope and read the scalar `value`.
        guard let value = PolicyEnvelope.decodeScalar(callPolicy(req)),
              let t = (value as? NSNumber)?.doubleValue else {
            return nil
        }
        return t
    }

    /// Encode a request dict, call `cpl_policy_anchor_propose`, and return the C string (nil on an
    /// encode failure). The caller's envelope decoder applies the conservative fallback.
    private static func callPolicy(_ req: [String: Any]) -> UnsafeMutablePointer<CChar>? {
        guard let data = try? JSONSerialization.data(withJSONObject: req),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return cpl_policy_anchor_propose(json)
    }

    /// The core `Overlap` wire dict for a closed range.
    private static func overlapDict(_ ov: ClosedRange<Double>) -> [String: Any] {
        ["start": ov.lowerBound, "end": ov.upperBound]
    }
}

extension AnchorProposal.Candidate {
    /// The core `AnchorCandidate` wire dict (`{"t","score","usable"}`).
    var wireDict: [String: Any] { ["t": t, "score": score, "usable": usable] }
}

extension AnchorProposal.Tuning {
    /// The core `AnchorTuning` wire dict (zones become `{lo,hi}` objects, matching core's `Zone`).
    var wireDict: [String: Any] {
        ["longOverlapMinS": longOverlapMinS,
         "zones": zones.map { ["lo": $0.lo, "hi": $0.hi] },
         "shortMinSeparationFraction": shortMinSeparationFraction,
         "addMinSeparationFraction": addMinSeparationFraction,
         "maxAnchors": maxAnchors]
    }
}
