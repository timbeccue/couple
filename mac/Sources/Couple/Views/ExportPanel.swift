// Export panel (UX §V2.12 / §V2.8 / §6; PLAN-COMPACT-MODE Phase 4.1): INTO
// (destination path + Change…; auto-tracks the last import folder until pinned),
// AS (Auto/MOV/MP4/WAV, one global format), "keep camera audio as a muted track"
// on its own line (unavailable for WAV), and "Export N files" (N = INCLUDED
// couples, D1; disabled at 0) running serial jobs with disk preflight and
// continue-past-failure (BatchExport; the finished toast carries Reveal in
// Finder). One view for BOTH arrangements: pinned at the bottom of the compact
// arrangement, and the top of the full window's export rail (above Files).

import AppKit
import CoupleKit
import SwiftUI

struct ExportPanel: View {
    @Environment(AppModel.self) private var model

    /// Width of the INTO / AS label column so the two values line up.
    private static let labelWidth: CGFloat = 30

    var body: some View {
        @Bindable var m = model
        VStack(alignment: .leading, spacing: 10) {
            // INTO — the destination path (mono, ellipsized) + Change…
            HStack(spacing: 6) {
                sectionLabel("INTO").frame(width: Self.labelWidth, alignment: .leading)
                Text(destinationText)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(model.exportDestination == nil ? Theme.textTertiary
                                                                    : Theme.textSecondary)
                    .lineLimit(1).truncationMode(.middle)
                    .help(model.exportDestination?.path ?? "No destination folder yet")
                Spacer(minLength: 4)
                Button { chooseDestination() } label: {
                    Text(model.exportDestination == nil ? "Choose…" : "Change…")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.accent)
                }
                .buttonStyle(.plain)
            }

            // AS — one global format for the whole batch
            HStack(spacing: 6) {
                sectionLabel("AS").frame(width: Self.labelWidth, alignment: .leading)
                Picker("", selection: $m.globalFormat) {
                    ForEach(PairSession.ExportFormat.allCases, id: \.self) { f in
                        Text(f.rawValue).tag(f)
                    }
                }
                .labelsHidden()
                .pickerStyle(.segmented).controlSize(.small)
                .fixedSize()
                Spacer(minLength: 0)
            }

            Toggle(isOn: $m.globalKeepCameraAudio) {
                Text("keep camera audio as a muted track")
                    .font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
            }
            .toggleStyle(.checkbox).controlSize(.small)
            .disabled(model.globalFormat == .wav)
            .help(model.globalFormat == .wav ? "Not available for WAV (audio only)" : "")

            exportButton
        }
    }

    /// The destination as a home-abbreviated path ("~/Movies/bach session").
    private var destinationText: String {
        guard let url = model.exportDestination else { return "no folder chosen" }
        return (url.path as NSString).abbreviatingWithTildeInPath
    }

    @ViewBuilder
    private var exportButton: some View {
        switch model.batch.phase {
        case .running(let done, let total):
            VStack(spacing: 4) {
                ProgressView(value: Double(done), total: Double(max(total, 1))).tint(Theme.good)
                Text("Exporting \(done)/\(total)…")
                    .font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
            }
        case .preflight:
            Text("Checking disk…").font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
        default:
            let n = model.includedCount
            Button { model.batch.run() } label: {
                Text(n == 0 ? "Export" : "Export \(n) file\(n == 1 ? "" : "s")")
                    .font(.system(size: 13, weight: .semibold))
                    .frame(maxWidth: .infinity).padding(.vertical, 8)
            }
            .buttonStyle(.borderedProminent).tint(Theme.good.opacity(0.85))
            .disabled(n == 0 || model.exportDestination == nil)
        }
    }

    private func chooseDestination() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        if let window = NSApp.keyWindow {
            panel.beginSheetModal(for: window) { resp in
                guard resp == .OK, let url = panel.url else { return }
                Task { @MainActor in
                    model.chooseExportDestination(url)
                }
            }
        }
    }
}
