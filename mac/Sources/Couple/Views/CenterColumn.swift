// Center column (§V2.1): the video hero (greedy), a resolve banner for couples
// that need a human choice (§V2.6), the waveform stage (narrowed overview sharing
// its row with the sync-tools panel; loupes summoned on demand, §V2.4), and the
// persistent bottom tool bar (⚙ sync tools / ◫ loupes — same-place
// toggles, nothing repositions). Everything is visible at once; there is no
// "editor screen" you navigate to — ⏎ / ⚙ just expands the sync tools in place.

import CoupleKit
import SwiftUI

/// Pure center-column layout math, kept out of the SwiftUI `View` so it emits a
/// stable, linkable symbol the layout fixture (`DebugDrive.layoutTest`) can call.
enum CenterLayout {
    /// Responsive cap (pt) for the resolve banner's *candidate* scroll region
    /// (TODO-006). Pure + deterministic so the layout fixture can assert it.
    ///
    /// The banner's title/reason and the candidate list live inside the greedy
    /// center stack, *above* the fixed WaveformRow (124) + BottomBar (~52) that
    /// must stay reachable. We therefore let candidates take only a modest
    /// fraction of the column and clamp it: the floor keeps ~3 rows visible even
    /// on a tiny window; the ceiling stops a huge list from dwarfing the video
    /// on a tall one. `containerHeight == 0` (first layout pass) uses the floor.
    static func candidateScrollCap(containerHeight: CGFloat) -> CGFloat {
        let floor: CGFloat = 96   // ~3 candidate rows — never less, even at min height
        let ceiling: CGFloat = 240
        guard containerHeight > 0 else { return floor }
        return min(ceiling, max(floor, containerHeight * 0.28))
    }
}

struct CenterColumn: View {
    @Environment(AppModel.self) private var model

    /// Live center-column height, captured so the resolve banner's candidate
    /// scroll cap can shrink at small windows (TODO-006) and keep the waveform
    /// row + bottom toolbar reachable at the minimum supported height.
    @State private var containerHeight: CGFloat = 0

    /// True while a valid audio drag hovers the center workspace (TODO-013), driving
    /// the highlight + copy overlay. Only armed when a couple is selected (the drop is
    /// scoped to the selected couple — no couple ⇒ no target).
    @State private var dropTargeted = false

    var body: some View {
        content
            .padding(.horizontal, 14)
            .padding(.top, 26) // traffic-light clearance
            .padding(.bottom, 8)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(GeometryReader { geo in
                Color.clear.onAppear { containerHeight = geo.size.height }
                    .onChange(of: geo.size.height) { _, h in containerHeight = h }
            })
            // ONE coherent audio drop target spanning the whole central content
            // region — video preview, resolve banner, waveform row, and sync-tools
            // area (TODO-013, §V2.6). Scoped to the selected couple: with none
            // selected there is no couple to attach the modifier to, so the region is
            // inert. Routes through the SHARED `requestRepairDrop` (the same path the
            // queue-row drop uses) so the two gestures produce identical results —
            // no-op on the current source, immediate replace when unpinned, confirm
            // before replacing a PINNED couple's pick. A non-audio payload / an
            // unavailable video is rejected there without changing state.
            .modifier(CenterDropTarget(
                couple: model.selectedCouple,
                isTargeted: $dropTargeted,
                draggingName: model.poolDraggingName))
    }

    /// The center stack: the shared preview block (PLAN-COMPACT-MODE 4.1 —
    /// title line, greedy video hero with the PairStrip, status line, inline
    /// candidate banner), then the full-only editing surfaces.
    private var content: some View {
        VStack(spacing: 8) {
            PairPreview(candidateMaxHeight: CenterLayout.candidateScrollCap(containerHeight: containerHeight))
                .layoutBlock(.preview)

            if model.selectedCouple?.pair.session != nil {
                if model.loupesShown {
                    LoupeArea()
                        .frame(height: model.pair.loupeLayout == .grid ? 220 : loupeStackHeight)
                        .padding(.horizontal, 14)
                        .layoutExtra()
                }
                WaveformRow()
                    .frame(height: 124)
                    .layoutExtra()
            }

            BottomBar()
                .layoutExtra()
        }
    }

    private var loupeStackHeight: CGFloat {
        let n = model.pair.anchors.count
        return CGFloat(n) * 96 + CGFloat(max(n - 1, 0)) * 8
    }
}

// MARK: - Center-workspace drop target (TODO-013, §V2.6)

/// Makes the selected couple's central content region one coherent AUDIO drop
/// target (TODO-013): drag an audio file from the media pool anywhere over the video
/// preview / resolve banner / waveform row / sync-tools area to re-pair the take.
/// During a valid drag it highlights the region in the app's accent language (a soft
/// accent wash + an accent stroke, NOT a yellow box) and shows a chip reading
/// `Pair selected video with <audio>` using the dragged file's display name. The
/// drop itself goes through `AppModel.requestRepairDrop` — the SAME shared path the
/// queue-row target uses — so the two gestures cannot diverge.
///
/// Scoped to the selected couple: with `couple == nil` the modifier attaches no drop
/// destination at all, so the region is inert (no highlight, no target) — satisfying
/// "no selected couple rejects the drop without changing state". The repair op's own
/// guards reject a non-audio payload or an unavailable video.
struct CenterDropTarget: ViewModifier {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow?
    @Binding var isTargeted: Bool
    /// The dragged audio's display name (for the live copy), or nil when unknown.
    let draggingName: String?

    func body(content: Content) -> some View {
        if let couple {
            content
                .overlay { if isTargeted { highlight } }
                .dropDestination(for: String.self) { items, _ in
                    isTargeted = false
                    model.poolDraggingKey = nil
                    guard let key = items.first else { return false }
                    return model.requestRepairDrop(couple, audioKey: key)
                } isTargeted: { hovering in
                    isTargeted = hovering
                }
        } else {
            content // no selected couple ⇒ no drop target (inert region)
        }
    }

    /// The valid-drop highlight: an accent wash + stroke (the app's selection/accent
    /// language, consistent with the queue-row drop affordance) and a centered chip
    /// naming the action. Reuses the panel/chip vocabulary; never a yellow box.
    private var highlight: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12)
                .fill(Theme.accent.opacity(0.10))
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(Theme.accent.opacity(0.7), lineWidth: 2)
            HStack(spacing: 8) {
                Image(systemName: "waveform.badge.plus")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                Text(copy)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
            .background(Theme.panelBg.opacity(0.95), in: Capsule())
            .overlay(Capsule().strokeBorder(Theme.accent.opacity(0.45)))
        }
        .allowsHitTesting(false) // the drop destination underneath owns the hit-test
        .padding(2)
    }

    /// `Pair selected video with <audio>.wav` (TODO-013), using the dragged file's
    /// display name when known; a graceful fallback when the live name is unavailable.
    private var copy: String {
        if let name = draggingName { return "Pair selected video with \(name)" }
        return "Pair selected video with this audio"
    }
}

// MARK: - Overview row (narrowed overview + fit status / sparkline / re-sync)

private struct WaveformRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        HStack(spacing: 10) {
            OverviewStrip()
                .frame(maxWidth: .infinity)
            VStack(alignment: .leading, spacing: 4) {
                fitStatusLine(pair)
                SparklineView()
                    .frame(width: 150, height: 56)
                Button {
                    pair.runAutoSync()
                } label: {
                    Label("re-sync", systemImage: "arrow.clockwise")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.textSecondary)
                }
                .buttonStyle(.bordered).controlSize(.small)
                .disabled(!pair.inputsEnabled)
                .help("Re-run auto-sync — never re-proposes anchors or re-frames a view you've seen")
            }
            .frame(width: 158)
        }
    }

    /// Fit status; clicking it summons the loupes (§V2.4). Surfaces the raw fit
    /// RMS so the owner can calibrate (v1's thresholds don't transfer — V2-F2).
    @ViewBuilder
    private func fitStatusLine(_ pair: PairSession) -> some View {
        Button {
            model.loupesShown = true
        } label: {
            Group {
                if case .running(let label, _) = pair.syncActivity {
                    Text(label)
                } else if case .failed(let message) = pair.syncActivity {
                    Text(message).foregroundStyle(Theme.bad).lineLimit(2)
                } else if let fit = pair.fitStatus {
                    // accepted/attempted when the last sync's telemetry is at hand
                    // (state persists accepted windows only); offset-only pairs say
                    // so instead of implying a fitted drift (§5.5 slope guard).
                    let attempted = (pair.lastSync?.windows.count).map { "/\($0)" } ?? ""
                    let drift = pair.lastSync.map { $0.driftMeasurable ? "" : " · offset-only" } ?? ""
                    Text(String(format: "fit RMS %.2f ms · %d%@ win%@",
                                fit.rmsMs, fit.windows, attempted, drift))
                        .foregroundStyle(Theme.textSecondary)
                } else {
                    Text("not measured").foregroundStyle(Theme.textTertiary)
                }
            }
            .font(.system(size: 9, design: .monospaced))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Persistent bottom tool bar (§V2.4)

private struct BottomBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        let hasSession = model.selectedCouple?.pair.session != nil
        VStack(spacing: 6) {
            if model.syncToolsExpanded && hasSession {
                HStack(alignment: .top, spacing: 16) {
                    SyncReadouts()
                    driftModePicker(pair)
                    Spacer()
                }
            }
            HStack(spacing: 10) {
                toggle("sync tools", icon: "gearshape", on: model.syncToolsExpanded) {
                    model.syncToolsExpanded.toggle()
                }
                .disabled(!hasSession)
                toggle("loupes", icon: "rectangle.split.3x1", on: model.loupesShown) {
                    model.loupesShown.toggle()
                }
                .disabled(!hasSession)
                Spacer()
                // No Approve control (UX §V2.12 / D4): inclusion is the row checkbox.
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Theme.panelBg, in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.panelStroke))
    }

    private func toggle(_ label: String, icon: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(label, systemImage: icon)
                .font(.system(size: 11, weight: on ? .semibold : .regular))
                .foregroundStyle(on ? Theme.accent : Theme.textSecondary)
                .padding(.horizontal, 9).padding(.vertical, 5)
                .background(on ? Theme.accent.opacity(0.15) : Theme.chipBg,
                            in: RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    private func driftModePicker(_ pair: PairSession) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            sectionLabel("DRIFT MODE")
            Picker("", selection: Binding(
                get: { pair.driftMode },
                set: { pair.setDriftMode($0) }
            )) {
                Text("Resample").tag(DriftMode.resample)
                Text("None").tag(DriftMode.none)
            }
            .pickerStyle(.segmented).controlSize(.mini).frame(width: 150)
            .disabled(!pair.inputsEnabled)
        }
    }

}
