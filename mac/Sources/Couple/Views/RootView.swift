// Root layout (§V2.1): three columns with no modal screens — everything visible
// at once, INCLUDING the empty session (revised 2026-07-11: no splash screen;
// an empty pool just renders the import surface in the center column with the
// rails dimmed-empty around it). Queue rail (left), video-hero center, export
// rail (right), with both side rails drag-resizable (HSplitView) and the center
// absorbing the slack. The whole window stays a drop target (§2.1).

import CoupleKit
import SwiftUI
import UniformTypeIdentifiers

struct RootView: View {
    @Environment(AppModel.self) private var model
    /// The window's UndoManager (Edit ▸ Undo / ⌘Z), handed to the model so queue
    /// mutations (approve, re-pair, export rename) can register their inverses.
    @Environment(\.undoManager) private var undoManager

    var body: some View {
        @Bindable var model = model
        return VStack(spacing: 0) {
            HSplitView {
                QueueRail()
                    .frame(minWidth: 248, idealWidth: 304, maxWidth: 440)
                CenterColumn()
                    .frame(minWidth: 460)
                    .layoutPriority(1)
                ExportRail()
                    .frame(minWidth: 224, idealWidth: 280, maxWidth: 400)
            }
            FooterBar()
        }
        .background(Theme.windowBg)
        .overlay(ToastOverlay(center: model.toasts))
        // Keep the model's UndoManager current (it is per-window and can be
        // replaced by the scene; `initial: true` covers first appearance).
        .onChange(of: undoManager, initial: true) { _, new in model.undoManager = new }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            Task { await ingest(providers) }
            return true
        }
        // Approved-replacement confirmation (TODO-013): a manual audio drop onto an
        // APPROVED couple (from EITHER drop target) arms this; replacing the source
        // removes the couple from the export queue and clears its current pair
        // session, so it asks first. Presented once here at the shared ancestor so
        // both gestures show the identical dialog. Confirm ⇒ replace; Cancel ⇒ no
        // change. The dialog states the consequence explicitly.
        .confirmationDialog(
            "Replace approved pairing?",
            isPresented: Binding(
                get: { model.pendingRepairConfirmation != nil },
                set: { if !$0 { model.cancelPendingRepair() } }),
            titleVisibility: .visible,
            presenting: model.pendingRepairConfirmation
        ) { pending in
            Button("Replace with \(pending.audioName)", role: .destructive) {
                model.confirmPendingRepair()
            }
            Button("Cancel", role: .cancel) { model.cancelPendingRepair() }
        } message: { pending in
            Text("\(pending.videoName) is approved and queued for export. Pairing it with \(pending.audioName) removes it from the export queue, clears its approval/export status, and rebuilds the current pair session.")
        }
        // Per-file removal confirmation (TODO-001): removing media that participates in
        // ≥1 couple regenerates those couples (a removed video's row disappears; a
        // removed audio's row re-resolves over the remaining files), so it asks ONCE for
        // the whole batch with the consequence counts. Unused media is removed without a
        // dialog (this never arms). Removal never deletes source media from disk.
        .confirmationDialog(
            "Remove selected media?",
            isPresented: Binding(
                get: { model.pendingMediaRemoval != nil },
                set: { if !$0 { model.cancelMediaRemoval() } }),
            titleVisibility: .visible,
            presenting: model.pendingMediaRemoval
        ) { pending in
            Button("Remove \(pending.keys.count) file\(pending.keys.count == 1 ? "" : "s")", role: .destructive) {
                model.confirmMediaRemoval()
            }
            Button("Cancel", role: .cancel) { model.cancelMediaRemoval() }
        } message: { pending in
            Text(removalMessage(pending))
        }
        // Clear Session confirmation (TODO-001): discards the whole queue and returns to
        // the import screen, so it always confirms with the media/couple/approved counts.
        // Source files and previously exported files are untouched.
        .confirmationDialog(
            "Clear this session?",
            isPresented: Binding(
                get: { model.pendingClearSession != nil },
                set: { if !$0 { model.cancelClearSession() } }),
            titleVisibility: .visible,
            presenting: model.pendingClearSession
        ) { _ in
            Button("Clear Session", role: .destructive) { model.confirmClearSession() }
            Button("Cancel", role: .cancel) { model.cancelClearSession() }
        } message: { pending in
            Text(clearMessage(pending))
        }
    }

    /// Consequence-focused copy for the per-file removal dialog (TODO-001): how many
    /// couples are affected and how many of those were approved/queued for export.
    private func removalMessage(_ p: AppModel.PendingMediaRemoval) -> String {
        let couples = "\(p.affectedCouples) couple\(p.affectedCouples == 1 ? "" : "s")"
        var msg = "This affects \(couples). Affected video rows re-resolve over the remaining files; a removed video's row is removed. "
        if p.affectedApproved > 0 {
            msg += "\(p.affectedApproved) approved row\(p.affectedApproved == 1 ? " is" : "s are") queued for export and will be removed from the queue. "
        }
        msg += "This removes the files from this session only — your source files are not deleted."
        return msg
    }

    /// Consequence-focused copy for the Clear Session dialog (TODO-001).
    private func clearMessage(_ p: AppModel.PendingClearSession) -> String {
        let media = "\(p.media) media file\(p.media == 1 ? "" : "s")"
        let couples = "\(p.couples) couple\(p.couples == 1 ? "" : "s")"
        let approved = "\(p.approved) approved"
        return "This discards \(media), \(couples) (\(approved)) and returns to the import screen. Your source files and any files you have already exported are not deleted."
    }

    private func ingest(_ providers: [NSItemProvider]) async {
        var urls: [URL] = []
        for p in providers {
            if let url = await withCheckedContinuation({ (c: CheckedContinuation<URL?, Never>) in
                _ = p.loadObject(ofClass: URL.self) { url, _ in c.resume(returning: url) }
            }) { urls.append(url) }
        }
        if !urls.isEmpty { model.importDropped(urls) }
    }
}

// MARK: - Footer cheat line (v2 keys)

struct FooterBar: View {
    var body: some View {
        HStack(spacing: 6) {
            Text("j/k move · v approve (=queue) · ⏎/⚙ sync tools · l/◫ loupes · esc collapse     ←/→ offset 1 ms · ⇧ 10 · ⌥ 0.1 · [ ] drift · drag loupe = offset · ⇧-drag = pivot")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
                .lineLimit(1)
            Spacer()
            Text("waveforms render real peak data")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textTertiary)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
    }
}
