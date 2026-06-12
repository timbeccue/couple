// Workflow strip (UX §1.2 band 2): Input | Sync | Export, intrinsic height,
// content capped at ~1340 pt and centered when wider.

import CoupleKit
import SwiftUI

struct WorkflowStrip: View {
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            InputPanel()
                .frame(width: 222)
            SyncPanel()
                .frame(minWidth: 500, maxWidth: .infinity)
            ExportPanel()
                .frame(width: 230)
        }
        .frame(maxWidth: 1340)
        .frame(maxWidth: .infinity) // center when wider
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
    }
}

// MARK: - Input panel (UX §1.3)

struct InputPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        VStack(alignment: .leading, spacing: 8) {
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
        }
        .panel()
    }

    private func sectionLabel(_ s: String) -> some View {
        Text(s)
            .font(.system(size: 9, weight: .bold))
            .kerning(1.2)
            .foregroundStyle(Theme.textTertiary)
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

// MARK: - Sync panel (UX §1.4)

struct SyncPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        HStack(alignment: .top, spacing: 12) {
            previewPlaceholder
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top, spacing: 14) {
                    offsetGroup
                    driftGroup
                    Spacer(minLength: 8)
                    autoSyncGroup
                }
                SparklineView()
                    .frame(height: 46)
            }
        }
        .panel()
        .disabled(!pair.inputsEnabled && !pair.isSyncRunning)
    }

    /// Video preview is Phase 6 — honest placeholder with the per-anchor jump
    /// buttons it will carry (UX §1.4).
    private var previewPlaceholder: some View {
        let pair = model.pair
        return VStack(spacing: 6) {
            ZStack {
                RoundedRectangle(cornerRadius: 6)
                    .fill(Theme.canvasBg)
                VStack(spacing: 4) {
                    Image(systemName: "video")
                        .foregroundStyle(Theme.textTertiary)
                    Text("video preview · Phase 6")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            .frame(width: 200, height: 112) // 16:9, intrinsic — never letterboxed filler
            HStack(spacing: 6) {
                Image(systemName: "play.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textTertiary)
                Text("0:00.00")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                Spacer()
                ForEach(pair.anchors.indices, id: \.self) { i in
                    Circle()
                        .fill(Theme.anchorColors[i % 4].opacity(0.35))
                        .frame(width: 14, height: 14)
                        .overlay(Text("\(i + 1)").font(.system(size: 8, weight: .bold))
                            .foregroundStyle(.black.opacity(0.7)))
                }
            }
            .frame(width: 200)
        }
    }

    private var offsetGroup: some View {
        let pair = model.pair
        return VStack(alignment: .leading, spacing: 4) {
            Text("OFFSET")
                .font(.system(size: 9, weight: .bold))
                .kerning(1.2)
                .foregroundStyle(Theme.textTertiary)
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                NumericField(value: pair.offsetMs, decimals: 2, width: 110) {
                    pair.commitTypedOffset($0)
                }
                Text("ms")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
            }
            Text(abs(pair.offsetMs) >= 1000 ? Fmt.humanOffset(ms: pair.offsetMs) : " ")
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
            nudgeRow(steps: [-10, -1, 1, 10], format: { $0 > 0 ? "+\(Int($0))" : "\(Int($0))" }) {
                pair.nudgeOffset($0)
            }
        }
    }

    private var driftGroup: some View {
        let pair = model.pair
        let disabled = pair.driftMode == .none
        return VStack(alignment: .leading, spacing: 4) {
            Text("DRIFT")
                .font(.system(size: 9, weight: .bold))
                .kerning(1.2)
                .foregroundStyle(Theme.textTertiary)
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                NumericField(value: pair.driftPpm, decimals: 2, width: 80) {
                    pair.commitTypedDrift($0)
                }
                Text("ppm")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
            }
            Text(driftSubline)
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
            nudgeRow(steps: [-1, -0.1, 0.1, 1], format: { $0 > 0 ? "+\($0)" : "\($0)" }) {
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

    private func nudgeRow(steps: [Double], format: @escaping (Double) -> String, action: @escaping (Double) -> Void) -> some View {
        HStack(spacing: 3) {
            ForEach(steps, id: \.self) { step in
                Button(format(step)) {
                    action(step)
                }
                .buttonStyle(.plain)
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.textSecondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 3)
                .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 4))
            }
        }
    }

    private var autoSyncGroup: some View {
        let pair = model.pair
        return VStack(alignment: .trailing, spacing: 5) {
            Button {
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
                .font(.system(size: 13, weight: .semibold))
                .frame(width: 150, height: 30)
            }
            .buttonStyle(.borderedProminent)
            .tint(Theme.accent)

            fitStatusLine
        }
    }

    @ViewBuilder
    private var fitStatusLine: some View {
        let pair = model.pair
        if case .failed(let message) = pair.syncActivity {
            Text(message)
                .font(.system(size: 9))
                .foregroundStyle(Theme.bad)
                .lineLimit(2)
                .frame(width: 150, alignment: .trailing)
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
}

// MARK: - Export panel (export action is Phase 6; Verified gate + drift-mode
// control live here because their semantics drive the live mapping, UX §1.5)

struct ExportPanel: View {
    @Environment(AppModel.self) private var model
    @State private var gateNote = false

    var body: some View {
        let pair = model.pair
        VStack(alignment: .leading, spacing: 8) {
            Text("EXPORT")
                .font(.system(size: 9, weight: .bold))
                .kerning(1.2)
                .foregroundStyle(Theme.textTertiary)

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

            Button {
                gateNote = true
            } label: {
                Text("Export muxed file")
                    .font(.system(size: 12, weight: .semibold))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 5)
            }
            .buttonStyle(.borderedProminent)
            .tint(pair.verified ? Theme.good.opacity(0.8) : Color.gray.opacity(0.3))
            .popover(isPresented: $gateNote) {
                Text(pair.verified
                     ? "Export lands in Phase 6 — use `couple export` meanwhile."
                     : "Verify the alignment first (v): check the loupes, then toggle Verified.")
                    .font(.system(size: 11))
                    .padding(10)
                    .frame(width: 220)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("DRIFT CORRECTION")
                    .font(.system(size: 8, weight: .bold))
                    .kerning(1)
                    .foregroundStyle(Theme.textTertiary)
                Picker("", selection: Binding(
                    get: { pair.driftMode },
                    set: { pair.setDriftMode($0) }
                )) {
                    Text("Resample").tag(DriftMode.resample)
                    Text("None").tag(DriftMode.none)
                }
                .pickerStyle(.segmented)
                .disabled(!pair.inputsEnabled)
                Text(pair.driftMode == .resample
                     ? "corrects drift, resamples audio"
                     : "audio untouched (offset only)")
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textTertiary)
            }

            summaryGrid
        }
        .panel()
    }

    private var summaryGrid: some View {
        let pair = model.pair
        let driftText: String = {
            if pair.driftMode == .none {
                return pair.savedDriftPpm != 0
                    ? String(format: "%.1f ms uncorrected", pair.endToEndDriftMs)
                    : "—"
            }
            return String(format: "%.2f ppm", pair.driftPpm)
        }()
        let fitText = pair.fitStatus.map { String(format: "%.2f ms", $0.rmsMs) } ?? "—"
        let overlapText: String = {
            if case .overlap(let f, _) = pair.overlapChip {
                return "\(Int((f * 100).rounded()))%"
            }
            return "—"
        }()
        return Grid(alignment: .leading, horizontalSpacing: 8, verticalSpacing: 2) {
            summaryRow("offset", String(format: "%+.2f ms", pair.offsetMs))
            summaryRow("drift", driftText)
            summaryRow("fit RMS", fitText)
            summaryRow("overlap", overlapText)
        }
    }

    private func summaryRow(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k)
                .font(.system(size: 9))
                .foregroundStyle(Theme.textTertiary)
            Text(v)
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.textSecondary)
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
            .font(.system(size: 17, weight: .semibold, design: .monospaced))
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
