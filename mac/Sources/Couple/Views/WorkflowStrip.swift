// Workflow strip: four uniform-height panels — Input | Video preview |
// Controls | Export — running from the window top down to the waveforms.
// This band (not the stage) absorbs extra window height, with the video
// preview growing to use it (owner redesign 2026-06-12, supersedes the
// UX §1.2 stage-greedy policy; SPEC-CHANGE logged in PROGRESS.md).

import AVFoundation
import CoupleKit
import SwiftUI

struct WorkflowStrip: View {
    var body: some View {
        GeometryReader { geo in
            HStack(alignment: .top, spacing: 10) {
                InputPanel()
                    .frame(width: 222)
                VideoPreviewPanel()
                    .frame(width: previewWidth(for: geo.size))
                ControlsPanel()
                    .frame(minWidth: 380, maxWidth: .infinity)
                ExportPanel()
                    .frame(width: 240)
            }
            .frame(maxWidth: 1340)
            .frame(maxWidth: .infinity)
            .frame(height: geo.size.height)
        }
        .padding(.horizontal, 14)
        .padding(.top, 26) // traffic-light clearance (hidden title bar)
        .padding(.bottom, 8)
    }

    /// 16:9 preview width derived from the strip height — extra window height
    /// goes primarily to the video preview — but never so wide that the other
    /// panels fall below their minimums.
    private func previewWidth(for size: CGSize) -> CGFloat {
        let byHeight: CGFloat = (size.height - 52) * 16 / 9
        let otherPanels: CGFloat = 222 + 380 + 240 + 30
        let byWidth: CGFloat = min(size.width, 1340) - otherPanels
        return min(max(min(byHeight, byWidth), 220), 560)
    }
}

private func sectionLabel(_ s: String) -> some View {
    Text(s)
        .font(.system(size: 9, weight: .bold))
        .kerning(1.2)
        .foregroundStyle(Theme.textTertiary)
}

// MARK: - Input panel (UX §1.3) — carries the wordmark now that the title row is gone

struct InputPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        VStack(alignment: .leading, spacing: 8) {
            Text("COUPLE")
                .font(.system(size: 13, weight: .bold, design: .rounded))
                .kerning(3)
                .foregroundStyle(Theme.textPrimary)
                .padding(.bottom, 2)
            sectionLabel("INPUT")
            fileRow(color: Theme.camera, track: pair.camera.track)
            fileRow(color: Theme.external, track: pair.external.track)
            Divider().overlay(Theme.panelStroke)
            sectionLabel("SESSION")
            HStack(spacing: 6) {
                Text(pair.pairName)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                Text("\(model.pairsOpened) of \(model.pairsOpened)")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(Theme.chipBg, in: Capsule())
            }
            Button {
                model.requestNewPair()
            } label: {
                HStack {
                    Image(systemName: "plus")
                        .font(.system(size: 9))
                    Text("add pair")
                        .font(.system(size: 11))
                }
                .foregroundStyle(Theme.textSecondary)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 5)
                .overlay(RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(Theme.textTertiary, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            }
            .buttonStyle(.plain)
            Spacer(minLength: 0)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .panel()
    }

    @ViewBuilder
    private func fileRow(color: Color, track: TrackHandle?) -> some View {
        HStack(spacing: 7) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(track.map { URL(fileURLWithPath: $0.meta.info.path).lastPathComponent } ?? "—")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer()
            Text(track.map { Fmt.hmmss($0.durationS) } ?? "")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textSecondary)
        }
    }
}

// MARK: - Video preview + transport (UX §1.4 / §11.4)

struct VideoPreviewPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        let audition = pair.audition
        VStack(spacing: 8) {
            PlayerLayerView(player: audition.player)
                .aspectRatio(16.0 / 9.0, contentMode: .fit)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Theme.canvasBg, in: RoundedRectangle(cornerRadius: 6))
                .clipShape(RoundedRectangle(cornerRadius: 6))

            HStack(spacing: 8) {
                Button {
                    let start = max(0, (pair.anchors.first ?? 0.8) - 0.8)
                    audition.toggle(defaultStart: start)
                } label: {
                    Image(systemName: audition.isPlaying ? "pause.fill" : "play.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textPrimary)
                        .frame(width: 22, height: 18)
                        .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 4))
                }
                .buttonStyle(.plain)
                .disabled(pair.session == nil)
                Text(timecode(audition.playheadT))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(audition.isPlaying ? Theme.textPrimary : Theme.textTertiary)
                if audition.audioUnavailable {
                    Text("audio output unavailable")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.warn)
                }
                Spacer()
                ForEach(pair.anchors.indices, id: \.self) { i in
                    Button {
                        audition.jump(toAnchorAt: pair.anchors[i], index: i)
                    } label: {
                        Circle()
                            .fill(Theme.anchorColors[i % 4]
                                .opacity(audition.focusedAnchor == i ? 0.9 : 0.45))
                            .frame(width: 15, height: 15)
                            .overlay(Text("\(i + 1)").font(.system(size: 8, weight: .bold))
                                .foregroundStyle(.black.opacity(0.7)))
                    }
                    .buttonStyle(.plain)
                    .help("Play from anchor \(i + 1) (−0.8 s)")
                }
            }

            Picker("", selection: Binding(
                get: { audition.monitorMode },
                set: { audition.monitorMode = $0 }
            )) {
                ForEach(MonitorMode.allCases, id: \.self) { m in
                    Text(m.label).tag(m)
                }
            }
            .pickerStyle(.segmented)
            .controlSize(.small)
            .disabled(pair.session == nil)
            .help("Null: tune for minimum residual — with different mics it's a relative dip, never silence")
        }
        .frame(maxHeight: .infinity)
        .panel()
    }

    private func timecode(_ t: Double) -> String {
        let s = max(0, t)
        return String(format: "%d:%05.2f", Int(s) / 60, s.truncatingRemainder(dividingBy: 60))
    }
}

/// AVPlayerLayer host (video rendering is shell-side, SPEC §4.7 boundary).
private struct PlayerLayerView: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> PlayerNSView {
        let v = PlayerNSView()
        v.playerLayer.player = player
        v.playerLayer.videoGravity = .resizeAspect
        return v
    }

    func updateNSView(_ nsView: PlayerNSView, context: Context) {
        if nsView.playerLayer.player !== player {
            nsView.playerLayer.player = player
        }
    }

    final class PlayerNSView: NSView {
        let playerLayer = AVPlayerLayer()

        init() {
            super.init(frame: .zero)
            wantsLayer = true
            layer = playerLayer
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) {
            fatalError()
        }
    }
}

// MARK: - Controls panel (UX §1.4, redesigned 2026-06-12: Auto-sync is the
// hero, numeric readouts demoted, sparkline near-square)

struct ControlsPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        VStack(alignment: .leading, spacing: 8) {
            autoSyncButton
            fitStatusLine
                .frame(maxWidth: .infinity, alignment: .center)
            Divider().overlay(Theme.panelStroke)
            ViewThatFits(in: .horizontal) {
                // Wide: readouts beside the (near-square) sparkline.
                HStack(alignment: .top, spacing: 16) {
                    VStack(alignment: .leading, spacing: 14) {
                        offsetGroup
                        driftGroup
                    }
                    Spacer(minLength: 8)
                    sparklineGroup
                }
                // Narrow: sparkline drops below the readouts.
                VStack(alignment: .leading, spacing: 12) {
                    offsetGroup
                    driftGroup
                    sparklineGroup
                }
            }
            Spacer(minLength: 0)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .panel()
        .disabled(!pair.inputsEnabled && !pair.isSyncRunning)
    }

    private var autoSyncButton: some View {
        let pair = model.pair
        return Button {
            if pair.isSyncRunning {
                pair.cancelSync()
            } else {
                pair.runAutoSync()
            }
        } label: {
            Group {
                if case .running(let label, _) = pair.syncActivity {
                    Text(label)
                } else {
                    Text("Auto-sync")
                }
            }
            .font(.system(size: 14, weight: .semibold))
            .frame(maxWidth: .infinity)
            .frame(height: 34)
        }
        .buttonStyle(.borderedProminent)
        .tint(Theme.accent)
    }

    @ViewBuilder
    private var fitStatusLine: some View {
        let pair = model.pair
        if case .failed(let message) = pair.syncActivity {
            Text(message)
                .font(.system(size: 9))
                .foregroundStyle(Theme.bad)
                .lineLimit(2)
        } else if let fit = pair.fitStatus {
            Text(String(format: "fit RMS %.2f ms · %d windows", fit.rmsMs, fit.windows))
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.rmsColor(fit.rmsMs))
        } else {
            Text("not measured")
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
        }
    }

    private var sparklineGroup: some View {
        VStack(alignment: .leading, spacing: 3) {
            sectionLabel("DRIFT FIT")
            SparklineView()
                .frame(width: 190, height: 132)
        }
    }

    private var offsetGroup: some View {
        let pair = model.pair
        return VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                sectionLabel("OFFSET")
                NumericField(value: pair.offsetMs, decimals: 2, width: 92) {
                    pair.commitTypedOffset($0)
                }
                Text("ms")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
                Text(abs(pair.offsetMs) >= 1000 ? Fmt.humanOffset(ms: pair.offsetMs) : "")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            nudgeRow(steps: [-10, -1, 1, 10], unit: "ms",
                     format: { $0 > 0 ? "+\(Int($0))" : "\(Int($0))" }) {
                pair.nudgeOffset($0)
            }
        }
    }

    private var driftGroup: some View {
        let pair = model.pair
        let disabled = pair.driftMode == .none
        return VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                sectionLabel("DRIFT")
                NumericField(value: pair.driftPpm, decimals: 2, width: 70) {
                    pair.commitTypedDrift($0)
                }
                Text("ppm")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
                Text(driftSubline)
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            nudgeRow(steps: [-1, -0.1, 0.1, 1], unit: "ppm",
                     format: { $0 > 0 ? "+\($0)" : "\($0)" }) {
                pair.nudgeDrift($0)
            }
        }
        .opacity(disabled ? 0.45 : 1) // visually disabled in None mode (UX §1.4)
    }

    private var driftSubline: String {
        let pair = model.pair
        if pair.driftMode == .none {
            return pair.savedDriftPpm != 0
                ? String(format: "off — measured %.2f ppm", pair.savedDriftPpm)
                : "off"
        }
        return String(format: "≈ %.1f ms over overlap", pair.endToEndDriftMs)
    }

    private func nudgeRow(steps: [Double], unit: String,
                          format: @escaping (Double) -> String,
                          action: @escaping (Double) -> Void) -> some View {
        HStack(spacing: 4) {
            ForEach(steps, id: \.self) { step in
                Button(format(step)) {
                    action(step)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .font(.system(size: 10, design: .monospaced))
            }
            Text(unit)
                .font(.system(size: 9))
                .foregroundStyle(Theme.textTertiary)
        }
    }
}

// MARK: - Export panel (export action is Phase 6; Verified gate, drift-mode
// control and file-type options live here)

struct ExportPanel: View {
    @Environment(AppModel.self) private var model
    @State private var gateNote = false

    var body: some View {
        let pair = model.pair
        @Bindable var pairB = pair
        VStack(alignment: .leading, spacing: 9) {
            sectionLabel("EXPORT")

            Button {
                pair.toggleVerified()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: pair.verified ? "checkmark.square.fill" : "square")
                        .foregroundStyle(pair.verified ? Theme.good : Theme.textSecondary)
                    Text("Verified")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Text("v")
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(Theme.textTertiary)
                }
                .padding(.vertical, 4)
                .padding(.horizontal, 6)
                .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 6))
            }
            .buttonStyle(.plain)

            VStack(alignment: .leading, spacing: 4) {
                sectionLabel("FORMAT")
                Picker("", selection: $pairB.exportFormat) {
                    ForEach(PairSession.ExportFormat.allCases, id: \.self) { f in
                        Text(f.rawValue).tag(f)
                    }
                }
                .pickerStyle(.segmented)
                .controlSize(.small)
                Text(formatHelp)
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textTertiary)
                Toggle(isOn: $pairB.keepCameraAudio) {
                    Text("keep camera audio as 2nd track")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textSecondary)
                }
                .toggleStyle(.checkbox)
                .controlSize(.small)
                .disabled(pair.exportFormat == .wav)
            }

            VStack(alignment: .leading, spacing: 4) {
                sectionLabel("DRIFT CORRECTION")
                Picker("", selection: Binding(
                    get: { pair.driftMode },
                    set: { pair.setDriftMode($0) }
                )) {
                    Text("Resample").tag(DriftMode.resample)
                    Text("None").tag(DriftMode.none)
                }
                .pickerStyle(.segmented)
                .controlSize(.small)
                .disabled(!pair.inputsEnabled)
                Text(pair.driftMode == .resample
                     ? "corrects drift, resamples audio"
                     : "audio untouched (offset only)")
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textTertiary)
            }

            if let cam = pair.camera.track {
                HStack(spacing: 5) {
                    sectionLabel("OUTPUT")
                    Text("\(Fmt.hmmss(cam.durationS)) · video timeline")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(Theme.textSecondary)
                }
            }

            exportButton
            Spacer(minLength: 0)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .panel()
    }

    @ViewBuilder
    private var exportButton: some View {
        let pair = model.pair
        if case .running(let phase, let fraction) = pair.exportActivity {
            VStack(spacing: 4) {
                ProgressView(value: fraction)
                    .tint(Theme.good)
                HStack {
                    Text(phase == "ExportMux" ? "muxing…" : "rendering audio…")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.textSecondary)
                    Spacer()
                    Button("Cancel") {
                        pair.cancelExport()
                    }
                    .buttonStyle(.plain)
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textSecondary)
                }
            }
        } else {
            Button {
                if pair.verified {
                    runSavePanel()
                } else {
                    gateNote = true // clicking while unverified explains the gate (UX §1.5)
                }
            } label: {
                Text("Export")
                    .font(.system(size: 12, weight: .semibold))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 5)
            }
            .buttonStyle(.borderedProminent)
            .tint(pair.verified ? Theme.good.opacity(0.8) : Color.gray.opacity(0.3))
            .popover(isPresented: $gateNote) {
                Text("Verify the alignment first (v): check the loupes, audition if in doubt, then toggle Verified.")
                    .font(.system(size: 11))
                    .padding(10)
                    .frame(width: 220)
            }
        }
    }

    private func runSavePanel() {
        let pair = model.pair
        let panel = NSSavePanel()
        panel.nameFieldStringValue = pair.suggestedExportFilename
        panel.canCreateDirectories = true
        if let window = NSApp.keyWindow {
            panel.beginSheetModal(for: window) { response in
                guard response == .OK, let url = panel.url else { return }
                Task { @MainActor in
                    pair.runExport(to: url)
                }
            }
        }
    }

    private var formatHelp: String {
        switch model.pair.exportFormat {
        case .auto: return "matches the camera container"
        case .mov: return "QuickTime, PCM audio"
        case .mp4: return "MP4, AAC 256 kbps audio"
        case .wav: return "aligned audio only, no video"
        }
    }
}

// MARK: - Numeric field (commit on Enter/blur; focus suppresses global keys)

struct NumericField: View {
    let value: Double
    let decimals: Int
    let width: CGFloat
    let commit: (Double) -> Void

    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        TextField("", text: $text)
            .textFieldStyle(.plain)
            .font(.system(size: 13, weight: .semibold, design: .monospaced))
            .foregroundStyle(Theme.textPrimary)
            .multilineTextAlignment(.trailing)
            .frame(width: width)
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(Theme.canvasBg, in: RoundedRectangle(cornerRadius: 5))
            .focused($focused)
            .onSubmit { commitText() }
            .onChange(of: focused) { _, isFocused in
                if !isFocused { commitText() }
            }
            .onChange(of: value) { _, v in
                if !focused { text = format(v) }
            }
            .onAppear { text = format(value) }
    }

    private func format(_ v: Double) -> String {
        String(format: "%+.\(decimals)f", v)
    }

    private func commitText() {
        let cleaned = text.replacingOccurrences(of: "−", with: "-")
            .replacingOccurrences(of: "+", with: "")
            .trimmingCharacters(in: .whitespaces)
        if let v = Double(cleaned) {
            commit(v)
        }
        text = format(value)
    }
}
