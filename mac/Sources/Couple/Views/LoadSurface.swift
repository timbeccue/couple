// Empty/launch state (§2.1): a single drop surface for a whole session — any
// number of files and/or folders. Everything is probed and classified into the
// media pool (video stream ⇒ video role, audio-only ⇒ audio); non-media is
// skipped silently and surfaced once. Drop anywhere, or choose files.

import CoupleKit
import SwiftUI
import UniformTypeIdentifiers

struct LoadSurface: View {
    @Environment(AppModel.self) private var model
    @State private var importerShown = false
    @State private var dropHover = false

    var body: some View {
        VStack(spacing: 18) {
            HStack {
                Text("COUPLE")
                    .font(.system(size: 13, weight: .bold, design: .rounded))
                    .kerning(3)
                    .foregroundStyle(Theme.textPrimary)
                Spacer()
            }
            .padding(.top, 24)
            .padding(.leading, 62) // below traffic lights
            Spacer()
            Image(systemName: "tray.and.arrow.down")
                .font(.system(size: 30))
                .foregroundStyle(Theme.textTertiary)
            Text("Drop a recording session")
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(Theme.textPrimary)
            Text("Any number of videos and audio files, or a whole folder. Couple finds the matching pairs automatically.")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)

            Button("Choose files…") { importerShown = true }
                .buttonStyle(.bordered)

            if model.pool.importing {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(model.pool.importProgress)
                        .font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                }
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
            Task { await ingest(providers) }
            return true
        }
        .fileImporter(isPresented: $importerShown,
                      allowedContentTypes: [.movie, .audio, .audiovisualContent, .folder],
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { model.importDropped(urls) }
        }
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
