// Empty/launch + import-in-progress states (UX §3): a single load surface with
// two slots, drag-drop anywhere, per-slot progress with cancellation. Roles come
// from probe (video stream ⇒ camera), not from which slot was targeted.

import CoupleKit
import SwiftUI
import UniformTypeIdentifiers

struct LoadSurface: View {
    @Environment(AppModel.self) private var model
    @State private var importerShown = false
    @State private var dropHover = false

    var body: some View {
        let pair = model.pair
        VStack(spacing: 20) {
            HStack {
                Text("COUPLE")
                    .font(.system(size: 13, weight: .bold, design: .rounded))
                    .kerning(3)
                    .foregroundStyle(Theme.textPrimary)
                Spacer()
            }
            .padding(.top, 24) // below traffic lights
            .padding(.leading, 62)
            Spacer()
            Text("Drop a video and an external recording")
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
            Text("Roles are detected automatically — the file with a video stream becomes the camera track.")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)

            HStack(spacing: 16) {
                SlotCard(kind: .camera, state: pair.camera)
                SlotCard(kind: .external, state: pair.external)
            }
            .frame(maxWidth: 760)

            Button("Choose files…") {
                importerShown = true
            }
            .buttonStyle(.bordered)

            if let err = pair.loadError {
                Text(err)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.bad)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .strokeBorder(dropHover ? Theme.accent : Theme.panelStroke,
                              style: StrokeStyle(lineWidth: 1.5, dash: [6, 4]))
                .padding(14)
        )
        .onDrop(of: [.fileURL], isTargeted: $dropHover) { providers in
            for provider in providers {
                _ = provider.loadObject(ofClass: URL.self) { url, _ in
                    if let url {
                        Task { @MainActor in
                            model.pair.loadFile(url)
                        }
                    }
                }
            }
            return true
        }
        .fileImporter(isPresented: $importerShown,
                      allowedContentTypes: [.movie, .audio, .audiovisualContent],
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result {
                model.pair.handleDropped(urls: urls)
            }
        }
    }
}

private struct SlotCard: View {
    @Environment(AppModel.self) private var model
    let kind: SlotKind
    let state: SlotState

    private var color: Color { kind == .camera ? Theme.camera : Theme.external }
    private var title: String { kind == .camera ? "Camera / video" : "External recorder" }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Circle().fill(color).frame(width: 8, height: 8)
                Text(title)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Theme.textSecondary)
                Spacer()
            }
            switch state {
            case .empty:
                Text("waiting for file…")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
            case .probing(let name):
                Text("\(name) — probing…")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textSecondary)
            case .importing(let name, let phase, let fraction):
                Text(name)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                HStack(spacing: 8) {
                    ProgressView(value: fraction)
                        .tint(color)
                    Button("Cancel") {
                        model.pair.cancelImport(kind)
                    }
                    .buttonStyle(.plain)
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
                }
                Text(phase == "Decode" ? "decoding audio…" : phase == "Pyramid" ? "building peaks…" : "finishing…")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
            case .loaded(let track):
                Text(URL(fileURLWithPath: track.meta.info.path).lastPathComponent)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Text("\(Fmt.hmmss(track.durationS)) · ready")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(Theme.good)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 96, alignment: .topLeading)
        .background(Theme.panelBg, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(
            state.track != nil ? color.opacity(0.4) : Theme.panelStroke))
    }
}
