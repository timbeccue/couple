// Root layout (§V2.1): three columns with no modal screens — everything visible
// at once. Empty session ⇒ the load surface; otherwise the queue rail (left),
// the video-hero center, and the export rail (right), with both side rails
// drag-resizable (HSplitView) and the center absorbing the slack. The whole
// window stays a drop target (§2.1).

import CoupleKit
import SwiftUI
import UniformTypeIdentifiers

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            Group {
                if !model.hasContent {
                    LoadSurface()
                } else {
                    HSplitView {
                        QueueRail()
                            .frame(minWidth: 248, idealWidth: 304, maxWidth: 440)
                        CenterColumn()
                            .frame(minWidth: 460)
                            .layoutPriority(1)
                        ExportRail()
                            .frame(minWidth: 224, idealWidth: 280, maxWidth: 400)
                    }
                }
            }
            FooterBar()
        }
        .background(Theme.windowBg)
        .overlay(ToastOverlay(center: model.toasts))
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            Task { await ingest(providers) }
            return true
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

// MARK: - Footer cheat line (v2 keys)

struct FooterBar: View {
    var body: some View {
        HStack(spacing: 6) {
            Text("j/k move · v approve (=queue) · ⏎/⚙ sync tools · l/◫ loupes · esc collapse     ←/→ offset 1 ms · ⇧ 10 · ⌥ 0.1 · [ ] drift · drag loupe = offset · ⇧-drag = pivot")
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
