// Root layout (§V2.1 / §V2.11): the window's two arrangements — COMPACT
// (CompactLayout: session line, fixed preview, couple list, export panel) or FULL
// (below). Full: three columns with no modal screens — everything visible
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
        // One window, two arrangements of the same session (UX §V2.11). The switch
        // is animated (AppModel.animateLayoutSwitch, WindowLayout.swift): the
        // incoming arrangement is swapped in at once, pinned at its final size
        // (top-left) while the window frame moves, and its four blocks FLIP from
        // their old frames to their new ones; Reduce Motion ⇒ instant.
        let pinned = model.layoutAnimating ? model.layoutTransitionSize : nil
        return ZStack(alignment: .topLeading) {
            switch model.layoutMode {
            case .compact:
                CompactLayout()
            case .full:
                VStack(spacing: 0) {
                    HSplitView {
                        QueueRail()
                            .frame(minWidth: 248, idealWidth: 304, maxWidth: 440)
                        CenterColumn()
                            .frame(minWidth: 460)
                            .layoutPriority(1)
                        ExportRail()
                            .frame(minWidth: 224, idealWidth: 280, maxWidth: 400)
                    }
                    // Unified long-running activity strip (TODO-005), pinned full window
                    // WIDTH at the bottom just above the footer (relocated here from the
                    // export rail 2026-07-11 to match the Electron shell, where the strip
                    // spans the whole window under the columns). It renders nothing when
                    // idle, so the footer sits directly below the columns until work starts.
                    ActivityStripView()
                    FooterBar()
                        .layoutExtra()
                }
            }
        }
        .id(model.layoutRenderEpoch)
        // While the switch animates: the incoming arrangement at its final size,
        // anchored top-left (the moving window edge reveals / covers it).
        .frame(width: pinned?.width, height: pinned?.height, alignment: .topLeading)
        // Per-arrangement limits — with the scene's `.windowResizability(.contentSize)`
        // these are the window's own resize limits: compact width 340–420, height
        // free (min 560) — §6.2; full min 640 × 400. Lifted while animating.
        .frame(minWidth: rootMinSize.width,
               maxWidth: model.layoutAnimating ? .infinity : (model.layoutMode == .compact ? AppModel.compactMaxWidth : nil),
               minHeight: rootMinSize.height,
               maxHeight: model.layoutAnimating ? .infinity : nil,
               alignment: .topLeading)
        // Hands the hosting NSWindow to the model once (frame + clamps per arrangement).
        .background(WindowAccessor { model.attachWindow($0) })
        .background(Theme.windowBg)
        .overlay(ToastOverlay(center: model.toasts))
        // Keep the model's UndoManager current (it is per-window and can be
        // replaced by the scene; `initial: true` covers first appearance).
        .onChange(of: undoManager, initial: true) { _, new in model.undoManager = new }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            Task { await ingest(providers) }
            return true
        }
        // Pinned-replacement confirmation (TODO-013, D9): a manual audio drop onto a
        // PINNED couple (a manual pick, from EITHER drop target) arms this; replacing
        // the source discards that deliberate choice, clears its export status and
        // its current pair session, so it asks first. An included-but-unpinned couple
        // replaces without asking. Presented once here at the shared ancestor so
        // both gestures show the identical dialog. Confirm ⇒ replace; Cancel ⇒ no
        // change. The dialog states the consequence explicitly.
        .confirmationDialog(
            "Replace the audio you chose?",
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
            Text("You chose the audio for \(pending.videoName) yourself. Pairing it with \(pending.audioName) replaces that choice, clears its export status, and rebuilds the current pair session.")
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
        // the import screen, so it always confirms with the media/couple/included counts.
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

    private var rootMinSize: CGSize {
        if model.layoutAnimating {
            return CGSize(width: AppModel.compactMinSize.width, height: AppModel.fullMinSize.height)
        }
        return model.layoutMode == .compact ? AppModel.compactMinSize : AppModel.fullMinSize
    }

    /// Consequence-focused copy for the per-file removal dialog (TODO-001): how many
    /// couples are affected and how many of those were included in the export (D7).
    private func removalMessage(_ p: AppModel.PendingMediaRemoval) -> String {
        let couples = "\(p.affectedCouples) couple\(p.affectedCouples == 1 ? "" : "s")"
        var msg = "This affects \(couples). Affected video rows re-resolve over the remaining files; a removed video's row is removed. "
        if p.affectedIncluded > 0 {
            msg += "\(p.affectedIncluded) of them \(p.affectedIncluded == 1 ? "is" : "are") included in the export and will leave it. "
        }
        msg += "This removes the files from this session only — your source files are not deleted."
        return msg
    }

    /// Consequence-focused copy for the Clear Session dialog (TODO-001).
    private func clearMessage(_ p: AppModel.PendingClearSession) -> String {
        let media = "\(p.media) media file\(p.media == 1 ? "" : "s")"
        let couples = "\(p.couples) couple\(p.couples == 1 ? "" : "s")"
        let included = "\(p.included) included in the export"
        return "This discards \(media), \(couples) (\(included)) and returns to the import screen. Your source files and any files you have already exported are not deleted."
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

// MARK: - Unified activity strip (TODO-005)

/// The global activity strip, spanning the FULL window width just above the footer
/// (TODO-005; relocated from the export rail 2026-07-11 to mirror the Electron shell,
/// where the strip runs the whole width under the columns). Renders
/// `model.activity.summary`: nothing when nil (idle, or work younger than the ~250 ms
/// debounce), an ERROR line (no bar) for a failed operation, a DETERMINATE bar when
/// the summary carries a trustworthy fraction, and an INDETERMINATE bar otherwise.
/// Never shows a fabricated percentage — that honesty is enforced upstream in the
/// pure `ActivitySummaryPolicy`; this view only draws what it is handed.
struct ActivityStripView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let summary = model.activity.summary {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 5) {
                    if summary.isError {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.bad)
                    }
                    Text(summary.text)
                        .font(.system(size: 11))
                        .foregroundStyle(summary.isError ? Theme.bad : Theme.textSecondary)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 0)
                }
                // A bar only when there is progress to show; an error line carries no
                // bar (TODO-005: failure is an actionable line, not a stuck bar). The
                // linear bar fills the strip's full width — the whole point of the move.
                if !summary.isError {
                    if let fraction = summary.fraction {
                        ProgressView(value: fraction)   // determinate (trustworthy %)
                            .progressViewStyle(.linear)
                            .controlSize(.mini)
                            .tint(Theme.accent)
                    } else {
                        ProgressView()                  // indeterminate (text-only honesty)
                            .progressViewStyle(.linear)
                            .controlSize(.mini)
                            .tint(Theme.accent)
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            // Top hairline separating the strip from the columns above, mirroring the
            // Electron strip's `border-top: 1px solid var(--border)`.
            .overlay(alignment: .top) {
                Rectangle().fill(Theme.panelStroke).frame(height: 1)
            }
            .transition(.opacity)
        }
    }
}

// MARK: - Footer cheat line (v2 keys)

struct FooterBar: View {
    var body: some View {
        HStack(spacing: 6) {
            Text("j/k move · ⏎/⚙ sync tools · l/◫ loupes · a camera/external · b both · esc collapse     ←/→ offset 1 ms · ⇧ 10 · ⌥ 0.1 · [ ] drift · drag loupe = offset · ⇧-drag = pivot")
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
