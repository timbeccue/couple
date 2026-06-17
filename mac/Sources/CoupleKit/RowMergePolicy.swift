// Re-resolution merge policy (TODO-007) — the PURE, testable core behind
// `AppModel.mergeResolution`. When new media is imported, the whole pool is
// re-resolved and each fresh `ResolvedRow` must be folded into the EXISTING queue
// row. The hard part is not the measurement (that is the resolver's job, and it
// stays authoritative) but the *application*: an unapproved, unpinned unresolved
// row must adopt the newly confident result WHOLESALE — selection, candidates,
// and the session/sync that follow — exactly as if both files had been imported
// together; while an approved or manually pinned row must NEVER have its chosen
// source changed silently (new evidence may only surface as extra alternates).
//
// That decision is expressible over content keys + two protection flags alone, so
// it is lifted here (out of the app target) where `swift test` can lock it down.
// AppModel owns the side effects (rebuild the PairSession, enqueue auto-sync,
// reset the `forced` chip); this type only decides WHAT the merged row should be,
// mirroring the codebase's other pure-decision idioms (PoolPreviewAction,
// TransportPlan, candidateOverflows).
//
// CONTRACT (the testable invariants):
//   1. PROTECTED ROWS ARE STABLE — when `approved || pinned`, the selected source
//      and the resolution display are preserved; only genuinely-new candidate keys
//      are appended (order preserved, never reordered), and the session is never
//      rebuilt. This is the SPEC-V2 §3.6 stability rule; `approved`/`pinned` are
//      the protection boundary and nothing here weakens it.
//   2. UNPROTECTED ROWS TAKE THE FRESH RESOLUTION WHOLE — selection, candidates,
//      and the display all become the fresh row's, so the result equals a couple
//      built fresh from the same resolution. A weak/plausible candidate that the
//      resolver did NOT select stays a non-selecting alternate (it appears in the
//      candidate list without promoting the row) because selection mirrors the
//      resolver's `selected` verbatim — this layer never invents a selection.
//   3. SESSION LIFECYCLE FIRES AT MOST ONCE — `rebuildSession` is true only when
//      the effective selected key actually changed, so a re-resolve that re-picks
//      the same source does no teardown/reattach and queues no duplicate sync.
//   4. IDEMPOTENT — applying the same fresh resolution twice yields the same row
//      and (after the first) `rebuildSession == false`, so repeated imports never
//      duplicate candidates, sessions, or sync work.
//
// RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase E): the DECISION now lives in
// `couple_core::policy::row_merge` and is reached over the FFI (`cpl_policy_row_merge`).
// This type is a thin WRAPPER. On a core error (a malformed/stale request —
// unreachable with this in-process call) it maps to the CONSERVATIVE PROTECTED-row
// plan: keep the existing selection, candidates, and display, with no session rebuild
// — i.e. treat the merge as if the row were protected, so a failure can NEVER silently
// swap a source or tear down an approved couple's session (CONTRACT invariant 1).

import CCouple
import Foundation

/// The complete, atomic outcome of folding one fresh `ResolvedRow` into an
/// existing queue row. `AppModel` reads this and performs the mutations together
/// (selection, candidate list, display fields, session rebuild, `forced` reset),
/// so the row is never left in a partially-updated state. Pure value type.
public struct RowMergePlan: Sendable, Equatable {
    /// The selected audio content key after the merge (`nil` ⇒ ambiguous/no-match
    /// carries no selection — never a coin-flip).
    public var selectedAudioKey: String?
    /// The candidate audio keys after the merge, in final order. For an
    /// unprotected row this is the fresh resolution's order; for a protected row
    /// it is the existing order followed by any genuinely-new fresh keys.
    public var candidateKeys: [String]
    /// Whether the caller must take the fresh row's `state`/`chip`/`detail` (and,
    /// in general, the full candidate VALUES, not just the keys). False for a
    /// protected row, whose display is preserved.
    public var adoptFreshDisplay: Bool
    /// Whether the selected source changed and the caller must therefore rebuild
    /// the `PairSession` and (re)queue auto-sync exactly once. Always false for a
    /// protected row (its selection never changes here).
    public var rebuildSession: Bool
    /// Whether the caller must clear a stale `forced` flag. Only an unprotected row
    /// that adopts a fresh resolution can clear it; a protected row keeps its
    /// honest `forced — below floor` chip. (Today `forced ⟹ pinned`, so this is a
    /// defensive guarantee that the invariant cannot silently rot.)
    public var clearForced: Bool

    public init(selectedAudioKey: String?, candidateKeys: [String],
                adoptFreshDisplay: Bool, rebuildSession: Bool, clearForced: Bool) {
        self.selectedAudioKey = selectedAudioKey
        self.candidateKeys = candidateKeys
        self.adoptFreshDisplay = adoptFreshDisplay
        self.rebuildSession = rebuildSession
        self.clearForced = clearForced
    }
}

public enum RowMergePolicy {
    /// Decide how to fold a fresh resolution into an existing row (TODO-007).
    ///
    /// - Parameters:
    ///   - protected: `approved || pinned` — the SPEC-V2 §3.6 protection boundary.
    ///   - currentSelectedKey: the row's selected audio key before the merge.
    ///   - currentCandidateKeys: the row's candidate keys before the merge, in order.
    ///   - freshSelectedKey: the resolver's selected key for the re-resolved row
    ///     (`nil` for ambiguous/no-match).
    ///   - freshCandidateKeys: the resolver's full candidate keys for the
    ///     re-resolved row, in resolver order (selected first when present).
    public static func plan(protected: Bool,
                            currentSelectedKey: String?,
                            currentCandidateKeys: [String],
                            freshSelectedKey: String?,
                            freshCandidateKeys: [String]) -> RowMergePlan {
        // Build the camelCase request and round-trip through `cpl_policy_row_merge`
        // (the decision now lives in `couple_core::policy::row_merge`).
        var req: [String: Any] = [
            "protected": protected,
            "currentSelectedKey": currentSelectedKey as Any,
            "currentCandidateKeys": currentCandidateKeys,
            "freshSelectedKey": freshSelectedKey as Any,
            "freshCandidateKeys": freshCandidateKeys,
        ]
        if currentSelectedKey == nil { req["currentSelectedKey"] = NSNull() }
        if freshSelectedKey == nil { req["freshSelectedKey"] = NSNull() }
        let value: [String: Any]?
        if let data = try? JSONSerialization.data(withJSONObject: req),
           let json = String(data: data, encoding: .utf8) {
            value = PolicyEnvelope.decodeValue(cpl_policy_row_merge(json))
        } else {
            value = nil
        }
        return plan(fromEnvelopeValue: value,
                    currentSelectedKey: currentSelectedKey,
                    currentCandidateKeys: currentCandidateKeys)
    }

    /// Map a decoded envelope `value` object to the merge plan, applying the CONSERVATIVE
    /// fallback. A nil value (core returned an error envelope, or the call/parse failed) or a
    /// malformed value ⇒ a PROTECTED-row plan computed from the current state: keep the existing
    /// selection and candidates, take no fresh display, rebuild nothing. A failure can therefore
    /// NEVER silently swap a source or tear down an approved couple's session. Internal so a test
    /// can drive the error path without reaching the C ABI (PLAN §3.2).
    @inline(never)
    static func plan(fromEnvelopeValue value: [String: Any]?,
                     currentSelectedKey: String?,
                     currentCandidateKeys: [String]) -> RowMergePlan {
        if let value,
           let candidateKeys = value["candidateKeys"] as? [String],
           let adoptFreshDisplay = value["adoptFreshDisplay"] as? Bool,
           let rebuildSession = value["rebuildSession"] as? Bool,
           let clearForced = value["clearForced"] as? Bool {
            // selectedAudioKey is `str|null`; an absent/NSNull value decodes to nil (never a
            // coin-flip), matching core's `Option<String>`.
            let selectedAudioKey = value["selectedAudioKey"] as? String
            return RowMergePlan(selectedAudioKey: selectedAudioKey,
                                candidateKeys: candidateKeys,
                                adoptFreshDisplay: adoptFreshDisplay,
                                rebuildSession: rebuildSession,
                                clearForced: clearForced)
        }
        // Conservative fallback: behave as a protected row over the current state — preserve the
        // user's selection and candidate list, change no display, rebuild no session.
        return RowMergePlan(selectedAudioKey: currentSelectedKey,
                            candidateKeys: currentCandidateKeys,
                            adoptFreshDisplay: false,
                            rebuildSession: false,
                            clearForced: false)
    }
}
