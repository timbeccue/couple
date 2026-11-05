// Manual drag-to-pair decision (TODO-013) — the PURE, testable core behind both
// drop targets that re-pair a couple with an audio file: the queue-row drop (the
// original gesture) and the center-workspace drop (this item). The two gestures
// MUST produce identical pairing results, so the "what should this drop do?"
// decision is lifted here (out of the app target / out of the SwiftUI views) where
// `swift test` can lock it down — mirroring the codebase's other pure-decision
// idioms (RowMergePolicy.plan, PoolSelection.select, MatchQualityPolicy.decide,
// AuditionController.transportPlan, candidateOverflows). The app owns the side
// effects (presenting the confirmation, then `repair`, which selects+pins the
// audio, clears the stale include/export inputs, rebuilds the session, and runs
// sync); this
// type only decides the OUTCOME CATEGORY so neither call site can diverge from the
// other or invent its own replacement rules.
//
// The replacement rules (TODO-013 / SPEC-V2 §V2.6) are deliberately simple:
//   - Dropping the ALREADY-selected audio is a no-op.
//   - A couple with NO current audio (unresolved) accepts the drop immediately.
//   - An UNPINNED couple with a DIFFERENT source replaces immediately, no dialog
//     (the old audio stays in the pool and can be re-selected).
//   - A PINNED couple (a manual pick) needs confirmation, because replacing it
//     discards a deliberate choice and clears the current pair session. Since
//     PLAN-COMPACT-MODE D9 the gate is the PIN, not approval/inclusion: with
//     default-on inclusion (D1) an inclusion gate would prompt on every drop. The
//     `approved` parameter/wire key carries `pinned` until Phase 6 (D12).
//   - There is nothing to pair WITHOUT a selected couple, an AUDIO payload, and an
//     available video — any of those missing rejects the drop with no state change.
//
// CONTRACT (the testable invariants):
//   1. REJECT needs no couple, no audio, or no video — `hasSelectedCouple == false`,
//      `payloadIsAudio == false`, or `videoAvailable == false` ⇒ `.reject`, checked
//      before any replacement reasoning (a malformed/non-audio drop never mutates).
//   2. SAME AUDIO IS A NO-OP — dropping the key that is already selected ⇒ `.noOp`
//      regardless of the pin (re-dropping the current source changes nothing, so it
//      never clears the include state or rebuilds the session).
//   3. UNRESOLVED ACCEPTS IMMEDIATELY — no current selection (and a different,
//      non-nil dropped key) ⇒ `.accept` (there is no manual pick to protect).
//   4. UNPINNED + DIFFERENT ACCEPTS IMMEDIATELY — a different source on a couple
//      that is NOT pinned ⇒ `.accept`, no confirmation.
//   5. PINNED + DIFFERENT NEEDS CONFIRMATION — a different source on a PINNED
//      couple ⇒ `.needsConfirmation`; the caller confirms (→ accept) or cancels
//      (→ no change). This is the ONLY outcome that asks first.
//
// RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase E): the DECISION now lives in
// `couple_core::policy::repair_drop` and is reached over the FFI
// (`cpl_policy_repair_drop`). This type is a thin WRAPPER. On a core error (a
// malformed/stale request — unreachable with this in-process call) it maps to the
// CONSERVATIVE fallback `.reject`, which mutates NO state (CONTRACT invariant 1).

import CCouple
import Foundation

/// What a manual audio drop onto a couple should do (TODO-013). The app maps each
/// case to a side effect: `.reject`/`.noOp` do nothing; `.accept` runs the shared
/// `repair`; `.needsConfirmation` presents the pinned-replacement dialog (D9) and runs
/// `repair` only on confirm. Both drop targets consume this so they cannot diverge.
public enum RepairDropOutcome: Sendable, Equatable {
    /// Drop the already-selected audio — nothing changes.
    case noOp
    /// Replace immediately (unresolved, or an unpinned couple with a new source).
    case accept
    /// Replacing a PINNED couple (a manual pick, D9) — confirm first (replacement
    /// discards that choice, its export status and the current pair session), then
    /// replace on confirm.
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
    ///   - approved: the couple is PINNED (a manual pick). Since PLAN-COMPACT-MODE D9
    ///     this input carries `pinned`, not approval/inclusion — the shell passes
    ///     `couple.pinned`; the label and wire key keep the old name until Phase 6
    ///     (D12). Only a pinned couple's DIFFERENT-source replacement asks first.
    public static func decide(hasSelectedCouple: Bool,
                              payloadIsAudio: Bool,
                              videoAvailable: Bool,
                              currentSelectedKey: String?,
                              droppedKey: String,
                              approved: Bool) -> RepairDropOutcome {
        // Build the camelCase request and round-trip through `cpl_policy_repair_drop`
        // (the decision now lives in `couple_core::policy::repair_drop`).
        var req: [String: Any] = [
            "hasSelectedCouple": hasSelectedCouple,
            "payloadIsAudio": payloadIsAudio,
            "videoAvailable": videoAvailable,
            "currentSelectedKey": currentSelectedKey as Any,
            "droppedKey": droppedKey,
            "approved": approved,
        ]
        if currentSelectedKey == nil { req["currentSelectedKey"] = NSNull() }
        let value: [String: Any]?
        if let data = try? JSONSerialization.data(withJSONObject: req),
           let json = String(data: data, encoding: .utf8) {
            value = PolicyEnvelope.decodeValue(cpl_policy_repair_drop(json))
        } else {
            value = nil
        }
        return outcome(fromEnvelopeValue: value)
    }

    /// Map a decoded envelope `value` object to the outcome, applying the CONSERVATIVE fallback.
    /// A nil value (core returned an error envelope, or the call/parse failed) or an unknown
    /// `outcome` string ⇒ `.reject` — which mutates NO state, never a silent accept/confirm.
    /// Internal so a test can drive the error path without reaching the C ABI (PLAN §3.2).
    /// `@inline(never)` keeps a standalone symbol so the test can link it (the optimizer would
    /// otherwise inline this sole-caller helper into `decide` and emit no symbol).
    @inline(never)
    static func outcome(fromEnvelopeValue value: [String: Any]?) -> RepairDropOutcome {
        guard let outcome = (value?["outcome"] as? String).flatMap(RepairDropOutcome.init(wire:)) else {
            return .reject
        }
        return outcome
    }

    /// Whether a KEPT manual pairing is `forced` — below the correlation floor (SPEC-V2 §3.6).
    /// Decided in `couple_core::policy::repair_drop::forced` over `cpl_policy_repair_forced`;
    /// called at pick time (`syncFailed: false`; a fresh pool drag carries `candidateZ: 0`,
    /// unknown until the sync correlates) and again when the sync completes. `forced` feeds
    /// `MatchQualityPolicy` (a forced pairing is always questionable); the app owns the chip.
    public static func forced(candidateZ: Double, zMin: Double, syncFailed: Bool) -> Bool {
        let req: [String: Any] = ["candidateZ": candidateZ, "zMin": zMin, "syncFailed": syncFailed]
        let value: [String: Any]?
        if let data = try? JSONSerialization.data(withJSONObject: req),
           let json = String(data: data, encoding: .utf8) {
            value = PolicyEnvelope.decodeValue(cpl_policy_repair_forced(json))
        } else {
            value = nil
        }
        return forced(fromEnvelopeValue: value)
    }

    /// Map a decoded envelope `value` to the forced verdict. The CONSERVATIVE fallback for a nil
    /// value (error envelope / failed call — unreachable in-process) is `true`: a pairing we could
    /// not judge stays questionable, never silently green (mirrors MatchQualityPolicy's fallback).
    /// Internal so a test can drive the error path without reaching the C ABI.
    @inline(never)
    static func forced(fromEnvelopeValue value: [String: Any]?) -> Bool {
        (value?["forced"] as? Bool) ?? true
    }
}

extension RepairDropOutcome {
    /// Decode a core wire string (`couple_core::policy::repair_drop::RepairDropOutcome`) into the
    /// UI enum; nil on an unknown value.
    init?(wire: String) {
        switch wire {
        case "noOp": self = .noOp
        case "accept": self = .accept
        case "needsConfirmation": self = .needsConfirmation
        case "reject": self = .reject
        default: return nil
        }
    }
}
