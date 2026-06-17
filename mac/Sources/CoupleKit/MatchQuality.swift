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
//
// RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase E): the DECISION now lives in
// `couple_core::policy::match_quality` and is reached over the FFI
// (`cpl_policy_match_quality`). This type is a thin WRAPPER: it builds the request
// JSON, calls core, and decodes the uniform `{ok,value|error}` envelope. The
// user-facing `MatchQuality`/`SelectionProvenance` enums and all strings stay here.
// On a core error (a malformed/stale request — which should never happen with this
// in-process call) it maps to the CONSERVATIVE fallback `.questionable` — never
// silently green (CONTRACT honesty posture).

import CCouple
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
        // Build the camelCase request the core policy expects and round-trip through
        // `cpl_policy_match_quality` (the decision now lives in
        // `couple_core::policy::match_quality`). manualMatchConfident == nil serializes
        // as JSON null ("not yet evaluated"), which core treats as not-confident.
        var req: [String: Any] = [
            "forced": forced,
            "selection": selection.wireValue,
            "autoVerdict": autoVerdict.wireValue,
            "manualMatchConfident": manualMatchConfident as Any,
        ]
        if manualMatchConfident == nil { req["manualMatchConfident"] = NSNull() }
        let value: [String: Any]?
        if let data = try? JSONSerialization.data(withJSONObject: req),
           let json = String(data: data, encoding: .utf8) {
            value = PolicyEnvelope.decodeValue(cpl_policy_match_quality(json))
        } else {
            value = nil
        }
        return quality(fromEnvelopeValue: value)
    }

    /// Map a decoded envelope `value` object to the UI quality, applying the CONSERVATIVE
    /// fallback. A nil value (core returned an `{ok:false,error}` envelope, or the call/parse
    /// failed) or an unknown/missing `quality` string ⇒ `.questionable` — an honest "check
    /// this", NEVER silently green. Internal so a test can drive the error path (a stale/invalid
    /// shape ⇒ the conservative fallback) without reaching the C ABI (PLAN §3.2).
    /// `@inline(never)` keeps a standalone symbol so the test can link it (the optimizer would
    /// otherwise inline this sole-caller helper into `decide` and emit no symbol).
    @inline(never)
    static func quality(fromEnvelopeValue value: [String: Any]?) -> MatchQuality {
        guard let quality = (value?["quality"] as? String).flatMap(MatchQuality.init(wire:)) else {
            return .questionable
        }
        return quality
    }
}

extension MatchQuality {
    /// The core wire string (`couple_core::policy::match_quality::MatchQuality`, camelCase).
    var wireValue: String {
        switch self {
        case .confident: return "confident"
        case .questionable: return "questionable"
        case .none: return "none"
        }
    }

    /// Decode a core wire string back into the UI enum; nil on an unknown value.
    init?(wire: String) {
        switch wire {
        case "confident": self = .confident
        case "questionable": self = .questionable
        case "none": self = .none
        default: return nil
        }
    }
}

extension SelectionProvenance {
    /// The core wire string — identical to the persisted `rawValue`.
    var wireValue: String { rawValue }
}
