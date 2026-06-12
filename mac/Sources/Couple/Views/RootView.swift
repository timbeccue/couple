// Window bands (owner redesign 2026-06-12): workflow strip (greedy — extra
// height grows the video preview) / waveform stage (bounded) / footer. Below
// ~1000×640 the window scrolls rather than crushing content.

import CoupleKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        GeometryReader { geo in
            let needsScroll = geo.size.width < 1000 || geo.size.height < 640
            Group {
                if needsScroll {
                    ScrollView([.horizontal, .vertical]) {
                        content
                            .frame(width: max(1000, geo.size.width),
                                   height: max(640, geo.size.height))
                    }
                } else {
                    content
                }
            }
        }
        .background(Theme.windowBg)
        .overlay(ToastOverlay(center: model.toasts))
        .confirmationDialog(
            "Replace \(model.pair.pendingReplace?.role == .camera ? "camera" : "external") file?",
            isPresented: Binding(
                get: { model.pair.pendingReplace != nil },
                set: { if !$0 { model.pair.pendingReplace = nil } }
            )
        ) {
            if let pending = model.pair.pendingReplace {
                Button("Replace with \(pending.filename)", role: .destructive) {
                    model.pair.confirmReplace(pending)
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The current alignment will be discarded.")
        }
        .confirmationDialog(
            "Open a new pair?",
            isPresented: Binding(
                get: { model.confirmingNewPair },
                set: { model.confirmingNewPair = $0 }
            )
        ) {
            Button("Discard current pair", role: .destructive) {
                model.startNewPair()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The current pair's alignment has not been exported. Opening a new pair replaces it.")
        }
    }

    @ViewBuilder
    private var content: some View {
        let pair = model.pair
        VStack(spacing: 0) {
            if pair.session == nil {
                LoadSurface()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                WorkflowStrip()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                WaveformStage()
            }
            FooterBar()
        }
    }
}

// MARK: - Footer

struct FooterBar: View {
    var body: some View {
        HStack(spacing: 6) {
            Text("←/→ move external 1 ms · ⇧ 10 ms · ⌥ 0.1 ms     [ ] drift 1 ppm · ⌥ 0.1 ppm     drag loupe = offset · ⇧-drag = pivot drift · scroll = zoom     v verify")
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
