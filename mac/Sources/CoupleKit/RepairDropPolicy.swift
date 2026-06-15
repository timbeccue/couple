// Manual drag-to-pair decision (TODO-013) — the PURE, testable core behind both
// drop targets that re-pair a couple with an audio file: the queue-row drop (the
// original gesture) and the center-workspace drop (this item). The two gestures
// MUST produce identical pairing results, so the "what should this drop do?"
// decision is lifted here (out of the app target / out of the SwiftUI views) where
// `swift test` can lock it down — mirroring the codebase's other pure-decision
// idioms (RowMergePolicy.plan, PoolSelection.select, MatchQualityPolicy.decide,
// AuditionController.transportPlan, candidateOverflows). The app owns the side
// effects (presenting the confirmation, then `repair`, which selects+pins the
// audio, clears stale export/approval, rebuilds the session, and runs sync); this
// type only decides the OUTCOME CATEGORY so neither call site can diverge from the
// other or invent its own replacement rules.
//
// The replacement rules (TODO-013 / SPEC-V2 §V2.6) are deliberately simple:
//   - Dropping the ALREADY-selected audio is a no-op.
//   - A couple with NO current audio (unresolved) accepts the drop immediately.
//   - An UNAPPROVED couple with a DIFFERENT source replaces immediately, no dialog
//     (the old audio stays in the pool and can be re-selected).
//   - An APPROVED couple needs confirmation, because replacing it removes it from
//     the export queue and clears the current pair session.
//   - There is nothing to pair WITHOUT a selected couple, an AUDIO payload, and an
//     available video — any of those missing rejects the drop with no state change.
//
// CONTRACT (the testable invariants):
//   1. REJECT needs no couple, no audio, or no video — `hasSelectedCouple == false`,
//      `payloadIsAudio == false`, or `videoAvailable == false` ⇒ `.reject`, checked
//      before any replacement reasoning (a malformed/non-audio drop never mutates).
//   2. SAME AUDIO IS A NO-OP — dropping the key that is already selected ⇒ `.noOp`
//      regardless of approval (re-dropping the current source changes nothing, so it
//      never clears approval or rebuilds the session).
//   3. UNRESOLVED ACCEPTS IMMEDIATELY — no current selection (and a different,
//      non-nil dropped key) ⇒ `.accept` (there is no approved export to protect).
//   4. UNAPPROVED + DIFFERENT ACCEPTS IMMEDIATELY — a different source on a couple
//      that is NOT approved ⇒ `.accept`, no confirmation.
//   5. APPROVED + DIFFERENT NEEDS CONFIRMATION — a different source on an APPROVED
//      couple ⇒ `.needsConfirmation`; the caller confirms (→ accept) or cancels
//      (→ no change). This is the ONLY outcome that asks first.

import Foundation

/// What a manual audio drop onto a couple should do (TODO-013). The app maps each
/// case to a side effect: `.reject`/`.noOp` do nothing; `.accept` runs the shared
/// `repair`; `.needsConfirmation` presents the approved-replacement dialog and runs
/// `repair` only on confirm. Both drop targets consume this so they cannot diverge.
public enum RepairDropOutcome: Sendable, Equatable {
    /// Drop the already-selected audio — nothing changes.
    case noOp
    /// Replace immediately (unresolved, or an unapproved couple with a new source).
    case accept
    /// Replacing an APPROVED couple — confirm first (replacement clears its
    /// approval/export status and the current pair session), then replace on confirm.
    case needsConfirmation
    /// No selected couple, a non-audio payload, or an unavailable video — ignore the
    /// drop without changing any state.
    case reject
}

public enum RepairDropPolicy {
    /// Decide the outcome of dropping an audio file onto the selected couple's drop
    /// target (TODO-013). Used by BOTH the queue-row and center-workspace gestures so
    /// they produce identical pairing results.
    ///
    /// - Parameters:
    ///   - hasSelectedCouple: there is a couple under/selected for this drop target.
    ///     A center-workspace drop with no selected couple, or a queue-row drop with a
    ///     nil row, is `.reject`.
    ///   - payloadIsAudio: the dragged content key names an AUDIO pool entry (the app
    ///     resolves this via `pool.audios`/`entry(key:).role`). A non-audio payload —
    ///     a video key, the empty sentinel a non-audio `.draggable` carries, or junk —
    ///     is `.reject`.
    ///   - videoAvailable: the couple's video can actually be paired (its track handle
    ///     exists). A stripped/unavailable video is `.reject` (there is nothing to
    ///     pair the audio WITH).
    ///   - currentSelectedKey: the couple's currently selected audio key, or `nil`
    ///     when unresolved (no selection yet).
    ///   - droppedKey: the dragged audio's content key.
    ///   - approved: the couple is approved (≡ queued for export). Only an approved
    ///     couple's DIFFERENT-source replacement asks for confirmation.
    public static func decide(hasSelectedCouple: Bool,
                              payloadIsAudio: Bool,
                              videoAvailable: Bool,
                              currentSelectedKey: String?,
                              droppedKey: String,
                              approved: Bool) -> RepairDropOutcome {
        // (1) Nothing to pair: no couple, a non-audio payload, or no usable video.
        //     Checked first so a malformed/non-audio drop can never reach the
        //     replacement reasoning and never mutates state.
        guard hasSelectedCouple, payloadIsAudio, videoAvailable else { return .reject }

        // (2) Re-dropping the already-selected source changes nothing (independent of
        //     approval — a no-op must never clear approval or rebuild the session).
        if droppedKey == currentSelectedKey { return .noOp }

        // (3)/(4) A new source on an unresolved or unapproved couple replaces
        //         immediately — there is no approved export to protect.
        guard approved else { return .accept }

        // (5) A new source on an APPROVED couple removes it from the export queue, so
        //     confirm first; the caller replaces only on confirm.
        return .needsConfirmation
    }
}
