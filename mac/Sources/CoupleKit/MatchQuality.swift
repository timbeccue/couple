// Match-quality decision (TODO-009) — the PURE, testable core behind
// `CoupleRow.verdict`. The verdict dot's color is the answer to "does the
// CURRENTLY SELECTED video/audio pair have confident match evidence?" — a fact
// about the recording, NOT a fact about the user's actions. Selecting a source is
// a decision; it is not evidence that the source matches. So a manual pick must
// stay questionable UNLESS that specific pair independently clears the SAME
// confident-match classifier an automatic confident pairing uses.
//
// The three signals TODO-009 separates are kept strictly distinct here:
//   - SELECTION (which audio, and whether it was auto-resolved or manually chosen)
//     is an input, never a promoter.
//   - MATCH QUALITY (this function's output) derives ONLY from classifier/resolver
//     evidence for the selected pair.
//   - APPROVAL and ALIGNMENT FIT (RMS) are deliberately NOT inputs: approving a
//     couple, a successful auto-sync, or a low alignment RMS must never turn the
//     dot green. (Alignment fit stays green in its own display while this can be
//     amber.)
//
// The decision is expressible over a handful of flags alone, so it is lifted here
// (out of the app target) where `swift test` can lock it down, mirroring the
// codebase's other pure-decision idioms (RowMergePolicy.plan, PoolSelection.select,
// candidateOverflows). The app maps its UI `Verdict` from this and owns the side
// effect of obtaining a manual pair's evidence (re-running the SAME classifier).
//
// CONTRACT (the testable invariants):
//   1. A MANUAL PICK ALONE NEVER GOES GREEN — `.manual` selection is `.questionable`
//      unless `manualMatchConfident == true`. The bug this fixes was treating any
//      non-nil selection as good.
//   2. SAME STANDARD FOR MANUAL AND AUTOMATIC — `manualMatchConfident` must be
//      computed from the resolver's classifier (the app passes the result of the
//      SAME `matchPair`/`verify` path the auto resolution used), so the only way to
//      green a manual pick is to clear the automatic confident bar.
//   3. FORCED STAYS QUESTIONABLE — a kept below-floor manual pairing is always
//      `.questionable`, regardless of any other signal (honest, SPEC-V2 §3.6).
//   4. NO SELECTION FALLS BACK TO THE RESOLVER — with no selected pair the verdict
//      is the resolver's own `autoVerdict` (confident/partial ⇒ good, ambiguous ⇒
//      questionable, no-match ⇒ none) so an unselected ambiguous/no-match row is
//      never a coin-flip.
//   5. AN AUTOMATIC SELECTION TRUSTS THE RESOLVER — an auto-resolved selection takes
//      `autoVerdict` directly (the resolver already classified that pair); no
//      separate manual evidence is needed or consulted.

import Foundation

/// Three-category pairing quality (UX-DECISIONS-V2 §V2.3) — the evidence-based dot
/// color, not a continuous score. The app's UI `Verdict` maps 1:1 from this.
public enum MatchQuality: Sendable, Equatable {
    /// The selected pair independently meets the confident-match criteria.
    case confident
    /// A human should check this: an ambiguous resolution, a manual pick whose pair
    /// did not clear the classifier, or a kept below-floor (`forced`) pairing.
    case questionable
    /// No supported match for this video (and nothing selected to vouch for).
    case none
}

/// How the row's currently selected audio came to be selected (TODO-009). Persisted
/// (round-tripped by the queue store) so a restored manual pick is not silently
/// re-classified as an automatic one. `pinned` manual picks are `.manual`.
public enum SelectionProvenance: String, Sendable, Codable {
    /// No audio is selected (ambiguous / no-match carries no selection).
    case none
    /// The resolver auto-selected this source for a confident/partial/etc. row.
    case automatic
    /// The user chose this source (candidate pick or pool drag); the row is pinned.
    case manual
}

public enum MatchQualityPolicy {
    /// Decide the evidence-based match quality for a row (TODO-009).
    ///
    /// - Parameters:
    ///   - forced: the pairing is a kept below-floor manual choice (always
    ///     questionable, honest — overrides every other signal).
    ///   - selection: how the current audio was selected (`.none`/`.automatic`/`.manual`).
    ///   - autoVerdict: the resolver's own quality for this row's resolution
    ///     (`RowState.verdict`) — the evidence for an automatic selection or the
    ///     fallback when nothing is selected.
    ///   - manualMatchConfident: for a `.manual` selection ONLY, whether that
    ///     specific pair independently cleared the confident classifier (the app
    ///     supplies the result of the SAME `matchPair`/`verify` path the resolver
    ///     uses). `nil` means "not yet evaluated" and is treated as not-confident, so
    ///     a manual pick is never green until real evidence says so. Ignored for
    ///     non-manual selections.
    public static func decide(forced: Bool,
                              selection: SelectionProvenance,
                              autoVerdict: MatchQuality,
                              manualMatchConfident: Bool?) -> MatchQuality {
        // (3) A below-floor forced pairing is always questionable — it is kept on
        //     purpose but the evidence does not support it.
        if forced { return .questionable }
        switch selection {
        case .none:
            // (4) Nothing selected ⇒ the resolver's verdict (never a coin-flip).
            return autoVerdict
        case .automatic:
            // (5) The resolver picked and classified this pair ⇒ trust its verdict.
            return autoVerdict
        case .manual:
            // (1)/(2) A manual pick is green ONLY when that exact pair independently
            //         cleared the same confident classifier; otherwise it stays
            //         questionable no matter how it was selected.
            return manualMatchConfident == true ? .confident : .questionable
        }
    }
}
