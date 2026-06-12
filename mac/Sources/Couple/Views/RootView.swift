// Four-band window (UX §1.2, DECIDED): title bar / workflow strip / waveform
// stage (the only greedy region) / footer. Below ~1000×560 the window scrolls
// rather than crushing the waveforms.

import CoupleKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        GeometryReader { geo in
            let needsScroll = geo.size.width < 1000 || geo.size.height < 560
            Group {
                if needsScroll {
                    ScrollView([.horizontal, .vertical]) {
                        content
                            .frame(width: max(1000, geo.size.width),
                                   height: max(560, geo.size.height))
                    }
                } else {
                    content
                }
            }
        }
        .background(Theme.windowBg)
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
            TitleBar()
            if pair.session == nil {
                LoadSurface()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                WorkflowStrip()
                WaveformStage()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            FooterBar()
        }
    }
}

// MARK: - Title bar (UX §1.2 band 1)

struct TitleBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        HStack(spacing: 12) {
            Spacer().frame(width: 58) // traffic-light clearance (hidden title bar)
            Text("COUPLE")
                .font(.system(size: 13, weight: .bold, design: .rounded))
                .kerning(3)
                .foregroundStyle(Theme.textPrimary)

            overlapChip

            Spacer()

            Picker("", selection: Binding(
                get: { pair.loupeLayout },
                set: { pair.setLoupeLayout($0) }
            )) {
                Text("Side-by-side").tag(LoupeLayout.grid)
                Text("Stacked").tag(LoupeLayout.stack)
            }
            .pickerStyle(.segmented)
            .frame(width: 190)
            .disabled(pair.session == nil)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private var overlapChip: some View {
        let chip = model.pair.overlapChip
        let (text, color): (String, Color) = {
            switch chip {
            case .noAlignment: return ("no alignment", Theme.warn)
            case .noOverlap: return ("no overlap", Theme.warn)
            case .overlap(let f, let s):
                return ("overlap \(Int((f * 100).rounded()))% · \(Fmt.hmmss(s))", Theme.good)
            }
        }()
        Text(text)
            .font(.system(size: 11, weight: .medium, design: .monospaced))
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(color.opacity(0.12), in: Capsule())
    }
}

// MARK: - Footer (UX §1.2 band 4)

struct FooterBar: View {
    var body: some View {
        HStack(spacing: 6) {
            Text("←/→ offset 1 ms · ⇧ 10 ms · ⌥ 0.1 ms     [ ] drift 1 ppm · ⌥ 0.1 ppm     drag loupe = offset · ⇧-drag = pivot drift · scroll = zoom     v verify")
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
