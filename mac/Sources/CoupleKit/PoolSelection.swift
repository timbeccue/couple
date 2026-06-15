// Media-pool selection (TODO-001 foundation) — the PURE, testable core behind
// `AppModel.selectPool`. The media pool is a Finder-style multi-select list: a
// plain click replaces the selection, ⌘-click toggles one entry, and ⇧-click
// selects the contiguous range over the DISPLAYED pool order. Independently of how
// many entries are selected, the sticky player follows a single PRIMARY — the
// last-clicked entry — so a multi-select highlight never splits the preview.
//
// That decision is expressible over content keys + the ordered key list + a
// modifier alone, so it is lifted here (out of the app target) where `swift test`
// can lock it down, mirroring the codebase's other pure-decision idioms
// (RowMergePolicy.plan, PoolPreviewAction, AuditionController.transportPlan,
// candidateOverflows). AppModel owns the side effects — drive the preview to the
// new primary (load-paused) or stop it when the primary becomes nil; this type
// only decides WHAT the new (selection, primary, anchor) should be.
//
// This is deliberately built so the future "remove media" feature (TODO-001) can
// reuse the selection SET verbatim (remove every selected key); removal is NOT
// implemented here. It is also ready for future arrow-key navigation (the anchor +
// ordered keys are all a range-extend needs) without adding that behavior in v1.
//
// CONTRACT (the testable invariants):
//   1. PLAIN replaces the whole selection with the clicked key, which becomes the
//      primary AND the anchor (the start of any subsequent ⇧-range).
//   2. COMMAND toggles the clicked key's membership without disturbing the rest.
//      Adding makes it the primary + anchor. Removing the current primary moves the
//      primary to another still-selected member (the one nearest in display order),
//      or nil when the set empties; the anchor follows the new primary.
//   3. SHIFT selects the inclusive contiguous range in DISPLAYED order from the
//      anchor to the clicked key (anchor preserved so the range can be re-dragged);
//      the clicked key becomes the primary. With no prior anchor, ⇧ behaves like a
//      plain click (anchor falls back to the clicked key).
//   4. A click on a key NOT in the ordered list is inert (returns the input
//      unchanged) — selection can only ever name live pool entries.

import Foundation

/// Which mouse modifier drove the click (Finder semantics). Mapped from
/// `NSEvent.modifierFlags` by the view; kept framework-free so the transition is
/// unit-testable with no AppKit.
public enum PoolSelectModifier: Sendable, Equatable {
    /// No modifier — replace the selection with the clicked entry.
    case plain
    /// ⌘ — toggle the clicked entry's membership.
    case command
    /// ⇧ — extend a contiguous range from the anchor to the clicked entry.
    case shift
}

/// The complete, atomic outcome of one pool click. `AppModel` reads this and
/// applies it together (selection set, primary→preview, anchor), so the selection
/// is never left half-updated. Pure value type, reusable by TODO-001 removal.
public struct PoolSelectionResult: Sendable, Equatable {
    /// The selected content keys after the click (Finder multi-select set).
    public var selection: Set<String>
    /// The single entry the sticky player follows — the last-clicked entry, or nil
    /// when the selection emptied (⇒ the caller stops the preview).
    public var primary: String?
    /// The range anchor for a subsequent ⇧-click (the last plain/⌘ click, or the
    /// new primary after a ⌘-remove). nil only when the selection is empty.
    public var anchor: String?

    public init(selection: Set<String>, primary: String?, anchor: String?) {
        self.selection = selection
        self.primary = primary
        self.anchor = anchor
    }
}

public enum PoolSelection {
    /// Decide the new (selection, primary, anchor) for a pool click (TODO-001).
    ///
    /// - Parameters:
    ///   - current: the selected keys before the click.
    ///   - primary: the entry the player currently follows (preserved verbatim if
    ///     the click turns out to be inert), or nil.
    ///   - anchor: the current range anchor (last plain/⌘ click), or nil.
    ///   - clicked: the content key that was clicked.
    ///   - ordered: the DISPLAYED pool order (import order) — the universe ⇧-range
    ///     walks and the membership a primary-reassignment is chosen from. A click
    ///     on a key absent from this list is inert.
    ///   - modifier: plain / command / shift (mapped from `NSEvent.modifierFlags`).
    public static func select(current: Set<String>,
                              primary: String?,
                              anchor: String?,
                              clicked: String,
                              ordered: [String],
                              modifier: PoolSelectModifier) -> PoolSelectionResult {
        // A click can only ever name a live pool entry; an unknown key is a true
        // no-op so a stale row (mid-removal) can never poison the selection.
        guard ordered.contains(clicked) else {
            return PoolSelectionResult(selection: current, primary: primary, anchor: anchor)
        }

        switch modifier {
        case .plain:
            // Replace everything; the clicked entry is selection, primary, and the
            // start of any subsequent range.
            return PoolSelectionResult(selection: [clicked], primary: clicked, anchor: clicked)

        case .command:
            var next = current
            if next.contains(clicked) {
                // Toggle OFF.
                next.remove(clicked)
                if next.isEmpty {
                    return PoolSelectionResult(selection: [], primary: nil, anchor: nil)
                }
                // Removing a NON-primary leaves the primary (and its anchor) intact —
                // the player keeps following the loaded entry. Only when the removed
                // key WAS the primary do we re-home it to the nearest remaining member
                // in display order; the anchor follows the new primary.
                if clicked == primary {
                    let newPrimary = nearestSelected(to: clicked, in: ordered, selection: next)
                        ?? firstSelected(in: ordered, selection: next)
                    return PoolSelectionResult(selection: next, primary: newPrimary, anchor: newPrimary)
                }
                let keptAnchor = anchor.flatMap { next.contains($0) ? $0 : nil } ?? primary
                return PoolSelectionResult(selection: next, primary: primary, anchor: keptAnchor)
            }
            // Toggle ON: add, and the freshly-added entry leads (primary + anchor).
            next.insert(clicked)
            return PoolSelectionResult(selection: next, primary: clicked, anchor: clicked)

        case .shift:
            // Range from the anchor to the clicked key over the displayed order. The
            // anchor is preserved so the user can re-extend; the clicked end leads.
            // With no usable anchor, ⇧ degrades to a plain click.
            guard let a = anchor, let ai = ordered.firstIndex(of: a),
                  let ci = ordered.firstIndex(of: clicked) else {
                return PoolSelectionResult(selection: [clicked], primary: clicked, anchor: clicked)
            }
            let lo = min(ai, ci), hi = max(ai, ci)
            let range = Set(ordered[lo...hi])
            return PoolSelectionResult(selection: range, primary: clicked, anchor: a)
        }
    }

    /// The selected key nearest to `key` in display order (prefers the closer
    /// neighbor; ties — equal distance on both sides — resolve to the earlier one),
    /// used to re-home the primary when a ⌘-click removes the current primary.
    private static func nearestSelected(to key: String, in ordered: [String],
                                        selection: Set<String>) -> String? {
        guard let idx = ordered.firstIndex(of: key) else { return nil }
        var best: String?
        var bestDist = Int.max
        for (i, k) in ordered.enumerated() where selection.contains(k) {
            let d = abs(i - idx)
            if d < bestDist { bestDist = d; best = k }
        }
        return best
    }

    /// The first selected key in display order (a stable fallback primary).
    private static func firstSelected(in ordered: [String], selection: Set<String>) -> String? {
        ordered.first { selection.contains($0) }
    }
}
