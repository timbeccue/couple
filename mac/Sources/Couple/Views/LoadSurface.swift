// Empty/launch state (§2.1, revised 2026-07-11): the center column's import
// surface, shown inside the full three-column shell — the app never hides its
// layout behind a splash screen. Accepts any number of files and/or folders;
// everything is probed and classified into the media pool (video stream ⇒ video
// role, audio-only ⇒ audio); non-media is skipped silently and surfaced once.
// Drop anywhere in the window, or choose files.

import CoupleKit
import SwiftUI
import UniformTypeIdentifiers

struct LoadSurface: View {
    @Environment(AppModel.self) private var model
    @State private var importerShown = false
    @State private var dropHover = false

    var body: some View {
        VStack(spacing: 16) {
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

            // Orientation for first-time users (the rails around this surface are
            // visible but empty): where results land and what happens after.
            Text("Matched pairs appear in the list on the left. Good matches are included in the export; untick any you don't want, then export from the right rail.")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textTertiary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)

            // A plain spinner (NO percentage bar) only while a drop is actively importing.
            // The full import→match→sync progress bar lives in the export rail, not here;
            // gating on `pool.importing` also keeps the empty import surface from ever
            // showing a stale bar carried over from a discarded session.
            if model.pool.importing {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Importing…")
                        .font(.system(size: 11)).foregroundStyle(Theme.textSecondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(dropHover ? Theme.accent : Theme.panelStroke,
                              style: StrokeStyle(lineWidth: 1.5, dash: [6, 4]))
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
