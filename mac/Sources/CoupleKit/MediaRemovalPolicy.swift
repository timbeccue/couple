// Media removal / clear-session decisions (TODO-001) — the PURE, testable core
// behind `AppModel.removeSelectedMedia` and `AppModel.clearSession`. Removing media
// from the open session and clearing the whole session are both expressible over
// content keys + which keys each couple references + which couples are approved, so
// the decision is lifted here (out of the app target / out of the SwiftUI views)
// where `swift test` can lock it down — mirroring the codebase's other pure-decision
// idioms (RowMergePolicy.plan, PoolSelection.select, MatchQualityPolicy.decide,
// RepairDropPolicy.decide, candidateOverflows). AppModel owns the side effects
// (stopping preview/audition/sync, freeing handles, re-running the resolver, wiping
// the persisted restore record); this type only decides WHETHER a confirmation is
// owed and WITH WHAT COUNTS, so the two removal entry points cannot diverge.
//
// The product rules (TODO-001 / SPEC-V2 §V2.7) are:
//   - UNUSED media (referenced by no couple) is removed IMMEDIATELY, no dialog.
//   - If ANY selected key participates in ≥1 couple, ONE consequence-focused
//     confirmation is shown for the WHOLE batch, stating the number of affected
//     couples and approved rows (never one dialog per file).
//   - A couple is AFFECTED by a removal when its video is removed (the row goes
//     away) OR its currently selected audio is removed (the row re-resolves).
//     A couple that merely lists a removed key as a non-selected ALTERNATE is NOT
//     counted as affected — its selected pairing is untouched (the resolver simply
//     stops offering the vanished alternate).
//   - Clearing the session always confirms (it discards the whole queue), with the
//     media/couple/approved counts so the consequence is explicit.
//
// CONTRACT (the testable invariants):
//   1. EMPTY removal (no keys, or only keys absent from the pool) needs no
//      confirmation and reports zero affected/approved (a no-op the caller skips).
//   2. UNUSED-ONLY removal needs no confirmation (`needsConfirmation == false`) and
//      reports zero affected couples — it is removed immediately.
//   3. A removal touching ≥1 referenced key needs confirmation; `affectedCouples`
//      counts the DISTINCT couples whose video or SELECTED audio is removed, and
//      `affectedApproved` counts how many of those were approved.
//   4. The counts are a function of the input sets ALONE (no call-site flavor), so
//      a Delete-key removal and a trash-button removal of the same selection decide
//      identically.
//
// RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase E): the DECISIONS now live in
// `couple_core::policy::media_removal` and are reached over the FFI
// (`cpl_policy_media_removal`, a single tagged endpoint dispatching `plan`/`clearCounts`).
// These functions are thin WRAPPERS. On a core error (a malformed/stale request —
// unreachable with this in-process call) `plan` maps to the CONSERVATIVE EMPTY plan
// (`needsConfirmation == false`, zero counts, no removable keys) so a failure removes
// NOTHING; `clearCounts` maps to all-zero counts. A failure can never silently delete
// media or hide an approved-couple consequence behind a wrong count.

import CCouple
import Foundation

/// The identity of one couple as removal needs to reason about it: its video key,
/// the audio key it currently uses (nil when unresolved), and whether it is approved
/// (≡ queued for export). Pure value so the plan is unit-testable with no app types.
public struct CoupleRef: Sendable, Equatable {
    public var videoKey: String
    public var selectedAudioKey: String?
    public var approved: Bool

    public init(videoKey: String, selectedAudioKey: String?, approved: Bool) {
        self.videoKey = videoKey
        self.selectedAudioKey = selectedAudioKey
        self.approved = approved
    }
}

/// The outcome of a per-file removal request (TODO-001). The app reads this and
/// applies it: when `removableKeys` is empty it does nothing; when `needsConfirmation`
/// is false it removes immediately; otherwise it shows ONE dialog using the counts and
/// removes only on confirm. The same struct drives both removal gestures so they
/// cannot diverge.
public struct MediaRemovalPlan: Sendable, Equatable {
    /// The selected keys that actually name a live pool entry (junk/stale keys are
    /// dropped). Empty ⇒ the caller no-ops.
    public var removableKeys: Set<String>
    /// True iff at least one removable key participates in a couple (as a video or a
    /// SELECTED audio) ⇒ one consequence-focused confirmation is owed before removal.
    public var needsConfirmation: Bool
    /// Distinct couples whose video is removed OR whose selected audio is removed.
    public var affectedCouples: Int
    /// How many of the affected couples were approved (queued for export).
    public var affectedApproved: Int

    public init(removableKeys: Set<String>, needsConfirmation: Bool,
                affectedCouples: Int, affectedApproved: Int) {
        self.removableKeys = removableKeys
        self.needsConfirmation = needsConfirmation
        self.affectedCouples = affectedCouples
        self.affectedApproved = affectedApproved
    }

    /// Nothing live was selected — the caller skips the whole operation.
    public var isEmpty: Bool { removableKeys.isEmpty }
}

/// Counts for the Clear Session confirmation (TODO-001): a pure projection so the
/// dialog copy is testable without the app.
public struct ClearSessionCounts: Sendable, Equatable {
    public var media: Int
    public var couples: Int
    public var approved: Int

    public init(media: Int, couples: Int, approved: Int) {
        self.media = media
        self.couples = couples
        self.approved = approved
    }
}

public enum MediaRemovalPolicy {
    /// Decide the per-file removal plan (TODO-001) for a selection over the live pool.
    ///
    /// - Parameters:
    ///   - selectedKeys: the content keys the user asked to remove (the pool
    ///     selection set). Keys not in `poolKeys` are dropped (a stale row mid-removal
    ///     can never poison the plan).
    ///   - poolKeys: every live pool content key (the universe a removal can name).
    ///   - couples: the current couples, each as a `CoupleRef`. A couple is affected
    ///     when its `videoKey` is removed OR its `selectedAudioKey` is removed; a
    ///     removed key that is only a non-selected alternate does NOT count.
    public static func plan(selectedKeys: Set<String>,
                            poolKeys: Set<String>,
                            couples: [CoupleRef]) -> MediaRemovalPlan {
        // Build the tagged `plan` request and round-trip through `cpl_policy_media_removal`
        // (the decision now lives in `couple_core::policy::media_removal`).
        let req: [String: Any] = [
            "op": "plan",
            "selectedKeys": Array(selectedKeys),
            "poolKeys": Array(poolKeys),
            "couples": couples.map(\.wireDict),
        ]
        return planFromEnvelope(callPolicy(req))
    }

    /// Map a decoded envelope `value` object to the removal plan, applying the CONSERVATIVE
    /// fallback. A nil value (core returned an error envelope, or the call/parse failed) or a
    /// malformed value ⇒ the EMPTY plan: no removable keys, `needsConfirmation == false`, zero
    /// counts — so a failure removes NOTHING and hides no consequence. Internal so a test can
    /// drive the error path without reaching the C ABI (PLAN §3.2).
    @inline(never)
    static func planFromEnvelope(_ value: [String: Any]?) -> MediaRemovalPlan {
        guard let value,
              let removable = value["removableKeys"] as? [String],
              let needsConfirmation = value["needsConfirmation"] as? Bool,
              let affectedCouples = (value["affectedCouples"] as? NSNumber)?.intValue,
              let affectedApproved = (value["affectedApproved"] as? NSNumber)?.intValue
        else {
            return MediaRemovalPlan(removableKeys: [], needsConfirmation: false,
                                    affectedCouples: 0, affectedApproved: 0)
        }
        return MediaRemovalPlan(removableKeys: Set(removable),
                                needsConfirmation: needsConfirmation,
                                affectedCouples: affectedCouples,
                                affectedApproved: affectedApproved)
    }

    /// The Clear Session confirmation counts (TODO-001) — media files, couples, and
    /// approved (queued-for-export) couples currently in the session.
    public static func clearCounts(mediaCount: Int, couples: [CoupleRef]) -> ClearSessionCounts {
        let req: [String: Any] = [
            "op": "clearCounts",
            "mediaCount": mediaCount,
            "couples": couples.map(\.wireDict),
        ]
        return clearCountsFromEnvelope(callPolicy(req))
    }

    /// Map a decoded envelope `value` object to the clear-session counts. A nil/malformed value ⇒
    /// all-zero counts (a failure shows no misleading consequence). Internal test seam (PLAN §3.2).
    @inline(never)
    static func clearCountsFromEnvelope(_ value: [String: Any]?) -> ClearSessionCounts {
        guard let value,
              let media = (value["media"] as? NSNumber)?.intValue,
              let couples = (value["couples"] as? NSNumber)?.intValue,
              let approved = (value["approved"] as? NSNumber)?.intValue
        else {
            return ClearSessionCounts(media: 0, couples: 0, approved: 0)
        }
        return ClearSessionCounts(media: media, couples: couples, approved: approved)
    }

    /// Encode a request dict, call `cpl_policy_media_removal`, and return the envelope `value`
    /// object (nil on any encode/call/parse failure or an error envelope — the caller's
    /// from-envelope mapper applies the conservative fallback).
    private static func callPolicy(_ req: [String: Any]) -> [String: Any]? {
        guard let data = try? JSONSerialization.data(withJSONObject: req),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return PolicyEnvelope.decodeValue(cpl_policy_media_removal(json))
    }
}

extension CoupleRef {
    /// The core `CoupleRef` wire dict (`{"videoKey","selectedAudioKey":str|null,"approved"}`).
    var wireDict: [String: Any] {
        ["videoKey": videoKey,
         "selectedAudioKey": selectedAudioKey as Any? ?? NSNull(),
         "approved": approved]
    }
}
