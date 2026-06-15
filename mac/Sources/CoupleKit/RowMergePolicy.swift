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
        if protected {
            // §3.6 stability: keep the user's selection and display untouched;
            // append only candidate keys not already present, preserving order so
            // a re-resolve cannot reshuffle the picker. Idempotent: a fresh set
            // already fully present appends nothing.
            var merged = currentCandidateKeys
            let known = Set(currentCandidateKeys)
            for k in freshCandidateKeys where !known.contains(k) {
                merged.append(k)
            }
            return RowMergePlan(selectedAudioKey: currentSelectedKey,
                                candidateKeys: merged,
                                adoptFreshDisplay: false,
                                rebuildSession: false,
                                clearForced: false)
        }
        // Unprotected: adopt the fresh resolution wholesale. The session is rebuilt
        // (and sync re-queued) only when the effective selection actually changes,
        // so re-picking the same source — or re-applying an identical resolution —
        // does no work and creates no duplicates.
        return RowMergePlan(selectedAudioKey: freshSelectedKey,
                            candidateKeys: freshCandidateKeys,
                            adoptFreshDisplay: true,
                            rebuildSession: currentSelectedKey != freshSelectedKey,
                            clearForced: true)
    }
}
