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
        // Only ever remove live entries; an unknown key is dropped so the plan can
        // never name a row that no longer exists.
        let removable = selectedKeys.intersection(poolKeys)
        guard !removable.isEmpty else {
            return MediaRemovalPlan(removableKeys: [], needsConfirmation: false,
                                    affectedCouples: 0, affectedApproved: 0)
        }

        // A couple is AFFECTED iff its video is going away (row removed) or the audio
        // it currently uses is going away (row re-resolves). A removed alternate is not
        // a consequence — the selected pairing is untouched.
        var affected = 0
        var affectedApproved = 0
        for c in couples {
            let videoRemoved = removable.contains(c.videoKey)
            let audioRemoved = c.selectedAudioKey.map { removable.contains($0) } ?? false
            if videoRemoved || audioRemoved {
                affected += 1
                if c.approved { affectedApproved += 1 }
            }
        }

        // Confirmation is owed exactly when the removal has a couple-level consequence;
        // a purely-unused removal is immediate (no dialog).
        return MediaRemovalPlan(removableKeys: removable,
                                needsConfirmation: affected > 0,
                                affectedCouples: affected,
                                affectedApproved: affectedApproved)
    }

    /// The Clear Session confirmation counts (TODO-001) — media files, couples, and
    /// approved (queued-for-export) couples currently in the session.
    public static func clearCounts(mediaCount: Int, couples: [CoupleRef]) -> ClearSessionCounts {
        ClearSessionCounts(media: mediaCount,
                           couples: couples.count,
                           approved: couples.filter(\.approved).count)
    }
}
