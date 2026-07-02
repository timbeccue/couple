// Media-pool selection + removal (owner decision 3; TODO-001), extracted from AppModel.
// Owns the Finder-style pool selection/primary/anchor, the pool-audio drag state, and
// the per-file removal / Clear Session flows. AppModel keeps the couples list, audition,
// preview, and export settings; this drives them through the back-reference, and re-runs
// the SAME resolver path (via `model.ingest`) so removal regenerates couples exactly as a
// fresh import would. Views reach these members through AppModel's forwarding accessors.

import AppKit
import CoupleKit
import Observation

@MainActor
@Observable
final class PoolController {
    @ObservationIgnored private unowned let model: AppModel

    /// Media-pool Finder-style selection (owner decision 3; TODO-001 foundation).
    /// `poolSelection` is the multi-select highlight set (reused verbatim by the
    /// future remove-media feature); `poolPrimary` is the single last-clicked entry
    /// the sticky player follows; `poolAnchor` is the ⇧-range start. The pure
    /// transition lives in CoupleKit.PoolSelection; `selectPool` applies it and
    /// drives the preview (load-paused on primary change, stop when nil).
    var poolSelection: Set<String> = []
    var poolPrimary: String?
    @ObservationIgnored private var poolAnchor: String?
    /// The primary entry as a pool Entry (sticky bar reads name/url/duration off it).
    var poolPrimaryEntry: MediaPool.Entry? { poolPrimary.flatMap { model.pool.entry(key: $0) } }

    /// The pool AUDIO key currently being dragged (TODO-013), recorded when a pool
    /// audio row starts a drag and cleared when it ends. Lets the center-workspace
    /// drop overlay name the dragged file in its copy (`Pair selected video with
    /// audio.wav`) DURING the hover — SwiftUI's `.dropDestination` exposes the payload
    /// only in its action closure, not while targeting, so this is the only honest
    /// source for the live name. Purely cosmetic: the actual repair always reads the
    /// dropped payload, never this, so a stale value can never mis-pair.
    var poolDraggingKey: String?
    /// The dragged audio's display name for the live overlay copy, or nil when no
    /// pool audio drag is in flight.
    var poolDraggingName: String? { poolDraggingKey.flatMap { model.pool.entry(key: $0)?.name } }

    init(model: AppModel) {
        self.model = model
    }

    // MARK: - Media-pool selection (owner decision 3; TODO-001 foundation)

    /// Finder-style pool click (owner decision 3). Maps the live `NSEvent`
    /// modifiers to the pure `PoolSelection` transition (plain replace / ⌘ toggle /
    /// ⇧ contiguous range over the displayed pool order), applies the resulting
    /// (selection, primary, anchor), then follows the SINGLE primary: a changed
    /// primary loads it into the sticky bar PAUSED (owner decision 1 — SELECT =
    /// load-paused; video entries feed `entry.url` so the bar plays the movie's
    /// embedded audio, decision 2); a nil primary stops the preview. Space is left
    /// to the global audition transport (decision 4) — no pool key binding here.
    func selectPool(key: String, modifiers: NSEvent.ModifierFlags) {
        let modifier: PoolSelectModifier
        if modifiers.contains(.shift) { modifier = .shift }
        else if modifiers.contains(.command) { modifier = .command }
        else { modifier = .plain }

        let result = PoolSelection.select(
            current: poolSelection, primary: poolPrimary, anchor: poolAnchor,
            clicked: key, ordered: model.pool.entries.map(\.id), modifier: modifier)

        let primaryChanged = result.primary != poolPrimary
        poolSelection = result.selection
        poolPrimary = result.primary
        poolAnchor = result.anchor
        guard primaryChanged else { return }
        followPrimary()
    }

    /// Drive the sticky preview to the current primary: load-paused (decision 1) for
    /// a real entry, stop when there is none. The single place primary→player flows,
    /// so multi-select never splits the preview.
    private func followPrimary() {
        guard let entry = poolPrimaryEntry else {
            model.preview.stop()
            return
        }
        model.preview.select(key: entry.id, url: entry.url)
    }

    /// Drop selection/primary/anchor that no longer name a live pool entry (files
    /// only ever get added today, but the future remove-media feature relies on this
    /// pruning, and a restore could in principle reshuffle keys). Re-homes the
    /// player if the primary vanished.
    func prunePoolSelection() {
        let live = Set(model.pool.entries.map(\.id))
        let prunedSelection = poolSelection.intersection(live)
        let prunedPrimary = poolPrimary.flatMap { live.contains($0) ? $0 : nil }
            ?? model.pool.entries.first { prunedSelection.contains($0.id) }?.id
        let prunedAnchor = poolAnchor.flatMap { live.contains($0) ? $0 : nil } ?? prunedPrimary
        let primaryChanged = prunedPrimary != poolPrimary
        poolSelection = prunedSelection
        poolPrimary = prunedPrimary
        poolAnchor = prunedAnchor
        if primaryChanged { followPrimary() }
    }

    // MARK: - Remove media / clear session (TODO-001)

    /// A pending per-file removal confirmation (TODO-001). Set when the selected media
    /// to remove participates in ≥1 couple — removal then regenerates those couples
    /// (a removed video's row disappears; a removed audio's row re-resolves), so the
    /// batch asks ONCE first with the affected counts. `nil` ⇒ no dialog. The pure
    /// decision is `MediaRemovalPolicy.plan`; this carries the confirmed keys + counts.
    struct PendingMediaRemoval: Identifiable {
        let id = UUID()
        let keys: Set<String>
        let affectedCouples: Int
        let affectedApproved: Int
    }
    var pendingMediaRemoval: PendingMediaRemoval?

    /// A pending Clear Session confirmation (TODO-001), carrying the counts the dialog
    /// states. `nil` ⇒ no dialog. Clearing always confirms (it discards the whole queue).
    struct PendingClearSession: Identifiable {
        let id = UUID()
        let media: Int
        let couples: Int
        let approved: Int
    }
    var pendingClearSession: PendingClearSession?

    /// The couples projected for the pure removal/clear policies (CoupleKit can't see
    /// the app `CoupleRow`).
    private var coupleRefs: [CoupleRef] {
        model.couples.map { CoupleRef(videoKey: $0.id, selectedAudioKey: $0.selectedAudioKey,
                                      approved: $0.approved) }
    }

    /// Remove the currently selected pool media from the open session (TODO-001), the
    /// SHARED entry point for both the media-pool trash button and the Delete key. The
    /// pure `MediaRemovalPolicy.plan` decides over the live pool + couples: an empty
    /// plan no-ops; an UNUSED-only removal happens IMMEDIATELY (no dialog); a removal
    /// touching ≥1 referenced file arms ONE confirmation stating the affected couple /
    /// approved counts, and removes only on confirm. Removal affects ONLY this session
    /// and NEVER deletes source media from disk.
    func removeSelectedMedia() {
        let plan = MediaRemovalPolicy.plan(selectedKeys: poolSelection,
                                           poolKeys: Set(model.pool.entries.map(\.id)),
                                           couples: coupleRefs)
        guard !plan.isEmpty else { return }
        if plan.needsConfirmation {
            pendingMediaRemoval = PendingMediaRemoval(
                keys: plan.removableKeys, affectedCouples: plan.affectedCouples,
                affectedApproved: plan.affectedApproved)
        } else {
            performMediaRemoval(plan.removableKeys)
        }
    }

    /// Confirm the pending per-file removal (TODO-001): remove the keys decided when the
    /// dialog was armed.
    func confirmMediaRemoval() {
        guard let pending = pendingMediaRemoval else { return }
        pendingMediaRemoval = nil
        performMediaRemoval(pending.keys)
    }

    /// Cancel the pending per-file removal (TODO-001): make no change.
    func cancelMediaRemoval() { pendingMediaRemoval = nil }

    /// Apply a removal of `keys` from the open session (TODO-001). Strategy: removal ≡
    /// "imported without these files". We (1) stop every bit of work that touches a
    /// removed key cleanly, (2) drop removed couples + reset rows whose SELECTED audio
    /// vanished to a clean unresolved state, (3) free the pool handles, then (4) re-run
    /// the SAME resolver path the importer uses, so affected couples are regenerated
    /// exactly as a fresh import would produce them (we never hand-edit individual
    /// couples). Finally selection/persistence are pruned so no content key survives.
    private func performMediaRemoval(_ keys: Set<String>) {
        guard !keys.isEmpty else { return }

        // (1) Stop work touching the removed keys BEFORE any handle is freed.
        // The pool-preview primary: stop the player if it follows a removed entry
        // (the §TODO-002 seam — `controller.stop()` before freeing a pool handle).
        if let active = model.preview.state.activeKey, keys.contains(active) { model.preview.stop() }
        // Any in-flight import touching these would re-add them; an import that has not
        // yet published an entry can't be named here, but a resolve over the old pool
        // must not race the removal — `resolveQueue` below runs over the NEW pool.

        // Tear down each couple whose video OR selected audio is leaving, so its core
        // session (and the shared audition stream, if it is the selected couple) is
        // closed before the underlying tracks are freed. A removed-video couple is
        // dropped entirely; a removed-audio couple is reset to a clean unresolved row
        // (stale candidate data, selection, pin/forced, approval, provenance, and pair
        // session all discarded) so the re-resolve treats it as never-paired.
        var survivingCouples: [CoupleRow] = []
        for couple in model.couples {
            if keys.contains(couple.id) {
                // Removing a VIDEO removes its couple row.
                if couple.id == model.selectedID { model.audition.teardown() }
                couple.pair.teardown()
                continue
            }
            let audioRemoved = couple.selectedAudioKey.map { keys.contains($0) } ?? false
            if audioRemoved {
                if couple.id == model.selectedID { model.audition.teardown() }
                couple.pair.teardown()
                couple.selectedAudioKey = nil
                couple.pinned = false
                couple.forced = false
                couple.selectionProvenance = .none
                couple.manualMatchConfident = nil
                // Drop EVERY candidate that names a removed key; the resolver will
                // repopulate the survivors. (Stale candidates can't linger on the row.)
            }
            // Drop any candidate (selected or alternate) that names a removed key, so a
            // removed content key never survives on a couple even before the re-resolve.
            couple.candidates.removeAll { keys.contains($0.audioKey) }
            survivingCouples.append(couple)
        }
        model.couples = survivingCouples
        // If the selected couple was removed, drop the selection (re-pick after resolve).
        if let sel = model.selectedID, keys.contains(sel) { model.selectedID = nil }

        // (3) Free the pool handles for the removed entries (releases the cache-GC pin;
        // not a cache purge). Source files on disk are untouched.
        model.pool.remove(keys: keys)

        // Drop removed keys from the pool selection/primary/anchor + re-home the player.
        prunePoolSelection()

        // (4) Re-resolve over the REMAINING media via the normal resolver path, so every
        // affected couple is regenerated exactly as if the survivors had been imported
        // without the removed file. `applyResolution` rebuilds `couples` (a removed
        // video has no row; a reset audio row re-resolves) and re-prunes selection.
        if model.pool.isEmpty {
            // Nothing left to resolve: empty the queue and wipe the restore record so the
            // emptied session is not offered for restore (same end state as Clear Session
            // reached by removing the last file).
            model.couples = []
            model.selectedID = nil
            QueueStore.clear()
        } else {
            Task { await model.ingest.resolveQueue(); model.saveQueue() }
        }
    }

    /// Request Clear Session (TODO-001): always confirms, stating the media / couple /
    /// approved counts. The button is only meaningful with content; an empty session
    /// no-ops. Blocked while a batch export is running (it owns the couples' files).
    func requestClearSession() {
        guard model.hasContent, !model.batch.isRunning else { return }
        let counts = MediaRemovalPolicy.clearCounts(mediaCount: model.pool.entries.count,
                                                    couples: coupleRefs)
        pendingClearSession = PendingClearSession(media: counts.media, couples: counts.couples,
                                                  approved: counts.approved)
    }

    /// Cancel the pending Clear Session (TODO-001): make no change.
    func cancelClearSession() { pendingClearSession = nil }

    /// Clear the whole session (TODO-001) and return to the initial import screen. Stops
    /// active work, releases every pooled track, clears queue/export state, REMOVES the
    /// persisted restore record, and resets to the empty state (`hasContent == false` ⇒
    /// RootView shows the LoadSurface). Source files and previously exported files are
    /// untouched; the cleared state does NOT reappear on relaunch (the restore record is
    /// gone and the empty state is never re-persisted by the save guard).
    func confirmClearSession() {
        pendingClearSession = nil
        // Stop active work: pool preview, the selected couple's audition stream, and
        // every couple's core session + sync.
        model.preview.stop()
        model.audition.teardown()
        for couple in model.couples { couple.pair.teardown() }
        // Clear queue + selection + export state.
        model.couples = []
        model.selectedID = nil
        model.resetExportSettings()
        // Release every pooled track (frees handles; not a cache purge).
        model.pool.teardown()
        model.pool.skipped = []
        // Clear the pool selection entirely (no key survives).
        poolSelection = []
        poolPrimary = nil
        poolAnchor = nil
        poolDraggingKey = nil
        // Remove the persisted restore record so the cleared queue is not offered again.
        QueueStore.clear()
        // Clear the activity strip so a stuck/in-flight bar from the discarded session does
        // not linger into the empty import screen (TODO-001).
        model.activity.reset()
    }
}
